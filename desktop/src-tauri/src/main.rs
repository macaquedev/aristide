#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use aristide_server::{ApiReply, DesktopRuntime, RuntimeStatus};
use std::sync::Arc;
use tauri::Manager;

#[tauri::command]
fn runtime_status(runtime: tauri::State<'_, Arc<DesktopRuntime>>) -> RuntimeStatus {
    runtime.status()
}

#[tauri::command]
async fn api_request(
    runtime: tauri::State<'_, Arc<DesktopRuntime>>,
    method: String,
    url: String,
) -> Result<ApiReply, String> {
    let runtime = Arc::clone(runtime.inner());
    tauri::async_runtime::spawn_blocking(move || runtime.request(&method, &url))
        .await
        .map_err(|_| "audio-unavailable".to_string())?
        .map_err(str::to_string)
}

fn main() {
    let _ = tracing_subscriber::fmt::try_init();
    tauri::Builder::default()
        .setup(|app| {
            app.manage(Arc::new(DesktopRuntime::start()?));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![runtime_status, api_request])
        .build(tauri::generate_context!())
        .expect("could not create the Aristide window")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                app.state::<Arc<DesktopRuntime>>().shutdown();
            }
        });
}
