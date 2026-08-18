// The organ picker: how the player gets from a bare console to a
// sounding instrument. The server starts organ-less, so this modal
// auto-opens with nothing behind it to interact with, and stays up
// (unclosable) until a load finishes. Once an organ is loaded it
// becomes an ordinary dialog, reachable from Organ ▸ Open organ… to
// switch instruments later.
//
// Two ways to name a file: the library (organs this machine has loaded
// before) and Browse (a live directory listing via /api/browse). Both
// end the same way — POSTing organLoad, which the usual send()/poll
// loop turns into a `loading` snapshot and eventually a new `organ`.

import { commands } from "./api.js";

export class Picker {
  constructor(root, base, send) {
    this.root = root;
    this.base = base;
    this.send = send;
    this.lastOrgan = undefined; // mirrors snapshot.organ, kept even while closed
    this.openedWithOrgan = undefined; // snapshot.organ at the moment this opened
    this.signature = null;
    // Browse state lives outside the snapshot — it's this client's own
    // directory listing, fetched directly rather than polled.
    this.dir = null;
    this.browseParent = null;
    this.browseEntries = null;
    this.browseError = null;
    this.el = {
      modal: root.getElementById("picker"),
      close: root.getElementById("picker-close"),
      error: root.getElementById("picker-error"),
      loading: root.getElementById("picker-loading"),
      loadingText: root.getElementById("picker-loading-text"),
      sections: root.getElementById("picker-sections"),
      library: root.getElementById("picker-library"),
      up: root.getElementById("picker-up"),
      dir: root.getElementById("picker-dir"),
      browseError: root.getElementById("picker-browse-error"),
      browseList: root.getElementById("picker-browse-list"),
    };
    this.wire();
  }

  get isOpen() {
    return !this.el.modal.classList.contains("hidden");
  }

  /// True once there is a console behind the picker worth going back to
  /// — the one thing that decides whether it can be closed at all.
  get closable() {
    return !!this.lastOrgan;
  }

  /// Manual open, from Organ ▸ Open organ…: whatever is loaded right
  /// now is "home" for this session, so a pick that lands on the same
  /// organ again wouldn't be mistaken for one that didn't land at all.
  open() {
    this.openedWithOrgan = this.lastOrgan;
    this.show();
  }

  show() {
    this.el.modal.classList.remove("hidden");
    this.el.close.classList.toggle("hidden", !this.closable);
    this.root.body.classList.add("modal-open");
    if (this.dir === null) this.browse();
  }

  close() {
    if (!this.closable) return;
    this.el.modal.classList.add("hidden");
    this.root.body.classList.remove("modal-open");
    // Next open starts at home again, not wherever browsing left off.
    this.dir = null;
    this.browseParent = null;
    this.browseEntries = null;
    this.browseError = null;
  }

  wire() {
    for (const closer of this.root.querySelectorAll("#picker [data-close]")) {
      closer.addEventListener("click", () => this.close());
    }
    window.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.isOpen) {
        event.preventDefault();
        this.close();
      }
    });
    this.el.up.addEventListener("click", () => {
      if (this.browseParent) this.browse(this.browseParent);
    });
  }

  // ---- snapshot ------------------------------------------------------

  update(snapshot) {
    const hasOrgan = !!snapshot.organ;
    const loading = snapshot.loading ?? null;
    const error = snapshot.load_error ?? null;
    const library = snapshot.library ?? [];
    this.lastOrgan = snapshot.organ;

    // Auto-open: there is nothing behind the console to use.
    if (!this.isOpen && !hasOrgan && !loading) {
      this.openedWithOrgan = undefined;
      this.show();
    }

    // A pick landed — either the auto-opened picker's first load, or a
    // manually opened one whose choice differs from what was loaded
    // when it was opened.
    if (this.isOpen && hasOrgan && snapshot.organ !== this.openedWithOrgan) {
      this.close();
    }

    if (!this.isOpen) return;

    this.el.close.classList.toggle("hidden", !this.closable);

    const signature = JSON.stringify([loading, error, library]);
    if (signature === this.signature) return;
    this.signature = signature;

    this.el.loading.classList.toggle("hidden", !loading);
    this.el.loadingText.textContent = loading ?? "";
    this.el.sections.classList.toggle("dim", !!loading);

    this.el.error.classList.toggle("hidden", !error);
    this.el.error.textContent = error ?? "";

    this.buildLibrary(library);
  }

  // ---- library ---------------------------------------------------------

  buildLibrary(library) {
    this.el.library.replaceChildren();
    if (!library.length) {
      this.el.library.append(
        this.emptyNote("No organs yet — browse for a sample set below.")
      );
      return;
    }
    for (const entry of library) this.el.library.append(this.libraryRow(entry));
  }

  /// A `<button>` can't nest the forget button (interactive content
  /// can't nest interactive content), so the row is a div playing
  /// button — click and Enter/Space both fire the load, same as a
  /// native one would.
  libraryRow(entry) {
    const row = document.createElement("div");
    row.className = "picker-row";
    row.setAttribute("role", "button");
    row.tabIndex = 0;
    row.title = entry.path;
    row.addEventListener("click", () => this.load(entry.path));
    row.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        this.load(entry.path);
      }
    });

    const text = document.createElement("span");
    text.className = "picker-row-text";
    const name = document.createElement("span");
    name.className = "picker-row-name";
    name.textContent = entry.name;
    const path = document.createElement("span");
    path.className = "picker-row-path";
    path.textContent = entry.path;
    text.append(name, path);
    row.append(text);

    const forget = document.createElement("button");
    forget.type = "button";
    forget.className = "picker-forget";
    forget.textContent = "×";
    forget.setAttribute("aria-label", `Forget ${entry.name}`);
    forget.addEventListener("click", (event) => {
      event.stopPropagation(); // don't also trigger the row's own load
      this.send(commands.libraryForget(entry.path));
    });
    row.append(forget);

    return row;
  }

  load(path) {
    this.send(commands.organLoad(path));
  }

  emptyNote(text) {
    const empty = document.createElement("p");
    empty.className = "pane-empty";
    empty.textContent = text;
    return empty;
  }

  // ---- browse ------------------------------------------------------------
  //
  // Not snapshot-driven: this is this client's own directory listing,
  // fetched directly (browse doesn't flow through the state machinery)
  // and re-fetched on navigation, never on a poll tick.

  async browse(dir) {
    try {
      const query = dir ? `/api/browse?dir=${encodeURIComponent(dir)}` : "/api/browse";
      const response = await fetch(this.base + query);
      if (!response.ok) {
        this.browseError = (await response.text()) || `${response.status} ${response.statusText}`;
        this.renderBrowse();
        return;
      }
      const data = await response.json();
      this.dir = data.dir;
      this.browseParent = data.parent;
      this.browseEntries = data.entries;
      this.browseError = null;
      this.renderBrowse();
    } catch (err) {
      this.browseError = String(err);
      this.renderBrowse();
    }
  }

  renderBrowse() {
    this.el.dir.textContent = this.dir ?? "";
    this.el.dir.title = this.dir ?? "";
    this.el.up.disabled = !this.browseParent;

    this.el.browseError.classList.toggle("hidden", !this.browseError);
    this.el.browseError.textContent = this.browseError ?? "";

    this.el.browseList.replaceChildren();
    if (this.browseError) return;
    const entries = this.browseEntries ?? [];
    if (!entries.length) {
      this.el.browseList.append(this.emptyNote("Nothing here."));
      return;
    }
    for (const entry of entries) this.el.browseList.append(this.browseRow(entry));
  }

  browseRow(entry) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = entry.dir ? "picker-row picker-browse-dir" : "picker-row";
    row.title = entry.path;
    row.addEventListener("click", () => {
      if (entry.dir) this.browse(entry.path);
      else this.load(entry.path);
    });

    const name = document.createElement("span");
    name.className = "picker-row-name";
    name.textContent = entry.name;
    row.append(name);
    return row;
  }
}
