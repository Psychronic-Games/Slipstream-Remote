//! Slipstream Remote desktop app.

mod cli;
mod gui;

use clap::{Parser, Subcommand};

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
    },
    /// Set the directory server (`<key>@<host>:<port>`).
    SetServer { address: String },
    /// Set (or with no argument, remove) the permanent unattended-access password.
    SetPassword { password: Option<String> },
    /// Print this device's ID and settings location.
    Info,
    /// Write the app icon as a PNG (used by packaging scripts).
    #[command(hide = true)]
    ExportIcon { path: std::path::PathBuf, #[arg(long, default_value_t = 256)] size: u32 },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,iroh=warn,noq=warn,noq_proto=warn,iroh_relay=warn".into()),
        )
        .init();
    let args = Args::parse();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    match args.command {
        Some(Command::Host { password }) => rt.block_on(cli::host(args.test_pattern, password)),
        Some(Command::Connect { id, password, seconds }) => rt.block_on(cli::connect(&id, &password, seconds)),
        Some(Command::SetServer { address }) => cli::set_server(&address),
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
            })
        }
    }
}
