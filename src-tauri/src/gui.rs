//! A saved interface choice; switching restarts after the old instance has exited.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Interface {
    #[default]
    Classic,
    Native,
}
impl Interface {
    fn available(self) -> bool {
        match self {
            Self::Classic => cfg!(feature = "legacy-ui"),
            Self::Native => cfg!(feature = "egui-ui"),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Choice {
    interface: Interface,
    import_native: bool,
}
impl Choice {
    fn load(dir: &Path) -> Result<Self, String> {
        match fs::read(dir.join("interface.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    fn save(&self, dir: &Path) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        atomic_write(
            &dir.join("interface.json"),
            &serde_json::to_vec(self).map_err(|e| e.to_string())?,
        )
    }
}

#[cfg(any(feature = "egui-ui", test))]
fn resolve(preferred: Interface, args: impl IntoIterator<Item = String>) -> Interface {
    let desired = args
        .into_iter()
        .fold(preferred, |current, arg| match arg.as_str() {
            "--egui" => Interface::Native,
            "--classic" => Interface::Classic,
            _ => current,
        });
    if desired.available() {
        desired
    } else if Interface::Classic.available() {
        Interface::Classic
    } else {
        Interface::Native
    }
}
#[cfg(feature = "egui-ui")]
pub(crate) fn selected() -> Interface {
    let preferred = Choice::load(data_dir()).unwrap_or_else(|error| {
        eprintln!("Could not read interface choice: {error}");
        Choice::default()
    });
    resolve(preferred.interface, std::env::args())
}

fn switch_args(args: Vec<String>, interface: Interface) -> Vec<String> {
    args.into_iter()
        .filter(|arg| !matches!(arg.as_str(), "--egui" | "--classic"))
        .chain(std::iter::once(
            match interface {
                Interface::Native => "--egui",
                Interface::Classic => "--classic",
            }
            .into(),
        ))
        .collect()
}

pub(crate) fn restart(interface: Interface) -> Result<(), String> {
    if !interface.available() {
        return Err("This build does not include the selected interface.".into());
    }
    let dir = data_dir();
    let previous = Choice::load(dir)?;
    Choice {
        interface,
        import_native: interface == Interface::Classic,
    }
    .save(dir)?;
    let result = std::env::current_exe().and_then(|exe| {
        let mut command = std::process::Command::new(exe);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // The new GUI must outlive a launcher that ends the old process group.
            command.process_group(0);
        }
        command.args(switch_args(restart_args(), interface)).spawn()
    });
    if let Err(error) = result {
        previous.save(dir).map_err(|rollback| {
            format!("Could not restart: {error}. Could not restore interface choice: {rollback}")
        })?;
        return Err(format!("Could not restart: {error}"));
    }
    Ok(())
}

#[cfg(feature = "legacy-ui")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Status {
    native_available: bool,
}
#[cfg(feature = "legacy-ui")]
#[tauri::command]
pub(super) fn gui_status() -> Status {
    Status {
        native_available: Interface::Native.available(),
    }
}

#[cfg(feature = "legacy-ui")]
#[tauri::command(async)]
pub(super) fn switch_gui(app: AppHandle, values: serde_json::Value) -> Result<(), String> {
    if !Interface::Native.available() {
        return Err("This build does not include the native interface.".into());
    }
    #[cfg(feature = "egui-ui")]
    native::migration::import_values(&values)?;
    #[cfg(not(feature = "egui-ui"))]
    let _ = values;
    restart(Interface::Native)?;
    background::exit_app(app);
    Ok(())
}

#[cfg(feature = "legacy-ui")]
pub(super) fn legacy_script() -> Result<String, String> {
    #[cfg(feature = "egui-ui")]
    if Choice::load(data_dir()).is_ok_and(|choice| choice.import_native) {
        let values = native::migration::export_values()?;
        return Ok(format!(
            r#"(() => {{
            const values = {values};
            if (!localStorage.getItem('traceplotter.classicBeforeNative')) {{
                localStorage.setItem('traceplotter.classicBeforeNative', JSON.stringify(Object.fromEntries(Object.keys(values).map(key => [key, localStorage.getItem(key)]))));
            }}
            for (const [key, value] of Object.entries(values)) {{
                if (value === null) localStorage.removeItem(key);
                else localStorage.setItem(key, value);
            }}
            window.addEventListener('DOMContentLoaded', () => {{
                window.__TAURI_INTERNALS__.invoke('finish_gui_import').catch(console.error);
            }}, {{ once: true }});
        }})();"#
        ));
    }
    Ok(String::new())
}

#[cfg(feature = "legacy-ui")]
#[tauri::command]
pub(super) fn finish_gui_import() -> Result<(), String> {
    #[cfg(feature = "egui-ui")]
    {
        let mut choice = Choice::load(data_dir())?;
        if choice.import_native {
            choice.import_native = false;
            choice.save(data_dir())?;
        }
    }
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension("json.new");
    let mut file = fs::File::create(&temp).map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    #[cfg(not(windows))]
    fs::rename(&temp, path).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let from: Vec<u16> = temp.as_os_str().encode_wide().chain([0]).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: both paths are valid, zero-terminated UTF-16 buffers for this call.
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_interface_overrides_preference_without_losing_restart_flags() {
        let args = switch_args(
            vec![
                "--demo".into(),
                "--demo-days".into(),
                "7".into(),
                "--mcp".into(),
                "--egui".into(),
                "--after".into(),
                "123".into(),
            ],
            Interface::Classic,
        );
        assert_eq!(
            args,
            [
                "--demo",
                "--demo-days",
                "7",
                "--mcp",
                "--after",
                "123",
                "--classic"
            ]
        );
        if Interface::Classic.available() {
            assert_eq!(resolve(Interface::Native, args), Interface::Classic);
        }
        if Interface::Native.available() {
            assert_eq!(
                resolve(Interface::Classic, ["--egui".into()]),
                Interface::Native
            );
        }
    }
    #[test]
    fn preference_round_trips_and_defaults_to_classic() {
        let dir =
            std::env::temp_dir().join(format!("traceplotter-interface-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(Choice::load(&dir).unwrap().interface, Interface::Classic);
        Choice {
            interface: Interface::Native,
            import_native: false,
        }
        .save(&dir)
        .unwrap();
        assert_eq!(Choice::load(&dir).unwrap().interface, Interface::Native);
        Choice {
            interface: Interface::Classic,
            import_native: true,
        }
        .save(&dir)
        .unwrap();
        assert!(Choice::load(&dir).unwrap().import_native);
        fs::remove_dir_all(dir).unwrap();
    }
}
