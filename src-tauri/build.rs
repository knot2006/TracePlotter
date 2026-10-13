// Raw sockets need Administrator on Windows, so the exe asks for elevation.
// The Common-Controls dependency is required by Tauri whenever the manifest is replaced.
const MANIFEST: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" />
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#;

fn main() {
    // The exe's icon is compiled into it here. Without this line a changed icon file does
    // not rerun the build script, and the exe keeps the old icon.
    println!("cargo:rerun-if-changed=icons");
    let windows = tauri_build::WindowsAttributes::new().app_manifest(MANIFEST);
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
}
