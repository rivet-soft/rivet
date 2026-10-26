//! Model Context Protocol server (`rivet mcp`): lets AI assistants list, edit
//! and send Rivet requests, manage variables and run collections.
//!
//! Transport: newline-delimited JSON-RPC 2.0 over stdin/stdout (MCP stdio).
//! Only protocol messages go to stdout; diagnostics go to stderr. Requests are
//! handled one at a time; every tool reloads `project.json` and mutating tools
//! save it right away, so the window and the server can share the file.
//! Permissions come from `settings.mcp` in the same file (Settings → MCP), and
//! every tool call is appended to `mcp-activity.jsonl` next to it.

mod args;
mod output;
mod tools;

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

pub use tools::{DISABLED_MESSAGE, Server, tool_names};

/// Protocol revisions this server speaks, newest first.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Rivet is an HTTP client. Saved requests live in collections; \
`{{variables}}` resolve from the active environment overlaid with collection variables. \
Use list_collections / list_requests to discover ids, send_request to execute (scripts run, \
variable changes and history are saved like in the app), run_collection for chained flows. \
Mock servers: save_example defines a response for a saved request (or sends it and keeps the real one); \
start_mock runs the collection's mock in Docker (answers with examples, else the OpenAPI responses), \
reload_mock picks up new examples, stop_mock / mock_status manage them. generate_code renders a request in 12 languages; \
export_collection / import_spec move collections (Rivet, Postman, OpenAPI, HAR). \
Secret-looking values are masked unless reveal_secrets is true (and the user allowed it in Settings → MCP). Destructive requests against \
an environment marked production need confirm_production: true — ask the user first.";

/// Runs the server on the process's stdin/stdout until stdin closes.
pub fn serve_stdio(project_path: PathBuf) -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve(Server::new(project_path), stdin.lock(), stdout.lock())
}

/// Serves JSON-RPC lines from `input`, writing responses to `output`.
pub fn serve(mut server: Server, input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(Value::Array(batch)) if !batch.is_empty() => {
                let replies: Vec<Value> = batch.iter().filter_map(|m| handle_message(&mut server, m)).collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(message) => handle_message(&mut server, &message),
            Err(e) => Some(error_response(Value::Null, -32700, &format!("Parse error: {e}"))),
        };
        if let Some(reply) = reply {
            let text = serde_json::to_string(&reply).unwrap_or_default();
            output.write_all(text.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// One JSON-RPC message → its response (`None` for notifications and replies).
fn handle_message(server: &mut Server, message: &Value) -> Option<Value> {
    let Some(obj) = message.as_object() else {
        return Some(error_response(Value::Null, -32600, "Invalid request"));
    };
    let id = obj.get("id").cloned();
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        // A response to something we never send, or garbage.
        return id.map(|id| error_response(id, -32600, "Invalid request: missing method"));
    };
    let id = id?; // Notifications (`notifications/initialized`, `…/cancelled`) need no reply.
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let client = params.pointer("/clientInfo/name").and_then(Value::as_str).map(String::from);
            server.set_client(client);
            Ok(initialize(&params))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools::definitions()})),
        "tools/call" => tools_call(server, &params),
        "logging/setLevel" => Ok(json!({})),
        "resources/list" => Ok(json!({"resources": []})),
        "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
        "prompts/list" => Ok(json!({"prompts": []})),
        other => Err((-32601, format!("Method not found: {other}"))),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => error_response(id, code, &message),
    })
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
    let version = PROTOCOL_VERSIONS.iter().find(|v| **v == requested).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "rivet", "title": "Rivet", "version": env!("RIVET_VERSION")},
        "instructions": INSTRUCTIONS,
    })
}

fn tools_call(server: &mut Server, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "tools/call: missing tool name".to_string()))?;
    if !tool_names().contains(&name) {
        return Err((-32602, format!("Unknown tool: {name}")));
    }
    let empty = json!({});
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => &empty,
        Some(v @ Value::Object(_)) => v,
        Some(_) => return Err((-32602, "tools/call: arguments must be an object".into())),
    };
    let (text, is_error) = match server.call(name, arguments) {
        Ok(reply) => (
            serde_json::to_string_pretty(&reply.value).unwrap_or_default(),
            reply.is_error,
        ),
        Err(message) => (message, true),
    };
    Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
}
