//! Native hosts share the control plane without opening a network port.
//! There is exactly one audio runtime per process, independent of webviews.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

use serde::Serialize;
use tiny_http::Method;

use crate::{Args, SHUTDOWN, State};

pub(crate) type Ready = Box<dyn FnOnce(Arc<Mutex<State>>) + Send>;
static STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
pub struct RuntimeStatus {
    pub ready: bool,
    /// Stable error code; diagnostics are logged, never shown as raw UI text.
    pub error: Option<&'static str>,
}

#[derive(Serialize)]
pub struct ApiReply {
    pub status: u16,
    pub body: serde_json::Value,
}

#[derive(Default)]
struct Connection {
    state: Option<Arc<Mutex<State>>>,
    failed: bool,
}

pub struct DesktopRuntime {
    connection: Arc<Mutex<Connection>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl DesktopRuntime {
    pub fn start() -> std::io::Result<Self> {
        if STARTED.swap(true, Ordering::SeqCst) {
            return Err(std::io::Error::other(
                "an audio runtime has already started",
            ));
        }
        let connection = Arc::new(Mutex::new(Connection::default()));
        let status = Arc::clone(&connection);
        let ready = Arc::clone(&connection);
        let worker = std::thread::Builder::new()
            .name("aristide-runtime".into())
            .spawn(move || {
                let result = crate::run_server(
                    Args::default(),
                    Some(Box::new(move |state| {
                        ready.lock().expect("runtime status poisoned").state = Some(state);
                    })),
                );
                if let Err(error) = result {
                    tracing::error!("audio runtime stopped: {error:#}");
                    let mut status = status.lock().expect("runtime status poisoned");
                    status.state = None;
                    status.failed = true;
                }
            })?;
        Ok(Self {
            connection,
            worker: Mutex::new(Some(worker)),
        })
    }

    pub fn status(&self) -> RuntimeStatus {
        let connection = self.connection.lock().expect("runtime status poisoned");
        RuntimeStatus {
            ready: connection.state.is_some(),
            error: connection.failed.then_some("audio-unavailable"),
        }
    }

    /// Tauri commands run this on the control side, never the audio callback.
    pub fn request(&self, method: &str, url: &str) -> Result<ApiReply, &'static str> {
        let state = self
            .connection
            .lock()
            .map_err(|_| "audio-unavailable")?
            .state
            .clone()
            .ok_or("audio-unavailable")?;
        request(&state, method, url)
    }

    pub fn shutdown(&self) {
        SHUTDOWN.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.lock().expect("runtime worker poisoned").take() {
            let _ = worker.join();
        }
    }
}

impl Drop for DesktopRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn request(state: &Mutex<State>, method: &str, url: &str) -> Result<ApiReply, &'static str> {
    let method = match method {
        "GET" => Method::Get,
        "POST" => Method::Post,
        _ => return Err("invalid-request"),
    };
    if !url.starts_with("/api/") || url.len() > 65_536 {
        return Err("invalid-request");
    }
    let reply = crate::http::respond(state, &method, url);
    let status = reply.status_code().0;
    let bytes = reply.into_reader().into_inner();
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    Ok(ApiReply { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_bridge_uses_the_same_controls_and_rejects_non_api_requests() {
        let (_, handle) = aristide_engine::Engine::new(48_000.0, Arc::new(Default::default()));
        let state = Mutex::new(State::new(handle, None, Default::default(), 0.178, None));
        let reply = request(&state, "POST", "/api/gain?v=0.25").unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body["gain"], 0.25);
        assert_eq!(
            request(&state, "GET", "/api/state").unwrap().body["gain"],
            0.25
        );
        assert!(request(&state, "DELETE", "/api/state").is_err());
        assert!(request(&state, "GET", "file:///etc/passwd").is_err());
        assert_eq!(request(&state, "POST", "/api/unknown").unwrap().status, 404);
    }
}
