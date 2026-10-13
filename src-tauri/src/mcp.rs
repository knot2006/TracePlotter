//! The MCP server: lets AI agents (Claude Code, Claude Desktop and other MCP clients) read
//! the open traces and start new ones. It is off until the user turns it on in the Tools
//! menu, and it listens on the loopback interface only.
//!
//! It speaks the Streamable HTTP transport of the Model Context Protocol in its simplest
//! form: every message is a POST to `/mcp` answered with one JSON body, and there is no
//! event stream. The tabs and their numbers live in the webview, so a tool call is passed
//! to it as an `mcp-request` event and its answer comes back through `mcp_respond`. That
//! keeps one implementation of the statistics, the one the window shows.

use std::collections::HashMap;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State};

/// How long a tool call may take in the webview. Starting a trace resolves a name and
/// reads its history, which can take a few seconds.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Largest request body read; MCP requests are small.
const MAX_BODY: u64 = 1024 * 1024;
/// Protocol versions this server can speak, newest first.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Told to the agent when it connects.
const INSTRUCTIONS: &str = "TracePlotter is a visual traceroute and ping monitor. Each open \
tab traces one target: every round it probes each hop on the route and the target itself. \
Use list_targets first to see the targets and their ids. Latency is in milliseconds, \
loss in percent, MOS is estimated call quality from 1 to about 4.4, and times are UTC.\n\
Reading the numbers: loss or high latency at a middle hop that does not carry on to the \
hops after it, nor to the round trip, is that router rate-limiting or deprioritising its \
replies to probes, not a network problem. Loss that starts at one hop and continues to \
every hop after it, and to the round trip, points to that hop or the link before it.\n\
Starting traces: many routers throttle their replies when probed more than once a second, \
which then looks like packet loss. start_trace therefore never probes faster than one \
round per second. Prefer resuming an existing tab over opening a new one, and do not start \
traces the user did not ask for.";

/// The tools, as `tools/list` describes them.
fn tools() -> Value {
    let target = json!({
        "type": "string",
        "description": "The target's id from list_targets, or its host name or address. May be left out when only one target is open."
    });
    let minutes = json!({
        "type": "number",
        "minimum": 0,
        "description": "How many of the newest minutes to cover; 0 is the whole history. Default 10."
    });
    json!([
        {
            "name": "list_targets",
            "title": "List targets",
            "description": "Every open target with its state (running, paused, alerting), probe settings, hop count and round-trip statistics over the newest minutes: loss, last, average, minimum and maximum latency, jitter and MOS.",
            "inputSchema": { "type": "object", "properties": { "minutes": minutes } },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "get_hops",
            "title": "Hops of a target",
            "description": "The route to one target, hop by hop: address, reverse DNS name, probes sent, loss, last, average, minimum and maximum latency, jitter and MOS, over the newest minutes. The last row is the round trip to the target itself.",
            "inputSchema": {
                "type": "object",
                "properties": { "target": target, "minutes": minutes }
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "get_route_changes",
            "title": "Route changes and alerts",
            "description": "What happened to one target, newest first: route changes (hops answering from a new address, the path getting longer or shorter), alerts raised when the target broke the user's latency or loss threshold, and recoveries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": target,
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500, "description": "Most entries to return. Default 50." }
                }
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "get_latency_history",
            "title": "Latency history",
            "description": "How latency and loss to one target (or one hop) changed over time: the period is cut into equal slices, each with probes sent, loss, average, minimum and maximum latency and jitter. Use it to find when a problem started or how often it comes back.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": target,
                    "minutes": {
                        "type": "number",
                        "minimum": 0,
                        "description": "How many of the newest minutes to cover; 0 is the whole history. Default 60."
                    },
                    "hop": { "type": "integer", "minimum": 1, "description": "Hop number to look at instead of the round trip to the target." },
                    "slices": { "type": "integer", "minimum": 1, "maximum": 300, "description": "Number of slices. Default 30." }
                }
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "start_trace",
            "title": "Start a trace",
            "description": "Starts tracing a host in a new tab of the app, or resumes the paused tab that already traces it. Probing begins at once and keeps going until the user or stop_trace pauses it. Many routers throttle their replies when probed more than once a second, which shows up as false packet loss, so the interval is never below 1000 ms. Only start traces the user asked for.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "host": { "type": "string", "description": "Host name or IP address to trace." },
                    "protocol": {
                        "type": "string",
                        "enum": ["winicmp", "icmp", "udp", "tcp"],
                        "description": "Probe type: winicmp is ICMP through the Windows ICMP API (Windows only, the default there), icmp is ICMP through a raw socket (the default elsewhere), udp and tcp send to a port."
                    },
                    "port": { "type": "integer", "minimum": 1, "maximum": 65535, "description": "Destination port for udp (default 33434) and tcp (default 443)." },
                    "interval_ms": { "type": "integer", "enum": [1000, 2500, 5000, 10000], "description": "Time between rounds in ms, one of the intervals the app offers. Default 1000, which is also the fastest an agent may use." }
                },
                "required": ["host"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true }
        },
        {
            "name": "stop_trace",
            "title": "Pause a trace",
            "description": "Pauses a target's probing. Its tab and history stay; start_trace with the same host resumes it.",
            "inputSchema": {
                "type": "object",
                "properties": { "target": target },
                "required": ["target"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
        }
    ])
}

/// A JSON-RPC answer to request `id`.
fn result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Answers one JSON-RPC message. `None` for a notification, which gets no answer.
/// `call` runs a tool with its arguments.
fn handle(message: &Value, call: impl FnOnce(&str, Value) -> Result<Value, String>) -> Option<Value> {
    let id = message.get("id")?;
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Some(error(id, -32600, "not a JSON-RPC request"));
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = PROTOCOL_VERSIONS
                .into_iter()
                .find(|v| *v == asked)
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "traceplotter",
                        "title": "TracePlotter",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions": INSTRUCTIONS
                }),
            )
        }
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({ "tools": tools() })),
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return Some(error(id, -32602, "tools/call needs a tool name"));
            };
            if !tools().as_array().unwrap().iter().any(|tool| tool["name"] == name) {
                return Some(error(id, -32602, &format!("unknown tool {name}")));
            }
            let arguments = match params.get("arguments") {
                Some(Value::Object(arguments)) => Value::Object(arguments.clone()),
                _ => json!({}),
            };
            // A tool that fails tells the agent why, as a result it can read and act on.
            let (text, is_error) = match call(name, arguments) {
                Ok(answer) => (serde_json::to_string_pretty(&answer).unwrap_or_default(), false),
                Err(message) => (message, true),
            };
            result(id, json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
        }
        other => error(id, -32601, &format!("method {other} is not supported")),
    })
}

/// Whether a request may come in. The server answers only clients on this machine that
/// name it by its loopback address: a web page cannot reach it, as browsers send an
/// `Origin` header, and a DNS-rebinding page would have a foreign `Host`.
fn allowed(host: Option<&str>, origin: Option<&str>, port: u16) -> bool {
    origin.is_none()
        && host.is_some_and(|host| host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}"))
}

/// The running server and the tool calls that wait for the webview.
#[derive(Default)]
pub struct Mcp {
    server: Mutex<Option<Arc<tiny_http::Server>>>,
    next_call: AtomicU64,
    calls: Mutex<HashMap<u64, mpsc::Sender<Result<Value, String>>>>,
}

#[derive(Clone, Serialize)]
struct CallEvent {
    id: u64,
    tool: String,
    arguments: Value,
}

/// Runs a tool in the webview and waits for its answer.
fn call_webview(app: &AppHandle, tool: &str, arguments: Value) -> Result<Value, String> {
    let mcp = app.state::<Mcp>();
    let id = mcp.next_call.fetch_add(1, Ordering::Relaxed);
    let (send, receive) = mpsc::channel();
    mcp.calls.lock().unwrap().insert(id, send);
    let event = CallEvent { id, tool: tool.into(), arguments };
    let answer = match app.emit_to("main", "mcp-request", event) {
        Ok(()) => receive
            .recv_timeout(CALL_TIMEOUT)
            .unwrap_or_else(|_| Err("TracePlotter did not answer in time".into())),
        Err(e) => Err(e.to_string()),
    };
    mcp.calls.lock().unwrap().remove(&id);
    answer
}

fn respond(request: tiny_http::Request, status: u16, body: Option<Value>) {
    let response = match body {
        Some(body) => tiny_http::Response::from_string(body.to_string()).with_header(
            tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
        ),
        None => tiny_http::Response::from_string(""),
    };
    let _ = request.respond(response.with_status_code(status));
}

fn serve(app: AppHandle, mut request: tiny_http::Request, port: u16) {
    let header = |name: &'static str| {
        request
            .headers()
            .iter()
            .find(|header| header.field.equiv(name))
            .map(|header| header.value.as_str().to_string())
    };
    if !allowed(header("Host").as_deref(), header("Origin").as_deref(), port) {
        return respond(request, 403, None);
    }
    let path = request.url().split('?').next().unwrap_or_default();
    if path != "/mcp" {
        return respond(request, 404, None);
    }
    // No event stream (GET) and no sessions to end (DELETE): only POST.
    if *request.method() != tiny_http::Method::Post {
        return respond(request, 405, None);
    }
    let mut body = String::new();
    if request.as_reader().take(MAX_BODY).read_to_string(&mut body).is_err() {
        return respond(request, 400, None);
    }
    let message: Value = match serde_json::from_str(&body) {
        Ok(message) => message,
        Err(_) => return respond(request, 400, Some(error(&Value::Null, -32700, "not JSON"))),
    };
    match handle(&message, |tool, arguments| call_webview(&app, tool, arguments)) {
        Some(answer) => respond(request, 200, Some(answer)),
        // A notification or a response from the client: accepted, nothing to say.
        None => respond(request, 202, None),
    }
}

/// Starts the MCP server on `port` of the loopback interface, or stops it when `port` is
/// `None`. Returns the address clients connect to.
#[tauri::command]
pub fn mcp_serve(app: AppHandle, port: Option<u16>, mcp: State<'_, Mcp>) -> Result<Option<String>, String> {
    let mut running = mcp.server.lock().unwrap();
    if let Some(server) = running.take() {
        server.unblock();
    }
    let Some(port) = port else {
        return Ok(None);
    };
    let server = tiny_http::Server::http(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
        .map(Arc::new)
        .map_err(|e| format!("cannot listen on port {port}: {e}"))?;
    *running = Some(server.clone());
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let app = app.clone();
            // Each in its own thread: a tool call waits for the webview.
            std::thread::spawn(move || serve(app, request, port));
        }
    });
    Ok(Some(format!("http://127.0.0.1:{port}/mcp")))
}

/// Whether the app was started with `--mcp`, which turns the MCP server on for this run
/// whatever the menu says. With `--demo` it gives agents made-up traces to work against.
#[tauri::command]
pub fn mcp_flag() -> bool {
    std::env::args().any(|arg| arg == "--mcp")
}

/// The webview's answer to a tool call: `result` on success, else `error`.
#[tauri::command]
pub fn mcp_respond(id: u64, result: Option<Value>, error: Option<String>, mcp: State<'_, Mcp>) {
    if let Some(waiting) = mcp.calls.lock().unwrap().remove(&id) {
        let _ = waiting.send(match error {
            Some(error) => Err(error),
            None => Ok(result.unwrap_or(Value::Null)),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params })
    }

    fn no_call(_: &str, _: Value) -> Result<Value, String> {
        panic!("no tool should run")
    }

    #[test]
    fn initialize_agrees_on_a_version_and_offers_tools() {
        let answer = handle(&request("initialize", json!({ "protocolVersion": "2025-06-18" })), no_call).unwrap();
        assert_eq!(answer["id"], 7);
        assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
        assert!(answer["result"]["capabilities"]["tools"].is_object());
        let unknown = handle(&request("initialize", json!({ "protocolVersion": "1999-01-01" })), no_call).unwrap();
        assert_eq!(unknown["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn notifications_get_no_answer_and_unknown_methods_an_error() {
        let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle(&notification, no_call).is_none());
        assert_eq!(handle(&request("resources/list", json!({})), no_call).unwrap()["error"]["code"], -32601);
    }

    #[test]
    fn every_tool_has_a_schema_and_start_is_limited_to_one_round_a_second() {
        let answer = handle(&request("tools/list", json!({})), no_call).unwrap();
        let tools = answer["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().all(|tool| tool["inputSchema"]["type"] == "object"));
        let start = tools.iter().find(|tool| tool["name"] == "start_trace").unwrap();
        assert_eq!(start["inputSchema"]["properties"]["interval_ms"]["enum"][0], 1000);
    }

    #[test]
    fn tool_calls_pass_their_arguments_and_report_failures_as_results() {
        let call = request("tools/call", json!({ "name": "get_hops", "arguments": { "target": "a" } }));
        let answer = handle(&call, |tool, arguments| {
            assert_eq!(tool, "get_hops");
            Ok(json!({ "echo": arguments["target"] }))
        })
        .unwrap();
        assert_eq!(answer["result"]["isError"], false);
        assert!(answer["result"]["content"][0]["text"].as_str().unwrap().contains("\"echo\": \"a\""));

        let failed = handle(&call, |_, _| Err("no target a".into())).unwrap();
        assert_eq!(failed["result"]["isError"], true);
        assert_eq!(failed["result"]["content"][0]["text"], "no target a");

        let unknown = request("tools/call", json!({ "name": "format_disk" }));
        assert_eq!(handle(&unknown, no_call).unwrap()["error"]["code"], -32602);
    }

    #[test]
    fn only_loopback_clients_without_an_origin_get_in() {
        assert!(allowed(Some("127.0.0.1:3917"), None, 3917));
        assert!(allowed(Some("localhost:3917"), None, 3917));
        assert!(!allowed(Some("127.0.0.1:3917"), Some("https://example.com"), 3917));
        assert!(!allowed(Some("evil.example:3917"), None, 3917));
        assert!(!allowed(None, None, 3917));
    }
}
