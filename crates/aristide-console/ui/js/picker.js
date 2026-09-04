// The organ loader: how the player gets from a bare console to a
// sounding instrument. The server starts organ-less, so this modal
// auto-opens with nothing behind it to interact with, and stays up
// (unclosable) until a load finishes. Loads chosen by the player show
// progress and errors here; background structural rebuilds leave the
// editor usable. The organ-name menu's load items land here too.
//
// Three ways in: a new blank organ (a name becomes a composite file,
// via organNew), a sample set picked in the native file dialog (Tauri;
// a plain browser falls back to the server-side directory listing),
// and Recent — organs this machine has played before, most recent
// first. All end the same way — POSTing a load, which the usual
// send()/poll loop turns into a `loading` snapshot and eventually a
// new `organ`. Taking an organ off Recent removes only the list entry:
// its file stays (Browse's "Your organs" shortcut still reaches it)
// and loading its set finds it again.

import { commands, localFetch } from "./api.js";
import { setText } from "./dom.js";

// What the native open dialog offers: everything the server can load,
// or is meant to — GrandOrgue sets, unencrypted Hauptwerk definitions,
// Aristide composites.
const ORGAN_FILTER = {
  name: "Organs (GrandOrgue, Hauptwerk, Aristide)",
  extensions: ["organ", "toml", "Organ_Hauptwerk_xml"],
};

/// The format a path is in, as the library rows label it. "" for a
/// path whose extension names nothing loadable (it may still load —
/// the server decides, this is only a label).
function formatOf(path) {
  const lower = path.toLowerCase();
  if (lower.endsWith(".organ")) return "GrandOrgue";
  if (lower.endsWith(".toml")) return "Aristide";
  if (lower.endsWith(".organ_hauptwerk_xml")) return "Hauptwerk";
  return "";
}

export class Picker {
  constructor(root, base, send) {
    this.root = root;
    this.base = base;
    this.send = send;
    this.lastOrgan = undefined; // mirrors snapshot.organ, kept even while closed
    this.openedWithOrgan = undefined; // snapshot.organ at the moment this opened
    this.pickedHere = false; // a load was started from this open picker
    this.library = []; // mirrors snapshot.library — the Load menu reads it too
    this.snapshot = null;
    this.pickPending = false;
    this.requestError = null;
    this.search = "";
    this.librarySignature = null;
    // Browse state lives outside the snapshot — it's this client's own
    // directory listing, fetched directly rather than polled.
    this.dir = null;
    this.browseParent = null;
    this.browseOrgans = null; // where the console's own organ files live
    this.browseEntries = null;
    this.browseError = null;
    this.el = {
      modal: root.getElementById("picker"),
      close: root.getElementById("picker-close"),
      error: root.getElementById("picker-error"),
      loading: root.getElementById("picker-loading"),
      loadingText: root.getElementById("picker-loading-text"),
      sections: root.getElementById("picker-sections"),
      newBlank: root.getElementById("picker-new-blank"),
      newSet: root.getElementById("picker-new-set"),
      nameForm: root.getElementById("picker-name-form"),
      name: root.getElementById("picker-name"),
      library: root.getElementById("picker-library"),
      search: root.getElementById("picker-search"),
      browseStatus: root.getElementById("picker-browse-status"),
      browsePane: root.getElementById("picker-browse"),
      up: root.getElementById("picker-up"),
      organs: root.getElementById("picker-organs"),
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

  /// Manual open, from the organ-name menu: whatever is loaded now is
  /// "home" for this session, so a pick that lands on the same organ
  /// again wouldn't be mistaken for one that didn't land at all.
  open() {
    this.openedWithOrgan = this.lastOrgan;
    this.pickedHere = false;
    this.requestError = null;
    this.show();
  }

  /// Organ name ▸ New blank organ…: the dialog, name field ready.
  newBlank() {
    this.open();
    this.showNameForm();
  }

  /// Organ name ▸ New organ from a sample set…: the native file
  /// dialog; a plain browser gets the dialog with the server-side
  /// listing revealed instead.
  newFromSet() {
    if (window.__TAURI__) {
      this.openedWithOrgan = this.lastOrgan;
      this.pickSampleSet();
    } else {
      this.open();
      this.showBrowse();
    }
  }

  show() {
    this.el.modal.classList.remove("hidden");
    this.el.close.classList.toggle("hidden", !this.closable);
    this.root.body.classList.add("modal-open");
    this.paintStatus();
    this.buildLibrary(this.library);
  }

  close() {
    if (!this.closable || this.pickPending) return;
    this.el.modal.classList.add("hidden");
    this.root.body.classList.remove("modal-open");
    // Next open starts at home again: name field blank, browser shut.
    this.el.nameForm.classList.add("hidden");
    this.el.name.value = "";
    this.el.browsePane.classList.add("hidden");
    this.dir = null;
    this.browseParent = null;
    this.browseOrgans = null;
    this.browseEntries = null;
    this.browseError = null;
    this.browseRequest = (this.browseRequest ?? 0) + 1;
    this.browseBusy = false;
    this.search = "";
    this.el.search.value = "";
    this.requestError = null;
  }

  showNameForm() {
    this.el.browsePane.classList.add("hidden");
    this.el.nameForm.classList.remove("hidden");
    this.el.name.focus();
  }

  showBrowse() {
    this.el.nameForm.classList.add("hidden");
    this.el.browsePane.classList.remove("hidden");
    if (this.dir === null) this.browse();
  }

  /// The native open dialog, filtered to loadable organ formats. A
  /// cancelled dialog is not an error — nothing happens.
  async pickSampleSet() {
    const picked = await window.__TAURI__.core
      .invoke("plugin:dialog|open", {
        options: {
          title: "Choose a sample set or organ",
          filters: [ORGAN_FILTER],
          multiple: false,
          directory: false,
        },
      })
      .catch((error) => {
        if (!this.isOpen) this.open();
        this.requestError = `Could not open the file chooser: ${error}`;
        this.paintStatus();
        return null;
      });
    const path = Array.isArray(picked) ? picked[0] : picked;
    if (typeof path === "string" && path) this.load(path);
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
    this.el.search.addEventListener("input", () => {
      this.search = this.el.search.value.trim().toLocaleLowerCase();
      this.buildLibrary(this.library);
    });
    this.el.newBlank.addEventListener("click", () => this.showNameForm());
    this.el.nameForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const name = this.el.name.value.trim();
      if (name) this.startPick(commands.organNew(name));
    });
    this.el.newSet.addEventListener("click", () => {
      window.__TAURI__ ? this.pickSampleSet() : this.showBrowse();
    });
    this.el.up.addEventListener("click", () => {
      if (this.browseParent) this.browse(this.browseParent);
    });
    // The organs folder lives under a dotted config directory the
    // listing hides, so this shortcut is how an organ taken off Recent
    // is found again.
    this.el.organs.addEventListener("click", () => {
      if (this.browseOrgans) this.browse(this.browseOrgans);
    });
  }

  // ---- snapshot ------------------------------------------------------

  update(snapshot) {
    this.snapshot = snapshot;
    const hasOrgan = !!snapshot.organ;
    const loading = snapshot.loading ?? null;
    const error = snapshot.load_error ?? null;
    const library = snapshot.library ?? [];
    this.lastOrgan = snapshot.organ;
    this.library = library;

    // Auto-open: there is nothing behind the console to use. `loading`
    // alone no longer forces this open — once an organ is up, it also
    // flags an in-place structural edit (a manual added, a stop pulled,
    // on the canvas), and that must never pop the picker over an open
    // popover. A load started with no organ yet (the common case)
    // still opens this because !hasOrgan already covers it.
    if (!this.isOpen && !hasOrgan) {
      this.openedWithOrgan = undefined;
      this.pickedHere = false;
      this.show();
    }

    // A pick landed — either the auto-opened picker's first load (an
    // organ where there was none), or a pick made from this open picker
    // that finished cleanly, even one that circled back to the organ it
    // opened over (picks queue last-click-wins, so a change of mind can
    // land on "home" again). Not while `loading`: an organ landing with
    // another still on the way stays up until the last one lands.
    const landed =
      snapshot.organ !== this.openedWithOrgan || (this.pickedHere && !error);
    if (this.isOpen && hasOrgan && !loading && landed && !this.pickPending && !this.requestError) {
      this.close();
    }

    if (!this.isOpen) return;

    this.paintStatus();
    this.buildLibrary(library);
  }

  paintStatus() {
    const loading = this.pickPending ? "Starting the organ…" : this.snapshot?.loading;
    const error = this.requestError ?? this.snapshot?.load_error;
    this.el.close.classList.toggle("hidden", !this.closable);
    this.el.close.disabled = this.pickPending;
    this.el.loading.classList.toggle("hidden", !loading);
    setText(this.el.loadingText, loading ?? "");
    // While the request is being sent, a second Enter must not create
    // a second file. Once acknowledged, another selection can replace
    // a queued load — the server's existing last-choice-wins behavior.
    this.el.sections.inert = this.pickPending;
    this.el.sections.classList.toggle("dim", this.pickPending);
    this.el.error.classList.toggle("hidden", !error);
    setText(this.el.error, error ?? "");
  }

  async startPick(query) {
    if (this.pickPending) return;
    if (!this.isOpen) this.open();
    const initiatingControl = this.root.activeElement;
    this.pickPending = true;
    this.pickedHere = false;
    this.requestError = null;
    this.paintStatus();
    const result = await this.send(query, { reportErrors: false });
    this.pickPending = false;
    this.pickedHere = result.ok;
    this.requestError = result.error;
    // connect delivers the acknowledged state before resolving. Only
    // now may a successful load close the picker; an older poll while
    // the request was in flight could not do so.
    if (this.snapshot) this.update(this.snapshot);
    else this.paintStatus();
    if (!result.ok && initiatingControl?.isConnected && this.isOpen) initiatingControl.focus();
  }

  // ---- library ---------------------------------------------------------

  buildLibrary(library) {
    const signature = JSON.stringify([library, this.search]);
    if (signature === this.librarySignature) return;
    this.librarySignature = signature;
    this.el.search.classList.toggle("hidden", !library.length);
    this.el.library.replaceChildren();
    if (!library.length) {
      this.el.library.append(
        this.emptyNote("Nothing here yet — every organ you load is remembered.")
      );
      return;
    }
    const matches = library.filter((entry) =>
      `${entry.name} ${entry.path}`.toLocaleLowerCase().includes(this.search)
    );
    if (!matches.length) this.el.library.append(this.emptyNote("No matching organs. Try another name."));
    for (const entry of matches) this.el.library.append(this.libraryRow(entry));
  }

  /// Loading and removing are separate native buttons: activating the
  /// remove button with Enter or Space must never bubble into a load.
  libraryRow(entry) {
    const row = document.createElement("div");
    row.className = "picker-row picker-recent-row";
    const open = document.createElement("button");
    open.type = "button";
    open.className = "picker-open";
    open.title = entry.path;
    open.addEventListener("click", () => this.load(entry.path));

    const text = document.createElement("span");
    text.className = "picker-row-text";
    const name = document.createElement("span");
    name.className = "picker-row-name";
    name.textContent = entry.name;
    const path = document.createElement("span");
    path.className = "picker-row-path";
    path.textContent = entry.path.split(/[\\/]/).pop();
    text.append(name, path);
    open.append(text);

    // The format, ledger-style in its own right-hand column.
    const kind = formatOf(entry.path);
    if (kind) {
      const tag = document.createElement("span");
      tag.className = "picker-row-kind";
      tag.textContent = kind;
      open.append(tag);
    }

    const forget = document.createElement("button");
    forget.type = "button";
    forget.className = "picker-forget";
    forget.textContent = "×";
    forget.title = "Remove from Recent (the organ's file is kept)";
    forget.setAttribute("aria-label", `Remove ${entry.name} from Recent`);
    forget.addEventListener("click", (event) => {
      event.stopPropagation(); // don't also trigger the row's own load
      if (forget.disabled) return;
      forget.disabled = true;
      this.send(commands.libraryForget(entry.path), { reportErrors: false }).then((result) => {
        if (!result.ok) {
          this.requestError = result.error;
          this.paintStatus();
          forget.disabled = false;
        }
      });
    });
    row.append(open, forget);

    return row;
  }

  load(path) {
    return this.startPick(commands.organLoad(path));
  }

  emptyNote(text) {
    const empty = document.createElement("p");
    empty.className = "picker-empty";
    empty.textContent = text;
    return empty;
  }

  // ---- browse ------------------------------------------------------------
  //
  // Not snapshot-driven: this is this client's own directory listing,
  // fetched directly (browse doesn't flow through the state machinery)
  // and re-fetched on navigation, never on a poll tick.

  async browse(dir) {
    const request = this.browseRequest = (this.browseRequest ?? 0) + 1;
    this.browseBusy = true;
    this.renderBrowse();
    const query = dir ? `/api/browse?dir=${encodeURIComponent(dir)}` : "/api/browse";
    const { ok, data, error } = await localFetch(this.base, query, { json: true });
    if (request !== this.browseRequest) return;
    this.browseBusy = false;
    if (!ok) {
      this.browseError = error;
    } else {
      this.dir = data.dir;
      this.browseParent = data.parent;
      this.browseOrgans = data.organs ?? null;
      this.browseEntries = data.entries;
      this.browseError = null;
    }
    this.renderBrowse();
  }

  renderBrowse() {
    this.el.browseStatus.classList.toggle("hidden", !this.browseBusy);
    this.el.browseList.setAttribute("aria-busy", String(!!this.browseBusy));
    this.el.dir.textContent = this.dir ?? "";
    this.el.dir.title = this.dir ?? "";
    this.el.up.disabled = !this.browseParent;
    this.el.organs.classList.toggle("hidden", !this.browseOrgans);
    this.el.organs.disabled = this.dir === this.browseOrgans;

    this.el.browseError.classList.toggle("hidden", !this.browseError);
    this.el.browseError.textContent = this.browseError ?? "";

    this.el.browseList.replaceChildren();
    if (this.browseError) return;
    // The server lists every file a console picker might mean (organs
    // AND Scala tuning files); this picker means organs.
    const loadable = (name) =>
      ORGAN_FILTER.extensions.some((ext) => name.toLowerCase().endsWith(`.${ext.toLowerCase()}`));
    const entries = (this.browseEntries ?? []).filter(
      (entry) => entry.dir || loadable(entry.name)
    );
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
