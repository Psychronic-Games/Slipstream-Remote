//! File transfer over dedicated bi-streams (either direction).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use slipstream_core::{
    framing::{expect_msg, write_msg},
    proto::{FileHeader, FileResult},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Where received files land: `~/Downloads/Slipstream`.
pub fn download_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(Path::to_path_buf))
        .or_else(|| directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
        .join("Slipstream")
}

/// Sends a file; returns the name the receiver saved it under.
pub async fn send_file(conn: &Connection, path: &Path, progress: impl Fn(u64, u64)) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await.with_context(|| format!("opening {}", path.display()))?;
    let size = file.metadata().await?.len();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
    let (mut send, mut recv) = conn.open_bi().await?;
    write_msg(&mut send, &FileHeader { name, size }).await?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut sent = 0u64;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        send.write_all(&buf[..n]).await?;
        sent += n as u64;
        progress(sent, size);
    }
    send.finish()?;
    match expect_msg::<FileResult>(&mut recv).await? {
        FileResult::Saved { name } => Ok(name),
        FileResult::Rejected(why) => bail!("rejected: {why}"),
    }
}

/// Strips anything path-like so a peer can't write outside the target dir.
fn sanitize(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    if clean.is_empty() { "received-file".into() } else { clean.chars().take(200).collect() }
}

fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (1..).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).expect("unbounded")
}

/// Receives one file from an accepted bi-stream into [`download_dir`].
pub async fn receive_file(mut send: SendStream, mut recv: RecvStream, allowed: bool) -> Result<PathBuf> {
    let header: FileHeader = expect_msg(&mut recv).await?;
    if !allowed {
        write_msg(&mut send, &FileResult::Rejected("file transfer is disabled".into())).await?;
        send.finish()?;
        bail!("file transfer disabled");
    }
    let dir = download_dir();
    tokio::fs::create_dir_all(&dir).await?;
    let path = unique_path(&dir, &sanitize(&header.name));
    let partial = path.with_extension("part");
    let mut file = tokio::fs::File::create(&partial).await?;
    let mut remaining = header.size;
    let mut buf = vec![0u8; 256 * 1024];
    while remaining > 0 {
        let want = buf.len().min(remaining as usize);
        let Some(n) = recv.read(&mut buf[..want]).await? else { break };
        file.write_all(&buf[..n]).await?;
        remaining -= n as u64;
    }
    file.flush().await?;
    drop(file);
    if remaining != 0 {
        let _ = tokio::fs::remove_file(&partial).await;
        bail!("transfer of {} interrupted", header.name);
    }
    tokio::fs::rename(&partial, &path).await?;
    let saved = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
    write_msg(&mut send, &FileResult::Saved { name: saved }).await?;
    send.finish()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_blocks_traversal() {
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize("C:\\Windows\\evil.exe"), "evil.exe");
        assert_eq!(sanitize(".bashrc"), "bashrc");
        assert_eq!(sanitize(""), "received-file");
        assert_eq!(sanitize("report final.pdf"), "report final.pdf");
    }
}
