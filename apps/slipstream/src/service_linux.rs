//! Linux service: unattended access including the login screen and locked
//! sessions.
//!
//! A systemd unit runs `slipstream service run` as root. That one process
//! is this computer's host. It sees the screen as the graphics card shows
//! it and types through kernel input devices
//! (`slipstream_platform::system`), so it reaches whatever is on the
//! displays: the login screen, a locked session, any Wayland or X11
//! desktop, a text console. No desktop permission prompt is involved.
//!
//! The host uses a machine profile in `/var/lib`, readable only by root.
//! `install` copies this computer's station settings, identity (so the ID
//! stays the same) and unattended password into it, and the program into
//! `/opt` (root's own copy: the service must not run a file a user can
//! replace). It updates itself from signed releases like any headless host.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use slipstream_core::{config::Store, directory::DeviceId};

pub const UNIT: &str = "slipstream-remote.service";
const UNIT_PATH: &str = "/etc/systemd/system/slipstream-remote.service";
const INSTALL_DIR: &str = "/opt/slipstream-remote";
const PUBLIC_DIR: &str = "/run/slipstream-remote";
const LOG_DIR: &str = "/var/log/slipstream-remote";

/// The machine profile the service's host uses.
pub fn machine_home() -> PathBuf {
    "/var/lib/slipstream-remote".into()
}

/// The service's log (readable by everyone: it holds no secrets, and
/// whoever looks after this computer should be able to read it).
pub fn log_path() -> PathBuf {
    Path::new(LOG_DIR).join("slipstream.log")
}

pub fn log_to_show() -> PathBuf {
    log_path()
}

/// Run by the service once its log is open: lets everyone read it (the
/// service's files are private by default).
pub fn share_log() {
    let _ = std::fs::set_permissions(log_path(), std::fs::Permissions::from_mode(0o644));
}

fn program() -> PathBuf {
    Path::new(INSTALL_DIR).join("slipstream")
}

fn unit_text() -> String {
    format!(
        "[Unit]\n\
         Description=Slipstream Remote (unattended access, including the login screen)\n\
         Documentation=https://github.com/Psychronic-Games/Slipstream-Remote\n\
         Wants=network-online.target\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         ExecStartPre=-/sbin/modprobe uinput\n\
         ExecStart={program} service run\n\
         Restart=always\n\
         RestartSec=3\n\
         UMask=0077\n\
         StateDirectory=slipstream-remote\n\
         StateDirectoryMode=0700\n\
         RuntimeDirectory=slipstream-remote\n\
         RuntimeDirectoryMode=0755\n\
         LogsDirectory=slipstream-remote\n\
         LogsDirectoryMode=0755\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        program = program().display()
    )
}

fn systemctl(args: &[&str]) -> Result<std::process::Output> {
    Command::new("systemctl").args(args).output().context("running systemctl (the service needs a system with systemd)")
}

fn is_root() -> bool {
    // SAFETY: a plain call.
    unsafe { libc::geteuid() == 0 }
}

/// Installed, whether running or not: either way it is this computer's
/// host, so the app mustn't host as well (two hosts with one identity push
/// each other off).
pub fn installed() -> bool {
    Path::new(UNIT_PATH).exists()
}

/// What systemd says ("active", "failed", …), looked up at most every two
/// seconds; `None` if it isn't installed.
fn state() -> Option<String> {
    static SEEN: std::sync::Mutex<Option<(Instant, Option<String>)>> = std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().unwrap();
    if let Some((at, state)) = &*seen
        && at.elapsed() < Duration::from_secs(2)
    {
        return state.clone();
    }
    let state = installed().then(|| {
        systemctl(&["is-active", UNIT])
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    });
    *seen = Some((Instant::now(), state.clone()));
    state
}

/// The service's state in words, for the Bridge and Config pages.
pub fn status_text() -> &'static str {
    match state().as_deref() {
        None => "Not installed",
        Some("active") => "Running",
        Some("activating" | "reloading") => "Starting…",
        Some("deactivating") => "Stopping…",
        Some(_) => "Installed, not running",
    }
}

pub fn is_running() -> bool {
    status_text() == "Running"
}

fn id_file(public: bool) -> PathBuf {
    Path::new(PUBLIC_DIR).join(if public { "public-id.txt" } else { "device-id.txt" })
}

/// The ID the service hosts this computer as (usually the app's own) on
/// its own station, or on Slipstream Public, while it is registered there.
/// Looked up at most every two seconds.
pub fn hosted_id(public: bool) -> Option<DeviceId> {
    static SEEN: std::sync::Mutex<Option<(Instant, [Option<DeviceId>; 2])>> = std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().unwrap();
    if let Some((at, ids)) = *seen
        && at.elapsed() < Duration::from_secs(2)
    {
        return ids[public as usize];
    }
    let read = |public| std::fs::read_to_string(id_file(public)).ok().and_then(|s| s.trim().parse().ok());
    let ids = [read(false), read(true)];
    *seen = Some((Instant::now(), ids));
    ids[public as usize]
}

/// Run by the service's host: tells signed-in users which IDs it
/// registered as, on the computer's own station and on Slipstream Public.
pub fn publish_ids(settings: &slipstream_core::config::Settings) {
    let custom = settings
        .server
        .url
        .as_deref()
        .is_some_and(|url| !url.trim().is_empty() && !slipstream_core::config::is_public_station(url));
    for (public, id) in
        [(false, settings.host.last_device_id.filter(|_| custom)), (true, settings.host.public_device_id)]
    {
        let path = id_file(public);
        let text = id.map(|id| id.0.to_string()).unwrap_or_default();
        if std::fs::read_to_string(&path).unwrap_or_default() == text {
            continue;
        }
        // The service's files are private by default (its umask).
        if std::fs::write(&path, text).is_ok() {
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
        }
    }
}

fn screen_problem_file() -> PathBuf {
    Path::new(PUBLIC_DIR).join("screen-problem.txt")
}

/// Who hosts now ("service" or "desktop"), written by the service.
pub fn host_file() -> PathBuf {
    Path::new(PUBLIC_DIR).join("host.txt")
}

static SCREEN_READABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// In the service: whether it could read the screen when last checked
/// (see [`watch_screen`]).
pub fn screen_readable() -> bool {
    SCREEN_READABLE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether a signed-in desktop is hosting in the running service's place
/// (see `service_desktop`). Looked up at most every two seconds.
pub fn hosted_from_desktop() -> bool {
    static SEEN: std::sync::Mutex<Option<(Instant, bool)>> = std::sync::Mutex::new(None);
    if !is_running() {
        return false;
    }
    let mut seen = SEEN.lock().unwrap();
    if let Some((at, desktop)) = *seen
        && at.elapsed() < Duration::from_secs(2)
    {
        return desktop;
    }
    let desktop = std::fs::read_to_string(host_file()).is_ok_and(|s| s.trim() == "desktop");
    *seen = Some((Instant::now(), desktop));
    desktop
}

/// Run by the app in a desktop session while the service is installed:
/// starts the helper that hosts from this desktop when the service lends
/// it the place (`slipstream service desktop`). It ends with the app.
pub fn spawn_desktop_host(profile: &Path) {
    use std::os::unix::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else { return };
    let mut command = Command::new(exe);
    command
        .args(["service", "desktop", "--from"])
        .arg(profile)
        // A profile of its own: two programs can't share a settings file.
        .env("SLIPSTREAM_HOME", profile.join("desktop-host"))
        .env_remove("SLIPSTREAM_LOG_DIR")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: one async-signal-safe call between fork and exec.
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    match command.spawn() {
        Ok(mut child) => {
            // Stays alive for the app's whole life: the "parent died"
            // signal follows the thread that started the helper.
            let _ = std::thread::Builder::new().name("desktop-host".into()).spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => tracing::warn!("desktop host not started: {e}"),
    }
}

/// Run by the service: keeps telling signed-in users' apps whether it can
/// show this computer's screen (the login screen and locked sessions
/// depend on it).
pub fn watch_screen() {
    let watch = || {
        // Let the login screen take the graphics card first.
        std::thread::sleep(Duration::from_secs(15));
        let mut said: Option<String> = None;
        loop {
            let problem = slipstream_platform::kms::unreadable_reason();
            SCREEN_READABLE.store(problem.is_none(), std::sync::atomic::Ordering::Relaxed);
            if problem != said {
                match &problem {
                    Some(why) => tracing::warn!("this computer's screen can't be shown: {why}"),
                    None => tracing::info!("this computer's screen can be shown"),
                }
                let path = screen_problem_file();
                if std::fs::write(&path, problem.as_deref().unwrap_or_default()).is_ok() {
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
                }
                said = problem;
            }
            std::thread::sleep(Duration::from_secs(60));
        }
    };
    if let Err(e) = std::thread::Builder::new().name("screen-check".into()).spawn(watch) {
        tracing::warn!("screen check not started: {e}");
    }
}

/// Why the running service can't show this computer's screen, if it has
/// said so. Looked up at most every two seconds.
pub fn screen_problem() -> Option<String> {
    static SEEN: std::sync::Mutex<Option<(Instant, Option<String>)>> = std::sync::Mutex::new(None);
    if !is_running() {
        return None;
    }
    let mut seen = SEEN.lock().unwrap();
    if let Some((at, problem)) = &*seen
        && at.elapsed() < Duration::from_secs(2)
    {
        return problem.clone();
    }
    let problem =
        std::fs::read_to_string(screen_problem_file()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    *seen = Some((Instant::now(), problem.clone()));
    problem
}

/// Set while the app installs or removes the service.
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

/// Runs this program with `args` as root, through the desktop's
/// administrator prompt (pkexec), and waits. Returns its exit code.
pub fn run_elevated(args: &[String]) -> Result<i32> {
    let exe = std::env::current_exe()?;
    let mut command = if is_root() {
        Command::new(&exe)
    } else {
        let mut c = Command::new("pkexec");
        c.arg(&exe);
        c
    };
    let status = command
        .args(args)
        .stdin(std::process::Stdio::null())
        .status()
        .context("couldn't ask for administrator rights (pkexec isn't installed; run `sudo slipstream service install` in a terminal instead)")?;
    match status.code() {
        Some(126) => bail!("the administrator prompt was cancelled"),
        Some(127) => bail!("this account isn't allowed to administer this computer"),
        Some(code) => Ok(code),
        None => bail!("the administrator helper was stopped"),
    }
}

/// What systemd and the service's log say, to explain a service that
/// won't run.
pub fn diagnose() -> String {
    let mut out = Vec::new();
    if let Ok(o) = systemctl(&["show", UNIT, "--property=ActiveState,SubState,Result,ExecMainStatus"]) {
        out.push(String::from_utf8_lossy(&o.stdout).split_whitespace().collect::<Vec<_>>().join(" · "));
    }
    let log = std::fs::read_to_string(log_path()).unwrap_or_default();
    let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    if !lines.is_empty() {
        out.push(format!("Its log ends:\n{}", lines[lines.len().saturating_sub(4)..].join("\n")));
    }
    out.retain(|l| !l.is_empty());
    out.join("\n")
}

/// The settings folder of the person who asked for administrator rights
/// (sudo or pkexec leave root's own behind `HOME`).
fn invoking_users_settings() -> Option<PathBuf> {
    let uid: u32 = std::env::var("PKEXEC_UID").or_else(|_| std::env::var("SUDO_UID")).ok()?.parse().ok()?;
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    let home = passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.nth(2)?.parse::<u32>().ok()? == uid).then(|| fields.nth(2).map(PathBuf::from))?
    })?;
    let config = home.join(".config/slipstream");
    config.join("settings.toml").exists().then_some(config)
}

/// Puts `bytes` at `dest` as a fresh file with `mode`, never writing
/// through whatever is there now (a running program, a link).
fn place(bytes: &[u8], dest: &Path, mode: u32) -> Result<()> {
    let tmp = dest.with_extension("new");
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&tmp, dest).with_context(|| format!("putting {} in place", dest.display()))
}

/// A folder of root's own, not a link someone left there.
fn private_dir(dir: &Path, mode: u32) -> Result<()> {
    if std::fs::symlink_metadata(dir).is_ok_and(|m| !m.file_type().is_dir()) {
        bail!("{} is not a folder; remove it and install again", dir.display());
    }
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// `slipstream service install`: needs root.
pub fn install(password: Option<&str>, from: Option<&Path>) -> Result<()> {
    if !is_root() {
        bail!("installing the service needs administrator rights: run `sudo slipstream service install`");
    }
    if !Path::new("/run/systemd/system").exists() {
        bail!("the Slipstream service needs systemd, which this system doesn't run");
    }
    // Not a reason to refuse: while someone is signed in, their desktop
    // hosts (see `service_desktop`). But say what won't be reachable.
    if let Some(why) = slipstream_platform::kms::unreadable_reason() {
        println!("Note: the service can't show this computer's login screen or a locked session: {why}.");
        println!(
            "It stays reachable while someone is signed in with Slipstream running, through the desktop's own screen sharing."
        );
    }

    // The machine profile, from this computer's settings.
    let user = match from.map(Path::to_path_buf).or_else(invoking_users_settings) {
        Some(dir) => Store::open(dir)?,
        None => Store::open_default()?,
    };
    let key = user.dir().join("identity.key");
    let mut settings = user.load_settings();
    if let Some(pw) = password {
        settings.host.set_permanent_password(Some(pw));
    }
    if settings.host.permanent_secret().is_none() {
        bail!(
            "set an unattended password first (Config → Unattended password, or --password): it is what lets you in at the login screen"
        );
    }
    settings.host.enabled = true;
    // Screen-sharing approval belongs to the user's desktop session.
    settings.host.portal_restore_token = None;

    let was_installed = installed();
    if was_installed {
        let _ = systemctl(&["stop", UNIT]);
    }

    // The program, as root's own copy.
    let exe = std::env::current_exe()?;
    private_dir(Path::new(INSTALL_DIR), 0o755)?;
    if exe != program() {
        let bytes = std::fs::read(&exe).with_context(|| format!("reading {}", exe.display()))?;
        place(&bytes, &program(), 0o755)?;
    }

    let home = machine_home();
    private_dir(&home, 0o700)?;
    let machine = Store::open(&home)?;
    // The installed service owns its existing machine identity: a
    // reinstall keeps the ID it has been reachable as.
    if home.join("identity.key").exists() {
        let old = machine.load_settings();
        settings.host.last_device_id =
            if old.server.url == settings.server.url { old.host.last_device_id } else { None };
        settings.host.public_device_id = old.host.public_device_id;
    } else if key.exists() {
        let bytes =
            std::fs::read(&key).with_context(|| format!("reading this computer's identity ({})", key.display()))?;
        place(&bytes, &home.join("identity.key"), 0o600)?;
    } else {
        settings.host.last_device_id = None;
        settings.host.public_device_id = None;
    }
    machine.save_settings(&settings)?;
    machine.identity()?;

    place(unit_text().as_bytes(), Path::new(UNIT_PATH), 0o644)?;
    for args in [&["daemon-reload"][..], &["enable", UNIT], &["restart", UNIT]] {
        let done = systemctl(args)?;
        if !done.status.success() {
            bail!("systemctl {} failed: {}", args.join(" "), String::from_utf8_lossy(&done.stderr).trim());
        }
    }
    // Check it stays up: one that stops at once would otherwise look
    // installed, and the app wouldn't say why.
    std::thread::sleep(Duration::from_secs(3));
    let active = systemctl(&["is-active", UNIT])?;
    if String::from_utf8_lossy(&active.stdout).trim() != "active" {
        bail!("the service was installed but didn't keep running\n{}", diagnose());
    }
    println!("Slipstream service installed and running ({}).", program().display());
    println!("This computer is reachable with its unattended password, including at the login screen.");
    Ok(())
}

/// `slipstream service uninstall`: needs root.
pub fn uninstall() -> Result<()> {
    if !is_root() {
        bail!("removing the service needs administrator rights: run `sudo slipstream service uninstall`");
    }
    if !installed() {
        bail!("the Slipstream service isn't installed");
    }
    let _ = systemctl(&["disable", "--now", UNIT]);
    std::fs::remove_file(UNIT_PATH).with_context(|| format!("removing {UNIT_PATH}"))?;
    let _ = systemctl(&["daemon-reload"]);
    // The profile holds the device key and password digest: remove it.
    let _ = std::fs::remove_dir_all(machine_home());
    if std::env::current_exe().ok() != Some(program()) {
        let _ = std::fs::remove_dir_all(INSTALL_DIR);
    }
    println!("Slipstream service removed.");
    Ok(())
}

/// `slipstream service start`: asks systemd (which asks for administrator
/// rights itself when needed).
pub fn start() -> Result<()> {
    let done = systemctl(&["start", UNIT])?;
    if !done.status.success() {
        bail!("the service didn't start: {}\n{}", String::from_utf8_lossy(&done.stderr).trim(), diagnose());
    }
    Ok(())
}

/// `slipstream service check`: what the service would capture and control
/// on this computer, without installing anything. With `picture`, saves
/// what each display shows as a PNG. Run as root to test for real.
pub fn check(picture: Option<&Path>) -> Result<()> {
    use slipstream_platform::kms;
    if !is_root() {
        println!("Not running as root: the screen's memory can't be read. Use `sudo slipstream service check`.");
    }
    for line in kms::describe() {
        println!("{line}");
    }
    let screens = kms::screens()?;
    if screens.is_empty() {
        println!("No display is showing anything (the service wakes a blanked screen itself when someone connects).");
    }
    for screen in &screens {
        let d = &screen.info;
        match kms::snapshot(screen) {
            Ok((width, height, rgba)) => {
                // A picture that is all one colour usually means the
                // driver keeps the real one elsewhere.
                let flat = rgba.as_chunks::<4>().0.iter().all(|p| p[..3] == rgba[..3]);
                println!(
                    "{}: captured {width}x{height}{}",
                    d.name,
                    if flat { " (all one colour: blank screen, or not the real picture)" } else { "" }
                );
                if let Some(path) = picture {
                    let path = if screens.len() > 1 {
                        path.with_file_name(format!(
                            "{}-{}.png",
                            path.file_stem().unwrap_or_default().to_string_lossy(),
                            d.index + 1
                        ))
                    } else {
                        path.to_path_buf()
                    };
                    let mut png = png::Encoder::new(
                        std::fs::File::create(&path).with_context(|| format!("saving {}", path.display()))?,
                        width,
                        height,
                    );
                    png.set_color(png::ColorType::Rgba);
                    png.set_depth(png::BitDepth::Eight);
                    png.write_header()?.write_image_data(&rgba)?;
                    println!("  saved {}", path.display());
                }
            }
            Err(e) => println!("{}: {}x{}, not captured: {e:#}", d.name, d.width, d.height),
        }
    }
    match std::fs::File::options().write(true).open("/dev/uinput") {
        Ok(_) => println!("Keyboard and mouse: available (/dev/uinput)"),
        Err(e) => println!("Keyboard and mouse: /dev/uinput can't be opened ({e}); try `sudo modprobe uinput`"),
    }
    Ok(())
}
