//! The desktop shell: a Tauri window around the web console in `ui/`.
//!
//! All console logic lives in the frontend; this binary opens the
//! window, makes sure a server is there to talk to, and tells the page
//! where it is. Usage:
//!
//! ```text
//! aristide-console [SERVER_URL]           # attach to a running server
//! aristide-console [server args…]         # start one and play
//! ```
//!
//! The second form is the one command a player needs: every argument is
//! handed to `aristide-server` untouched (`set.organ`, `--stops`,
//! `--buffer`, …), the server runs as a child for exactly as long as
//! the window is open, and its logs land in the launching terminal. If
//! a server already answers on the port, the console attaches to it
//! instead of starting a second one — the headless split stays real:
//! the server never needs this shell, this shell merely saves you a
//! terminal.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::Duration;

/// Where the frontend should point, and the server we started to make
/// that address answer — `None` when one was already running (or the
/// player named a remote one), in which case its lifetime is not ours
/// to manage.
struct Backend {
    url: String,
    child: Mutex<Option<Child>>,
}

/// The server base URL the frontend should talk to.
#[tauri::command]
fn server_url(backend: tauri::State<Backend>) -> String {
    backend.url.clone()
}

/// Zoom the whole page, as a browser's Ctrl+plus would: `1.0` is the
/// console's native size. The frontend asks for this from Preferences
/// (theme.js) — a webview zoom rather than a CSS one so that pointer
/// coordinates, panel layout and every popover's geometry stay in the
/// same CSS pixels the code was written in. The choice itself is the
/// player's and lives in the page's localStorage; the page re-applies
/// it at every start.
#[tauri::command]
fn set_zoom(webview: tauri::Webview, scale: f64) -> Result<(), String> {
    webview.set_zoom(scale).map_err(|err| err.to_string())
}

/// The port the spawned server will serve on: whatever `--http-port`
/// says, read the same way the server reads it, so the window and the
/// child cannot disagree about where to meet.
fn http_port(args: &[String]) -> u16 {
    args.iter()
        .position(|arg| arg == "--http-port")
        .and_then(|at| args.get(at + 1))
        .and_then(|port| port.parse().ok())
        .unwrap_or(9669)
}

fn server_running(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

/// Start `aristide-server` with the console's own arguments: the copy
/// installed next to this binary if there is one (which is where both
/// `cargo build` and any packaging put it), else whatever `PATH` finds.
fn spawn_server(args: &[String]) -> Option<Child> {
    let sibling = std::env::current_exe().ok().and_then(|exe| {
        let name = format!("aristide-server{}", std::env::consts::EXE_SUFFIX);
        let path = exe.parent()?.join(name);
        path.exists().then_some(path)
    });
    let program = sibling.unwrap_or_else(|| "aristide-server".into());
    match Command::new(&program).args(args).spawn() {
        Ok(child) => Some(child),
        Err(err) => {
            eprintln!("aristide-console: could not start {}: {err}", program.display());
            eprintln!("start `aristide-server` yourself, or pass its URL");
            None
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let backend = match args.first().filter(|arg| arg.starts_with("http")) {
        Some(url) => Backend {
            url: url.trim_end_matches('/').to_string(),
            child: Mutex::new(None),
        },
        None => {
            let port = http_port(&args);
            let child = (!server_running(port)).then(|| spawn_server(&args)).flatten();
            Backend {
                url: format!("http://127.0.0.1:{port}"),
                child: Mutex::new(child),
            }
        }
    };
    tauri::Builder::default()
        // The native open-file dialog, for picking a sample set to
        // load — the one thing the web console can't do by itself.
        .plugin(tauri_plugin_dialog::init())
        .manage(backend)
        .invoke_handler(tauri::generate_handler![server_url, set_zoom])
        .build(tauri::generate_context!())
        .expect("build tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                let backend: tauri::State<Backend> = app.state();
                // A kill is safe here: the server persists every change
                // as it happens, so there is nothing to flush at exit.
                if let Some(mut child) = backend.child.lock().unwrap().take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        });
}
