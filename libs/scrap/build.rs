// Modified for desklib by Matt Timmermans, 2026 (from RustDesk's libs/scrap). See NOTICE.md.

fn main() {
    // in this crate, these are also valid configurations
    println!("cargo:rustc-check-cfg=cfg(dxgi,quartz,x11)");

    match std::env::var("CARGO_CFG_TARGET_OS").unwrap().as_str() {
        // The first choice is Windows because DXGI is amazing.
        "windows" => println!("cargo:rustc-cfg=dxgi"),
        // Quartz is second because macOS is the (annoying) exception.
        "macos" => println!("cargo:rustc-cfg=quartz"),
        // On other UNIXes we pray that X11 (with XCB) is available.
        _ if std::env::var_os("CARGO_CFG_UNIX").is_some() => println!("cargo:rustc-cfg=x11"),
        _ => {}
    }
}
