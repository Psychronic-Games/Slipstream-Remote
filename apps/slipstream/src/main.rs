//! Slipstream Remote desktop app.

// A GUI app on Windows: no console window when double-clicked.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod cli;
mod gui;
#[cfg(windows)]
mod service;

use clap::{Parser, Subcommand};
use tracing_subscriber::Layer as _;

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
        #[arg(long)]
        password: String,
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
    },
    /// Stop and remove the service (as administrator).
    Uninstall,
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
        unsafe { std::env::set_var("SLIPSTREAM_HOME", service::machine_home()) };
    }
    init_logging();
    if !matches!(args.command, Some(Command::ExportIcon { .. })) {
        apply_station_config();
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    match args.command {
        Some(Command::Host { password }) => rt.block_on(cli::host(args.test_pattern, password, false)),
        #[cfg(windows)]
        Some(Command::Service { action }) => match action {
            ServiceCmd::Install { password, from } => service::install(password.as_deref(), from.as_deref()),
            ServiceCmd::Uninstall => service::uninstall(),
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
        Some(Command::Connect { id, password, seconds, ls, put, to, get, forward }) => {
            rt.block_on(cli::connect(&id, &password, seconds, cli::FileOps { ls, put, to, get }, forward))
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

/// If this copy of the app came from a station's download page, adopt that
/// station's address and access key (see `slipstream_core::provision`).
fn apply_station_config() {
    let Some(cfg) = slipstream_core::provision::embedded() else { return };
    let Ok(store) = slipstream_core::config::Store::open_default() else { return };
    let mut settings = store.load_settings();
    if slipstream_core::provision::apply(&mut settings, &cfg) {
        tracing::info!(server = %cfg.server, "configured from station download");
        let _ = store.save_settings(&settings);
    }
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

/// Logs to stderr and to `slipstream.log` in the settings folder, so
/// problems can be diagnosed even when there's no console (Windows, or an
/// app started from a desktop icon).
fn init_logging() {
    use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};
    let filter = || {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info,iroh=warn,noq=warn,noq_proto=warn,noq_udp=error,iroh_relay=warn,wgpu=warn,wgpu_hal=error,netwatch=error".into())
    };
    let file = slipstream_core::config::Store::open_default().ok().and_then(|store| {
        let path = store.dir().join("slipstream.log");
        // Keep the log small: start fresh once it passes 2 MB.
        let append = std::fs::metadata(&path).map(|m| m.len() < 2 * 1024 * 1024).unwrap_or(false);
        std::fs::OpenOptions::new().create(true).append(append).write(true).truncate(!append).open(path).ok()
    });
    let registry = tracing_subscriber::registry().with(fmt::layer().with_writer(std::io::stderr).with_filter(filter()));
    match file {
        Some(f) => registry
            .with(fmt::layer().with_ansi(false).with_writer(std::sync::Mutex::new(f)).with_filter(filter()))
            .init(),
        None => registry.init(),
    }
}
