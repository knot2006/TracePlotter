fn main() {
    // The exe's icon is compiled into it here. Without this line a changed icon file does
    // not rerun the build script, and the exe keeps the old icon.
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-changed=windows.manifest");
    #[cfg(feature = "legacy-ui")]
    tauri_build::build();
    #[cfg(all(windows, feature = "egui-ui", not(feature = "legacy-ui")))]
    tauri_winres::WindowsResource::new()
        .set_icon("icons/icon.ico")
        .set_manifest(&include_str!("windows.manifest").replace(
            "@VERSION@",
            concat!(
                env!("CARGO_PKG_VERSION_MAJOR"),
                ".",
                env!("CARGO_PKG_VERSION_MINOR"),
                ".",
                env!("CARGO_PKG_VERSION_PATCH"),
                ".0"
            ),
        ))
        .compile()
        .expect("Windows icon and manifest must compile");
}
