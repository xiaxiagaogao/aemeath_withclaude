use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

use crate::state::StateChangeEvent;

pub type McpState = Arc<Mutex<crate::state::StateManager>>;

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

impl JsonRpcResponse {
    fn success(id: Option<Value>, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }
    fn error(id: Option<Value>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(json!({ "code": code, "message": message.into() })),
        }
    }
}

pub fn create_mcp_router(state: McpState, tx: broadcast::Sender<StateChangeEvent>) -> Router {
    Router::new()
        .route("/mcp", post(handle_mcp_request))
        .route("/sse", get(handle_sse))
        .with_state(McpAppState { state, tx })
}

#[derive(Clone)]
struct McpAppState {
    state: McpState,
    tx: broadcast::Sender<StateChangeEvent>,
}

async fn handle_mcp_request(
    State(app): State<McpAppState>,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    if req.id.is_none() {
        return StatusCode::ACCEPTED.into_response();
    }

    let response = match req.method.as_str() {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {
                "name": "aemeath",
                "version": "0.1.0"
            },
            "capabilities": {
                "tools": {},
                "resources": {}
            }
        }),

        "tools/list" => json!({
            "tools": [
                {
                    "name": "aemeath_show",
                    "description": "Show a custom bubble message on the Aemeath pet",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "msg": { "type": "string", "description": "Message to display" }
                        },
                        "required": ["msg"]
                    }
                },
                {
                    "name": "aemeath_ask",
                    "description": "Ask the user a question through the pet UI",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string" },
                            "options": {
                                "type": "array",
                                "items": { "type": "string" }
                            }
                        },
                        "required": ["question"]
                    }
                },
                {
                    "name": "aemeath_play",
                    "description": "Force play a specific animation",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "state": { "type": "string", "enum": ["idle", "thinking", "running", "review", "failed", "waving", "jumping"] },
                            "duration_ms": { "type": "number" }
                        },
                        "required": ["state"]
                    }
                }
            ]
        }),

        "tools/call" => {
            let params = req.params.unwrap_or_default();
            let tool_name = params["name"].as_str().unwrap_or("");
            let args = &params["arguments"];

            match tool_name {
                "aemeath_show" => {
                    let msg = args["msg"].as_str().unwrap_or("");
                    let _ = app.tx.send(StateChangeEvent {
                        animation: "waiting".into(),
                        bubble: msg.to_string(),
                    });
                    json!({ "content": [{ "type": "text", "text": format!("Message shown: {}", msg) }] })
                }
                "aemeath_ask" => {
                    let question = args["question"].as_str().unwrap_or("");
                    let _options: Vec<String> = args["options"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    let _ = app.tx.send(StateChangeEvent {
                        animation: "waving".into(),
                        bubble: question.to_string(),
                    });
                    json!({ "content": [{ "type": "text", "text": "User dismissed" }] })
                }
                "aemeath_play" => {
                    let state_name = args["state"].as_str().unwrap_or("idle");
                    let _ = app.tx.send(StateChangeEvent {
                        animation: state_name.to_string(),
                        bubble: "".into(),
                    });
                    json!({ "content": [{ "type": "text", "text": format!("Playing: {}", state_name) }] })
                }
                _ => {
                    return Json(JsonRpcResponse::error(
                        req.id,
                        -32601,
                        format!("Unknown tool: {}", tool_name),
                    ))
                    .into_response();
                }
            }
        }

        "resources/list" => json!({
            "resources": [
                {
                    "uri": "aemeath://status",
                    "name": "Pet Status",
                    "description": "Current pet state and animation info"
                },
                {
                    "uri": "aemeath://history",
                    "name": "State History",
                    "description": "Recent state change records"
                }
            ]
        }),

        "resources/read" => {
            let params = req.params.unwrap_or_default();
            let uri = params["uri"].as_str().unwrap_or("");
            match uri {
                "aemeath://status" => {
                    let mgr = app.state.lock().await;
                    let current = mgr.current_state();
                    json!({
                        "contents": [{
                            "uri": "aemeath://status",
                            "text": format!("State: {:?}", current)
                        }]
                    })
                }
                "aemeath://history" => {
                    let mgr = app.state.lock().await;
                    let history = mgr.history();
                    json!({
                        "contents": [{
                            "uri": "aemeath://history",
                            "text": serde_json::to_string(history).unwrap_or_default()
                        }]
                    })
                }
                _ => {
                    return Json(JsonRpcResponse::error(
                        req.id,
                        -32602,
                        format!("Unknown resource: {}", uri),
                    ))
                    .into_response();
                }
            }
        }

        _ => {
            return Json(JsonRpcResponse::error(
                req.id,
                -32601,
                format!("Unknown method: {}", req.method),
            ))
            .into_response();
        }
    };

    Json(JsonRpcResponse::success(req.id, response)).into_response()
}

async fn handle_sse() -> StatusCode {
    StatusCode::OK
}
