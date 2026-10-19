//! Receive into a private, unique staging file. Dropping a failed/cancelled
//! transfer removes it; publishing never replaces an existing destination.
use anyhow::{Context, Result, ensure};
use iroh::endpoint::RecvStream;
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;

const CHUNK: usize = 256 * 1024;

pub(crate) async fn receive(
    recv: &mut RecvStream,
    dest: &Path,
    size: u64,
    mut progress: impl FnMut(u64),
) -> Result<PathBuf> {
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_owned();
    tokio::fs::create_dir_all(&parent).await?;
    let staged =
        tokio::task::spawn_blocking(move || NamedTempFile::with_prefix_in(".slipstream-", parent))
            .await??;
    let mut file = tokio::fs::File::from_std(staged.as_file().try_clone()?);
    let mut remaining = size;
    let mut buf = vec![0u8; CHUNK];
    while remaining > 0 {
        let want = remaining.min(CHUNK as u64) as usize;
        let Some(n) = recv.read(&mut buf[..want]).await? else {
            break;
        };
        file.write_all(&buf[..n]).await?;
        remaining -= n as u64;
        progress(n as u64);
    }
    ensure!(
        remaining == 0,
        "transfer of {} was interrupted ({remaining} bytes missing)",
        dest.display()
    );
    // A reset after the payload must not publish a cancelled upload. Also
    // reject a source which grew after advertising its original length.
    ensure!(
        recv.read(&mut [0u8; 1]).await?.is_none(),
        "file size changed during transfer"
    );
    file.flush().await?;
    drop(file);
    let dest = dest.to_owned();
    tokio::task::spawn_blocking(move || publish(staged, &dest)).await?
}

fn publish(mut staged: NamedTempFile, requested: &Path) -> Result<PathBuf> {
    loop {
        let dest = crate::fsops::unique_path(requested);
        match staged.persist_noclobber(&dest) {
            Ok(_) => return Ok(dest),
            Err(error) if error.error.kind() == ErrorKind::AlreadyExists => staged = error.file,
            Err(error) => return Err(error.error).context("saving the received file"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn concurrent_publish_preserves_existing_files_and_abandoned_staging_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("same.bin");
        let files: Vec<_> = (0..8u8)
            .map(|n| {
                let mut staged = NamedTempFile::new_in(dir.path()).unwrap();
                staged.write_all(&[n; 32]).unwrap();
                (n, staged)
            })
            .collect();
        std::thread::scope(|scope| {
            let jobs: Vec<_> = files
                .into_iter()
                .map(|(n, staged)| {
                    let dest = &dest;
                    scope.spawn(move || (n, publish(staged, dest).unwrap()))
                })
                .collect();
            let mut paths = std::collections::HashSet::new();
            for job in jobs {
                let (n, path) = job.join().unwrap();
                assert!(paths.insert(path.clone()));
                assert_eq!(std::fs::read(path).unwrap(), [n; 32]);
            }
        });
        let abandoned = NamedTempFile::new_in(dir.path()).unwrap();
        let path = abandoned.path().to_owned();
        drop(abandoned);
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 8);
    }
}
