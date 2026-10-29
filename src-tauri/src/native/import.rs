//! Samples CSV import uses the recorder's existing JSONL format.
use super::*;
use serde_json::{Value, json};

pub(super) fn parse(text: &str) -> Result<Vec<Value>, String> {
    let mut lines = text.lines();
    if lines
        .next()
        .unwrap_or_default()
        .trim_start_matches('\u{feff}')
        .trim()
        != "time,hop,ip,rtt_ms"
    {
        return Err(
            "not a TracePlotter samples export: its first line is not time,hop,ip,rtt_ms".into(),
        );
    }
    let mut rounds = BTreeMap::<i64, Vec<Value>>::new();
    for line in lines {
        let cells = line.split(',').collect::<Vec<_>>();
        let Some(t) = cells
            .first()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.timestamp_millis())
        else {
            continue;
        };
        let Some(ttl) = cells
            .get(1)
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.fract() == 0.0 && (1.0..=255.0).contains(v))
        else {
            continue;
        };
        let addr = cells.get(2).filter(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        });
        let rtt = cells
            .get(3)
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|n| n.is_finite() && *n >= 0.0);
        rounds
            .entry(t)
            .or_default()
            .push(json!({"ttl":ttl as u8,"addr":addr,"rtt":rtt}));
    }
    if rounds.is_empty() {
        return Err("the export holds no samples".into());
    }
    Ok(rounds
        .into_iter()
        .map(|(t, mut hops)| {
            hops.sort_by_key(|h| h["ttl"].as_u64());
            json!({"t":t,"hops":hops})
        })
        .collect())
}
pub(super) fn interval(samples: &[Value]) -> u64 {
    let mut gaps = samples
        .windows(2)
        .map(|w| w[1]["t"].as_i64().unwrap_or(0) - w[0]["t"].as_i64().unwrap_or(0))
        .collect::<Vec<_>>();
    gaps.sort_unstable();
    let usual = gaps.get(gaps.len() / 2).copied().unwrap_or(1000);
    [500u64, 1000, 2500, 5000, 10000]
        .into_iter()
        .min_by_key(|ms| (*ms as i64 - usual).abs())
        .unwrap_or(1000)
}
pub(super) fn host(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("traceplotter_")
        && lower.ends_with(".csv")
        && let Some(index) = lower.rfind("_samples_")
        && index > 13
        && index + 9 < name.len() - 4
    {
        return name[13..index].into();
    }
    if lower.starts_with("traceplotter-") && lower.ends_with(".csv") {
        let rest = &name[13..name.len() - 4];
        let parts = rest.rsplitn(3, '-').collect::<Vec<_>>();
        if parts.len() == 3
            && parts[0].len() == 6
            && parts[1].len() == 8
            && parts[..2]
                .iter()
                .all(|p| p.chars().all(|c| c.is_ascii_digit()))
        {
            return parts[2].into();
        }
    }
    if lower.ends_with(".csv") {
        name[..name.len() - 4].into()
    } else {
        name.into()
    }
}

pub(super) fn pick(settings: &logic::Settings) -> Result<Option<Config>, String> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Samples CSV", &["csv"])
        .pick_file()
    else {
        return Ok(None);
    };
    if fs::metadata(&path).map_err(|e| e.to_string())?.len() > IMPORT_MAX_BYTES {
        return Err(format!("{} is too large to import", path.display()));
    }
    let samples = parse(&fs::read_to_string(&path).map_err(|e| e.to_string())?)?;
    let mut config = Config::new(
        host(&path.file_name().unwrap_or_default().to_string_lossy()),
        settings,
    );
    config.interval_ms = interval(&samples);
    config.paused = true;
    let dir = history_dir(&config.session_id)?;
    if !day_files(&dir).is_empty() {
        return Err("the import session already holds history".into());
    }
    let lines = samples.iter().map(Value::to_string).collect::<Vec<_>>();
    write_lines(&dir, lines.iter().map(String::as_str)).map_err(|e| e.to_string())?;
    Ok(Some(config))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_and_reordered_csv() {
        let text = "\u{feff}time,hop,ip,rtt_ms\r\nsoon,1,10.0.0.1,1\r\n2026-10-14T01:51:22Z,0,10.0.0.1,1\r\n2026-10-14T01:51:22Z,3,<b>,-4\r\n2026-10-14T01:51:22Z,1,::1,2.5";
        let samples = parse(text).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0]["hops"][0]["ttl"], 1);
        assert_eq!(samples[0]["hops"][1]["addr"], Value::Null);
        assert_eq!(samples[0]["hops"][1]["rtt"], Value::Null);
        assert_eq!(interval(&samples), 1000);
        assert!(parse("time,hop,ip,rtt_ms\n").is_err());
        assert!(parse("Hop,IP,Loss").is_err());
    }
    #[test]
    fn filenames() {
        assert_eq!(
            host("TracePlotter_my_host_samples_14.10.2026_16-09-18.csv"),
            "my_host"
        );
        assert_eq!(
            host("traceplotter-example.com-20261014-015122.csv"),
            "example.com"
        );
        assert_eq!(host("my capture.CSV"), "my capture");
    }
}
