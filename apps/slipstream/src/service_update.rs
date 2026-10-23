//! Privileged update of existing executables, without importing a user profile.
use super::*;
use slipstream_session::update::UpdateInfo;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::OpenOptionsExt,
    },
};
use windows_service::service::Service;

fn installed(access: ServiceAccess) -> Result<Option<Service>> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    match manager.open_service(NAME, access) {
        Ok(service) => Ok(Some(service)),
        Err(windows_service::Error::Winapi(error)) if error.raw_os_error() == Some(1060) => {
            Ok(None)
        }
        Err(error) => Err(error).context("querying the installed Slipstream service"),
    }
}

fn program(service: &Service) -> Result<PathBuf> {
    use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
    let command = service.query_config()?.executable_path;
    let command: Vec<u16> = command.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        let mut count = 0;
        let args = CommandLineToArgvW(command.as_ptr(), &mut count);
        anyhow::ensure!(
            !args.is_null() && count >= 1,
            "the installed service command is invalid"
        );
        let first = *args;
        let mut length = 0;
        while *first.add(length) != 0 {
            length += 1;
        }
        let path = PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(
            first, length,
        )));
        LocalFree(args.cast());
        anyhow::ensure!(
            path.is_absolute(),
            "the installed service executable path is not absolute"
        );
        std::fs::canonicalize(path).context("locating the installed service executable")
    }
}

fn version(path: &Path) -> Result<((u64, u64, u64), u64)> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        let size = GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut());
        anyhow::ensure!(
            size > 0 && size <= 1024 * 1024,
            "could not read the installed service version"
        );
        let mut data = vec![0u8; size as usize];
        anyhow::ensure!(
            GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) != 0,
            "could not read the installed service version"
        );
        let mut pointer = std::ptr::null_mut();
        let mut length = 0;
        anyhow::ensure!(
            VerQueryValueW(
                data.as_ptr().cast(),
                wide("\\").as_ptr(),
                &mut pointer,
                &mut length
            ) != 0
                && !pointer.is_null()
                && length as usize >= size_of::<VS_FIXEDFILEINFO>(),
            "the installed service has no version resource"
        );
        let info = std::ptr::read_unaligned(pointer.cast::<VS_FIXEDFILEINFO>());
        anyhow::ensure!(
            info.dwSignature == 0xFEEF04BD,
            "invalid installed service version resource"
        );
        Ok((
            (
                u64::from(info.dwFileVersionMS >> 16),
                u64::from(info.dwFileVersionMS & 65535),
                u64::from(info.dwFileVersionLS >> 16),
            ),
            u64::from(info.dwFileVersionLS & 65535),
        ))
    }
}

pub fn needs_app_update() -> Result<bool> {
    let Some(service) = installed(ServiceAccess::QUERY_CONFIG)? else {
        return Ok(false);
    };
    Ok(version(&program(&service)?)? < UpdateInfo::current_version_key())
}

fn trusted_directory() -> Result<PathBuf> {
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath},
    };
    unsafe {
        let mut pointer = std::ptr::null_mut();
        anyhow::ensure!(
            SHGetKnownFolderPath(
                &FOLDERID_ProgramFiles,
                0,
                std::ptr::null_mut(),
                &mut pointer
            ) >= 0
                && !pointer.is_null(),
            "could not locate the protected Program Files folder"
        );
        let mut length = 0;
        while *pointer.add(length) != 0 {
            length += 1;
        }
        let path = PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(
            pointer, length,
        )));
        CoTaskMemFree(pointer.cast());
        Ok(path.join("Slipstream Remote"))
    }
}

fn marker() -> Result<PathBuf> {
    Ok(trusted_directory()?.join(".slipstream-updating"))
}

pub fn update_in_progress() -> bool {
    marker()
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|time| time.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(300))
}

pub struct UpdateGuard {
    file: Option<File>,
    path: PathBuf,
}
impl UpdateGuard {
    pub fn acquire() -> Result<Self> {
        std::fs::create_dir_all(trusted_directory()?)?;
        let path = marker()?;
        // A live exclusive handle prevents removal; a stale marker can go.
        let _ = std::fs::remove_file(&path);
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(0)
            .open(&path)
            .context(
                "another app or service update is in progress — try again after it finishes",
            )?;
        Ok(Self {
            file: Some(file),
            path,
        })
    }
}
impl Drop for UpdateGuard {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

struct Busy;
impl Busy {
    fn new() -> Result<Self> {
        BUSY.compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .map_err(|_| {
            anyhow::anyhow!(
                "another service operation is in progress — try again after it finishes"
            )
        })?;
        Ok(Self)
    }
}
impl Drop for Busy {
    fn drop(&mut self) {
        set_busy(false);
    }
}

fn quoted(path: &Path) -> Result<String> {
    let text = path
        .to_str()
        .context("the update path is not valid Unicode")?;
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in text.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        out.extend(std::iter::repeat_n(
            '\\',
            if c == '"' { slashes * 2 + 1 } else { slashes },
        ));
        out.push(c);
        slashes = 0;
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    Ok(out)
}

pub fn install_app_update(staged: &Path, info: &UpdateInfo) -> Result<PathBuf> {
    install_update(staged, info, false)
}

/// Catch up an older service from the already-installed signed app. There is
/// no package download, app replacement or app restart in this path.
pub fn sync_app_service(info: &UpdateInfo) -> Result<()> {
    anyhow::ensure!(info.version_key() == UpdateInfo::current_version_key(),
        "the station package does not match this app — use Install & restart to update both together");
    if !needs_app_update()? { return Ok(()); }
    install_update(&std::env::current_exe()?, info, true)?;
    Ok(())
}

fn install_update(staged: &Path, info: &UpdateInfo, service_only: bool) -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let has_service = installed(ServiceAccess::QUERY_STATUS)?.is_some();
    let writable = !service_only && exe.parent().is_some_and(|p| {
        tempfile::NamedTempFile::with_prefix_in(".slipstream-write-test-", p).is_ok()
    });
    if !has_service && service_only { return Ok(exe); }
    if !has_service && writable {
        // TEMP may be on another drive; the final rename must be local.
        let local = slipstream_session::update::verified_copy(
            staged,
            exe.parent().context("app has no folder")?,
            info,
        )?
        .into_temp_path();
        return slipstream_session::update::install(&local);
    }
    let _busy = Busy::new()?;
    let mut metadata = tempfile::NamedTempFile::with_prefix("slipstream-update-info-")?;
    serde_json::to_writer(&mut metadata, info)?;
    metadata.flush()?;
    let report_id = slipstream_core::directory::hex(&rand::random::<[u8; 16]>());
    let report = report_path(&report_id)?;
    // Close the handles before the elevated process opens these files.
    let metadata = metadata.into_temp_path();
    let args = format!(
        "service apply-update --staged {} --info {} --report-id {}{}",
        quoted(staged)?,
        quoted(&metadata)?,
        report_id,
        if service_only { " --service-only" } else { "" }
    );
    let code = run_elevated(&args)?;
    if code != 0 {
        let mut message = String::new();
        if let Ok(file) = File::open(&report) {
            let _ = file.take(16384).read_to_string(&mut message);
        }
        if message.is_empty() {
            message = format!("Windows could not update the app and service (exit {code})");
        }
        bail!("{message}");
    }
    Ok(exe)
}

fn report_path(id: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid update report identifier"
    );
    Ok(trusted_directory()?
        .join("update-reports")
        .join(format!("{id}.txt")))
}

fn write_report(id: &str, error: &anyhow::Error) -> Result<()> {
    let path = report_path(id)?;
    let directory = path.parent().context("report has no folder")?;
    std::fs::create_dir_all(directory)?;
    let message: String = format!("{error:#}").chars().take(4096).collect();
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?
        .write_all(message.as_bytes())?;
    let mut previous = std::fs::read_dir(directory)?
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.len() == 36
                && name.ends_with(".txt")
                && name.as_bytes()[..32].iter().all(|b| b.is_ascii_hexdigit())
                && entry.path() != path
        })
        .collect::<Vec<_>>();
    previous
        .sort_by_key(|entry| std::cmp::Reverse(entry.metadata().and_then(|m| m.modified()).ok()));
    for entry in previous.into_iter().skip(7) {
        let _ = std::fs::remove_file(entry.path());
    }
    Ok(())
}

pub fn apply_app_update(staged: &Path, metadata: &Path, report_id: &str, service_only: bool) -> Result<()> {
    // An elevated helper accepts only a nonce, never a caller-chosen path
    // that could turn an error report into an arbitrary privileged write.
    report_path(report_id)?;
    let result = (|| {
        let info: UpdateInfo = serde_json::from_reader(File::open(metadata)?.take(16384))?;
        apply_update_to(staged, &info, &std::env::current_exe()?, service_only)
    })();
    if let Err(error) = &result
        && let Err(report_error) = write_report(report_id, error)
    {
        tracing::warn!("could not save the service update error: {report_error:#}");
    }
    result
}

fn apply_update_to(staged: &Path, info: &UpdateInfo, app: &Path, service_only: bool) -> Result<()> {
    let _guard = UpdateGuard::acquire()?;
    // A privileged snapshot prevents a changed user-writable download from
    // becoming a SYSTEM service executable after it was checked in the GUI.
    let candidate = slipstream_session::update::verified_copy(staged, &trusted_directory()?, info)?;
    let app = std::fs::canonicalize(app)?;
    anyhow::ensure!(
        info.version_key() >= version(&app)?,
        "the offered update is older than the installed app"
    );
    let service = installed(
        ServiceAccess::QUERY_CONFIG
            | ServiceAccess::QUERY_STATUS
            | ServiceAccess::STOP
            | ServiceAccess::START,
    )?;
    anyhow::ensure!(!service_only || info.version_key() == version(&app)?,
        "service catch-up requires the signed package for the installed app version");
    let mut targets = if service_only { Vec::new() } else { vec![app.clone()] };
    let mut restart = false;
    let mut update_service = false;
    if let Some(service) = &service {
        let path = program(service)?;
        // Keep a service that already has a newer build; never downgrade it.
        if version(&path)? < info.version_key() {
            update_service = true;
            restart = service.query_status()?.current_state != ServiceState::Stopped;
            if service_only || !path
                .to_string_lossy()
                .eq_ignore_ascii_case(&app.to_string_lossy())
            {
                targets.push(path);
            }
        }
    }
    if targets.is_empty() { return Ok(()); }
    let checked = std::cell::Cell::new(false);
    let stop = || -> Result<()> {
        if update_service
            && let Some(service) = &service
            && service.query_status()?.current_state != ServiceState::Stopped
        {
            service
                .stop()
                .context("stopping the Slipstream service for its update")?;
            wait_for(service, ServiceState::Stopped, 30);
            anyhow::ensure!(
                service.query_status()?.current_state == ServiceState::Stopped,
                "the service did not stop; the update was not installed"
            );
        }
        if !checked.replace(true) {
            anyhow::ensure!(
                version(&app)? <= info.version_key(),
                "the app advanced to a newer build during this update — restart it instead"
            );
            if update_service && let Some(service) = &service {
                anyhow::ensure!(
                    version(&program(service)?)? <= info.version_key(),
                    "the service advanced to a newer build during this update — check for updates again"
                );
            }
        }
        Ok(())
    };
    let start = || -> Result<()> {
        if update_service
            && restart
            && let Some(service) = &service
        {
            // An older GUI in another login session may have started the
            // previous image between stop and replacement. Load the new file.
            if service.query_status()?.current_state != ServiceState::Stopped {
                service
                    .stop()
                    .context("stopping a service restarted during the update")?;
                wait_for(service, ServiceState::Stopped, 30);
                anyhow::ensure!(
                    service.query_status()?.current_state == ServiceState::Stopped,
                    "the service did not stop for its restart"
                );
            }
            service
                .start::<&str>(&[])
                .context("starting the updated Slipstream service")?;
            wait_for(service, ServiceState::Running, 30);
            std::thread::sleep(Duration::from_secs(2));
            anyhow::ensure!(
                service.query_status()?.current_state == ServiceState::Running,
                "the updated service did not stay running"
            );
        }
        Ok(())
    };
    crate::update::transaction::replace(candidate.path(), &targets, stop, start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevated_error_reports_reject_caller_chosen_paths() {
        for id in [
            "../../Windows/System32/file",
            "C:\\Windows\\file",
            "123",
            "0123456789abcdef0123456789abcde/",
        ] {
            let error = apply_app_update(Path::new("missing.exe"), Path::new("missing.json"), id, false)
                .unwrap_err();
            assert!(error.to_string().contains("report identifier"));
        }
    }

    #[test]
    fn update_paths_round_trip_the_windows_command_line_parser() {
        use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
        for value in [
            r"C:\Users\Name With Spaces\update.exe",
            r"C:\Profiles\Ω & 'quoted'\update.exe",
            "C:\\trailing\\",
            "contains\"quote",
        ] {
            let command = wide(&format!(
                "slipstream.exe {}",
                quoted(Path::new(value)).unwrap()
            ));
            unsafe {
                let mut count = 0;
                let args = CommandLineToArgvW(command.as_ptr(), &mut count);
                assert!(!args.is_null());
                assert_eq!(count, 2);
                let pointer = *args.add(1);
                let mut length = 0;
                while *pointer.add(length) != 0 {
                    length += 1;
                }
                let parsed =
                    String::from_utf16(std::slice::from_raw_parts(pointer, length)).unwrap();
                LocalFree(args.cast());
                assert_eq!(parsed, value);
            }
        }
    }

    #[test]
    #[ignore = "requires an isolated Wine prefix, signed fixture exe and matching update metadata"]
    fn wine_service_update_preserves_profile_stopped_state_and_newer_service() {
        assert_eq!(
            std::env::var("SLIPSTREAM_SERVICE_UPDATE_QA").as_deref(),
            Ok("1")
        );
        assert!(
            std::env::var("WINEPREFIX")
                .unwrap_or_default()
                .contains("slipstream-service-update-")
        );
        slipstream_core::init_crypto();
        assert!(
            installed(ServiceAccess::QUERY_STATUS).unwrap().is_none(),
            "the disposable prefix must have no Slipstream service"
        );
        let source = PathBuf::from(std::env::var_os("SLIPSTREAM_UPDATE_TEST_EXE").unwrap());
        let metadata = PathBuf::from(std::env::var_os("SLIPSTREAM_UPDATE_TEST_INFO").unwrap());
        let info: UpdateInfo = serde_json::from_reader(File::open(metadata).unwrap()).unwrap();
        info.verify_signature(&slipstream_core::release::trusted_keys())
            .unwrap();
        let bytes = std::fs::read(&source).unwrap();
        let mut old = bytes.clone();
        let signature = [0xbd, 0x04, 0xef, 0xfe, 0, 0, 1, 0];
        let offset = old.windows(8).position(|part| part == signature).unwrap();
        old[offset + 8..offset + 12].copy_from_slice(&24u32.to_le_bytes());
        old[offset + 12..offset + 16].copy_from_slice(&127u32.to_le_bytes());
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("App with spaces Ω").join("slipstream.exe");
        let service_exe = root
            .path()
            .join("Service with spaces Ω")
            .join("slipstream.exe");
        for path in [&app, &service_exe] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, &old).unwrap();
        }
        assert_eq!(version(&app).unwrap(), ((0, 24, 0), 127));
        let home = machine_home();
        assert!(!home.exists(), "profile fixture must be fresh");
        std::fs::create_dir_all(&home).unwrap();
        let identity = home.join("identity.key");
        let settings = home.join("settings.toml");
        std::fs::write(&identity, b"service machine identity fixture").unwrap();
        std::fs::write(
            &settings,
            b"private station, cached IDs and unattended password fixture",
        )
        .unwrap();
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .unwrap();
        let fixture = ServiceInfo {
            name: NAME.into(),
            display_name: DISPLAY_NAME.into(),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::OnDemand,
            error_control: ServiceErrorControl::Normal,
            executable_path: service_exe.clone(),
            launch_arguments: vec!["service".into(), "run".into()],
            dependencies: vec![],
            account_name: None,
            account_password: None,
        };
        let service = manager
            .create_service(&fixture, ServiceAccess::ALL_ACCESS)
            .unwrap();
        struct Cleanup(Option<Service>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Some(service) = self.0.take() {
                    let _ = service.stop();
                    let _ = service.delete();
                }
            }
        }
        let mut cleanup = Cleanup(Some(service));
        let corrupt = root.path().join("damaged.exe");
        std::fs::write(&corrupt, b"damaged").unwrap();
        assert!(apply_update_to(&corrupt, &info, &app, false).is_err());
        assert!(std::fs::read(&app).unwrap() == old);
        assert!(std::fs::read(&service_exe).unwrap() == old);
        apply_update_to(&source, &info, &app, false).unwrap();
        assert!(std::fs::read(&app).unwrap() == bytes);
        assert!(std::fs::read(&service_exe).unwrap() == bytes);
        assert_eq!(
            cleanup
                .0
                .as_ref()
                .unwrap()
                .query_status()
                .unwrap()
                .current_state,
            ServiceState::Stopped
        );
        assert_eq!(
            std::fs::read(&identity).unwrap(),
            b"service machine identity fixture"
        );
        assert_eq!(
            std::fs::read(&settings).unwrap(),
            b"private station, cached IDs and unattended password fixture"
        );
        std::fs::write(&service_exe, &old).unwrap();
        let app_handle = OpenOptions::new().read(true).share_mode(1).open(&app).unwrap();
        apply_update_to(&app, &info, &app, true).unwrap();
        assert!(std::fs::read(&service_exe).unwrap() == bytes,
            "catch-up must install the already-present app into the service");
        assert!(std::fs::read(&app).unwrap() == bytes);
        // An already-current service must not be replaced either.
        let service_handle = OpenOptions::new().read(true).share_mode(1).open(&service_exe).unwrap();
        apply_update_to(&app, &info, &app, true).unwrap();
        drop(app_handle);
        std::fs::write(&app, &old).unwrap();
        assert!(apply_update_to(&source, &info, &app, true).is_err(),
            "service-only catch-up cannot advance beyond the installed app");
        apply_update_to(&source, &info, &app, false).unwrap();
        assert!(std::fs::read(&app).unwrap() == bytes,
            "a normal app update must leave the already-current locked service alone");
        drop(service_handle);
        assert_eq!(std::fs::read(&identity).unwrap(), b"service machine identity fixture");
        assert_eq!(std::fs::read(&settings).unwrap(), b"private station, cached IDs and unattended password fixture");
        let mut newer = bytes.clone();
        newer[offset + 8..offset + 12].copy_from_slice(&99u32.to_le_bytes());
        std::fs::write(&service_exe, &newer).unwrap();
        std::fs::write(&app, &old).unwrap();
        apply_update_to(&source, &info, &app, false).unwrap();
        assert!(
            std::fs::read(&service_exe).unwrap() == newer,
            "a newer installed service must not be downgraded"
        );
        assert!(std::fs::read(&app).unwrap() == bytes);
        let service = cleanup.0.take().unwrap();
        service.delete().unwrap();
        drop(service);
        for _ in 0..30 {
            if installed(ServiceAccess::QUERY_STATUS).unwrap().is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        std::fs::write(&app, &old).unwrap();
        apply_update_to(&source, &info, &app, false).unwrap();
        assert!(
            installed(ServiceAccess::QUERY_STATUS).unwrap().is_none(),
            "updating an app must not install a new service"
        );
        assert!(std::fs::read(&app).unwrap() == bytes);
        let app_handle = OpenOptions::new().read(true).share_mode(1).open(&app).unwrap();
        apply_update_to(&app, &info, &app, true).unwrap();
        assert!(installed(ServiceAccess::QUERY_STATUS).unwrap().is_none());
        drop(app_handle);
    }
}
