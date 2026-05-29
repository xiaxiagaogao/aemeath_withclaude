use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};

use crate::state::{PendingInputSlot, PetState, SharedState, StateChangeEvent};
use crate::ClaudeTarget;

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
    pub _input_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UserMessageRequest {
    pub value: String,
}

pub fn create_router(
    state: SharedState,
    tx: broadcast::Sender<StateChangeEvent>,
    pending_input: PendingInputSlot,
    claude_target: ClaudeTarget,
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
        .route("/api/user/message", post(handle_user_message))
        .route("/api/user/message/pending", get(handle_user_message_pending))
        .layer(cors)
        .with_state(AppState {
            state,
            tx,
            pending_input,
            claude_target,
        })
}

#[derive(Clone)]
struct AppState {
    state: SharedState,
    tx: broadcast::Sender<StateChangeEvent>,
    pending_input: PendingInputSlot,
    claude_target: ClaudeTarget,
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

/// POST /api/user/message — receive message from pet UI, relay to Claude Code terminal
async fn handle_user_message(
    State(app): State<AppState>,
    Json(body): Json<UserMessageRequest>,
) -> StatusCode {
    let msg = body.value.clone();
    let mut mgr = app.state.lock().await;
    mgr.push_message(msg.clone());
    drop(mgr);

    let target = app.claude_target.lock().unwrap().clone();

    // Relay the message to Claude Code terminal as keystrokes.
    // Each backend (Windows / macOS) is implemented below.
    relay_to_terminal(&msg, target);

    StatusCode::OK
}

// ============================================================================
// relay_to_terminal — platform-specific implementations
// ============================================================================

#[cfg(target_os = "windows")]
fn relay_to_terminal(msg: &str, target: String) {
    use std::os::windows::process::CommandExt;

    // 1. Copy message to clipboard via PowerShell
    let escaped = msg.replace('\'', "''");
    let set_clip = format!("Set-Clipboard -Value '{}'", escaped);
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &set_clip])
        .creation_flags(0x08000000)
        .output();

    // 2. Paste into Claude Code window
    let bound_hwnd: isize = target.parse().unwrap_or(0);
    std::thread::spawn(move || unsafe {
        win::paste_to_window(bound_hwnd);
    });
}

#[cfg(target_os = "macos")]
fn relay_to_terminal(msg: &str, bound_bundle: String) {
    // 1. Copy to clipboard via pbcopy
    use std::io::Write;
    if let Ok(mut child) = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(msg.as_bytes());
        }
        let _ = child.wait();
    }

    // 2. Activate the bound terminal app and send Cmd+V + Return via AppleScript.
    //    Falls back to whichever app is currently frontmost if the bound bundle
    //    is empty or fails to activate (e.g. the terminal was force-quit).
    let bundle = bound_bundle.clone();
    std::thread::spawn(move || {
        macos::paste_to_app(&bundle);
    });
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn relay_to_terminal(_msg: &str, _target: String) {
    // No-op on unsupported platforms.
}

// ============================================================================
// Platform back-ends
// ============================================================================

#[cfg(target_os = "windows")]
mod win {
    static FOUND_HWND: std::sync::Mutex<isize> = std::sync::Mutex::new(0);

    /// Search terms for finding the Claude Code terminal window (case-insensitive).
    const SEARCH_TERMS: &[&str] = &["claude"];

    unsafe extern "system" fn enum_callback(hwnd: isize, _lparam: isize) -> i32 {
        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), 512);
        if len > 0 {
            let title = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
            for term in SEARCH_TERMS {
                if title.contains(term) {
                    *FOUND_HWND.lock().unwrap() = hwnd;
                    return 0; // stop
                }
            }
        }
        1 // continue
    }

    pub(super) unsafe fn paste_to_window(bound_hwnd: isize) {
        // 1. Search for the current topmost "claude" window
        {
            *FOUND_HWND.lock().unwrap() = 0;
        }
        EnumWindows(Some(enum_callback), 0);
        let mut hwnd = *FOUND_HWND.lock().unwrap();

        // 2. Fall back to bound HWND if search found nothing (e.g. terminal minimized)
        if hwnd == 0 && bound_hwnd != 0 && IsWindow(bound_hwnd) != 0 {
            hwnd = bound_hwnd;
        }

        if hwnd == 0 {
            return;
        }

        let prev = GetForegroundWindow();

        ShowWindow(hwnd, 9); // SW_RESTORE
        SetForegroundWindow(hwnd);
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Ctrl+V
        keybd_event(0x11, 0, 0, 0);
        keybd_event(0x56, 0, 0, 0);
        keybd_event(0x56, 0, 2, 0);
        keybd_event(0x11, 0, 2, 0);

        std::thread::sleep(std::time::Duration::from_millis(80));

        // Enter
        keybd_event(0x0D, 0, 0, 0);
        keybd_event(0x0D, 0, 2, 0);

        // Restore previous focus
        std::thread::sleep(std::time::Duration::from_millis(150));
        if prev != 0 {
            SetForegroundWindow(prev);
        }
    }

    extern "system" {
        fn EnumWindows(cb: Option<unsafe extern "system" fn(isize, isize) -> i32>, lp: isize) -> i32;
        fn GetWindowTextW(hwnd: isize, text: *mut u16, max: i32) -> i32;
        fn IsWindowVisible(hwnd: isize) -> i32;
        fn IsWindow(hwnd: isize) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
        fn GetForegroundWindow() -> isize;
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
    }
}

#[cfg(target_os = "macos")]
mod macos {
    /// Bundle identifiers we recognise as Claude Code terminals. Order matters:
    /// the first match in the running-process list wins when the bound bundle
    /// is unknown/empty.
    const KNOWN_TERMINAL_BUNDLES: &[&str] = &[
        "com.googlecode.iterm2",     // iTerm2
        "com.apple.Terminal",         // Terminal.app
        "com.mitchellh.ghostty",      // Ghostty
        "net.kovidgoyal.kitty",       // kitty
        "io.alacritty",               // Alacritty
        "com.github.wez.wezterm",     // WezTerm
        "dev.warp.Warp-Stable",       // Warp
    ];

    /// Use AppleScript via osascript to:
    ///   1. Find the right terminal bundle (live search across running terminals,
    ///      with fallback to the bound bundle from startup).
    ///   2. Activate that app.
    ///   3. Send Cmd+V then Return via System Events.
    ///
    /// Requires Accessibility permission for System Events. macOS will prompt
    /// the first time the app tries to send keystrokes.
    pub(super) fn paste_to_app(bound_bundle: &str) {
        let bundle = resolve_target_bundle(bound_bundle);
        if bundle.is_empty() {
            return;
        }

        let script = format!(
            r#"
tell application id "{b}" to activate
delay 0.1
tell application "System Events"
    keystroke "v" using command down
    delay 0.08
    keystroke return
end tell
"#,
            b = bundle.replace('"', "\\\"")
        );

        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output();
    }

    /// Live-search for a known running terminal; fall back to the bound bundle
    /// captured at startup. This mirrors the Windows EnumWindows + fallback flow.
    fn resolve_target_bundle(bound: &str) -> String {
        // Live search: ask System Events which terminal bundles are running.
        let bundle_list = KNOWN_TERMINAL_BUNDLES
            .iter()
            .map(|b| format!("\"{b}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let probe = format!(
            r#"
set known to {{{}}}
tell application "System Events"
    repeat with b in known
        try
            if (exists (first process whose bundle identifier is b)) then
                return b as string
            end if
        end try
    end repeat
end tell
return ""
"#,
            bundle_list
        );
        if let Ok(out) = std::process::Command::new("osascript")
            .args(["-e", &probe])
            .output()
        {
            if out.status.success() {
                let found = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !found.is_empty() {
                    return found;
                }
            }
        }

        // Fall back to the bundle captured at startup.
        bound.to_string()
    }
}

/// GET /api/user/message/pending — return and clear pending user messages
async fn handle_user_message_pending(
    State(app): State<AppState>,
) -> Json<serde_json::Value> {
    let mut mgr = app.state.lock().await;
    let msgs = mgr.drain_messages();
    Json(json!({
        "messages": msgs,
        "count": msgs.len(),
    }))
}
