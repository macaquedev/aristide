//! The desktop shell: a Tauri window around the web console in `ui/`.
//!
//! All console logic lives in the frontend; this binary only opens the
//! window and tells the page where the aristide server is. Usage:
//!
//! ```text
//! aristide-console [SERVER_URL]      # default http://127.0.0.1:9669
//! ```

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// The server base URL the frontend should talk to.
#[tauri::command]
fn server_url() -> String {
    std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:9669".into())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![server_url])
        .run(tauri::generate_context!())
        .expect("run tauri application");
}
