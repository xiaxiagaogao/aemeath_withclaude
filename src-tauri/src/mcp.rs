use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::{broadcast, oneshot, Mutex};

use crate::state::{PendingInput, PendingInputSlot, PetState, StateChangeEvent};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

pub fn create_mcp_router(
    state: McpState,
    tx: broadcast::Sender<StateChangeEvent>,
    pending_input: PendingInputSlot,
) -> Router {
    Router::new()
        .route("/mcp", post(handle_mcp_request))
        .route("/sse", get(handle_sse))
        .with_state(McpAppState {
            state,
            tx,
            pending_input,
        })
}

#[derive(Clone)]
struct McpAppState {
    state: McpState,
    tx: broadcast::Sender<StateChangeEvent>,
    pending_input: PendingInputSlot,
}

async fn handle_mcp_request(
    State(app): State<McpAppState>,
    Json(req): Json<JsonRpcRequest>,
) -> Json<JsonRpcResponse> {
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
                },
                {
                    "name": "aemeath_get_user_input",
                    "description": "Block and wait for user input through the pet UI. Supports text, confirm (yes/no), and select (pick from options) types.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "prompt": { "type": "string", "description": "Question or prompt to show above the pet" },
                            "placeholder": { "type": "string", "description": "Placeholder text inside the input field (text type only)" },
                            "type": { "type": "string", "enum": ["text", "confirm", "select"], "description": "Input type: text=freeform, confirm=yes/no, select=pick from options. Default: text" },
                            "options": { "type": "array", "items": { "type": "string" }, "description": "List of options for select type" },
                            "timeout_secs": { "type": "number", "description": "Seconds to wait before auto-cancelling (default 60, max 300)" }
                        },
                        "required": ["prompt"]
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
                        core_signal: "waiting".into(),
                        tool_label: None,
                        overlay: None,
                        input_type: None,
                        options: None,
                    });
                    json!({ "content": [{ "type": "text", "text": format!("Message shown: {}", msg) }] })
                }
                "aemeath_ask" => {
                    let question = args["question"].as_str().unwrap_or("").to_string();
                    let options: Option<Vec<String>> = args["options"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                        .filter(|v: &Vec<String>| !v.is_empty());
                    let input_type = if options.is_some() { "select" } else { "text" }.to_string();

                    // C3: check-and-write merged into single lock (same as aemeath_get_user_input)
                    let mut pi = app.pending_input.lock().await;
                    if pi.is_some() {
                        return Json(JsonRpcResponse {
                            id: req.id,
                            result: None,
                            error: Some(json!({
                                "code": -32603,
                                "message": "Another input request is already pending"
                            })),
                        });
                    }
                    let (tx_oneshot, rx) = oneshot::channel::<String>();
                    *pi = Some(PendingInput {
                        tx: tx_oneshot,
                        input_type: input_type.clone(),
                        options: options.clone(),
                    });
                    drop(pi);

                    // Notify frontend to show input UI
                    let _ = app.tx.send(StateChangeEvent {
                        animation: "waiting".into(),
                        bubble: question,
                        core_signal: "waiting".into(),
                        tool_label: None,
                        overlay: Some("input".into()),
                        input_type: Some(input_type),
                        options: options.clone(),
                    });

                    // Block until user responds or timeout (60s)
                    let timeout = tokio::time::Duration::from_secs(60);
                    let result = tokio::time::timeout(timeout, rx).await;

                    // Clear pending input slot
                    {
                        let mut pi = app.pending_input.lock().await;
                        *pi = None;
                    }

                    match result {
                        Ok(Ok(value)) => {
                            let _ = app.tx.send(StateChangeEvent {
                                animation: "running".into(),
                                bubble: format!("收到: {}", value),
                                core_signal: "running".into(),
                                tool_label: None,
                                overlay: None,
                                input_type: None,
                                options: None,
                            });
                            json!({ "content": [{ "type": "text", "text": value }] })
                        }
                        Ok(Err(_)) => {
                            let _ = app.tx.send(StateChangeEvent {
                                animation: "idle".into(),
                                bubble: "".into(),
                                core_signal: "idle".into(),
                                tool_label: None,
                                overlay: None,
                                input_type: None,
                                options: None,
                            });
                            json!({ "content": [{ "type": "text", "text": "User did not respond" }] })
                        }
                        Err(_) => {
                            let _ = app.tx.send(StateChangeEvent {
                                animation: "idle".into(),
                                bubble: "".into(),
                                core_signal: "idle".into(),
                                tool_label: None,
                                overlay: None,
                                input_type: None,
                                options: None,
                            });
                            json!({ "content": [{ "type": "text", "text": "User did not respond (timeout)" }] })
                        }
                    }
                }
                "aemeath_play" => {
                    let state_name = args["state"].as_str().unwrap_or("idle");
                    // S3: derive core_signal from the PetState mapping
                    let pet_state = PetState::from_hook(state_name, None);
                    let core_signal = pet_state.core_signal().to_string();
                    let _ = app.tx.send(StateChangeEvent {
                        animation: state_name.to_string(),
                        bubble: "".into(),
                        core_signal,
                        tool_label: None,
                        overlay: None,
                        input_type: None,
                        options: None,
                    });
                    json!({ "content": [{ "type": "text", "text": format!("Playing: {}", state_name) }] })
                }
                "aemeath_get_user_input" => {
                    let prompt = args["prompt"].as_str().unwrap_or("").to_string();
                    let input_type = args["type"].as_str().unwrap_or("text").to_string();
                    let options: Option<Vec<String>> = args["options"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
                    let timeout_secs = args["timeout_secs"]
                        .as_f64()
                        .unwrap_or(60.0)
                        .max(1.0)
                        .min(300.0);

                    // Validate input_type
                    let input_type = match input_type.as_str() {
                        "text" | "confirm" | "select" => input_type,
                        _ => {
                            return Json(JsonRpcResponse {
                                id: req.id,
                                result: None,
                                error: Some(json!({
                                    "code": -32602,
                                    "message": "Invalid type: must be text, confirm, or select"
                                })),
                            });
                        }
                    };

                    // C2: check-and-write merged into single lock operation
                    let mut pi = app.pending_input.lock().await;
                    if pi.is_some() {
                        return Json(JsonRpcResponse {
                            id: req.id,
                            result: None,
                            error: Some(json!({
                                "code": -32603,
                                "message": "Another input request is already pending"
                            })),
                        });
                    }

                    // Create oneshot channel, store sender + metadata
                    let (tx_oneshot, rx) = oneshot::channel::<String>();
                    *pi = Some(PendingInput {
                        tx: tx_oneshot,
                        input_type: input_type.clone(),
                        options: options.clone(),
                    });
                    drop(pi);

                    // Notify frontend to show input UI
                    let _ = app.tx.send(StateChangeEvent {
                        animation: "waiting".into(),
                        bubble: prompt,
                        core_signal: "waiting".into(),
                        tool_label: None,
                        overlay: Some("input".into()),
                        input_type: Some(input_type),
                        options: options.clone(),
                    });

                    // Block until user responds or timeout
                    let timeout = tokio::time::Duration::from_secs_f64(timeout_secs);
                    let result = tokio::time::timeout(timeout, rx).await;

                    // Clear pending input slot
                    {
                        let mut pi = app.pending_input.lock().await;
                        *pi = None;
                    }

                    match result {
                        Ok(Ok(value)) => {
                            // Clear input overlay
                            let _ = app.tx.send(StateChangeEvent {
                                animation: "running".into(),
                                bubble: format!("收到: {}", value),
                                core_signal: "running".into(),
                                tool_label: None,
                                overlay: None,
                                input_type: None,
                                options: None,
                            });
                            json!({ "content": [{ "type": "text", "text": value }] })
                        }
                        Ok(Err(_)) => {
                            // sender dropped (shouldn't normally happen)
                            let _ = app.tx.send(StateChangeEvent {
                                animation: "idle".into(),
                                bubble: "".into(),
                                core_signal: "idle".into(),
                                tool_label: None,
                                overlay: None,
                                input_type: None,
                                options: None,
                            });
                            json!({ "content": [{ "type": "text", "text": "User did not respond" }] })
                        }
                        Err(_) => {
                            // timeout
                            let _ = app.tx.send(StateChangeEvent {
                                animation: "idle".into(),
                                bubble: "".into(),
                                core_signal: "idle".into(),
                                tool_label: None,
                                overlay: None,
                                input_type: None,
                                options: None,
                            });
                            json!({ "content": [{ "type": "text", "text": "User did not respond (timeout)" }] })
                        }
                    }
                }
                _ => {
                    return Json(JsonRpcResponse {
                        id: req.id,
                        result: None,
                        error: Some(json!({"code": -32601, "message": format!("Unknown tool: {}", tool_name)})),
                    });
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
                },
                {
                    "uri": "aemeath://user-messages",
                    "name": "User Messages",
                    "description": "Pending user messages sent from the pet UI — read to receive and clear them"
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
                "aemeath://user-messages" => {
                    let mut mgr = app.state.lock().await;
                    let msgs = mgr.drain_messages();
                    let text = if msgs.is_empty() {
                        "(no pending messages)".to_string()
                    } else {
                        msgs.join("\n---\n")
                    };
                    json!({
                        "contents": [{
                            "uri": "aemeath://user-messages",
                            "text": text
                        }]
                    })
                }
                _ => {
                    return Json(JsonRpcResponse {
                        id: req.id,
                        result: None,
                        error: Some(json!({"code": -32602, "message": format!("Unknown resource: {}", uri)})),
                    });
                }
            }
        }

        _ => {
            return Json(JsonRpcResponse {
                id: req.id,
                result: None,
                error: Some(json!({"code": -32601, "message": format!("Unknown method: {}", req.method)})),
            });
        }
    };

    Json(JsonRpcResponse {
        id: req.id,
        result: Some(response),
        error: None,
    })
}

async fn handle_sse() -> StatusCode {
    StatusCode::OK
}
