//! Packages the native executable without the Tauri CLI's webview dependencies.
use std::path::Path;
use std::process::Command;
use tauri_bundler::{
    AppCategory, BundleBinary, BundleSettings, DebianSettings, PackageSettings, PackageType,
    RpmSettings, SettingsBuilder,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or("packaging tool must live under tools/bundle-linux")?;
    let metadata = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(app.join("Cargo.toml"))
        .output()?;
    if !metadata.status.success() {
        return Err(String::from_utf8_lossy(&metadata.stderr)
            .into_owned()
            .into());
    }
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout)?;
    let package = &metadata["packages"][0];
    let text = |key: &str| -> Result<String, Box<dyn std::error::Error>> {
        Ok(package[key]
            .as_str()
            .ok_or_else(|| format!("missing package {key}"))?
            .into())
    };
    let output = app.join("target/release");
    if !output.join("traceplotter").is_file() {
        return Err("build the native release executable before packaging".into());
    }
    std::env::set_current_dir(app)?;
    let settings = SettingsBuilder::new()
        .package_settings(PackageSettings {
            product_name: "TracePlotter".into(),
            version: text("version")?,
            description: text("description")?,
            homepage: Some(text("repository")?),
            authors: None,
            default_run: Some("traceplotter".into()),
        })
        .bundle_settings(BundleSettings {
            identifier: Some("io.github.knot2006.traceplotter".into()),
            publisher: Some("TracePlotter contributors".into()),
            icon: Some(vec![
                "icons/32x32.png".into(),
                "icons/128x128.png".into(),
                "icons/128x128@2x.png".into(),
            ]),
            license: Some(text("license")?),
            category: Some(AppCategory::Utility),
            deb: DebianSettings {
                depends: Some(vec![
                    "libcap2-bin".into(),
                    "libxkbcommon0".into(),
                    "libwayland-client0".into(),
                    "libwayland-cursor0".into(),
                    "libx11-6".into(),
                    "libxcursor1".into(),
                    "libxrandr2".into(),
                    "libxi6".into(),
                    "libxcb1".into(),
                    "libegl1".into(),
                    "libgl1".into(),
                    "libvulkan1".into(),
                    "libdbus-1-3".into(),
                ]),
                post_install_script: Some("linux/postinst.sh".into()),
                ..Default::default()
            },
            rpm: RpmSettings {
                release: "1".into(),
                depends: Some(vec![
                    "libcap".into(),
                    "libxkbcommon.so.0()(64bit)".into(),
                    "libwayland-client.so.0()(64bit)".into(),
                    "libwayland-cursor.so.0()(64bit)".into(),
                    "libX11.so.6()(64bit)".into(),
                    "libXcursor.so.1()(64bit)".into(),
                    "libXrandr.so.2()(64bit)".into(),
                    "libXi.so.6()(64bit)".into(),
                    "libxcb.so.1()(64bit)".into(),
                    "libEGL.so.1()(64bit)".into(),
                    "libGL.so.1()(64bit)".into(),
                    "libvulkan.so.1()(64bit)".into(),
                    "libdbus-1.so.3()(64bit)".into(),
                ]),
                post_install_script: Some("linux/postinst.sh".into()),
                ..Default::default()
            },
            ..Default::default()
        })
        .binaries(vec![BundleBinary::new("traceplotter".into(), true)])
        .package_types(vec![PackageType::Deb, PackageType::Rpm])
        .binary_patching(false)
        .project_out_directory(output)
        .build()?;
    tauri_bundler::bundle_project(&settings)?;
    Ok(())
}
