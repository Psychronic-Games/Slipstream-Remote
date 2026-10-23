//! Self-update from the station the app came from.
//!
//! A station publishes `clients/manifest.json` (version, build number,
//! SHA-256 and release signature of each platform's app). The app asks its
//! station for the build that fits it, checks the signature against the
//! release keys built into it (see `slipstream_core::release`), downloads
//! the build — preconfigured for the station, like the download page does —
//! verifies it against the signed checksum, swaps it in and restarts. Settings, identity and passwords live in the settings folder,
//! so they survive updates untouched.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// What the station offers for this platform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub build: u64,
    #[serde(default)]
    pub commit: String,
    /// Size of the app itself (without the station's config trailer).
    pub size: u64,
    /// SHA-256 (hex) of the app itself.
    pub sha256: String,
    /// `os` value for the station's download page.
    pub os: String,
    /// Release signature (hex) over platform, version, build, size and
    /// SHA-256. Empty from stations built before updates were signed.
    #[serde(default)]
    pub sig: String,
}

fn parse_version(v: &str) -> (u64, u64, u64) {
    let mut it = v.split(['.', '-', '+']).map(|p| p.parse::<u64>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

impl UpdateInfo {
    pub fn current_version_key() -> ((u64, u64, u64), u64) {
        (parse_version(slipstream_core::APP_VERSION), slipstream_core::BUILD_NUMBER)
    }
    pub fn version_key(&self) -> ((u64, u64, u64), u64) {
        (parse_version(&self.version), self.build)
    }

    /// Is this newer than the running app?
    pub fn is_newer(&self) -> bool {
        let mine = Self::current_version_key();
        self.version_key() > mine
    }

    pub fn label(&self) -> String {
        format!("{} (build {})", self.version, self.build)
    }

    /// Checks the release signature for this platform against `keys`.
    pub fn verify_signature(&self, keys: &[iroh::PublicKey]) -> Result<()> {
        let platform = slipstream_core::platform();
        slipstream_core::release::SignedUpdate {
            platform: &platform,
            version: &self.version,
            build: self.build,
            size: self.size,
            sha256: &self.sha256,
        }
        .verify(keys, &self.sig)
    }
}

/// The running executable, if it can be replaced in place.
fn exe_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("cannot locate the app")?;
    let dir = exe.parent().context("app has no folder")?;
    if tempfile::NamedTempFile::with_prefix_in(".slipstream-write-test-",dir).is_err() {
        bail!("this copy of Slipstream is installed in a protected folder ({}) — download the new version from your station instead", dir.display());
    }
    Ok(exe)
}

/// Length of the app without a station config trailer (see `provision`).
fn base_len(bytes: &[u8]) -> usize {
    let n = bytes.len();
    if n >= 12 && &bytes[n - 8..] == slipstream_core::provision::TRAILER_MAGIC {
        let json = u32::from_le_bytes(bytes[n - 12..n - 8].try_into().expect("4 bytes")) as usize;
        if json > 0 && json <= slipstream_core::provision::MAX_CONFIG as usize && json + 12 <= n {
            return n - 12 - json;
        }
    }
    n
}

/// Downloads `info` from the station and verifies it. Returns the staged
/// file, ready for [`install`].
pub async fn download(
    station: &reqwest::Url,
    key: Option<&str>,
    info: &UpdateInfo,
    progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    // The checksum below is only worth something if it's signed.
    info.verify_signature(&slipstream_core::release::trusted_keys())?;
    let staged = staging_path()?;
    download_to(station, key, info, &staged, progress).await?;
    Ok(staged)
}

pub(crate) fn staging_path() -> Result<PathBuf> {
    let exe = exe_path()?;
    Ok(exe.with_file_name(format!(
        "{}.update",
        exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "slipstream".into())
    )))
}

/// Stages outside the app folder when a privileged Windows installer will
/// perform the replacement. The same signature and size checks still apply.
pub async fn download_to(
    station: &reqwest::Url,
    key: Option<&str>,
    info: &UpdateInfo,
    staged: &Path,
    mut progress: impl FnMut(u64, u64),
) -> Result<()> {
    download_to_with_keys(station, key, info, staged, &slipstream_core::release::trusted_keys(), &mut progress).await
}

async fn download_to_with_keys(station: &reqwest::Url, key: Option<&str>, info: &UpdateInfo, staged: &Path, keys: &[iroh::PublicKey], mut progress: impl FnMut(u64, u64)) -> Result<()> {
    info.verify_signature(keys)?;
    let mut url = station.join("download.php")?;
    url.query_pairs_mut().append_pair("os", &info.os).append_pair("sha256", &info.sha256);
    let client = slipstream_core::directory::http_client()?;
    let mut req = client.get(url);
    if let Some(k) = key {
        req = req.header("X-Slipstream-Key", k);
    }
    let response = tokio::time::timeout(std::time::Duration::from_secs(30),req.send()).await
        .context("update response timed out")?.context("downloading the update")?;
    stage_download(response,info,staged,&mut progress).await
}

pub(crate) async fn available(dc: &slipstream_core::directory::DirectoryClient, keys: &[iroh::PublicKey]) -> Result<Option<UpdateInfo>> {
    let Some(value) = dc.update_check(&slipstream_core::platform()).await? else { return Ok(None) };
    let info: UpdateInfo = serde_json::from_value(value).context("unreadable update info")?;
    info.verify_signature(keys)?;
    Ok(Some(info))
}

pub(crate) async fn download_latest(dc: &slipstream_core::directory::DirectoryClient, requested: &UpdateInfo, staged: &Path, keys: &[iroh::PublicKey], mut progress: impl FnMut(u64, u64)) -> Result<UpdateInfo> {
    // The visible offer may predate a publication. Always refresh it first.
    let mut info = available(dc, keys).await?.context("the station has no signed update package")?;
    anyhow::ensure!(info.version_key() >= UpdateInfo::current_version_key(), "the station offers an older app — not installing a downgrade");
    if info != *requested { tracing::info!(version = %info.label(), "refreshed the update offer before downloading"); }
    for attempt in 0..2 {
        match download_to_with_keys(dc.base_url(), dc.key(), &info, staged, keys, &mut progress).await {
            Ok(()) => return Ok(info),
            Err(error) => {
                // Publication may also race this download. Only retry with a
                // separately verified, newer signed offer; never skip a hash.
                if attempt == 0 && let Ok(Some(next)) = available(dc, keys).await
                    && next.version_key() > info.version_key() {
                    info = next;
                    continue;
                }
                return Err(error).context("could not verify the update — check for updates again, or download the latest app from your station");
            }
        }
    }
    unreachable!("download loop returns on its final attempt")
}

/// Copies into a trusted directory before verifying again. An elevated
/// installer must never trust a previous check of a user-writable source.
pub fn verified_copy(source: &Path, directory: &Path, info: &UpdateInfo) -> Result<tempfile::NamedTempFile> {
    verified_copy_with_keys(source, directory, info, &slipstream_core::release::trusted_keys())
}

fn verified_copy_with_keys(source: &Path, directory: &Path, info: &UpdateInfo, keys: &[iroh::PublicKey]) -> Result<tempfile::NamedTempFile> {
    use std::io::{Read, Seek, Write};
    info.verify_signature(keys)?;
    let limit = info.size.checked_add(u64::from(slipstream_core::provision::MAX_CONFIG) + 12).context("invalid update length")?;
    let mut copy = tempfile::NamedTempFile::with_prefix_in(".slipstream-verified-", directory)?;
    let source = std::fs::File::open(source)?;
    let copied = std::io::copy(&mut source.take(limit.checked_add(1).context("invalid update length")?), &mut copy)?;
    anyhow::ensure!(copied <= limit && copied >= info.size, "the downloaded update has an invalid length");
    copy.flush()?;
    copy.rewind()?;
    let mut digest = Sha256::new();
    let mut remaining = info.size;
    let mut buffer = [0u8; 65536];
    while remaining > 0 {
        let length = (remaining.min(buffer.len() as u64)) as usize;
        copy.read_exact(&mut buffer[..length])?;
        digest.update(&buffer[..length]);
        remaining -= length as u64;
    }
    let mut trailer = Vec::new();
    copy.read_to_end(&mut trailer)?;
    anyhow::ensure!(slipstream_core::directory::hex(&digest.finalize()).eq_ignore_ascii_case(&info.sha256)
        && (trailer.is_empty() || base_len(&trailer) == 0), "the downloaded update is damaged (checksum mismatch) — check for updates again");
    copy.rewind()?;
    Ok(copy)
}

async fn stage_download(mut resp:reqwest::Response,info:&UpdateInfo,staged:&Path,mut progress:impl FnMut(u64,u64))->Result<()> {
    anyhow::ensure!(resp.status().is_success(),"the station refused the download (HTTP {})",resp.status());
    let limit=info.size.checked_add(u64::from(slipstream_core::provision::MAX_CONFIG)+12).context("invalid update length")?;
    anyhow::ensure!(resp.content_length().is_none_or(|len|len<=limit),"update response exceeds the signed size");
    let parent=staged.parent().context("update path has no parent")?.to_owned();
    let temp=tokio::task::spawn_blocking(move || tempfile::NamedTempFile::with_prefix_in(".slipstream-update-",parent)).await??;
    let mut file=tokio::fs::File::from_std(temp.as_file().try_clone()?);
    let (mut done,mut digest,mut trailer)=(0u64,Sha256::new(),Vec::new());
    let total=resp.content_length().unwrap_or(info.size);
    loop {
        let chunk=tokio::time::timeout(std::time::Duration::from_secs(30),resp.chunk()).await.context("update download stopped responding")??;
        let Some(chunk)=chunk else {break};
        anyhow::ensure!(chunk.len() as u64<=limit-done,"update response exceeds the signed size");
        let base=(info.size.saturating_sub(done)).min(chunk.len() as u64) as usize;
        digest.update(&chunk[..base]);trailer.extend_from_slice(&chunk[base..]);
        file.write_all(&chunk).await?;done+=chunk.len() as u64;progress(done,total);
    }
    file.flush().await?;drop(file);
    let hex=slipstream_core::directory::hex(&digest.finalize());
    anyhow::ensure!(done>=info.size && (trailer.is_empty() || base_len(&trailer)==0)
        && hex.eq_ignore_ascii_case(&info.sha256),"the downloaded update is damaged (checksum mismatch) — try again");
    let staged=staged.to_owned();
    tokio::task::spawn_blocking(move || temp.persist(staged).map_err(|e|e.error)).await??;
    Ok(())
}

/// Swaps the staged update in for the running app. Returns the app path to
/// relaunch. (Windows can't overwrite a running .exe but can rename it, so
/// the old one is moved aside and deleted on the next start. On macOS the
/// update is a zipped app bundle; see [`install_bundle`].)
pub fn install(staged: &Path) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    if std::fs::read(staged).is_ok_and(|b| b.starts_with(b"PK")) {
        return install_bundle(staged);
    }
    let exe = exe_path()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(staged, &exe).context("replacing the app")?;
    }
    #[cfg(windows)]
    {
        let old = exe.with_extension("old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old).context("moving the old app aside")?;
        if let Err(e) = std::fs::rename(staged, &exe) {
            let _ = std::fs::rename(&old, &exe); // roll back
            return Err(e).context("installing the update");
        }
    }
    Ok(exe)
}

/// The bundle `exe` lives in (`X.app/Contents/MacOS/exe`) and its Contents.
#[cfg(target_os = "macos")]
fn bundle_of(exe: &Path) -> Option<(PathBuf, PathBuf)> {
    let contents = exe.parent()?.parent()?;
    let bundle = contents.parent()?;
    (contents.ends_with("Contents") && bundle.extension().is_some_and(|e| e == "app"))
        .then(|| (bundle.to_path_buf(), contents.to_path_buf()))
}

/// macOS: swaps the whole `Contents` folder of the running bundle for the
/// one in the zipped update, so the bundle stays consistent with its code
/// signature. Unpacked inside the bundle, so the swap is a rename.
#[cfg(target_os = "macos")]
fn install_bundle(staged: &Path) -> Result<PathBuf> {
    use anyhow::ensure;
    let exe = exe_path()?;
    let (bundle, contents) = bundle_of(&exe).context("this copy of Slipstream isn't inside an app bundle")?;
    let work = bundle.join(".update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let unpacked = std::process::Command::new("ditto").args(["-x", "-k"]).arg(staged).arg(&work).status()?;
    ensure!(unpacked.success(), "unpacking the update failed");
    let fresh = std::fs::read_dir(&work)?
        .flatten()
        .map(|e| e.path().join("Contents"))
        .find(|c| c.join("MacOS").join(exe.file_name().unwrap_or_default()).is_file())
        .context("the update doesn't contain the app")?;
    let old = bundle.join(".Contents.old");
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(&contents, &old).context("moving the old app aside")?;
    if let Err(e) = std::fs::rename(&fresh, &contents) {
        let _ = std::fs::rename(&old, &contents); // roll back
        return Err(e).context("installing the update");
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(exe)
}

/// Removes leftovers of a previous update (call at startup).
pub fn cleanup() {
    #[cfg(target_os = "macos")]
    if let Some((bundle, _)) = std::env::current_exe().ok().as_deref().and_then(bundle_of) {
        let _ = std::fs::remove_dir_all(bundle.join(".Contents.old"));
        let _ = std::fs::remove_dir_all(bundle.join(".update"));
    }
    if let Ok(exe) = std::env::current_exe() {
        #[cfg(windows)]
        if let Some(parent) = exe.parent() && let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name();let name = name.to_string_lossy();
                if name.starts_with(".slipstream-previous-") && name.ends_with(".old") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let _ = std::fs::remove_file(exe.with_extension("old"));
        // A second Windows GUI can start while the SYSTEM host is staging
        // its update. That fixed staging file is replaced on the next download.
        #[cfg(not(windows))]
        if let Some(name) = exe.file_name() {
            let _ = std::fs::remove_file(exe.with_file_name(format!("{}.update", name.to_string_lossy())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_info(key: &iroh::SecretKey, build: u64, payload: &[u8]) -> UpdateInfo {
        let mut info = UpdateInfo { version: "999.0.0".into(), build, commit: "fixture".into(), size: payload.len() as u64,
            sha256: slipstream_core::directory::hex(&Sha256::digest(payload)), os: "windows".into(), sig: String::new() };
        info.sig = slipstream_core::release::SignedUpdate { platform: &slipstream_core::platform(), version: &info.version,
            build, size: info.size, sha256: &info.sha256 }.sign(key);
        info
    }

    #[test]
    fn privileged_copy_reverifies_the_snapshot_and_cleans_rejected_files() {
        let key = iroh::SecretKey::generate();let keys = [key.public()];
        let dir = tempfile::tempdir().unwrap();let trusted = tempfile::tempdir().unwrap();
        let source = dir.path().join("download.exe");let payload = b"signed executable fixture";
        let info = signed_info(&key, 1, payload);
        std::fs::write(&source, payload).unwrap();
        let snapshot = verified_copy_with_keys(&source, trusted.path(), &info, &keys).unwrap();
        std::fs::write(&source, b"changed after copying").unwrap();
        assert_eq!(std::fs::read(snapshot.path()).unwrap(), payload);
        drop(snapshot);
        assert!(verified_copy_with_keys(&source, trusted.path(), &info, &keys).is_err());
        std::fs::write(&source, payload).unwrap();
        assert!(verified_copy_with_keys(&source, trusted.path(), &info, &[iroh::SecretKey::generate().public()]).is_err());
        let mut bad_trailer = payload.to_vec();bad_trailer.extend_from_slice(b"unexpected appended data");
        std::fs::write(&source, bad_trailer).unwrap();
        assert!(verified_copy_with_keys(&source, trusted.path(), &info, &keys).is_err());
        let mut valid = payload.to_vec();valid.extend(slipstream_core::provision::trailer(&slipstream_core::provision::StationConfig { server: "station.invalid".into(), key: None, org: None }));
        std::fs::write(&source, &valid).unwrap();
        let snapshot = verified_copy_with_keys(&source, trusted.path(), &info, &keys).unwrap();
        assert_eq!(std::fs::read(snapshot.path()).unwrap(), valid);drop(snapshot);
        assert_eq!(std::fs::read_dir(trusted.path()).unwrap().count(), 0);
    }

    struct Offers {
        dc: slipstream_core::directory::DirectoryClient,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        downloads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Offers { fn drop(&mut self) { self.task.abort(); } }
    async fn offers(key: &iroh::SecretKey, publish_during_download: bool, corrupt: bool, untrusted_next: bool) -> Offers {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
        let first = signed_info(key, 1, b"first build");
        let other_key = iroh::SecretKey::generate();
        let second = signed_info(if untrusted_next { &other_key } else { key }, 2, b"second build");
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dc = slipstream_core::directory::DirectoryClient::new(&format!("http://{}", socket.local_addr().unwrap()), None).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));let requests = calls.clone();
        let downloads = Arc::new(AtomicUsize::new(0));let gets = downloads.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut conn, _) = socket.accept().await.unwrap();
                let mut request = Vec::new();let mut buf = [0u8; 1024];
                let header_end = loop {
                    let length = conn.read(&mut buf).await.unwrap();if length == 0 { return; }
                    request.extend_from_slice(&buf[..length]);
                    if let Some(end) = request.windows(4).position(|b| b == b"\r\n\r\n") { break end+4; }
                    assert!(request.len() < 16384);
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let length = headers.lines().find_map(|line| line.to_lowercase().strip_prefix("content-length:").map(|v|v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                let post = headers.starts_with("POST ");
                while request.len() < header_end+length {
                    let length = conn.read(&mut buf).await.unwrap();if length == 0 { return; }
                    request.extend_from_slice(&buf[..length]);
                }
                let body = if post {
                    let count = requests.fetch_add(1, Ordering::SeqCst);
                    let info = if count > 0 && publish_during_download { &second } else { &first };
                    serde_json::to_vec(&serde_json::json!({"ok":true,"update":info})).unwrap()
                } else {
                    gets.fetch_add(1, Ordering::SeqCst);
                    if corrupt { b"damaged build".to_vec() } else if publish_during_download { b"second build".to_vec() } else { b"first build".to_vec() }
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                conn.write_all(response.as_bytes()).await.unwrap();conn.write_all(&body).await.unwrap();
            }
        });
        Offers { dc, calls, downloads, task }
    }

    #[tokio::test]
    async fn stale_offers_refresh_and_publication_races_retry_only_verified_newer_builds() {
        slipstream_core::init_crypto();
        use std::sync::atomic::Ordering;
        let key = iroh::SecretKey::generate();let keys = [key.public()];
        let stale = signed_info(&key, 0, b"old cached build");
        for (races, corrupt, untrusted) in [(false, false, false), (true, false, false), (false, true, false), (true, false, true)] {
            let station = offers(&key, races, corrupt, untrusted).await;
            let dir = tempfile::tempdir().unwrap();let staged = dir.path().join("app.update");
            let result = download_latest(&station.dc, &stale, &staged, &keys, |_, _| {}).await;
            if corrupt || untrusted {
                assert!(result.is_err());assert!(!staged.exists());
                assert_eq!(station.downloads.load(Ordering::SeqCst), 1);
            } else {
                let info = result.unwrap();assert_eq!(info.build, if races {2} else {1});
                assert_eq!(std::fs::read(staged).unwrap(), if races {b"second build".as_slice()} else {b"first build".as_slice()});
                assert_eq!(station.downloads.load(Ordering::SeqCst), if races {2} else {1});
            }
            assert_eq!(station.calls.load(Ordering::SeqCst), if races || corrupt {2} else {1});
        }
    }

    #[tokio::test]
    async fn streamed_updates_are_bounded_verified_and_cleaned_on_failure() {
        slipstream_core::init_crypto();
        use tokio::io::{AsyncReadExt,AsyncWriteExt};
        async fn response(body:Vec<u8>,length:Option<usize>)->reqwest::Response {
            let server=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=server.local_addr().unwrap();
            tokio::spawn(async move {
                let (mut socket,_)=server.accept().await.unwrap();let mut request=[0u8;2048];let _=socket.read(&mut request).await;
                let length=length.map(|n|format!("Content-Length: {n}\r\n")).unwrap_or_default();
                let _=socket.write_all(format!("HTTP/1.1 200 OK\r\n{length}Connection: close\r\n\r\n").as_bytes()).await;
                let _=socket.write_all(&body).await;
            });
            reqwest::Client::new().get(format!("http://{addr}/")).send().await.unwrap()
        }
        let payload:Vec<u8>=(0..300_000).map(|n|(n % 251) as u8).collect();
        let info=UpdateInfo {size:payload.len() as u64,sha256:slipstream_core::directory::hex(&Sha256::digest(&payload)),version:String::new(),build:0,commit:String::new(),os:String::new(),sig:String::new()};
        let dir=tempfile::tempdir().unwrap();let staged=dir.path().join("app.update");
        let mut bytes=payload.to_vec();bytes.extend(slipstream_core::provision::trailer(&slipstream_core::provision::StationConfig {server:"example.invalid".into(),key:None,org:None}));
        stage_download(response(bytes.clone(),Some(bytes.len())).await,&info,&staged,|_,_|{}).await.unwrap();
        assert_eq!(tokio::fs::read(&staged).await.unwrap(),bytes);
        tokio::fs::remove_file(&staged).await.unwrap();
        stage_download(response(payload.clone(),None).await,&info,&staged,|_,_|{}).await.unwrap();
        assert_eq!(tokio::fs::read(&staged).await.unwrap(),payload);
        tokio::fs::remove_file(&staged).await.unwrap();
        let oversized=payload.len()+slipstream_core::provision::MAX_CONFIG as usize+13;
        for (body,length) in [(b"damaged".to_vec(),None),(vec![0;oversized],None),(Vec::new(),Some(1_000_000))] {
            assert!(stage_download(response(body,length).await,&info,&staged,|_,_|{}).await.is_err());
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(),0,"failed update left staging data");
        }
    }

    #[test]
    fn version_ordering() {
        let mk = |v: &str, b: u64| UpdateInfo {
            version: v.into(),
            build: b,
            commit: String::new(),
            size: 0,
            sha256: String::new(),
            os: String::new(),
            sig: String::new(),
        };
        assert!(mk("99.0.0", 0).is_newer());
        assert!(!mk("0.0.1", 0).is_newer());
        assert!(mk(slipstream_core::APP_VERSION, slipstream_core::BUILD_NUMBER + 1).is_newer());
        assert!(!mk(slipstream_core::APP_VERSION, slipstream_core::BUILD_NUMBER).is_newer());
    }

    #[test]
    fn signature_covers_the_manifest_entry() {
        let key = iroh::SecretKey::generate();
        let mut info: UpdateInfo = serde_json::from_value(serde_json::json!({
            "version": "0.5.1", "build": 20, "size": 9, "sha256": "ab12", "os": "linux"
        }))
        .unwrap();
        // Stations built before signing send no `sig`.
        assert!(info.verify_signature(&[key.public()]).is_err());
        info.sig = slipstream_core::release::SignedUpdate {
            platform: &slipstream_core::platform(),
            version: "0.5.1",
            build: 20,
            size: 9,
            sha256: "ab12",
        }
        .sign(&key);
        assert!(info.verify_signature(&[key.public()]).is_ok());
        info.sha256 = "ab13".into();
        assert!(info.verify_signature(&[key.public()]).is_err());
    }

    #[test]
    fn trailer_is_excluded_from_checksum() {
        let mut bytes = b"APP BYTES".to_vec();
        let cfg = slipstream_core::provision::StationConfig { server: "s".into(), key: None, org: None };
        bytes.extend(slipstream_core::provision::trailer(&cfg));
        assert_eq!(base_len(&bytes), 9);
        assert_eq!(base_len(b"no trailer"), 10);
    }
}
