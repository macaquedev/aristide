// One owner per finger, shared ownership for duplicate hex keys.
// Releasing a finger must never release the rest of a chord.
export class PointerNotes {
  constructor(send, host = window) {
    this.send = send;
    this.pointers = new Map();
    host.addEventListener("pointerup", (event) => this.release(event.pointerId));
    host.addEventListener("pointercancel", (event) => this.release(event.pointerId));
    host.addEventListener("blur", () => this.releaseAll());
    host.document?.addEventListener("visibilitychange", () => {
      if (host.document.hidden) this.releaseAll();
    });
  }

  bind(key, manual, midi) {
    key.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      this.release(event.pointerId);
      const sounding = this.hasNote(manual, midi);
      this.pointers.set(event.pointerId, { key, manual, midi });
      key.classList.add("pressed", "held");
      if (!sounding) this.send(manual, midi, true);
      // Touch keeps a note held while the finger drifts off its small
      // key. Mouse retains the existing leave-to-release behavior.
      if (event.pointerType !== "mouse") key.setPointerCapture?.(event.pointerId);
    });
    key.addEventListener("pointerleave", (event) => {
      if (event.pointerType === "mouse") this.release(event.pointerId);
    });
    key.addEventListener("lostpointercapture", (event) => this.release(event.pointerId));
  }

  hasNote(manual, midi) {
    return [...this.pointers.values()].some((note) => note.manual === manual && note.midi === midi);
  }

  release(id) {
    const note = this.pointers.get(id);
    if (!note) return;
    this.pointers.delete(id);
    if (![...this.pointers.values()].some((other) => other.key === note.key)) {
      note.key.classList.remove("pressed", "held");
    }
    if (!this.hasNote(note.manual, note.midi)) this.send(note.manual, note.midi, false);
  }

  releaseManual(manual) {
    for (const [id, note] of this.pointers) if (note.manual === manual) this.release(id);
  }

  releaseAll() {
    for (const id of this.pointers.keys()) this.release(id);
  }
}
