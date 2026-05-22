use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tower_http::cors::{Any, CorsLayer};

use crate::state::{PendingInputSlot, PetState, SharedState, StateChangeEvent};

#[derive(Debug, Deserialize)]
pub struct StateRequest {
    pub s: String,
    #[serde(default)]
    pub tool: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CurrentResponse {
    pub animation: String,
    pub bubble: String,
    pub core_signal: String,
    pub tool_label: Option<String>,
    pub overlay: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UserInputRequest {
    pub value: String,
    #[serde(rename = "type", default)]
    pub input_type: Option<String>,
}

pub fn create_router(
    state: SharedState,
    tx: broadcast::Sender<StateChangeEvent>,
    pending_input: PendingInputSlot,
) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/api/state", post(handle_state))
        .route("/api/heartbeat", get(handle_heartbeat))
        .route("/api/current", get(handle_current))
        .route("/api/hook/thinking", post(handle_hook_thinking))
        .route("/api/hook/working", post(handle_hook_working))
        .route("/api/hook/done", post(handle_hook_done))
        .route("/api/hook/idle", post(handle_hook_idle))
        .route("/api/hook/permission", post(handle_hook_permission))
        .route("/api/user/input", post(handle_user_input))
        .route("/api/user/pending", get(handle_user_pending))
        .layer(cors)
        .with_state(AppState {
            state,
            tx,
            pending_input,
        })
}

#[derive(Clone)]
struct AppState {
    state: SharedState,
    tx: broadcast::Sender<StateChangeEvent>,
    pending_input: PendingInputSlot,
}

/// Helper: build a StateChangeEvent from state + tool, with derived core_signal/overlay
fn build_event(state: &PetState, tool: Option<&str>) -> StateChangeEvent {
    StateChangeEvent {
        animation: state.animation_name().to_string(),
        bubble: state.bubble_text(tool).to_string(),
        core_signal: state.core_signal().to_string(),
        tool_label: tool.map(|s| s.to_string()),
        overlay: state.overlay().map(|s| s.to_string()),
        input_type: None,
        options: None,
    }
}

async fn handle_state(
    State(app): State<AppState>,
    Json(body): Json<StateRequest>,
) -> StatusCode {
    let pet_state = PetState::from_hook(&body.s, body.tool.as_deref());
    let tool = body.tool.clone();

    {
        let mut mgr = app.state.lock().await;
        mgr.set_state(pet_state.clone(), tool.clone());
    }

    let event = build_event(&pet_state, tool.as_deref());
    let _ = app.tx.send(event);
    StatusCode::OK
}

async fn handle_heartbeat() -> StatusCode {
    StatusCode::OK
}

async fn handle_current(
    State(app): State<AppState>,
) -> Json<CurrentResponse> {
    let mgr = app.state.lock().await;
    let current = mgr.current_state();
    let tool = mgr.current_tool();
    Json(CurrentResponse {
        animation: current.animation_name().to_string(),
        bubble: current.bubble_text(tool).to_string(),
        core_signal: current.core_signal().to_string(),
        tool_label: tool.map(|s| s.to_string()),
        overlay: current.overlay().map(|s| s.to_string()),
    })
}

async fn handle_hook_thinking(
    State(app): State<AppState>,
) -> StatusCode {
    set_pet_state(&app, PetState::Chatting, None).await;
    StatusCode::OK
}

async fn handle_hook_working(
    State(app): State<AppState>,
    body: String,
) -> StatusCode {
    let tool = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("tool_name").and_then(|t| t.as_str().map(String::from)));

    // Map specific tools to specific animations
    let state = match tool.as_deref() {
        Some("WebFetch") => PetState::Fetching,
        Some("WebSearch") => PetState::Searching,
        Some("Write") | Some("Edit") => PetState::Building,
        Some("Agent") | Some("TaskCreate") | Some("TaskUpdate") => PetState::Analyzing,
        _ => PetState::Running,
    };

    set_pet_state(&app, state, tool).await;
    StatusCode::OK
}

async fn handle_hook_done(
    State(app): State<AppState>,
) -> StatusCode {
    set_pet_state(&app, PetState::Celebrating, None).await;
    StatusCode::OK
}

async fn handle_hook_idle(
    State(app): State<AppState>,
) -> StatusCode {
    set_pet_state(&app, PetState::Idle, None).await;
    StatusCode::OK
}

async fn handle_hook_permission(
    State(app): State<AppState>,
) -> StatusCode {
    {
        let mut mgr = app.state.lock().await;
        mgr.set_state(PetState::Permission, None);
    }
    let event = build_event(&PetState::Permission, None);
    let _ = app.tx.send(event);
    StatusCode::OK
}

async fn set_pet_state(app: &AppState, state: PetState, tool: Option<String>) {
    // W1: single lock acquisition to avoid TOCTOU
    let mut mgr = app.state.lock().await;
    let is_active = matches!(mgr.current_state(),
        PetState::Running | PetState::Chatting | PetState::Fetching
        | PetState::Searching | PetState::Analyzing | PetState::Building
    );
    if !matches!(state,
        PetState::Running | PetState::Chatting | PetState::Fetching
        | PetState::Searching | PetState::Analyzing | PetState::Building
    ) && is_active
        && mgr.should_keep_running(800)
    {
        return;
    }
    mgr.set_state(state.clone(), tool.clone());
    drop(mgr);
    let event = build_event(&state, tool.as_deref());
    let _ = app.tx.send(event);
}

/// POST /api/user/input — receive user response from frontend, forward to pending oneshot
async fn handle_user_input(
    State(app): State<AppState>,
    Json(body): Json<UserInputRequest>,
) -> StatusCode {
    let mut pi = app.pending_input.lock().await;
    let pending = pi.take();
    drop(pi); // C1: release pending_input lock before acquiring app.state

    if let Some(pending) = pending {
        let _ = pending.tx.send(body.value);
        // Clear input overlay — send a running event to reset
        let anim = app.state.lock().await.current_state().animation_name().to_string();
        let _ = app.tx.send(StateChangeEvent {
            animation: anim,
            bubble: "收到!".to_string(),
            core_signal: "running".to_string(),
            tool_label: None,
            overlay: None,
            input_type: None,
            options: None,
        });
    } else {
        // C4: No pending input — broadcast message as bubble instead of silent discard
        let anim = app.state.lock().await.current_state().animation_name().to_string();
        let _ = app.tx.send(StateChangeEvent {
            animation: anim,
            bubble: body.value,
            core_signal: "waiting".to_string(),
            tool_label: None,
            overlay: None,
            input_type: None,
            options: None,
        });
    }
    StatusCode::OK
}

/// GET /api/user/pending — tell frontend whether backend is waiting for user input + input type
async fn handle_user_pending(
    State(app): State<AppState>,
) -> Json<serde_json::Value> {
    let pi = app.pending_input.lock().await;
    if let Some(ref pending) = *pi {
        Json(json!({
            "waiting": true,
            "input_type": pending.input_type,
            "options": pending.options,
        }))
    } else {
        Json(json!({ "waiting": false }))
    }
}
