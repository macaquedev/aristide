// Computer-keyboard note entry: the two QWERTY letter rows play a manual
// like a piano, in the mapping every DAW uses — bottom row from C, top row
// an octave above, the row above each holding its sharps.
//
// Keys are addressed by `event.code` (physical position), so the layout is
// the same shape on QWERTZ or AZERTY as it is on QWERTY. Every press goes
// to the server as a *key*, not as a note: the computer keyboard is an
// input like any other there, with a manual, a shift, and bindings, so
// what a key does is the server's to decide. This file only draws what
// that decision came to — the table below mirrors `control::KEYBOARD_ROWS`
// and must stay in step with it.

import { commands, COMPUTER_KEYBOARD } from "./api.js";
import { keyName, shiftWords } from "./pitch.js";

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

const SHARPS = new Set([1, 3, 6, 8, 10]);

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
    this.down = new Set(); // codes currently held, so a repeat is not a second press
    this.keyboard = null; // {manual, transpose, low, high} from the server
    this.bound = new Set(); // key codes the player has bound to an action
    this.learning = false; // Preferences is waiting to be taught a control
    this.target = null; // the manual notes go to
    this.signature = null;
    this.el = { panel: root.getElementById("keys-legend") };
    this.buildLegend();
    this.wire();
  }

  // ---- state ----------------------------------------------------------

  /// Follow the snapshot. The legend only *shows* the assignment —
  /// where the keyboard plays is set in Preferences → MIDI, the same
  /// place as every other device.
  update(snapshot) {
    // Which keys are worth sending is snapshot state too: a key nobody
    // has bound should keep doing whatever the browser does with it.
    this.bound = new Set(
      (snapshot.controls ?? [])
        .filter((c) => c.device === COMPUTER_KEYBOARD && c.trigger.startsWith("key:"))
        .map((c) => c.trigger.slice(4))
    );
    this.learning = snapshot.control_learning != null;

    const signature = JSON.stringify([
      snapshot.manuals.map((m) => [m.idx, m.name, m.first_key, m.key_count]),
      snapshot.keyboard ?? null,
    ]);
    if (signature === this.signature) return;
    this.signature = signature;

    // Where the keyboard plays, and how far it is shifted, are the
    // server's: an octave button on a MIDI console moves it too.
    this.keyboard = snapshot.keyboard ?? null;
    this.target =
      snapshot.manuals.find((m) => m.idx === this.keyboard?.manual) ?? null;
    this.paintLegend();
  }

  /// The MIDI note a code plays right now, or null if it is unbound, the
  /// keyboard is unassigned, or the note falls outside its manual.
  noteFor(code) {
    const offset = OFFSETS.get(code);
    if (offset === undefined || !this.target || !this.keyboard) return null;
    const midi = offset + this.keyboard.transpose;
    const first = this.target.first_key;
    if (midi < first || midi >= first + this.target.key_count) return null;
    return midi;
  }

  // ---- playing ---------------------------------------------------------

  press(code) {
    if (this.down.has(code)) return;
    this.down.add(code);
    this.send(commands.key(code, true));
    this.paintCap(code, true);
  }

  release(code) {
    if (!this.down.delete(code)) return;
    this.send(commands.key(code, false));
    this.paintCap(code, false);
  }

  /// Losing the window mid-chord must not leave pipes speaking.
  releaseAll() {
    for (const code of [...this.down]) this.release(code);
  }

  // ---- input -----------------------------------------------------------

  /// The organ has the keyboard only when no dialog is up: inside
  /// Preferences a stray "z" must not sound a pipe.
  get busy() {
    return document.body.classList.contains("modal-open");
  }

  wire() {
    window.addEventListener("keydown", (event) => {
      if (event.repeat || event.ctrlKey || event.metaKey || event.altKey) return;
      // Text entry always wins. A dialog otherwise silences the keys,
      // except while it is listening for the one to bind.
      if (typing(event.target) || (this.busy && !this.learning)) return;
      // Every key the server has a use for goes to it: the note rows,
      // anything bound to an action, and — while Preferences is waiting
      // to be taught — whatever the player presses next.
      if (!this.learning && !OFFSETS.has(event.code) && !this.bound.has(event.code)) {
        return;
      }
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
  }

  get isOpen() {
    return !this.el.panel.classList.contains("hidden");
  }

  toggle() {
    this.el.panel.classList.toggle("hidden");
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
    title.textContent = "COMPUTER KEYBOARD";
    this.el.plays = document.createElement("span");
    head.append(title, this.el.plays);
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

  /// Caps show what they play under the current octave shift; keys off the
  /// end of the manual read as unavailable. Where the keyboard plays is
  /// only reported here — it is assigned in Preferences → MIDI, the
  /// same place as every other device.
  paintLegend() {
    for (const [code, { key, note }] of this.caps) {
      const midi = this.noteFor(code);
      key.classList.toggle("sharp", SHARPS.has(OFFSETS.get(code) % 12));
      key.classList.toggle("out", midi === null);
      note.textContent = midi === null ? "—" : keyName(midi);
    }
    this.el.plays.textContent = this.target
      ? `plays ${this.target.name}`
      : "plays nothing";
    const spoken = shiftWords(this.keyboard?.transpose ?? 0);
    this.el.octave.textContent = this.keyboard
      ? [spoken && `sounding ${spoken}`, "octave keys are bindings, in Preferences → Controls"]
          .filter(Boolean)
          .join(" · ")
      : "assign it a manual in Preferences → MIDI, like any device";
  }

  paintCap(code, on) {
    this.caps.get(code)?.key.classList.toggle("playing", on);
  }
}
