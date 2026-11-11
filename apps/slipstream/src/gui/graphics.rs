//! How the Windows app's full window gets drawn, whatever the graphics
//! drivers do. The ways, best first: Vulkan, Direct3D 12, the system's
//! OpenGL, the built-in software renderer (`softgl`), that renderer from a
//! copy of the app beside it, and last the compact native window (`lite`).
//!
//! A broken driver doesn't say "not available": it crashes or hangs whoever
//! loads it (old Intel Vulkan drivers do). So:
//!
//! - Each driver is first tried in a throwaway process ([`probe`]). One that
//!   crashes, hangs or can't make a device there is passed over.
//! - The app then loads only the one that passed ([`backends`]), never the
//!   others.
//! - While the window starts, a marker file names the way being tried. A
//!   crash, panic or error before the first frames are drawn starts the app
//!   again without that way ([`start_failed`], `crash`), down to the compact
//!   window. A marker simply left behind (a hang, or the computer going
//!   off) counts the second time in a row.
//!
//! What failed is remembered per build in `graphics/failed`, so an update
//! tries everything again, as does the compact window's "full window"
//! button ([`retry`]).

#![cfg_attr(not(windows), allow(dead_code))]

/// One way to draw the full window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Vulkan,
    Dx12,
    SystemGl,
    /// Mesa loaded into the app as installed.
    SoftGl,
    /// Mesa from the copy of the app beside it (`softgl::prepare_beside`).
    SoftGlBeside,
}

impl Tier {
    const ALL: [Tier; 5] = [Tier::Vulkan, Tier::Dx12, Tier::SystemGl, Tier::SoftGl, Tier::SoftGlBeside];

    /// The name in files and environment variables.
    fn key(self) -> &'static str {
        match self {
            Tier::Vulkan => "vulkan",
            Tier::Dx12 => "dx12",
            Tier::SystemGl => "gl",
            Tier::SoftGl => "softgl",
            Tier::SoftGlBeside => "softgl-beside",
        }
    }

    fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.key() == key)
    }

    /// The name in the log.
    fn label(self) -> &'static str {
        match self {
            Tier::Vulkan => "Vulkan",
            Tier::Dx12 => "Direct3D 12",
            Tier::SystemGl => "the system's OpenGL",
            Tier::SoftGl => "the built-in software renderer",
            Tier::SoftGlBeside => "the software renderer beside its own copy of the app",
        }
    }
}

/// A marker left behind this many starts in a row means that way hangs.
const STRIKES: u32 = 2;

/// `graphics/failed`: "build N", then one way per line. Another build's
/// list doesn't count.
fn parse_failed(text: &str, build: u64) -> Vec<Tier> {
    let mut lines = text.lines().map(str::trim);
    if lines.next().and_then(|l| l.strip_prefix("build ")).and_then(|b| b.parse().ok()) != Some(build) {
        return Vec::new();
    }
    let mut failed = Vec::new();
    for tier in lines.filter_map(Tier::parse) {
        if !failed.contains(&tier) {
            failed.push(tier);
        }
    }
    failed
}

fn format_failed(failed: &[Tier], build: u64) -> String {
    let mut text = format!("build {build}\n");
    for tier in failed {
        text.push_str(tier.key());
        text.push('\n');
    }
    text
}

/// "vulkan,dx12": the list a relaunch is handed.
fn keys(failed: &[Tier]) -> String {
    failed.iter().map(|t| t.key()).collect::<Vec<_>>().join(",")
}

/// `graphics/starting`: the way being tried and how many starts in a row
/// have left it behind ("vulkan 1").
fn parse_marker(text: &str) -> Option<(Tier, u32)> {
    let mut words = text.split_whitespace();
    Some((Tier::parse(words.next()?)?, words.next()?.parse().ok()?))
}

/// What this start must avoid, and the marker it carries over: the saved
/// list, plus what the start that relaunched us reports (`handed`), plus a
/// way whose marker was left behind [`STRIKES`] times.
fn resolve(saved: &[Tier], handed: Option<&str>, marker: Option<(Tier, u32)>) -> (Vec<Tier>, Option<(Tier, u32)>) {
    let mut failed = saved.to_vec();
    for tier in handed.unwrap_or_default().split(',').filter_map(|k| Tier::parse(k.trim())) {
        if !failed.contains(&tier) {
            failed.push(tier);
        }
    }
    match marker {
        Some((tier, _)) if failed.contains(&tier) => (failed, None),
        Some((tier, strikes)) if strikes >= STRIKES => {
            failed.push(tier);
            (failed, None)
        }
        marker => (failed, marker),
    }
}

/// `graphics/crash`, written as the app crashes: the exception code, then
/// the path of the module it happened in (UTF-16). Returns "0xC0000005 in
/// igvk64.dll".
fn parse_crash(bytes: &[u8]) -> Option<String> {
    let code = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    let path: Vec<u16> = bytes[4..].as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
    let path = String::from_utf16_lossy(&path);
    let module = path.rsplit(['\\', '/']).next().unwrap_or_default();
    Some(if module.is_empty() { format!("{code:#010X}") } else { format!("{code:#010X} in {module}") })
}

#[cfg(windows)]
pub use win::*;

#[cfg(windows)]
mod win {
    use std::{
        io::Read,
        path::PathBuf,
        sync::{
            Mutex,
            atomic::{AtomicBool, AtomicU32, Ordering},
        },
        time::{Duration, Instant},
    };

    use eframe::egui_wgpu::{WgpuSetupCreateNew, wgpu};

    use super::{super::softgl, super::video, STRIKES, Tier, format_failed, keys, parse_crash, parse_failed, parse_marker, resolve};

    /// Set for the throwaway process that tries one driver ([`probe`]).
    pub const ENV_PROBE: &str = "SLIPSTREAM_PROBE_GRAPHICS";
    /// Handed to a relaunch: the ways that have failed so far, so it moves
    /// on even where the profile can't be written.
    const ENV_FAILED: &str = "SLIPSTREAM_GRAPHICS_FAILED";
    /// Test hook, comma-separated `what:way` (ways as in [`Tier::key`]):
    /// `probe-crash`, `probe-hang`, `start-crash`, `start-panic`,
    /// `start-error`. E.g. `probe-crash:vulkan,start-crash:dx12`.
    const ENV_TEST: &str = "SLIPSTREAM_TEST_GRAPHICS";

    pub enum Renderer {
        Full,
        Compact,
        /// Run this copy of the app instead (see `softgl::prepare_beside`).
        Relaunch(PathBuf),
    }

    struct Run {
        failed: Vec<Tier>,
        /// The marker found at start: a way that didn't finish starting,
        /// and how often in a row.
        strike: Option<(Tier, u32)>,
        /// Being started: its failure moves the app on to the next way.
        armed: Option<Tier>,
        chosen: Option<Tier>,
        /// What misbehaved on the way, for the system report.
        passed_over: Vec<String>,
    }

    static RUN: Mutex<Run> = Mutex::new(Run { failed: Vec::new(), strike: None, armed: None, chosen: None, passed_over: Vec::new() });

    fn pass_over(tier: Tier, why: &str) {
        RUN.lock().unwrap().passed_over.push(format!("{} ({why})", tier.label()));
    }

    /// Tells the system report how the window is drawn: `how` ("Direct3D 12
    /// · Intel HD Graphics 4600"), and the drivers that crashed, hung or
    /// failed on the way there.
    pub fn report(how: &str) {
        let passed_over = RUN.lock().unwrap().passed_over.join(", ");
        let line = if passed_over.is_empty() { how.to_string() } else { format!("{how}; passed over: {passed_over}") };
        slipstream_platform::machine::set_app_graphics(&line);
    }
    /// The window's first frames aren't drawn yet ([`frame`]).
    static PENDING: AtomicBool = AtomicBool::new(false);
    static FRAMES: AtomicU32 = AtomicU32::new(0);

    fn dir() -> PathBuf {
        directories::ProjectDirs::from("dev", "Slipstream", "Slipstream")
            .map(|d| d.data_local_dir().to_path_buf())
            .unwrap_or_else(std::env::temp_dir)
            .join("graphics")
    }

    fn marker() -> PathBuf {
        dir().join("starting")
    }

    fn failed_file() -> PathBuf {
        dir().join("failed")
    }

    fn crash_file() -> PathBuf {
        dir().join("crash")
    }

    fn test(what: &str, tier: Tier) -> bool {
        std::env::var(ENV_TEST).is_ok_and(|v| v.split(',').any(|t| t.trim().split_once(':') == Some((what, tier.key()))))
    }

    fn save_failed(failed: &[Tier]) {
        let _ = std::fs::create_dir_all(dir());
        if let Err(e) = std::fs::write(failed_file(), format_failed(failed, slipstream_core::BUILD_NUMBER)) {
            tracing::warn!("couldn't note which graphics failed: {e}");
        }
    }

    /// Reads what earlier starts left behind.
    fn load() {
        let saved = std::fs::read_to_string(failed_file()).map(|t| parse_failed(&t, slipstream_core::BUILD_NUMBER)).unwrap_or_default();
        let handed = std::env::var(ENV_FAILED).ok();
        let marker = std::fs::read_to_string(marker()).ok().and_then(|t| parse_marker(&t));
        let crash = std::fs::read(crash_file()).ok().and_then(|b| parse_crash(&b));
        let _ = std::fs::remove_file(crash_file());
        let (failed, strike) = resolve(&saved, handed.as_deref(), marker);
        for tier in failed.iter().filter(|t| !saved.contains(t)) {
            match &crash {
                Some(crash) => tracing::warn!("{} crashed while the window was starting ({crash}): not using it", tier.label()),
                None => tracing::warn!("{} didn't get the window started: not using it", tier.label()),
            }
        }
        if let Some((tier, strikes)) = strike {
            tracing::warn!("{} didn't finish starting last time ({strikes} of {STRIKES} before it's passed over)", tier.label());
        }
        if failed != saved {
            save_failed(&failed);
        }
        // Not for the processes started from here; `arm` sets it again.
        // SAFETY: Windows locks its environment itself.
        unsafe { std::env::remove_var(ENV_FAILED) };
        let mut run = RUN.lock().unwrap();
        run.failed = failed;
        run.strike = strike;
    }

    fn failed(tier: Tier) -> bool {
        let failed = RUN.lock().unwrap().failed.contains(&tier);
        if failed {
            pass_over(tier, "failed to start the window");
            tracing::info!("passing over {}: it failed to start before (an update tries it again)", tier.label());
        }
        failed
    }

    /// A way to draw the full window failed to start before: the compact
    /// window offers to try them all again.
    pub fn failed_before() -> bool {
        !RUN.lock().unwrap().failed.is_empty()
    }

    /// Forgets what failed, so the next start tries everything again.
    pub fn retry() {
        let _ = std::fs::remove_file(failed_file());
        let _ = std::fs::remove_file(marker());
    }

    /// The one graphics API the window may use, once [`pick`] chose it.
    pub fn backends() -> Option<wgpu::Backends> {
        RUN.lock().unwrap().chosen.map(backends_of)
    }

    fn backends_of(tier: Tier) -> wgpu::Backends {
        match tier {
            Tier::Vulkan => wgpu::Backends::VULKAN,
            Tier::Dx12 => wgpu::Backends::DX12,
            Tier::SystemGl | Tier::SoftGl | Tier::SoftGlBeside => wgpu::Backends::GL,
        }
    }

    /// Decides how the window draws (see the module's description).
    pub fn pick(rt: &tokio::runtime::Runtime) -> Renderer {
        softgl::note_build();
        load();
        let renderer = choose(rt);
        if matches!(renderer, Renderer::Compact) {
            let _ = std::fs::remove_file(marker());
        }
        renderer
    }

    fn choose(rt: &tokio::runtime::Runtime) -> Renderer {
        if softgl::running_beside() {
            return if !failed(Tier::SoftGlBeside) && start_software(rt, Tier::SoftGlBeside) { Renderer::Full } else { Renderer::Compact };
        }
        // SLIPSTREAM_SOFTWARE_RENDERER=1 goes straight to it (troubleshooting).
        if std::env::var_os("SLIPSTREAM_SOFTWARE_RENDERER").is_none() {
            // A real graphics card on any of them comes before one that
            // only draws on the processor (Windows' own, through Direct3D).
            let mut on_processor = None;
            for tier in [Tier::Vulkan, Tier::Dx12, Tier::SystemGl] {
                if failed(tier) {
                    continue;
                }
                match probe_in_child(tier) {
                    Probe::Hardware => {
                        arm(tier);
                        return Renderer::Full;
                    }
                    Probe::Processor => {
                        on_processor.get_or_insert(tier);
                    }
                    Probe::Unusable => {}
                }
            }
            if let Some(tier) = on_processor {
                arm(tier);
                return Renderer::Full;
            }
        }
        if !failed(Tier::SoftGl) && start_software(rt, Tier::SoftGl) {
            return Renderer::Full;
        }
        // The software renderer didn't draw from here: try the copy beside
        // it (which itself goes to the compact window if it can't).
        if softgl::available() && !failed(Tier::SoftGlBeside) {
            match softgl::prepare().and_then(|()| softgl::prepare_beside()) {
                Ok(exe) => return Renderer::Relaunch(exe),
                Err(e) => softgl::set_problem(format!("{e:#}")),
            }
        }
        Renderer::Compact
    }

    /// Loads Mesa into this process and checks it offers a renderer.
    fn start_software(rt: &tokio::runtime::Runtime, tier: Tier) -> bool {
        if let Err(e) = softgl::prepare() {
            softgl::set_problem(format!("{e:#}"));
            return false;
        }
        // SLIPSTREAM_TEST_BESIDE=1: fail here, to test the copy beside Mesa.
        if tier == Tier::SoftGl && std::env::var_os("SLIPSTREAM_TEST_BESIDE").is_some() {
            softgl::set_problem("test: not from here".into());
            return false;
        }
        arm(tier);
        if use_software_gl() {
            if count_adapters(rt, wgpu::Backends::GL) > 0 {
                return true;
            }
            softgl::set_problem(gl_problem());
        }
        fail(tier);
        false
    }

    /// Starts watching `tier`: until the window has drawn, its failure
    /// starts the app again without it.
    fn arm(tier: Tier) {
        let mut run = RUN.lock().unwrap();
        let strikes = match run.strike {
            Some((t, n)) if t == tier => n + 1,
            _ => 1,
        };
        let _ = std::fs::create_dir_all(dir());
        let _ = std::fs::write(marker(), format!("{} {strikes}", tier.key()));
        let mut without = run.failed.clone();
        without.push(tier);
        // What a relaunch from the crash handler inherits. The single-
        // instance guard then waits for this process to go.
        // SAFETY: Windows locks its environment itself.
        unsafe {
            std::env::set_var(ENV_FAILED, keys(&without));
            std::env::set_var("SLIPSTREAM_RESTARTED", "1");
        }
        run.armed = Some(tier);
        run.chosen = Some(tier);
        drop(run);
        FRAMES.store(0, Ordering::Relaxed);
        PENDING.store(true, Ordering::Relaxed);
        crash::arm(&crash_file());
        tracing::info!("starting the window with {}", tier.label());
    }

    /// Stops watching: the way being started worked, or was given up.
    fn disarm() -> Option<Tier> {
        crash::disarm();
        PENDING.store(false, Ordering::Relaxed);
        // In a panic the lock may be held: then nothing was being started.
        let tier = RUN.try_lock().ok()?.armed.take()?;
        let _ = std::fs::remove_file(marker());
        // SAFETY: Windows locks its environment itself.
        unsafe {
            std::env::remove_var(ENV_FAILED);
            std::env::remove_var("SLIPSTREAM_RESTARTED");
        }
        Some(tier)
    }

    /// Gives `tier` up for this build.
    fn fail(tier: Tier) {
        disarm();
        let Ok(mut run) = RUN.try_lock() else { return };
        if !run.failed.contains(&tier) {
            run.failed.push(tier);
            save_failed(&run.failed);
        }
    }

    /// Called every frame. Once the first one has been drawn and shown (the
    /// second is under way), the window has started.
    pub fn frame() {
        if !PENDING.load(Ordering::Relaxed) {
            return;
        }
        let armed = RUN.lock().unwrap().armed;
        if let Some(tier) = armed {
            if test("start-crash", tier) {
                // SAFETY: none. The test is what a crashing driver does.
                unsafe { std::ptr::null_mut::<u32>().write_volatile(1) };
            }
            if test("start-panic", tier) {
                panic!("test: {} panics while starting", tier.label());
            }
        }
        if FRAMES.fetch_add(1, Ordering::Relaxed) == 0 {
            return;
        }
        if let Some(tier) = disarm() {
            tracing::info!("the window is drawn with {}", tier.label());
        }
    }

    /// The test hook's error from the window's set-up.
    pub fn test_start_error() -> Option<String> {
        let tier = RUN.lock().unwrap().armed?;
        test("start-error", tier).then(|| format!("test: {} fails to start", tier.label()))
    }

    /// The window failed (an error or a panic on the main thread). While it
    /// was still starting, gives up that way of drawing, starts the app
    /// again for the next one and returns true: the caller only has to exit.
    pub fn start_failed(why: &str) -> bool {
        let Some(tier) = RUN.try_lock().ok().and_then(|run| run.armed) else { return false };
        tracing::warn!("{} failed while the window was starting ({why}): starting again without it", tier.label());
        fail(tier);
        let Ok(run) = RUN.try_lock() else { return false };
        let Ok(exe) = std::env::current_exe() else { return false };
        std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .env(ENV_FAILED, keys(&run.failed))
            .env("SLIPSTREAM_RESTARTED", "1")
            .spawn()
            .is_ok()
    }

    enum Probe {
        /// A graphics card can draw the window.
        Hardware,
        /// Only a renderer on the processor can.
        Processor,
        Unusable,
    }

    /// Tries `tier`'s driver in a copy of the app ([`probe`]), which may
    /// crash or hang without harm: 10 s to answer.
    fn probe_in_child(tier: Tier) -> Probe {
        use std::process::{Command, Stdio};
        let label = tier.label();
        let Ok(exe) = std::env::current_exe() else { return Probe::Unusable };
        let child = Command::new(exe).env(ENV_PROBE, tier.key()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        let Ok(mut child) = child else { return Probe::Unusable };
        // Read as it comes: a driver that fills the pipe mustn't block.
        let out = child.stdout.take().map(|mut out| {
            std::thread::spawn(move || {
                let mut text = Vec::new();
                let _ = out.read_to_end(&mut text);
                String::from_utf8_lossy(&text).lines().last().unwrap_or_default().trim().to_string()
            })
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let said = out.and_then(|t| t.join().ok()).unwrap_or_default();
        match status.map(|s| s.code()) {
            Some(Some(0)) => {
                tracing::info!("{label} works: {said}");
                Probe::Hardware
            }
            Some(Some(2)) => {
                tracing::info!("{label} draws on the processor only: {said}");
                Probe::Processor
            }
            Some(Some(1)) => {
                tracing::info!("{label} isn't usable here: {said}");
                Probe::Unusable
            }
            Some(code) => {
                // A crash's exit code is its exception (0xC0000005: access violation).
                let code = code.unwrap_or(-1) as u32;
                pass_over(tier, &format!("crashed when tried, {code:#010X}"));
                tracing::warn!("{label} crashed when tried (exit code {code:#010X}): not using it");
                Probe::Unusable
            }
            None => {
                pass_over(tier, "didn't answer when tried");
                tracing::warn!("{label} didn't answer within 10 s when tried: not using it");
                Probe::Unusable
            }
        }
    }

    /// The throwaway process: makes a device on the one API named by
    /// [`ENV_PROBE`], as the window would. Exit code 0: on a graphics card;
    /// 2: on the processor; 1: not at all. Says which adapter on stdout.
    pub fn probe(which: &str) -> i32 {
        use windows_sys::Win32::System::Diagnostics::Debug::{SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode};
        // SAFETY: plain Win32 call. A crash here must not put up a dialog.
        unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX) };
        let Some(tier) = Tier::parse(which).filter(|t| matches!(t, Tier::Vulkan | Tier::Dx12 | Tier::SystemGl)) else { return 1 };
        if test("probe-crash", tier) {
            // SAFETY: none. The test is what a crashing driver does.
            unsafe { std::ptr::null_mut::<u32>().write_volatile(1) };
        }
        if test("probe-hang", tier) {
            std::thread::sleep(Duration::from_secs(3600));
        }
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else { return 1 };
        let adapters = rt.block_on(instance(backends_of(tier)).enumerate_adapters(backends_of(tier)));
        let adapter = match video::pick_adapter(&adapters, None) {
            Ok(adapter) => adapter,
            Err(_) => {
                println!("no adapter");
                return 1;
            }
        };
        let info = adapter.get_info();
        let name = format!("{} ({:?}, driver {} {})", info.name, info.device_type, info.driver, info.driver_info);
        let descriptor = (WgpuSetupCreateNew::without_display_handle().device_descriptor)(&adapter);
        if let Err(e) = rt.block_on(adapter.request_device(&descriptor)) {
            println!("{name} gave no device: {e}");
            return 1;
        }
        println!("{name}");
        if video::draws_on_processor(&info) { 2 } else { 0 }
    }

    /// A wgpu instance that loads only these APIs' drivers.
    fn instance(backends: wgpu::Backends) -> wgpu::Instance {
        let mut descriptor = WgpuSetupCreateNew::without_display_handle().instance_descriptor;
        descriptor.backends = backends;
        wgpu::Instance::new(descriptor)
    }

    /// How many adapters wgpu finds on these backends.
    fn count_adapters(rt: &tokio::runtime::Runtime, backends: wgpu::Backends) -> usize {
        let adapters = rt.block_on(instance(backends).enumerate_adapters(backends));
        for a in &adapters {
            let info = a.get_info();
            tracing::info!(name = %info.name, backend = ?info.backend, kind = ?info.device_type, "graphics adapter found");
        }
        adapters.len()
    }

    /// Why OpenGL (here: the software renderer) offers wgpu nothing, from the
    /// OpenGL backend itself.
    fn gl_problem() -> String {
        use wgpu_hal::Instance as _;
        let desc = wgpu_hal::InstanceDescriptor {
            name: "slipstream",
            flags: wgpu::InstanceFlags::empty(),
            memory_budget_thresholds: Default::default(),
            backend_options: Default::default(),
            telemetry: None,
            display: None,
        };
        // SAFETY: creating and dropping a throwaway OpenGL instance.
        match unsafe { wgpu_hal::gles::Instance::init(&desc) } {
            Err(e) => format!("the software renderer's OpenGL didn't start ({e})"),
            Ok(instance) => match unsafe { instance.enumerate_adapters(None) }.len() {
                0 => "the software renderer's OpenGL started but offered no usable adapter".into(),
                n => format!("the software renderer has {n} OpenGL adapter(s), but they weren't listed"),
            },
        }
    }

    /// Makes Mesa's `opengl32.dll` (unpacked by `softgl::prepare`) the one this
    /// process uses, with its CPU renderer.
    fn use_software_gl() -> bool {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::LibraryLoader::{LoadLibraryW, SetDllDirectoryW};
        let dir = softgl::dir();
        let wide = |p: &std::path::Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
        // SAFETY: NUL-terminated paths. Windows locks its environment itself.
        let loaded = unsafe {
            std::env::set_var("GALLIUM_DRIVER", "llvmpipe");
            // Mesa's opengl32.dll finds libgallium_wgl.dll through this folder.
            SetDllDirectoryW(wide(&dir).as_ptr());
            // Loaded by full path now, it's the opengl32.dll that wgpu's later
            // by-name loads get (opengl32 is delay-loaded, see build.rs).
            !LoadLibraryW(wide(&dir.join("opengl32.dll")).as_ptr()).is_null()
        };
        if !loaded {
            let e = std::io::Error::last_os_error();
            softgl::set_problem(format!("Windows wouldn't load the software renderer ({e})"));
            return false;
        }
        tracing::info!(dir = %dir.display(), "using the built-in software renderer");
        true
    }

    /// A crash (in a driver, usually) while the window starts: Windows
    /// would just close the app. This starts it again first, with what
    /// [`arm`] put in the environment, and notes where the crash was.
    mod crash {
        use std::{
            os::windows::ffi::OsStrExt,
            path::Path,
            sync::{
                Once,
                atomic::{AtomicBool, AtomicPtr, Ordering},
            },
        };

        use windows_sys::Win32::{
            Foundation::{CloseHandle, GENERIC_WRITE, HMODULE, INVALID_HANDLE_VALUE},
            Storage::FileSystem::{CREATE_ALWAYS, CreateFileW, WriteFile},
            System::{
                Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter},
                Environment::GetCommandLineW,
                LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW},
                Threading::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW},
            },
        };

        static ARMED: AtomicBool = AtomicBool::new(false);
        static INSTALL: Once = Once::new();
        // Made ahead (and never freed): a crashing process shouldn't allocate.
        static EXE: AtomicPtr<u16> = AtomicPtr::new(std::ptr::null_mut());
        static COMMAND_LINE: AtomicPtr<u16> = AtomicPtr::new(std::ptr::null_mut());
        static NOTE: AtomicPtr<u16> = AtomicPtr::new(std::ptr::null_mut());

        fn keep(text: Vec<u16>) -> *mut u16 {
            Box::leak(text.into_boxed_slice()).as_mut_ptr()
        }

        pub fn arm(note: &Path) {
            INSTALL.call_once(|| {
                let wide = |p: &Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
                let Ok(exe) = std::env::current_exe() else { return };
                // SAFETY: the process's command line, NUL-terminated and
                // valid for its lifetime.
                let command_line = unsafe {
                    let start = GetCommandLineW();
                    let mut len = 0;
                    while *start.add(len) != 0 {
                        len += 1;
                    }
                    std::slice::from_raw_parts(start, len + 1).to_vec()
                };
                EXE.store(keep(wide(&exe)), Ordering::SeqCst);
                COMMAND_LINE.store(keep(command_line), Ordering::SeqCst);
                NOTE.store(keep(wide(note)), Ordering::SeqCst);
                // SAFETY: `on_crash` is a valid filter for the process's lifetime.
                unsafe { SetUnhandledExceptionFilter(Some(on_crash)) };
            });
            ARMED.store(true, Ordering::SeqCst);
        }

        pub fn disarm() {
            ARMED.store(false, Ordering::SeqCst);
        }

        /// Windows calls this for a crash nothing else handles, on the
        /// crashing thread.
        unsafe extern "system" fn on_crash(info: *const EXCEPTION_POINTERS) -> i32 {
            const CONTINUE_SEARCH: i32 = 0; // crash as usual
            const EXECUTE_HANDLER: i32 = 1; // end the process quietly
            let (exe, command_line, note) = (EXE.load(Ordering::SeqCst), COMMAND_LINE.load(Ordering::SeqCst), NOTE.load(Ordering::SeqCst));
            if !ARMED.swap(false, Ordering::SeqCst) || exe.is_null() || command_line.is_null() {
                return CONTINUE_SEARCH;
            }
            // SAFETY: Windows hands over valid exception information; the
            // strings are NUL-terminated and never freed; only stack memory
            // and Win32 calls are used.
            unsafe {
                if !note.is_null() && !info.is_null() && !(*info).ExceptionRecord.is_null() {
                    let record = &*(*info).ExceptionRecord;
                    let mut path = [0u16; 520];
                    let mut module: HMODULE = std::ptr::null_mut();
                    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
                    let len = if GetModuleHandleExW(flags, record.ExceptionAddress as *const u16, &mut module) != 0 {
                        GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32).min(path.len() as u32)
                    } else {
                        0
                    };
                    let file = CreateFileW(note, GENERIC_WRITE, 0, std::ptr::null(), CREATE_ALWAYS, 0, std::ptr::null_mut());
                    if file != INVALID_HANDLE_VALUE {
                        let code = (record.ExceptionCode as u32).to_le_bytes();
                        let mut written = 0;
                        WriteFile(file, code.as_ptr(), 4, &mut written, std::ptr::null_mut());
                        WriteFile(file, path.as_ptr().cast(), len * 2, &mut written, std::ptr::null_mut());
                        CloseHandle(file);
                    }
                }
                let mut startup: STARTUPINFOW = std::mem::zeroed();
                startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
                let mut process: PROCESS_INFORMATION = std::mem::zeroed();
                if CreateProcessW(exe, command_line, std::ptr::null(), std::ptr::null(), 0, 0, std::ptr::null(), std::ptr::null(), &startup, &mut process) == 0 {
                    return CONTINUE_SEARCH;
                }
                CloseHandle(process.hThread);
                CloseHandle(process.hProcess);
            }
            EXECUTE_HANDLER
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_list_round_trips_and_belongs_to_one_build() {
        let text = format_failed(&[Tier::Vulkan, Tier::SoftGl], 7);
        assert_eq!(parse_failed(&text, 7), [Tier::Vulkan, Tier::SoftGl]);
        assert_eq!(parse_failed(&text, 8), []);
        assert_eq!(parse_failed("", 7), []);
        assert_eq!(parse_failed("build 7\nvulkan\nnonsense\nvulkan\n", 7), [Tier::Vulkan]);
    }

    #[test]
    fn every_tier_has_its_own_key() {
        for tier in Tier::ALL {
            assert_eq!(Tier::parse(tier.key()), Some(tier));
        }
    }

    #[test]
    fn a_relaunch_is_told_what_failed() {
        let handed = keys(&[Tier::Vulkan, Tier::Dx12]);
        // Even with nothing saved (a profile that can't be written), and
        // the crashed start's marker is settled by it.
        assert_eq!(resolve(&[], Some(&handed), Some((Tier::Dx12, 1))), (vec![Tier::Vulkan, Tier::Dx12], None));
        assert_eq!(resolve(&[Tier::Vulkan], Some(&handed), None), (vec![Tier::Vulkan, Tier::Dx12], None));
    }

    #[test]
    fn a_marker_left_behind_counts_the_second_time() {
        assert_eq!(resolve(&[], None, Some((Tier::Vulkan, 1))), (vec![], Some((Tier::Vulkan, 1))));
        assert_eq!(resolve(&[], None, Some((Tier::Vulkan, STRIKES))), (vec![Tier::Vulkan], None));
        assert_eq!(resolve(&[Tier::Dx12], None, None), (vec![Tier::Dx12], None));
        assert_eq!(parse_marker("vulkan 2"), Some((Tier::Vulkan, 2)));
        assert_eq!(parse_marker(""), None);
        assert_eq!(parse_marker("vulkan"), None);
    }

    #[test]
    fn crash_note_names_the_module() {
        let mut note = 0xC000_0005u32.to_le_bytes().to_vec();
        note.extend("C:\\Windows\\System32\\igvk64.dll".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(parse_crash(&note).as_deref(), Some("0xC0000005 in igvk64.dll"));
        assert_eq!(parse_crash(&0xC000_001Du32.to_le_bytes()).as_deref(), Some("0xC000001D"));
        assert_eq!(parse_crash(&[1, 2]), None);
    }
}
