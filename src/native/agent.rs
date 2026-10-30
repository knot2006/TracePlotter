//! MCP read answers shared by the native controller and compatibility fixtures.
use super::*;
use crate::logic::{self, Stats, stats};
use model::{Hop, Trace};
use serde_json::{Value, json};

pub(super) fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
fn iso(seconds: f64) -> String {
    iso_time((seconds.max(0.0) * 1000.0).round() as u64)
}
fn count(value: &Value, fallback: usize, min: usize, max: usize) -> usize {
    value
        .as_f64()
        .filter(|n| n.is_finite())
        .map_or(fallback, |n| {
            n.round().clamp(min as f64, max as f64) as usize
        })
}
fn seconds(args: &Value, fallback: f64) -> Result<f64, String> {
    let span = args["minutes"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .unwrap_or(fallback)
        * 60.0;
    if span.is_finite() {
        Ok(span)
    } else {
        Err("minutes is too large".into())
    }
}

fn text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}
fn newest(hop: &Hop, span: f64) -> std::ops::Range<usize> {
    let end = hop.times.last().copied().unwrap_or(0.0);
    hop.bounds(
        if span == 0.0 {
            f64::NEG_INFINITY
        } else {
            end - span
        },
        end,
    )
}
pub(super) fn numbers(s: Stats) -> Value {
    json!({"sent":s.sent,"received":s.received,"loss_pct":round(s.loss_pct),"last_ms":s.last.map(round),"avg_ms":s.avg.map(round),"min_ms":s.min.map(round),"max_ms":s.max.map(round),"jitter_ms":s.jitter.map(round),"mos":logic::mos(s).map(round)})
}
pub(super) fn find(traces: &[Trace], target: &Value) -> Result<usize, String> {
    let known = || {
        let names = traces
            .iter()
            .map(|t| format!("{} ({})", t.config.session_id, t.config.host))
            .collect::<Vec<_>>()
            .join(", ");
        if names.is_empty() {
            "none".into()
        } else {
            names
        }
    };
    if target.is_null() || target.as_str() == Some("") {
        return if traces.len() == 1 {
            Ok(0)
        } else {
            Err(format!("Say which target: {}", known()))
        };
    }
    let wanted = text(target).trim().to_lowercase();
    if let Some(i) = traces.iter().position(|t| t.config.session_id == wanted) {
        return Ok(i);
    }
    let matches = traces
        .iter()
        .enumerate()
        .filter(|(_, t)| t.config.host == wanted || t.addr.is_some_and(|a| a.to_string() == wanted))
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [i] => Ok(*i),
        [] => Err(format!(
            "No open target {wanted}. Open targets: {}",
            known()
        )),
        _ => Err(format!(
            "More than one tab traces {wanted}; use an id: {}",
            known()
        )),
    }
}
fn older(trace: &Trace, span: f64) -> bool {
    let (from, _) = trace.period(span);
    trace.disk_start.is_some_and(|disk| {
        trace.target.times.first().is_some_and(|held| {
            from.max(disk as f64 / 1000.0) < *held - 3.0 * trace.config.interval_ms as f64 / 1000.0
        })
    })
}
fn totals(trace: &Trace, span: f64) -> Result<Option<Range>, String> {
    if !older(trace, span) {
        return Ok(None);
    }
    let (from, to) = trace.period(span);
    history_range(
        trace.config.session_id.clone(),
        (from.max(0.0) * 1000.0) as u64,
        (to * 1000.0) as u64,
        ((to - from) / 60.0)
            .ceil()
            .clamp(1.0, (u64::MAX / 60000) as f64) as u64
            * 60000,
        vec![],
    )
    .map(Some)
}
pub(super) fn describe(trace: &Trace, span: f64, disk: Option<&Range>) -> Value {
    let target = newest(&trace.target, span);
    let total = disk
        .and_then(|d| d.hops.iter().find(|h| h.ttl == 0))
        .map_or_else(
            || stats(&trace.target.rtts[target.clone()]),
            |h| h.total.into(),
        );
    let c = &trace.config;
    let mut probe = json!({"protocol":c.protocol,"interval_ms":c.interval_ms});
    if ["tcp", "udp"].contains(&c.protocol.as_str()) {
        probe["port"] = json!(c.port);
    }
    let from = if disk.is_some() {
        Some(trace.period(span).0)
    } else {
        trace.target.times.get(target.start).copied()
    };
    json!({"id":c.session_id,"host":c.host,"address":trace.addr,"state":if trace.paused {"paused"}else if trace.alerting{"alerting"}else{"running"},"error":trace.error,"probe":probe,"hops":if trace.path_end==0 {None}else{Some(trace.path_end)},"alert_thresholds":{"latency_ms":(c.alert_latency_ms!=0).then_some(c.alert_latency_ms),"loss_pct":(c.alert_loss_pct!=0).then_some(c.alert_loss_pct),"window_s":c.alert_seconds},"from":from.map(iso),"to":trace.target.times.last().copied().map(iso),"round_trip":numbers(total)})
}
fn slice(from: f64, to: f64, s: Stats) -> Value {
    json!({"from":iso(from),"to":iso(to),"sent":s.sent,"loss_pct":round(s.loss_pct),"avg_ms":s.avg.map(round),"min_ms":s.min.map(round),"max_ms":s.max.map(round),"jitter_ms":s.jitter.map(round)})
}
pub(super) fn read(traces: &[Trace], tool: &str, args: &Value) -> Result<Value, String> {
    let span = seconds(
        args,
        if tool == "get_latency_history" {
            60.0
        } else {
            10.0
        },
    )?;
    if tool == "list_targets" {
        return Ok(
            json!({"targets":traces.iter().map(|t|totals(t,span).map(|disk|describe(t,span,disk.as_ref()))).collect::<Result<Vec<_>,_>>()?}),
        );
    }
    let trace = &traces[find(traces, &args["target"])?];
    match tool {
        "get_hops" => {
            let disk = totals(trace, span)?;
            let mut answer = describe(trace, span, disk.as_ref());
            answer["route"] = json!(
                trace
                    .hops
                    .iter()
                    .take(if trace.path_end == 0 {
                        trace.hops.len()
                    } else {
                        usize::from(trace.path_end)
                    })
                    .map(|h| {
                        let s = if let Some(disk) = &disk {
                            disk.hops
                                .iter()
                                .find(|r| r.ttl == h.ttl)
                                .map_or_else(Stats::default, |r| r.total.into())
                        } else {
                            stats(&h.rtts[newest(h, span)])
                        };
                        let mut row = numbers(s);
                        row["hop"] = json!(h.ttl);
                        row["address"] = json!(h.addr);
                        row["name"] = json!(h.name);
                        row
                    })
                    .collect::<Vec<_>>()
            );
            Ok(answer)
        }
        "get_route_changes" => {
            let groups = logic::group_events(&trace.events);
            let events=groups.iter().rev().take(count(&args["limit"],50,1,500)).map(|g|if g["kind"]=="route" {
                json!({"time":iso(g["t"].as_f64().unwrap_or(0.0)),"kind":"route","hop_count":g["length"],"hops":g["hops"].as_array().into_iter().flatten().map(|h|json!({"hop":h["ttl"],"from":h["from"],"to":h["to"]})).collect::<Vec<_>>()})
            }else{json!({"time":iso(g["t"].as_f64().unwrap_or(0.0)),"kind":g["kind"],"text":g["text"]})}).collect::<Vec<_>>();
            Ok(
                json!({"id":trace.config.session_id,"host":trace.config.host,"total":groups.len(),"events":events}),
            )
        }
        "get_latency_history" => {
            let ttl = args.get("hop").map(|h| count(h, 0, 0, 255));
            let hop = if let Some(ttl) = ttl {
                trace.hops.get(ttl.wrapping_sub(1)).ok_or_else(|| {
                    format!(
                        "{} has no hop {}; it has {}",
                        trace.config.host,
                        text(&args["hop"]),
                        trace.hops.len()
                    )
                })?
            } else {
                &trace.target
            };
            let parts = count(&args["slices"], 30, 1, 300);
            let mut slices = vec![];
            if older(trace, span) {
                let (from, to) = trace.period(span);
                let width =
                    app::stretch_width(to - from, parts, trace.config.interval_ms as f64 / 1000.0);
                let result = history_range(
                    trace.config.session_id.clone(),
                    (from.max(0.0) * 1000.0) as u64,
                    (to * 1000.0) as u64,
                    (width * 1000.0) as u64,
                    vec![ttl.map_or(0, |_| hop.ttl)],
                )?;
                if let Some(h) = result
                    .hops
                    .iter()
                    .find(|h| h.ttl == ttl.map_or(0, |_| hop.ttl))
                {
                    for s in &h.stretches {
                        slices.push(slice(
                            s.t as f64 / 1000.0,
                            s.t as f64 / 1000.0 + width,
                            s.tally.into(),
                        ));
                    }
                }
            } else {
                let range = newest(hop, span);
                let times = &hop.times[range.clone()];
                let rtts = &hop.rtts[range];
                if let (Some(&start), Some(&end)) = (times.first(), times.last()) {
                    let width = ((end - start) / parts as f64).max(1e-9);
                    let mut first = 0;
                    for p in 0..parts {
                        let last = if p + 1 == parts {
                            times.len()
                        } else {
                            times.partition_point(|t| *t < start + width * (p + 1) as f64)
                        };
                        if last > first {
                            slices.push(slice(
                                times[first],
                                times[last - 1],
                                stats(&rtts[first..last]),
                            ));
                            first = last;
                        }
                    }
                }
            }
            Ok(
                json!({"id":trace.config.session_id,"host":trace.config.host,"hop":ttl.map_or(json!("round trip"),|_|json!(hop.ttl)),"slices":slices}),
            )
        }
        _ => Err(format!("unknown tool {tool}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn trace(value: &Value) -> Trace {
        let mut result = Trace::new(serde_json::from_value(value["config"].clone()).unwrap());
        result.addr = value["addr"].as_str().and_then(|s| s.parse().ok());
        result.hops = serde_json::from_value(value["hops"].clone()).unwrap();
        result.target = serde_json::from_value(value["target"].clone()).unwrap();
        result.events = value["events"].as_array().unwrap().clone();
        result.path_end = value["pathEnd"].as_u64().unwrap() as u8;
        result.loaded = true;
        result.paused = value["paused"].as_bool().unwrap_or(false);
        result.error = value["error"].as_str().map(str::to_owned);
        result
    }
    fn equivalent(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equivalent(a, b))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(k, v)| b.get(k).is_some_and(|b| equivalent(v, b)))
            }
            _ => a == b,
        }
    }
    #[test]
    fn original_mcp_contract() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../../fixtures/legacy-agent.json")).unwrap();
        for (index, case) in cases.iter().enumerate() {
            let traces = case["traces"]
                .as_array()
                .unwrap()
                .iter()
                .map(trace)
                .collect::<Vec<_>>();
            let result = read(&traces, case["tool"].as_str().unwrap(), &case["args"]);
            if let Some(error) = case["error"].as_str() {
                assert_eq!(result.unwrap_err(), error, "case {index}");
            } else {
                assert!(
                    equivalent(&result.unwrap(), &case["expected"]),
                    "case {index}: {case}"
                );
            }
        }
    }
    #[test]
    fn oversized_minutes_do_not_panic_a_tool_worker() {
        assert_eq!(
            read(&[], "list_targets", &json!({"minutes":1e308})).unwrap_err(),
            "minutes is too large"
        );
    }
}
