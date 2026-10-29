//! Calculations shared by the native interface and agent answers.

use super::Tally;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Stats {
    pub sent: u32,
    pub received: u32,
    pub loss_pct: f64,
    pub last: Option<f64>,
    pub avg: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub jitter: Option<f64>,
}

impl From<Tally> for Stats {
    fn from(t: Tally) -> Self {
        Self {
            sent: t.sent,
            received: t.received,
            loss_pct: if t.sent > 0 {
                f64::from(t.sent - t.received) / f64::from(t.sent) * 100.0
            } else {
                0.0
            },
            last: t.last,
            avg: (t.received > 0).then(|| t.sum / f64::from(t.received)),
            min: t.min,
            max: t.max,
            jitter: (t.jitter_count > 0).then(|| t.jitter_sum / f64::from(t.jitter_count)),
        }
    }
}

pub(super) fn stats(rtts: &[Option<f64>]) -> Stats {
    let mut tally = Tally::default();
    let mut previous = None;
    for &rtt in rtts {
        tally.add(rtt, previous);
        if rtt.is_some() {
            previous = rtt;
        }
    }
    tally.into()
}

pub(super) fn alert_probes(seconds: u64, interval: u64) -> usize {
    ((seconds * 1000) as f64 / interval.max(1) as f64)
        .round()
        .max(1.0) as usize
}
pub(super) fn breaches(rtts: &[Option<f64>], probes: usize, latency: f64, loss: f64) -> bool {
    if rtts.len() < probes {
        return false;
    }
    let s = stats(&rtts[rtts.len().saturating_sub(probes)..]);
    (latency > 0.0 && s.avg.is_some_and(|v| v > latency))
        || (loss > 0.0 && s.sent > 0 && s.loss_pct >= loss)
}
pub(super) fn mos(s: Stats) -> Option<f64> {
    let latency = s.avg? + 2.0 * s.jitter.unwrap_or(0.0) + 10.0;
    let base = if latency < 160.0 {
        93.2 - latency / 40.0
    } else {
        93.2 - (latency - 120.0) / 10.0
    };
    let r = (base - 2.5 * s.loss_pct).clamp(0.0, 100.0);
    Some(1.0 + 0.035 * r + 0.000007 * r * (r - 60.0) * (100.0 - r))
}

pub(super) fn jitter(rtts: &[Option<f64>]) -> Vec<Option<f64>> {
    let (mut previous, mut estimate): (Option<f64>, Option<f64>) = (None, None);
    rtts.iter()
        .map(|&rtt| {
            let rtt = rtt?;
            if let Some(previous) = previous {
                let change = (rtt - previous).abs();
                estimate = Some(estimate.map_or(change, |j| j + (change - j) / 16.0));
            }
            previous = Some(rtt);
            estimate
        })
        .collect()
}

pub(super) fn loss(times: &[f64], rtts: &[Option<f64>], width: f64) -> Vec<f64> {
    let mut buckets = BTreeMap::<i64, (u64, u64)>::new();
    for (&t, rtt) in times.iter().zip(rtts) {
        let count = buckets.entry((t / width).floor() as i64).or_default();
        count.0 += 1;
        count.1 += u64::from(rtt.is_none());
    }
    times
        .iter()
        .map(|t| {
            let (sent, lost) = buckets[&((t / width).floor() as i64)];
            lost as f64 / sent as f64 * 100.0
        })
        .collect()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct GraphData {
    pub xs: Vec<f64>,
    pub ys: Vec<Option<f64>>,
    pub js: Vec<Option<f64>>,
    pub ls: Vec<Option<f64>>,
    #[serde(default)]
    pub lo: Vec<Option<f64>>,
    #[serde(default)]
    pub hi: Vec<Option<f64>>,
}

pub(super) fn thin(
    data: &GraphData,
    breaks: &HashSet<u64>,
    width: f64,
    radius: usize,
) -> GraphData {
    if width <= 0.0 {
        return GraphData {
            lo: data.ys.clone(),
            hi: data.ys.clone(),
            ..data.clone()
        };
    }
    let mut out = GraphData::default();
    let mut counts = Vec::new();
    let mut start = 0;
    while start < data.xs.len() {
        let x = data.xs[start];
        if breaks.contains(&x.to_bits()) {
            out.xs.push(x);
            out.ys.push(None);
            out.js.push(None);
            out.ls.push(None);
            out.lo.push(None);
            out.hi.push(None);
            counts.push(0);
            start += 1;
            continue;
        }
        let mut end = start;
        let mut values = Vec::new();
        let mut jitters = Vec::new();
        while end < data.xs.len()
            && (data.xs[end] / width).floor() == (x / width).floor()
            && !breaks.contains(&data.xs[end].to_bits())
        {
            if let Some(v) = data.ys[end] {
                values.push(v);
            }
            if let Some(v) = data.js[end] {
                jitters.push(v);
            }
            end += 1;
        }
        out.xs.push(x);
        out.ys
            .push((!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64));
        out.js.push(
            (!jitters.is_empty()).then(|| jitters.iter().sum::<f64>() / jitters.len() as f64),
        );
        out.ls.push(data.ls[start]);
        out.lo.push(values.iter().copied().reduce(f64::min));
        out.hi.push(values.iter().copied().reduce(f64::max));
        counts.push(values.len());
        start = end;
    }
    blend(&mut out, &counts, breaks, radius);
    out
}

fn blend(out: &mut GraphData, counts: &[usize], breaks: &HashSet<u64>, radius: usize) {
    if radius > 0 {
        let averages = out.ys.clone();
        for (i, y) in out.ys.iter_mut().enumerate() {
            if y.is_none() {
                continue;
            }
            let (mut sum, mut weight) = (0.0, 0usize);
            for direction in [-1, 1] {
                for k in usize::from(direction > 0)..=radius {
                    let j = i as isize + direction * k as isize;
                    if j < 0
                        || j as usize >= averages.len()
                        || breaks.contains(&out.xs[j as usize].to_bits())
                    {
                        break;
                    }
                    let j = j as usize;
                    let w = (radius + 1 - k) * counts[j];
                    if let Some(v) = averages[j].filter(|_| w > 0) {
                        sum += w as f64 * v;
                        weight += w;
                    }
                }
            }
            *y = (weight > 0).then(|| sum / weight as f64);
        }
    }
}

/// Graphs minute tallies with the same gaps and reply-weighted smoothing as raw probes.
pub(super) fn range_graph(
    stretches: &[super::Stretch],
    width: f64,
    loss_seconds: f64,
    radius: usize,
    interval: f64,
) -> GraphData {
    let mut buckets = BTreeMap::<i64, (u32, u32)>::new();
    for s in stretches {
        let c = buckets
            .entry((s.t as f64 / 1000.0 / loss_seconds).floor() as i64)
            .or_default();
        c.0 += s.tally.sent;
        c.1 += s.tally.sent - s.tally.received;
    }
    let mut data = GraphData::default();
    let mut counts = vec![];
    let mut breaks = HashSet::new();
    for (i, s) in stretches.iter().enumerate() {
        let x = s.t as f64 / 1000.0;
        if i > 0 {
            let before = stretches[i - 1].t as f64 / 1000.0;
            if x - before > (1.5 * width).max(3.0 * interval).max(5.0) {
                let t = before + width;
                breaks.insert(t.to_bits());
                data.xs.push(t);
                data.ys.push(None);
                data.js.push(None);
                data.ls.push(None);
                data.lo.push(None);
                data.hi.push(None);
                counts.push(0);
            }
        }
        let stats = Stats::from(s.tally);
        let (sent, lost) = buckets[&((x / loss_seconds).floor() as i64)];
        data.xs.push(x);
        data.ys.push(stats.avg);
        data.js.push(stats.jitter);
        data.ls
            .push(Some(f64::from(lost) / f64::from(sent) * 100.0));
        data.lo.push(stats.min);
        data.hi.push(stats.max);
        counts.push(s.tally.received as usize);
    }
    if width > interval {
        blend(&mut data, &counts, &breaks, radius);
    }
    data
}

pub(super) fn nice_ceil(value: f64) -> f64 {
    if value <= 0.0 {
        return 1.0;
    }
    let power = 10.0f64.powf(value.log10().floor());
    [1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]
        .into_iter()
        .find(|s| s * power >= value * (1.0 - 1e-9))
        .unwrap_or(10.0)
        * power
}

pub(super) fn latency_axis(min: Option<f64>, max: Option<f64>) -> [f64; 2] {
    let zero = nice_ceil(max.unwrap_or(0.0) * 1.2);
    let (Some(min), Some(max)) = (min, max) else {
        return [0.0, zero];
    };
    let margin = ((max - min) * 0.2).max(max * 0.1).max(5.0);
    let fifth = (max - min + 2.0 * margin) / 5.0;
    let power = 10.0f64.powf(fifth.log10().floor());
    let step = [5.0, 2.0, 1.0]
        .into_iter()
        .map(|s| s * power)
        .find(|s| *s <= fifth)
        .unwrap_or(power);
    let bottom = ((min - margin) / step).floor() * step;
    if bottom > zero / 5.0 {
        [bottom, ((max + margin) / step).ceil() * step]
    } else {
        [0.0, zero]
    }
}

fn typed_number(text: &str) -> Option<(f64, String)> {
    let typed = text.trim().to_lowercase();
    let n = typed
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != ',')
        .unwrap_or(typed.len());
    let number = &typed[..n];
    let unit = typed[n..].trim();
    if number.is_empty()
        || !number.chars().last()?.is_ascii_digit()
        || !unit.chars().all(|c| c.is_ascii_lowercase())
    {
        return None;
    }
    let value = number.replace(',', ".").parse::<f64>().ok()?;
    value.is_finite().then(|| (value, unit.to_owned()))
}

pub(super) fn parse_span(text: &str) -> Option<f64> {
    let text = text.trim().to_lowercase();
    if !text.is_empty() && "all".starts_with(&text) {
        return Some(0.0);
    }
    if !text.chars().next()?.is_ascii_digit() {
        return None;
    }
    let (v, unit) = typed_number(&text)?;
    let scale = match unit.chars().next().unwrap_or('m') {
        's' => 1.0,
        'm' => 60.0,
        'h' => 3600.0,
        'd' => 86400.0,
        _ => return None,
    };
    let span = (v * scale).round().max(10.0);
    span.is_finite().then_some(span)
}

pub(super) fn parse_interval(text: &str) -> Option<u64> {
    let (v, unit) = typed_number(text)?;
    let scale = if unit.is_empty() {
        if v > 60.0 { 1.0 } else { 1000.0 }
    } else if unit.starts_with("ms") || unit.starts_with("mil") {
        1.0
    } else if unit.starts_with('m') {
        60000.0
    } else if unit.starts_with('s') {
        1000.0
    } else {
        return None;
    };
    Some((v * scale).round().clamp(100.0, 60000.0) as u64)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Settings {
    pub protocol: String,
    pub interval_ms: u64,
    pub timeout_ms: u64,
    pub packet_size: u16,
    pub alert_latency_ms: u64,
    pub alert_loss_pct: u64,
    pub alert_seconds: u64,
    pub mcp_port: u16,
    pub warn_ms: u64,
    pub bad_ms: u64,
    pub history_days: u64,
    pub notifications: bool,
    pub loss_by_target: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            protocol: "icmp".into(),
            interval_ms: 1000,
            timeout_ms: 1000,
            packet_size: 84,
            alert_latency_ms: 0,
            alert_loss_pct: 0,
            alert_seconds: 30,
            mcp_port: 3917,
            warn_ms: 100,
            bad_ms: 200,
            history_days: 7,
            notifications: true,
            loss_by_target: false,
        }
    }
}

pub(super) fn clean_settings(raw: &Value, system_icmp: bool) -> Settings {
    let mut out = serde_json::to_value(Settings::default()).expect("settings serialize");
    if system_icmp {
        out["protocol"] = json!("winicmp");
    }
    for (key, lo, hi) in [
        ("timeoutMs", 100, 10000),
        ("packetSize", 48, 1024),
        ("alertLatencyMs", 0, 60000),
        ("alertLossPct", 0, 100),
        ("alertSeconds", 1, 3600),
        ("mcpPort", 1024, 65535),
        ("warnMs", 1, 10000),
        ("badMs", 2, 60000),
        ("historyDays", 1, 365),
    ] {
        let v = &raw[key];
        if v.is_null() || v.as_str() == Some("") {
            continue;
        }
        let number = v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .or_else(|| v.as_bool().map(|b| f64::from(u8::from(b))));
        if let Some(n) = number.filter(|n| n.is_finite()) {
            out[key] = json!(n.round().clamp(f64::from(lo), f64::from(hi)) as u64);
        }
    }
    let interval = raw["intervalMs"]
        .as_u64()
        .or_else(|| raw["intervalMs"].as_str().and_then(|s| s.parse().ok()));
    if let Some(ms) = interval.filter(|ms| [500, 1000, 2500, 5000, 10000].contains(ms)) {
        out["intervalMs"] = json!(ms);
    }
    if let Some(p) = raw["protocol"]
        .as_str()
        .filter(|p| ["icmp", "udp", "tcp"].contains(p) || (system_icmp && *p == "winicmp"))
    {
        out["protocol"] = json!(p);
    }
    for key in ["notifications", "lossByTarget"] {
        if let Some(v) = raw[key].as_bool() {
            out[key] = json!(v);
        }
    }
    let mut result: Settings = serde_json::from_value(out).expect("validated settings");
    result.bad_ms = result.bad_ms.max(result.warn_ms + 1);
    result
}

pub(super) fn group_events(events: &[Value]) -> Vec<Value> {
    let mut groups = Vec::new();
    type OpenRoute = (f64, f64, Option<Value>, BTreeMap<u64, (String, String)>);
    let mut open: Option<OpenRoute> = None;
    let close = |open: &mut Option<OpenRoute>, groups: &mut Vec<Value>| {
        if let Some((t, _, length, hops)) = open.take() {
            let length = length.filter(|l| l["from"] != l["to"]);
            let hops: Vec<Value> = hops
                .into_iter()
                .filter(|(_, (a, b))| a != b)
                .map(|(ttl, (from, to))| json!({"ttl":ttl,"from":from,"to":to}))
                .collect();
            if length.is_some() || !hops.is_empty() {
                groups.push(json!({"t":t,"kind":"route","length":length,"hops":hops}));
            }
        }
    };
    for event in events {
        let kind = event["kind"].as_str().unwrap_or_default();
        if kind != "hop" && kind != "length" {
            close(&mut open, &mut groups);
            groups.push(event.clone());
            continue;
        }
        let t = event["t"].as_f64().unwrap_or(0.0);
        if open.as_ref().is_some_and(|o| t - o.1 > 5.0) {
            close(&mut open, &mut groups);
        }
        let o = open.get_or_insert_with(|| (t, t, None, BTreeMap::new()));
        o.1 = t;
        if kind == "length" {
            o.2 = Some(
                json!({"from":o.2.as_ref().map_or(&event["from"], |l| &l["from"]),"to":event["to"]}),
            );
        } else {
            let h =
                o.3.entry(event["ttl"].as_u64().unwrap_or(0))
                    .or_insert_with(|| {
                        (
                            event["from"].as_str().unwrap_or_default().into(),
                            String::new(),
                        )
                    });
            h.1 = event["to"].as_str().unwrap_or_default().into();
        }
    }
    close(&mut open, &mut groups);
    groups
}

pub(super) fn whois_record(text: &str) -> String {
    let mut out = String::new();
    let mut blanks = 0;
    for line in text.lines().filter(|l| !l.starts_with(['%', '#'])) {
        blanks = if line.is_empty() { blanks + 1 } else { 0 };
        if blanks <= 1 {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_owned()
}

pub(super) fn whois_facts(text: &str) -> Vec<Value> {
    let facts = [
        (
            "Organisation",
            vec!["org-name", "orgname", "owner", "organization", "descr"],
        ),
        ("Network", vec!["netname"]),
        ("Range", vec!["inetnum", "inet6num", "netrange"]),
        ("Route", vec!["route", "route6", "cidr"]),
        ("AS number", vec!["origin", "originas", "aut-num"]),
        ("Country", vec!["country"]),
        (
            "Abuse contact",
            vec!["abuse", "orgabuseemail", "abuse-mailbox"],
        ),
    ];
    let lines: Vec<_> = text.lines().collect();
    let field = |line: &str| {
        line.split_once(':').and_then(|(k, v)| {
            (!k.is_empty()
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !v.trim().is_empty())
            .then(|| (k.to_lowercase(), v.trim().to_owned()))
        })
    };
    let last = lines
        .iter()
        .rposition(|l| {
            field(l).is_some_and(|(k, _)| ["inetnum", "inet6num", "netrange"].contains(&k.as_str()))
        })
        .unwrap_or(0);
    let mut fields = BTreeMap::new();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("% Abuse contact for ")
            && let Some(v) = line.split('\'').nth(1)
        {
            fields.entry("abuse".into()).or_insert(v.into());
        }
        if i >= last
            && let Some((k, v)) = field(line)
        {
            fields.entry(k).or_insert(v);
        }
    }
    facts
        .into_iter()
        .filter_map(|(label, keys)| {
            keys.into_iter()
                .find_map(|k| fields.get(k))
                .map(|v| json!({"label":label,"value":v}))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    fn compare(got: &Value, expected: &Value, context: &str) {
        match (got, expected) {
            (Value::Number(a), Value::Number(b)) => {
                let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
                assert!(
                    (a - b).abs() <= 1e-9 * b.abs().max(1.0),
                    "{context}: {a} != {b}"
                );
            }
            (Value::Array(a), Value::Array(b)) => {
                assert_eq!(a.len(), b.len(), "{context}: array length");
                for (i, (a, b)) in a.iter().zip(b).enumerate() {
                    compare(a, b, &format!("{context}[{i}]"));
                }
            }
            (Value::Object(a), Value::Object(b)) => {
                assert_eq!(a.len(), b.len(), "{context}: fields");
                for (key, b) in b {
                    compare(
                        a.get(key).unwrap_or(&Value::Null),
                        b,
                        &format!("{context}.{key}"),
                    );
                }
            }
            _ => assert_eq!(got, expected, "{context}"),
        }
    }

    #[test]
    fn original_typescript_functions_match_the_rust_port() {
        let fixtures: Vec<Value> =
            serde_json::from_str(include_str!("../fixtures/legacy-logic.json")).unwrap();
        for (i, case) in fixtures.iter().enumerate() {
            let got = evaluate_fixture(case["name"].as_str().unwrap(), &case["args"]);
            compare(
                &got,
                &case["expected"],
                &format!("case {i}: {}", case["name"]),
            );
        }
    }

    fn evaluate_fixture(name: &str, args: &Value) -> Value {
        use super::*;
        let rtts = || serde_json::from_value::<Vec<Option<f64>>>(args[0].clone()).unwrap();
        match name {
            "alertProbes" => json!(alert_probes(
                args[0].as_u64().unwrap(),
                args[1].as_u64().unwrap()
            )),
            "breaches" => json!(breaches(
                &serde_json::from_value::<Vec<Option<f64>>>(args[0].clone()).unwrap(),
                args[1].as_u64().unwrap() as usize,
                args[2].as_f64().unwrap(),
                args[3].as_f64().unwrap()
            )),
            "niceCeil" => json!(nice_ceil(args[0].as_f64().unwrap())),
            "stats" => json!(stats(&rtts())),
            "mos" => json!(mos(stats(&rtts()))),
            "jitter" => json!(jitter(&rtts())),
            "loss" => json!(loss(
                &serde_json::from_value::<Vec<f64>>(args[0].clone()).unwrap(),
                &serde_json::from_value::<Vec<Option<f64>>>(args[1].clone()).unwrap(),
                args[2].as_f64().unwrap()
            )),
            "thin" => json!(thin(
                &serde_json::from_value(args[0].clone()).unwrap(),
                &args[1]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_f64().unwrap().to_bits())
                    .collect(),
                args[2].as_f64().unwrap(),
                args[3].as_u64().unwrap() as usize
            )),
            "axis" => json!(latency_axis(args[0].as_f64(), args[1].as_f64())),
            "span" => json!(parse_span(args[0].as_str().unwrap())),
            "interval" => json!(parse_interval(args[0].as_str().unwrap())),
            "settings" => json!(clean_settings(&args[0], args[1].as_bool().unwrap())),
            "events" => json!(group_events(args[0].as_array().unwrap())),
            "whoisFacts" => json!(whois_facts(args[0].as_str().unwrap())),
            "rangeGraph" => json!(range_graph(
                &serde_json::from_value::<Vec<super::super::Stretch>>(args[0].clone()).unwrap(),
                args[1].as_f64().unwrap(),
                args[2].as_f64().unwrap(),
                args[3].as_u64().unwrap() as usize,
                args[4].as_f64().unwrap()
            )),
            "whoisRecord" => json!(whois_record(args[0].as_str().unwrap())),
            _ => panic!("unknown fixture {name}"),
        }
    }
}
