//! The app's log: stderr and `slipstream.log` in the settings folder.

use tracing_subscriber::Layer as _;

/// Logs to stderr and to `slipstream.log` in the settings folder, so
/// problems can be diagnosed even when there's no console (Windows, or an
/// app started from a desktop icon).
pub fn init() {
    use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};
    // enigo logs every input event Windows refuses (each mouse move into an
    // administrator window); the input thread reports those itself, once.
    // On Android, winit warns about every event it doesn't handle yet.
    let android = if cfg!(target_os = "android") { ",winit=error" } else { "" };
    let filter = || {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            format!("info,iroh=warn,noq=warn,noq_proto=warn,noq_udp=error,iroh_relay=warn,wgpu=warn,wgpu_hal=error,netwatch=error,enigo=off{android}").into()
        })
    };
    // Local time, no module paths: the log is for people.
    let timer = || fmt::time::ChronoLocal::new("%Y-%m-%d %H:%M:%S".into());
    // The Windows service logs where every user can read it (its profile
    // is administrators-only); see main().
    let dir = std::env::var_os("SLIPSTREAM_LOG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| slipstream_core::config::Store::open_default().ok().map(|s| s.dir().to_path_buf()));
    let file = dir.and_then(|dir| {
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("slipstream.log");
        // Keep the log small: start fresh once it passes 2 MB.
        let append = std::fs::metadata(&path).map(|m| m.len() < 2 * 1024 * 1024).unwrap_or(false);
        std::fs::OpenOptions::new().create(true).append(append).write(true).truncate(!append).open(path).ok()
    });
    #[cfg(not(target_os = "android"))]
    let console = fmt::layer().with_timer(timer()).with_target(false).with_writer(std::io::stderr);
    // Android has no stderr to read: there it's logcat (`adb logcat -s
    // Slipstream`), which keeps its own times.
    #[cfg(target_os = "android")]
    let console = fmt::layer().with_ansi(false).without_time().with_target(false).with_writer(logcat::Logcat);
    let registry = tracing_subscriber::registry().with(console.with_filter(filter()));
    match file {
        Some(f) => registry
            .with(
                fmt::layer()
                    .with_ansi(false)
                    .with_timer(timer())
                    .with_target(false)
                    .with_writer(LogFile(std::sync::Mutex::new(Repeats { file: f, last: String::new(), count: 0 })))
                    .with_filter(filter()),
            )
            .init(),
        None => registry.init(),
    }
}

/// The log file. A line that comes again and again (apart from its time) is
/// written once, then "… repeated N more times".
struct LogFile(std::sync::Mutex<Repeats>);

struct Repeats {
    file: std::fs::File,
    last: String,
    count: u64,
}

/// One formatted line, handed to [`LogFile`] when complete.
struct LogLine<'a>(Vec<u8>, &'a LogFile);

impl std::io::Write for LogLine<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for LogLine<'_> {
    fn drop(&mut self) {
        use std::io::Write;
        let line = String::from_utf8_lossy(&self.0);
        // "2026-10-18 00:40:23 " is the time; the rest decides "the same".
        let key = line.get(20..).unwrap_or(&line).to_string();
        let Ok(mut r) = self.1.0.lock() else { return };
        if key == r.last {
            r.count += 1;
            return;
        }
        if r.count > 0 {
            let n = std::mem::take(&mut r.count);
            let _ = writeln!(r.file, "                    … repeated {n} more time{}", if n == 1 { "" } else { "s" });
        }
        let _ = r.file.write_all(&self.0);
        r.last = key;
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogFile {
    type Writer = LogLine<'a>;
    fn make_writer(&'a self) -> Self::Writer {
        LogLine(Vec::new(), self)
    }
}

#[cfg(target_os = "android")]
mod logcat {
    use std::ffi::{CString, c_char, c_int};

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    /// Writes each formatted line to logcat at its level.
    pub struct Logcat;

    pub struct Line(Vec<u8>, c_int);

    impl std::io::Write for Line {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Line {
        fn drop(&mut self) {
            let text = String::from_utf8_lossy(&self.0).trim_end().replace('\0', " ");
            let Ok(text) = CString::new(text) else { return };
            // SAFETY: both strings are NUL-terminated and outlive the call.
            unsafe { __android_log_write(self.1, c"Slipstream".as_ptr(), text.as_ptr()) };
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logcat {
        type Writer = Line;
        fn make_writer(&'a self) -> Line {
            Line(Vec::new(), 4)
        }
        fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Line {
            // android/log.h: VERBOSE 2, DEBUG 3, INFO 4, WARN 5, ERROR 6.
            let prio = match *meta.level() {
                tracing::Level::TRACE => 2,
                tracing::Level::DEBUG => 3,
                tracing::Level::INFO => 4,
                tracing::Level::WARN => 5,
                tracing::Level::ERROR => 6,
            };
            Line(Vec::new(), prio)
        }
    }
}
