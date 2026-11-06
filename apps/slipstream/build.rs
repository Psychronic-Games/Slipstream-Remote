//! Embeds the icon and version info into the Windows executable, delay-loads
//! OpenGL there, and bundles the software renderer when it's provided.
fn main() {
    println!("cargo:rerun-if-changed=../../packaging/windows/slipstream.rc");
    println!("cargo:rerun-if-changed=../../packaging/windows/slipstream.ico");
    println!("cargo:rerun-if-changed=../../packaging/windows/slipstream.manifest");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if windows {
        // The .exe's version info, from Cargo.toml's version with the build
        // number (git commit count) as the fourth part.
        for p in ["../../.git/HEAD", "../../.git/refs/heads", "../../.git/packed-refs"] {
            if std::path::Path::new(p).exists() {
                println!("cargo:rerun-if-changed={p}");
            }
        }
        let version = std::env::var("CARGO_PKG_VERSION").unwrap();
        let build = std::process::Command::new("git")
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u16>().ok())
            .unwrap_or(0);
        let parts: Vec<String> = version.split(['.', '-', '+']).take(3).map(|p| p.parse::<u16>().unwrap_or(0).to_string()).collect();
        let numeric = format!("{},{build}", parts.join(","));
        let macros = [format!("SLIP_VER={numeric}"), format!("SLIP_VER_STR=\"{version}\"")];
        embed_resource::compile("../../packaging/windows/slipstream.rc", macros.iter())
            .manifest_optional()
            .expect("embedding Windows resources");
        // OpenGL loads on first use instead of at start, so the software
        // renderer's opengl32.dll can be loaded first (gui::use_software_gl).
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            // Direct3D too: under Wine its d3d11 is built on OpenGL and would
            // load that first (and loading on first use costs nothing).
            // Media Foundation is optional on Windows Server/N editions.
            // camera::media_foundation checks it before any camera API call.
            for dll in ["opengl32.dll", "d3d11.dll", "dxgi.dll", "mf.dll", "mfplat.dll", "mfreadwrite.dll"] {
                println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
            }
            println!("cargo:rustc-link-lib=delayimp");
        }
    }
    software_renderer(windows);
    older_windows();
}

/// `SLIPSTREAM_YY_THUNKS`: YY-Thunks' Windows 7 object, pinned in
/// packaging/windows/yy-thunks.txt. Linked first, it answers the imports
/// Windows 7/8 lack with stubs that use the real function where it exists,
/// so one executable runs on every Windows version. Without it the build
/// needs Windows 10 (scripts/check-windows-imports.py says which imports).
fn older_windows() {
    println!("cargo:rerun-if-env-changed=SLIPSTREAM_YY_THUNKS");
    let msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if let (true, Ok(path)) = (msvc, std::env::var("SLIPSTREAM_YY_THUNKS")) {
        let path = std::path::Path::new(&path).canonicalize().unwrap_or_else(|_| panic!("SLIPSTREAM_YY_THUNKS: no file at {path}"));
        println!("cargo:rerun-if-changed={}", path.display());
        println!("cargo:rustc-link-arg-bins={}", path.display());
        // The object asks for these import libraries in mixed case
        // ("PowrProf.lib"), which a case-sensitive cross-build sysroot
        // can't find. Ask for them in lower case instead (Windows doesn't mind).
        for lib in ["Advapi32", "crypt32", "Dnsapi", "DXGI", "Gdi32", "Iphlpapi", "mfplat", "Ole32", "PowrProf", "User32", "uuid", "winhttp", "Ws2_32"] {
            println!("cargo:rustc-link-arg-bins=/NODEFAULTLIB:{lib}.lib");
            println!("cargo:rustc-link-arg-bins={}.lib", lib.to_lowercase());
        }
    }
}

/// `SLIPSTREAM_SOFTGL_DIR`: a folder with the gzipped Mesa DLLs named in
/// packaging/windows/softgl.txt (`opengl32.dll.gz`, …), as
/// scripts/build-station.sh prepares them. Without it the app is built
/// without the software renderer.
fn software_renderer(windows: bool) {
    println!("cargo:rerun-if-env-changed=SLIPSTREAM_SOFTGL_DIR");
    println!("cargo:rerun-if-changed=../../packaging/windows/softgl.txt");
    let pins = std::fs::read_to_string("../../packaging/windows/softgl.txt").expect("packaging/windows/softgl.txt");
    let names: Vec<&str> = pins.lines().filter_map(|l| l.strip_prefix("file ")?.split_whitespace().next()).collect();
    let files = match std::env::var("SLIPSTREAM_SOFTGL_DIR") {
        Ok(dir) if windows => {
            let entries: Vec<String> = names
                .iter()
                .map(|name| {
                    let path = std::path::Path::new(&dir).join(format!("{name}.gz"));
                    let path = path.canonicalize().unwrap_or_else(|_| panic!("SLIPSTREAM_SOFTGL_DIR has no {}", path.display()));
                    println!("cargo:rerun-if-changed={}", path.display());
                    format!("({name:?}, include_bytes!({:?}))", path.display().to_string())
                })
                .collect();
            format!("Some(&[{}])", entries.join(", "))
        }
        _ => "None".into(),
    };
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("softgl.rs");
    std::fs::write(out, format!("pub const EMBEDDED: Option<&[(&str, &[u8])]> = {files};\n")).unwrap();
}
