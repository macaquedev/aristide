// The server client: state polling and control commands over the local
// HTTP API — the JS twin of the egui client (crates/aristide-gui/src/client.rs).
//
// Every POST returns the refreshed state snapshot, so commands double as
// an immediate poll: the UI never waits out the poll interval to confirm
// what it just did.

const POLL_INTERVAL_MS = 250;

/// Where the server is. Priority: CLI arg via the Tauri shell,
/// `?server=` query param, then the page's own origin (for the day the
/// server serves this console itself).
export async function resolveBase() {
  if (window.__TAURI__) {
    return await window.__TAURI__.core.invoke("server_url");
  }
  const param = new URLSearchParams(location.search).get("server");
  if (param) return param.replace(/\/$/, "");
  return "";
}

export const commands = {
  stop: (id, on) => `/api/stop?id=${id}&on=${on ? 1 : 0}`,
  coupler: (idx, on) => `/api/coupler?idx=${idx}&on=${on ? 1 : 0}`,
  tremulant: (on) => `/api/trem?on=${on ? 1 : 0}`,
  gain: (v) => `/api/gain?v=${v}`,
  reverb: (wet) => `/api/reverb?wet=${wet}`,
  noises: (on, vol) => `/api/noises?on=${on ? 1 : 0}&vol=${vol}`,
  // Partial updates: the server only applies the params present, so a
  // temperament change never has to re-send pitch or transposition.
  tuning: (fields) => `/api/tuning?${new URLSearchParams(fields)}`,
  enclosure: (idx, value) => `/api/enclosure?idx=${idx}&v=${value}`,
  note: (manual, key, on) =>
    `/api/note?manual=${manual}&key=${key}&on=${on ? 1 : 0}`,
  panic: () => "/api/panic",
};

/// Start the client. Calls `onState(snapshot)` for every fresh snapshot
/// and `onError(message)` when the server is unreachable. Returns
/// `send(query)`, which POSTs one command and feeds its response back
/// through `onState`.
export function connect(base, onState, onError) {
  let pollTimer = null;

  async function request(method, query) {
    try {
      const response = await fetch(base + query, { method });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      const snapshot = await response.json();
      onState(snapshot);
    } catch (err) {
      onError(String(err));
    }
  }

  function schedule() {
    pollTimer = setTimeout(async () => {
      await request("GET", "/api/state");
      schedule();
    }, POLL_INTERVAL_MS);
  }

  request("GET", "/api/state").then(schedule);

  return function send(query) {
    // A command interrupts the cadence; its response is the poll.
    clearTimeout(pollTimer);
    request("POST", query).then(schedule);
  };
}
