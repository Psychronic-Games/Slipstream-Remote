//! Slipstream Remote desktop app.

// A GUI app on Windows: no console window when double-clicked.
#![cfg_attr(windows, windows_subsystem = "windows")]

use clap::{Parser, Subcommand};
use slipstream::{cli, gui};
#[cfg(windows)]
use slipstream::service;

#[derive(Debug, Parser)]
#[command(
    name = "slipstream",
    version = slipstream_core::VERSION_STRING,
    about = "Fast, free, open-source remote desktop",
    args_conflicts_with_subcommands = true
)]
struct Args {
    /// Share an animated test pattern instead of the real screen (demo/testing).
    #[arg(long, global = true)]
    test_pattern: bool,

    /// Connect to this ID on startup (GUI).
    #[arg(long, requires = "password")]
    connect: Option<String>,

    /// Password for --connect.
    #[arg(long)]
    password: Option<String>,

    /// Start hidden in the system tray (used by autostart).
    #[arg(long)]
    minimized: bool,

    #[cfg(windows)]
    #[arg(long, hide = true)]
    service_chat: bool,

    /// Save a screenshot of the window to this PNG after a delay, then quit (testing).
    #[arg(long, hide = true)]
    screenshot: Option<std::path::PathBuf>,

    /// Seconds to wait before --screenshot.
    #[arg(long, hide = true, default_value_t = 3.0)]
    screenshot_delay: f64,

    /// Open the file transfer window once connected (testing).
    #[arg(long, hide = true)]
    open_files: bool,

    /// Open on this page: bridge, links, config, about (testing).
    #[arg(long, hide = true)]
    page: Option<String>,

    /// Initial window size, e.g. 1180x1400 (testing).
    #[arg(long, hide = true, value_parser = parse_size)]
    window_size: Option<[f32; 2]>,

    /// A `slipstream://link?…` URL to open (the station page's Connect button).
    #[arg(value_name = "LINK", hide = true, value_parser = parse_link)]
    link: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run headless, accepting incoming sessions (prints ID and password).
    Host {
        /// Use this permanent password instead of a one-time one.
        #[arg(long)]
        password: Option<String>,
    },
    /// Connect headless and print stream statistics (for testing).
    Connect {
        /// The 9-digit ID of the remote device.
        id: String,
        /// Its password (with --station, the one saved for it is used if left out).
        #[arg(long)]
        password: Option<String>,
        /// Find it on this station you're signed in to (see sign-in), not
        /// this app's own.
        #[arg(long)]
        station: Option<String>,
        /// How long to stay connected.
        #[arg(long, default_value_t = 10)]
        seconds: u64,
        /// List a remote folder ("" = home) and exit.
        #[arg(long)]
        ls: Option<String>,
        /// Upload a local file to the remote Desktop (or --to FOLDER) and exit.
        #[arg(long)]
        put: Option<std::path::PathBuf>,
        /// Remote folder for --put.
        #[arg(long)]
        to: Option<String>,
        /// Download a remote file into the current folder and exit.
        #[arg(long)]
        get: Option<String>,
        /// Forward a local port through the link, until Ctrl+C: LOCAL:HOST:PORT
        /// (HOST as seen from the remote computer), or LOCAL:PORT for its
        /// localhost. E.g. `--forward 13389:3389` for its Remote Desktop.
        #[arg(long, value_name = "LOCAL:HOST:PORT")]
        forward: Vec<String>,
    },
    /// Set the server (a domain or URL, e.g. `relay.example.com`).
    SetServer {
        address: String,
        /// Access key, for private stations.
        #[arg(long)]
        key: Option<String>,
    },
    /// Set (or with no argument, remove) the permanent unattended-access password.
    SetPassword { password: Option<String> },
    /// Print this device's ID and settings location.
    Info,
    /// Sign in to a station as a technician (any station, not only this
    /// app's own), to reach its devices and keep your list of them there.
    SignIn {
        /// The station's address, e.g. `relay.example.com`.
        station: String,
        username: String,
        /// Read from the terminal if left out.
        #[arg(long)]
        password: Option<String>,
    },
    /// Sign out of a station.
    SignOut { station: String },
    /// List the devices saved on the stations you're signed in to.
    Saved,
    /// Save a device in your list on a station you're signed in to.
    Save {
        id: String,
        #[arg(long)]
        station: String,
        #[arg(long, default_value = "")]
        name: String,
        #[arg(long, default_value = "")]
        notes: String,
        /// Its unattended password, kept sealed (only your sign-in opens it).
        #[arg(long)]
        password: Option<String>,
        /// Into the station's shared list instead (station admins).
        #[arg(long)]
        shared: bool,
        /// Remove it from the list instead.
        #[arg(long)]
        remove: bool,
    },
    /// Update to the newest build your station offers.
    Update,
    /// Windows: run as a service, for unattended access including the login
    /// screen and administrator prompts.
    #[cfg(windows)]
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
    /// Write the app icon as a PNG (used by packaging scripts).
    #[command(hide = true)]
    ExportIcon { path: std::path::PathBuf, #[arg(long, default_value_t = 256)] size: u32 },
}

#[cfg(windows)]
#[derive(Debug, Subcommand)]
enum ServiceCmd {
    /// Install and start the service (as administrator). Uses this
    /// computer's station and unattended password.
    Install {
        /// Set this unattended password for the service.
        #[arg(long)]
        password: Option<String>,
        /// Settings folder to copy from (default: this user's).
        #[arg(long)]
        from: Option<std::path::PathBuf>,
        /// File to write an error to (for the app, which can't see this
        /// elevated process's output).
        #[arg(long, hide = true)]
        report: Option<std::path::PathBuf>,
    },
    /// Stop and remove the service (as administrator).
    Uninstall {
        #[arg(long, hide = true)]
        report: Option<std::path::PathBuf>,
    },
    /// Start the installed service (no administrator rights needed).
    Start,
    /// Show whether the service is installed and running.
    Status,
    /// Run as the service (started by Windows).
    #[command(hide = true)]
    Run,
    /// The host the service runs in the user's session.
    #[command(hide = true)]
    Host,
}

fn main() -> anyhow::Result<()> {
    // A copy started to test the system's OpenGL (see gui::pick_renderer).
    #[cfg(windows)]
    if std::env::var_os("SLIPSTREAM_PROBE_GL").is_some() {
        std::process::exit(gui::probe_gl());
    }
    #[cfg(windows)]
    attach_parent_console();
    let args = Args::parse();
    // The service and its host use the machine profile, logs included.
    #[cfg(windows)]
    if let Some(Command::Service { action: ServiceCmd::Run | ServiceCmd::Host }) = &args.command {
        // SAFETY: still single-threaded here.
        unsafe {
            std::env::set_var("SLIPSTREAM_HOME", service::machine_home());
            std::env::set_var("SLIPSTREAM_LOG_DIR", service::log_path().parent().expect("in a folder"));
        }
    }
    slipstream::logging::init();
    if !matches!(args.command, Some(Command::ExportIcon { .. })) {
        slipstream::apply_station_config();
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    match args.command {
        Some(Command::Host { password }) => rt.block_on(cli::host(args.test_pattern, password, false)),
        #[cfg(windows)]
        Some(Command::Service { action }) => match action {
            ServiceCmd::Install { password, from, report } => reported(report, service::install(password.as_deref(), from.as_deref())),
            ServiceCmd::Uninstall { report } => reported(report, service::uninstall()),
            ServiceCmd::Start => service::start(),
            ServiceCmd::Status => {
                println!("{}", match service::state() {
                    None => "not installed".to_string(),
                    Some(s) => format!("{s:?}"),
                });
                Ok(())
            }
            ServiceCmd::Run => {
                drop(rt);
                service::run()
            }
            ServiceCmd::Host => {
                slipstream_platform::windesk::set_follow_input_desktop(true);
                rt.block_on(cli::host(false, None, true))
            }
        },
        Some(Command::Connect { id, password, station, seconds, ls, put, to, get, forward }) => {
            rt.block_on(cli::connect(&id, password, station, seconds, cli::FileOps { ls, put, to, get }, forward))
        }
        Some(Command::SignIn { station, username, password }) => rt.block_on(cli::sign_in(&station, &username, password)),
        Some(Command::SignOut { station }) => rt.block_on(cli::sign_out(&station)),
        Some(Command::Saved) => rt.block_on(cli::saved()),
        Some(Command::Save { id, station, name, notes, password, shared, remove }) => {
            rt.block_on(cli::save(&id, &station, name, notes, password, shared, remove))
        }
        Some(Command::SetServer { address, key }) => cli::set_server(&address, key),
        Some(Command::SetPassword { password }) => cli::set_password(password.as_deref()),
        Some(Command::Info) => rt.block_on(cli::info()),
        Some(Command::Update) => rt.block_on(cli::update()),
        Some(Command::ExportIcon { path, size }) => gui::export_icon(&path, size),
        None => {
            drop(rt);
            report_crashes();
            let result = gui::run(gui::GuiOptions {
                test_pattern: args.test_pattern,
                connect: args.connect.zip(args.password),
                screenshot: args.screenshot.map(|p| (p, args.screenshot_delay)),
                page: args.page,
                minimized: args.minimized,
                #[cfg(windows)]
                service_chat: args.service_chat,
                open_files: args.open_files,
                window_size: args.window_size,
                link: args.link,
            });
            if let Err(e) = &result {
                fatal(&format!("Slipstream Remote couldn't start:\n\n{e:#}"));
            }
            // The window is gone and the client has shut down (signed off
            // with the station; settings and logs are written as they
            // change). End right here: tearing down the async runtime waits
            // on blocking tasks still running (capture, file I/O), and on
            // Windows a DLL's exit-time cleanup can hang ExitProcess too
            // (seen after a session), either way leaving a windowless
            // process behind.
            let code = if result.is_ok() { 0 } else { 1 };
            #[cfg(windows)]
            // SAFETY: ends this process; nothing runs after it.
            unsafe {
                windows_sys::Win32::System::Threading::TerminateProcess(
                    windows_sys::Win32::System::Threading::GetCurrentProcess(),
                    code as u32,
                );
            }
            std::process::exit(code);
        }
    }
}

/// Logs panics (the log file is all there is without a console) and shows
/// one on the main thread, which ends the app.
fn report_crashes() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        tracing::error!(%thread, "panic: {info}");
        if thread == "main" {
            fatal(&format!("Slipstream Remote stopped unexpectedly:\n\n{info}"));
        }
        default(info);
    }));
}

/// A fatal GUI error: logged, and on Windows (where the app has no console,
/// so it would otherwise just vanish) shown in a message box.
fn fatal(text: &str) {
    tracing::error!("{text}");
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let log = slipstream_core::config::Store::open_default()
            .map(|s| format!("\n\nDetails are in {}", s.dir().join("slipstream.log").display()))
            .unwrap_or_default();
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (body, title) = (wide(&format!("{text}{log}")), wide("Slipstream Remote"));
        // SAFETY: valid NUL-terminated UTF-16 strings; no owner window.
        unsafe { MessageBoxW(std::ptr::null_mut(), body.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR) };
    }
}

/// Only `slipstream://` URLs, so a mistyped command still gets an error.
fn parse_link(s: &str) -> Result<String, String> {
    if s.starts_with("slipstream://") {
        Ok(s.to_string())
    } else {
        Err(format!("unknown command {s:?} (see --help)"))
    }
}

fn parse_size(s: &str) -> Result<[f32; 2], String> {
    let (w, h) = s.split_once('x').ok_or("expected WIDTHxHEIGHT")?;
    let n = |v: &str| v.trim().parse::<f32>().map_err(|e| e.to_string());
    Ok([n(w)?, n(h)?])
}

/// GUI-subsystem apps have no console; when started from a terminal (e.g.
/// `slipstream.exe info`), reattach so CLI output is visible.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: plain Win32 call; failure (no parent console) is harmless.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Logs a failed service install or removal, and writes it to `report` for
/// the app that asked for it.
#[cfg(windows)]
fn reported(report: Option<std::path::PathBuf>, result: anyhow::Result<()>) -> anyhow::Result<()> {
    if let Err(e) = &result {
        tracing::error!("{e:#}");
        if let Some(path) = report {
            let _ = std::fs::write(path, format!("{e:#}"));
        }
    }
    result
}
