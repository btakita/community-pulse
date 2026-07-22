//! Demo-scoped MCP transport for the in-process [`ToolBridge`](crate::ToolBridge).
//!
//! The server binds localhost without authentication and intentionally supports
//! only tools over plain JSON streamable HTTP. It must run in the GUI process so
//! external agents and both Slint windows mutate one shared reactive state.

use crate::ChatRole;
use crate::reactive::UiSnapshot;
use crate::tools::ToolBridge;
use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

pub const PROTOCOL_VERSION: &str = "2025-03-26";

pub fn handle(bridge: &ToolBridge, request: &Value) -> Option<Value> {
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    if method == "notifications/initialized" {
        return None;
    }

    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "community-pulse",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
        "ping" => json!({}),
        "tools/list" => {
            let tools = ToolBridge::tool_definitions()
                .into_iter()
                .filter_map(|definition| {
                    let function = definition.get("function")?;
                    Some(json!({
                        "name": function.get("name")?,
                        "description": function.get("description")?,
                        "inputSchema": function.get("parameters")?
                    }))
                })
                .collect::<Vec<_>>();
            json!({ "tools": tools })
        }
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match bridge.call(name, &arguments.to_string()) {
                Ok(value) => tool_result(value.to_string(), false),
                Err(error) => tool_result(format!("{error:#}"), true),
            }
        }
        _ => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {method}") }
            }));
        }
    };

    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn tool_result(text: String, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

type OnChange = Arc<dyn Fn(UiSnapshot) + Send + Sync>;

#[derive(Clone)]
struct McpState {
    bridge: ToolBridge,
    on_change: OnChange,
}

pub async fn serve(
    bridge: ToolBridge,
    port: u16,
    on_change: impl Fn(UiSnapshot) + Send + Sync + 'static,
) -> Result<()> {
    let state = McpState {
        bridge,
        on_change: Arc::new(on_change),
    };
    let app = Router::new()
        .route("/mcp", post(mcp_endpoint))
        .with_state(state);
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("bind MCP endpoint on {address}"))?;
    eprintln!("mcp: listening on http://{address}/mcp");
    axum::serve(listener, app)
        .await
        .context("serve MCP endpoint")
}

async fn mcp_endpoint(State(state): State<McpState>, Json(request): Json<Value>) -> Response {
    let is_tool_call = request.get("method").and_then(Value::as_str) == Some("tools/call");
    let tool_name = request
        .pointer("/params/name")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    match handle(&state.bridge, &request) {
        Some(response) => {
            if is_tool_call {
                let failed =
                    response.pointer("/result/isError").and_then(Value::as_bool) == Some(true);
                let detail = if failed {
                    response
                        .pointer("/result/content/0/text")
                        .and_then(Value::as_str)
                        .map(compact_text)
                        .unwrap_or_else(|| "external call failed".to_owned())
                } else {
                    "external call completed".to_owned()
                };
                state.bridge.state().append_chat(
                    ChatRole::Tool,
                    detail,
                    Some(format!("{tool_name} · via mcp")),
                );
                (state.on_change)(state.bridge.snapshot());
            }
            (StatusCode::OK, Json(response)).into_response()
        }
        None => StatusCode::ACCEPTED.into_response(),
    }
}

fn compact_text(text: &str) -> String {
    const MAX_CHARS: usize = 180;
    let mut compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() > MAX_CHARS {
        compact = compact.chars().take(MAX_CHARS - 1).collect::<String>();
        compact.push('…');
    }
    compact
}
