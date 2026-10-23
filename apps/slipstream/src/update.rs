//! App updates include an already-installed Windows service.
use anyhow::Result;
use slipstream_session::{Client, update::UpdateInfo};
use std::path::PathBuf;

pub async fn install(
    client: &Client,
    info: &UpdateInfo,
    progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    #[cfg(not(windows))]
    {
        client.install_update(info, progress).await
    }
    #[cfg(windows)]
    {
        let (info, staged) = client.stage_update(info, progress).await?;
        client.notify_restarting().await;
        tokio::task::spawn_blocking(move || crate::service::install_app_update(&staged, &info))
            .await?
    }
}

#[cfg(any(windows, test))]
pub(crate) mod transaction {
    use anyhow::{Context, Result, bail};
    use std::path::{Path, PathBuf};

    struct Replacement {
        target: PathBuf,
        fresh: tempfile::TempPath,
        previous: Option<tempfile::TempPath>,
        moved: bool,
    }

    /// Stage every copy before stopping the service. On failure, restore all
    /// originals and restart the previous service before reporting the error.
    pub fn replace(
        source: &Path,
        targets: &[PathBuf],
        mut stop: impl FnMut() -> Result<()>,
        mut start: impl FnMut() -> Result<()>,
    ) -> Result<()> {
        let mut files = Vec::new();
        for target in targets {
            let directory = target.parent().context("update target has no folder")?;
            let fresh = tempfile::NamedTempFile::with_prefix_in(".slipstream-new-", directory)?
                .into_temp_path();
            std::fs::copy(source, &fresh)
                .with_context(|| format!("staging {}", target.display()))?;
            let previous =
                tempfile::NamedTempFile::with_prefix_in(".slipstream-previous-", directory)?
                    .into_temp_path();
            std::fs::remove_file(&previous)?;
            files.push(Replacement {
                target: target.clone(),
                fresh,
                previous: Some(previous),
                moved: false,
            });
        }
        if let Err(error) = stop() {
            // A stop may have succeeded before its status check failed.
            let recovery = start();
            return match recovery {
                Ok(()) => Err(error),
                Err(recovery) => Err(error.context(format!(
                    "the previous service also failed to restart: {recovery:#}"
                ))),
            };
        }
        let result = (|| {
            for file in &mut files {
                std::fs::rename(
                    &file.target,
                    file.previous.as_deref().expect("rollback path"),
                )
                .with_context(|| format!("moving {} aside", file.target.display()))?;
                file.moved = true;
                std::fs::rename(&file.fresh, &file.target)
                    .with_context(|| format!("installing {}", file.target.display()))?;
            }
            start()
        })();
        if let Err(error) = result {
            let mut failures = Vec::new();
            if let Err(error) = stop() {
                failures.push(format!("stopping the new service: {error:#}"));
            }
            for file in files.iter_mut().rev().filter(|file| file.moved) {
                if file.target.exists()
                    && let Err(error) = std::fs::remove_file(&file.target)
                {
                    // A running Windows executable can be renamed, not deleted.
                    if let Err(rename) = std::fs::rename(&file.target, &file.fresh) {
                        failures.push(format!("moving the failed update aside: {error}; {rename}"));
                        // Preserve the only rollback copy if restoration is blocked.
                        let mut previous = file.previous.take().expect("rollback path");
                        previous.disable_cleanup(true);
                        failures.push(format!(
                            "previous executable kept at {}",
                            previous.display()
                        ));
                        continue;
                    }
                }
                if let Err(error) = std::fs::rename(
                    file.previous.as_deref().expect("rollback path"),
                    &file.target,
                ) {
                    failures.push(format!("restoring {}: {error}", file.target.display()));
                    let mut previous = file.previous.take().expect("rollback path");
                    previous.disable_cleanup(true);
                    failures.push(format!(
                        "previous executable kept at {}",
                        previous.display()
                    ));
                }
            }
            if let Err(error) = start() {
                failures.push(format!("restarting the previous service: {error:#}"));
            }
            if !failures.is_empty() {
                bail!(
                    "{error:#}; recovery needs attention: {}",
                    failures.join("; ")
                );
            }
            return Err(
                error.context("the update failed; the previous app and service were restored")
            );
        }
        for file in &mut files {
            let previous = file.previous.take().expect("rollback path");
            // Only successful transactions get a disposable .old suffix.
            // Failed-recovery backups remain distinguishable and preserved.
            let old = previous.with_extension("old");
            if std::fs::rename(&previous, &old).is_ok() {
                let _ = std::fs::remove_file(old);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{cell::Cell, rc::Rc};

        #[test]
        fn app_and_service_are_replaced_together_without_touching_the_profile() {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("new.exe");
            let app = dir.path().join("app.exe");
            let service = dir.path().join("service.exe");
            let profile = dir.path().join("identity.key");
            for (path, bytes) in [
                (&source, b"new".as_slice()),
                (&app, b"old app"),
                (&service, b"old service"),
                (&profile, b"machine identity"),
            ] {
                std::fs::write(path, bytes).unwrap();
            }
            let stopped = Cell::new(false);
            replace(
                &source,
                &[app.clone(), service.clone()],
                || {
                    stopped.set(true);
                    Ok(())
                },
                || {
                    assert!(stopped.get());
                    assert_eq!(std::fs::read(&app).unwrap(), b"new");
                    assert_eq!(std::fs::read(&service).unwrap(), b"new");
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(std::fs::read(profile).unwrap(), b"machine identity");
        }

        #[test]
        fn failed_restart_restores_both_executables_and_restarts_the_old_service() {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("new.exe");
            let app = dir.path().join("app.exe");
            let service = dir.path().join("service.exe");
            std::fs::write(&source, b"new").unwrap();
            std::fs::write(&app, b"old app").unwrap();
            std::fs::write(&service, b"old service").unwrap();
            let starts = Rc::new(Cell::new(0));
            let count = starts.clone();
            assert!(
                replace(
                    &source,
                    &[app.clone(), service.clone()],
                    || Ok(()),
                    || {
                        count.set(count.get() + 1);
                        if count.get() == 1 {
                            anyhow::bail!("new service failed");
                        }
                        assert_eq!(std::fs::read(&service).unwrap(), b"old service");
                        Ok(())
                    }
                )
                .is_err()
            );
            assert_eq!(starts.get(), 2);
            assert_eq!(std::fs::read(app).unwrap(), b"old app");
            assert_eq!(std::fs::read(service).unwrap(), b"old service");
        }

        #[test]
        fn blocked_recovery_keeps_the_previous_executables_for_manual_restore() {
            let root = tempfile::tempdir().unwrap();
            let installed = root.path().join("installed");
            let moved = root.path().join("moved");
            std::fs::create_dir(&installed).unwrap();
            let source = root.path().join("new.exe");
            let app = installed.join("app.exe");
            let service = installed.join("service.exe");
            std::fs::write(&source, b"new").unwrap();
            std::fs::write(&app, b"old app").unwrap();
            std::fs::write(&service, b"old service").unwrap();
            let starts = Cell::new(0);
            let error = replace(
                &source,
                &[app, service],
                || Ok(()),
                || {
                    starts.set(starts.get() + 1);
                    if starts.get() == 1 {
                        std::fs::rename(&installed, &moved).unwrap();
                        std::fs::write(&installed, b"blocks recovery").unwrap();
                        anyhow::bail!("new service failed");
                    }
                    std::fs::remove_file(&installed).unwrap();
                    std::fs::rename(&moved, &installed).unwrap();
                    Ok(())
                },
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains("recovery needs attention"));
            let mut backups = std::fs::read_dir(&installed)
                .unwrap()
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".slipstream-previous-")
                })
                .map(|entry| std::fs::read(entry.path()).unwrap())
                .collect::<Vec<_>>();
            backups.sort();
            assert_eq!(backups, [b"old app".to_vec(), b"old service".to_vec()]);
        }

        #[test]
        fn a_second_replacement_failure_also_restores_the_first() {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("new.exe");
            let app = dir.path().join("app.exe");
            let missing = dir.path().join("missing.exe");
            std::fs::write(&source, b"new").unwrap();
            std::fs::write(&app, b"old").unwrap();
            assert!(replace(&source, &[app.clone(), missing], || Ok(()), || Ok(())).is_err());
            assert_eq!(std::fs::read(app).unwrap(), b"old");
        }
    }
}
