//! Local display times and file names; history and MCP remain UTC.
use super::*;
use chrono::{DateTime, Datelike, Local, TimeZone};

pub(super) fn at(seconds: f64) -> DateTime<Local> {
    Local
        .timestamp_millis_opt((seconds * 1000.0).round() as i64)
        .single()
        .unwrap_or_else(Local::now)
}
fn format_at(date: DateTime<Local>, locale: &str) -> (String, String) {
    let locale = locale.to_lowercase();
    if locale.starts_with("en-us") {
        return (
            date.format("%-m/%-d/%Y").to_string(),
            date.format("%-I:%M:%S %p").to_string(),
        );
    }
    if locale.starts_with("cs") {
        return (
            date.format("%-d. %-m. %Y").to_string(),
            date.format("%-H:%M:%S").to_string(),
        );
    }
    if locale.starts_with("pl") {
        return (
            date.format("%d.%m.%Y").to_string(),
            date.format("%H:%M:%S").to_string(),
        );
    }
    let parts = locale.split(['-', '_']).collect::<Vec<_>>();
    let name = if let Some(region) = parts.get(1) {
        format!("{}_{}", parts[0], region.to_uppercase())
    } else {
        locale
    };
    let locale = name
        .parse::<chrono::Locale>()
        .unwrap_or(chrono::Locale::en_US);
    (
        date.format_localized("%x", locale).to_string(),
        date.format_localized("%X", locale).to_string(),
    )
}
pub(super) fn date_time(seconds: f64) -> String {
    let (date, time) = format_at(at(seconds), locale());
    format!("{date} {time}")
}
fn locale() -> &'static str {
    static LOCALE: OnceLock<String> = OnceLock::new();
    LOCALE.get_or_init(|| host_locale().unwrap_or_else(|| "en-US".into()))
}
fn clock(seconds: f64, seconds_shown: bool, locale: &str) -> String {
    let date = at(seconds);
    if seconds_shown {
        format_at(date, locale).1
    } else {
        date.format(if locale.eq_ignore_ascii_case("en-US") {
            "%I:%M %p"
        } else {
            "%H:%M"
        })
        .to_string()
    }
}
pub(super) fn tick(seconds: f64, step: f64, from: f64, to: f64) -> String {
    let date = at(seconds);
    let mut time = clock(seconds, step % 60.0 != 0.0, locale());
    if at(from).date_naive() != at(to).date_naive()
        && (seconds - step < from || date.date_naive() != at(seconds - step).date_naive())
    {
        time.push('\n');
        time.push_str(&format_at(date, locale()).0);
    }
    time
}
pub(super) fn grid(input: egui_plot::GridInput) -> Vec<egui_plot::GridMark> {
    let step = [
        1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0,
        10800.0, 21600.0, 43200.0, 86400.0, 172800.0, 604800.0, 2592000.0,
    ]
    .into_iter()
    .find(|s| *s >= input.base_step_size)
    .unwrap_or(2592000.0);
    let (from, to) = input.bounds;
    let mut marks = Vec::new();
    if step >= 86400.0 {
        let days = (step / 86400.0) as i64;
        let mut date = at(from).date_naive();
        for _ in 0..370 {
            if i64::from(date.num_days_from_ce()).rem_euclid(days) == 0 {
                let midnight = date
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight is a valid clock time");
                if let Some(t) = Local.from_local_datetime(&midnight).earliest() {
                    let value = t.timestamp() as f64;
                    if value > to {
                        break;
                    }
                    if value >= from {
                        marks.push(egui_plot::GridMark {
                            value,
                            step_size: step,
                        });
                    }
                }
            }
            let Some(next) = date.succ_opt() else {
                break;
            };
            date = next;
        }
    } else {
        let first = (from / step).ceil() * step;
        for i in 0..1000 {
            let value = first + f64::from(i) * step;
            if value > to {
                break;
            }
            marks.push(egui_plot::GridMark {
                value,
                step_size: step,
            });
        }
    }
    marks
}

fn safe(text: &str) -> String {
    let mut result = String::new();
    let mut separator = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if !result.is_empty() && !separator.is_empty() {
                result.push_str(if separator == "." { "." } else { "-" });
            }
            separator.clear();
            result.push(ch);
        } else {
            separator.push(ch);
        }
    }
    result
}
pub(super) fn export_name(host: &str, kind: &str, extension: &str) -> String {
    let (date, time) = format_at(Local::now(), locale());
    format!(
        "TracePlotter_{}_{}_{}_{}.{}",
        file_safe(host),
        kind,
        safe(&date),
        safe(&time),
        extension
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn regional_export_names() {
        let date = Local.with_ymd_and_hms(2026, 10, 14, 16, 9, 18).unwrap();
        for (locale, day, time) in [
            ("pl-PL", "14.10.2026", "16-09-18"),
            ("cs-CZ", "14-10-2026", "16-09-18"),
            ("en-US", "10-14-2026", "4-09-18-PM"),
        ] {
            let (d, t) = format_at(date, locale);
            assert_eq!(safe(&d), day);
            assert_eq!(safe(&t), time);
        }
    }
    #[test]
    fn axis_times_follow_region_and_minute_precision() {
        let noon = Local
            .with_ymd_and_hms(2026, 10, 9, 12, 0, 0)
            .unwrap()
            .timestamp() as f64;
        assert_eq!(clock(noon, false, "en-GB"), "12:00");
        assert_eq!(clock(noon + 30.0, true, "en-GB"), "12:00:30");
        assert_eq!(clock(noon, false, "en-US"), "12:00 PM");
        let marks = grid(egui_plot::GridInput {
            bounds: (noon, noon + 600.0),
            base_step_size: 80.0,
        });
        assert!(marks.iter().all(|m| m.value % 60.0 == 0.0));
    }
}
