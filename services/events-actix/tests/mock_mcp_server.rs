//! A minimal MCP server over stdio, for the bridge tests to spawn.
//!
//! Built as a test binary rather than mocked in-process because the thing under test *is* the
//! subprocess boundary: spawning, stdin writes, stdout framing, EOF, and reaping. An in-process
//! fake would exercise none of it.
//!
//! It speaks only what criterion 4 asks for — `initialize`, `tools/list`, `tools/call` — plus two
//! behaviours the bridge's own guarantees need proving against:
//!
//! * `tools/call` with `{"name":"huge"}` returns a text block past the filter's ceiling, so the
//!   filter is exercised on a real frame rather than a hand-built string.
//! * `tools/call` with `{"name":"die"}` exits immediately, which closes stdout mid-stream and is
//!   the disconnect half of criterion 4.

use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            // A malformed line gets a JSON-RPC parse error, as a real server would send.
            let _ = writeln!(
                stdout,
                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32700,"message":"parse error"}}}}"#
            );
            let _ = stdout.flush();
            continue;
        };

        let id = request
            .get("id")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let method = request
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();

        let response = match method {
            "initialize" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "serverInfo": { "name": "mock-mcp-server", "version": "0.1.0" },
                    "capabilities": { "tools": {} },
                },
            }),
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        { "name": "echo", "description": "returns its argument" },
                        { "name": "huge", "description": "returns an oversized text block" },
                        { "name": "die", "description": "exits without replying" },
                    ],
                },
            }),
            "tools/call" => {
                let tool = request
                    .get("params")
                    .and_then(|params| params.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                match tool {
                    // Exit before writing, closing stdout mid-stream.
                    "die" => return,
                    "huge" => {
                        // Repeated same-role siblings, which is what the filter collapses.
                        let mut text = String::new();
                        for index in 0..400 {
                            text.push_str(&format!("  - listitem \"row {index}\"\n"));
                        }
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "content": [{ "type": "text", "text": text }] },
                        })
                    }
                    _ => {
                        let argument = request
                            .get("params")
                            .and_then(|params| params.get("arguments"))
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{ "type": "text", "text": argument.to_string() }],
                            },
                        })
                    }
                }
            }
            other => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("no such method: {other}") },
            }),
        };

        let _ = writeln!(stdout, "{response}");
        let _ = stdout.flush();
    }
}
