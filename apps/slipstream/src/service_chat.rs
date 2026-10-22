//! Chat and one-time codes between the SYSTEM host and the signed-in user's
//! app. Neither is written to disk. The transport is a session-checked pipe;
//! the GUI authenticates its host against the protected machine identity.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Line {
    sequence: u64,
    ours: bool,
    text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Session {
    id: u64,
    peer: String,
    lines: Vec<Line>,
    #[serde(default)]
    status: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Snapshot {
    epoch: u64,
    sequence: u64,
    sessions: Vec<Session>,
    /// Sent only over the authenticated, session-checked local pipe.
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    call: Option<slipstream_session::meetings::calls::IncomingCall>,
    #[serde(default)]
    directory: Option<slipstream_session::DirStatus>,
    #[serde(default)]
    public_directory: Option<slipstream_session::DirStatus>,
}

impl Snapshot {
    fn message(&mut self, session: u64, ours: bool, text: String) {
        let Some(index) = self.sessions.iter().position(|s| s.id == session) else {
            return;
        };
        self.sequence += 1;
        self.sessions[index].lines.push(Line {
            sequence: self.sequence,
            ours,
            text: text.chars().take(4000).collect(),
        });
        while self.sessions.iter().map(|s| s.lines.len()).sum::<usize>() > 128 {
            let Some(oldest) = self
                .sessions
                .iter()
                .enumerate()
                .filter_map(|(i, s)| s.lines.first().map(|l| (i, l.sequence)))
                .min_by_key(|x| x.1)
                .map(|x| x.0)
            else {
                break;
            };
            self.sessions[oldest].lines.remove(0);
        }
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Action {
    AnswerCall { id: u64, accepted: bool },
    MeetingCalls { enabled: bool },
    Chat { session: u64, text: String },
    Disconnect { session: u64 },
    RegeneratePassword,
}
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Command {
    epoch: u64,
    action: Action,
}
#[cfg(windows)]
#[derive(Debug, Serialize, Deserialize)]
struct Challenge {
    nonce: [u8; 32],
    #[serde(default)]
    meeting_calls: bool,
}
#[cfg(windows)]
#[derive(Debug, Serialize, Deserialize)]
struct Proof {
    signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Directories { primary: Option<slipstream_session::DirStatus>, public: Option<slipstream_session::DirStatus> },
    MeetingCall(Option<slipstream_session::meetings::calls::IncomingCall>),
    Started {
        session: u64,
        peer: String,
    },
    Ended {
        session: u64,
    },
    Message {
        session: u64,
        ours: bool,
        text: String,
    },
    Password(Option<String>),
    Status { session: u64, text: String },
}

#[derive(Default)]
struct Cursor {
    call: Option<slipstream_session::meetings::calls::IncomingCall>,
    epoch: Option<u64>,
    sessions: BTreeMap<u64, u64>,
    password: Option<String>,
    statuses: BTreeMap<u64, String>,
    directory: Option<slipstream_session::DirStatus>,
    public_directory: Option<slipstream_session::DirStatus>,
}
impl Cursor {
    fn update(&mut self, snapshot: &Snapshot) -> Vec<Event> {
        let mut events = Vec::new();
        if self.directory != snapshot.directory || self.public_directory != snapshot.public_directory {
            self.directory = snapshot.directory.clone();
            self.public_directory = snapshot.public_directory.clone();
            events.push(Event::Directories { primary: self.directory.clone(), public: self.public_directory.clone() });
        }
        if self.call != snapshot.call || self.epoch != Some(snapshot.epoch) {
            if self.call.is_some() || snapshot.call.is_some() {
                events.push(Event::MeetingCall(snapshot.call.clone()));
            }
            self.call = snapshot.call.clone();
        }
        if self.password != snapshot.password {
            self.password = snapshot.password.clone();
            events.push(Event::Password(self.password.clone()));
        }
        let old = std::mem::take(&mut self.sessions);
        let previous_statuses = std::mem::take(&mut self.statuses);
        for &id in old.keys() {
            if self.epoch != Some(snapshot.epoch) || !snapshot.sessions.iter().any(|s| s.id == id) {
                events.push(Event::Ended { session: id });
            }
        }
        for session in &snapshot.sessions {
            let previous = if self.epoch == Some(snapshot.epoch) {
                old.get(&session.id).copied()
            } else {
                None
            };
            if previous.is_none() {
                events.push(Event::Started {
                    session: session.id,
                    peer: session.peer.clone(),
                });
            }
            if (!session.status.is_empty() || previous_statuses.get(&session.id).is_some_and(|s| !s.is_empty()))
                && (previous.is_none() || previous_statuses.get(&session.id) != Some(&session.status)) {
                events.push(Event::Status {session: session.id, text: session.status.clone()});
            }
            self.statuses.insert(session.id, session.status.clone());
            let mut seen = previous.unwrap_or(0);
            for line in &session.lines {
                if line.sequence > seen {
                    events.push(Event::Message {
                        session: session.id,
                        ours: line.ours,
                        text: line.text.clone(),
                    });
                    seen = line.sequence;
                }
            }
            self.sessions.insert(session.id, seen);
        }
        self.epoch = Some(snapshot.epoch);
        events
    }
}

#[cfg(any(windows, test))]
#[derive(Clone)]
pub struct Sender {
    tx: tokio::sync::mpsc::UnboundedSender<Command>,
    epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
    connected: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
#[cfg(any(windows, test))]
impl Sender {
    pub fn meeting_calls(&self, enabled: bool) {
        if self.connected() {
            let _ = self.tx.send(Command {
                epoch: self.epoch.load(std::sync::atomic::Ordering::Acquire),
                action: Action::MeetingCalls { enabled },
            });
        }
    }
    pub fn answer_call(&self, id: u64, accepted: bool) -> bool {
        self.connected()
            && self
                .tx
                .send(Command {
                    epoch: self.epoch.load(std::sync::atomic::Ordering::Acquire),
                    action: Action::AnswerCall { id, accepted },
                })
                .is_ok()
    }
    pub fn regenerate_password(&self) -> bool {
        self.connected() && self.tx.send(Command {
            epoch: self.epoch.load(std::sync::atomic::Ordering::Acquire),
            action: Action::RegeneratePassword,
        }).is_ok()
    }
    pub fn connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn chat(&self, session: u64, text: String) -> bool {
        self.connected()
            && self
                .tx
                .send(Command {
                    epoch: self.epoch.load(std::sync::atomic::Ordering::Acquire),
                    action: Action::Chat { session, text },
                })
                .is_ok()
    }
    pub fn disconnect(&self, session: u64) {
        if self.connected() {
            let _ = self.tx.send(Command {
                epoch: self.epoch.load(std::sync::atomic::Ordering::Acquire),
                action: Action::Disconnect { session },
            });
        }
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use anyhow::{Context, Result, bail};
    use slipstream_core::{
        config::Store,
        iroh::{PublicKey, SecretKey, Signature},
    };
    use slipstream_session::{Client, ClientEvent};
    use std::{
        os::windows::io::AsRawHandle,
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };
    use tokio::{
        io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
        net::windows::named_pipe::{NamedPipeClient, NamedPipeServer, ServerOptions},
        sync::{mpsc, watch},
        task::{JoinHandle, JoinSet},
    };
    use windows_sys::Win32::{
        Foundation::{INVALID_HANDLE_VALUE, LocalFree},
        Security::{Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, SECURITY_ATTRIBUTES},
        Storage::FileSystem::{CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT},
        System::{Pipes::GetNamedPipeClientSessionId, RemoteDesktop::ProcessIdToSessionId, Threading::GetCurrentProcessId},
    };

    const MAX_PACKET: usize = 4 * 1024 * 1024;
    pub fn session_id() -> u32 {
        let mut id = 0;
        unsafe {
            ProcessIdToSessionId(GetCurrentProcessId(), &mut id);
        };
        id
    }
    fn pipe_name(id: u32) -> String {
        format!(r"\\.\pipe\SlipstreamRemote.Chat.{id}")
    }
    fn key_path() -> PathBuf {
        crate::service::machine_home().join("public").join("chat-identity.txt")
    }
    fn challenge_message(nonce: &[u8; 32], session: u32) -> Vec<u8> {
        let mut message = b"slipstream local chat v1\0".to_vec();
        message.extend_from_slice(nonce);
        message.extend_from_slice(&session.to_le_bytes());
        message
    }

    async fn read<T: serde::de::DeserializeOwned>(r: &mut (impl AsyncRead + Unpin)) -> Result<T> {
        let n = r.read_u32_le().await? as usize;
        if n > MAX_PACKET {
            bail!("local chat message too large");
        }
        let mut bytes = vec![0; n];
        r.read_exact(&mut bytes).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }
    async fn write<T: Serialize>(w: &mut (impl AsyncWrite + Unpin), value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() > MAX_PACKET {
            bail!("local chat message too large");
        }
        w.write_u32_le(bytes.len() as u32).await?;
        w.write_all(&bytes).await?;
        Ok(())
    }

    fn create_pipe(session: u32, first: bool) -> Result<NamedPipeServer> {
        // Specific data/attribute rights exclude FILE_CREATE_PIPE_INSTANCE.
        // Interactive users may exchange chat only; SYSTEM/admins own the pipe.
        let sddl = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x00120083;;;IU)".encode_utf16().chain([0]).collect::<Vec<_>>();
        unsafe {
            let mut sd = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut sd, std::ptr::null_mut()) == 0 {
                bail!("chat pipe security: {}", std::io::Error::last_os_error());
            }
            let mut attrs = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd,
                bInheritHandle: 0,
            };
            let result = ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(pipe_name(session), (&mut attrs as *mut SECURITY_ATTRIBUTES).cast());
            LocalFree(sd);
            Ok(result?)
        }
    }

    fn open_pipe(session: u32) -> Result<NamedPipeClient> {
        let name = pipe_name(session).encode_utf16().chain([0]).collect::<Vec<_>>();
        // Identification prevents an untrusted server from impersonating the
        // client. The caller authenticates the real service before any commands.
        unsafe {
            let handle = CreateFileW(
                name.as_ptr(),
                0x00100083,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                std::ptr::null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(NamedPipeClient::from_raw_handle(handle)?)
        }
    }

    pub struct Server {
        custom_station: bool,
        broker: std::sync::Arc<slipstream_session::meetings::calls::Broker>,
        state: Arc<Mutex<Snapshot>>,
        changes: watch::Sender<Snapshot>,
        clients: Arc<AtomicUsize>,
        task: JoinHandle<()>,
        last_launch: Option<Instant>,
        session: u32,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    impl Server {
        pub fn start(client: Client) -> Result<Self> {
            let broker = client.call_broker();
            let session = session_id();
            let custom_station = client.has_custom_station();
            let pipe = create_pipe(session, true)?;
            let key = Store::open_default()?.identity()?;
            crate::service::ensure_public_dir();
            let path = key_path();
            let temp = path.with_extension("tmp");
            std::fs::write(&temp, key.public().to_string())?;
            std::fs::rename(temp, path)?;
            let initial = Snapshot {
                epoch: rand::random(),
                sequence: 0,
                sessions: Vec::new(),
                password: Some(client.one_time_password()),
                call: None,
                directory: Some(client.dir_status()),
                public_directory: Some(client.public_dir_status()),
            };
            let state = Arc::new(Mutex::new(initial.clone()));
            let (changes, _) = watch::channel(initial);
            let clients = Arc::new(AtomicUsize::new(0));
            let (s, tx, count) = (state.clone(), changes.clone(), clients.clone());
            let task = tokio::spawn(async move {
                let mut pipe = pipe;
                let mut jobs = JoinSet::new();
                loop {
                    tokio::select! {
                        result=pipe.connect()=>{if result.is_err(){break;}},
                        Some(_)=jobs.join_next(),if !jobs.is_empty()=>{continue;},
                    }
                    let mut peer_session = 0;
                    let allowed = unsafe { GetNamedPipeClientSessionId(pipe.as_raw_handle().cast(), &mut peer_session) != 0 && peer_session == session };
                    let next = match create_pipe(session, false) {
                        Ok(next) => next,
                        Err(e) => {
                            tracing::warn!("chat pipe: {e:#}");
                            break;
                        }
                    };
                    let connected = std::mem::replace(&mut pipe, next);
                    if !allowed || jobs.len() >= 8 {
                        drop(connected);
                        continue;
                    }
                    let (s, tx, count, key, client) = (s.clone(), tx.clone(), count.clone(), key.clone(), client.clone());
                    jobs.spawn(async move {
                        if let Err(e) = serve(connected, session, key, s, tx, count, client).await {
                            tracing::debug!("local chat disconnected: {e:#}");
                        }
                    });
                }
            });
            Ok(Self {
                broker,
                custom_station,
                state,
                changes,
                clients,
                task,
                last_launch: None,
                session,
            })
        }
        pub fn observe(&mut self, event: &ClientEvent) {
            let mut state = self.state.lock().expect("service chat state");
            match event {
                ClientEvent::CallChanged => state.call = self.broker.incoming(),
                ClientEvent::Directory(status) if self.custom_station => state.directory = Some(status.clone()),
                ClientEvent::PublicDirectory(status) => {
                    state.public_directory = Some(status.clone());
                    // The default directory follows the public station for raw apps.
                    if !self.custom_station { state.directory = Some(status.clone()); }
                },
                ClientEvent::Password(password) => state.password = Some(password.clone()),
                ClientEvent::HostSessionStarted { session, peer } => {
                    if state.sessions.len() >= 32 {
                        state.sessions.remove(0);
                    }
                    state.sessions.push(Session {
                        id: *session,
                        peer: peer.clone(),
                        lines: Vec::new(),
                        status: String::new(),
                    });
                }
                ClientEvent::HostSessionEnded { session, .. } => state.sessions.retain(|s| s.id != *session),
                ClientEvent::HostStatus { session, text } => {
                    if let Some(s) = state.sessions.iter_mut().find(|s| s.id == *session) {s.status = text.clone();}
                }
                ClientEvent::HostChat { session, text } => state.message(*session, false, text.clone()),
                _ => return,
            }
            self.changes.send_replace(state.clone());
            drop(state);
            if (matches!(event, ClientEvent::HostChat { .. }) || matches!(event, ClientEvent::HostStatus { text, .. } if !text.is_empty()))
                && self.clients.load(Ordering::Acquire) == 0
                && self.last_launch.is_none_or(|t| t.elapsed() > Duration::from_secs(10))
            {
                self.last_launch = Some(Instant::now());
                if let Err(e) = crate::service::launch_user_chat(self.session) {
                    tracing::warn!("opening host chat: {e:#}");
                }
            }
        }
    }

    async fn serve(
        mut pipe: NamedPipeServer,
        session: u32,
        key: SecretKey,
        state: Arc<Mutex<Snapshot>>,
        changes: watch::Sender<Snapshot>,
        clients: Arc<AtomicUsize>,
        client: Client,
    ) -> Result<()> {
        let hello: Challenge = tokio::time::timeout(Duration::from_secs(3), read(&mut pipe)).await??;
        let signature = key.sign(&challenge_message(&hello.nonce, session)).to_bytes().to_vec();
        write(&mut pipe, &Proof { signature }).await?;
        let _meeting_receiver = hello.meeting_calls.then(|| client.call_broker().receiver());
        clients.fetch_add(1, Ordering::AcqRel);
        struct Active(Arc<AtomicUsize>);
        impl Drop for Active {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::AcqRel);
            }
        }
        let _active = Active(clients);
        let (mut reader, mut writer) = tokio::io::split(pipe);
        let mut updates = changes.subscribe();
        let receive = async {
            loop {
                let command: Command = read(&mut reader).await?;
                let mut s = state.lock().expect("service chat state");
                if command.epoch != s.epoch {
                    continue;
                }
                match command.action {
                    Action::AnswerCall { id, accepted } => {
                        client.call_broker().answer(id, accepted)
                    }
                    Action::MeetingCalls { enabled } => {
                        client.update_settings(|s| s.app.block_meeting_calls = !enabled)
                    }
                    Action::Chat { session, text }
                        if s.sessions.iter().any(|s| s.id == session) =>
                    {
                        let text: String = text.chars().take(4000).collect();
                        client.host_chat(session, text.clone());
                        s.message(session, true, text);
                        changes.send_replace(s.clone());
                    }
                    Action::Disconnect { session } => client.disconnect_host_session(session),
                    Action::RegeneratePassword => { client.regenerate_password(); }
                    _ => {}
                }
            }
            #[allow(unreachable_code)]
            anyhow::Ok(())
        };
        let transmit = async {
            loop {
                let snapshot = updates.borrow_and_update().clone();
                write(&mut writer, &snapshot).await?;
                updates.changed().await?;
            }
            #[allow(unreachable_code)]
            anyhow::Ok(())
        };
        tokio::select! {r=receive=>r,r=transmit=>r}
    }

    pub struct Gui {
        pub events: mpsc::UnboundedReceiver<Event>,
        commands: mpsc::UnboundedSender<Command>,
        epoch: Arc<AtomicU64>,
        connected: Arc<AtomicBool>,
        task: JoinHandle<()>,
    }
    impl Drop for Gui {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    impl Gui {
        pub fn sender(&self) -> Sender {
            Sender {
                tx: self.commands.clone(),
                epoch: self.epoch.clone(),
                connected: self.connected.clone(),
            }
        }
        pub fn start(rt: &tokio::runtime::Runtime, ctx: eframe::egui::Context) -> Self {
            let (tx, events) = mpsc::unbounded_channel();
            let (commands, mut rx) = mpsc::unbounded_channel();
            let epoch = Arc::new(AtomicU64::new(0));
            let connected = Arc::new(AtomicBool::new(false));
            let (e, c) = (epoch.clone(), connected.clone());
            let task = rt.spawn(async move {
                let session = session_id();
                let mut cursor = Cursor::default();
                loop {
                    let result = async {
                        let public: PublicKey = std::fs::read_to_string(key_path())?.trim().parse()?;
                        let mut pipe = open_pipe(session)?;
                        let nonce = rand::random();
                        write(
                            &mut pipe,
                            &Challenge {
                                nonce,
                                meeting_calls: true,
                            },
                        )
                        .await?;
                        let proof: Proof =
                            tokio::time::timeout(Duration::from_secs(3), read(&mut pipe)).await??;
                        let bytes: [u8; 64] = proof
                            .signature
                            .try_into()
                            .map_err(|_| anyhow::anyhow!("bad host-chat proof"))?;
                        public
                            .verify(&challenge_message(&nonce, session), &Signature::from_bytes(&bytes))
                            .context("host-chat identity mismatch")?;
                        let (mut reader, mut writer) = tokio::io::split(pipe);
                        let receive = async {
                            loop {
                                let snapshot: Snapshot = read(&mut reader).await?;
                                e.store(snapshot.epoch, Ordering::Release);
                                c.store(true, Ordering::Release);
                                for event in cursor.update(&snapshot) {
                                    if tx.send(event).is_err() {
                                        bail!("GUI closed");
                                    }
                                }
                                ctx.send_viewport_cmd_to(eframe::egui::ViewportId::ROOT, eframe::egui::ViewportCommand::RequestPaintWhileHidden);
                                ctx.request_repaint();
                            }
                            #[allow(unreachable_code)]
                            anyhow::Ok(())
                        };
                        let transmit = async {
                            while let Some(command) = rx.recv().await {
                                write(&mut writer, &command).await?;
                            }
                            anyhow::Ok(())
                        };
                        tokio::select! {r=receive=>r,r=transmit=>r}
                    }
                    .await;
                    c.store(false, Ordering::Release);
                    cursor.directory = None;
                    cursor.public_directory = None;
                    let _ = tx.send(Event::Directories { primary: None, public: None });
                    if cursor.call.take().is_some() {
                        let _ = tx.send(Event::MeetingCall(None));
                    }
                    if cursor.password.take().is_some() {
                        let _ = tx.send(Event::Password(None));
                    }
                    if let Err(e) = result {
                        tracing::debug!("waiting for host-chat service: {e:#}");
                    }
                    ctx.send_viewport_cmd_to(eframe::egui::ViewportId::ROOT, eframe::egui::ViewportCommand::RequestPaintWhileHidden);
                ctx.request_repaint();
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            });
            Self {
                events,
                commands,
                epoch,
                connected,
                task,
            }
        }
    }
}

#[cfg(windows)]
pub use native::{Gui, Server};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_id_updates_and_older_service_snapshots_clear_the_public_id() {
        use slipstream_session::DirStatus;
        use slipstream_core::directory::DeviceId;
        let mut snapshot: Snapshot = serde_json::from_str(r#"{"epoch":1,"sequence":0,"sessions":[]}"#).unwrap();
        let mut cursor = Cursor::default();
        assert!(cursor.update(&snapshot).is_empty());
        snapshot.directory = Some(DirStatus::Online { id: DeviceId(123456789) });
        snapshot.public_directory = Some(DirStatus::Online { id: DeviceId(987654321) });
        assert_eq!(cursor.update(&snapshot), vec![Event::Directories {
            primary: snapshot.directory.clone(), public: snapshot.public_directory.clone(),
        }]);
        assert!(cursor.update(&snapshot).is_empty());
        let older: Snapshot = serde_json::from_str(r#"{"epoch":2,"sequence":0,"sessions":[]}"#).unwrap();
        assert_eq!(cursor.update(&older), vec![Event::Directories { primary: None, public: None }]);
    }

    #[test]
    fn retained_sender_uses_the_current_service_epoch_for_each_command() {
        use std::sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}};
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let connected = Arc::new(AtomicBool::new(false));
        let sender = Sender {tx, epoch: epoch.clone(), connected: connected.clone()};
        assert!(!sender.regenerate_password());
        assert!(!sender.chat(7, "offline".into()));
        sender.disconnect(7);
        assert!(rx.try_recv().is_err());
        epoch.store(41, Ordering::Release);
        connected.store(true, Ordering::Release);
        assert!(sender.regenerate_password());
        let first = rx.try_recv().unwrap();
        assert_eq!(first.epoch, 41);
        assert!(matches!(first.action, Action::RegeneratePassword));
        epoch.store(42, Ordering::Release);
        assert!(sender.chat(7, "hello".into()));
        let chat = rx.try_recv().unwrap();
        assert_eq!(chat.epoch, 42);
        assert!(matches!(chat.action, Action::Chat {session: 7, ..}));
        sender.disconnect(7);
        let stop = rx.try_recv().unwrap();
        assert_eq!(stop.epoch, 42);
        assert!(matches!(stop.action, Action::Disconnect {session: 7}));
        // Commands already queued for an old service retain their old epoch.
        assert_eq!(first.epoch, 41);
    }
    #[test]
    fn service_codes_rotate_without_replaying_chat_and_older_snapshots_still_load() {
        let mut snapshot: Snapshot = serde_json::from_str(r#"{"epoch":1,"sequence":0,"sessions":[]}"#).unwrap();
        let mut cursor = Cursor::default();
        assert!(cursor.update(&snapshot).is_empty());
        snapshot.password = Some("code-one".into());
        assert_eq!(cursor.update(&snapshot), vec![Event::Password(Some("code-one".into()))]);
        assert!(cursor.update(&snapshot).is_empty());
        snapshot.password = Some("code-two".into());
        assert_eq!(cursor.update(&snapshot), vec![Event::Password(Some("code-two".into()))]);
        snapshot.password = None;
        assert_eq!(cursor.update(&snapshot), vec![Event::Password(None)]);
    }

    #[test]
    fn service_status_is_optional_and_changes_clear_without_replaying() {
        let mut snapshot: Snapshot = serde_json::from_str(r#"{"epoch":1,"sequence":0,"sessions":[{"id":7,"peer":"Tech","lines":[]}]}"#).unwrap();
        let mut cursor=Cursor::default();
        assert_eq!(cursor.update(&snapshot),vec![Event::Started{session:7,peer:"Tech".into()}]);
        snapshot.sessions[0].status="Checking the printer".into();
        assert_eq!(cursor.update(&snapshot),vec![Event::Status{session:7,text:"Checking the printer".into()}]);
        assert!(cursor.update(&snapshot).is_empty());
        snapshot.sessions[0].status.clear();
        assert_eq!(cursor.update(&snapshot),vec![Event::Status{session:7,text:String::new()}]);
        snapshot.sessions.clear();
        assert_eq!(cursor.update(&snapshot),vec![Event::Ended{session:7}]);
    }

    #[test]
    fn reconnect_does_not_repeat_messages_and_restart_resets_sessions() {
        let mut snapshot = Snapshot {
            epoch: 1,
            sequence: 0,
            sessions: vec![Session {
                id: 0,
                peer: "Technician".into(),
                lines: Vec::new(),
                status: String::new(),
            }],
            password: None,
            call: None,
            directory: None,
            public_directory: None,
        };
        snapshot.message(0, false, "Hello".into());
        let mut cursor = Cursor::default();
        let first = cursor.update(&snapshot);
        assert_eq!(first.len(), 2);
        assert!(cursor.update(&snapshot).is_empty());
        snapshot.message(0, true, "Reply".into());
        assert_eq!(
            cursor.update(&snapshot),
            vec![Event::Message {
                session: 0,
                ours: true,
                text: "Reply".into()
            }]
        );
        snapshot.epoch = 2;
        snapshot.sessions[0].lines.clear();
        assert_eq!(
            cursor.update(&snapshot),
            vec![
                Event::Ended { session: 0 },
                Event::Started {
                    session: 0,
                    peer: "Technician".into()
                }
            ]
        );
        for i in 0..200 {
            snapshot.message(0, false, format!("{i}"));
        }
        assert_eq!(snapshot.sessions[0].lines.len(), 128);
    }
}
