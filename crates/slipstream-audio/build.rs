//! Builds the vendored libopus (see opus/VENDORED.txt) with `cc`, the same
//! way on every target (no CMake): the float build, without the optional
//! SIMD and neural-network parts. Source lists come from libopus's own .mk
//! files.
use std::path::Path;

/// Files listed under `var` in a libopus `.mk` file (`VAR = a.c \ b.c …`).
fn sources(mk: &Path, var: &str) -> Vec<String> {
    let text = std::fs::read_to_string(mk).expect("opus source list");
    let mut out = Vec::new();
    let mut lines = text.lines().skip_while(|l| l.split('=').next().map(str::trim) != Some(var));
    let Some(first) = lines.next() else { panic!("{var} not in {}", mk.display()) };
    let mut line = first.split_once('=').map(|(_, rest)| rest).unwrap_or("");
    loop {
        let more = line.trim_end().ends_with('\\');
        out.extend(line.trim_end_matches('\\').split_whitespace().map(str::to_string));
        if !more {
            break;
        }
        line = lines.next().unwrap_or("");
    }
    out
}

fn main() {
    let root = Path::new("opus");
    println!("cargo:rerun-if-changed=opus");
    let mut files = Vec::new();
    for (mk, var) in [
        ("celt_sources.mk", "CELT_SOURCES"),
        ("silk_sources.mk", "SILK_SOURCES"),
        ("silk_sources.mk", "SILK_SOURCES_FLOAT"),
        ("opus_sources.mk", "OPUS_SOURCES"),
        ("opus_sources.mk", "OPUS_SOURCES_FLOAT"),
    ] {
        files.extend(sources(&root.join(mk), var).into_iter().map(|f| root.join(f)));
    }
    cc::Build::new()
        .files(&files)
        .include(root.join("include"))
        .include(root.join("celt"))
        .include(root.join("silk"))
        .include(root.join("silk/float"))
        .include(root)
        .define("OPUS_BUILD", None)
        .define("USE_ALLOCA", None)
        .define("HAVE_LRINTF", None)
        .define("HAVE_LRINT", None)
        .opt_level(3)
        .warnings(false)
        .compile("opus");
}
