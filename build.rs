fn main() {
    // App icon (shown in Explorer and Task Manager).
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
    // DPI awareness, common controls v6, Windows 10+ compatibility.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("app.manifest");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
}
