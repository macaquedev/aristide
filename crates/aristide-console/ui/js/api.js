// The server client: state polling and control commands over the local
// HTTP API — the JS twin of the egui client (crates/aristide-gui/src/client.rs).
//
// Every POST returns the refreshed state snapshot, so commands double as
// an immediate poll: the UI never waits out the poll interval to confirm
// what it just did.

const POLL_INTERVAL_MS = 120;

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
  // Input routing, manual first: this manual listens to that input.
  // `slot` numbers a manual's inputs; one past the end adds another.
  // `channel` is 1-16 or "any"; omitting it lets the server pick the
  // channel the sample set suggests for that manual.
  // `low`/`high` are MIDI notes, or "set" to go back to the sample
  // set's own compass; omitted, the input keeps the range it has.
  midiBind: (manual, slot, device, channel, low, high) =>
    `/api/midi/bind?manual=${manual}&slot=${slot}&device=${encodeURIComponent(device)}` +
    (channel == null ? "" : `&ch=${channel}`) +
    (low == null ? "" : `&low=${low}`) +
    (high == null ? "" : `&high=${high}`),
  midiUnbind: (manual, slot) => `/api/midi/unbind?manual=${manual}&slot=${slot}`,
  // With no target, stop listening.
  midiLearn: (manual, slot) =>
    manual == null ? "/api/midi/learn" : `/api/midi/learn?manual=${manual}&slot=${slot}`,
  midiRescan: () => "/api/midi/rescan",
  // Controls: a message doing an action, with no manual of its own.
  // `fields` is the same "only what's given" partial update as tuning's —
  // device/ch/trigger/manual, any left out keeps what the slot already had.
  controlBind: (slot, action, fields = {}) =>
    `/api/control/bind?slot=${slot}&action=${encodeURIComponent(action)}` +
    (Object.keys(fields).length ? `&${new URLSearchParams(fields)}` : ""),
  controlUnbind: (slot) => `/api/control/unbind?slot=${slot}`,
  // With no slot, stop listening — same shape as midiLearn.
  controlLearn: (slot) =>
    slot == null ? "/api/control/learn" : `/api/control/learn?slot=${slot}`,
  // Where the computer keyboard plays, and any action by name — the
  // same verbs a binding uses, so a menu item and a piston can't drift.
  keyboardManual: (manual) => `/api/keyboard?manual=${manual}`,
  action: (name, device) =>
    `/api/action?do=${encodeURIComponent(name)}` +
    (device == null ? "" : `&device=${encodeURIComponent(device)}`),
  // A computer key, by physical position. The server decides what it
  // means — a note on the assigned manual, or whatever it is bound to.
  key: (code, on) => `/api/key?code=${encodeURIComponent(code)}&on=${on ? 1 : 0}`,
  note: (manual, key, on) =>
    `/api/note?manual=${manual}&key=${key}&on=${on ? 1 : 0}`,
  panic: () => "/api/panic",
  cancel: () => "/api/cancel",
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
