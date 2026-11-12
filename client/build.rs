//! Embeds the app icon into the Windows executables.

fn main() {
    println!("cargo:rerun-if-changed=assets/synctube.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let icon = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("assets")
        .join("synctube.ico");
    if !icon.exists() {
        // First run of the `write_ico` test generates it.
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let rc = out.join("synctube.rc");
    let icon = icon.to_string_lossy().replace('\\', "/");
    std::fs::write(&rc, format!("1 ICON \"{icon}\"\n")).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
