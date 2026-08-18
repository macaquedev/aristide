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
  // `fields.manual`, when given, tunes that division apart from the
  // instrument rather than the whole organ; `fields.reset: 1` alongside
  // a manual drops that division's own tuning and goes back to sharing
  // the instrument's.
  tuning: (fields) => `/api/tuning?${new URLSearchParams(fields)}`,
  enclosure: (idx, value) => `/api/enclosure?idx=${idx}&v=${value}`,
  // Input routing, manual first: this manual listens to that input.
  // `slot` numbers a manual's inputs; one past the end adds another.
  // `channel` is 1-16 or "any"; omitting it lets the server pick the
  // channel the sample set suggests for that manual.
  // `low`/`high` are MIDI notes, or "set" to go back to the sample
  // set's own compass; omitted, the input keeps the range it has.
  // `transpose` shifts every note the keyboard sends, in semitones;
  // omitted, the input keeps the shift it has.
  midiBind: (manual, slot, device, channel, low, high, transpose) =>
    `/api/midi/bind?manual=${manual}&slot=${slot}&device=${encodeURIComponent(device)}` +
    (channel == null ? "" : `&ch=${channel}`) +
    (low == null ? "" : `&low=${low}`) +
    (high == null ? "" : `&high=${high}`) +
    (transpose == null ? "" : `&transpose=${transpose}`),
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
  // A manual's compass, MIDI notes 0-127. Omitting low/high goes back to
  // the sample set's own (native) compass rather than declaring one.
  organCompass: (manual, low, high) =>
    `/api/organ/compass?manual=${manual}` +
    (low == null ? "" : `&low=${low}`) +
    (high == null ? "" : `&high=${high}`),
  // Writes the current combination out as a composite organ file, so it
  // loads the same way next time instead of being re-assembled from the
  // command line.
  organSave: (path) => `/api/organ/save?path=${encodeURIComponent(path)}`,
  // Reassigns a stop to a different manual's division.
  organMove: (stopId, manual) => `/api/organ/move?stop=${stopId}&manual=${manual}`,
  // Takes a coupler off the console (keep=0) or restores it (keep=1) —
  // distinct from the rail's own on/off, which only engages a coupler
  // that's already on the console.
  organCoupler: (idx, keep) => `/api/organ/coupler?idx=${idx}&keep=${keep ? 1 : 0}`,
  // Queues loading a `.organ`/`.toml` file, from the library or from
  // Browse. 400s with a plain-text reason if the path isn't a file or a
  // load is already running.
  organLoad: (path) => `/api/organ/load?path=${encodeURIComponent(path)}`,
  // Drops one entry from the library without touching the file itself.
  libraryForget: (path) => `/api/library/forget?path=${encodeURIComponent(path)}`,
};

/// Start the client. Calls `onState(snapshot)` for every fresh snapshot
/// and `onError(message)` when the server is unreachable. Returns
/// `send(query)`, which POSTs one command and feeds its response back
/// through `onState`.
export function connect(base, onState, onError) {
  let pollTimer = null;
  let issued = 0; // requests dispatched, in order
  let applied = 0; // the newest request whose snapshot has been applied

  async function request(method, query) {
    // Responses come back out of order: a slow poll dispatched before a
    // command can resolve after it, and repainting the UI with that
    // older snapshot undoes what the command just showed — a flicker on
    // every interaction. Snapshots carry no clock of their own, so the
    // dispatch order is the one we have: a response is applied only if
    // nothing dispatched later has been applied already.
    const id = ++issued;
    try {
      const response = await fetch(base + query, { method });
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      const snapshot = await response.json();
      if (id > applied) {
        applied = id;
        onState(snapshot);
      }
    } catch (err) {
      onError(String(err));
    }
  }

  // Replaces any pending poll rather than adding one: every command
  // chain ends by calling this, and two overlapping commands must not
  // leave two poll loops running (each leak multiplies the request
  // rate, forever).
  function schedule() {
    clearTimeout(pollTimer);
    pollTimer = setTimeout(async () => {
      await request("GET", "/api/state");
      schedule();
    }, POLL_INTERVAL_MS);
  }

  request("GET", "/api/state").then(schedule);

  return function send(query) {
    // A command interrupts the cadence; its response is the poll.
    request("POST", query).then(schedule);
  };
}
