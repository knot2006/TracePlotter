//! Imports browser-stored sessions once, without starting a webview.
use super::*;
use serde_json::Value;

pub(super) fn load(dir: &Path) -> Result<Option<Session>, String> {
    let browser = dir.join("webview");
    read_browser(&browser)
        .and_then(|values| values.as_ref().map(decode).transpose())
        .map_err(|e| format!(
            "Could not import the saved browser session: {e}. Close the old app and retry. The original data has been kept."
        ))
}

fn read_browser(browser: &Path) -> Result<Option<Value>, String> {
    // Production origins take precedence over a development server's separate profile.
    for name in [
        "tauri_localhost_0",
        "http_tauri.localhost_0",
        "https_tauri.localhost_0",
        "http_localhost_1420",
    ] {
        let path = browser
            .join("localstorage")
            .join(format!("{name}.localstorage"));
        if path.try_exists().map_err(|e| e.to_string())? {
            let values = read_sqlite(&path)?;
            if has_session(&values) {
                return Ok(Some(values));
            }
        }
    }
    for profile in [browser.join("EBWebView/Default"), browser.join("Default")] {
        let path = profile.join("Local Storage/leveldb");
        if path
            .join("CURRENT")
            .try_exists()
            .map_err(|e| e.to_string())?
        {
            let values = read_leveldb(&path)?;
            if let Some(values) = values {
                return Ok(Some(values));
            }
        }
    }
    Ok(None)
}

fn has_session(values: &Value) -> bool {
    ["traceplotter.session", "traceplotter.settings"]
        .iter()
        .any(|key| values[*key].is_string())
}

fn utf16(bytes: &[u8]) -> Result<String, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("incomplete UTF-16 browser value".into());
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();
    String::from_utf16(&units).map_err(|e| e.to_string())
}

fn chromium_string(bytes: &[u8]) -> Result<String, String> {
    match bytes.split_first() {
        Some((0, text)) => utf16(text),
        Some((1, text)) => Ok(text.iter().map(|b| char::from(*b)).collect()),
        _ => Err("unknown Chromium storage encoding".into()),
    }
}

fn read_sqlite(path: &Path) -> Result<Value, String> {
    // Work on a copy, including pending WAL writes, so opening SQLite never touches the profile.
    let copy = tempfile::tempdir().map_err(|e| e.to_string())?;
    let database = copy.path().join("storage.sqlite");
    fs::copy(path, &database).map_err(|e| e.to_string())?;
    let wal = path.with_file_name(format!(
        "{}-wal",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    if wal.try_exists().map_err(|e| e.to_string())? {
        fs::copy(wal, database.with_file_name("storage.sqlite-wal")).map_err(|e| e.to_string())?;
    }
    let read = || -> Result<Value, String> {
        let connection = rusqlite::Connection::open(&database).map_err(|e| e.to_string())?;
        let mut query = connection
            .prepare("SELECT key, value FROM ItemTable WHERE key LIKE 'traceplotter.%'")
            .map_err(|e| e.to_string())?;
        let mut rows = query.query([]).map_err(|e| e.to_string())?;
        let mut values = serde_json::Map::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let key: String = row.get(0).map_err(|e| e.to_string())?;
            let value = match row.get_ref(1).map_err(|e| e.to_string())? {
                rusqlite::types::ValueRef::Blob(bytes) => utf16(bytes)?,
                value => value.as_str().map_err(|e| e.to_string())?.to_owned(),
            };
            values.insert(key, Value::String(value));
        }
        Ok(Value::Object(values))
    };
    read().map_err(|e| format!("{}: {e}", path.display()))
}

fn read_leveldb(path: &Path) -> Result<Option<Value>, String> {
    use rusty_leveldb::{DB, Options};
    // LevelDB recovery writes its log and manifest. Only the disposable copy may be opened.
    let copy = tempfile::tempdir().map_err(|e| e.to_string())?;
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_file() && entry.file_name() != "LOCK" {
            fs::copy(entry.path(), copy.path().join(entry.file_name()))
                .map_err(|e| e.to_string())?;
        }
    }
    let mut database = DB::open(
        copy.path(),
        Options {
            create_if_missing: false,
            paranoid_checks: true,
            ..Options::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let snapshot = database.get_snapshot();
    for origin in [
        "http://tauri.localhost",
        "https://tauri.localhost",
        "tauri://localhost",
        "http://localhost:1420",
    ] {
        let mut values = serde_json::Map::new();
        for key in [
            "session",
            "settings",
            "activeTab",
            "hiddenColumns",
            "columnOrder",
            "columnWidths",
            "compare",
            "extraColumnsAdded",
            "tableHeight",
            "timeSpan",
            "checkUpdatesAtStart",
            "mcpServer",
        ] {
            let name = format!("traceplotter.{key}");
            for encoding in [1, 0] {
                let mut key = format!("_{origin}\0").into_bytes();
                key.push(encoding);
                if encoding == 1 {
                    key.extend_from_slice(name.as_bytes());
                } else {
                    key.extend(name.encode_utf16().flat_map(u16::to_le_bytes));
                }
                if let Some(value) = database
                    .get_at(&snapshot, &key)
                    .map_err(|e| e.to_string())?
                {
                    values.insert(name.clone(), Value::String(chromium_string(&value)?));
                    break;
                }
            }
        }
        let values = Value::Object(values);
        if has_session(&values) {
            return Ok(Some(values));
        }
    }
    Ok(None)
}

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
        return Err("No TracePlotter storage was found at this browser origin. Original browser data has been kept; restore it from a backup to retry.".into());
    }
    let settings = parsed("traceplotter.settings")?;
    if values["traceplotter.settings"].is_string() && !settings.is_object() {
        return Err("invalid legacy settings: expected an object".into());
    }
    let mut state = Session {
        settings: logic::clean_settings(&settings, cfg!(any(windows, target_os = "linux"))),
        ..Session::default()
    };
    if let Some(text) = values["traceplotter.session"].as_str() {
        state.targets =
            serde_json::from_str(text).map_err(|e| format!("invalid legacy session: {e}"))?;
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
        assert!(decode(&json!({"traceplotter.settings":"[]"})).is_err());
        for session in ["broken", "{}", "null", "42"] {
            assert!(decode(&json!({"traceplotter.session":session})).is_err());
        }
    }
    #[test]
    fn chromium_storage_imports_unicode_and_keeps_the_original_database() {
        use rusty_leveldb::{DB, Options};
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("webview/EBWebView/Default/Local Storage/leveldb");
        fs::create_dir_all(&path).unwrap();
        let mut database = DB::open(&path, Options::default()).unwrap();
        let key =
            |origin: &str, key: &str| format!("_{origin}\0\x01traceplotter.{key}").into_bytes();
        let value = r#"[{"sessionId":"original-id","host":"żółć.example","paused":true}]"#;
        let mut encoded = vec![0];
        encoded.extend(value.encode_utf16().flat_map(u16::to_le_bytes));
        database
            .put(&key("http://tauri.localhost", "session"), b"\x01[]")
            .unwrap();
        let mut settings_key = b"_http://tauri.localhost\0\x00".to_vec();
        settings_key.extend(
            "traceplotter.settings"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        );
        database
            .put(&settings_key, b"\x01{\"historyDays\":14}")
            .unwrap();
        database.compact_range(b"", b"\xff").unwrap();
        // The WAL's newer session must win over the older value in a table file.
        database
            .put(&key("http://tauri.localhost", "session"), &encoded)
            .unwrap();
        database
            .put(&key("http://localhost:1420", "session"), b"\x01[]")
            .unwrap();
        database
            .put(&key("https://foreign.example", "settings"), b"\x01broken")
            .unwrap();
        database.flush().unwrap();
        drop(database);
        let before: BTreeMap<_, _> = fs::read_dir(&path)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (e.file_name(), fs::read(e.path()).unwrap())
            })
            .collect();
        let session = load(dir.path()).unwrap().unwrap();
        assert_eq!(session.targets[0].session_id, "original-id");
        assert_eq!(session.targets[0].host, "żółć.example");
        assert_eq!(session.settings.history_days, 14);
        assert!(session.targets[0].paused);
        let after: BTreeMap<_, _> = fs::read_dir(&path)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (e.file_name(), fs::read(e.path()).unwrap())
            })
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn storage_decoders_reject_truncated_and_unknown_encodings() {
        assert_eq!(chromium_string(b"\x01caf\xe9").unwrap(), "café");
        assert!(chromium_string(b"\x02text").is_err());
        assert!(chromium_string(b"\x00x").is_err());
        assert!(utf16(&[0x00, 0xd8]).is_err());
    }
}
