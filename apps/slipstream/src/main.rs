//! Slipstream Remote desktop app.

// A GUI app on Windows: no console window when double-clicked.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod cli;
mod gui;

use clap::{Parser, Subcommand};
use tracing_subscriber::Layer as _;

#[derive(Debug, Parser)]
#[command(name = "slipstream", version, about = "Fast, free, open-source remote desktop")]
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
    /// Write the app icon as a PNG (used by packaging scripts).
    #[command(hide = true)]
    ExportIcon { path: std::path::PathBuf, #[arg(long, default_value_t = 256)] size: u32 },
}

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    attach_parent_console();
    init_logging();
    let args = Args::parse();
    if !matches!(args.command, Some(Command::ExportIcon { .. })) {
        apply_station_config();
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    match args.command {
        Some(Command::Host { password }) => rt.block_on(cli::host(args.test_pattern, password)),
        Some(Command::Connect { id, password, seconds, ls, put, to, get }) => {
            rt.block_on(cli::connect(&id, &password, seconds, cli::FileOps { ls, put, to, get }))
        }
        Some(Command::SetServer { address, key }) => cli::set_server(&address, key),
        Some(Command::SetPassword { password }) => cli::set_password(password.as_deref()),
        Some(Command::Info) => rt.block_on(cli::info()),
        Some(Command::ExportIcon { path, size }) => gui::export_icon(&path, size),
        None => {
            drop(rt);
            gui::run(gui::GuiOptions {
                test_pattern: args.test_pattern,
                connect: args.connect.zip(args.password),
                screenshot: args.screenshot.map(|p| (p, args.screenshot_delay)),
                page: args.page,
                minimized: args.minimized,
                open_files: args.open_files,
            })
        }
    }
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
