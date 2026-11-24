//! The MCP protocol over JSON-RPC: what a client sends to /mcp, and what it gets back. Tools only, and no sessions.

use serde_json::{Value, json};

use crate::tools::{self, Ctx, INSTRUCTIONS};

const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// The answer to one POST body: a status, and a body unless the status is 202 (a notification, nothing to say back).
pub fn handle(ctx: &Ctx, body: &[u8]) -> (u16, Option<String>) {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return (400, Some(error(Value::Null, -32700, "that isn't JSON").to_string()));
    };
    match v {
        Value::Array(items) => {
            let out: Vec<Value> = items.iter().filter_map(|m| one(ctx, m)).collect();
            if out.is_empty() { (202, None) } else { (200, Some(Value::Array(out).to_string())) }
        }
        m => match one(ctx, &m) {
            Some(r) => (200, Some(r.to_string())),
            None => (202, None),
        },
    }
}

fn one(ctx: &Ctx, m: &Value) -> Option<Value> {
    let method = m["method"].as_str()?; // no method: a client's answer to something, nothing to do
    let id = m.get("id").cloned()?; // no id: a notification
    Some(match method {
        "initialize" => {
            let asked = m["params"]["protocolVersion"].as_str().unwrap_or("");
            let version = VERSIONS.iter().find(|v| **v == asked).unwrap_or(&VERSIONS[0]);
            result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "trunk-recorder", "title": "Trunk Recorder Pro: calls and transcripts", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({ "tools": tools::list() })),
        "tools/call" => {
            let name = m["params"]["name"].as_str().unwrap_or("");
            let args = m["params"].get("arguments").cloned().unwrap_or(json!({}));
            if name.is_empty() {
                return Some(error(id, -32602, "a tool call needs a name"));
            }
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tools::call_content(ctx, name, &args)));
            let (content, is_error) = match run {
                Ok(Ok(c)) => (c, false),
                Ok(Err(e)) => (vec![json!({ "type": "text", "text": e })], true),
                Err(_) => (vec![json!({ "type": "text", "text": "the tool failed" })], true),
            };
            result(id, json!({ "content": content, "isError": is_error }))
        }
        "resources/list" => result(id, json!({ "resources": [] })),
        "prompts/list" => result(id, json!({ "prompts": [] })),
        _ => error(id, -32601, &format!("no method {method}")),
    })
}

fn result(id: Value, r: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": r })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
