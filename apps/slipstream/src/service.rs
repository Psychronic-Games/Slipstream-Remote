//! Windows service: unattended access including UAC prompts, elevated
//! windows and the login screen.
//!
//! The service (`slipstream service run`, LocalSystem) only supervises: it
//! keeps a host (`slipstream service host`) running in the active console
//! session, as SYSTEM, and replaces it when the session changes (log on,
//! log off, fast user switching). Running as SYSTEM inside the session is
//! what lets the host follow the input desktop onto the login screen and
//! UAC prompts and type into elevated windows (see
//! `slipstream_platform::windesk`). The service also generates
//! Ctrl+Alt+Del (`SendSAS`) for it, and restarts itself when an update has
//! replaced its program.
//!
//! The host uses a machine profile in %ProgramData%, readable only by
//! SYSTEM and administrators. `install` copies this computer's station
//! settings, identity (so the ID stays the same) and unattended password
//! into it.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail};
use slipstream_core::config::Store;
use windows_service::{
    define_windows_service,
    service::{
        ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept, ServiceErrorControl,
        ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState,
        ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
    service_manager::{ServiceManager, ServiceManagerAccess},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, STILL_ACTIVE, WAIT_OBJECT_0},
    Security::{
        DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TOKEN_ALL_ACCESS, TokenPrimary, TokenSessionId,
    },
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        },
        LibraryLoader::{GetProcAddress, LoadLibraryW},
        RemoteDesktop::WTSGetActiveConsoleSessionId,
        Threading::{
            CREATE_NO_WINDOW, CreateEventW, CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken,
            PROCESS_INFORMATION, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
        },
    },
};

pub const NAME: &str = "SlipstreamRemote";
const DISPLAY_NAME: &str = "Slipstream Remote";

#[path = "service_update.rs"]
mod updates;
pub use updates::{UpdateGuard, apply_app_update, install_app_update, needs_app_update, update_in_progress};

/// The machine profile the service's host uses.
pub fn machine_home() -> PathBuf {
    let base = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| r"C:\ProgramData".into());
    base.join("Slipstream Remote")
}

/// Where the service tells signed-in users which ID it hosts this computer
/// as: readable by everyone, unlike the rest of the profile.
fn public_dir() -> PathBuf {
    machine_home().join("public")
}

/// The service's log (in the public folder: it holds no secrets, and
/// whoever looks after this computer should be able to read it).
pub fn log_path() -> PathBuf {
    public_dir().join("slipstream.log")
}

/// The ID the service hosts this computer as (it's usually the app's own,
/// but needn't be: then partners need this one). Looked up at most every
/// two seconds.
pub fn hosted_id() -> Option<slipstream_core::directory::DeviceId> {
    static SEEN: std::sync::Mutex<Option<(Instant, Option<slipstream_core::directory::DeviceId>)>> = std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().unwrap();
    if let Some((at, id)) = *seen
        && at.elapsed() < Duration::from_secs(2)
    {
        return id;
    }
    let id = std::fs::read_to_string(public_dir().join("device-id.txt")).ok().and_then(|s| s.trim().parse().ok());
    *seen = Some((Instant::now(), id));
    id
}

/// Makes everything in the profile inherit the folder's permissions.
/// Installs before build 36 set them file by file (`icacls /T`), which on a
/// reinstall left the files already there with no permissions at all: the
/// install then couldn't replace the identity, and the service couldn't
/// read it (older builds then made a new one: a new ID).
fn repair_permissions() {
    let _ = quiet("icacls").arg(machine_home().join("*")).args(["/reset", "/T", "/C", "/Q"]).output();
}

/// Creates the public folder, readable by all users (the profile around it
/// is administrators-only).
pub(crate) fn ensure_public_dir() -> PathBuf {
    let dir = public_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = quiet("icacls").arg(&dir).args(["/grant", "*S-1-5-32-545:(OI)(CI)RX", "/Q"]).output();
    dir
}

/// Run by the service: publishes the ID its host registered as, when it
/// changes.
fn publish_id(last: &mut Option<slipstream_core::directory::DeviceId>) {
    let Ok(store) = Store::open(machine_home()) else { return };
    let settings = store.load_settings();
    let custom = settings.server.url.as_deref().is_some_and(|url| !url.trim().is_empty() && !slipstream_core::config::is_public_station(url));
    let id = if custom { settings.host.last_device_id } else { settings.host.public_device_id };
    if id.is_some() && id != *last {
        let dir = public_dir();
        let _ = std::fs::create_dir_all(&dir);
        if std::fs::write(dir.join("device-id.txt"), id.map(|i| i.0.to_string()).unwrap_or_default()).is_ok() {
            *last = id;
        }
    }
}

fn install_dir() -> PathBuf {
    let base = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| r"C:\Program Files".into());
    base.join("Slipstream Remote")
}

/// The service's state, or `None` if it isn't installed. Works without
/// administrator rights.
pub fn state() -> Option<ServiceState> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?;
    let service = manager.open_service(NAME, ServiceAccess::QUERY_STATUS).ok()?;
    service.query_status().ok().map(|s| s.current_state)
}

/// Running, or on its way: either way it hosts this computer, so the app
/// mustn't as well (two hosts with one identity push each other off).
pub fn running() -> bool {
    matches!(state(), Some(ServiceState::Running | ServiceState::StartPending | ServiceState::ContinuePending))
}

/// The service's state in words, for the Config page (looked up at most
/// every two seconds).
pub fn status_text() -> &'static str {
    static SEEN: std::sync::Mutex<Option<(Instant, Option<ServiceState>)>> = std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().unwrap();
    let state = match *seen {
        Some((at, s)) if at.elapsed() < Duration::from_secs(2) => s,
        _ => {
            let s = state();
            *seen = Some((Instant::now(), s));
            s
        }
    };
    match state {
        None => "Not installed",
        Some(ServiceState::Running) => "Running",
        Some(ServiceState::StartPending | ServiceState::ContinuePending) => "Starting…",
        Some(ServiceState::StopPending | ServiceState::PausePending) => "Stopping…",
        Some(ServiceState::Stopped | ServiceState::Paused) => "Installed, not running",
    }
}

pub fn installed() -> bool {
    status_text() != "Not installed"
}

pub fn is_running() -> bool {
    status_text() == "Running"
}

/// Keeps an installed service running while the app is open: the service
/// is this computer's host, so when it's stopped nobody can reach it.
/// Started without the administrator prompt (see SERVICE_SDDL); gives up
/// after a few failures in a row, which then show on the Config page.
pub fn keep_running() {
    std::thread::spawn(|| {
        let mut failures = 0;
        let mut was_busy = false;
        loop {
            // Never while the app installs, starts or removes it (an
            // install stops the service to replace the program: starting
            // it then would run the old one), nor just after.
            let busy = BUSY.load(std::sync::atomic::Ordering::Relaxed);
            if !busy && !was_busy && !update_in_progress() && state() == Some(ServiceState::Stopped) && failures < 3 {
                match start() {
                    Ok(()) => {
                        tracing::info!("started the Slipstream service, which wasn't running");
                        failures = 0;
                    }
                    Err(e) => {
                        failures += 1;
                        tracing::warn!("the Slipstream service isn't running and didn't start: {e:#}");
                        set_problem(Some(format!("{e:#}")));
                    }
                }
            }
            was_busy = busy;
            std::thread::sleep(Duration::from_secs(20));
        }
    });
}

/// Set while the app installs, starts or removes the service.
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_busy(on: bool) {
    BUSY.store(on, std::sync::atomic::Ordering::Relaxed);
}

static PROBLEM: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Remembers why the service couldn't be installed or started (`None`
/// once it worked), for the Bridge and Config to show.
pub fn set_problem(why: Option<String>) {
    *PROBLEM.lock().unwrap() = why;
}

/// Why the service isn't running, if the app found out.
pub fn start_problem() -> Option<String> {
    if is_running() {
        return None;
    }
    PROBLEM.lock().unwrap().clone()
}

/// The service's log: where this build writes it, or where builds before
/// 34 did (administrators only).
pub fn log_to_show() -> PathBuf {
    let new = log_path();
    if new.exists() { new } else { machine_home().join("slipstream.log") }
}

/// Starts the installed service and checks that it keeps running. Works
/// without administrator rights once [`install`] has let users start it.
pub fn start() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::START).context("opening the Slipstream service")?;
    if service.query_status()?.current_state != ServiceState::Running
        && let Err(e) = service.start::<&str>(&[])
    {
        bail!("Windows didn't start the service: {:#}\n{}", anyhow::Error::from(e), diagnose());
    }
    wait_for(&service, ServiceState::Running, 20);
    std::thread::sleep(Duration::from_secs(3));
    let now = service.query_status()?;
    if now.current_state != ServiceState::Running {
        bail!("the service started but stopped again ({:?}, exit {:?})\n{}", now.current_state, now.exit_code, diagnose());
    }
    Ok(())
}

/// Windows' default service permissions, plus: signed-in users may start
/// this one (not stop or change it), so the app can bring it back up
/// without the administrator prompt.
const SERVICE_SDDL: &str = "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWRPLOCRRC;;;IU)(A;;CCLCSWLOCRRC;;;SU)";

/// The last few lines of the service's log, to explain a failed start.
fn log_tail(lines: usize) -> String {
    let log = std::fs::read_to_string(log_path()).unwrap_or_default();
    let all: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// A console tool (reg, icacls, sc…), without a console window flashing up.
fn quiet(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// What Windows says about the service, for when it won't run: its state
/// and exit code, the command it starts, and the Service Control Manager's
/// messages about it in the last hour (the System event log).
pub fn diagnose() -> String {
    let run = |args: &[&str]| {
        quiet(args[0]).args(&args[1..]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    };
    let pick = |text: &str, keys: &[&str]| {
        text.lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|l| keys.iter().any(|k| l.starts_with(k)))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let mut out = vec![
        pick(&run(&["sc", "queryex", NAME]), &["STATE", "WIN32_EXIT_CODE", "SERVICE_EXIT_CODE"]),
        pick(&run(&["sc", "qc", NAME]), &["START_TYPE", "BINARY_PATH_NAME"]),
    ];
    let events = run(&[
        "wevtutil",
        "qe",
        "System",
        "/q:*[System[Provider[@Name='Service Control Manager'] and TimeCreated[timediff(@SystemTime) <= 3600000]]]",
        "/c:40",
        "/rd:true",
        "/f:text",
    ]);
    for e in events.split("Event[").filter(|e| e.contains("Slipstream")).take(3) {
        let date = e.lines().find_map(|l| l.trim().strip_prefix("Date:")).unwrap_or("").trim().to_string();
        if let Some(d) = e.split("Description:").nth(1) {
            out.push(format!("Windows ({date}): {}", d.split_whitespace().collect::<Vec<_>>().join(" ")));
        }
    }
    out.retain(|l| !l.is_empty());
    out.join("\n")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Runs this program with `args` as administrator (the UAC prompt) and
/// waits for it. Returns its exit code. For the GUI's install button.
pub fn run_elevated(args: &str) -> Result<u32> {
    use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    struct Apartment(bool);
    impl Drop for Apartment {
        fn drop(&mut self) { if self.0 { unsafe { CoUninitialize(); } } }
    }
    // ShellExecute may use COM shell extensions on this worker thread.
    let _apartment = Apartment(unsafe { CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) } >= 0);
    let exe = wide(&std::env::current_exe()?.display().to_string());
    let (verb, params) = (wide("runas"), wide(args));
    // SAFETY: ShellExecuteExW with valid strings; the process handle it
    // returns is waited on and closed.
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = exe.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = 0; // SW_HIDE
        if ShellExecuteExW(&mut info) == 0 || info.hProcess.is_null() {
            bail!("the administrator prompt was cancelled");
        }
        WaitForSingleObject(info.hProcess, u32::MAX);
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        Ok(code)
    }
}

/// Waits for the service to reach `want` (or gives up after `secs`).
fn wait_for(service: &windows_service::service::Service, want: ServiceState, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if service.query_status().is_ok_and(|s| s.current_state == want) {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Starts the app (hidden, in the tray) at every user's sign-in while the
/// service is installed.
const RUN_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Slipstream Remote tray";

/// `slipstream service install`: needs administrator rights.
pub fn install(password: Option<&str>, from: Option<&Path>) -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
        .context("installing the service needs administrator rights (run it as administrator)")?;

    // An existing service is stopped and reconfigured.
    let existing = manager
        .open_service(NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .ok();
    if let Some(s) = &existing
        && s.query_status().is_ok_and(|st| st.current_state != ServiceState::Stopped)
    {
        let _ = s.stop();
        wait_for(s, ServiceState::Stopped, 20);
    }

    // The program, somewhere that outlives the download folder.
    let exe = std::env::current_exe()?;
    let dir = install_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let installed = dir.join("slipstream.exe");
    if exe != installed {
        replace_exe(&exe, &installed)?;
    }

    // The machine profile, from this computer's settings.
    let user = match from {
        Some(dir) => Store::open(dir)?,
        None => Store::open_default()?,
    };
    user.identity()?; // Bind/refresh an imaged user profile before copying its settings.
    let mut settings = user.load_settings();
    if settings.server.url.is_none() {
        bail!("connect Slipstream to your station first (it has no server address)");
    }
    if let Some(pw) = password {
        settings.host.set_permanent_password(Some(pw));
    }
    if settings.host.permanent_secret().is_none() {
        bail!("set an unattended password first (Config → Unattended password, or --password) for access before sign-in; signed-in users can also share the service's one-time code");
    }
    settings.host.enabled = true;
    let home = machine_home();
    std::fs::create_dir_all(&home)?;
    // SYSTEM and administrators only: the profile holds the device key and
    // the password digest.
    let acl = quiet("icacls")
        .arg(&home)
        .args(["/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "/grant:r", "*S-1-5-32-544:(OI)(CI)F", "/Q"])
        .status()?;
    if !acl.success() {
        bail!("couldn't restrict access to {}", home.display());
    }
    repair_permissions();
    // Everyone may read the ID the service hosts as (see hosted_id); the
    // old one is gone until the service has registered again.
    let public = ensure_public_dir();
    let _ = std::fs::remove_file(public.join("device-id.txt"));
    let machine = Store::open(&home)?;
    // The installed service owns its existing machine identity. After cloning,
    // the user and service profiles may have independently rekeyed; a routine
    // update must not replace the service's new stable ID with the user's key.
    if home.join("identity.key").exists() {
        machine.identity()?;
        let old = machine.load_settings();
        settings.host.last_device_id = if old.server.url == settings.server.url { old.host.last_device_id } else { None };
        settings.host.public_device_id = old.host.public_device_id;
    } else {
        hand_over_identity(&user.dir().join("identity.key"), &home.join("identity.key"))?;
        settings.host.last_device_id=None;
        settings.host.public_device_id=None;
    }
    machine.save_settings(&settings)?;
    machine.identity()?; // Establish the machine binding before image capture.

    // Every user's sign-in starts the app hidden in the tray, to show the
    // service is there (each user can turn it off under Config).
    let _ = quiet("reg")
        .args(["add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d"])
        .arg(format!("\"{}\" --minimized", installed.display()))
        .arg("/f")
        .status();

    // Let a service generate Ctrl+Alt+Del (SoftwareSASGeneration = services).
    let _ = quiet("reg")
        .args([
            "add",
            r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System",
            "/v",
            "SoftwareSASGeneration",
            "/t",
            "REG_DWORD",
            "/d",
            "1",
            "/f",
        ])
        .status();

    let info = ServiceInfo {
        name: NAME.into(),
        display_name: DISPLAY_NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: installed.clone(),
        launch_arguments: vec![OsString::from("service"), OsString::from("run")],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let service = match existing {
        Some(s) => {
            s.change_config(&info)?;
            s
        }
        None => manager.create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS)?,
    };
    // Nice to have, so not fatal (Wine, for one, lacks some of these).
    match quiet("sc").args(["sdset", NAME, SERVICE_SDDL]).output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => tracing::warn!("letting users start the service: {}", String::from_utf8_lossy(&o.stdout).trim()),
        Err(e) => tracing::warn!("letting users start the service: {e}"),
    }
    if let Err(e) = service.set_description("Lets your support provider reach this computer, including the login screen and administrator prompts.") {
        tracing::warn!("service description: {:#}", anyhow::Error::from(e));
    }
    // Restart after crashes, and after it exits to load an updated program.
    let restart = ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(3) };
    let failure = service
        .update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
            reboot_msg: None,
            command: None,
            actions: Some(vec![restart.clone(), restart.clone(), restart]),
        })
        .and_then(|()| service.set_failure_actions_on_non_crash_failures(true));
    if let Err(e) = failure {
        tracing::warn!("service restart policy (updates then need a manual restart): {:#}", anyhow::Error::from(e));
    }
    if let Err(e) = service.start::<&str>(&[]) {
        bail!("Windows didn't start the service: {:#}\n{}", anyhow::Error::from(e), diagnose());
    }
    wait_for(&service, ServiceState::Running, 20);
    // Check it stays up: one that stops at once would otherwise look
    // installed, and the app wouldn't say why.
    std::thread::sleep(Duration::from_secs(3));
    let now = service.query_status().context("checking on the service")?;
    if now.current_state != ServiceState::Running {
        let tail = log_tail(4);
        bail!(
            "the service was installed but didn't keep running ({:?}, exit {:?})\n{}{}",
            now.current_state,
            now.exit_code,
            diagnose(),
            if tail.is_empty() { String::new() } else { format!("\nIts log ends:\n{tail}") }
        );
    }
    println!("Slipstream service installed and running ({}).", installed.display());
    if let Some(id) = settings.host.last_device_id {
        println!("This computer is reachable as ID {id} with its unattended password.");
    }
    Ok(())
}

/// Gives the service this computer's identity (so it keeps the app's ID).
/// Put in place as a fresh file, like the settings, never written over the
/// old one: a copy left by an earlier install or by the service itself can
/// refuse writes (read-only, or permissions only SYSTEM has), while
/// replacing it in this administrators' folder always works.
fn hand_over_identity(key: &Path, dest: &Path) -> Result<()> {
    let bytes = std::fs::read(key).with_context(|| format!("reading this computer's identity ({})", key.display()))?;
    if std::fs::read(dest).is_ok_and(|have| have == bytes) {
        return Ok(());
    }
    let tmp = dest.with_extension("new");
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing the service's identity ({})", tmp.display()))?;
    if let Ok(meta) = std::fs::metadata(dest)
        && meta.permissions().readonly()
    {
        let mut p = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        let _ = std::fs::set_permissions(dest, p);
    }
    if let Err(e) = std::fs::rename(&tmp, dest) {
        // Last resort for a copy nobody may replace: administrators can
        // take it over, then remove it.
        let _ = quiet("takeown").arg("/F").arg(dest).arg("/A").output();
        let _ = quiet("icacls").arg(dest).args(["/grant", "*S-1-5-32-544:F", "/Q"]).output();
        let _ = std::fs::remove_file(dest);
        std::fs::rename(&tmp, dest).map_err(|_| e).with_context(|| format!("replacing the service's identity ({})", dest.display()))?;
    }
    if std::fs::read(dest).ok().as_deref() != Some(bytes.as_slice()) {
        bail!("the service's copy of this computer's identity doesn't match (it would get a new ID)");
    }
    Ok(())
}

/// Copies the app over the installed one. That one is usually still running
/// (the tray icon, or the old service's host finishing up), and Windows
/// won't overwrite a running program but will rename it: so it's moved
/// aside, and removed at the next install once it has exited.
fn replace_exe(exe: &Path, installed: &Path) -> Result<()> {
    let dir = installed.parent().unwrap_or(Path::new("."));
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().ends_with(".old") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let mut last = None;
    for attempt in 0..10 {
        match std::fs::copy(exe, installed) {
            Ok(_) => return Ok(()),
            Err(e) => last = Some(e),
        }
        if installed.exists() {
            let aside = dir.join(format!("slipstream-{}-{attempt}.exe.old", std::process::id()));
            if std::fs::rename(installed, &aside).is_ok() {
                continue;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let e = last.map(anyhow::Error::from).unwrap_or_else(|| anyhow::anyhow!("unknown error"));
    Err(e.context(format!("replacing {} (close any Slipstream windows and try again)", installed.display())))
}

/// `slipstream service uninstall`: needs administrator rights.
pub fn uninstall() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("removing the service needs administrator rights")?;
    let service = manager
        .open_service(NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE)
        .context("the Slipstream service isn't installed (or this needs administrator rights)")?;
    if service.query_status().is_ok_and(|s| s.current_state != ServiceState::Stopped) {
        let _ = service.stop();
        wait_for(&service, ServiceState::Stopped, 20);
    }
    service.delete()?;
    drop(service);
    // The profile holds the device key and password digest: remove it.
    let _ = std::fs::remove_dir_all(machine_home());
    let installed = install_dir().join("slipstream.exe");
    if std::env::current_exe().ok().as_deref() != Some(&installed) {
        let _ = std::fs::remove_file(&installed);
        if let Ok(entries) = std::fs::read_dir(install_dir()) {
            for e in entries.flatten() {
                if e.file_name().to_string_lossy().ends_with(".old") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let _ = std::fs::remove_dir(install_dir());
    }
    let _ = quiet("reg")
        .args(["delete", r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System", "/v", "SoftwareSASGeneration", "/f"])
        .status();
    let _ = quiet("reg").args(["delete", RUN_KEY, "/v", RUN_VALUE, "/f"]).status();
    println!("Slipstream service removed.");
    Ok(())
}

// ── The service itself ─────────────────────────────────────────────────────

define_windows_service!(ffi_service_main, service_main);

/// `slipstream service run`: hands over to the service control manager.
pub fn run() -> Result<()> {
    service_dispatcher::start(NAME, ffi_service_main).context("not started by the service manager (use `slipstream service install`)")?;
    Ok(())
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!("service: {e:#}");
    }
}

fn run_service() -> Result<()> {
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let status = service_control_handler::register(NAME, handler)?;
    let report = |state, exit: u32| {
        let _ = status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code: if exit == 0 { ServiceExitCode::Win32(0) } else { ServiceExitCode::ServiceSpecific(exit) },
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });
    };
    report(ServiceState::Running, 0);
    repair_permissions();
    ensure_public_dir();
    tracing::info!("service started (build {})", slipstream_core::BUILD_NUMBER);
    let exit = supervise(&stop_rx);
    tracing::info!("service stopped{}", if exit == 0 { ": Windows or an administrator asked it to" } else { ", to restart" });
    report(ServiceState::Stopped, exit);
    Ok(())
}

/// The host process in a console session.
struct Child {
    process: HANDLE,
    session: u32,
    started: Instant,
}

impl Child {
    fn running(&self) -> bool {
        let mut code = 0u32;
        // SAFETY: a live process handle.
        unsafe { GetExitCodeProcess(self.process, &mut code) != 0 && code == STILL_ACTIVE as u32 }
    }

    fn stop(self) {
        // SAFETY: a live process handle, closed afterwards.
        unsafe {
            TerminateProcess(self.process, 0);
            WaitForSingleObject(self.process, 3000);
            CloseHandle(self.process);
        }
    }
}

/// Keeps a host running in the active console session until stopped.
/// Returns the exit code: non-zero asks the service manager to restart us
/// (after an update replaced the program).
fn supervise(stop: &std::sync::mpsc::Receiver<()>) -> u32 {
    let exe = std::env::current_exe().unwrap_or_default();
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let started_as: Option<SystemTime> = modified(&exe);
    let sas_name = wide(slipstream_platform::windesk::SAS_EVENT);
    // SAFETY: an auto-reset named event; only SYSTEM processes (our host) can
    // open it with the default security.
    let sas = unsafe { CreateEventW(std::ptr::null(), 0, 0, sas_name.as_ptr()) };
    // Hosts live in a job that dies with us, so a crashed service can't
    // leave one running beside the next.
    let job = kill_on_close_job();
    let mut child: Option<Child> = None;
    let (mut published, mut next_publish) = (None, Instant::now());
    let mut backoff = Duration::from_secs(1);
    let mut next_spawn = Instant::now();
    loop {
        if stop.try_recv().is_ok() {
            if let Some(c) = child.take() {
                c.stop();
            }
            return 0;
        }
        if modified(&exe) != started_as {
            tracing::info!("the program was updated: restarting the service");
            if let Some(c) = child.take() {
                c.stop();
            }
            return 1;
        }
        if Instant::now() >= next_publish {
            publish_id(&mut published);
            next_publish = Instant::now() + Duration::from_secs(5);
        }
        // SAFETY: plain query.
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        if let Some(c) = &child
            && (!c.running() || c.session != session)
        {
            let c = child.take().expect("checked");
            if c.started.elapsed() < Duration::from_secs(10) {
                backoff = (backoff * 2).min(Duration::from_secs(60));
            } else {
                backoff = Duration::from_secs(1);
            }
            tracing::info!(session = c.session, "host stopped or the session changed");
            c.stop();
            next_spawn = Instant::now() + backoff;
        }
        if child.is_none() && session != u32::MAX && Instant::now() >= next_spawn {
            match spawn_host(&exe, session) {
                Ok(process) => {
                    // SAFETY: live job and process handles.
                    if !job.is_null() {
                        unsafe { AssignProcessToJobObject(job, process) };
                    }
                    tracing::info!(session, "host started");
                    child = Some(Child { process, session, started: Instant::now() });
                }
                Err(e) => {
                    tracing::warn!("starting the host: {e:#}");
                    next_spawn = Instant::now() + Duration::from_secs(5);
                }
            }
        }
        // Wait for a Ctrl+Alt+Del request, or half a second.
        // SAFETY: a live event handle.
        if !sas.is_null() && unsafe { WaitForSingleObject(sas, 500) } == WAIT_OBJECT_0 {
            send_sas();
        } else if sas.is_null() {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

/// A job object whose processes are killed when its last handle (ours)
/// closes; null if it can't be made.
fn kill_on_close_job() -> HANDLE {
    // SAFETY: creating and configuring an anonymous job object.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return job;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const info).cast(), size_of_val(&info) as u32);
        job
    }
}

/// Starts `exe service host` in `session` as SYSTEM.
fn spawn_host(exe: &Path, session: u32) -> Result<HANDLE> {
    let mut cmd = wide(&format!("\"{}\" service host", exe.display()));
    let desktop = wide("winsta0\\default");
    // SAFETY: token duplication and process creation with valid, owned
    // handles; all strings outlive the calls.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut token) == 0 {
            bail!("OpenProcessToken failed");
        }
        let mut dup: HANDLE = std::ptr::null_mut();
        let ok = DuplicateTokenEx(token, TOKEN_ALL_ACCESS, std::ptr::null(), SecurityImpersonation, TokenPrimary, &mut dup);
        CloseHandle(token);
        if ok == 0 {
            bail!("DuplicateTokenEx failed");
        }
        let sid = session;
        if SetTokenInformation(dup, TokenSessionId, (&raw const sid).cast(), 4) == 0 {
            CloseHandle(dup);
            bail!("SetTokenInformation failed");
        }
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        si.lpDesktop = desktop.as_ptr().cast_mut();
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let ok = CreateProcessAsUserW(
            dup,
            std::ptr::null(),
            cmd.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_NO_WINDOW,
            std::ptr::null(),
            std::ptr::null(),
            &si,
            &mut pi,
        );
        CloseHandle(dup);
        if ok == 0 {
            bail!("CreateProcessAsUser failed ({})", std::io::Error::last_os_error());
        }
        CloseHandle(pi.hThread);
        Ok(pi.hProcess)
    }
}

/// Generates Ctrl+Alt+Del (allowed for services by SoftwareSASGeneration).
fn send_sas() {
    let lib = wide("sas.dll");
    // SAFETY: loading sas.dll and calling SendSAS(FALSE) as documented.
    unsafe {
        let module = LoadLibraryW(lib.as_ptr());
        if module.is_null() {
            tracing::warn!("sas.dll not found: Ctrl+Alt+Del unavailable");
            return;
        }
        match GetProcAddress(module, c"SendSAS".as_ptr().cast()) {
            Some(f) => {
                let send: unsafe extern "system" fn(i32) = std::mem::transmute(f);
                send(0);
                tracing::info!("sent Ctrl+Alt+Del");
            }
            None => tracing::warn!("SendSAS not found"),
        }
    }
}

/// Open the chat app as the signed-in user, with that user's environment/profile.
/// The service's SYSTEM profile and privileges must never reach this window.
pub(crate) fn launch_user_chat(session: u32) -> Result<()> {
    use windows_sys::Win32::System::{RemoteDesktop::WTSQueryUserToken, Environment::{CreateEnvironmentBlock,DestroyEnvironmentBlock},Threading::CREATE_UNICODE_ENVIRONMENT};
    let exe=std::env::current_exe()?;
    let application=wide(&exe.to_string_lossy());
    let mut command=wide(&format!("\"{}\" --minimized --service-chat",exe.display()));
    let desktop=wide("winsta0\\default");
    unsafe {
        let mut token=std::ptr::null_mut();
        if WTSQueryUserToken(session,&mut token)==0 {bail!("no signed-in user for chat: {}",std::io::Error::last_os_error());}
        let mut environment=std::ptr::null_mut();
        if CreateEnvironmentBlock(&mut environment,token,0)==0 {let error=std::io::Error::last_os_error();CloseHandle(token);return Err(error.into());}
        let mut startup:STARTUPINFOW=std::mem::zeroed();startup.cb=size_of::<STARTUPINFOW>() as u32;startup.lpDesktop=desktop.as_ptr().cast_mut();
        let mut process:PROCESS_INFORMATION=std::mem::zeroed();
        let ok=CreateProcessAsUserW(token,application.as_ptr(),command.as_mut_ptr(),std::ptr::null(),std::ptr::null(),0,CREATE_UNICODE_ENVIRONMENT|CREATE_NO_WINDOW,environment,std::ptr::null(),&startup,&mut process);
        let error=std::io::Error::last_os_error();DestroyEnvironmentBlock(environment);CloseHandle(token);
        if ok==0 {return Err(error.into());}
        CloseHandle(process.hThread);CloseHandle(process.hProcess);
    }
    Ok(())
}
