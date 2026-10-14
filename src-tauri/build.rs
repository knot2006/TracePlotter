fn main() {
    // The exe's icon is compiled into it here. Without this line a changed icon file does
    // not rerun the build script, and the exe keeps the old icon.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
