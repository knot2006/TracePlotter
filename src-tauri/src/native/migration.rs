//! Reads legacy storage through its original webview origin, in a separate process.
use super::*;
#[cfg(any(feature = "legacy-ui", test))]
use serde_json::Value;

pub(super) fn prepare() -> Result<(), String> {
    if data_dir().join("native-state.json").exists() {
        return Ok(());
    }
    let legacy = data_dir().join("webview");
    if !fs::read_dir(&legacy).is_ok_and(|mut files| files.next().is_some()) {
        return Ok(());
    }
    #[cfg(feature = "legacy-migration")]
    {
        let mut command =
            std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        command.arg("--migrate-legacy-state");
        if demo() {
            command.arg("--demo");
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot start session migration: {e}"))?;
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "Legacy session migration timed out. Original browser data has been kept."
                        .into(),
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if !status.success() || !data_dir().join("native-state.json").exists() {
            return Err("Legacy session migration did not complete. The original browser data has been kept. Run the legacy interface to recover or retry.".into());
        }
        Ok(())
    }
    #[cfg(not(feature="legacy-migration"))]
    Err("Saved browser data exists. Build with legacy-migration to import it before using the native interface. The original data has been kept.".into())
}

#[cfg(feature = "legacy-migration")]
#[tauri::command]
fn store_migration(app: tauri::AppHandle, values: Value) -> Result<(), String> {
    let state = decode(&values)?;
    state.save()?;
    app.exit(0);
    Ok(())
}

#[cfg(feature = "legacy-migration")]
#[tauri::command]
fn migration_failed(app: tauri::AppHandle, error: String) {
    eprintln!("Legacy session migration failed: {error}");
    app.exit(1);
}

#[cfg(any(feature = "legacy-ui", test))]
fn decode(values: &Value) -> Result<Session, String> {
    let parsed = |key: &str| -> Result<Value, String> {
        match values[key].as_str() {
            Some(text) => {
                serde_json::from_str(text).map_err(|e| format!("invalid legacy {key}: {e}"))
            }
            None => Ok(Value::Null),
        }
    };
    if ["traceplotter.session", "traceplotter.settings"]
        .iter()
        .all(|key| values[key].is_null())
    {
        return Err("No TracePlotter storage was found at this browser origin. Original browser data has been kept; run the matching legacy interface to recover it.".into());
    }
    let mut state = Session {
        settings: logic::clean_settings(
            &parsed("traceplotter.settings")?,
            cfg!(any(windows, target_os = "linux")),
        ),
        ..Session::default()
    };
    if let Some(targets) = parsed("traceplotter.session")?.as_array() {
        state.targets = targets
            .iter()
            .map(|v| serde_json::from_value(v.clone()).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
    }
    if let Some(active) = values["traceplotter.activeTab"].as_str() {
        state.active = active.into();
    }
    for (key, out) in [
        ("traceplotter.hiddenColumns", &mut state.hidden_columns),
        ("traceplotter.columnOrder", &mut state.column_order),
        ("traceplotter.compare", &mut state.compare),
    ] {
        let v = parsed(key)?;
        if !v.is_null() {
            *out = serde_json::from_value(v).map_err(|e| format!("invalid legacy {key}: {e}"))?;
        }
    }
    // A legacy profile from before 0.1.13 has not seen the detailed columns yet.
    state.extra_columns_added = values["traceplotter.extraColumnsAdded"].as_str() == Some("yes");
    let widths = parsed("traceplotter.columnWidths")?;
    if !widths.is_null() {
        state.column_widths = serde_json::from_value(widths).map_err(|e| e.to_string())?;
    }
    state.table_height = values["traceplotter.tableHeight"]
        .as_str()
        .and_then(|s| s.parse::<f32>().ok());
    state.span = values["traceplotter.timeSpan"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(600.0);
    state.check_updates = values["traceplotter.checkUpdatesAtStart"].as_str() != Some("no");
    state.mcp = values["traceplotter.mcpServer"].as_str() == Some("on");
    if state.active.is_empty() {
        state.active = state
            .targets
            .first()
            .map_or_else(|| "summary".into(), |t| t.session_id.clone());
    }
    Session::decode(&serde_json::to_string(&state).map_err(|e| e.to_string())?)
}

#[cfg(feature = "legacy-ui")]
pub(crate) fn import_values(values: &Value) -> Result<(), String> {
    let mut state = decode(values)?;
    // The classic interface has no theme preference, so keep the native choice.
    state.theme = Session::load()?.theme;
    state.save()
}

#[cfg(feature = "legacy-ui")]
pub(crate) fn export_values() -> Result<Value, String> {
    legacy_values(&Session::load()?)
}

#[cfg(any(feature = "legacy-ui", test))]
fn legacy_values(state: &Session) -> Result<Value, String> {
    let mut values = serde_json::Map::new();
    for (key, value) in [
        ("session", serde_json::to_value(&state.targets)),
        ("settings", serde_json::to_value(&state.settings)),
        ("hiddenColumns", serde_json::to_value(&state.hidden_columns)),
        ("columnOrder", serde_json::to_value(&state.column_order)),
        ("columnWidths", serde_json::to_value(&state.column_widths)),
        ("compare", serde_json::to_value(&state.compare)),
    ] {
        values.insert(
            format!("traceplotter.{key}"),
            Value::String(value.map_err(|e| e.to_string())?.to_string()),
        );
    }
    for (key, value) in [
        ("activeTab", state.active.clone()),
        ("timeSpan", state.span.to_string()),
        (
            "checkUpdatesAtStart",
            if state.check_updates { "yes" } else { "no" }.into(),
        ),
        ("mcpServer", if state.mcp { "on" } else { "off" }.into()),
        (
            "extraColumnsAdded",
            if state.extra_columns_added {
                "yes"
            } else {
                "no"
            }
            .into(),
        ),
    ] {
        values.insert(format!("traceplotter.{key}"), Value::String(value));
    }
    values.insert(
        "traceplotter.tableHeight".into(),
        state
            .table_height
            .map_or(Value::Null, |height| Value::String(height.to_string())),
    );
    Ok(Value::Object(values))
}

#[cfg(feature = "legacy-migration")]
pub(crate) fn run() {
    let result = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![store_migration, migration_failed])
        .setup(|app| {
            let window=tauri::WebviewWindowBuilder::new(app,"main",tauri::WebviewUrl::App("migration.html".into())).data_directory(data_dir().join("webview")).visible(false);
            #[cfg(debug_assertions)]
            let window=if let Some(path)=std::env::var_os("TRACEPLOTTER_MIGRATION_FIXTURE").filter(|_|demo()) {
                let values:Value=serde_json::from_slice(&fs::read(path)?)?;
                window.initialization_script(format!("for (const [key,value] of Object.entries({values})) {{if (value !== null) localStorage.setItem(key,value);}}"))
            }else{window};
            window.build()?;
            Ok(())
        })
        .run(tauri::generate_context!());
    if result.is_err() {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn exports_all_layout_and_session_keys_without_changing_ids() {
        let legacy = json!({"traceplotter.session":r#"[{"sessionId":"original","host":"example.com","paused":true}]"#,"traceplotter.activeTab":"original","traceplotter.mcpServer":"on","traceplotter.hiddenColumns":"[\"mos\",\"name\"]","traceplotter.columnWidths":"{\"ip\":240}","traceplotter.tableHeight":"300","traceplotter.timeSpan":"3600","traceplotter.compare":"[\"original\"]"});
        let state = decode(&legacy).unwrap();
        assert_eq!(state.targets[0].session_id, "original");
        assert!(state.targets[0].paused);
        assert!(state.mcp);
        assert_eq!(state.span, 3600.0);
        assert_eq!(state.column_widths["ip"], 240.0);
        assert_eq!(state.active, "original");
    }
    #[test]
    fn corrupt_legacy_session_is_reported_instead_of_overwritten() {
        assert!(decode(&json!({"traceplotter.session":"broken"})).is_err());
    }
    #[test]
    fn switching_interfaces_keeps_sessions_settings_and_layout() {
        let source = json!({"traceplotter.session":r#"[{"sessionId":"kept-id","host":"example.com","paused":true}]"#,"traceplotter.activeTab":"kept-id","traceplotter.settings":r#"{"warnMs":150,"historyDays":14}"#,"traceplotter.timeSpan":"3600","traceplotter.tableHeight":"200","traceplotter.hiddenColumns":"[\"mos\"]","traceplotter.columnOrder":"[\"name\",\"ip\"]","traceplotter.columnWidths":"{\"ip\":220}","traceplotter.compare":"[\"kept-id\"]","traceplotter.mcpServer":"on","traceplotter.checkUpdatesAtStart":"no"});
        let original = decode(&source).unwrap();
        let values = legacy_values(&original).unwrap();
        let restored = decode(&values).unwrap();
        assert_eq!(
            serde_json::to_value(&original).unwrap(),
            serde_json::to_value(restored).unwrap()
        );
        assert_eq!(values["traceplotter.extraColumnsAdded"], "yes");
    }
}
