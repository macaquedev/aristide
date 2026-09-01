// The organ-structure editor: a Max-MSP-style unlockable patch, not a
// dialog. Locked, the console behaves exactly as it always has, except
// that a ctrl-drag still edits — that's the "reach through the lock"
// gesture the rest of this module exists to serve. Unlocked, plain
// drags do the same thing, panels move by their title bars, and
// double-clicking empty canvas adds to the organ right there.
//
// This owns the editing chrome (padlock, drawer, bin, hint, the add
// popovers, the rebuild status strip) and decorates the DOM Console
// already built — it never builds jambs or keyboards itself.
// `decorateConsole(snapshot)` is called by Console right after every
// structural rebuild (see console.js's `decorate` hook); `update(snapshot)`
// is called on every poll, the same as the other panels.

import { commands } from "./api.js";
import {
  formatFootage,
  keyName,
  keySpellings,
  parseFootage,
  parseKeyName,
  splitFootageName,
  tidyKeyName,
} from "./pitch.js";
import {
  buildManualInputs,
  buildControlsList,
  pistonRow,
  keyboardNote,
  PITCH_ACTIONS,
} from "./wiring.js";

// What the native add-source dialog offers. Narrower than the picker's
// filter: a source must be a sample set — the server refuses another
// organ file — so Aristide's own .toml composites are left out.
const SET_FILTER = {
  name: "Sample sets (GrandOrgue, Hauptwerk)",
  extensions: ["organ", "Organ_Hauptwerk_xml"],
};

/// The keyboard context menu's "Change type" radio group, in the order
/// they're offered — the same vocabulary the add menu and the server's
/// `kind=` param share.
const KEYBOARD_KINDS = [
  ["manual", "Manual"],
  ["pedal", "Pedalboard"],
  ["microtonal", "Microtonal keyboard"],
];

/// Swallows the click a suppressed drag would otherwise leave behind —
/// a drag that crossed the threshold must not also toggle the drawknob
/// (or fire whatever else the element's own click listener does).
function suppressClick(event) {
  event.preventDefault();
  event.stopImmediatePropagation();
}

/// Anything with behavior of its own — a panel drag must never start on
/// these, or a drawknob could not be clicked and a key could not play.
const INTERACTIVE = ".knob, .key, .cheek, .rocker, .shoe, button, input, select, textarea";

/// What the tuning popover's "Recorded: …" line calls each of the
/// temperaments `snapshot.home` can *identify* by name — `null` there
/// means an unequal temperament that doesn't match any of these
/// tables, not an absence of measurement (that's `home` itself null).
const HOME_TEMPERAMENT_NAMES = {
  equal: "equal",
  werckmeister3: "Werckmeister III",
  kirnberger3: "Kirnberger III",
  meantone4: "¼-comma meantone",
  pythagorean: "Pythagorean",
};

/// The pitch a sample set was actually recorded at for MIDI key `key`,
/// per the snapshot's `home` block: `home.a4_hz` is the measured A4,
/// `home.offsets_cents` the other eleven pitch classes' deviation from
/// equal temperament under that A4 (C = index 0). Used only to decide
/// whether the "as recorded" reference button has anything to do —
/// the popover otherwise just echoes `home.a4_hz` verbatim.
function homeHz(home, key) {
  const equalCents = 1200 * Math.log2(home.a4_hz / 440);
  const classCents = home.offsets_cents[key % 12];
  return 440 * 2 ** ((key - 69) / 12 + (equalCents + classCents) / 1200);
}

export class Editor {
  constructor(root, base, send) {
    this.root = root;
    this.base = base;
    this.send = send;
    this.unlocked = false;
    this.drawerOpen = false;
    this.drag = null; // the live structural drag, if any — see startDrag()
    this.panelDrag = null; // the live panel move, if any
    this.lastSnapshot = null;
    this.autoUnlockedFor = null; // organ name already auto-unlocked once
    this.offerings = null;
    this.offeringsFile = null; // setup.file the cached offerings were fetched for
    this.renamingManual = null; // manual idx whose cheek is a rename input
    this.pendingRemove = null; // {kind: "manual"|"enclosure", ...} awaiting confirm
    this.pendingPlace = null; // {name, x, y}: place this manual's panels once it lands
    this.addAnchor = null; // where the add popover was opened, in px
    this.addKind = "manual"; // "manual" | "pedal" | "microtonal" — the add-manual form's target
    // The tuning popover's open scope, or null: {kind:"organ"} |
    // {kind:"division", idx} | {kind:"source", alias} |
    // {kind:"stop", id} | {kind:"rank", stop, rank}.
    this.tuningScope = null;
    this.tuningResolved = null; // the tuning object the fields currently show (see syncTuningForm)
    this.hexManual = null; // manual idx the hex-layout popover is open for, or null
    this.tremOpen = null; // trem idx the shape popover is open for, or null
    this.stopOpen = null; // stop id the stop-editor popover is open for, or null
    this.stopSrcOpen = false; // the stop popover's own source-picker subview is showing
    this.stopLabelSync = null; // {id, base, relabel}: the pending rename-offer's answer
    this.stopLabelSyncDeclined = new Set(); // stop ids whose offer was declined — don't nag again

    this.couplerOpen = null; // coupler idx the route-editor popover is open for, or null
    // The open coupler's routes, edited locally and auto-applied:
    // every change posts the whole array through a coalescing queue
    // (see scheduleCouplerApply) — each apply rebuilds the organ, so
    // edits made mid-rebuild wait it out and only the newest state is
    // ever sent. Once an apply settles, the server's echo folds back
    // into the working copy (syncCouplerForm), never under the pointer.
    this.couplerRoutes = null;
    this.couplerPending = null; // the newest unapplied {idx, routes}, if any
    this.couplerApplying = false; // an apply pump is running
    this.couplerResync = false; // a settled apply awaits folding back in
    this.addCouplerNamed = false; // the add form's name field was typed in
    this.pendingLink = null; // {onYes, onNo}: the duplicate-coupler dialog's answers
    this.tuningBrowseKind = null; // "scale" | "keymap" | null — the tuning form's own file browser
    this.tuningBrowseDir = null;
    this.tuningBrowseParent = null;
    this.tuningBrowseEntries = null;
    this.tuningBrowseError = null;
    this.addBrowseDir = null;
    this.addBrowseParent = null;
    this.addBrowseEntries = null;
    this.addBrowseError = null;
    this._lockNoteTimer = null;
    this.midiManual = null; // manual idx the MIDI-input popover is open for, or null
    this.midiSignature = null; // rebuild the popover's rows only when these change
    this.compassManual = null; // manual idx the compass popover is open for, or null
    this.compassSignature = null;
    this.roomOpen = false; // the Room & noises popover
    this.roomDragging = new Set(); // slider keys mid-drag: the snapshot keeps its hands off
    this.bindingsOpen = false; // the flat Bindings popover
    this.bindingsSignature = null;
    this.saveOpen = false; // the save-as popover
    this.saveAsOpen = false; // the save-as dialog (a copy under a new name)
    this.saveAsFor = null; // the organ it was opened for
    this.saveAsPending = null; // the refused command to send again after saving
    this.savePromptedFor = null; // organ name already auto-prompted to save, once
    // A quick-bind in flight: Listen was pressed on a piston row, and
    // once the learned trigger lands at `slot` the action (and target
    // manual, for pitch actions) is bound over the learned default.
    this.quickBind = null; // {action, manual?, slot}

    this.el = {
      lock: root.getElementById("editor-lock"),
      lockGlyph: root.getElementById("editor-lock-glyph"),
      lockNote: root.getElementById("editor-lock-note"),
      hint: root.getElementById("editor-hint"),
      status: root.getElementById("editor-status"),
      statusText: root.getElementById("editor-status-text"),
      error: root.getElementById("editor-error"),
      canvas: root.getElementById("console-canvas"),
      emptyCard: root.getElementById("organ-empty-card"),
      drawerTab: root.getElementById("editor-drawer-tab"),
      drawer: root.getElementById("editor-drawer"),
      drawerClose: root.getElementById("editor-drawer-close"),
      offerings: root.getElementById("editor-offerings"),
      bin: root.getElementById("editor-bin"),
      removeConfirm: root.getElementById("editor-remove-confirm"),
      removeConfirmText: root.getElementById("editor-remove-confirm-text"),
      removeConfirmYes: root.getElementById("editor-remove-confirm-yes"),
      removeConfirmNo: root.getElementById("editor-remove-confirm-no"),
      add: root.getElementById("editor-add"),
      addMenu: root.getElementById("editor-add-menu"),
      addManual: root.getElementById("editor-add-manual"),
      addPedal: root.getElementById("editor-add-pedal"),
      addMicrotonal: root.getElementById("editor-add-microtonal"),
      addEnc: root.getElementById("editor-add-enc"),
      addSource: root.getElementById("editor-add-source"),
      addManualForm: root.getElementById("editor-add-manual-form"),
      addManualName: root.getElementById("editor-add-manual-name"),
      addManualLow: root.getElementById("editor-add-manual-low"),
      addManualHigh: root.getElementById("editor-add-manual-high"),
      addManualCancel: root.getElementById("editor-add-manual-cancel"),
      addEncForm: root.getElementById("editor-add-enc-form"),
      addEncName: root.getElementById("editor-add-enc-name"),
      addEncCancel: root.getElementById("editor-add-enc-cancel"),
      addSourceForm: root.getElementById("editor-add-source-form"),
      addSourcePath: root.getElementById("editor-add-source-path"),
      addSourceAdd: root.getElementById("editor-add-source-add"),
      addSourceCancel: root.getElementById("editor-add-source-cancel"),
      addBrowseUp: root.getElementById("editor-add-browse-up"),
      addBrowseDir: root.getElementById("editor-add-browse-dir"),
      addBrowseError: root.getElementById("editor-add-browse-error"),
      addBrowseList: root.getElementById("editor-add-browse-list"),
      divisionMenu: root.getElementById("editor-division-menu"),
      keyboardMenu: root.getElementById("editor-keyboard-menu"),
      tuning: root.getElementById("editor-tuning"),
      tuningForm: root.getElementById("editor-tuning-form"),
      tuningTitle: root.getElementById("editor-tuning-title"),
      tuningFollowRow: root.getElementById("editor-tuning-follow-row"),
      tuningFollow: root.getElementById("editor-tuning-follow"),
      tuningResolvedPrimary: root.getElementById("editor-tuning-resolved-primary"),
      tuningResolvedChain: root.getElementById("editor-tuning-resolved-chain"),
      tuningScaleRow: root.getElementById("editor-tuning-scale-row"),
      tuningScalePick: root.getElementById("editor-tuning-scale-pick"),
      tuningScaleActive: root.getElementById("editor-tuning-scale-active"),
      tuningScaleName: root.getElementById("editor-tuning-scale-name"),
      tuningScaleClear: root.getElementById("editor-tuning-scale-clear"),
      tuningKeymapRow: root.getElementById("editor-tuning-keymap-row"),
      tuningKeymapName: root.getElementById("editor-tuning-keymap-name"),
      tuningKeymapPick: root.getElementById("editor-tuning-keymap-pick"),
      tuningKeymapClear: root.getElementById("editor-tuning-keymap-clear"),
      tuningTemperamentRow: root.getElementById("editor-tuning-temperament-row"),
      tuningTemperament: root.getElementById("editor-tuning-temperament"),
      tuningHome: root.getElementById("editor-tuning-home"),
      tuningPipesRow: root.getElementById("editor-tuning-pipes-row"),
      tuningPipes: root.getElementById("editor-tuning-pipes"),
      tuningEdoRow: root.getElementById("editor-tuning-edo-row"),
      tuningEdo: root.getElementById("editor-tuning-edo"),
      tuningRefRow: root.getElementById("editor-tuning-ref-row"),
      tuningRefKey: root.getElementById("editor-tuning-ref-key"),
      tuningRefHz: root.getElementById("editor-tuning-ref-hz"),
      tuningRefStepper: root.getElementById("editor-tuning-ref-stepper"),
      tuningRefHome: root.getElementById("editor-tuning-ref-home"),
      pitchNames: root.getElementById("pitch-names"),
      tuningTransposeRow: root.getElementById("editor-tuning-transpose-row"),
      tuningTranspose: root.getElementById("editor-tuning-transpose"),
      tuningError: root.getElementById("editor-tuning-error"),
      tuningClose: root.getElementById("editor-tuning-close"),
      tuningBrowse: root.getElementById("editor-tuning-browse"),
      tuningBrowseTitle: root.getElementById("editor-tuning-browse-title"),
      tuningBrowseUp: root.getElementById("editor-tuning-browse-up"),
      tuningBrowseDir: root.getElementById("editor-tuning-browse-dir"),
      tuningBrowseError: root.getElementById("editor-tuning-browse-error"),
      tuningBrowseList: root.getElementById("editor-tuning-browse-list"),
      tuningBrowseCancel: root.getElementById("editor-tuning-browse-cancel"),
      trem: root.getElementById("editor-trem"),
      tremTitle: root.getElementById("editor-trem-title"),
      tremRate: root.getElementById("editor-trem-rate"),
      tremDepth: root.getElementById("editor-trem-depth"),
      tremRamp: root.getElementById("editor-trem-ramp"),
      tremWobble: root.getElementById("editor-trem-wobble"),
      tremError: root.getElementById("editor-trem-error"),
      tremClose: root.getElementById("editor-trem-close"),
      stop: root.getElementById("editor-stop"),
      stopForm: root.getElementById("editor-stop-form"),
      stopTitle: root.getElementById("editor-stop-title"),
      stopReset: root.getElementById("editor-stop-reset"),
      stopName: root.getElementById("editor-stop-name"),
      stopFootage: root.getElementById("editor-stop-footage"),
      stopCents: root.getElementById("editor-stop-cents"),
      stopGain: root.getElementById("editor-stop-gain"),
      stopSrc: root.getElementById("editor-stop-src"),
      stopSrcChange: root.getElementById("editor-stop-src-change"),
      stopError: root.getElementById("editor-stop-error"),
      stopClose: root.getElementById("editor-stop-close"),
      stopSrcView: root.getElementById("editor-stop-src-view"),
      stopSrcList: root.getElementById("editor-stop-src-list"),
      stopSrcCancel: root.getElementById("editor-stop-src-cancel"),
      stopLabelMode: root.getElementById("editor-stop-label-mode"),
      stopLabelText: root.getElementById("editor-stop-label-text"),
      stopLabelSync: root.getElementById("editor-stop-label-sync"),
      stopLabelSyncText: root.getElementById("editor-stop-label-sync-text"),
      stopLabelSyncYes: root.getElementById("editor-stop-label-sync-yes"),
      stopLabelSyncNo: root.getElementById("editor-stop-label-sync-no"),
      stopOwnPipes: root.getElementById("editor-stop-own-pipes"),
      stopTuningSummary: root.getElementById("editor-stop-tuning-summary"),
      stopTuningEdit: root.getElementById("editor-stop-tuning-edit"),
      stopRanks: root.getElementById("editor-stop-ranks"),
      stopDelete: root.getElementById("editor-stop-delete"),
      coupler: root.getElementById("editor-coupler"),
      couplerForm: root.getElementById("editor-coupler-form"),
      couplerTitle: root.getElementById("editor-coupler-title"),
      couplerName: root.getElementById("editor-coupler-name"),
      couplerRoutesBox: root.getElementById("editor-coupler-routes"),
      couplerRouteAdd: root.getElementById("editor-coupler-route-add"),
      couplerKeys: root.getElementById("editor-coupler-keys"),
      couplerLinkedBox: root.getElementById("editor-coupler-linked-box"),
      couplerDelete: root.getElementById("editor-coupler-delete"),
      couplerError: root.getElementById("editor-coupler-error"),
      couplerClose: root.getElementById("editor-coupler-close"),
      couplersMenu: root.getElementById("editor-couplers-menu"),
      coupledKeys: root.getElementById("editor-coupled-keys"),
      linkConfirm: root.getElementById("editor-link-confirm"),
      linkConfirmText: root.getElementById("editor-link-confirm-text"),
      linkConfirmYes: root.getElementById("editor-link-confirm-yes"),
      linkConfirmNo: root.getElementById("editor-link-confirm-no"),
      addCoupler: root.getElementById("editor-add-coupler"),
      addCouplerForm: root.getElementById("editor-add-coupler-form"),
      addCouplerName: root.getElementById("editor-add-coupler-name"),
      addCouplerSounds: root.getElementById("editor-add-coupler-sounds"),
      addCouplerOn: root.getElementById("editor-add-coupler-on"),
      addCouplerAt: root.getElementById("editor-add-coupler-at"),
      addCouplerCancel: root.getElementById("editor-add-coupler-cancel"),
      midi: root.getElementById("editor-midi"),
      midiTitle: root.getElementById("editor-midi-title"),
      midiRescan: root.getElementById("editor-midi-rescan"),
      midiInputs: root.getElementById("editor-midi-inputs"),
      midiPistons: root.getElementById("editor-midi-pistons"),
      midiPorts: root.getElementById("editor-midi-ports"),
      midiClose: root.getElementById("editor-midi-close"),
      compass: root.getElementById("editor-compass"),
      compassTitle: root.getElementById("editor-compass-title"),
      compassRow: root.getElementById("editor-compass-row"),
      compassError: root.getElementById("editor-compass-error"),
      compassClose: root.getElementById("editor-compass-close"),
      room: root.getElementById("editor-room"),
      roomReverbRow: root.getElementById("editor-room-reverb-row"),
      roomReverb: root.getElementById("editor-room-reverb"),
      roomNoisesRow: root.getElementById("editor-room-noises-row"),
      roomNoisesOn: root.getElementById("editor-room-noises-on"),
      roomNoisesVol: root.getElementById("editor-room-noises-vol"),
      roomClose: root.getElementById("editor-room-close"),
      bindings: root.getElementById("editor-bindings"),
      bindingsList: root.getElementById("editor-bindings-list"),
      bindingsAdd: root.getElementById("editor-bindings-add"),
      bindingsKeyboard: root.getElementById("editor-bindings-keyboard"),
      bindingsClose: root.getElementById("editor-bindings-close"),
      save: root.getElementById("editor-save"),
      saveNote: root.getElementById("editor-save-note"),
      savePath: root.getElementById("editor-save-path"),
      saveBtn: root.getElementById("editor-save-btn"),
      saveError: root.getElementById("editor-save-error"),
      saveClose: root.getElementById("editor-save-close"),
      saveAs: root.getElementById("save-as"),
      saveAsNote: root.getElementById("save-as-note"),
      saveAsName: root.getElementById("save-as-name"),
      saveAsBtn: root.getElementById("save-as-btn"),
      saveAsCancel: root.getElementById("save-as-cancel"),
      saveAsError: root.getElementById("save-as-error"),
      stopPistons: root.getElementById("editor-stop-pistons"),
      couplerPistons: root.getElementById("editor-coupler-pistons"),
      addCouplerRestore: root.getElementById("editor-add-coupler-restore"),
      hex: root.getElementById("editor-hex"),
      hexTitle: root.getElementById("editor-hex-title"),
      hexReset: root.getElementById("editor-hex-reset"),
      hexRight: root.getElementById("editor-hex-right"),
      hexUpright: root.getElementById("editor-hex-upright"),
      hexRows: root.getElementById("editor-hex-rows"),
      hexCols: root.getElementById("editor-hex-cols"),
      hexAnchor: root.getElementById("editor-hex-anchor"),
      hexError: root.getElementById("editor-hex-error"),
      hexClose: root.getElementById("editor-hex-close"),
    };

    this.wireLock();
    this.wireDrawer();
    this.wireRemoveConfirm();
    this.wireLinkConfirm();
    this.wireCouplersMenu();
    this.wireAdd();
    this.wireTuningForm();
    this.wireHexForm();
    this.wireTremForm();
    this.wireStopForm();
    this.wireCouplerForm();
    this.wireMidiForm();
    this.wireCompassForm();
    this.wireRoomForm();
    this.wireBindingsForm();
    this.wireSaveForm();
    this.wireSaveAsForm();
    this.wireCanvas();
  }

  // ---- the padlock ---------------------------------------------------------

  wireLock() {
    this.el.lock.addEventListener("click", () => this.togglePadlock());
    window.addEventListener("keydown", (event) => {
      if (event.key.toLowerCase() !== "e" || !(event.ctrlKey || event.metaKey)) return;
      if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) return;
      event.preventDefault();
      this.togglePadlock();
    });
  }

  togglePadlock() {
    if (this.unlocked) {
      this.lock();
      return;
    }
    // An ad-hoc combination has no file for a structural edit to write
    // into — the server would 400 on the first one. Say so instead of
    // unlocking into affordances that can't work yet.
    if (this.lastSnapshot?.setup?.implicit) {
      this.showLockNote(
        "This organ hasn't been saved as a file yet — save the combination " +
          "first (organ menu), then its structure can be edited here."
      );
      return;
    }
    this.unlock();
  }

  /// The empty-organ card's "Unlock and build". Unlocking alone would
  /// be a no-op there — an empty organ auto-unlocks the moment it
  /// loads (see `update`) — so the button delivers the build half too:
  /// the add menu opens where the click landed, the same menu the
  /// double-click gesture reaches.
  beginBuild(x, y) {
    if (!this.unlocked) this.unlock();
    const rect = this.el.canvas.getBoundingClientRect();
    this.openAddMenu(x ?? rect.left + rect.width / 2, y ?? rect.top + rect.height / 2);
  }

  unlock() {
    this.unlocked = true;
    this.hideLockNote();
    this.root.body.classList.add("editing");
    this.el.lock.classList.add("on");
    this.el.lock.setAttribute("aria-pressed", "true");
    this.el.lock.setAttribute("aria-label", "Lock editing");
    this.el.lock.dataset.tip = "Lock editing (Ctrl+E)";
    this.el.lockGlyph.innerHTML = "&#128275;"; // open padlock
    this.el.hint.classList.remove("hidden");
    this.el.drawerTab.classList.remove("hidden");
  }

  lock() {
    this.unlocked = false;
    this.root.body.classList.remove("editing");
    this.el.lock.classList.remove("on");
    this.el.lock.setAttribute("aria-pressed", "false");
    this.el.lock.setAttribute("aria-label", "Unlock editing");
    this.el.lock.dataset.tip = "Unlock editing (Ctrl+E)";
    this.el.lockGlyph.innerHTML = "&#128274;"; // closed padlock
    this.el.hint.classList.add("hidden");
    this.el.drawerTab.classList.add("hidden");
    this.closeDrawer();
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeTuningForm();
    this.closeHexForm();
    this.closeTremForm();
    this.closeStopForm();
    this.closeCouplerForm();
    this.closeCouplersMenu();
  }

  // A double-click on a locked canvas is someone reaching for the add
  // gesture — silence would read as "there is no such gesture", so the
  // padlock answers instead.
  nudgeUnlock() {
    this.showLockNote(
      "The console is locked — click the padlock (Ctrl+E) to edit, " +
        "or hold Ctrl to reach through it."
    );
  }

  showLockNote(text) {
    this.el.lockNote.textContent = text;
    this.el.lockNote.classList.remove("hidden");
    clearTimeout(this._lockNoteTimer);
    this._lockNoteTimer = setTimeout(() => this.hideLockNote(), 6000);
  }

  hideLockNote() {
    this.el.lockNote.classList.add("hidden");
  }

  // ---- per-poll state --------------------------------------------------

  /// Called on every snapshot, structural rebuild or not — the rebuild
  /// status, the empty-organ auto-unlock, queued edits and offerings
  /// staleness all need to track fields (`loading`, `setup.file`) that
  /// don't necessarily change Console's own structural signature.
  update(snapshot) {
    this.lastSnapshot = snapshot;

    const empty = !!snapshot.organ && !snapshot.stops?.length && !snapshot.manuals?.length;
    if (empty && this.autoUnlockedFor !== snapshot.organ) {
      this.autoUnlockedFor = snapshot.organ;
      this.unlock();
    }

    const showStatus = !!snapshot.organ && !!snapshot.loading;
    this.el.status.classList.toggle("hidden", !showStatus);
    this.el.statusText.textContent = snapshot.loading ?? "";

    // A load that failed with the picker closed (an organ picked from
    // the menu's Recent list) would otherwise fail into silence: the
    // picker shows load_error only while it is up, so the error strip
    // carries it here. Warnings ride along — an organ whose file lines
    // were healed over loads emptier than the file intends, and that
    // must say so where the player is looking. Only transitions matter
    // — repainting on every poll would clobber the strip's own
    // transient command errors.
    const warnings = snapshot.load_warnings ?? [];
    const loadError =
      snapshot.load_error ??
      (warnings.length
        ? `the organ loaded, but ${warnings.length} line${
            warnings.length === 1 ? "" : "s"
          } of its file did not resolve — e.g. ${warnings[0]}`
        : null);
    if (loadError !== this.lastLoadError) {
      this.lastLoadError = loadError;
      loadError ? this.showError(loadError) : this.hideError();
    }

    const file = snapshot.setup?.file ?? null;
    if (file !== this.offeringsFile) {
      this.offeringsFile = file;
      this.offerings = null;
      if (this.drawerOpen) this.fetchOfferings();
    }

    this.pumpQuickBind(snapshot);

    if (this.tuningScope != null) this.syncTuningForm();
    if (this.hexManual != null) this.syncHexForm();
    if (this.tremOpen != null) this.syncTremForm();
    if (this.stopOpen != null) this.syncStopForm();
    if (this.couplerOpen != null) this.syncCouplerForm();
    if (!this.el.couplersMenu.classList.contains("hidden")) this.syncCouplersMenu();
    if (this.midiManual != null) this.syncMidiForm();
    if (this.compassManual != null) this.syncCompassForm();
    if (this.roomOpen) this.syncRoomForm();
    if (this.bindingsOpen) this.syncBindingsForm();
    if (this.saveOpen) this.syncSaveForm();
    if (this.saveAsOpen) this.syncSaveAsForm();
    this.refreshSilentBadges();
    this.refreshTuningChips();

    // An organ combined ad hoc on the command line has nobody to ask
    // "keep this?" but the player — offer the save-as popover once, and
    // never fight them for it again.
    if (
      snapshot.setup?.implicit &&
      snapshot.organ &&
      this.savePromptedFor !== snapshot.organ &&
      !snapshot.loading
    ) {
      this.savePromptedFor = snapshot.organ;
      this.openSaveForm();
    }
  }

  /// The second half of a piston row's Listen: the server learned a
  /// trigger into `slot` (as a default-action binding), and now the
  /// action the row stands for is bound over it. Cleared whenever the
  /// learn was cancelled or stolen by another Listen.
  pumpQuickBind(snapshot) {
    const quick = this.quickBind;
    if (!quick) return;
    const learning = snapshot.control_learning ?? null;
    if (learning === quick.slot) return; // still waiting for the press
    const landed = (snapshot.controls ?? []).find(
      (c) => c.slot === quick.slot && c.trigger
    );
    this.quickBind = null;
    if (learning == null && landed) {
      const fields = quick.manual ? { manual: quick.manual } : {};
      this.send(commands.controlBind(quick.slot, quick.action, fields));
    }
  }

  /// Starts (or cancels) a quick-bind from a piston row: learn a fresh
  /// trigger at the end of the list, then point it at `action`.
  quickBindListen(action, manual, cancelling) {
    if (cancelling) {
      this.quickBind = null;
      this.send(commands.controlLearn(null));
      return;
    }
    const slot = (this.lastSnapshot?.controls ?? []).length;
    this.quickBind = { action, manual: manual ?? null, slot };
    this.send(commands.controlLearn(slot));
  }

  /// Called by Console right after every structural rebuild (see its
  /// `decorate` hook) — wires drag sources, drop targets and the
  /// editing chrome onto the DOM it just built. Nothing here duplicates
  /// Console's own rendering; it only adds listeners and small controls.
  decorateConsole(snapshot) {
    this.lastSnapshot = snapshot;
    const empty = !!snapshot.organ && !snapshot.stops.length && !snapshot.manuals.length;
    if (empty) return; // the empty card has its own single button, wired by Console
    this.tagManualTargets();
    this.wireStopDrags(snapshot);
    this.wireStopContextMenus();
    this.wireCheekDrags(snapshot);
    this.wireShoeDrags(snapshot);
    this.wireTremKnob();
    this.wireCheekRename();
    this.wireKeyboardContextMenu();
    this.wireCouplerContextMenus();
    this.wireCouplerDrags();
    this.wireCouplersPanel();
    this.wirePanelMoves(snapshot);
    this.wirePanelResize();
    this.addDivisionButtons(snapshot);
    this.placePending(snapshot);
    this.refreshSilentBadges();
    this.refreshTuningChips();
  }

  /// A keyboard whose manual has no MIDI input wears a quiet badge —
  /// silence is the honest default for an unwired organ, but it looks
  /// like a fault unless the console says so where the player is
  /// looking. Clicking the badge opens the MIDI popover right there;
  /// it isn't gated by the padlock, for the same reason the old
  /// Preferences dialog wasn't — wiring is the first thing a player
  /// does, not an act of organ building. Runs on every poll (wiring
  /// isn't structural, so decorate alone would miss changes).
  refreshSilentBadges() {
    const midiManuals = this.lastSnapshot?.midi?.manuals ?? [];
    for (const board of this.root.querySelectorAll(".keyboard[data-manual]")) {
      const idx = Number(board.dataset.manual);
      const entry = midiManuals.find((m) => m.idx === idx);
      const silent = !!entry && !entry.inputs.length;
      let badge = board.querySelector(".kb-silent");
      if (!silent) {
        badge?.remove();
        continue;
      }
      if (!badge) {
        badge = document.createElement("button");
        badge.className = "kb-silent";
        badge.textContent = "silent — no input";
        badge.title = "This keyboard has no MIDI input yet — click to give it one";
        badge.addEventListener("click", (event) => {
          event.stopPropagation();
          const rect = badge.getBoundingClientRect();
          this.openMidiForm(idx, rect.left, rect.bottom + 6);
        });
        board.append(badge);
      }
    }
  }

  /// Organ facts too, the same "visible locked or not" contract as
  /// refreshSilentBadges — a keyboard whose division plays a tuning
  /// apart from the instrument's wears a small chip naming it; a stop
  /// whose tuning is pinned, its own, or has an own-tuning rank wears a
  /// dot. Neither is editing chrome, so both stay independent of
  /// body.editing's panel-chrome.
  refreshTuningChips() {
    const snap = this.lastSnapshot;
    if (!snap) return;

    for (const board of this.root.querySelectorAll(".keyboard[data-manual]")) {
      const idx = Number(board.dataset.manual);
      const own = (snap.manual_tuning ?? []).find((t) => t.idx === idx);
      let chip = board.querySelector(".kb-tuning-chip");
      if (!own) {
        chip?.remove();
        continue;
      }
      if (!chip) {
        chip = document.createElement("button");
        chip.type = "button";
        chip.className = "kb-tuning-chip";
        chip.addEventListener("click", (event) => {
          event.stopPropagation();
          const rect = chip.getBoundingClientRect();
          this.openTuningForm({ kind: "division", idx }, rect.left, rect.bottom + 6);
        });
        board.append(chip);
      }
      chip.textContent = this.tuningSummary(own);
      chip.title = "This keyboard's division plays its own tuning — click to edit";
    }

    for (const stop of snap.stops ?? []) {
      const knob = this.root.querySelector(`.knob[data-key="stop-${stop.id}"]`);
      if (!knob) continue;
      const info = stop.tuning ?? { scope: "organ", follow: "auto" };
      const marked = info.follow !== "auto" || (stop.ranks ?? []).some((r) => r.own);
      let dot = knob.querySelector(".stop-tuning-dot");
      if (!marked) {
        dot?.remove();
        continue;
      }
      if (!dot) {
        dot = document.createElement("span");
        dot.className = "stop-tuning-dot";
        knob.append(dot);
      }
      dot.title = this.stopTuningLine(stop);
    }
  }

  /// Every keyboard and every jamb division carries its manual index in
  /// the DOM, so both are drop targets — including empty divisions,
  /// which is precisely where a new manual's first stop goes.
  tagManualTargets() {
    for (const board of this.root.querySelectorAll(".keyboard[data-manual]")) {
      board.dataset.dropManual = board.dataset.manual;
    }
    for (const division of this.root.querySelectorAll(".division[data-division]")) {
      division.dataset.dropManual = division.dataset.division;
    }
  }

  wireStopDrags() {
    for (const knob of this.root.querySelectorAll('.knob[data-key^="stop-"]')) {
      const id = Number(knob.dataset.key.slice("stop-".length));
      knob.title =
        "Drag to reorder, move, or enclose this stop (ctrl reaches through the lock) — " +
        "right-click to edit it.";
      this.wireDragSource(knob, () => {
        const stop = this.lastSnapshot?.stops.find((s) => s.id === id);
        if (!stop) return null;
        return { kind: "stop", payload: { id: stop.id, midx: stop.midx, name: stop.name }, label: stop.name };
      });
    }
  }

  /// Right-click any stop drawknob in edit mode opens its editor — name,
  /// voicing, and source — the same reach-through-the-lock contract as
  /// the keyboard and tremulant context menus.
  wireStopContextMenus() {
    for (const knob of this.root.querySelectorAll('.knob[data-key^="stop-"]')) {
      const id = Number(knob.dataset.key.slice("stop-".length));
      knob.addEventListener("contextmenu", (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (!this.unlocked && !event.ctrlKey) {
          this.nudgeUnlock();
          return;
        }
        this.openStopForm(id, event.clientX, event.clientY);
      });
    }
  }

  wireCheekDrags() {
    for (const board of this.root.querySelectorAll(".keyboard[data-manual]")) {
      const idx = Number(board.dataset.manual);
      const cheek = board.querySelector(".cheek");
      if (!cheek) continue;
      cheek.title =
        "Ctrl-drag to reorder or remove this manual — unlock to drag it plain. Double-click to rename.";
      this.wireDragSource(cheek, () => {
        const manual = this.lastSnapshot?.manuals.find((m) => m.idx === idx);
        if (!manual) return null;
        const stopCount = (this.lastSnapshot?.stops ?? []).filter((s) => s.midx === idx).length;
        return { kind: "manual", payload: { idx, name: manual.name, stopCount }, label: manual.name };
      });
    }
  }

  wireShoeDrags() {
    for (const shoe of this.root.querySelectorAll(".shoe[data-enclosure]")) {
      const idx = Number(shoe.dataset.enclosure);
      const label = shoe.querySelector(".shoe-label");
      if (!label) continue;
      label.title = "Ctrl-drag to the bin to remove this swell box — its stops stay, unenclosed.";
      this.wireDragSource(label, () => {
        const enclosure = this.lastSnapshot?.enclosures.find((e) => e.idx === idx);
        if (!enclosure) return null;
        const stopCount = (this.lastSnapshot?.stops ?? []).filter((s) => (s.enc ?? []).includes(idx)).length;
        return { kind: "enclosure", payload: { name: enclosure.name, stopCount }, label: enclosure.name };
      });
    }
  }

  wireCheekRename() {
    for (const board of this.root.querySelectorAll(".keyboard[data-manual]")) {
      const idx = Number(board.dataset.manual);
      const cheek = board.querySelector(".cheek");
      if (!cheek) continue;
      cheek.addEventListener("dblclick", (event) => {
        if (!this.unlocked && !event.ctrlKey) return;
        this.startManualRename(idx);
      });
    }
  }

  /// Right-click a keyboard panel — locked, it's the same nudge as any
  /// other reach at editing; unlocked, it opens the kind/tuning menu
  /// right there, the same popover idiom as the division "+".
  wireKeyboardContextMenu() {
    for (const board of this.root.querySelectorAll(".keyboard[data-manual]")) {
      const idx = Number(board.dataset.manual);
      board.addEventListener("contextmenu", (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (!this.unlocked) {
          this.nudgeUnlock();
          return;
        }
        this.openKeyboardMenu(idx, event.clientX, event.clientY);
      });
    }
  }

  /// Right-click a coupler — its rail rocker or its jamb drawknob —
  /// in edit mode opens its route editor, the same reach-through-the-
  /// lock contract as the stop and keyboard context menus. The cancel
  /// piston's `data-key` is "cancel", not "coupler-N", so the selector
  /// already leaves it out.
  wireCouplerContextMenus() {
    for (const control of this.root.querySelectorAll('[data-key^="coupler-"]')) {
      const idx = Number(control.dataset.key.slice("coupler-".length));
      control.title =
        "Drag to a jamb to seat this coupler among the stops (ctrl reaches " +
        "through the lock) — right-click to edit it.";
      control.addEventListener("contextmenu", (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (!this.unlocked && !event.ctrlKey) {
          this.nudgeUnlock();
          return;
        }
        this.openCouplerForm(idx, event.clientX, event.clientY);
      });
    }
  }

  /// Couplers drag like stops: into a jamb to seat them among the
  /// stops, back to the rail to unseat, to the bin to delete.
  wireCouplerDrags() {
    for (const control of this.root.querySelectorAll('[data-key^="coupler-"]')) {
      const idx = Number(control.dataset.key.slice("coupler-".length));
      this.wireDragSource(control, () => {
        const coupler = this.lastSnapshot?.couplers.find((c) => c.idx === idx);
        if (!coupler) return null;
        return {
          kind: "coupler",
          payload: { idx, midx: coupler.midx ?? null, name: coupler.name },
          label: coupler.name,
        };
      });
    }
  }

  /// The coupler rail's own chrome: a + that adds a coupler right
  /// there, and a right-click for this organ's coupler-wide settings
  /// (organ facts live on the console, never in Preferences).
  wireCouplersPanel() {
    const panel = this.el.canvas.querySelector('.panel[data-panel="couplers"]');
    const rail = panel?.querySelector(".coupler-rail");
    if (!panel || !rail) return;
    const add = document.createElement("button");
    add.type = "button";
    add.className = "division-add";
    add.textContent = "+";
    add.setAttribute("aria-label", "Add a coupler");
    add.title = "Add a coupler.";
    add.addEventListener("click", (event) => {
      event.stopPropagation();
      const rect = add.getBoundingClientRect();
      this.openCouplerAddAt(rect.right + 6, rect.top);
    });
    rail.append(add);
    panel.addEventListener("contextmenu", (event) => {
      if (event.target.closest('[data-key^="coupler-"]')) return; // its own editor
      event.preventDefault();
      event.stopPropagation();
      if (!this.unlocked && !event.ctrlKey) {
        this.nudgeUnlock();
        return;
      }
      this.openCouplersMenu(event.clientX, event.clientY);
    });
  }

  /// The add popover, opened straight onto its coupler form — the
  /// rail's + skips the menu the canvas double-click goes through.
  openCouplerAddAt(x, y) {
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeCouplerForm();
    this.closeCouplersMenu();
    this.addAnchor = { x, y };
    this.el.add.classList.remove("hidden");
    this.openCouplerAddForm();
  }

  // ---- the couplers menu: this organ's coupler-wide settings --------------

  wireCouplersMenu() {
    this.el.coupledKeys.addEventListener("change", () => {
      this.organCommand(commands.organCoupledKeys(this.el.coupledKeys.checked));
    });
  }

  openCouplersMenu(x, y) {
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeCouplerForm();
    this.syncCouplersMenu();
    this.el.couplersMenu.classList.remove("hidden");
    this.positionPopover(this.el.couplersMenu, x, y);
  }

  closeCouplersMenu() {
    this.el.couplersMenu.classList.add("hidden");
  }

  syncCouplersMenu() {
    if (this.root.activeElement !== this.el.coupledKeys) {
      this.el.coupledKeys.checked = this.lastSnapshot?.coupled_keys !== false;
    }
  }

  startManualRename(idx) {
    if (this.renamingManual === idx) return;
    const board = this.root.querySelector(`.keyboard[data-manual="${idx}"]`);
    const cheek = board?.querySelector(".cheek");
    const manual = this.lastSnapshot?.manuals.find((m) => m.idx === idx);
    if (!board || !cheek || !manual) return;
    this.renamingManual = idx;
    cheek.style.visibility = "hidden";

    const input = document.createElement("input");
    input.className = "editor-cheek-rename";
    input.value = manual.name;
    input.setAttribute("aria-label", `Rename ${manual.name}`);

    const commit = () => {
      if (this.renamingManual !== idx) return;
      this.renamingManual = null;
      input.remove();
      cheek.style.visibility = "";
      const name = input.value.trim();
      if (name && name !== manual.name) this.organCommand(commands.organManualRename(idx, name));
    };
    const abandon = () => {
      this.renamingManual = null;
      input.remove();
      cheek.style.visibility = "";
    };
    input.addEventListener("keydown", (event) => {
      event.stopPropagation(); // never falls through to a key binding
      if (event.key === "Enter") {
        event.preventDefault();
        commit();
      } else if (event.key === "Escape") {
        event.preventDefault();
        abandon();
      }
    });
    input.addEventListener("blur", commit);
    board.append(input);
    requestAnimationFrame(() => {
      input.focus();
      input.select();
    });
  }

  // ---- moving panels ------------------------------------------------------
  //
  // Every panel moves by its title bar when unlocked, and a ctrl-drag
  // anywhere on a panel that isn't a control moves it even locked —
  // "ctrl-drag anything" holds for panels too. The move is applied
  // live in pixels and persisted on release as fractions of the canvas
  // (POST /api/organ/panel/place), so it lands in the organ file.

  wirePanelMoves() {
    for (const panel of this.el.canvas.querySelectorAll(".panel")) {
      const chrome = panel.querySelector(".panel-chrome");
      chrome?.addEventListener("pointerdown", (event) => {
        if (event.button !== 0) return;
        this.startPanelDrag(panel, event);
      });
      panel.addEventListener("pointerdown", (event) => {
        if (event.button !== 0) return;
        if (!(event.ctrlKey || this.unlocked)) return;
        if (event.target.closest(INTERACTIVE)) return;
        if (event.target.closest(".panel-chrome")) return; // chrome handled above
        this.startPanelDrag(panel, event);
      });
    }
  }

  startPanelDrag(panel, event) {
    event.preventDefault();
    const rect = panel.getBoundingClientRect();
    const canvasRect = this.el.canvas.getBoundingClientRect();
    this.panelDrag = {
      panel,
      dx: event.clientX - rect.left,
      dy: event.clientY - rect.top,
      canvasRect,
      moved: false,
    };
    const move = (e) => this.panelDragMove(e);
    const up = (e) => {
      window.removeEventListener("pointermove", move);
      this.endPanelDrag(e);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up, { once: true });
  }

  panelDragMove(event) {
    const drag = this.panelDrag;
    if (!drag) return;
    if (!drag.moved) {
      drag.moved = true;
      drag.panel.dataset.dragging = "1";
      drag.panel.classList.add("dragging");
    }
    const { canvasRect, panel } = drag;
    const w = panel.offsetWidth;
    const h = panel.offsetHeight;
    const x = Math.min(canvasRect.width - w, Math.max(0, event.clientX - canvasRect.left - drag.dx));
    const y = Math.min(canvasRect.height - h, Math.max(0, event.clientY - canvasRect.top - drag.dy));
    panel.style.left = `${Math.round(x)}px`;
    panel.style.top = `${Math.round(y)}px`;
  }

  endPanelDrag() {
    const drag = this.panelDrag;
    this.panelDrag = null;
    if (!drag || !drag.moved) return;
    const { panel, canvasRect } = drag;
    delete panel.dataset.dragging;
    panel.classList.remove("dragging");
    const x = parseFloat(panel.style.left) / canvasRect.width;
    const y = parseFloat(panel.style.top) / canvasRect.height;
    this.organCommand(commands.organPanelPlace(panel.dataset.panel, x, y));
  }

  // ---- resizing: the grip in the corner ------------------------------------
  //
  // Dragging a jamb's grip sets the panel's width — which is what
  // wraps the knob rank into columns (see .division-knobs); the height
  // always follows the content, so nothing is ever clipped. Dragging a
  // keyboard's grip scales its keys instead (see --kb-scale in
  // style.css): the panel keeps hugging the scaled content. Either
  // size persists as canvas fractions alongside the panel's position,
  // the panel-placement contract.

  wirePanelResize() {
    for (const panel of this.el.canvas.querySelectorAll(".panel-jamb, .panel-keyboard")) {
      const keyboard = panel.classList.contains("panel-keyboard");
      const grip = document.createElement("div");
      grip.className = "panel-grip";
      grip.title = keyboard
        ? "Drag to resize the keyboard."
        : "Drag to widen — the stops wrap into columns.";
      grip.addEventListener("pointerdown", (event) => {
        if (event.button !== 0) return;
        if (!(event.ctrlKey || this.unlocked)) return;
        event.preventDefault();
        event.stopPropagation(); // never also a panel move
        if (keyboard) this.startKeyboardResize(panel, event);
        else this.startPanelResize(panel, event);
      });
      panel.append(grip);
    }
  }

  /// A keyboard's resize: the chrome (cheek, padding) doesn't scale,
  /// so the factor is solved against the key field alone — the same
  /// math console.js's scaleKeyboard applies when the stored size
  /// comes back off the file.
  startKeyboardResize(panel, event) {
    const keys = panel.querySelector(".keys");
    if (!keys) return;
    const start = {
      x: event.clientX,
      w: panel.offsetWidth,
      chrome: panel.offsetWidth - keys.offsetWidth,
      natural:
        keys.offsetWidth / (parseFloat(panel.style.getPropertyValue("--kb-scale")) || 1),
    };
    if (!(start.natural > 0)) return;
    panel.dataset.dragging = "1"; // layoutPanels leaves a mid-gesture panel alone
    const move = (e) => {
      const target = start.w + e.clientX - start.x;
      const scale = Math.max(0.35, Math.min(3, (target - start.chrome) / start.natural));
      panel.style.setProperty("--kb-scale", scale.toFixed(4));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      delete panel.dataset.dragging;
      const canvas = this.el.canvas.getBoundingClientRect();
      if (!canvas.width || !canvas.height) return;
      this.organCommand(
        commands.organPanelPlace(
          panel.dataset.panel,
          parseFloat(panel.style.left || "0") / canvas.width,
          parseFloat(panel.style.top || "0") / canvas.height,
          {
            w: panel.offsetWidth / canvas.width,
            h: panel.offsetHeight / canvas.height,
          }
        )
      );
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up, { once: true });
  }

  startPanelResize(panel, event) {
    const start = { x: event.clientX, w: panel.offsetWidth };
    // Never narrower than one knob and the body's padding — a rank
    // can wrap, but a knob must not be clipped.
    const floor = 118;
    panel.dataset.dragging = "1"; // layoutPanels leaves a mid-gesture panel alone
    panel.classList.add("sized");
    const move = (e) => {
      panel.style.width = `${Math.max(floor, start.w + e.clientX - start.x)}px`;
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      delete panel.dataset.dragging;
      const canvas = this.el.canvas.getBoundingClientRect();
      if (!canvas.width || !canvas.height) return;
      this.organCommand(
        commands.organPanelPlace(
          panel.dataset.panel,
          parseFloat(panel.style.left || "0") / canvas.width,
          parseFloat(panel.style.top || "0") / canvas.height,
          {
            w: panel.offsetWidth / canvas.width,
            h: panel.offsetHeight / canvas.height,
          }
        )
      );
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up, { once: true });
  }

  // ---- the per-division "+" ------------------------------------------------

  /// Each jamb division gets a small + beside its name while editing:
  /// add a stop from what the sources offer, or throw the division's
  /// stops into a swell box of their own.
  addDivisionButtons(snapshot) {
    for (const head of this.el.canvas.querySelectorAll(".division-head")) {
      const idx = Number(head.parentElement.dataset.division);
      const button = document.createElement("button");
      button.type = "button";
      button.className = "division-add";
      button.textContent = "+";
      const manual = snapshot.manuals.find((m) => m.idx === idx);
      button.setAttribute("aria-label", `Add to ${manual?.name ?? "this division"}`);
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        this.openDivisionMenu(idx, button);
      });
      head.append(button);
    }
  }

  openDivisionMenu(idx, anchor) {
    this.closeCouplerForm();
    const menu = this.el.divisionMenu;
    menu.replaceChildren();
    this.buildDivisionMenuItems(menu, idx);
    menu.classList.remove("hidden");
    const rect = anchor.getBoundingClientRect();
    this.positionPopover(menu, rect.right + 6, rect.top);
  }

  buildDivisionMenuItems(menu, idx) {
    const snapshot = this.lastSnapshot;
    const manual = snapshot?.manuals.find((m) => m.idx === idx);
    if (!manual) return;

    const addStop = document.createElement("button");
    addStop.className = "menu-item";
    addStop.innerHTML = "<span>Add a stop&hellip;</span>";
    addStop.addEventListener("click", async (event) => {
      event.stopPropagation();
      await this.showDivisionStops(menu, manual);
    });
    menu.append(addStop);

    // "No enclosure already": none of this division's stops are boxed
    // and no box carries its name. The box takes the whole division.
    const stops = (snapshot.stops ?? []).filter((s) => s.midx === idx);
    const enclosed = stops.some((s) => (s.enc ?? []).length);
    const named = (snapshot.enclosures ?? []).some((e) => e.name === manual.name);
    if (stops.length && !enclosed && !named) {
      const addBox = document.createElement("button");
      addBox.className = "menu-item";
      addBox.innerHTML = "<span>Enclose in a swell box</span>";
      addBox.addEventListener("click", () => {
        this.closeDivisionMenu();
        // One rebuild each; runQueue waits each rebuild out.
        this.runQueue([
          commands.organEnclosureAdd(manual.name),
          ...stops.map((stop) => commands.organEnclosureAssign(manual.name, stop.id, true)),
        ]);
      });
      menu.append(addBox);
    }
  }

  /// Swap the division menu's items for a pick-list of every stop the
  /// sources still offer; clicking one pulls it onto this manual. The
  /// list stays open so a division can be registered in one visit.
  async showDivisionStops(menu, manual) {
    menu.replaceChildren(this.emptyNote("Reading the sources…"));
    if (!this.offerings) await this.fetchOfferings(false);
    menu.replaceChildren();
    const sources = this.offerings;
    if (sources == null) {
      menu.append(this.emptyNote("Couldn't read this organ's sources."));
      return;
    }
    let any = false;
    for (const source of sources) {
      for (const srcManual of source.manuals ?? []) {
        const remaining = (srcManual.stops ?? []).filter((s) => !s.pulled);
        if (!remaining.length) continue;
        any = true;
        const group = document.createElement("div");
        group.className = "division-add-group";
        const title = document.createElement("span");
        title.className = "organ-stop-group-title";
        title.textContent = `${source.alias} · ${srcManual.name}`;
        group.append(title);
        for (const stop of remaining) {
          const row = document.createElement("button");
          row.type = "button";
          row.className = "menu-item";
          row.innerHTML = `<span>${stop.name}</span>`;
          row.addEventListener("click", (event) => {
            event.stopPropagation();
            row.disabled = true; // optimistic: pulled now
            this.organCommand(
              commands.organPull(source.alias, srcManual.name, manual.name, stop.name)
            );
          });
          group.append(row);
        }
        menu.append(group);
      }
    }
    if (!any) {
      menu.append(
        this.emptyNote("The sources have nothing left to offer — add a sample set first.")
      );
    }
  }

  closeDivisionMenu() {
    this.el.divisionMenu.classList.add("hidden");
    this.el.divisionMenu.replaceChildren();
  }

  // ---- the keyboard context menu: change a manual's kind or tuning --------
  //
  // Right-click a keyboard panel while unlocked: a radio group of the
  // three kinds (picking a different one is a structural edit, same
  // contract as the add menu) and a way into the tuning popover below.

  openKeyboardMenu(idx, x, y) {
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeTuningForm();
    this.closeCouplerForm();
    this.closeSettingsPopovers();
    const menu = this.el.keyboardMenu;
    menu.replaceChildren();
    this.buildKeyboardMenuItems(menu, idx);
    menu.classList.remove("hidden");
    this.positionPopover(menu, x, y);
  }

  buildKeyboardMenuItems(menu, idx) {
    const manual = this.lastSnapshot?.manuals.find((m) => m.idx === idx);
    if (!manual) return;
    const currentKind = manual.kind ?? (manual.pedal ? "pedal" : "manual");

    const heading = document.createElement("span");
    heading.className = "menu-heading";
    heading.textContent = "Change type";
    menu.append(heading);

    for (const [kind, label] of KEYBOARD_KINDS) {
      const item = document.createElement("button");
      item.className = "menu-item radio";
      item.classList.toggle("checked", kind === currentKind);
      item.innerHTML = `<span>${label}</span>`;
      item.addEventListener("click", (event) => {
        event.stopPropagation();
        this.closeKeyboardMenu();
        if (kind !== currentKind) this.organCommand(commands.organManualKind(idx, kind));
      });
      menu.append(item);
    }

    menu.append(document.createElement("hr"));

    // The bin gesture as a menu item — same confirm, same command.
    const remove = document.createElement("button");
    remove.className = "menu-item";
    remove.innerHTML = "<span>Remove keyboard&hellip;</span>";
    remove.addEventListener("click", (event) => {
      event.stopPropagation();
      this.closeKeyboardMenu();
      const stopCount = (this.lastSnapshot?.stops ?? []).filter((s) => s.midx === idx).length;
      this.showRemoveConfirm("manual", { idx, name: manual.name, stopCount });
    });
    menu.append(remove);

    // The manual's own wiring and reach, popovers of their own. Both
    // sit above "Change tuning…" so the tuning item stays the menu's
    // last (harness-hooks.js counts on that).
    const midi = document.createElement("button");
    midi.className = "menu-item";
    midi.innerHTML = "<span>MIDI input&hellip;</span>";
    midi.addEventListener("click", (event) => {
      event.stopPropagation();
      const rect = menu.getBoundingClientRect();
      this.closeKeyboardMenu();
      this.openMidiForm(idx, rect.left, rect.top);
    });
    menu.append(midi);

    const compass = document.createElement("button");
    compass.className = "menu-item";
    compass.innerHTML = "<span>Compass&hellip;</span>";
    compass.addEventListener("click", (event) => {
      event.stopPropagation();
      const rect = menu.getBoundingClientRect();
      this.closeKeyboardMenu();
      this.openCompassForm(idx, rect.left, rect.top);
    });
    menu.append(compass);

    // A hex field is a microtonal-manual fact; the other kinds have
    // no layout to offer.
    if (currentKind === "microtonal") {
      const hex = document.createElement("button");
      hex.className = "menu-item";
      hex.innerHTML = "<span>Hex layout&hellip;</span>";
      hex.addEventListener("click", (event) => {
        event.stopPropagation();
        const rect = menu.getBoundingClientRect();
        this.closeKeyboardMenu();
        this.openHexForm(idx, rect.left, rect.top);
      });
      menu.append(hex);
    }

    const tuning = document.createElement("button");
    tuning.className = "menu-item";
    tuning.innerHTML = "<span>Change tuning&hellip;</span>";
    tuning.addEventListener("click", (event) => {
      event.stopPropagation();
      const rect = menu.getBoundingClientRect();
      this.closeKeyboardMenu();
      this.openTuningForm({ kind: "division", idx }, rect.left, rect.top);
    });
    menu.append(tuning);
  }

  closeKeyboardMenu() {
    this.el.keyboardMenu.classList.add("hidden");
    this.el.keyboardMenu.replaceChildren();
  }

  // ---- the tuning popover: this manual's own pitch, apart from the --------
  // instrument's, applied live field by field — never a rebuild. A
  // Scala scale (and its optional keymap) is just another field on the
  // same /api/tuning contract; picking one supersedes the temperament.
  //
  // Every field goes through `tuningCommand` rather than the plain
  // `send()` the rest of the console uses: a scale path can 400 (a bad
  // file, an unparseable one), and that reason needs to land in this
  // popover, not the app-wide status strip — see `showTuningError`.

  wireTuningForm() {
    // Every MIDI note for the reference-key field's autocomplete —
    // built here rather than hand-written into index.html, in the
    // ASCII spellings a player can type, black keys under both names.
    for (let key = 0; key <= 127; key++) {
      for (const spelling of keySpellings(key)) {
        const option = document.createElement("option");
        option.value = spelling;
        this.el.pitchNames.append(option);
      }
    }

    this.el.tuningClose.addEventListener("click", () => this.closeTuningForm());

    // The Follows select: what a division/source/rank falls back to is
    // binary (the parent, or its own), a stop's is the five-way
    // auto/division/source/organ/own vocabulary the server's `follow`
    // param speaks directly — so every kind maps straight onto one
    // /api/tuning call, never a client-side branch on what "own" means.
    this.el.tuningFollow.addEventListener("change", () => {
      const scope = this.tuningScope;
      if (!scope) return;
      const value = this.el.tuningFollow.value;
      if (scope.kind === "stop") {
        this.tuningCommand(this.tuningFields({ follow: value }));
      } else if (value === "own") {
        this.tuningCommand(this.tuningFields({ follow: "own" }));
      } else {
        this.tuningCommand(this.tuningFields({ reset: 1 }));
      }
      this.el.tuningFollow.blur();
    });

    this.el.tuningTemperament.addEventListener("change", () => {
      if (!this.tuningScope) return;
      // Naming a temperament here is allowed even with a scale active —
      // the server reads it as leaving the scale (http.rs's /api/tuning
      // arm clears `tuning.scale` whenever `temperament` is given).
      this.tuningCommand(this.tuningFields({ temperament: this.el.tuningTemperament.value }));
      this.el.tuningTemperament.blur();
    });

    this.el.tuningPipes.addEventListener("change", () => {
      if (!this.tuningScope) return;
      this.tuningCommand(this.tuningFields({ pipes: this.el.tuningPipes.value }));
      this.el.tuningPipes.blur();
    });

    this.el.tuningEdo.addEventListener("change", () => {
      if (!this.tuningScope) return;
      const edo = Math.min(311, Math.max(1, Math.round(Number(this.el.tuningEdo.value) || 12)));
      this.el.tuningEdo.value = edo;
      // Like naming a temperament, choosing a division count leaves
      // any active scale (the server clears it on this field).
      this.tuningCommand(this.tuningFields({ edo }));
    });

    // The pitch anchor is a key/Hz *pair* — "a′" only names a key in
    // 12-EDO — so either field changing re-sends both: the server keeps
    // whichever one didn't move.
    this.el.tuningRefKey.addEventListener("change", () => {
      if (!this.tuningScope) return;
      const key = parseKeyName(this.el.tuningRefKey.value);
      if (key == null) {
        this.showTuningError(`"${this.el.tuningRefKey.value}" doesn't name a key`);
        this.el.tuningRefKey.blur();
        this.syncTuningForm(); // restores the last known-good spelling
        return;
      }
      // The player's own spelling stays on screen: D#4 typed is D#4
      // shown, not the E♭4 the canonical printer would pick. The
      // server only ever sees the key number's canonical name.
      this.tuningRefKeySpelling = { key, text: tidyKeyName(this.el.tuningRefKey.value) };
      this.el.tuningRefKey.value = this.tuningRefKeySpelling.text;
      const hz = Number(this.el.tuningRefHz.value);
      this.tuningCommand(this.tuningFields({ reference_key: keyName(key), reference_hz: hz }));
      this.el.tuningRefKey.blur();
    });

    this.el.tuningRefHz.addEventListener("change", () => {
      if (!this.tuningScope) return;
      const hz = Number(this.el.tuningRefHz.value);
      // No hard range here — the server clamps so the implied shift
      // stays within a′ 300–500 Hz equivalents and the next snapshot
      // reflects the clamped value; a bad number just reverts.
      if (!Number.isFinite(hz) || hz <= 0) {
        this.el.tuningRefHz.blur();
        this.syncTuningForm();
        return;
      }
      const key = parseKeyName(this.el.tuningRefKey.value);
      this.tuningCommand(this.tuningFields({
        reference_key: key != null ? keyName(key) : this.el.tuningRefKey.value,
        reference_hz: hz,
      }));
      this.el.tuningRefHz.blur();
    });

    // "As recorded": put the reference back on whatever the sample set
    // itself sounds on the current reference key — the server reads
    // `reference_hz=home` as that instruction rather than a literal Hz
    // number (see /api/tuning). Only shown when it would move anything;
    // see the visibility check in syncTuningForm.
    this.el.tuningRefHome.addEventListener("click", () => {
      if (!this.tuningScope) return;
      this.tuningCommand(this.tuningFields({ reference_hz: "home" }));
    });

    this.el.tuningTranspose.addEventListener("change", () => {
      if (!this.tuningScope) return;
      const transpose = Math.min(12, Math.max(-12, Math.round(Number(this.el.tuningTranspose.value) || 0)));
      this.el.tuningTranspose.value = transpose;
      this.tuningCommand(this.tuningFields({ transpose }));
      this.el.tuningTranspose.blur();
    });

    this.el.tuningScalePick.addEventListener("click", () => this.openTuningBrowse("scale"));
    this.el.tuningScaleClear.addEventListener("click", () => {
      if (!this.tuningScope) return;
      this.tuningCommand(this.tuningFields({ scale: "off" }));
    });

    this.el.tuningKeymapPick.addEventListener("click", () => this.openTuningBrowse("keymap"));
    this.el.tuningKeymapClear.addEventListener("click", () => {
      if (!this.tuningScope) return;
      const scl = this.currentScalePath();
      if (!scl) return;
      // An empty `keymap` param is indistinguishable, server-side, from
      // an omitted one (http.rs filters both to "no keymap") — sending
      // it explicitly just documents the intent here.
      this.tuningCommand(this.tuningFields({ scale: scl, keymap: "" }));
    });

    this.el.tuningBrowseUp.addEventListener("click", () => {
      if (this.tuningBrowseParent) this.tuningBrowse(this.tuningBrowseParent);
    });
    this.el.tuningBrowseCancel.addEventListener("click", () => this.closeTuningBrowse());
  }

  /// `scope` is one of {kind:"organ"} | {kind:"division", idx} |
  /// {kind:"source", alias} | {kind:"stop", id} | {kind:"rank", stop, rank}
  /// — see the class-level comment by `this.tuningScope`. The bare
  /// string "organ" (main.js's own two call sites, predating the
  /// scope object) is still accepted as shorthand for {kind:"organ"}.
  openTuningForm(scope, x, y) {
    if (scope === "organ") scope = { kind: "organ" };
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeHexForm();
    this.closeCouplerForm();
    this.closeSettingsPopovers();
    this.tuningScope = scope;
    this.hideTuningError();
    this.closeTuningBrowse();
    this.syncTuningForm();
    // A bad scope (a stop/division/rank the snapshot doesn't have)
    // closes right back — sync's own job, since a later poll can find
    // the same thing gone. Nothing left to show.
    if (this.tuningScope == null) return;
    // A source or stop's popover names the governing sample set by its
    // offerings entry (display name, not just the bare alias) — fetch
    // once, quietly, if the drawer never has been opened this session.
    if ((scope.kind === "source" || scope.kind === "stop") && this.offerings == null) {
      this.fetchOfferings(false).then(() => this.syncTuningForm());
    }
    this.el.tuning.classList.remove("hidden");
    this.positionPopover(this.el.tuning, x, y);
  }

  closeTuningForm() {
    this.tuningScope = null;
    this.tuningResolved = null;
    this.el.tuning.classList.add("hidden");
    this.hideTuningError();
    this.closeTuningBrowse();
  }

  /// Replaces the Follows select's own options — each scope kind offers
  /// a different vocabulary (division/source: instrument or own; stop:
  /// the full auto/division/source/organ/own cascade; rank: the stop or
  /// own). `pairs` is `[value, label]`.
  setFollowOptions(pairs) {
    this.el.tuningFollow.replaceChildren(
      ...pairs.map(([value, label]) => {
        const option = document.createElement("option");
        option.value = value;
        option.textContent = label;
        return option;
      })
    );
  }

  /// The short label a tuning reads by — a scale's name, an EDO count,
  /// or the temperament select's own friendly text for the value (so
  /// renaming an option, like "original" → "As recorded", only has to
  /// happen in index.html).
  tuningLabel(tuning) {
    if (!tuning) return "";
    if (tuning.scale) return `${tuning.scale.name} (${tuning.scale.notes} notes)`;
    const edo = tuning.edo ?? 12;
    if (edo !== 12) return `${edo}-EDO`;
    const option = this.el.tuningTemperament.querySelector(`option[value="${tuning.temperament}"]`);
    return option ? option.textContent : tuning.temperament;
  }

  /// The resolved line's "…: <summary>" half — the label above plus
  /// where it's anchored, e.g. "¼-comma meantone · A4 = 415.3 Hz".
  tuningSummary(tuning) {
    if (!tuning?.reference) return "";
    const hz = tuning.reference.hz.toFixed(1).replace(/\.0$/, "");
    return `${this.tuningLabel(tuning)} · ${keyName(tuning.reference.key)} = ${hz} Hz`;
  }

  /// A source's display name from the cached offerings list, falling
  /// back to its bare alias when the offerings haven't loaded yet.
  sourceDisplayName(alias) {
    return this.offerings?.find((s) => s.alias === alias)?.name ?? alias ?? "?";
  }

  /// What a stop is actually playing right now, cutting straight to the
  /// concrete tuning object regardless of how it got there — used both
  /// to populate its own popover and (via a rank's "follows the stop")
  /// to resolve a rank that doesn't have one of its own.
  stopEffectiveTuning(stop) {
    const snap = this.lastSnapshot;
    const info = stop?.tuning ?? { scope: "organ" };
    if (info.scope === "stop") {
      return (snap.stop_tuning ?? []).find((t) => t.stop === stop.id) ?? snap.tuning;
    }
    if (info.scope === "division") {
      return (snap.manual_tuning ?? []).find((t) => t.idx === stop.midx) ?? snap.tuning;
    }
    if (info.scope === "source") {
      return (snap.source_tuning ?? []).find((t) => t.source === stop.src?.from) ?? snap.tuning;
    }
    return snap.tuning;
  }

  /// The stop editor's compact "Tuning" line — also the accent dot's
  /// title attribute on the console (refreshTuningChips): "Automatic →
  /// Récit · 19-EDO", "Pinned · sample set", "Own · Pythagorean".
  stopTuningLine(stop) {
    const snap = this.lastSnapshot;
    const info = stop.tuning ?? { scope: "organ", follow: "auto" };
    const manual = snap.manuals.find((m) => m.idx === stop.midx);
    if (info.follow === "own") {
      const own = (snap.stop_tuning ?? []).find((t) => t.stop === stop.id);
      return `Own · ${this.tuningLabel(own)}`;
    }
    if (info.follow && info.follow !== "auto") {
      const target = { division: manual?.name ?? "division", source: "sample set", organ: "instrument" }[info.follow];
      return `Pinned · ${target}`;
    }
    const target =
      info.scope === "division" ? manual?.name ?? "division" : info.scope === "source" ? "sample set" : "instrument";
    return `Automatic → ${target} · ${this.tuningLabel(this.stopEffectiveTuning(stop))}`;
  }

  /// Fills `#editor-tuning-resolved-primary`/`-chain` with "Plays X's
  /// tuning: <summary> — open →" and dims the fields below to match —
  /// the common tail every following (non-own) scope shares. `link`
  /// jumps this same popover to the scope that actually governs.
  setResolvedLines(primaryLabel, link, tuning, chainText) {
    this.el.tuningResolvedPrimary.replaceChildren(`Plays ${primaryLabel}'s tuning: ${this.tuningSummary(tuning)} — `);
    const open = document.createElement("button");
    open.type = "button";
    open.className = "tuning-open-link";
    open.textContent = "open →";
    open.addEventListener("click", () => {
      const rect = this.el.tuning.getBoundingClientRect();
      this.openTuningForm(link, rect.left, rect.top);
    });
    this.el.tuningResolvedPrimary.append(open);
    this.el.tuningResolvedPrimary.classList.remove("hidden");
    this.el.tuningResolvedChain.textContent = chainText ?? "";
    this.el.tuningResolvedChain.classList.toggle("hidden", !chainText);
  }

  /// Dims (and disables) the spec fields while a scope is following
  /// someone else's tuning — item 3 of the tuning-cascade UI: the
  /// values still read as what's actually playing, but nothing here is
  /// live until "Own tuning" seeds a copy to edit.
  setTuningFollowing(following) {
    for (const row of [
      this.el.tuningScaleRow, this.el.tuningKeymapRow, this.el.tuningEdoRow,
      this.el.tuningTemperamentRow, this.el.tuningPipesRow, this.el.tuningRefRow,
      this.el.tuningTransposeRow,
    ]) {
      row.classList.toggle("tuning-following", following);
    }
    for (const field of [
      this.el.tuningScalePick, this.el.tuningScaleClear, this.el.tuningKeymapPick, this.el.tuningKeymapClear,
      this.el.tuningEdo, this.el.tuningTemperament, this.el.tuningPipes,
      this.el.tuningRefKey, this.el.tuningRefHz, this.el.tuningRefHome, this.el.tuningTranspose,
    ]) {
      field.disabled = following;
    }
  }

  /// Every scope kind's tuning popover: what governs it right now (its
  /// own tuning, or someone else's — organ/division/source/stop cascade
  /// down to instrument), reflected into the Follows select, the
  /// resolved-line prose, and the spec fields below (which show the
  /// governing values, live and editable only when it's this scope's
  /// own). Called on open and on every later poll, so a shared value
  /// another panel (or session) changes keeps the popover honest. Never
  /// touches the file-browser sub-view (see `openTuningBrowse`/
  /// `closeTuningBrowse`) — a poll landing mid-navigation must not yank
  /// it shut.
  syncTuningForm() {
    const scope = this.tuningScope;
    const snap = this.lastSnapshot;
    if (!scope || !snap) return;

    let tuning; // populates the fields below
    let following = false; // fields read someone else's tuning, disabled
    this.el.tuningFollowRow.classList.toggle("hidden", scope.kind === "organ");
    this.el.tuningTransposeRow.classList.toggle("hidden", scope.kind === "source" || scope.kind === "stop" || scope.kind === "rank");

    if (scope.kind === "organ") {
      this.el.tuningTitle.textContent = "Whole instrument";
      tuning = snap.tuning;
    } else if (scope.kind === "division") {
      const manual = snap.manuals.find((m) => m.idx === scope.idx);
      if (!manual) return this.closeTuningForm();
      this.el.tuningTitle.textContent = manual.name;
      const own = (snap.manual_tuning ?? []).find((t) => t.idx === scope.idx);
      this.setFollowOptions([["organ", "Whole instrument"], ["own", "Own tuning"]]);
      if (this.root.activeElement !== this.el.tuningFollow) this.el.tuningFollow.value = own ? "own" : "organ";
      if (own) {
        tuning = own;
      } else {
        tuning = snap.tuning;
        following = true;
        this.setResolvedLines("the instrument", { kind: "organ" }, tuning);
      }
    } else if (scope.kind === "source") {
      // No existence check against the offerings list: it may not have
      // loaded yet (openTuningForm kicks that fetch off but doesn't wait
      // on it), and the alias is otherwise all this scope needs — the
      // display name just falls back to the bare alias until it lands.
      const name = this.sourceDisplayName(scope.alias);
      this.el.tuningTitle.textContent = `${name} · sample set`;
      const own = (snap.source_tuning ?? []).find((t) => t.source === scope.alias);
      this.setFollowOptions([["organ", "Whole instrument"], ["own", "Own tuning"]]);
      if (this.root.activeElement !== this.el.tuningFollow) this.el.tuningFollow.value = own ? "own" : "organ";
      if (own) {
        tuning = own;
      } else {
        tuning = snap.tuning;
        following = true;
        this.setResolvedLines("the instrument", { kind: "organ" }, tuning);
      }
    } else if (scope.kind === "stop") {
      const stop = snap.stops.find((s) => s.id === scope.id);
      if (!stop) return this.closeTuningForm();
      const manual = snap.manuals.find((m) => m.idx === stop.midx);
      this.el.tuningTitle.textContent = `${stop.name} · ${manual?.name ?? ""}`;
      const info = stop.tuning ?? { scope: "organ", follow: "auto" };
      const divOwn = (snap.manual_tuning ?? []).find((t) => t.idx === stop.midx);
      const srcAlias = stop.src?.from;
      const srcOwn = srcAlias ? (snap.source_tuning ?? []).find((t) => t.source === srcAlias) : null;
      const autoLabel = divOwn ? "division" : srcOwn ? "sample set" : "instrument";
      this.setFollowOptions([
        ["auto", `Automatic (→ ${autoLabel})`],
        ["division", `Division · ${manual?.name ?? "?"}`],
        ["source", `Sample set · ${srcAlias ? this.sourceDisplayName(srcAlias) : "?"}`],
        ["organ", "Whole instrument"],
        ["own", "Own tuning"],
      ]);
      if (this.root.activeElement !== this.el.tuningFollow) this.el.tuningFollow.value = info.follow ?? "auto";

      if (info.scope === "stop") {
        tuning = (snap.stop_tuning ?? []).find((t) => t.stop === stop.id) ?? snap.tuning;
      } else {
        following = true;
        if (info.scope === "division") {
          tuning = divOwn ?? snap.tuning;
          const sourceStatus = srcAlias ? (srcOwn ? "own tuning" : "follows instrument") : "no sample set";
          this.setResolvedLines(
            manual?.name ?? "this division", { kind: "division", idx: stop.midx }, tuning,
            `Sample set: ${sourceStatus} · Instrument: ${this.tuningSummary(snap.tuning)}`
          );
        } else if (info.scope === "source") {
          tuning = srcOwn ?? snap.tuning;
          this.setResolvedLines(
            this.sourceDisplayName(srcAlias), { kind: "source", alias: srcAlias }, tuning,
            `Instrument: ${this.tuningSummary(snap.tuning)}`
          );
        } else {
          tuning = snap.tuning;
          this.setResolvedLines("the instrument", { kind: "organ" }, tuning);
        }
      }
    } else if (scope.kind === "rank") {
      const stop = snap.stops.find((s) => s.id === scope.stop);
      const rankInfo = stop?.ranks?.find((r) => r.id === scope.rank);
      if (!stop || !rankInfo) return this.closeTuningForm();
      this.el.tuningTitle.textContent = `${rankInfo.name} · ${stop.name}`;
      this.setFollowOptions([["stop", "This stop"], ["own", "Own tuning"]]);
      if (this.root.activeElement !== this.el.tuningFollow) this.el.tuningFollow.value = rankInfo.own ? "own" : "stop";
      if (rankInfo.own) {
        tuning = (snap.rank_tuning ?? []).find((t) => t.stop === scope.stop && t.rank === scope.rank);
      } else {
        following = true;
        tuning = this.stopEffectiveTuning(stop);
        this.setResolvedLines("this stop", { kind: "stop", id: scope.stop }, tuning);
      }
    }

    if (!tuning) return;
    this.tuningResolved = tuning;
    this.el.tuningResolvedPrimary.classList.toggle("hidden", !following);
    if (!following) this.el.tuningResolvedChain.classList.add("hidden");
    this.setTuningFollowing(following);

    // Temperaments are twelve-class vocabulary: the row shows only
    // while the division count is 12 (absent on an old snapshot = 12).
    const edo = tuning.edo ?? 12;
    this.el.tuningTemperamentRow.classList.toggle("hidden", edo !== 12);
    if (this.root.activeElement !== this.el.tuningTemperament) {
      this.el.tuningTemperament.value = tuning.temperament;
    }
    if (this.root.activeElement !== this.el.tuningEdo) this.el.tuningEdo.value = edo;
    if (this.root.activeElement !== this.el.tuningRefKey) {
      const spelling = this.tuningRefKeySpelling;
      this.el.tuningRefKey.value =
        spelling?.key === tuning.reference.key ? spelling.text : keyName(tuning.reference.key);
    }
    if (this.root.activeElement !== this.el.tuningRefHz) this.el.tuningRefHz.value = tuning.reference.hz;
    if (this.root.activeElement !== this.el.tuningTranspose) this.el.tuningTranspose.value = tuning.transpose ?? 0;

    // "Recorded: …" — what the sample set itself sounds, measured at
    // load time. Lives on the snapshot's top level (`home`) for the
    // instrument as a whole; at set scope, `source_home` swaps in that
    // set's own recorded A4 alongside the instrument-wide temperament
    // and spread (every division of one set was recorded together).
    let home = this.lastSnapshot?.home ?? null;
    if (home && scope.kind === "source") {
      const setHome = (snap.source_home ?? []).find((h) => h.source === scope.alias);
      if (setHome) home = { ...home, a4_hz: setHome.a4_hz };
    }
    if (!home) {
      this.el.tuningHome.textContent = "Recorded: not measured (assuming A4 = 440 equal)";
    } else {
      const name = HOME_TEMPERAMENT_NAMES[home.temperament] ?? "unequal (unnamed)";
      const mixed = home.spread_cents > 8 ? " · mixed pitch standards?" : "";
      this.el.tuningHome.textContent =
        `Recorded: A4 = ${home.a4_hz.toFixed(1).replace(/\.0$/, "")} Hz · ${name} · ` +
        `±${home.spread_cents.toFixed(1).replace(/\.0$/, "")} ¢ · ${home.measured} of ${home.pipes} pipes${mixed}`;
    }
    // The ↺ button next to Reference: only worth showing when it would
    // actually move something — when the reference isn't already
    // sitting on what the recording itself sounds on that key. A
    // button that silently does nothing on click is worse than none.
    // `.wrap` rides along on the same condition: the row has no spare
    // width for a fourth thing, so the button (and "Hz" alongside it)
    // only wrap to their own line while there's a button to make room
    // for — the row's ordinary layout is untouched the rest of the time.
    const showRefHome =
      !following && home != null && Math.abs(tuning.reference.hz - homeHz(home, tuning.reference.key)) > 0.05;
    this.el.tuningRefHome.classList.toggle("hidden", !showRefHome);
    this.el.tuningRefStepper.classList.toggle("wrap", showRefHome);

    const scale = tuning.scale ?? null;
    this.el.tuningScalePick.classList.toggle("hidden", !!scale);
    this.el.tuningScaleActive.classList.toggle("hidden", !scale);
    if (scale) {
      this.el.tuningScaleName.textContent = `${scale.name} · ${scale.notes} notes`;
      this.el.tuningScaleName.title = scale.scl;
    }

    this.el.tuningKeymapRow.classList.toggle("hidden", !scale);
    if (scale) {
      if (scale.kbm) {
        this.el.tuningKeymapName.textContent = scale.kbm.split("/").pop();
        this.el.tuningKeymapName.title = scale.kbm;
        this.el.tuningKeymapClear.classList.remove("hidden");
      } else {
        this.el.tuningKeymapName.textContent = "linear";
        this.el.tuningKeymapName.title = "";
        this.el.tuningKeymapClear.classList.add("hidden");
      }
    }

    // The scale IS the tuning while one is active — the temperament
    // select and the division count stay live (setting either is a
    // valid way back out) but read as superseded rather than in
    // effect.
    this.el.tuningTemperamentRow.classList.toggle("tuning-dimmed", !!scale);
    this.el.tuningEdoRow.classList.toggle("tuning-dimmed", !!scale);
    this.el.tuningTemperament.title = scale
      ? "A scale is active — picking a temperament here leaves it"
      : "";
    this.el.tuningEdo.title = scale
      ? "A scale is active — setting a division count here leaves it"
      : "";

    // Pipes only mean something under a target: as recorded, every
    // pipe is exactly where it is, so the row stays visible (the
    // choice persists into the next target) but reads dimmed.
    if (this.root.activeElement !== this.el.tuningPipes) {
      this.el.tuningPipes.value = tuning.pipes ?? "original";
    }
    const asRecorded = tuning.temperament === "original" && edo === 12 && !scale;
    this.el.tuningPipesRow.classList.toggle("tuning-dimmed", asRecorded);
    this.el.tuningPipes.title = asRecorded
      ? "As recorded, pipes are exactly where they are — this applies under a target tuning"
      : "";
  }

  /// The scope's effective scale path right now, or null with none —
  /// what a keymap pick or clear re-sends alongside, since /api/tuning
  /// takes the scale and its keymap together (see http.rs). Reads the
  /// same resolved tuning the fields above are already showing, so it
  /// agrees with them even while following (fields are disabled then,
  /// but the browse-cancel/clear paths still call in).
  currentScalePath() {
    return this.tuningResolved?.scale?.scl ?? null;
  }

  /// The tuning popover's target as /api/tuning fields — the scope
  /// selector each kind speaks (see the endpoint contract): none for
  /// the instrument, `manual=` for a division, `source=` for a set,
  /// `stop=` for a stop, `stop=`+`rank=` for one of its ranks.
  tuningFields(extra) {
    const scope = this.tuningScope;
    if (!scope || scope.kind === "organ") return extra;
    if (scope.kind === "division") return { manual: scope.idx, ...extra };
    if (scope.kind === "source") return { source: scope.alias, ...extra };
    if (scope.kind === "stop") return { stop: scope.id, ...extra };
    if (scope.kind === "rank") return { stop: scope.stop, rank: scope.rank, ...extra };
    return extra;
  }

  /// Sends a tuning field update directly (not through the app-wide
  /// `send()`), so a 400's reason can land in this popover instead of
  /// the global status strip — the same local-fetch idiom
  /// `organCommandResult` uses for structural edits.
  async tuningCommand(fields) {
    this.hideTuningError();
    const query = commands.tuning(fields);
    try {
      const response = await fetch(this.base + query, { method: "POST" });
      if (!response.ok) {
        if (this.deferToSaveAs(response, query)) return false;
        this.showTuningError((await response.text()) || `${response.status} ${response.statusText}`);
        return false;
      }
      return true;
    } catch (err) {
      this.showTuningError(String(err));
      return false;
    }
  }

  showTuningError(text) {
    this.el.tuningError.textContent = text;
    this.el.tuningError.classList.remove("hidden");
  }

  hideTuningError() {
    this.el.tuningError.classList.add("hidden");
    this.el.tuningError.textContent = "";
  }

  // ---- the MIDI-input popover: what plays this manual ---------------------
  //
  // "What drives the Récit?" is asked at the Récit: the keyboard menu's
  // "MIDI input…" (or the silent badge an unwired keyboard wears) opens
  // this manual's own input rows — device, channel, shift, bend, Listen
  // — plus quick piston rows for the pitch actions that shift it. The
  // wiring is an organ fact and lands in the organ's file; the rows are
  // the shared builders in wiring.js.

  wireMidiForm() {
    this.el.midiClose.addEventListener("click", () => this.closeMidiForm());
    this.el.midiRescan.addEventListener("click", () => this.send(commands.midiRescan()));
  }

  openMidiForm(idx, x, y) {
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeTuningForm();
    this.closeHexForm();
    this.closeCouplerForm();
    this.closeSettingsPopovers();
    this.midiManual = idx;
    this.midiSignature = null;
    this.syncMidiForm();
    this.el.midi.classList.remove("hidden");
    this.positionPopover(this.el.midi, x, y);
  }

  closeMidiForm() {
    if (this.midiManual == null) return;
    // Leaving the popover ends any wait for a key: the next thing the
    // player touches should sound, not be swallowed as an assignment.
    if (this.lastSnapshot?.midi?.learning) this.send(commands.midiLearn(null));
    this.midiManual = null;
    this.midiSignature = null;
    this.el.midi.classList.add("hidden");
  }

  /// Rebuilt only when something the rows depend on changes — the same
  /// signature discipline the old dialog kept, so a poll never tears a
  /// select out from under the pointer.
  syncMidiForm() {
    const idx = this.midiManual;
    const midi = this.lastSnapshot?.midi ?? { ports: [], manuals: [] };
    const entry = midi.manuals.find((m) => m.idx === idx);
    if (!entry) {
      this.closeMidiForm();
      return;
    }
    const keyboardSpan = this.lastSnapshot?.keyboard
      ? [this.lastSnapshot.keyboard.low, this.lastSnapshot.keyboard.high]
      : null;
    const pitchBindings = (this.lastSnapshot?.controls ?? []).filter(
      (c) => PITCH_ACTIONS.includes(c.action) && c.manual === entry.name
    );
    const signature = JSON.stringify([
      midi.ports, entry, midi.learning ?? null, keyboardSpan, pitchBindings, this.quickBind,
    ]);
    if (signature === this.midiSignature) return;
    this.midiSignature = signature;

    this.el.midiTitle.textContent = `${entry.name} · MIDI input`;
    this.el.midiInputs.replaceChildren();
    buildManualInputs(this.el.midiInputs, {
      midi,
      manualEntry: entry,
      keyboardSpan,
      send: this.send,
    });

    // The pitch actions that shift *this* keyboard, as quick piston
    // rows. Bindings that shift "the same keyboard" (no manual of
    // their own) live in the Bindings popover, where the whole list is.
    this.el.midiPistons.replaceChildren();
    const heading = document.createElement("span");
    heading.className = "menu-heading";
    heading.textContent = "Pistons";
    this.el.midiPistons.append(heading);
    for (const [action, label] of [
      ["octave-up", "Octave up"],
      ["octave-down", "Octave down"],
      ["transpose-up", "Transpose up"],
      ["transpose-down", "Transpose down"],
    ]) {
      const ctx = {
        snapshot: this.lastSnapshot,
        send: this.send,
        manual: entry.name,
        listening: this.quickBind?.action === action && this.quickBind?.manual === entry.name,
      };
      const row = document.createElement("div");
      row.className = "settings-row";
      const name = document.createElement("span");
      name.className = "rail-label";
      name.textContent = label;
      row.append(
        name,
        pistonRow(ctx, action, (act, cancelling) =>
          this.quickBindListen(act, entry.name, cancelling)
        )
      );
      this.el.midiPistons.append(row);
    }

    this.el.midiPorts.replaceChildren();
    if (!midi.ports.length) {
      this.el.midiPorts.append(
        this.emptyNote("No MIDI inputs. Plug the console in — the list finds it by itself.")
      );
    }
    for (const port of midi.ports) {
      const row = document.createElement("div");
      row.className = "midi-port";
      row.textContent = port.name;
      row.title = port.name;
      this.el.midiPorts.append(row);
    }
  }

  // ---- the compass popover: how far this manual reaches -------------------

  wireCompassForm() {
    this.el.compassClose.addEventListener("click", () => this.closeCompassForm());
  }

  openCompassForm(idx, x, y) {
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeTuningForm();
    this.closeHexForm();
    this.closeCouplerForm();
    this.closeSettingsPopovers();
    this.compassManual = idx;
    this.compassSignature = null;
    this.hideCompassError();
    this.syncCompassForm();
    this.el.compass.classList.remove("hidden");
    this.positionPopover(this.el.compass, x, y);
  }

  closeCompassForm() {
    this.compassManual = null;
    this.compassSignature = null;
    this.el.compass.classList.add("hidden");
    this.hideCompassError();
  }

  syncCompassForm() {
    const idx = this.compassManual;
    const manual = this.lastSnapshot?.manuals.find((m) => m.idx === idx);
    const compass = (this.lastSnapshot?.setup?.compass ?? []).find((c) => c.idx === idx);
    if (!manual || !compass) {
      this.closeCompassForm();
      return;
    }
    const signature = JSON.stringify([manual.name, compass]);
    if (signature === this.compassSignature) return;
    this.compassSignature = signature;
    this.el.compassTitle.textContent = `${manual.name} · compass`;
    this.el.compassRow.replaceChildren(this.compassRow(manual, compass));
  }

  /// One manual's compass: two editable bounds and the two ways to
  /// change them — type new values and press Set, or fall back to
  /// whatever the sample set itself declares.
  compassRow(manual, compass) {
    const row = document.createElement("div");
    row.className = "organ-compass-row";

    const low = this.compassField(compass.low ?? compass.native_low, compass.native_low);
    const high = this.compassField(compass.high ?? compass.native_high, compass.native_high);
    row.append(low.wrap, high.wrap);

    const set = document.createElement("button");
    set.className = "ghost";
    set.textContent = "Set";
    set.title = "Declare this manual's compass";
    set.addEventListener("click", () => {
      const lo = parseKeyName(low.input.value);
      const hi = parseKeyName(high.input.value);
      // A bound that doesn't name a note stays marked by its own field;
      // nothing is sent until both read as pitches.
      if (lo == null || hi == null) return;
      low.input.value = keyName(lo);
      high.input.value = keyName(hi);
      this.compassCommand(commands.organCompass(manual.idx, lo, hi));
    });
    row.append(set);

    if (compass.declared) {
      const native = document.createElement("button");
      native.className = "ghost";
      native.textContent = "Native";
      native.title = "Go back to the sample set's own compass";
      native.addEventListener("click", () =>
        this.compassCommand(commands.organCompass(manual.idx))
      );
      row.append(native);
    }

    return row;
  }

  /// A bound of the compass as a note name — "C2", "F♯4" — never as a
  /// MIDI number. The echo confirms what a nonstandard spelling ("bb2")
  /// reads as and flags text that names no note at all. Purely local
  /// until Set is pressed: typing here never sends anything.
  compassField(value, native) {
    const wrap = document.createElement("span");
    wrap.className = "compass-field";

    const input = document.createElement("input");
    input.type = "text";
    input.autocomplete = "off";
    input.spellcheck = false;
    input.value = keyName(value);
    input.placeholder = keyName(native);
    input.title = `Sample set's own: ${keyName(native)} · C4 is middle C`;

    const note = document.createElement("i");
    input.addEventListener("input", () => {
      const parsed = parseKeyName(input.value);
      input.classList.toggle("invalid", parsed == null);
      const canonical = parsed == null ? null : keyName(parsed);
      note.textContent = parsed == null ? "?" : canonical === input.value.trim() ? "" : canonical;
    });

    wrap.append(input, note);
    return { wrap, input };
  }

  async compassCommand(query) {
    this.hideCompassError();
    const { ok, error } = await this.organCommandResult(query);
    if (error != null) this.showCompassError(error);
    return ok;
  }

  showCompassError(text) {
    this.el.compassError.textContent = text;
    this.el.compassError.classList.remove("hidden");
  }

  hideCompassError() {
    this.el.compassError.classList.add("hidden");
    this.el.compassError.textContent = "";
  }

  // ---- the Room & noises popover: organ-wide sound character --------------
  //
  // Reverb wet and the mechanism noises are the organ's, not the
  // player's: both live in the organ's file and travel with it. The
  // sliders report live while they move (~30 commands/s) and persist
  // only on release, so a drag never writes the file per frame.

  wireRoomForm() {
    this.el.roomClose.addEventListener("click", () => this.closeRoomForm());

    this.throttledRoomSlider(this.el.roomReverb, "reverb", (persist) =>
      this.send(commands.reverb(this.el.roomReverb.value, persist))
    );
    const sendNoises = (persist) =>
      this.send(
        commands.noises(this.el.roomNoisesOn.checked, this.el.roomNoisesVol.value, persist)
      );
    this.el.roomNoisesOn.addEventListener("change", () => sendNoises(true));
    this.throttledRoomSlider(this.el.roomNoisesVol, "noises-vol", sendNoises);
  }

  /// A slider that reports while it moves: ~30 commands/s during the
  /// drag, one final, persisted value on release.
  throttledRoomSlider(slider, key, send) {
    let lastSent = 0;
    slider.addEventListener("pointerdown", () => this.roomDragging.add(key));
    slider.addEventListener("input", () => {
      const now = performance.now();
      if (now - lastSent > 33) {
        lastSent = now;
        send(false);
      }
    });
    slider.addEventListener("change", () => {
      this.roomDragging.delete(key);
      send(true);
    });
  }

  openRoomForm(x, y) {
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeTuningForm();
    this.closeHexForm();
    this.closeCouplerForm();
    this.closeSettingsPopovers();
    this.roomOpen = true;
    this.syncRoomForm();
    this.el.room.classList.remove("hidden");
    this.positionPopover(this.el.room, x, y);
  }

  closeRoomForm() {
    this.roomOpen = false;
    this.el.room.classList.add("hidden");
  }

  syncRoomForm() {
    const snapshot = this.lastSnapshot ?? {};
    this.el.roomReverbRow.classList.toggle("hidden", snapshot.reverb == null);
    if (snapshot.reverb != null && !this.roomDragging.has("reverb")) {
      this.el.roomReverb.value = snapshot.reverb;
    }
    this.el.roomNoisesRow.classList.toggle("hidden", !snapshot.noises);
    if (snapshot.noises) {
      this.el.roomNoisesOn.checked = snapshot.noises.on;
      if (!this.roomDragging.has("noises-vol")) {
        this.el.roomNoisesVol.value = snapshot.noises.vol;
      }
    }
  }

  // ---- the Bindings popover: the whole flat list --------------------------
  //
  // Every piston, pedal and key this organ answers to, in one place —
  // the piston rows on stop and coupler editors are filtered views
  // over this same list. Action-first, not manual-first: a binding
  // doesn't belong to a manual, so a flat list is the honest shape.

  wireBindingsForm() {
    this.el.bindingsClose.addEventListener("click", () => this.closeBindingsForm());
    // A new slot doesn't exist on the server until either a bind or a
    // learned trigger names it; learning one past the end is enough —
    // learn_control defaults a slot with nothing saved to "octave-up".
    this.el.bindingsAdd.addEventListener("click", () =>
      this.send(commands.controlLearn((this.lastSnapshot?.controls ?? []).length))
    );
  }

  openBindingsForm(x, y) {
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeTuningForm();
    this.closeHexForm();
    this.closeCouplerForm();
    this.closeSettingsPopovers();
    this.bindingsOpen = true;
    this.bindingsSignature = null;
    this.syncBindingsForm();
    this.el.bindings.classList.remove("hidden");
    this.positionPopover(this.el.bindings, x, y);
  }

  closeBindingsForm() {
    if (!this.bindingsOpen) return;
    // Same contract as the MIDI popover: leaving ends any wait for a key.
    if (this.lastSnapshot?.control_learning != null && !this.quickBind) {
      this.send(commands.controlLearn(null));
    }
    this.bindingsOpen = false;
    this.bindingsSignature = null;
    this.el.bindings.classList.add("hidden");
  }

  syncBindingsForm() {
    const snapshot = this.lastSnapshot;
    if (!snapshot) return;
    const learning = snapshot.control_learning ?? null;
    const signature = JSON.stringify([
      snapshot.controls ?? [],
      snapshot.actions ?? [],
      learning,
      (snapshot.stops ?? []).map((s) => s.name),
      (snapshot.couplers ?? []).map((c) => c.name),
      (snapshot.enclosures ?? []).map((e) => e.name),
      (snapshot.manuals ?? []).map((m) => m.name),
      snapshot.keyboard ?? null,
    ]);
    if (signature === this.bindingsSignature) return;
    this.bindingsSignature = signature;
    this.el.bindingsList.replaceChildren();
    buildControlsList(this.el.bindingsList, { snapshot, learning, send: this.send });
    this.el.bindingsKeyboard.textContent = keyboardNote(snapshot);
  }

  // ---- the save-as popover: an ad-hoc combination becomes a file ----------
  //
  // Opened from the organ-name menu, and once, automatically, for an
  // organ combined on the command line. Saving bypasses send()/poll:
  // a bad path has a specific, useful reason the server already wrote
  // out, and it belongs in this popover.

  wireSaveForm() {
    this.el.saveClose.addEventListener("click", () => this.closeSaveForm());
    this.el.saveBtn.addEventListener("click", () => this.saveOrgan());
    this.el.savePath.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        this.saveOrgan();
      }
    });
  }

  openSaveForm(x, y) {
    const setup = this.lastSnapshot?.setup;
    if (!setup || setup.file) return; // nothing unsaved to write
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeTuningForm();
    this.closeHexForm();
    this.closeCouplerForm();
    this.closeSettingsPopovers();
    this.saveOpen = true;
    this.el.savePath.value = "";
    this.hideSaveError();
    this.el.save.classList.remove("hidden");
    this.positionPopover(
      this.el.save,
      x ?? window.innerWidth / 2 - 180,
      y ?? 96
    );
    requestAnimationFrame(() => this.el.savePath.focus());
  }

  closeSaveForm() {
    this.saveOpen = false;
    this.el.save.classList.add("hidden");
    this.hideSaveError();
  }

  /// The popover only makes sense while the organ has no file — once
  /// the save lands (this session's or another's), it closes itself.
  syncSaveForm() {
    const setup = this.lastSnapshot?.setup;
    if (!setup || setup.file) this.closeSaveForm();
  }

  async saveOrgan() {
    const path = this.el.savePath.value.trim();
    if (!path) {
      this.showSaveError("Give it a path first.");
      return;
    }
    this.el.saveBtn.disabled = true;
    try {
      const response = await fetch(this.base + commands.organSave(path), { method: "POST" });
      if (!response.ok) {
        this.showSaveError((await response.text()) || `${response.status} ${response.statusText}`);
        return;
      }
      // The next poll picks up the now-saved organ; syncSaveForm sees
      // setup.file and closes the popover.
      this.hideSaveError();
    } catch (err) {
      this.showSaveError(String(err));
    } finally {
      this.el.saveBtn.disabled = false;
    }
  }

  showSaveError(text) {
    this.el.saveError.textContent = text;
    this.el.saveError.classList.remove("hidden");
  }

  hideSaveError() {
    this.el.saveError.classList.add("hidden");
    this.el.saveError.textContent = "";
  }

  // ---- the save-as dialog: a set's own organ becomes the player's --------
  //
  // A sample set's own organ (its file marked `adopted`) is kept exactly
  // as the set defines it: the server answers every change with 409,
  // and main.js routes that here with the refused command in hand.
  // Saving copies the file under the new name, the server switches to
  // the copy, and the refused command is sent again — so the player's
  // gesture lands after all, on an organ that is theirs. The same
  // dialog is the organ-name menu's "Save as…" for any organ with a
  // file, with nothing to replay.

  wireSaveAsForm() {
    for (const closer of this.el.saveAs.querySelectorAll("[data-close]")) {
      closer.addEventListener("click", () => this.closeSaveAsForm());
    }
    this.el.saveAsCancel.addEventListener("click", () => this.closeSaveAsForm());
    this.el.saveAsBtn.addEventListener("click", () => this.saveOrganAs());
    this.el.saveAsName.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        this.saveOrganAs();
      }
    });
  }

  /// `pending` is the refused command, if a refusal opened the dialog.
  openSaveAsForm(pending = null) {
    const snapshot = this.lastSnapshot;
    if (!snapshot?.setup?.file || !snapshot.organ) return;
    const organ = snapshot.organ;
    const adopted = Boolean(snapshot.setup.adopted);
    // A refused rename IS this dialog: the name goes on the copy, and
    // there is nothing left to send again.
    const rename = pending?.match(/^\/api\/organ\/rename\?name=([^&]*)/);
    this.saveAsPending = rename ? null : pending;
    this.saveAsFor = organ;
    this.saveAsOpen = true;
    this.closeSaveForm();
    const strong = (text) => Object.assign(document.createElement("strong"), { textContent: text });
    this.el.saveAsNote.replaceChildren(
      ...(adopted
        ? [
            strong(organ),
            " is the sample set's own organ, and Aristide keeps it exactly as the set " +
              "defines it. Save it under a different name and the copy is yours to change" +
              (pending && !rename ? " — this change and every one after it." : ".") +
              " The set's own organ stays as it was.",
          ]
        : [
            "Save a copy of ",
            strong(organ),
            " under a new name and carry on playing the copy. ",
            strong(organ),
            " stays as it is.",
          ])
    );
    this.el.saveAsName.value = rename
      ? decodeURIComponent(rename[1].replace(/\+/g, " "))
      : `My ${organ}`;
    this.hideSaveAsError();
    this.el.saveAs.classList.remove("hidden");
    this.root.body.classList.add("modal-open");
    requestAnimationFrame(() => {
      this.el.saveAsName.focus();
      this.el.saveAsName.select();
    });
  }

  closeSaveAsForm() {
    if (!this.saveAsOpen) return;
    this.saveAsOpen = false;
    this.saveAsPending = null;
    this.saveAsFor = null;
    this.el.saveAs.classList.add("hidden");
    this.root.body.classList.remove("modal-open");
    this.hideSaveAsError();
  }

  /// The dialog is about one organ: if another loads under it, or the
  /// one it is about has been saved elsewhere already, it no longer
  /// applies.
  syncSaveAsForm() {
    const snapshot = this.lastSnapshot;
    if (!snapshot?.setup?.file || snapshot.organ !== this.saveAsFor) this.closeSaveAsForm();
  }

  async saveOrganAs() {
    const name = this.el.saveAsName.value.trim();
    if (!name) {
      this.showSaveAsError("Give it a name first.");
      return;
    }
    if (name === this.saveAsFor) {
      this.showSaveAsError("Give the copy a name of its own.");
      return;
    }
    const pending = this.saveAsPending;
    this.el.saveAsBtn.disabled = true;
    try {
      const response = await fetch(this.base + commands.organSaveAs(name), { method: "POST" });
      if (!response.ok) {
        this.showSaveAsError((await response.text()) || `${response.status} ${response.statusText}`);
        return;
      }
      this.closeSaveAsForm();
      // The server has switched to the copy; the change it refused a
      // moment ago goes through now. The next poll shows the new name.
      if (pending) this.send(pending);
    } catch (err) {
      this.showSaveAsError(String(err));
    } finally {
      this.el.saveAsBtn.disabled = false;
    }
  }

  showSaveAsError(text) {
    this.el.saveAsError.textContent = text;
    this.el.saveAsError.classList.remove("hidden");
  }

  hideSaveAsError() {
    this.el.saveAsError.classList.add("hidden");
    this.el.saveAsError.textContent = "";
  }

  /// The settings popovers as a family, closed whenever another
  /// popover opens over them.
  closeSettingsPopovers() {
    this.closeMidiForm();
    this.closeCompassForm();
    this.closeRoomForm();
    this.closeBindingsForm();
    this.closeSaveForm();
  }

  // ---- the tremulant-shape popover: right-click the Tremblant knob --------
  //
  // A tremulant is a valve venting the wind, so its shape is spoken in
  // wind terms: rate, pitch depth in cents (gain and timbre follow
  // pressure physically), spin-up, unevenness. Every field posts on
  // change — live on the engine, written to the organ file's
  // [tremulant] section — and the next poll echoes what the server
  // settled, the tuning popover's contract.

  /// The Tremblant knob doubles as the tremulant's editor: right-click
  /// while unlocked (or ctrl through the lock) opens the shape popover.
  /// Wave tremulants offer no shape, so the gesture stays silent then.
  wireTremKnob() {
    const knob = this.root.querySelector('[data-key="trem"]');
    if (!knob) return;
    knob.addEventListener("contextmenu", (event) => {
      if (!(this.unlocked || event.ctrlKey)) return;
      if (!this.shapeableTrem()) return;
      event.preventDefault();
      event.stopPropagation();
      this.openTremForm(event.clientX, event.clientY);
    });
  }

  wireTremForm() {
    this.el.tremClose.addEventListener("click", () => this.closeTremForm());
    for (const [field, input] of [
      ["rate", this.el.tremRate],
      ["depth", this.el.tremDepth],
      ["ramp", this.el.tremRamp],
      ["wobble", this.el.tremWobble],
    ]) {
      input.addEventListener("change", () => {
        if (this.tremOpen == null) return;
        const value = Number(input.value);
        if (!Number.isFinite(value)) return;
        this.tremCommand({ idx: this.tremOpen, [field]: value });
      });
    }
  }

  /// The first shapeable tremulant — wave trems are recorded in their
  /// samples and offer nothing to edit.
  shapeableTrem() {
    return (this.lastSnapshot?.trems ?? []).find((t) => !t.wave) ?? null;
  }

  openTremForm(x, y) {
    const trem = this.shapeableTrem();
    if (!trem) return;
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeTuningForm();
    this.closeHexForm();
    this.closeCouplerForm();
    this.tremOpen = trem.idx;
    this.hideTremError();
    this.syncTremForm();
    this.el.trem.classList.remove("hidden");
    this.positionPopover(this.el.trem, x, y);
  }

  closeTremForm() {
    this.tremOpen = null;
    this.el.trem.classList.add("hidden");
    this.hideTremError();
  }

  syncTremForm() {
    const trem = (this.lastSnapshot?.trems ?? []).find((t) => t.idx === this.tremOpen);
    if (!trem || trem.wave) {
      this.closeTremForm();
      return;
    }
    this.el.tremTitle.textContent = trem.name;
    for (const [input, value] of [
      [this.el.tremRate, trem.rate],
      [this.el.tremDepth, trem.depth],
      [this.el.tremRamp, trem.ramp],
      [this.el.tremWobble, trem.wobble],
    ]) {
      if (this.root.activeElement !== input) input.value = value;
    }
  }

  async tremCommand(fields) {
    this.hideTremError();
    const { ok, error } = await this.organCommandResult(commands.tremParams(fields));
    if (error != null) this.showTremError(error);
    return ok;
  }

  showTremError(text) {
    this.el.tremError.textContent = text;
    this.el.tremError.classList.remove("hidden");
  }

  hideTremError() {
    this.el.tremError.classList.add("hidden");
    this.el.tremError.textContent = "";
  }

  // ---- the stop-editor popover: right-click any drawknob ------------------
  //
  // Name and voicing (footage, cents, gain) post live, field by field —
  // no rebuild, exactly the tuning popover's contract. Retargeting a
  // stop's source is structural, so picking one swaps a subview in over
  // the form (`openStopSrcView`/`closeStopSrcView`), the same idiom as
  // the tuning popover's own scale browser.

  wireStopForm() {
    this.el.stopClose.addEventListener("click", () => this.closeStopForm());
    this.el.stopSrcChange.addEventListener("click", () => this.openStopSrcView());
    this.el.stopSrcCancel.addEventListener("click", () => this.closeStopSrcView());

    // Deleting is the drag-to-bin gesture as a button: the stop comes
    // off the console, its source still offers it — no confirm, same
    // as the bin.
    this.el.stopDelete.addEventListener("click", () => {
      if (this.stopOpen == null) return;
      const id = this.stopOpen;
      this.closeStopForm();
      this.organCommand(commands.organUnpull(id));
    });

    // Every field commits on change already — Enter in the name field
    // must not also reload the page.
    this.el.stopForm.addEventListener("submit", (event) => event.preventDefault());

    this.el.stopName.addEventListener("change", () => {
      if (this.stopOpen == null) return;
      const stop = this.lastSnapshot?.stops.find((s) => s.id === this.stopOpen);
      const name = this.el.stopName.value.trim();
      if (!stop || !name || name === stop.name) return;
      // A hand-typed name supersedes any pending rename offer.
      this.hideStopLabelSync();
      this.stopCommand(commands.organStopRename(this.stopOpen, name));
    });

    this.el.stopFootage.addEventListener("change", async () => {
      if (this.stopOpen == null) return;
      const stop = this.lastSnapshot?.stops.find((s) => s.id === this.stopOpen);
      const text = this.el.stopFootage.value.trim();
      const ok = await this.stopCommand(commands.organStopVoice(this.stopOpen, { footage: text || "native" }));
      if (ok && stop) this.offerStopLabelSync(stop, text);
    });

    this.el.stopCents.addEventListener("change", () => {
      if (this.stopOpen == null) return;
      const cents = Number(this.el.stopCents.value);
      if (!Number.isFinite(cents)) return;
      this.stopCommand(commands.organStopVoice(this.stopOpen, { cents }));
    });

    this.el.stopGain.addEventListener("change", () => {
      if (this.stopOpen == null) return;
      const gain = Number(this.el.stopGain.value);
      if (!Number.isFinite(gain)) return;
      this.stopCommand(commands.organStopVoice(this.stopOpen, { gain }));
    });

    this.el.stopReset.addEventListener("click", async () => {
      if (this.stopOpen == null) return;
      const stop = this.lastSnapshot?.stops.find((s) => s.id === this.stopOpen);
      const ok = await this.stopCommand(commands.organStopVoice(this.stopOpen, { reset: 1 }));
      if (ok && stop) this.offerStopLabelSync(stop, "native");
    });

    // The rename offer's answers (see offerStopLabelSync). Yes renames
    // the stop to its name minus the footage tail — the server's
    // rename carries every file reference along — and, if a custom or
    // hidden engraving was set, returns it to auto so the knob face
    // reads the footage off the real pitch from now on. No remembers
    // the refusal for this stop so later edits don't nag.
    this.el.stopLabelSyncYes.addEventListener("click", async () => {
      const pending = this.stopLabelSync;
      this.hideStopLabelSync();
      if (!pending || pending.id !== this.stopOpen) return;
      const ok = await this.stopCommand(commands.organStopRename(pending.id, pending.base));
      if (ok && pending.relabel) {
        this.stopCommand(commands.organStopLabel(pending.id, { auto: 1 }));
      }
    });
    this.el.stopLabelSyncNo.addEventListener("click", () => {
      if (this.stopLabelSync) this.stopLabelSyncDeclined.add(this.stopLabelSync.id);
      this.hideStopLabelSync();
    });

    this.el.stopLabelMode.addEventListener("change", () => {
      if (this.stopOpen == null) return;
      const mode = this.el.stopLabelMode.value;
      this.el.stopLabelText.classList.toggle("hidden", mode !== "custom");
      if (mode === "auto") {
        this.stopCommand(commands.organStopLabel(this.stopOpen, { auto: 1 }));
      } else if (mode === "none") {
        this.stopCommand(commands.organStopLabel(this.stopOpen, { label: "" }));
      } else {
        // "custom" posts nothing yet — reveal the text field and let
        // the player type the engraving; it commits on its own change.
        this.el.stopLabelText.focus();
      }
    });

    this.el.stopLabelText.addEventListener("change", () => {
      if (this.stopOpen == null) return;
      this.stopCommand(commands.organStopLabel(this.stopOpen, { label: this.el.stopLabelText.value }));
    });

    this.el.stopOwnPipes.addEventListener("change", () => {
      if (this.stopOpen == null) return;
      this.stopCommand(commands.organStopOwnPipes(this.stopOpen, this.el.stopOwnPipes.checked));
    });

    this.el.stopTuningEdit.addEventListener("click", () => {
      if (this.stopOpen == null) return;
      const id = this.stopOpen;
      const rect = this.el.stop.getBoundingClientRect();
      this.closeStopForm();
      this.openTuningForm({ kind: "stop", id }, rect.left, rect.top);
    });
  }

  openStopForm(id, x, y) {
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeTuningForm();
    this.closeHexForm();
    this.closeTremForm();
    this.closeCouplerForm();
    this.closeSettingsPopovers();
    this.stopOpen = id;
    this.stopPistonsSignature = null;
    this.hideStopError();
    this.hideStopLabelSync();
    this.closeStopSrcView();
    this.syncStopForm();
    this.el.stop.classList.remove("hidden");
    this.positionPopover(this.el.stop, x, y);
  }

  closeStopForm() {
    this.stopOpen = null;
    this.el.stop.classList.add("hidden");
    this.hideStopError();
    this.hideStopLabelSync();
    this.closeStopSrcView();
  }

  /// Refills the form from the snapshot's stop entry — on open and on
  /// every later poll, so a rebuild's or another session's edit lands
  /// in the fields. Never touches the source-picker subview — a poll
  /// landing mid-navigation must not yank it shut (the tuning popover's
  /// browse idiom).
  syncStopForm() {
    const stop = this.lastSnapshot?.stops.find((s) => s.id === this.stopOpen);
    if (!stop) {
      this.closeStopForm();
      return;
    }
    this.el.stopTitle.textContent = stop.name;
    const pitch = stop.pitch ?? {};
    this.el.stopReset.classList.toggle("hidden", !pitch.own);

    if (this.root.activeElement !== this.el.stopName) this.el.stopName.value = stop.name;

    if (this.root.activeElement !== this.el.stopFootage) {
      this.el.stopFootage.value = formatFootage(pitch.footage ?? pitch.native);
    }
    // A mixture speaks several footages at once — there is no single
    // number the footage field could hold, so it's disabled and the
    // stop is voiced in cents alone.
    const mixture = pitch.native == null;
    this.el.stopFootage.disabled = mixture;
    this.el.stopFootage.title = mixture
      ? "A mixture speaks several footages — tune it in cents"
      : "";

    if (this.root.activeElement !== this.el.stopCents) this.el.stopCents.value = pitch.cents ?? 0;
    if (this.root.activeElement !== this.el.stopGain) this.el.stopGain.value = pitch.gain ?? 0;

    // label absent = auto, "" = hidden, anything else = that exact text.
    const labelMode = stop.label == null ? "auto" : stop.label === "" ? "none" : "custom";
    if (this.root.activeElement !== this.el.stopLabelMode) this.el.stopLabelMode.value = labelMode;
    this.el.stopLabelText.classList.toggle("hidden", labelMode !== "custom");
    if (labelMode === "custom" && this.root.activeElement !== this.el.stopLabelText) {
      this.el.stopLabelText.value = stop.label;
    }

    this.el.stopOwnPipes.checked = !!stop.own_pipes;

    const src = stop.src;
    this.el.stopSrc.textContent = src
      ? `${src.from} · ${src.manual}${src.stop ? ` · ${src.stop}` : ""}`
      : "—";

    this.el.stopTuningSummary.textContent = this.stopTuningLine(stop);

    // A mixture's individual ranks, only when there's more than one to
    // tell apart — a single-rank stop's tuning is the row above, in full.
    this.el.stopRanks.replaceChildren();
    const ranks = stop.ranks ?? [];
    if (ranks.length > 1) {
      for (const rank of ranks) {
        const row = document.createElement("div");
        row.className = "stop-rank-row";
        const name = document.createElement("span");
        name.className = "stop-rank-name";
        name.textContent = rank.name;
        const status = document.createElement("span");
        status.className = "stop-rank-status";
        status.textContent = rank.own
          ? `own · ${this.tuningLabel((this.lastSnapshot?.rank_tuning ?? []).find(
              (t) => t.stop === stop.id && t.rank === rank.id
            ))}`
          : "follows stop";
        const edit = document.createElement("button");
        edit.type = "button";
        edit.className = "ghost";
        edit.textContent = "Edit…";
        edit.addEventListener("click", () => {
          const rect = this.el.stop.getBoundingClientRect();
          this.closeStopForm();
          this.openTuningForm({ kind: "rank", stop: stop.id, rank: rank.id }, rect.left, rect.top);
        });
        row.append(name, status, edit);
        this.el.stopRanks.append(row);
      }
    }

    this.syncPistonRow(this.el.stopPistons, `stop:${stop.name}`, "stopPistonsSignature");
  }

  /// One popover's quick piston row, rebuilt only when the bindings it
  /// shows (or the quick-bind in flight) change — a poll must never
  /// recreate the Listen button under the pointer.
  syncPistonRow(container, action, signatureKey) {
    const listening = this.quickBind?.action === action && this.quickBind?.manual == null;
    const bound = (this.lastSnapshot?.controls ?? []).filter((c) => c.action === action);
    const signature = JSON.stringify([action, bound, listening]);
    if (signature === this[signatureKey]) return;
    this[signatureKey] = signature;
    container.replaceChildren(
      pistonRow(
        { snapshot: this.lastSnapshot, send: this.send, listening },
        action,
        (act, cancelling) => this.quickBindListen(act, null, cancelling)
      )
    );
  }

  /// Sends a stop field update directly (not through the app-wide
  /// `send()`), so a 400's reason lands in this popover rather than the
  /// global status strip — the same local-fetch idiom `tremCommand` uses.
  async stopCommand(query) {
    this.hideStopError();
    const { ok, error } = await this.organCommandResult(query);
    if (error != null) this.showStopError(error);
    return ok;
  }

  /// After a footage edit lands: if the stop's *name* still carries a
  /// footage tail that no longer reads as what the stop now speaks
  /// ("Montre 8'" revoiced to 16'), offer to move the footage out of
  /// the name. The knob face is already honest — auto engraving strips
  /// the name's tail and writes the real pitch — but the name itself
  /// would keep saying 8' in the popover title, piston bindings and
  /// stop lists. Yes renames to the bare name and returns a custom or
  /// hidden engraving to auto, so the footage is thereafter inferred
  /// from the pitch alone; the answer machinery is in wireStopForm.
  /// `text` is the footage the edit sent — "native" or the field's text.
  offerStopLabelSync(stop, text) {
    this.hideStopLabelSync();
    if (this.stopLabelSyncDeclined.has(stop.id)) return;
    const split = splitFootageName(stop.name);
    if (!split) return;
    const feet =
      !text || /^native$/i.test(text) ? stop.pitch?.native : parseFootage(text);
    if (feet == null || formatFootage(feet) === formatFootage(split.feet)) return;
    this.stopLabelSync = { id: stop.id, base: split.base, relabel: stop.label != null };
    const em = (words) => {
      const el = document.createElement("em");
      el.textContent = words;
      return el;
    };
    this.el.stopLabelSyncText.replaceChildren(
      "The name still says ",
      em(`${split.tail}`),
      " — rename the stop ",
      em(split.base),
      ` and engrave the ${formatFootage(feet)}' it now speaks?`
    );
    this.el.stopLabelSync.classList.remove("hidden");
  }

  hideStopLabelSync() {
    this.stopLabelSync = null;
    this.el.stopLabelSync.classList.add("hidden");
  }

  showStopError(text) {
    this.el.stopError.textContent = text;
    this.el.stopError.classList.remove("hidden");
  }

  hideStopError() {
    this.el.stopError.classList.add("hidden");
    this.el.stopError.textContent = "";
  }

  /// Swaps the source-picker subview in over the form: every source's
  /// every division's every stop, including already-pulled ones —
  /// retargeting a stop at one already on the console is legal
  /// borrowing, not a claim that has to be free first.
  async openStopSrcView() {
    if (this.stopOpen == null) return;
    this.stopSrcOpen = true;
    this.el.stopForm.classList.add("hidden");
    this.el.stopSrcView.classList.remove("hidden");
    this.el.stopSrcList.replaceChildren(this.emptyNote("Reading the sources…"));
    if (!this.offerings) await this.fetchOfferings(false);
    this.renderStopSrcList();
  }

  closeStopSrcView() {
    this.stopSrcOpen = false;
    this.el.stopSrcView.classList.add("hidden");
    this.el.stopForm.classList.remove("hidden");
  }

  renderStopSrcList() {
    this.el.stopSrcList.replaceChildren();
    const sources = this.offerings;
    if (sources == null) {
      this.el.stopSrcList.append(this.emptyNote("Couldn't read this organ's sources."));
      return;
    }
    const stop = this.lastSnapshot?.stops.find((s) => s.id === this.stopOpen);
    const current = stop?.src;
    let any = false;
    for (const source of sources) {
      for (const manual of source.manuals ?? []) {
        const stops = manual.stops ?? [];
        if (!stops.length) continue;
        any = true;
        const title = document.createElement("span");
        title.className = "organ-stop-group-title";
        title.textContent = `${source.alias} · ${manual.name}`;
        this.el.stopSrcList.append(title);
        for (const srcStop of stops) {
          const isCurrent =
            !!current &&
            current.from === source.alias &&
            current.manual === manual.name &&
            current.stop === srcStop.name;
          const row = document.createElement("button");
          row.type = "button";
          row.className = "menu-item";
          row.classList.toggle("checked", isCurrent);
          row.disabled = isCurrent;
          row.innerHTML = `<span>${srcStop.name}</span>`;
          row.addEventListener("click", async () => {
            if (this.stopOpen == null) return;
            const { ok, error } = await this.organCommandResult(
              commands.organStopSource(this.stopOpen, source.alias, manual.name, srcStop.name)
            );
            // The response is a snapshot mid-rebuild, not the settled
            // result — the popover stays open and the next poll's
            // syncStopForm() will re-sync its source line once the
            // rebuild lands.
            if (error != null) this.showStopError(error);
            else this.closeStopSrcView();
          });
          this.el.stopSrcList.append(row);
        }
      }
    }
    if (!any) {
      this.el.stopSrcList.append(this.emptyNote("The sources have nothing to offer."));
    }
  }

  // ---- the coupler-route popover: right-click any coupler rocker ----------
  //
  // Everything posts live, field by field, the stop popover's own
  // contract. A routes change always rebuilds the organ (the file's
  // coupler line is rewritten outright), so route edits go through a
  // coalescing queue: each change posts the whole array, an apply in
  // flight makes later changes wait, and only the newest state is ever
  // sent — clicking through three pitches costs one rebuild, not
  // three. Later polls refresh the title and name; the routes fold
  // back in from the server's echo only once an apply has settled and
  // the pointer is elsewhere (syncCouplerForm).

  wireCouplerForm() {
    this.el.couplerClose.addEventListener("click", () => this.closeCouplerForm());
    // Every field commits on its own change — Enter in the name field
    // must not also reload the page.
    this.el.couplerForm.addEventListener("submit", (event) => event.preventDefault());

    this.el.couplerName.addEventListener("change", () => {
      if (this.couplerOpen == null) return;
      const coupler = this.lastSnapshot?.couplers.find((c) => c.idx === this.couplerOpen);
      const name = this.el.couplerName.value.trim();
      if (!coupler || !name || name === coupler.name) return;
      this.couplerCommand(commands.organCouplerRename(this.couplerOpen, name));
    });

    this.el.couplerRouteAdd.addEventListener("click", () => {
      if (!this.couplerRoutes) return;
      this.couplerRoutes.push({ from: 0, to: 0, shift: 0 });
      this.renderCouplerRoutes();
      this.scheduleCouplerApply();
    });

    // The coupled-keys override — display only, live, the same
    // per-field contract as the name.
    this.el.couplerKeys.addEventListener("change", () => {
      if (this.couplerOpen == null) return;
      this.couplerCommand(commands.organCouplerKeys(this.couplerOpen, this.el.couplerKeys.value));
    });

    this.el.couplerDelete.addEventListener("click", () => {
      if (this.couplerOpen == null) return;
      const coupler = this.lastSnapshot?.couplers.find((c) => c.idx === this.couplerOpen);
      if (!coupler) return;
      // Skip the close-time duplicate nag — the coupler is leaving.
      this.couplerOpen = null;
      this.couplerRoutes = null;
      this.closeCouplerForm();
      this.showRemoveConfirm("coupler", { idx: coupler.idx, name: coupler.name });
    });
  }

  /// Queue the working copy for an auto-apply. Coalescing: the newest
  /// state replaces anything still waiting, so however many fields
  /// change while a rebuild is in flight, exactly one apply follows.
  scheduleCouplerApply() {
    if (this.couplerOpen == null || !this.couplerRoutes) return;
    this.couplerPending = {
      idx: this.couplerOpen,
      routes: structuredClone(this.couplerRoutes),
    };
    this.pumpCouplerApply();
  }

  /// Drains the pending apply, waiting out rebuilds the same way
  /// runQueue does — the server refuses structural edits mid-rebuild,
  /// and every apply here starts one. The pending edit is captured at
  /// schedule time, so it still lands if the popover closes meanwhile.
  async pumpCouplerApply() {
    if (this.couplerApplying) return;
    this.couplerApplying = true;
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    while (this.couplerPending) {
      const { idx, routes } = this.couplerPending;
      this.couplerPending = null;
      for (let attempt = 0; attempt < 40; attempt++) {
        while (this.lastSnapshot?.loading) await sleep(150);
        const ok = await this.couplerCommand(commands.organCouplerRoutes(idx, routes));
        if (ok) {
          this.couplerResync = true;
          break;
        }
        // A real refusal stays shown; only "still loading" retries.
        if (!/loading/i.test(this.el.couplerError.textContent)) break;
        this.hideCouplerError();
        await sleep(250);
      }
      // Give the poll a beat to notice the rebuild this apply started,
      // or the next iteration's wait would sail right past it.
      await sleep(300);
    }
    this.couplerApplying = false;
  }

  openCouplerForm(idx, x, y) {
    const coupler = this.lastSnapshot?.couplers.find((c) => c.idx === idx);
    if (!coupler) return;
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeTuningForm();
    this.closeHexForm();
    this.closeTremForm();
    this.closeStopForm();
    this.closeSettingsPopovers();
    this.couplerOpen = idx;
    this.couplerPistonsSignature = null;
    this.hideCouplerError();
    this.couplerRoutes = structuredClone(coupler.routes ?? []);
    this.renderCouplerRoutes();
    this.el.couplerTitle.textContent = coupler.name;
    if (this.root.activeElement !== this.el.couplerName) this.el.couplerName.value = coupler.name;
    this.el.coupler.classList.remove("hidden");
    this.positionPopover(this.el.coupler, x, y);
  }

  closeCouplerForm() {
    const idx = this.couplerOpen;
    const routes = this.couplerRoutes;
    this.couplerOpen = null;
    this.couplerRoutes = null;
    this.el.coupler.classList.add("hidden");
    this.hideCouplerError();
    // Editing done: if the routes now duplicate another coupler's,
    // offer the permanent link — the same warning adding a duplicate
    // gets, at the same "finished" moment rather than on every
    // transient state mid-edit.
    if (idx != null && routes) this.warnDuplicateCoupler(idx, routes);
  }

  /// The first other coupler whose routes do exactly what `routes` do
  /// — field-for-field, order-blind — or null. Hidden couplers don't
  /// count: they're off the console.
  duplicateCouplerOf(excludeIdx, routes) {
    const signature = (routes) =>
      JSON.stringify(
        (routes ?? [])
          .map((route) => [
            route.from ?? null,
            route.to ?? null,
            route.shift ?? 0,
            route.low ?? null,
            route.high ?? null,
            !!route.unison_off,
            route.scope ?? "",
            route.repitch ?? null,
            !!route.own_pipes,
          ])
          .map((fields) => JSON.stringify(fields))
          .sort()
      );
    const mine = signature(routes);
    return (
      (this.lastSnapshot?.couplers ?? []).find(
        (coupler) =>
          coupler.idx !== excludeIdx && !coupler.hidden && signature(coupler.routes) === mine
      ) ?? null
    );
  }

  warnDuplicateCoupler(idx, routes) {
    const coupler = this.lastSnapshot?.couplers.find((c) => c.idx === idx);
    if (!coupler) return;
    const twin = this.duplicateCouplerOf(idx, routes);
    if (!twin || (coupler.linked ?? []).includes(twin.idx)) return;
    this.showLinkConfirm(
      `${coupler.name} now does exactly what ${twin.name} does. Link them, ` +
        "so either control moves both?",
      () => this.runQueue([commands.organCouplerLink(idx, twin.idx, true)]),
      null
    );
  }

  /// Refreshes the title and (unless focused) the name field from the
  /// snapshot; the routes stay the local working copy until an
  /// auto-apply has settled, when the server's echo folds back in —
  /// but never while the pointer is in the route table, and never
  /// mid-queue. A coupler that vanished (removed from elsewhere)
  /// takes its popover with it.
  syncCouplerForm() {
    const coupler = this.lastSnapshot?.couplers.find((c) => c.idx === this.couplerOpen);
    if (!coupler) {
      this.closeCouplerForm();
      return;
    }
    this.el.couplerTitle.textContent = coupler.name;
    if (this.root.activeElement !== this.el.couplerName) this.el.couplerName.value = coupler.name;
    if (this.root.activeElement !== this.el.couplerKeys) {
      this.el.couplerKeys.value = coupler.keys ?? "auto";
    }
    this.syncPistonRow(this.el.couplerPistons, `coupler:${coupler.name}`, "couplerPistonsSignature");
    this.renderCouplerLinks(coupler);
    if (
      this.couplerResync &&
      !this.couplerApplying &&
      !this.couplerPending &&
      !this.lastSnapshot?.loading &&
      !this.el.couplerRoutesBox.contains(this.root.activeElement)
    ) {
      this.couplerResync = false;
      this.couplerRoutes = structuredClone(coupler.routes ?? []);
      this.renderCouplerRoutes();
    }
  }

  /// The popover's linked-partners lines: one per linked coupler,
  /// with its undo. Rebuilt from the snapshot on every sync — links
  /// change rarely and never under the pointer mid-gesture.
  renderCouplerLinks(coupler) {
    const box = this.el.couplerLinkedBox;
    box.replaceChildren();
    for (const linkedIdx of coupler.linked ?? []) {
      const partner = this.lastSnapshot?.couplers.find((c) => c.idx === linkedIdx);
      if (!partner) continue;
      const row = document.createElement("div");
      row.className = "coupler-linked";
      const label = document.createElement("span");
      label.textContent = `Linked with ${partner.name} — either control moves both.`;
      const unlink = document.createElement("button");
      unlink.type = "button";
      unlink.className = "ghost";
      unlink.textContent = "Unlink";
      unlink.addEventListener("click", () => {
        if (this.couplerOpen == null) return;
        this.couplerCommand(commands.organCouplerLink(this.couplerOpen, linkedIdx, false));
      });
      row.append(label, unlink);
      box.append(row);
    }
  }

  /// Rebuilds the route blocks from `this.couplerRoutes` — the local
  /// working copy, never the snapshot directly. Each route reads the
  /// way a coupler is named: "Swell to Great" means SWELL'S STOPS
  /// SOUND when the GREAT is played, so the row says "Sounds [Swell]
  /// on [Great] at [Sub-octave (16′)]" — the wire's from/to (played/
  /// sounding) stays under the hood. Fields this form doesn't expose
  /// (low/high/repitch) are left on the route object untouched, so
  /// every auto-apply round-trips them.
  renderCouplerRoutes() {
    const container = this.el.couplerRoutesBox;
    container.replaceChildren();
    const manuals = this.lastSnapshot?.manuals ?? [];
    const word = (text) => {
      const span = document.createElement("span");
      span.className = "rail-label";
      span.textContent = text;
      return span;
    };
    const options = (select, entries) => {
      for (const [value, text] of entries) {
        const opt = document.createElement("option");
        opt.value = value;
        opt.textContent = text;
        select.append(opt);
      }
    };
    const manualEntries = manuals.map((manual) => [String(manual.idx), manual.name]);

    this.couplerRoutes.forEach((route, i) => {
      const block = document.createElement("div");
      block.className = "coupler-route";

      // "Sounds <division> on <keyboard>" — the coupler's own word
      // order. Sounding nothing turns the route into a pure silencer
      // (the classic Unison Off stop), which needs no pitch either.
      const what = document.createElement("div");
      what.className = "coupler-route-row";
      const soundsSelect = document.createElement("select");
      options(soundsSelect, [...manualEntries, ["", "(nothing — silence)"]]);
      soundsSelect.value = route.to == null ? "" : String(route.to);
      const onSelect = document.createElement("select");
      onSelect.title = "The keyboard you play — where the coupler listens.";
      options(onSelect, manualEntries);
      onSelect.value = route.from == null ? "" : String(route.from);
      onSelect.addEventListener("change", () => {
        route.from = Number(onSelect.value);
        this.scheduleCouplerApply();
      });
      soundsSelect.title = "Whose stops speak — the division this coupler borrows.";
      soundsSelect.addEventListener("change", () => {
        if (soundsSelect.value === "") {
          route.to = null;
          // A route that sounds nothing must at least silence, or it
          // does nothing at all (the server refuses a dead line).
          route.unison_off = true;
        } else {
          route.to = Number(soundsSelect.value);
        }
        this.renderCouplerRoutes();
        this.scheduleCouplerApply();
      });
      what.append(word("Sounds"), soundsSelect, word("on"), onSelect);
      block.append(what);

      // "at <pitch>" — the organ's own words for the shift, with the
      // raw key count only for the odd coupler (a fourths coupler,
      // a quint) the presets don't name.
      if (route.to != null) {
        const at = document.createElement("div");
        at.className = "coupler-route-row";
        const pitchSelect = document.createElement("select");
        options(pitchSelect, [
          ["0", "Unison"],
          ["-12", "Sub-octave (16′)"],
          ["12", "Super-octave (4′)"],
          ["custom", "Other…"],
        ]);
        const keysInput = document.createElement("input");
        keysInput.type = "number";
        keysInput.min = "-24";
        keysInput.max = "24";
        keysInput.step = "1";
        keysInput.title = "Keys added to what you play: −12 an octave down, +7 a fifth up…";
        const keysWord = word("keys");
        const showKeys = (shown) => {
          keysInput.classList.toggle("hidden", !shown);
          keysWord.classList.toggle("hidden", !shown);
        };
        const shift = route.shift ?? 0;
        const preset = ["0", "-12", "12"].includes(String(shift));
        pitchSelect.value = preset ? String(shift) : "custom";
        keysInput.value = shift;
        showKeys(!preset);
        pitchSelect.addEventListener("change", () => {
          if (pitchSelect.value === "custom") {
            showKeys(true);
            keysInput.focus();
            return;
          }
          route.shift = Number(pitchSelect.value);
          keysInput.value = route.shift;
          showKeys(false);
          this.scheduleCouplerApply();
        });
        keysInput.addEventListener("change", () => {
          const value = Number(keysInput.value);
          if (!Number.isFinite(value)) return;
          route.shift = Math.round(value);
          this.scheduleCouplerApply();
        });
        at.append(word("at"), pitchSelect, keysInput, keysWord);
        block.append(at);
      }

      const how = document.createElement("div");
      how.className = "coupler-route-row";
      const scopeSelect = document.createElement("select");
      scopeSelect.title =
        "Which played keys couple: all of them, or only the lowest/highest " +
        "held — the intelligent Bass and Melody couplers.";
      options(scopeSelect, [
        ["", "every key"],
        ["bass", "lowest key held (Bass)"],
        ["melody", "highest key held (Melody)"],
      ]);
      scopeSelect.value = route.scope ?? "";
      scopeSelect.addEventListener("change", () => {
        if (scopeSelect.value) route.scope = scopeSelect.value;
        else delete route.scope;
        this.scheduleCouplerApply();
      });

      const unisonLabel = document.createElement("label");
      unisonLabel.title =
        "Silence the played keyboard's own stops here, so the note moves " +
        "instead of doubling.";
      const unisonCheck = document.createElement("input");
      unisonCheck.type = "checkbox";
      unisonCheck.checked = !!route.unison_off;
      unisonCheck.disabled = route.to == null; // a pure silencer must silence
      unisonCheck.addEventListener("change", () => {
        if (unisonCheck.checked) route.unison_off = true;
        else delete route.unison_off;
        this.scheduleCouplerApply();
      });
      unisonLabel.append(unisonCheck, document.createTextNode(" own stops off"));

      const ownPipesLabel = document.createElement("label");
      ownPipesLabel.title =
        "Speak an independent set of pipes — copies double pipes already " +
        "sounding instead of sharing them";
      const ownPipesCheck = document.createElement("input");
      ownPipesCheck.type = "checkbox";
      ownPipesCheck.checked = !!route.own_pipes;
      ownPipesCheck.addEventListener("change", () => {
        if (ownPipesCheck.checked) route.own_pipes = true;
        else delete route.own_pipes;
        this.scheduleCouplerApply();
      });
      ownPipesLabel.append(ownPipesCheck, document.createTextNode(" own pipes"));

      how.append(scopeSelect, unisonLabel, ownPipesLabel);

      if (this.couplerRoutes.length > 1) {
        const remove = document.createElement("button");
        remove.type = "button";
        remove.className = "ghost coupler-route-remove";
        remove.title = "Remove this route";
        remove.textContent = "×";
        remove.addEventListener("click", () => {
          this.couplerRoutes.splice(i, 1);
          this.renderCouplerRoutes();
          this.scheduleCouplerApply();
        });
        how.append(remove);
      }

      block.append(how);
      container.append(block);
    });
  }

  /// Sends a coupler field update directly (not through the app-wide
  /// `send()`), so a 400's reason lands in this popover rather than the
  /// global status strip — the same local-fetch idiom `stopCommand` uses.
  async couplerCommand(query) {
    this.hideCouplerError();
    const { ok, error } = await this.organCommandResult(query);
    if (error != null) this.showCouplerError(error);
    return ok;
  }

  showCouplerError(text) {
    this.el.couplerError.textContent = text;
    this.el.couplerError.classList.remove("hidden");
  }

  hideCouplerError() {
    this.el.couplerError.classList.add("hidden");
    this.el.couplerError.textContent = "";
  }

  // ---- the hex-layout popover: a microtonal manual's isomorphic grid ------
  //
  // Two step-vectors (right, up-right, in key-number steps), the grid
  // size, and the bottom-left key. Every field posts on change —
  // structural, so the keyboard redraws and the next snapshot echoes
  // back what the server settled on (a preset refits the width, wild
  // values clamp), keeping the form honest the same way the tuning
  // popover is.

  wireHexForm() {
    this.el.hexClose.addEventListener("click", () => this.closeHexForm());
    this.el.hexReset.addEventListener("click", () => this.hexCommand({ reset: 1 }));
    for (const button of this.el.hex.querySelectorAll("[data-preset]")) {
      button.addEventListener("click", () => this.hexCommand({ preset: button.dataset.preset }));
    }
    for (const [field, input] of [
      ["right", this.el.hexRight],
      ["upright", this.el.hexUpright],
      ["rows", this.el.hexRows],
      ["cols", this.el.hexCols],
    ]) {
      input.addEventListener("change", () => {
        if (this.hexManual == null) return;
        const value = Number(input.value);
        if (Number.isInteger(value)) this.hexCommand({ [field]: value });
      });
    }
    this.el.hexAnchor.addEventListener("change", () => {
      if (this.hexManual == null) return;
      // A note name ("C2") or a raw key number — numbers past MIDI's
      // 127 are legal on a widened manual, so they pass through.
      const text = this.el.hexAnchor.value.trim();
      const key = parseKeyName(text) ?? (/^\d+$/.test(text) ? Number(text) : null);
      if (key == null || key > 65535) {
        this.showHexError(`${text || "(empty)"} does not name a key`);
        return;
      }
      this.hexCommand({ anchor: key });
    });
  }

  openHexForm(idx, x, y) {
    this.closeAdd();
    this.closeDivisionMenu();
    this.closeKeyboardMenu();
    this.closeTuningForm();
    this.closeCouplerForm();
    this.hexManual = idx;
    this.hideHexError();
    this.syncHexForm();
    this.el.hex.classList.remove("hidden");
    this.positionPopover(this.el.hex, x, y);
  }

  closeHexForm() {
    this.hexManual = null;
    this.el.hex.classList.add("hidden");
    this.hideHexError();
  }

  /// Refills the form from the snapshot's effective layout — on open
  /// and on every poll, so the server's settling (clamps, refits,
  /// another session's edit) lands in the fields. A manual that
  /// stopped being microtonal takes its popover with it.
  syncHexForm() {
    const idx = this.hexManual;
    const manual = this.lastSnapshot?.manuals.find((m) => m.idx === idx);
    if (!manual?.hex) {
      this.closeHexForm();
      return;
    }
    this.el.hexTitle.textContent = `${manual.name} · hex layout`;
    const fields = [
      [this.el.hexRight, manual.hex.right],
      [this.el.hexUpright, manual.hex.upright],
      [this.el.hexRows, manual.hex.rows],
      [this.el.hexCols, manual.hex.cols],
      [
        this.el.hexAnchor,
        manual.hex.anchor <= 127 ? keyName(manual.hex.anchor) : String(manual.hex.anchor),
      ],
    ];
    for (const [input, value] of fields) {
      if (this.root.activeElement !== input) input.value = value;
    }
  }

  async hexCommand(fields) {
    if (this.hexManual == null) return false;
    this.hideHexError();
    const { ok, error } = await this.organCommandResult(
      commands.organManualHex(this.hexManual, fields)
    );
    if (error != null) this.showHexError(error);
    return ok;
  }

  showHexError(text) {
    this.el.hexError.textContent = text;
    this.el.hexError.classList.remove("hidden");
  }

  hideHexError() {
    this.el.hexError.classList.add("hidden");
    this.el.hexError.textContent = "";
  }

  // ---- the tuning popover's own file browser: picks a .scl or .kbm --------
  // path, the same /api/browse idiom as the add-source browse, filtered
  // client-side to the relevant extension (directories stay navigable).

  openTuningBrowse(kind) {
    this.tuningBrowseKind = kind;
    this.tuningBrowseDir = null;
    this.tuningBrowseParent = null;
    this.tuningBrowseEntries = null;
    this.tuningBrowseError = null;
    this.el.tuningBrowseTitle.textContent = kind === "keymap" ? "Choose a keymap" : "Choose a scale";
    this.el.tuningForm.classList.add("hidden");
    this.el.tuningBrowse.classList.remove("hidden");
    this.tuningBrowse();
  }

  closeTuningBrowse() {
    this.tuningBrowseKind = null;
    this.el.tuningBrowse.classList.add("hidden");
    this.el.tuningForm.classList.remove("hidden");
  }

  async tuningBrowse(dir) {
    try {
      const query = dir ? `/api/browse?dir=${encodeURIComponent(dir)}` : "/api/browse";
      const response = await fetch(this.base + query);
      if (!response.ok) {
        this.tuningBrowseError = (await response.text()) || `${response.status} ${response.statusText}`;
        this.renderTuningBrowse();
        return;
      }
      const data = await response.json();
      this.tuningBrowseDir = data.dir;
      this.tuningBrowseParent = data.parent;
      this.tuningBrowseEntries = data.entries;
      this.tuningBrowseError = null;
      this.renderTuningBrowse();
    } catch (err) {
      this.tuningBrowseError = String(err);
      this.renderTuningBrowse();
    }
  }

  renderTuningBrowse() {
    this.el.tuningBrowseDir.textContent = this.tuningBrowseDir ?? "";
    this.el.tuningBrowseDir.title = this.tuningBrowseDir ?? "";
    this.el.tuningBrowseUp.disabled = !this.tuningBrowseParent;
    this.el.tuningBrowseError.classList.toggle("hidden", !this.tuningBrowseError);
    this.el.tuningBrowseError.textContent = this.tuningBrowseError ?? "";
    this.el.tuningBrowseList.replaceChildren();
    if (this.tuningBrowseError) return;
    const ext = this.tuningBrowseKind === "keymap" ? ".kbm" : ".scl";
    const entries = (this.tuningBrowseEntries ?? []).filter(
      (entry) => entry.dir || entry.name.toLowerCase().endsWith(ext)
    );
    if (!entries.length) {
      this.el.tuningBrowseList.append(this.emptyNote("Nothing here."));
      return;
    }
    for (const entry of entries) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = entry.dir ? "picker-row picker-browse-dir" : "picker-row";
      row.title = entry.path;
      row.addEventListener("click", () => {
        if (entry.dir) this.tuningBrowse(entry.path);
        else this.pickTuningFile(entry.path);
      });
      const name = document.createElement("span");
      name.className = "picker-row-name";
      name.textContent = entry.name;
      row.append(name);
      this.el.tuningBrowseList.append(row);
    }
  }

  async pickTuningFile(path) {
    if (!this.tuningScope) return;
    const fields =
      this.tuningBrowseKind === "keymap"
        ? this.tuningFields({ scale: this.currentScalePath(), keymap: path })
        : this.tuningFields({ scale: path });
    if (fields.scale == null) return;
    const ok = await this.tuningCommand(fields);
    if (ok) this.closeTuningBrowse();
  }

  // ---- drag controller: plain when unlocked, ctrl-drag always -------------
  //
  // Plain pointer events, not HTML5 drag-and-drop: a floating label
  // follows the pointer and the drop target is read straight off
  // `elementFromPoint`. Every drag source waits for ~4px of movement
  // before committing to a drag — below that it's a click (a drawknob
  // still toggles its stop; a cheek's dblclick still renames).

  binAllowed(kind) {
    return kind === "stop" || kind === "manual" || kind === "enclosure" || kind === "coupler";
  }

  manualAllowed(kind) {
    return kind !== "enclosure";
  }

  encAllowed(kind) {
    return kind === "stop";
  }

  /// The kinds that live in a division's knob rank — the ones a drop
  /// on a jamb carries a position for.
  rankKind(kind) {
    return kind === "stop" || kind === "coupler";
  }

  /// The dragged control's rank token — the vocabulary the order
  /// endpoint and the snapshot's `rank` share.
  dragToken(drag) {
    return drag.kind === "coupler" ? `c${drag.payload.idx}` : `s${drag.payload.id}`;
  }

  /// A division's current display rank as tokens, from the snapshot —
  /// the list a reorder splices into, so seated couplers keep their
  /// places when a stop moves and vice versa.
  rankTokens(midx) {
    const manual = this.lastSnapshot?.manuals.find((m) => m.idx === midx);
    if (manual?.rank) return [...manual.rank];
    return (this.lastSnapshot?.stops ?? [])
      .filter((stop) => stop.midx === midx)
      .map((stop) => `s${stop.id}`);
  }

  /// The destination rank with the dragged control where the drop's
  /// seam showed — in front of `beforeToken`, or at the bottom when
  /// the drop carried no position (null, or a keyboard drop).
  spliceRank(midx, drag) {
    const token = this.dragToken(drag);
    const tokens = this.rankTokens(midx).filter((t) => t !== token);
    const before = drag.insert?.beforeToken ?? null;
    const at = before == null ? tokens.length : tokens.indexOf(before);
    tokens.splice(at < 0 ? tokens.length : at, 0, token);
    return tokens;
  }

  /// `getInfo()` returns `{kind, payload, label}` for the drag about to
  /// start, or null to refuse it. Called only once the pointer has
  /// actually moved past the threshold, so it can read live state.
  wireDragSource(el, getInfo) {
    el.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      if (!(event.ctrlKey || this.unlocked)) return;
      event.stopPropagation(); // a control drag is never a panel move
      const startX = event.clientX;
      const startY = event.clientY;
      let moved = false;
      const onMove = (e) => {
        if (moved) return;
        if (Math.hypot(e.clientX - startX, e.clientY - startY) < 4) return;
        moved = true;
        window.removeEventListener("pointermove", onMove);
        const info = getInfo();
        if (!info) return;
        el.addEventListener("click", suppressClick, { capture: true, once: true });
        this.startDrag(e, info.kind, info.payload, info.label);
      };
      const onUp = () => window.removeEventListener("pointermove", onMove);
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp, { once: true });
    });
  }

  startDrag(event, kind, payload, label) {
    event.preventDefault();
    const ghost = document.createElement("div");
    ghost.className = "organ-drag-ghost";
    ghost.textContent = label;
    document.body.append(ghost);
    this.drag = { kind, payload, ghost, label, targetType: null, targetIdx: null, insert: null };
    this.positionGhost(event.clientX, event.clientY);
    if (this.binAllowed(kind)) this.el.bin.classList.add("visible");
    this._dragMove = (e) => this.dragMove(e);
    window.addEventListener("pointermove", this._dragMove);
    window.addEventListener("pointerup", (e) => this.endDrag(e), { once: true });
  }

  positionGhost(x, y) {
    if (!this.drag) return;
    this.drag.ghost.style.left = `${x}px`;
    this.drag.ghost.style.top = `${y}px`;
  }

  dragMove(event) {
    if (!this.drag) return;
    this.positionGhost(event.clientX, event.clientY);
    this.applyDropHighlight(this.findDropTarget(event.clientX, event.clientY));
  }

  findDropTarget(x, y) {
    const el = document.elementFromPoint(x, y);
    if (!el || !this.drag) return null;
    if (el.closest("[data-drop-bin]") && this.binAllowed(this.drag.kind)) return { type: "bin" };
    const shoe = el.closest(".shoe[data-enclosure]");
    if (shoe && this.encAllowed(this.drag.kind)) {
      return { type: "shoe", idx: Number(shoe.dataset.enclosure) };
    }
    // The coupler rail takes a dragged coupler home: unseated from
    // whatever jamb held it, a tablet again.
    if (this.drag.kind === "coupler" && el.closest(".panel-couplers")) {
      return { type: "rail" };
    }
    const manual = el.closest("[data-drop-manual]");
    if (manual && this.manualAllowed(this.drag.kind)) {
      const hit = { type: "manual", idx: Number(manual.dataset.dropManual) };
      // Over a jamb division a dragged stop or coupler carries a
      // *position* too: where in the knob rank it would land. A
      // keyboard is a plain "onto this manual" target, as before.
      if (this.rankKind(this.drag.kind) && manual.classList.contains("division")) {
        hit.insert = this.insertionPoint(manual, x, y);
      }
      return hit;
    }
    return null;
  }

  /// Where in a division's knob rank the dragged control would land:
  /// the nearest rank knob — stop or seated coupler, the dragged one
  /// doesn't count — and which side of it the pointer sits, normalized
  /// to "before this token", with null meaning the bottom of the rank.
  /// Works unchanged when a resized jamb has wrapped the rank into
  /// columns: nearest-knob is a distance, not an index.
  insertionPoint(division, x, y) {
    const dragged =
      this.drag.kind === "coupler"
        ? `coupler-${this.drag.payload.idx}`
        : `stop-${this.drag.payload.id}`;
    const knobs = [
      ...division.querySelectorAll('.knob[data-key^="stop-"], .knob[data-key^="coupler-"]'),
    ].filter((knob) => knob.dataset.key !== dragged);
    if (!knobs.length) return { beforeToken: null, marker: null, side: "after" };
    let nearest = null;
    let best = Infinity;
    for (const knob of knobs) {
      const rect = knob.getBoundingClientRect();
      const dx = x - (rect.left + rect.width / 2);
      const dy = y - (rect.top + rect.height / 2);
      const d = dx * dx + dy * dy;
      if (d < best) {
        best = d;
        nearest = knob;
      }
    }
    const rect = nearest.getBoundingClientRect();
    // "stop-12" → "s12", "coupler-3" → "c3": the rank vocabulary.
    const token = (knob) =>
      knob.dataset.key.startsWith("coupler-")
        ? `c${knob.dataset.key.slice("coupler-".length)}`
        : `s${knob.dataset.key.slice("stop-".length)}`;
    // Which side of the nearest knob the pointer means: judged along
    // whichever axis it's further out on, so a wrapped grid reads
    // left/right within a row and above/below across rows — and the
    // seam is drawn on the matching edge.
    const dx = (x - (rect.left + rect.width / 2)) / rect.width;
    const dy = (y - (rect.top + rect.height / 2)) / rect.height;
    const before = Math.abs(dx) > Math.abs(dy) ? dx < 0 : dy < 0;
    const horizontal = Math.abs(dx) > Math.abs(dy);
    const side = horizontal ? (before ? "left" : "right") : before ? "before" : "after";
    if (before) {
      return { beforeToken: token(nearest), marker: nearest, side };
    }
    const next = knobs[knobs.indexOf(nearest) + 1] ?? null;
    return { beforeToken: next ? token(next) : null, marker: nearest, side };
  }

  applyDropHighlight(hit) {
    for (const el of this.root.querySelectorAll(".drop-target")) el.classList.remove("drop-target");
    for (const el of this.root.querySelectorAll(
      ".insert-before, .insert-after, .insert-left, .insert-right"
    )) {
      el.classList.remove("insert-before", "insert-after", "insert-left", "insert-right");
    }
    this.el.bin.classList.remove("drop-target");
    for (const el of this.root.querySelectorAll(".panel-couplers.drop-target")) {
      el.classList.remove("drop-target");
    }
    this.drag.targetType = hit?.type ?? null;
    this.drag.targetIdx = hit?.idx ?? null;
    this.drag.insert = hit?.insert
      ? { manual: hit.idx, beforeToken: hit.insert.beforeToken }
      : null;
    this.drag.ghost.textContent = this.drag.label;
    if (!hit) return;

    if (hit.type === "bin") {
      this.el.bin.classList.add("drop-target");
      this.drag.ghost.textContent =
        this.drag.kind === "enclosure"
          ? `Remove the ${this.drag.label} box`
          : this.drag.kind === "manual"
            ? `Remove ${this.drag.label}`
            : this.drag.kind === "coupler"
              ? `Delete the ${this.drag.label} coupler`
              : `Drop to remove ${this.drag.label}`;
      return;
    }

    // Home again: a coupler over the rail reads as its tablet's return
    // — unless it never left.
    if (hit.type === "rail") {
      if (this.drag.payload.midx == null) return;
      this.root.querySelector('.panel[data-panel="couplers"]')?.classList.add("drop-target");
      this.drag.ghost.textContent = `${this.drag.label} → the coupler rail`;
      return;
    }

    if (hit.type === "shoe") {
      const shoeEl = this.root.querySelector(`.shoe[data-enclosure="${hit.idx}"]`);
      shoeEl?.classList.add("drop-target");
      const enclosure = this.lastSnapshot?.enclosures.find((e) => e.idx === hit.idx);
      const stop = this.lastSnapshot?.stops.find((s) => s.id === this.drag.payload.id);
      if (enclosure) {
        const already = stop?.enc?.includes(hit.idx);
        this.drag.ghost.textContent = already
          ? `In ${enclosure.name} — drop to take out`
          : `Drop to add to ${enclosure.name}`;
      }
      return;
    }

    // Over a jamb division a dragged stop or coupler shows where it
    // would land — a seam beside the nearest knob — whether it's
    // coming home to its own rank (a pure reorder) or arriving from
    // the rail or another manual.
    if (this.rankKind(this.drag.kind) && hit.insert) {
      hit.insert.marker?.classList.add(`insert-${hit.insert.side}`);
      const manual = this.lastSnapshot?.manuals.find((m) => m.idx === hit.idx);
      this.drag.ghost.textContent =
        hit.idx === this.drag.payload.midx
          ? `Place ${this.drag.label} here`
          : `${this.drag.label} → ${manual?.name ?? "here"}`;
      return;
    }

    // Dropping a stop back on its own manual, or a manual's cheek on its
    // own board, isn't a move — no need to light it up as one.
    if (this.rankKind(this.drag.kind) && hit.idx === this.drag.payload.midx) return;
    if (this.drag.kind === "manual" && hit.idx === this.drag.payload.idx) return;
    for (const el of this.root.querySelectorAll(`[data-drop-manual="${hit.idx}"]`)) {
      el.classList.add("drop-target");
    }
    const manual = this.lastSnapshot?.manuals.find((m) => m.idx === hit.idx);
    if (manual) this.drag.ghost.textContent = `${this.drag.label} → ${manual.name}`;
  }

  endDrag(event) {
    window.removeEventListener("pointermove", this._dragMove);
    const drag = this.drag;
    this.drag = null;
    if (!drag) return;
    drag.ghost.remove();
    this.el.bin.classList.remove("visible", "drop-target");
    for (const el of this.root.querySelectorAll(".drop-target")) el.classList.remove("drop-target");
    for (const el of this.root.querySelectorAll(
      ".insert-before, .insert-after, .insert-left, .insert-right"
    )) {
      el.classList.remove("insert-before", "insert-after", "insert-left", "insert-right");
    }

    const { targetType, targetIdx } = drag;
    if (!targetType) return;

    if (drag.kind === "stop") {
      if (targetType === "bin") {
        this.organCommand(commands.organUnpull(drag.payload.id));
      } else if (targetType === "shoe") {
        const enclosure = this.lastSnapshot?.enclosures.find((e) => e.idx === targetIdx);
        const stop = this.lastSnapshot?.stops.find((s) => s.id === drag.payload.id);
        if (enclosure) {
          const already = stop?.enc?.includes(targetIdx);
          this.organCommand(commands.organEnclosureAssign(enclosure.name, drag.payload.id, !already));
        }
      } else if (targetType === "manual") {
        const sameManual = targetIdx === drag.payload.midx;
        if (drag.insert && drag.insert.manual === targetIdx) {
          // The drop carried a position: deal the destination rank out
          // anew with the dragged stop where the seam showed — tokens,
          // so any couplers seated in the rank keep their places.
          const tokens = this.spliceRank(targetIdx, drag);
          if (sameManual) {
            this.organCommand(commands.organRankOrder(targetIdx, tokens));
          } else {
            // Arriving from another manual: move first (live), then
            // place — the queue waits each response out, and refusals
            // surface like any other edit's.
            this.runQueue([
              commands.organMove(drag.payload.id, targetIdx),
              commands.organRankOrder(targetIdx, tokens),
            ]);
          }
        } else if (!sameManual) {
          // A keyboard drop names no position — the stop joins the
          // manual at the bottom of its rank, as it always has. Live,
          // but the server refuses it mid-rebuild (stale names would
          // poison the file), so it goes through the queue.
          this.runQueue([commands.organMove(drag.payload.id, targetIdx)]);
        }
      }
    } else if (drag.kind === "coupler") {
      if (targetType === "bin") {
        this.showRemoveConfirm("coupler", drag.payload);
      } else if (targetType === "rail") {
        // Home to the rail: the seat's division deals its rank out
        // without the coupler, which unseats it.
        if (drag.payload.midx != null) {
          const token = this.dragToken(drag);
          const tokens = this.rankTokens(drag.payload.midx).filter((t) => t !== token);
          this.organCommand(commands.organRankOrder(drag.payload.midx, tokens));
        }
      } else if (targetType === "manual") {
        // Seat it in the jamb where the seam showed — or, from a
        // keyboard drop, at the bottom of that division's rank. The
        // server unseats it everywhere else.
        this.organCommand(
          commands.organRankOrder(targetIdx, this.spliceRank(targetIdx, drag))
        );
      }
    } else if (drag.kind === "manual") {
      if (targetType === "bin") {
        this.showRemoveConfirm("manual", drag.payload);
      } else if (targetType === "manual" && targetIdx !== drag.payload.idx) {
        this.organCommand(commands.organManualOrder(drag.payload.idx, targetIdx));
      }
    } else if (drag.kind === "enclosure" && targetType === "bin") {
      this.showRemoveConfirm("enclosure", drag.payload);
    } else if (drag.kind === "offering-stop" && targetType === "manual") {
      const manual = this.lastSnapshot?.manuals.find((m) => m.idx === targetIdx);
      if (manual) {
        this.organCommand(
          commands.organPull(drag.payload.alias, drag.payload.manualName, manual.name, drag.payload.stopName)
        );
      }
    } else if (drag.kind === "offering-division" && targetType === "manual") {
      const manual = this.lastSnapshot?.manuals.find((m) => m.idx === targetIdx);
      if (manual) this.organCommand(commands.organPull(drag.payload.alias, drag.payload.manualName, manual.name));
    }
  }

  // ---- removal: manuals and swell boxes, both confirmed the same way -----

  showRemoveConfirm(kind, payload) {
    this.pendingRemove = { kind, ...payload };
    const n = payload.stopCount;
    this.el.removeConfirmText.textContent =
      kind === "enclosure"
        ? `Remove the ${payload.name} box? Its stops stay, unenclosed.`
        : kind === "coupler"
          ? `Delete the ${payload.name} coupler? A sample set's own goes ` +
            "off the console instead, restorable from the add menu."
          : `Remove ${payload.name} and its ${n} stop${n === 1 ? "" : "s"}? ` +
            "Sources still offer everything.";
    this.el.removeConfirm.classList.remove("hidden");
  }

  hideRemoveConfirm() {
    this.pendingRemove = null;
    this.el.removeConfirm.classList.add("hidden");
  }

  wireRemoveConfirm() {
    this.el.removeConfirmYes.addEventListener("click", () => {
      const target = this.pendingRemove;
      this.hideRemoveConfirm();
      if (!target) return;
      if (target.kind === "enclosure") this.organCommand(commands.organEnclosureRemove(target.name));
      else if (target.kind === "coupler") this.organCommand(commands.organCouplerRemove(target.idx));
      else this.organCommand(commands.organManualRemove(target.idx));
    });
    this.el.removeConfirmNo.addEventListener("click", () => this.hideRemoveConfirm());
  }

  // ---- the duplicate-coupler dialog: warn, and offer the link -------------
  //
  // Two couplers doing exactly the same thing is usually a console
  // convenience (a thumb piston and a toe stud for one action), so the
  // deliberate answer is a permanent link: either control moves both.
  // The dialog offers it wherever a duplicate is born — adding one, or
  // editing one into sameness.

  /// `onYes` runs on "Link them"; `onNo` (optional) on "Keep separate".
  showLinkConfirm(text, onYes, onNo) {
    this.pendingLink = { onYes, onNo };
    this.el.linkConfirmText.textContent = text;
    this.el.linkConfirm.classList.remove("hidden");
  }

  wireLinkConfirm() {
    const answer = (yes) => {
      const pending = this.pendingLink;
      this.pendingLink = null;
      this.el.linkConfirm.classList.add("hidden");
      if (!pending) return;
      (yes ? pending.onYes : pending.onNo)?.();
    };
    this.el.linkConfirmYes.addEventListener("click", () => answer(true));
    this.el.linkConfirmNo.addEventListener("click", () => answer(false));
  }

  /// Add a coupler and link it to its twin: the add rebuilds the
  /// organ, so the link waits until both wear console indexes.
  async addCouplerLinked(name, routes, twinName) {
    await this.runQueue([commands.organCouplerAdd(name, routes)]);
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    for (let attempt = 0; attempt < 60; attempt++) {
      const couplers = this.lastSnapshot?.couplers ?? [];
      const added = couplers.find((c) => c.name === name);
      const twin = couplers.find((c) => c.name === twinName);
      if (added && twin) {
        this.runQueue([commands.organCouplerLink(added.idx, twin.idx, true)]);
        return;
      }
      await sleep(200);
    }
  }

  // ---- organ edits: a fetch of their own, not send()/poll ------------------
  //
  // A structural edit can 400 with a specific, useful reason (a
  // duplicate name, a load already running) worth showing exactly, and
  // it doesn't land immediately — the server answers with a snapshot
  // mid-rebuild, and the real result arrives over the ordinary poll once
  // `loading` clears.

  async organCommand(query) {
    const { ok, error } = await this.organCommandResult(query);
    if (error != null) this.showError(error);
    return ok;
  }

  /// A 409 is the sample set's own organ refusing to change: not an
  /// error to show, but the save-as dialog to open with the refused
  /// command in hand (see openSaveAsForm). True when handled that way.
  deferToSaveAs(response, query) {
    if (response.status !== 409) return false;
    this.openSaveAsForm(query);
    return true;
  }

  /// `error` is null when nothing is the caller's to show — the edit
  /// went through, or the save-as dialog has taken it over.
  async organCommandResult(query) {
    this.hideError();
    try {
      const response = await fetch(this.base + query, { method: "POST" });
      if (!response.ok) {
        if (this.deferToSaveAs(response, query)) return { ok: false, error: null };
        return { ok: false, error: (await response.text()) || `${response.status} ${response.statusText}` };
      }
      // Any successful edit can change what the sources offer (a new
      // source, a pull claiming a stop) — the cached offerings are
      // stale now whether or not the drawer is up to show it. The
      // division menu reads this cache, so a kept stale [] would keep
      // insisting there are no sources right after one was added.
      this.offerings = null;
      if (this.drawerOpen) this.fetchOfferings();
      return { ok: true, error: null };
    } catch (err) {
      return { ok: false, error: String(err) };
    }
  }

  /// Runs structural edits back to back. Each one rebuilds the organ,
  /// and the server refuses edits while a rebuild is in flight — so
  /// between commands this waits out `loading` (as the poll reports
  /// it), and a "still loading" refusal is retried rather than shown.
  async runQueue(queue) {
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    for (const query of queue) {
      for (let attempt = 0; attempt < 40; attempt++) {
        while (this.lastSnapshot?.loading) await sleep(150);
        const { ok, error } = await this.organCommandResult(query);
        if (ok) break;
        if (error == null) return; // the save-as dialog has it now
        if (!/loading/i.test(error ?? "")) {
          this.showError(error);
          return;
        }
        await sleep(250);
      }
      // Give the poll a beat to notice the rebuild this command started,
      // or the next iteration's wait would sail right past it.
      await sleep(300);
    }
  }

  showError(text) {
    this.el.error.textContent = text;
    this.el.error.classList.remove("hidden");
  }

  hideError() {
    this.el.error.classList.add("hidden");
    this.el.error.textContent = "";
  }

  // ---- the library drawer: what each source offers, and what's pulled -----

  wireDrawer() {
    this.el.drawerTab.addEventListener("click", () => this.toggleDrawer());
    this.el.drawerClose.addEventListener("click", () => this.closeDrawer());
  }

  toggleDrawer() {
    if (this.drawerOpen) this.closeDrawer();
    else this.openDrawer();
  }

  openDrawer() {
    this.drawerOpen = true;
    this.el.drawer.classList.remove("hidden");
    this.fetchOfferings();
  }

  closeDrawer() {
    this.drawerOpen = false;
    this.el.drawer.classList.add("hidden");
  }

  async fetchOfferings(render = true) {
    try {
      const response = await fetch(this.base + commands.organOfferings());
      this.offerings = response.ok ? ((await response.json()).sources ?? []) : null;
    } catch {
      this.offerings = null;
    }
    if (render) this.buildOfferings(this.offerings);
  }

  buildOfferings(sources) {
    const container = this.el.offerings;
    container.replaceChildren();
    if (sources == null) {
      container.append(this.emptyNote("Couldn't read this organ's sources."));
      return;
    }
    if (!sources.length) {
      container.append(
        this.emptyNote("No sources yet — double-click the console to add a sample set.")
      );
      return;
    }
    for (const source of sources) container.append(this.offeringSourceRow(source));
  }

  emptyNote(text) {
    const p = document.createElement("p");
    p.className = "pane-empty";
    p.textContent = text;
    return p;
  }

  offeringSourceRow(source) {
    const details = document.createElement("details");
    details.className = "organ-offerings-source";
    details.open = true;

    const summary = document.createElement("summary");
    const alias = document.createElement("span");
    alias.className = "organ-offerings-alias";
    alias.textContent = source.alias;
    const name = document.createElement("span");
    name.className = "organ-offerings-name";
    name.textContent = source.name ?? "(unreadable)";
    const path = document.createElement("span");
    path.className = "organ-offerings-path";
    path.textContent = source.path;
    path.title = source.path;
    summary.append(alias, name, path);

    // This set's own tuning, or "follows instrument" — a small mono
    // chip through to its tuning popover (the stop/division chips' own
    // idiom). A native <summary> toggles its <details> on any click, so
    // the chip has to swallow its own.
    const own = (this.lastSnapshot?.source_tuning ?? []).find((t) => t.source === source.alias);
    const tuningChip = document.createElement("button");
    tuningChip.type = "button";
    tuningChip.className = "organ-offerings-tuning";
    // The drawer is narrow: the chip names the tuning, the tooltip
    // carries the anchor.
    tuningChip.textContent = own ? this.tuningLabel(own) : "follows instrument";
    tuningChip.title = own ? this.tuningSummary(own) : "Follows the instrument's tuning";
    tuningChip.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      const rect = tuningChip.getBoundingClientRect();
      this.openTuningForm({ kind: "source", alias: source.alias }, rect.left, rect.bottom + 6);
    });
    summary.append(tuningChip);
    details.append(summary);

    if (source.error) {
      const error = document.createElement("p");
      error.className = "organ-offerings-error";
      error.textContent = source.error;
      details.append(error);
      return details;
    }

    const body = document.createElement("div");
    body.className = "organ-offerings-body";
    for (const manual of source.manuals ?? []) body.append(this.offeringDivision(source.alias, manual));
    details.append(body);
    return details;
  }

  offeringDivision(alias, manual) {
    const div = document.createElement("div");
    div.className = "organ-offerings-division";

    const head = document.createElement("div");
    head.className = "organ-offerings-division-head";
    if (!manual.pulled) {
      this.wireDragSource(head, () => ({
        kind: "offering-division",
        payload: { alias, manualName: manual.name },
        label: `${manual.name} (whole division)`,
      }));
    }
    const title = document.createElement("span");
    title.className = "organ-stop-group-title";
    title.textContent = manual.name;
    head.append(title);
    if (manual.pedal) {
      const tag = document.createElement("span");
      tag.className = "organ-manual-pedal-tag";
      tag.textContent = "pedal";
      head.append(tag);
    }
    if (manual.kind === "microtonal") {
      const tag = document.createElement("span");
      tag.className = "organ-manual-pedal-tag";
      tag.textContent = "microtonal";
      head.append(tag);
    }
    if (manual.pulled) {
      const tag = document.createElement("span");
      tag.className = "organ-manual-pedal-tag";
      tag.textContent = "pulled";
      head.append(tag);
    }
    div.append(head);

    for (const stop of manual.stops ?? []) div.append(this.offeringStop(alias, manual.name, stop));
    return div;
  }

  offeringStop(alias, manualName, stop) {
    const row = document.createElement("div");
    row.className = "organ-offerings-stop";
    row.classList.toggle("pulled", !!stop.pulled);
    if (!stop.pulled) {
      this.wireDragSource(row, () => ({
        kind: "offering-stop",
        payload: { alias, manualName, stopName: stop.name },
        label: stop.name,
      }));
    }
    const check = document.createElement("span");
    check.className = "organ-offerings-stop-check";
    check.textContent = stop.pulled ? "✓" : "";
    const name = document.createElement("span");
    name.textContent = stop.name;
    row.append(check, name);
    return row;
  }

  // ---- adding to the organ: double-click the canvas -----------------------
  //
  // The Max gesture: double-click empty canvas (unlocked — or
  // ctrl-double-click through the lock) and the add menu opens where
  // you clicked. A manual or pedalboard added this way lands its
  // panels at that spot, via `pendingPlace` once the rebuild settles.

  wireCanvas() {
    // Double-click and right-click on empty space are the same gesture:
    // the add menu, or — locked — the padlock's nudge. (The webview's
    // own context menu is suppressed page-wide in main.js, so right-
    // click is ours to answer.) The empty-organ card floats over the
    // canvas and its copy says "double-click anywhere" — the card
    // itself must count too, or the instruction swallows its own
    // gesture; only its button stays out.
    const addGesture = (event) => {
      event.preventDefault();
      if (!(this.unlocked || event.ctrlKey)) {
        this.nudgeUnlock();
        return;
      }
      this.openAddMenu(event.clientX, event.clientY);
    };
    for (const type of ["dblclick", "contextmenu"]) {
      this.el.canvas.addEventListener(type, (event) => {
        if (event.target !== this.el.canvas) return; // empty space only
        addGesture(event);
      });
      this.el.emptyCard.addEventListener(type, (event) => {
        if (event.target.closest("button")) return;
        addGesture(event);
      });
    }
    // Popovers close on a click anywhere outside themselves.
    for (const el of [
      this.el.add,
      this.el.divisionMenu,
      this.el.keyboardMenu,
      this.el.tuning,
      this.el.hex,
      this.el.trem,
      this.el.stop,
      this.el.coupler,
      this.el.couplersMenu,
      this.el.midi,
      this.el.compass,
      this.el.room,
      this.el.bindings,
      this.el.save,
    ]) {
      el.addEventListener("click", (event) => event.stopPropagation());
    }
    window.addEventListener("click", () => {
      this.closeAdd();
      this.closeDivisionMenu();
      this.closeKeyboardMenu();
      this.closeTuningForm();
      this.closeHexForm();
      this.closeTremForm();
      this.closeStopForm();
      this.closeCouplerForm();
      this.closeCouplersMenu();
      this.closeSettingsPopovers();
    });
    window.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      // The dialog is modal: Escape means it, and nothing under it.
      if (this.saveAsOpen) {
        event.preventDefault();
        this.closeSaveAsForm();
        return;
      }
      this.closeAdd();
      this.closeDivisionMenu();
      this.closeKeyboardMenu();
      this.closeTuningForm();
      this.closeHexForm();
      this.closeTremForm();
      this.closeStopForm();
      this.closeCouplerForm();
      this.closeCouplersMenu();
      this.closeSettingsPopovers();
    });
  }

  openAddMenu(x, y) {
    this.closeDivisionMenu();
    this.closeCouplerForm();
    this.closeSettingsPopovers();
    this.addAnchor = { x, y };
    this.closeAddPanels();
    this.el.add.classList.remove("hidden");
    this.el.addMenu.classList.remove("hidden");
    this.positionPopover(this.el.add, x, y);
  }

  positionPopover(el, x, y) {
    el.style.left = "0px";
    el.style.top = "0px";
    const { width, height } = el.getBoundingClientRect();
    el.style.left = `${Math.max(8, Math.min(x, window.innerWidth - width - 8))}px`;
    el.style.top = `${Math.max(8, Math.min(y, window.innerHeight - height - 8))}px`;
  }

  closeAdd() {
    this.el.add.classList.add("hidden");
    this.closeAddPanels();
  }

  closeAddPanels() {
    this.el.addMenu.classList.add("hidden");
    this.el.addManualForm.classList.add("hidden");
    this.el.addEncForm.classList.add("hidden");
    this.el.addCouplerForm.classList.add("hidden");
    this.el.addSourceForm.classList.add("hidden");
  }

  wireAdd() {
    this.el.addManual.addEventListener("click", () => this.openManualForm("manual"));
    this.el.addPedal.addEventListener("click", () => this.openManualForm("pedal"));
    this.el.addMicrotonal.addEventListener("click", () => this.openManualForm("microtonal"));
    this.el.addEnc.addEventListener("click", () => this.openEncForm());
    this.el.addCoupler.addEventListener("click", () => this.openCouplerAddForm());
    this.el.addSource.addEventListener("click", () => this.openSourceForm());
    this.el.addManualCancel.addEventListener("click", () => this.closeAdd());
    this.el.addEncCancel.addEventListener("click", () => this.closeAdd());
    this.el.addCouplerCancel.addEventListener("click", () => this.closeAdd());
    this.el.addSourceCancel.addEventListener("click", () => this.closeAdd());

    this.el.addManualForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const name = this.el.addManualName.value.trim();
      if (!name) return;
      // The bounds are note names ("C2", "F♯4"), the same reading as the
      // compass fields in Preferences; a field naming no note keeps the
      // form open rather than guessing a compass.
      const low = parseKeyName(this.el.addManualLow.value);
      const high = parseKeyName(this.el.addManualHigh.value);
      this.el.addManualLow.classList.toggle("invalid", low == null);
      this.el.addManualHigh.classList.toggle("invalid", high == null);
      if (low == null || high == null) return;
      this.organCommand(commands.organManualAdd(name, low, high, this.addKind)).then((ok) => {
        if (!ok) return;
        this.rememberPlacement(name);
        this.closeAdd();
      });
    });

    this.el.addEncForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const name = this.el.addEncName.value.trim();
      if (!name) return;
      this.organCommand(commands.organEnclosureAdd(name)).then((ok) => ok && this.closeAdd());
    });

    // The name follows the selection ("Swell to Great", "16′ Swell to
    // Great") until the player types one of their own — see
    // suggestCouplerName.
    for (const select of [this.el.addCouplerSounds, this.el.addCouplerOn, this.el.addCouplerAt]) {
      select.addEventListener("change", () => this.suggestCouplerName());
    }
    this.el.addCouplerName.addEventListener("input", () => {
      this.addCouplerNamed = this.el.addCouplerName.value.trim() !== "";
    });
    this.el.addCouplerForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const name = this.el.addCouplerName.value.trim();
      if (!name) return;
      // Spoken order to wire order: what SOUNDS is the route's target,
      // what it's played ON is where the route listens.
      const to = Number(this.el.addCouplerSounds.value);
      const from = Number(this.el.addCouplerOn.value);
      const shift = Number(this.el.addCouplerAt.value) || 0;
      if (!Number.isFinite(from) || !Number.isFinite(to)) return;
      const routes = [{ from, to, shift }];
      // A coupler that duplicates an existing one gets the warning —
      // and, accepted, a permanent link: either control moves both.
      const twin = this.duplicateCouplerOf(null, routes);
      if (twin) {
        this.closeAdd();
        this.showLinkConfirm(
          `${twin.name} already does exactly this. Add ${name} anyway, ` +
            "linked, so either control moves both?",
          () => this.addCouplerLinked(name, routes, twin.name),
          null
        );
        return;
      }
      this.organCommand(commands.organCouplerAdd(name, routes)).then(
        (ok) => ok && this.closeAdd()
      );
    });

    this.el.addSourceAdd.addEventListener("click", () => {
      const path = this.el.addSourcePath.value.trim();
      if (!path) return;
      this.organCommand(commands.organSourceAdd(path)).then((ok) => {
        if (ok) this.el.addSourcePath.value = "";
      });
    });
    this.el.addBrowseUp.addEventListener("click", () => {
      if (this.addBrowseParent) this.addBrowse(this.addBrowseParent);
    });
  }

  /// The new manual's panels should land where the add menu was opened,
  /// not wherever the default layout would seat them.
  rememberPlacement(name) {
    if (!this.addAnchor) return;
    const rect = this.el.canvas.getBoundingClientRect();
    this.pendingPlace = {
      name,
      x: this.addAnchor.x - rect.left,
      y: this.addAnchor.y - rect.top,
    };
  }

  /// Runs on every structural rebuild: once the awaited manual exists,
  /// seat its keyboard at the remembered spot and its jamb just left of
  /// it, then persist both. Sizes are real by now — the panels are in
  /// the DOM this decorate pass is decorating.
  placePending(snapshot) {
    const pending = this.pendingPlace;
    if (!pending) return;
    if (!snapshot.manuals.some((m) => m.name === pending.name)) return;
    this.pendingPlace = null;
    const canvas = this.el.canvas;
    const W = canvas.clientWidth;
    const H = canvas.clientHeight;
    const keyboard = canvas.querySelector(`.panel[data-panel="keyboard:${CSS.escape(pending.name)}"]`);
    const jamb = canvas.querySelector(`.panel[data-panel="jamb:${CSS.escape(pending.name)}"]`);
    if (!W || !H || !keyboard) return;
    const kx = Math.max(0, Math.min(pending.x, W - keyboard.offsetWidth));
    const ky = Math.max(0, Math.min(pending.y, H - keyboard.offsetHeight));
    const places = [commands.organPanelPlace(`keyboard:${pending.name}`, kx / W, ky / H)];
    if (jamb) {
      const jx = Math.max(0, kx - jamb.offsetWidth - 16);
      places.push(commands.organPanelPlace(`jamb:${pending.name}`, jx / W, ky / H));
    }
    this.runQueue(places);
  }

  openManualForm(kind) {
    this.addKind = kind;
    this.closeAddPanels();
    this.el.addManualForm.classList.remove("hidden");
    this.el.addManualName.value = "";
    this.el.addManualLow.value = "C2";
    this.el.addManualHigh.value = kind === "pedal" ? "G4" : "C7";
    this.el.addManualLow.classList.remove("invalid");
    this.el.addManualHigh.classList.remove("invalid");
    if (this.addAnchor) this.positionPopover(this.el.add, this.addAnchor.x, this.addAnchor.y);
    requestAnimationFrame(() => this.el.addManualName.focus());
  }

  openEncForm() {
    this.closeAddPanels();
    this.el.addEncForm.classList.remove("hidden");
    if (this.addAnchor) this.positionPopover(this.el.add, this.addAnchor.x, this.addAnchor.y);
    requestAnimationFrame(() => this.el.addEncName.focus());
  }

  openCouplerAddForm() {
    this.closeAddPanels();
    this.el.addCouplerForm.classList.remove("hidden");
    this.el.addCouplerName.value = "";
    this.addCouplerNamed = false;
    this.el.addCouplerAt.value = "0";
    // Couplers taken off the console come back from here — a set's own
    // can't be deleted, only hidden, and hiding must be reversible
    // where couplers are added, not in some other surface.
    this.el.addCouplerRestore.replaceChildren();
    const hidden = (this.lastSnapshot?.couplers ?? []).filter((c) => c.hidden);
    if (hidden.length) {
      const heading = document.createElement("span");
      heading.className = "menu-heading";
      heading.textContent = "Off the console";
      this.el.addCouplerRestore.append(heading);
      for (const coupler of hidden) {
        const row = document.createElement("div");
        row.className = "coupler-restore-row";
        const name = document.createElement("span");
        name.textContent = coupler.name;
        name.title = coupler.name;
        const restore = document.createElement("button");
        restore.type = "button";
        restore.className = "ghost";
        restore.textContent = "Restore";
        restore.addEventListener("click", () => {
          this.closeAdd();
          this.organCommand(commands.organCoupler(coupler.idx, true));
        });
        row.append(name, restore);
        this.el.addCouplerRestore.append(row);
      }
    }
    const manuals = this.lastSnapshot?.manuals ?? [];
    for (const select of [this.el.addCouplerSounds, this.el.addCouplerOn]) {
      select.replaceChildren();
      for (const manual of manuals) {
        const opt = document.createElement("option");
        opt.value = manual.idx;
        opt.textContent = manual.name;
        select.append(opt);
      }
    }
    // The classic default: the second manual sounding on the first —
    // and a name to match, ready to be overtyped.
    if (manuals.length > 1) {
      this.el.addCouplerSounds.value = String(manuals[1].idx);
      this.el.addCouplerOn.value = String(manuals[0].idx);
    }
    this.suggestCouplerName();
    if (this.addAnchor) this.positionPopover(this.el.add, this.addAnchor.x, this.addAnchor.y);
  }

  /// The conventional name for what the add form's selects say:
  /// "Swell to Great", "16′ Swell to Great" for a sub-octave, "Great
  /// 4′" when a manual couples to itself at a pitch. Only fills the
  /// name until the player types their own.
  suggestCouplerName() {
    if (this.addCouplerNamed) return;
    const manuals = this.lastSnapshot?.manuals ?? [];
    const name = (value) => manuals.find((m) => String(m.idx) === value)?.name;
    const sounds = name(this.el.addCouplerSounds.value);
    const on = name(this.el.addCouplerOn.value);
    if (!sounds || !on) return;
    const shift = Number(this.el.addCouplerAt.value) || 0;
    const pitch = shift === -12 ? "16′" : shift === 12 ? "4′" : "";
    this.el.addCouplerName.value =
      sounds === on
        ? `${sounds} ${pitch || "Unison"}`.trim()
        : `${pitch} ${sounds} to ${on}`.trim();
  }

  openSourceForm() {
    // The desktop shell has a real file dialog — use it, as the picker
    // does. The in-form browser below stays the web fallback, and the
    // right tool again should this console ever front a remote server
    // (a native dialog would then pick paths on the wrong machine).
    if (window.__TAURI__) {
      this.pickSourceNative();
      return;
    }
    this.closeAddPanels();
    this.el.addSourceForm.classList.remove("hidden");
    this.el.addSourcePath.value = "";
    this.addBrowseDir = null;
    this.addBrowseParent = null;
    if (this.addAnchor) this.positionPopover(this.el.add, this.addAnchor.x, this.addAnchor.y);
    this.addBrowse();
  }

  /// The native open dialog, filtered to sample sets. A cancelled
  /// dialog is not an error — nothing happens; a pick goes straight to
  /// the server, whose refusals surface like any other edit's.
  async pickSourceNative() {
    this.closeAdd();
    const picked = await window.__TAURI__.core
      .invoke("plugin:dialog|open", {
        options: {
          title: "Choose a sample set",
          filters: [SET_FILTER],
          multiple: false,
          directory: false,
        },
      })
      .catch(() => null);
    const path = Array.isArray(picked) ? picked[0] : picked;
    if (typeof path === "string" && path) this.organCommand(commands.organSourceAdd(path));
  }

  /// This organ's own directory listing, the same idiom as the picker's
  /// Browse pane but scoped to this form: fetched directly, not
  /// snapshot-driven, and picking a file adds it as a source outright
  /// rather than loading it.
  async addBrowse(dir) {
    try {
      const query = dir ? `/api/browse?dir=${encodeURIComponent(dir)}` : "/api/browse";
      const response = await fetch(this.base + query);
      if (!response.ok) {
        this.addBrowseError = (await response.text()) || `${response.status} ${response.statusText}`;
        this.renderAddBrowse();
        return;
      }
      const data = await response.json();
      this.addBrowseDir = data.dir;
      this.addBrowseParent = data.parent;
      this.addBrowseEntries = data.entries;
      this.addBrowseError = null;
      this.renderAddBrowse();
    } catch (err) {
      this.addBrowseError = String(err);
      this.renderAddBrowse();
    }
  }

  renderAddBrowse() {
    this.el.addBrowseDir.textContent = this.addBrowseDir ?? "";
    this.el.addBrowseDir.title = this.addBrowseDir ?? "";
    this.el.addBrowseUp.disabled = !this.addBrowseParent;
    this.el.addBrowseError.classList.toggle("hidden", !this.addBrowseError);
    this.el.addBrowseError.textContent = this.addBrowseError ?? "";
    this.el.addBrowseList.replaceChildren();
    if (this.addBrowseError) return;
    // The server also lists Scala tuning files now; this browser means
    // loadable sets and organs.
    const loadable = /\.(organ|toml|organ_hauptwerk_xml)$/i;
    const entries = (this.addBrowseEntries ?? []).filter(
      (entry) => entry.dir || loadable.test(entry.name)
    );
    if (!entries.length) {
      this.el.addBrowseList.append(this.emptyNote("Nothing here."));
      return;
    }
    for (const entry of entries) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = entry.dir ? "picker-row picker-browse-dir" : "picker-row";
      row.title = entry.path;
      row.addEventListener("click", () => {
        if (entry.dir) {
          this.addBrowse(entry.path);
        } else {
          this.el.addSourcePath.value = entry.path;
          this.organCommand(commands.organSourceAdd(entry.path));
        }
      });
      const name = document.createElement("span");
      name.className = "picker-row-name";
      name.textContent = entry.name;
      row.append(name);
      this.el.addBrowseList.append(row);
    }
  }
}
