// The Preferences dialog: the player's, never the organ's. Everything
// in it is local to this console (theme.js writes localStorage) and
// nothing in it sends a command to the server — that separation is the
// contract, not an accident. Organ facts — wiring, tuning, the room,
// structure — are edited on the console surface and land in the
// organ's own file (see editor.js).

export class Preferences {
  constructor(root) {
    this.root = root;
    this.el = {
      modal: root.getElementById("prefs"),
      about: root.getElementById("about"),
    };
    this.wire();
  }

  open() {
    this.el.modal.classList.remove("hidden");
    this.root.body.classList.add("modal-open");
  }

  close() {
    this.el.modal.classList.add("hidden");
    this.el.about.classList.add("hidden");
    this.root.body.classList.remove("modal-open");
  }

  get isOpen() {
    return !this.el.modal.classList.contains("hidden");
  }

  openAbout() {
    this.el.about.classList.remove("hidden");
    this.root.body.classList.add("modal-open");
  }

  wire() {
    for (const closer of this.root.querySelectorAll("#prefs [data-close], #about [data-close]")) {
      closer.addEventListener("click", () => this.close());
    }
    // Esc closes preferences or about when either is the one up — not
    // just whenever *some* modal (the organ picker, say) has set
    // `modal-open` on the body. The console keeps its keys otherwise
    // (keys.js stays quiet while `modal-open` is set).
    window.addEventListener("keydown", (event) => {
      const open = this.isOpen || !this.el.about.classList.contains("hidden");
      if (event.key === "Escape" && open) {
        event.preventDefault();
        this.close();
      }
    });
  }
}
