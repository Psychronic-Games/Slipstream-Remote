//! Embeds the icon and version info into the Windows executable.
fn main() {
    println!("cargo:rerun-if-changed=../../packaging/windows/slipstream.rc");
    println!("cargo:rerun-if-changed=../../packaging/windows/slipstream.ico");
    println!("cargo:rerun-if-changed=../../packaging/windows/slipstream.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("../../packaging/windows/slipstream.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("embedding Windows resources");
    }
}
