//! The support report: one self-contained HTML file for the displayed period.
use super::*;
use crate::insights::Detail;
use serde_json::Value;

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}
/// `Number.prototype.toFixed`: an exact half rounds up, where Rust rounds to even.
fn fixed(value: f64, digits: usize) -> String {
    let scale = 10f64.powi(digits as i32);
    let scaled = value * scale;
    if (scaled - scaled.trunc()).abs() == 0.5 {
        format!("{:.digits$}", (scaled.trunc() + scaled.signum()) / scale)
    } else {
        format!("{value:.digits$}")
    }
}
fn ms(value: Option<f64>) -> String {
    value.map_or_else(|| "–".into(), |v| fixed(v, 1))
}
/// `Date.prototype.toISOString`, which the report's timeline uses.
fn utc(t: f64) -> String {
    chrono::DateTime::from_timestamp_millis((t * 1000.0).round() as i64)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub(super) struct SupportReport<'a> {
    pub target: &'a str,
    pub from: f64,
    pub to: f64,
    pub probe: &'a str,
    pub details: &'a [Detail],
    pub events: &'a [Value],
    /// A PNG data URL of the graphs.
    pub graph: Option<&'a str>,
    pub redact: &'a dyn Fn(&str) -> String,
    pub redacted: bool,
    pub demo: bool,
}

/// A printable report with no scripts or external resources, as the legacy interface writes.
pub(super) fn support_report(report: &SupportReport) -> String {
    let SupportReport {
        target,
        from,
        to,
        probe,
        details,
        events,
        graph,
        redact,
        redacted,
        demo,
    } = *report;
    let text = |s: &str| escape(&redact(s));
    let rows: String = details
        .iter()
        .map(|d| {
            let s = logic::Stats::from(d.total);
            let cells = [
                if d.ttl == 0 {
                    "Round trip".into()
                } else {
                    format!("Hop {}", d.ttl)
                },
                d.addr.map_or_else(|| "–".into(), |a| a.to_string()),
                s.sent.to_string(),
                s.received.to_string(),
                format!("{}%", fixed(s.loss_pct, 2)),
                logic::reply_availability(s.sent, s.received),
                ms(s.last),
                ms(s.avg),
                ms(s.min),
                ms(s.max),
                ms(s.jitter),
                ms(d.median),
                ms(d.p95),
                ms(d.p99),
                d.longest_lost.to_string(),
                fixed(d.longest_loss_seconds, 1),
            ];
            let cells: String = cells
                .iter()
                .enumerate()
                .map(|(i, cell)| {
                    format!(
                        "<td>{}</td>",
                        if i == 1 { text(cell) } else { escape(cell) }
                    )
                })
                .collect();
            format!("<tr>{cells}</tr>")
        })
        .collect();
    let time = |e: &Value| e["t"].as_f64().unwrap_or(0.0);
    let selected: Vec<Value> = events
        .iter()
        .filter(|e| (from..=to).contains(&time(e)))
        .cloned()
        .collect();
    let timeline: String = logic::group_events(&selected)
        .iter()
        .map(|e| {
            format!(
                "<li><time>{}</time> {}</li>",
                utc(time(e)),
                text(&logic::event_label(e))
            )
        })
        .collect();
    let outages: String = logic::incidents(events)
        .into_iter()
        .filter(|i| i.from <= to && i.to.unwrap_or(to) >= from)
        .map(|i| {
            format!(
                "<li>{} → {} · {} s within this period · {}</li>",
                utc(i.from),
                i.to.map_or_else(|| "no recovery recorded".into(), utc),
                fixed((i.to.unwrap_or(to).min(to) - i.from.max(from)).max(0.0), 0),
                text(&i.text)
            )
        })
        .collect();
    // Only our own PNG data URL may appear in the image attribute.
    let image = graph
        .filter(|g| {
            g.strip_prefix("data:image/png;base64,").is_some_and(|b| {
                !b.is_empty()
                    && b.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c))
            })
        })
        .map_or_else(
            || "<p>No graph available.</p>".into(),
            |g| format!("<img alt=\"Selected-period latency graphs\" src=\"{g}\">"),
        );
    let headers: String = [
        "Hop",
        "Latest address in period",
        "Sent",
        "Received",
        "Loss",
        "Reply availability",
        "Last",
        "Average",
        "Min",
        "Max",
        "Jitter",
        "Median",
        "p95",
        "p99",
        "Longest loss (probes)",
        "Longest loss (s)",
    ]
    .iter()
    .map(|h| format!("<th>{h}</th>"))
    .collect();
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'"><title>TracePlotter support report</title><style>body{{font:14px system-ui,sans-serif;line-height:1.5;margin:32px;color:#18202c}}h1{{font-size:24px}}table{{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}}th,td{{padding:6px;border:1px solid #ccd2da;text-align:right}}th:first-child,td:first-child{{text-align:left}}img{{max-width:100%;background:#0f1319}}li{{margin:8px 0}}time{{font-family:monospace}}section{{overflow:auto}}@media print{{body{{margin:0;font-size:10px}}img{{break-inside:avoid}}th{{background:#eee}}}}</style></head><body>
<h1>TracePlotter support report{demo}</h1>
<p>Target: {target}<br>Period (UTC): {from} → {to}<br>Probe settings: {probe}<br>Address redaction: {redaction}</p>
<h2>Selected-period statistics</h2><p>Latencies are milliseconds, recorded here at 0.1 ms resolution. p95 and p99 use nearest rank; the median averages the two central replies for an even count. Availability is the percentage of recorded probes answered, not service uptime. Pauses and unrecorded time are excluded. Loss durations estimate consecutive missed-probe spans from the configured interval.</p>
<section><table><thead><tr>{headers}</tr></thead><tbody>{rows}</tbody></table></section>
<h2>Graphs</h2>{image}<p>Graph labels use the recording computer's local time; the timeline below uses UTC. Red marks indicate alerts, green recoveries and blue route changes.</p>
<h2>Alert incidents</h2><ul>{outages}</ul>
<h2>Events in period</h2><ul>{timeline}</ul></body></html>"#,
        demo = if demo { " (DEMO — made-up data)" } else { "" },
        target = text(target),
        from = utc(from),
        to = utc(to),
        probe = escape(probe),
        redaction = if redacted {
            "on (consistent placeholders)"
        } else {
            "off"
        },
        outages = if outages.is_empty() {
            "<li>No alert incidents recorded for this period.</li>".into()
        } else {
            outages
        },
        timeline = if timeline.is_empty() {
            "<li>No recorded events.</li>".into()
        } else {
            timeline
        },
    )
}

/// Standard base64, for the graph's data URL.
pub(super) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_legacy_report() {
        let fixtures: Vec<Value> =
            serde_json::from_str(include_str!("../../fixtures/legacy-logic.json")).unwrap();
        let cases: Vec<_> = fixtures
            .iter()
            .filter(|c| c["name"] == "supportReport")
            .collect();
        assert_eq!(cases.len(), 2);
        for case in cases {
            let args = &case["args"];
            let details: Vec<Detail> = args[0]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| {
                    let mut detail = Detail::default();
                    detail.ttl = d["ttl"].as_u64().unwrap() as u8;
                    detail.addr = d["addr"].as_str().map(|a| a.parse().unwrap());
                    detail.total = serde_json::from_value(d["total"].clone()).unwrap();
                    detail.median = d["median"].as_f64();
                    detail.p95 = d["p95"].as_f64();
                    detail.p99 = d["p99"].as_f64();
                    detail.longest_lost = d["longest_lost"].as_u64().unwrap();
                    detail.longest_loss_seconds = d["longest_loss_seconds"].as_f64().unwrap();
                    detail
                })
                .collect();
            let redacted = args[3].as_bool().unwrap();
            let redactor =
                logic::redactor(&serde_json::from_value::<Vec<String>>(args[2].clone()).unwrap());
            let plain = |t: &str| t.to_owned();
            let html = support_report(&SupportReport {
                target: "example.net (192.0.2.1)",
                from: 990.0,
                to: 1100.25,
                probe: "ICMP, interval 1000 ms",
                details: &details,
                events: args[1].as_array().unwrap(),
                graph: Some("data:image/png;base64,iVBORw0KGgo="),
                redact: if redacted { &redactor } else { &plain },
                redacted,
                demo: redacted,
            });
            assert_eq!(html, case["expected"].as_str().unwrap());
        }
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe]), "//4=");
    }
}
