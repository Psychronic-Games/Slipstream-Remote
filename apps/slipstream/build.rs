//! Embeds the icon and version info into the Windows executable, and
//! delay-loads OpenGL there.
fn main() {
    println!("cargo:rerun-if-changed=../../packaging/windows/slipstream.rc");
    println!("cargo:rerun-if-changed=../../packaging/windows/slipstream.ico");
    println!("cargo:rerun-if-changed=../../packaging/windows/slipstream.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("../../packaging/windows/slipstream.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("embedding Windows resources");
        // OpenGL loads on first use instead of at start, so the software
        // driver's folder (SetDllDirectory, see gui::use_software_gl) is
        // searched before System32's opengl32.dll.
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:opengl32.dll");
            println!("cargo:rustc-link-lib=delayimp");
        }
    }
}
