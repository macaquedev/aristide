// Computer-keyboard note entry: the two QWERTY letter rows play a manual
// like a piano, in the mapping every DAW uses — bottom row from C, top row
// an octave above, the row above each holding its sharps.
//
// Keys are addressed by `event.code` (physical position), so the layout is
// the same shape on QWERTZ or AZERTY as it is on QWERTY. Notes go out
// through the same `/api/note` command the on-screen keyboard uses, so the
// server's `held` set lights the pressed keys with no extra plumbing.

import { commands } from "./api.js";

/// Physical key codes in semitone order, low to high. Index = semitones
/// above the row's base C; both rows are one unbroken run, so the mapping
/// is the list itself rather than a table of pairs.
const LOWER = [
  "KeyZ", "KeyS", "KeyX", "KeyD", "KeyC", "KeyV", "KeyG", "KeyB", "KeyH",
  "KeyN", "KeyJ", "KeyM", "Comma", "KeyL", "Period", "Semicolon", "Slash",
];
const UPPER = [
  "KeyQ", "Digit2", "KeyW", "Digit3", "KeyE", "KeyR", "Digit5", "KeyT",
  "Digit6", "KeyY", "Digit7", "KeyU", "KeyI", "Digit9", "KeyO", "Digit0",
  "KeyP",
];

const LOWER_BASE = 48; // C3, with middle C = C4 = 60
const UPPER_BASE = 60;

const OCTAVE_DOWN = "Minus";
const OCTAVE_UP = "Equal";
const OCTAVE_LIMIT = 3;

const NAMES = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
const SHARPS = new Set([1, 3, 6, 8, 10]);

const noteName = (midi) => `${NAMES[midi % 12]}${Math.floor(midi / 12) - 1}`;

/// "KeyZ" -> "Z", "Digit2" -> "2", punctuation spelled as its glyph.
function cap(code) {
  const punctuation = { Comma: ",", Period: ".", Semicolon: ";", Slash: "/" };
  return punctuation[code] ?? code.replace(/^(Key|Digit)/, "");
}

/// Semitone offsets by code, built once from the two rows.
const OFFSETS = new Map();
for (const [row, base] of [[LOWER, LOWER_BASE], [UPPER, UPPER_BASE]]) {
  row.forEach((code, semitone) => OFFSETS.set(code, base + semitone));
}

/// Text entry wins: a keystroke aimed at a field is never a note.
function typing(target) {
  if (!target || !target.tagName) return false;
  return (
    /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName) || target.isContentEditable
  );
}

export class PianoKeys {
  constructor(root, send) {
    this.root = root;
    this.send = send;
    this.down = new Map(); // code -> {manual, midi} sounded at keydown
    this.octave = 0;
    this.manuals = [];
    this.target = null; // the manual notes go to
    this.signature = null;
    this.el = {
      button: root.getElementById("keys"),
      panel: root.getElementById("keys-legend"),
      settings: root.getElementById("settings"),
    };
    this.buildLegend();
    this.wire();
  }

  // ---- state ----------------------------------------------------------

  /// Follow the snapshot: keyboard-playable manuals are the ones the
  /// console draws, minus the pedalboard. The default target is the Great
  /// (or whatever the organ calls its principal manual), else the first.
  update(snapshot) {
    const signature = JSON.stringify(
      snapshot.manuals.map((m) => [m.idx, m.name, m.first_key, m.key_count])
    );
    if (signature === this.signature) return;
    this.signature = signature;

    const pedal = snapshot.manuals.find((m) => /p[ée]d/i.test(m.name))
      ?? snapshot.manuals[0];
    this.manuals = snapshot.manuals.filter((m) => m !== pedal);
    if (!this.manuals.length) this.manuals = snapshot.manuals;

    const great = this.manuals.find((m) => /great|haupt|grand.?orgue|main/i.test(m.name));
    this.target = great ?? this.manuals[0] ?? null;
    this.fillManuals();
    this.paintLegend();
  }

  /// The MIDI note a code plays right now, or null if it is unbound or
  /// falls outside the target manual's compass.
  noteFor(code) {
    const offset = OFFSETS.get(code);
    if (offset === undefined || !this.target) return null;
    const midi = offset + 12 * this.octave;
    const first = this.target.first_key;
    if (midi < first || midi >= first + this.target.key_count) return null;
    return midi;
  }

  // ---- playing ---------------------------------------------------------

  press(code) {
    if (this.down.has(code)) return;
    const midi = this.noteFor(code);
    if (midi === null) return;
    const manual = this.target.idx;
    this.down.set(code, { manual, midi });
    this.send(commands.note(manual, midi, true));
    this.paintCap(code, true);
  }

  release(code) {
    const note = this.down.get(code);
    if (!note) return;
    this.down.delete(code);
    this.send(commands.note(note.manual, note.midi, false));
    this.paintCap(code, false);
  }

  /// Losing the window mid-chord must not leave pipes speaking.
  releaseAll() {
    for (const code of [...this.down.keys()]) this.release(code);
  }

  shift(by) {
    const octave = Math.min(OCTAVE_LIMIT, Math.max(-OCTAVE_LIMIT, this.octave + by));
    if (octave === this.octave) return;
    this.releaseAll(); // the held notes belong to the old octave
    this.octave = octave;
    this.paintLegend();
  }

  // ---- input -----------------------------------------------------------

  wire() {
    window.addEventListener("keydown", (event) => {
      if (event.repeat || event.ctrlKey || event.metaKey || event.altKey) return;
      if (typing(event.target)) return;
      if (event.code === OCTAVE_DOWN || event.code === OCTAVE_UP) {
        event.preventDefault();
        this.shift(event.code === OCTAVE_UP ? +1 : -1);
        return;
      }
      if (!OFFSETS.has(event.code)) return;
      event.preventDefault(); // "/" opens WebKit's quick find otherwise
      this.press(event.code);
    });

    window.addEventListener("keyup", (event) => {
      if (this.down.has(event.code)) event.preventDefault();
      this.release(event.code);
    });

    // Alt-tabbing away or hiding the window releases everything: no key
    // up ever arrives for a chord held across a focus change.
    window.addEventListener("blur", () => this.releaseAll());
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) this.releaseAll();
    });

    this.el.button.addEventListener("click", () => this.toggle());
    // The settings drawer opens in the same strip of window; whichever
    // was asked for last is the one on screen.
    this.root.getElementById("tuning").addEventListener("click", () => this.close());
    this.el.manual.addEventListener("change", () => {
      this.releaseAll();
      const idx = Number(this.el.manual.value);
      this.target = this.manuals.find((m) => m.idx === idx) ?? this.target;
      this.paintLegend();
      this.el.manual.blur(); // give the keys back to the organ
    });
  }

  toggle() {
    const closed = this.el.panel.classList.toggle("hidden");
    this.el.button.classList.toggle("on", !closed);
    if (!closed) this.el.settings.classList.add("hidden"); // one drawer at a time
  }

  close() {
    this.el.panel.classList.add("hidden");
    this.el.button.classList.remove("on");
  }

  // ---- legend -----------------------------------------------------------

  /// A flat map of the two rows: one cap per key, sharps drawn dark, with
  /// the note each cap currently plays under it.
  buildLegend() {
    const panel = this.el.panel;
    panel.replaceChildren();

    const head = document.createElement("div");
    head.className = "legend-head";
    const title = document.createElement("span");
    title.className = "rail-label";
    title.textContent = "PLAY WITH";
    this.el.manual = document.createElement("select");
    this.el.manual.id = "keys-manual";
    head.append(title, this.el.manual);
    panel.append(head);

    this.caps = new Map();
    for (const row of [UPPER, LOWER]) {
      const line = document.createElement("div");
      line.className = "legend-row";
      for (const code of row) {
        const key = document.createElement("span");
        key.className = "legend-cap";
        const label = document.createElement("b");
        label.textContent = cap(code);
        const note = document.createElement("i");
        key.append(label, note);
        line.append(key);
        this.caps.set(code, { key, note });
      }
      panel.append(line);
    }

    const foot = document.createElement("div");
    foot.className = "legend-foot";
    this.el.octave = document.createElement("span");
    foot.append(this.el.octave);
    panel.append(foot);
  }

  fillManuals() {
    this.el.manual.replaceChildren();
    for (const manual of this.manuals) {
      const option = document.createElement("option");
      option.value = manual.idx;
      option.textContent = manual.name;
      this.el.manual.append(option);
    }
    if (this.target) this.el.manual.value = this.target.idx;
  }

  /// Caps show what they play under the current octave shift; keys off the
  /// end of the manual read as unavailable.
  paintLegend() {
    for (const [code, { key, note }] of this.caps) {
      const midi = this.noteFor(code);
      key.classList.toggle("sharp", SHARPS.has(OFFSETS.get(code) % 12));
      key.classList.toggle("out", midi === null);
      note.textContent = midi === null ? "—" : noteName(midi);
    }
    const shift = this.octave > 0 ? `+${this.octave}` : `${this.octave}`;
    this.el.octave.textContent =
      `− / =  shift octave  (${shift})`;
  }

  paintCap(code, on) {
    this.caps.get(code)?.key.classList.toggle("playing", on);
  }
}
