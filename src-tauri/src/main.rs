#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod http;
mod mcp;
mod state;
mod tray;

use state::PendingInputSlot;
use state::StateManager;
use state::StateChangeEvent;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tauri::Emitter;

/// Cross-platform handle identifying the terminal Claude Code is running in.
///
/// - On Windows: stores the foreground HWND (as a decimal string) captured at
///   startup, before Tauri opens its own window. Used to paste-and-Enter into
///   Claude Code's terminal.
/// - On macOS: stores the bundle identifier of the frontmost application at
///   startup (e.g. "com.googlecode.iterm2", "com.apple.Terminal"). Used to
///   `activate` that app and send Cmd+V + Return via AppleScript.
/// - On other platforms: empty; relay is a no-op.
pub type ClaudeTarget = Arc<std::sync::Mutex<String>>;

#[tokio::main]
async fn main() {
    let claude_target: ClaudeTarget = Arc::new(std::sync::Mutex::new(String::new()));

    // Capture the parent terminal handle BEFORE Tauri creates its own window.
    // At startup (triggered by the SessionStart hook), the foreground app/window
    // is Claude Code's terminal.
    #[cfg(target_os = "windows")]
    unsafe {
        let hwnd = GetForegroundWindow();
        *claude_target.lock().unwrap() = hwnd.to_string();
        println!("Claude Code HWND bound: {}", hwnd);
    }

    #[cfg(target_os = "macos")]
    {
        match capture_frontmost_bundle_id() {
            Some(bundle) => {
                println!("Claude Code parent app bound: {}", bundle);
                *claude_target.lock().unwrap() = bundle;
            }
            None => {
                println!("Could not capture frontmost app; will fall back at send time.");
            }
        }
    }

    let state_manager = Arc::new(Mutex::new(StateManager::new()));
    let (tx, _rx) = broadcast::channel::<StateChangeEvent>(32);
    let pending_input: PendingInputSlot = Arc::new(Mutex::new(None));

    let sm_http = state_manager.clone();
    let tx_http = tx.clone();
    let pi_http = pending_input.clone();
    let target_http = claude_target.clone();

    // Spawn HTTP server on :9527
    tokio::spawn(async move {
        let app = http::create_router(sm_http, tx_http, pi_http, target_http);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:9527").await.unwrap();
        println!("HTTP server listening on http://127.0.0.1:9527");
        axum::serve(listener, app).await.unwrap();
    });

    let sm_mcp = state_manager.clone();
    let tx_mcp = tx.clone();
    let pi_mcp = pending_input.clone();

    // Spawn MCP server on :9528
    tokio::spawn(async move {
        let app = mcp::create_mcp_router(sm_mcp, tx_mcp, pi_mcp);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:9528").await.unwrap();
        println!("MCP server listening on http://127.0.0.1:9528");
        axum::serve(listener, app).await.unwrap();
    });

    // Build Tauri app
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![start_drag, hide_window, exit_app])
        .setup(move |app| {
            // Listen to broadcast channel, forward state changes to frontend
            let handle = app.handle().clone();
            let mut rx = tx.subscribe();
            let handle2 = handle.clone();
            tokio::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    let _ = handle2.emit("state-change", event);
                }
            });

            // Send initial waving state
            let _ = handle.emit(
                "state-change",
                StateChangeEvent {
                    animation: "waving".to_string(),
                    bubble: "爱弥斯已上线~".to_string(),
                    core_signal: "idle".to_string(),
                    tool_label: None,
                    overlay: None,
                    input_type: None,
                    options: None,
                },
            );

            // Enable system tray
            if let Err(e) = tray::setup(app) {
                eprintln!("Failed to setup tray: {}", e);
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Aemeath Pet");
}

#[cfg(target_os = "windows")]
extern "system" {
    fn GetForegroundWindow() -> isize;
}

/// macOS: capture the bundle identifier of the frontmost application via
/// AppleScript. Runs synchronously at startup before Tauri activates its own
/// window, so the result is the launching terminal app.
#[cfg(target_os = "macos")]
fn capture_frontmost_bundle_id() -> Option<String> {
    let output = std::process::Command::new("osascript")
        .args([
            "-e",
            "tell application \"System Events\" to get bundle identifier of first process whose frontmost is true",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[tauri::command]
fn start_drag(window: tauri::Window) {
    let _ = window.start_dragging();
}

#[tauri::command]
fn hide_window(window: tauri::Window) {
    let _ = window.hide();
}

#[tauri::command]
fn exit_app(app: tauri::AppHandle) {
    app.exit(0);
}
