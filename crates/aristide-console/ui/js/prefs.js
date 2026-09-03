// The Preferences dialog: the player's, never the organ's. The skin is
// local to this console (theme.js writes localStorage); sample memory
// is this machine's (the user config, through /api/prefs — the one
// thing here that talks to the server, and it never sends an organ
// command). That separation is the contract, not an accident. Organ
// facts — wiring, tuning, the room, structure — are edited on the
// console surface and land in the organ's own file (see editor.js).

import { commands } from "./api.js";
import { setText } from "./dom.js";
import { segmented } from "./theme.js";

const STREAMING = ["auto", "on", "off"];
const STREAMING_LABEL = { auto: "Auto", on: "Stream", off: "In RAM" };

const formatMiB = (mb) => (mb >= 1024 ? `${(mb / 1024).toFixed(mb % 1024 ? 1 : 0)} GiB` : `${mb} MiB`);

export class Preferences {
  constructor(root, send) {
    this.root = root;
    this.send = send ?? (() => {});
    this.snapshot = {};
    this.el = {
      modal: root.getElementById("prefs"),
      about: root.getElementById("about"),
      streaming: root.getElementById("streaming-segment"),
      streamingNote: root.getElementById("streaming-note"),
      budget: root.getElementById("ram-budget"),
      budgetNote: root.getElementById("budget-note"),
      bits: root.getElementById("bits-segment"),
      cache: root.getElementById("cache-segment"),
      status: root.getElementById("memory-status"),
      stale: root.getElementById("memory-stale"),
      reload: root.getElementById("memory-reload"),
    };
    this.wire();
    this.wireMemory();
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

  // ---- sample memory --------------------------------------------------

  wireMemory() {
    const { el } = this;
    if (!el.streaming) return;
    this.setStreaming = segmented(el.streaming, STREAMING, "auto", (v) => STREAMING_LABEL[v], (v) =>
      this.send(commands.prefsSamples({ streaming: v }))
    );
    this.setBits = segmented(el.bits, [16, 32], 16, (v) => `${v}-bit`, (v) =>
      this.send(commands.prefsSamples({ bits: v }))
    );
    this.setCache = segmented(el.cache, [true, false], true, (v) => (v ? "On" : "Off"), (v) =>
      this.send(commands.prefsSamples({ cache: v ? 1 : 0 }))
    );
    // The budget commits on change (Enter or blur), never per keystroke:
    // each commit is a write of the config file. Empty means the
    // default — half of physical RAM — and the field says so.
    el.budget.addEventListener("change", () => {
      const raw = el.budget.value.trim();
      if (raw === "") {
        this.send(commands.prefsSamples({ ram_budget_mb: "" }));
        return;
      }
      const mb = Math.round(Number(raw));
      if (!Number.isFinite(mb) || mb <= 0) {
        this.paintMemory();
        return;
      }
      this.send(commands.prefsSamples({ ram_budget_mb: mb }));
    });
    el.reload.addEventListener("click", () => {
      const setup = this.snapshot.setup;
      const paths = setup?.file ? [setup.file] : (setup?.sources ?? []).map((s) => s.path);
      if (paths.length === 0) return;
      this.close();
      this.send(commands.organReload(paths));
    });
  }

  update(snapshot) {
    this.snapshot = snapshot;
    this.paintMemory();
  }

  paintMemory() {
    const { el } = this;
    if (!el.streaming) return;
    const prefs = this.snapshot.prefs?.samples;
    if (!prefs) return;
    const ram = this.snapshot.prefs?.physical_ram_mb;
    const memory = this.snapshot.memory;

    this.setStreaming(prefs.streaming);
    this.setBits(prefs.bits);
    this.setCache(prefs.cache);
    // A field being typed into is the player's; the poll only fills
    // it when they are not.
    if (this.root.activeElement !== el.budget) {
      const want = prefs.ram_budget_mb == null ? "" : String(prefs.ram_budget_mb);
      if (el.budget.value !== want) el.budget.value = want;
    }
    const placeholder = ram ? String(Math.floor(ram / 2)) : "";
    if (el.budget.placeholder !== placeholder) el.budget.placeholder = placeholder;

    const budget = prefs.ram_budget_mb ?? (ram ? Math.floor(ram / 2) : null);
    const budgetText = budget ? formatMiB(budget) : "unknown";
    setText(
      el.streamingNote,
      {
        auto: `Tails stream only when a set would outgrow the budget (${budgetText}); smaller sets sit whole in RAM.`,
        on: "Every release tail plays from the disk; attacks, loops and the head of each tail stay resident.",
        off: "The whole set sits in RAM, as if streaming did not exist.",
      }[prefs.streaming] ?? ""
    );
    setText(
      el.budgetNote,
      ram
        ? `Leave it empty for half of this machine's ${formatMiB(ram)}. Only Auto reads it.`
        : "This machine's RAM is unknown, so Auto never streams unless a budget is set here."
    );

    if (memory) {
      const resident = `${formatMiB(memory.resident_mb)} resident`;
      const streamed =
        memory.streamed_mb > 0
          ? `, ${formatMiB(memory.streamed_mb)} streaming (${memory.streamed_samples} of ${memory.samples} samples)`
          : `, nothing streams (${memory.samples} samples)`;
      setText(el.status, `This organ: ${resident}${streamed}, ${memory.built_with.bits}-bit.`);
    } else {
      setText(el.status, "No organ loaded.");
    }

    const built = memory?.built_with;
    const stale =
      !!built &&
      (built.bits !== prefs.bits ||
        built.cache !== prefs.cache ||
        built.streaming !== prefs.streaming ||
        (built.ram_budget_mb ?? null) !== (prefs.ram_budget_mb ?? null));
    el.stale.classList.toggle("hidden", !stale);
    const busy = !!this.snapshot.loading;
    if (el.reload.disabled !== busy) el.reload.disabled = busy;
  }
}
