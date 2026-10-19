//! File browsing and transfer over dedicated bi-streams.
//!
//! The host serves its filesystem to an authenticated viewer (when file
//! transfer is allowed): listings, downloads, uploads into any folder. The
//! viewer uses [`RemoteFs`]. See `StreamOpen` in the protocol.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use slipstream_core::{
    framing::{expect_msg, write_msg},
    proto::{DirListing, FilePreview, Place, StreamOpen, StreamReply, UploadTarget},
};
use tokio::io::AsyncReadExt;

use crate::fsops;

const CHUNK: usize = 256 * 1024;

/// What a peer may do on our side.
#[derive(Debug, Clone, Copy)]
pub struct FilePolicy {
    /// List folders and download files.
    pub browse: bool,
    /// Receive uploads.
    pub receive: bool,
    /// Forward TCP connections (`StreamOpen::Tunnel`).
    pub tunnels: bool,
    pub network_scan: bool,
    pub remote_input: bool,
}

/// Serves file requests arriving on `conn` until it closes. `on_saved` is
/// told about each received file.
pub async fn serve(
    conn: Connection,
    policy: FilePolicy,
    on_saved: Arc<dyn Fn(PathBuf) + Send + Sync>,
    clipboard: Option<slipstream_platform::clipboard::Clipboard>,
) {
    let scans = Arc::new(tokio::sync::Semaphore::new(1));
    let mut requests = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = requests.join_next(), if !requests.is_empty() => {},
            incoming = conn.accept_bi(), if requests.len() < 64 => {
                let Ok((send,recv)) = incoming else {break};
                let (on_saved, clipboard, scans) = (on_saved.clone(), clipboard.clone(), scans.clone());
                requests.spawn(async move {
                    if let Err(e) = handle(send, recv, policy, on_saved, clipboard, scans).await {
                        tracing::debug!("file request failed: {e:#}");
                    }
                });
            }
            _ = conn.closed() => break,
        }
    }
    // JoinSet drops/aborts requests if the session ends, including uploads
    // and their staging files, rather than detaching them from the owner.
}

async fn reply(send: &mut SendStream, r: &StreamReply) -> Result<()> {
    write_msg(send, r).await
}

async fn handle(
    mut send: SendStream,
    mut recv: RecvStream,
    policy: FilePolicy,
    on_saved: Arc<dyn Fn(PathBuf) + Send + Sync>,
    clipboard: Option<slipstream_platform::clipboard::Clipboard>,
    scans: Arc<tokio::sync::Semaphore>,
) -> Result<()> {
    let req: StreamOpen = tokio::time::timeout(std::time::Duration::from_secs(10), expect_msg(&mut recv)).await.context("file request header timed out")??;
    let denied = |what: &str| StreamReply::Error(format!("{what} is not allowed on this computer"));
    match req {
        StreamOpen::FilePreview { path } => {
            if !policy.browse { return reply(&mut send, &denied("Browsing files")).await; }
            // Ordinary unreadable/unsupported files keep their generic icon.
            tokio::select! {
                preview = crate::preview::load(path) => {
                    reply(&mut send, &StreamReply::FilePreview(preview.ok().flatten())).await?;
                }
                _ = send.stopped() => {},
            }
        }
        StreamOpen::PublicIp { refresh } => {
            let response = match crate::public_ip::get(refresh).await {
                Ok(address) => StreamReply::PublicIp(address),
                Err(error) => StreamReply::Error(error),
            };
            reply(&mut send, &response).await?;
        }
        StreamOpen::OpenNetworkWeb { address, port } => {
            if !policy.network_scan || !policy.remote_input {
                return reply(&mut send, &denied("Opening the remote browser (network scan and remote control)")).await;
            }
            let result = tokio::task::spawn_blocking(move || crate::desktop_shell::open_web(address, port)).await?;
            let response = match result {
                Ok(()) => StreamReply::Done,
                Err(e) => StreamReply::Error(format!("Could not open the remote browser: {e:#}")),
            };
            reply(&mut send, &response).await?;
        }
        StreamOpen::DropTarget { x, y } => {
            if !policy.receive { return reply(&mut send, &denied("Receiving files")).await; }
            let result = tokio::task::spawn_blocking(move || crate::desktop_shell::drop_folder(x, y)).await?;
            let response = match result {
                Ok(path) => StreamReply::DropTarget { path: fsops::display(&path) },
                Err(e) => StreamReply::Error(format!("Cannot choose the drop folder: {e:#}")),
            };
            reply(&mut send, &response).await?;
        }
        StreamOpen::NetworkNames { interface, addresses } => {
            if !policy.network_scan { return reply(&mut send, &denied("Network scanning")).await; }
            let interfaces = tokio::task::spawn_blocking(slipstream_platform::machine::network_interfaces).await?;
            let stopped = send.stopped();
            tokio::select! {
                result = crate::network_names::lookup_batch(interface, addresses, &interfaces) => {
                    let response = match result {
                        Ok(names) => StreamReply::NetworkNames(names),
                        Err(e) => StreamReply::Error(e.to_string()),
                    };
                    reply(&mut send, &response).await?;
                },
                _ = stopped => {},
            }
        }
        StreamOpen::NetworkInterfaces => {
            if !policy.network_scan { return reply(&mut send, &denied("Network scanning")).await; }
            let interfaces = tokio::task::spawn_blocking(slipstream_platform::machine::network_interfaces).await?;
            reply(&mut send, &StreamReply::NetworkInterfaces(interfaces)).await?;
        }
        StreamOpen::NetworkScan(request) => {
            if !policy.network_scan { return reply(&mut send, &denied("Network scanning")).await; }
            let Ok(_permit) = scans.try_acquire_owned() else {
                return reply(&mut send, &StreamReply::Error("A network scan is already running in this session".into())).await;
            };
            let interfaces = tokio::task::spawn_blocking(slipstream_platform::machine::network_interfaces).await?;
            // Resolves on STOP_SENDING or connection close; dropping the
            // survey drops its JoinSets and cancels outstanding async probes.
            let stopped = send.stopped();
            tokio::select! {
                r = crate::network::serve(&mut send, request, interfaces) => r?,
                _ = stopped => {},
            }
        }
        StreamOpen::ClipboardFiles { batch } => {
            let r = match &clipboard {
                Some(cb) if policy.receive => {
                    let dir = fsops::paste_dir(batch);
                    let items: Vec<PathBuf> =
                        std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
                    cb.set_files(items);
                    StreamReply::Done
                }
                _ => denied("Pasting files"),
            };
            reply(&mut send, &r).await?;
        }
        StreamOpen::Tunnel { host, port } => {
            if !policy.tunnels {
                return reply(&mut send, &denied("Port forwarding")).await;
            }
            let hostname = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(&host);
            let target = if hostname.contains(':') {format!("[{hostname}]:{port}")} else {format!("{hostname}:{port}")};
            match tokio::time::timeout(std::time::Duration::from_secs(10), tokio::net::TcpStream::connect((hostname, port)))
                .await
            {
                Ok(Ok(tcp)) => {
                    tracing::info!(%target, "tunnel opened");
                    reply(&mut send, &StreamReply::Ready).await?;
                    crate::tunnel::splice(tcp, send, recv).await;
                }
                Ok(Err(e)) => {
                    reply(&mut send, &StreamReply::Error(format!("can't reach {target} from that computer: {e}")))
                        .await?
                }
                Err(_) => reply(&mut send, &StreamReply::Error(format!("{target} didn't answer"))).await?,
            }
        }
        // What the computer is: anyone allowed in may see it.
        StreamOpen::SystemInfo => {
            let info = tokio::task::spawn_blocking(slipstream_platform::machine::collect).await?;
            reply(&mut send, &StreamReply::SystemInfo(info)).await?;
        }
        StreamOpen::Pace => {
            // The host's version first (no figures), so the viewer's log
            // says which build these figures come from.
            let version = slipstream_core::proto::PaceStage { name: format!("Slipstream {}", slipstream_core::VERSION_STRING), fps: 0.0, ms: 0.0 };
            let stages = std::iter::once(version)
                .chain(slipstream_platform::pace::live().into_iter().map(|(name, fps, ms)| slipstream_core::proto::PaceStage { name, fps, ms }))
                .collect();
            reply(&mut send, &StreamReply::Pace(stages)).await?;
        }
        StreamOpen::Places => {
            let r = if policy.browse { StreamReply::Places(fsops::places()) } else { denied("Browsing files") };
            reply(&mut send, &r).await?;
        }
        StreamOpen::List { path } => {
            let r = if !policy.browse {
                denied("Browsing files")
            } else {
                match tokio::select! {
                    listing = fsops::list_async(path) => listing,
                    _ = send.stopped() => return Ok(()),
                } {
                    Ok(l) => StreamReply::Listing(l),
                    Err(e) => StreamReply::Error(format!("{e:#}")),
                }
            };
            reply(&mut send, &r).await?;
        }
        StreamOpen::MakeDir { path } => {
            let r = if !policy.browse {
                denied("Changing files")
            } else {
                match tokio::fs::create_dir_all(&path).await {
                    Ok(()) => StreamReply::Done,
                    Err(e) => StreamReply::Error(format!("cannot create {path}: {e}")),
                }
            };
            reply(&mut send, &r).await?;
        }
        StreamOpen::Download { path } => {
            if !policy.browse {
                reply(&mut send, &denied("Downloading files")).await?;
                return Ok(());
            }
            let (mut file, size) = match open_regular(Path::new(&path)).await {
                Ok(f) => f,
                Err(e) => {
                    reply(&mut send, &StreamReply::Error(format!("cannot open {path}: {e}"))).await?;
                    return Ok(());
                }
            };
            reply(&mut send, &StreamReply::File { size }).await?;
            let mut buf = vec![0u8; CHUNK];
            let mut remaining = size;
            while remaining > 0 {
                let want = remaining.min(CHUNK as u64) as usize;
                let n = file.read(&mut buf[..want]).await?;
                anyhow::ensure!(n > 0, "source file became shorter during download");
                send.write_all(&buf[..n]).await?;
                remaining -= n as u64;
            }
            send.finish()?;
        }
        StreamOpen::Upload { target, rel_path, size } => {
            if !policy.receive {
                reply(&mut send, &denied("Receiving files")).await?;
                return Ok(());
            }
            let rel = match fsops::safe_relative(&rel_path) {
                Ok(r) => r,
                Err(e) => {
                    reply(&mut send, &StreamReply::Error(format!("{e:#}"))).await?;
                    return Ok(());
                }
            };
            let base = match &target {
                UploadTarget::Default => fsops::default_upload_dir(),
                UploadTarget::Folder(f) => PathBuf::from(f),
                UploadTarget::UnderCursor => {
                    crate::desktop_shell::folder_under_cursor().unwrap_or_else(fsops::default_upload_dir)
                }
                UploadTarget::Clipboard { batch } => {
                    let dir = fsops::paste_dir(*batch);
                    let _ = tokio::fs::create_dir_all(&dir).await;
                    dir
                }
            };
            if !base.is_dir() {
                reply(&mut send, &StreamReply::Error(format!("folder {} does not exist", base.display()))).await?;
                return Ok(());
            }
            let requested = base.join(&rel);
            reply(&mut send, &StreamReply::Ready).await?;
            let dest = match crate::incoming::receive(&mut recv, &requested, size, |_| {}).await {
                Ok(path) => path,
                Err(error) => {
                    reply(&mut send, &StreamReply::Error(format!("Could not receive {rel_path}: {error:#}"))).await?;
                    return Ok(());
                }
            };
            reply(&mut send, &StreamReply::Saved { path: fsops::display(&dest) }).await?;
            send.finish().ok();
            on_saved(dest);
        }
    }
    Ok(())
}

/// Refuse devices/FIFOs before opening them, and check the opened handle too.
async fn open_regular(path: &Path) -> Result<(tokio::fs::File, u64)> {
    anyhow::ensure!(tokio::fs::metadata(path).await?.is_file(), "{} is not a regular file", path.display());
    let file = tokio::fs::File::open(path).await.with_context(|| format!("opening {}", path.display()))?;
    let metadata = file.metadata().await?;
    anyhow::ensure!(metadata.is_file(), "{} is not a regular file", path.display());
    Ok((file, metadata.len()))
}

/// The remote computer's files, as seen from the viewer.
#[derive(Debug, Clone)]
pub struct RemoteFs {
    conn: Connection,
}

impl RemoteFs {
    /// Current transport state, also available to independently drawn tools
    /// while their parent window is minimized and its UI isn't running.
    pub fn is_connected(&self)->bool {self.conn.close_reason().is_none()}

    pub(crate) fn new(conn: Connection) -> Self {
        Self { conn }
    }

    pub(crate) async fn ask(&self, req: StreamOpen) -> Result<(SendStream, RecvStream, StreamReply)> {
        let (send, recv, reply) = self.ask_reply(req).await?;
        if let StreamReply::Error(e) = reply {
            bail!("{e}");
        }
        Ok((send, recv, reply))
    }

    /// Keep host errors separate from transport/unsupported-stream failures.
    pub(crate) async fn ask_reply(&self, req: StreamOpen) -> Result<(SendStream, RecvStream, StreamReply)> {
        let (mut send, mut recv) = self.conn.open_bi().await.context("connection closed")?;
        write_msg(&mut send, &req).await?;
        let reply: StreamReply = expect_msg(&mut recv).await?;
        Ok((send, recv, reply))
    }

    /// Freeze the destination before walking/uploading a dropped folder.
    /// This request is independent of the input stream: no cursor-move race.
    pub async fn drop_target(&self, x: i32, y: i32) -> Result<UploadTarget> {
        let response = tokio::time::timeout(std::time::Duration::from_secs(12), self.ask(StreamOpen::DropTarget { x, y }))
            .await.context("remote drop-folder lookup timed out")?
            .context("drop-folder lookup failed; update the remote app if it predates this feature")?.2;
        match response {
            StreamReply::DropTarget { path } => Ok(UploadTarget::Folder(path)),
            other => bail!("unexpected drop-folder reply {other:?}"),
        }
    }

    /// Optional appended RPC: older hosts keep browsing with generic icons.
    pub async fn preview(&self, path: String) -> Result<Option<FilePreview>> {
        let response = tokio::time::timeout(std::time::Duration::from_secs(12), async {
            let (mut send, mut recv) = self.conn.open_bi().await?;
            send.set_priority(-10)?;
            write_msg(&mut send, &StreamOpen::FilePreview { path }).await?;
            expect_msg::<StreamReply>(&mut recv).await
        }).await.context("file preview timed out")??;
        match response {
            StreamReply::FilePreview(preview) => {
                if preview.as_ref().is_some_and(|p| !p.valid()) { bail!("invalid file preview"); }
                Ok(preview)
            }
            StreamReply::Error(e) => bail!("{e}"),
            _ => bail!("unsupported file preview reply"),
        }
    }

    pub async fn places(&self) -> Result<Vec<Place>> {
        match self.ask(StreamOpen::Places).await?.2 {
            StreamReply::Places(p) => Ok(p),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    pub async fn list(&self, path: Option<String>) -> Result<DirListing> {
        match self.ask(StreamOpen::List { path }).await?.2 {
            StreamReply::Listing(l) => Ok(l),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    pub async fn make_dir(&self, path: String) -> Result<()> {
        self.ask(StreamOpen::MakeDir { path }).await.map(|_| ())
    }

    /// Uploads one local file. Returns where it was saved remotely.
    pub async fn upload(&self, local: &Path, target: UploadTarget, rel_path: String, mut progress: impl FnMut(u64)) -> Result<String> {
        let (mut file, size) = open_regular(local).await?;
        let (mut send, mut recv, reply) = self.ask(StreamOpen::Upload { target, rel_path, size }).await?;
        if !matches!(reply, StreamReply::Ready) {
            bail!("unexpected reply {reply:?}");
        }
        let mut remaining = size;
        let mut buf = vec![0u8; CHUNK];
        while remaining > 0 {
            let want = remaining.min(CHUNK as u64) as usize;
            let n = file.read(&mut buf[..want]).await?;
            anyhow::ensure!(n > 0, "source file became shorter during upload");
            send.write_all(&buf[..n]).await?;
            remaining -= n as u64;
            progress(n as u64);
        }
        send.finish()?;
        match expect_msg::<StreamReply>(&mut recv).await? {
            StreamReply::Saved { path } => Ok(path),
            StreamReply::Error(e) => bail!("{e}"),
            other => bail!("unexpected reply {other:?}"),
        }
    }

    /// Downloads one remote file to `dest` (a full local path; parents are
    /// created). Returns the local path actually used (made unique).
    pub async fn download(&self, remote: String, dest: &Path, mut progress: impl FnMut(u64)) -> Result<PathBuf> {
        let (_send, mut recv, reply) = self.ask(StreamOpen::Download { path: remote.clone() }).await?;
        let StreamReply::File { size } = reply else { bail!("unexpected reply {reply:?}") };
        crate::incoming::receive(&mut recv, dest, size, &mut progress)
            .await.with_context(|| format!("downloading {remote}"))
    }

    /// All files under a remote path, with paths relative to its parent
    /// (`Folder/sub/file`) and sizes. A file yields itself.
    pub async fn walk(&self, path: String, is_dir: bool, name: String, size: u64) -> Result<Vec<(String, String, u64)>> {
        if !is_dir {
            return Ok(vec![(path, name, size)]);
        }
        let mut out = Vec::new();
        let mut stack = vec![(path, name)];
        let mut visited = std::collections::HashSet::new();
        while let Some((dir, rel)) = stack.pop() {
            let listing = self.list(Some(dir)).await?;
            // Hosts return canonical listing paths, including symlink targets.
            if !visited.insert(listing.path.clone()) {continue;}
            for e in listing.entries {
                let child = format!("{}{}{}", listing.path.trim_end_matches(['/', '\\']), listing.sep, e.name);
                let child_rel = format!("{rel}/{}", e.name);
                if e.is_dir {
                    stack.push((child, child_rel));
                } else {
                    out.push((child, child_rel, e.size));
                }
            }
        }
        Ok(out)
    }
}
