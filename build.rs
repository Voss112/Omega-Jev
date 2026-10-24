//! Embeds the Windows icon and version resource into the executable.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=assets/omega.rc");
    println!("cargo:rerun-if-changed=assets/omega.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let rc = out_dir.join("omega.rc");
    write_resources(&rc);
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().expect("windows resources");
        return;
    }
    // GNU: call windres from OUT_DIR with relative input (embed-resource mis-quotes paths with spaces).
    let out = out_dir.join("omega-res.o");
    let status = Command::new("windres")
        .current_dir(&out_dir)
        .args(["-i", "omega.rc", "-O", "coff", "-o"])
        .arg(&out)
        .status()
        .expect("windres not found (install MinGW binutils)");
    assert!(status.success(), "windres failed");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

/// Fills the template: the file version always follows Cargo.toml, the icon is referenced by absolute path.
fn write_resources(rc: &Path) {
    let var = |key: &str| env::var(key).unwrap_or_else(|_| panic!("{key}"));
    let icon = Path::new(&var("CARGO_MANIFEST_DIR")).join("assets").join("omega.ico");
    let text = fs::read_to_string("assets/omega.rc")
        .expect("assets/omega.rc")
        .replace("@ICON@", &icon.display().to_string().replace('\\', "\\\\"))
        .replace("@MAJOR@", &var("CARGO_PKG_VERSION_MAJOR"))
        .replace("@MINOR@", &var("CARGO_PKG_VERSION_MINOR"))
        .replace("@PATCH@", &var("CARGO_PKG_VERSION_PATCH"))
        .replace("@VERSION@", &var("CARGO_PKG_VERSION"));
    fs::write(rc, text).expect("generated omega.rc");
}
