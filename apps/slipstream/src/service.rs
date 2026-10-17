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

/// The machine profile the service's host uses.
pub fn machine_home() -> PathBuf {
    let base = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| r"C:\ProgramData".into());
    base.join("Slipstream Remote")
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

pub fn running() -> bool {
    state() == Some(ServiceState::Running)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Runs this program with `args` as administrator (the UAC prompt) and
/// waits for it. Returns its exit code. For the GUI's install button.
pub fn run_elevated(args: &str) -> Result<u32> {
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
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
        std::fs::copy(&exe, &installed).with_context(|| format!("copying the app to {}", installed.display()))?;
    }

    // The machine profile, from this computer's settings.
    let user = match from {
        Some(dir) => Store::open(dir)?,
        None => Store::open_default()?,
    };
    let mut settings = user.load_settings();
    if settings.server.url.is_none() {
        bail!("connect Slipstream to your station first (it has no server address)");
    }
    if let Some(pw) = password {
        settings.host.set_permanent_password(Some(pw));
    }
    if settings.host.permanent_secret().is_none() {
        bail!("set an unattended password first (Config → Unattended password, or --password): the service has no one-time passwords to show");
    }
    settings.host.enabled = true;
    let home = machine_home();
    std::fs::create_dir_all(&home)?;
    // SYSTEM and administrators only: the profile holds the device key and
    // the password digest.
    let acl = Command::new("icacls")
        .arg(&home)
        .args(["/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "/grant:r", "*S-1-5-32-544:(OI)(CI)F", "/T", "/Q"])
        .status()?;
    if !acl.success() {
        bail!("couldn't restrict access to {}", home.display());
    }
    let machine = Store::open(&home)?;
    machine.save_settings(&settings)?;
    // Same identity, so the computer keeps its ID.
    let key = user.dir().join("identity.key");
    if key.exists() {
        std::fs::copy(&key, home.join("identity.key"))?;
    }

    // Let a service generate Ctrl+Alt+Del (SoftwareSASGeneration = services).
    let _ = Command::new("reg")
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
    if let Err(e) = service.set_description("Lets your support provider reach this computer, including the login screen and administrator prompts.") {
        tracing::warn!("service description: {e}");
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
        tracing::warn!("service restart policy (updates then need a manual restart): {e}");
    }
    service.start::<&str>(&[])?;
    wait_for(&service, ServiceState::Running, 20);
    println!("Slipstream service installed and running ({}).", installed.display());
    if let Some(id) = settings.host.last_device_id {
        println!("This computer is reachable as ID {id} with its unattended password.");
    }
    Ok(())
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
        let _ = std::fs::remove_dir(install_dir());
    }
    let _ = Command::new("reg")
        .args(["delete", r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System", "/v", "SoftwareSASGeneration", "/f"])
        .status();
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
    tracing::info!("service started");
    let exit = supervise(&stop_rx);
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
