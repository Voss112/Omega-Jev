//! Embeds the Windows icon into the executable.

use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=assets/omega.rc");
    println!("cargo:rerun-if-changed=assets/omega.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        embed_resource::compile("assets/omega.rc", embed_resource::NONE).manifest_optional().expect("windows resources");
        return;
    }
    // GNU: call windres from the assets dir with relative input (embed-resource mis-quotes paths with spaces).
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("omega-res.o");
    let status = Command::new("windres")
        .current_dir("assets")
        .args(["-i", "omega.rc", "-O", "coff", "-o"])
        .arg(&out)
        .status()
        .expect("windres not found (install MinGW binutils)");
    assert!(status.success(), "windres failed");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}
