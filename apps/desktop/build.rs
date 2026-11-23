//! On Windows, puts the Wall icon and version details into wall.exe, so
//! Explorer, the taskbar, the Start menu and Settings > Apps show them.
//! Needs the Windows SDK's resource compiler (rc.exe), which comes with the
//! Visual Studio Build Tools that build Rust on Windows anyway.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/wall.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/wall.ico")
        .set("ProductName", "Wall")
        .set("FileDescription", "Wall")
        .set("InternalName", "wall.exe")
        .set("OriginalFilename", "wall.exe");
    // A missing resource compiler shouldn't stop a build: Wall just has the
    // default icon then.
    if let Err(e) = res.compile() {
        println!("cargo:warning=wall.exe gets no icon: {e}");
    }
}
