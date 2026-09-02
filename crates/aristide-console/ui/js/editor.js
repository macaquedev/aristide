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

import { keyboardScale, measureKeyboard } from "./kb-scale.js";
import { commands, localFetch } from "./api.js";
import { setText } from "./dom.js";
import { menuItem } from "./menu.js";
import { popovers, closeQuickMenus, openingPopover, closeAllPopovers } from "./editor/popovers.js";
import { wireDragSource } from "./editor/drag-controller.js";
import { wireDrawer, openDrawer, closeDrawer, fetchOfferings } from "./editor/library-drawer.js";
import {
  wireCanvas,
  openAddMenu,
  closeAdd,
  wireAdd,
  placePending,
  openCouplerAddForm,
} from "./editor/add-menu.js";
import {
  wireStopForm,
  openStopForm,
  closeStopForm,
  syncStopForm,
  syncPistonRow,
} from "./editor/stop-editor.js";
import {
  wireCouplerForm,
  openCouplerForm,
  closeCouplerForm,
  duplicateCouplerOf,
  syncCouplerForm,
} from "./editor/coupler-routes.js";
import {
  wireTuningForm,
  openTuningForm,
  closeTuningForm,
  tuningLabel,
  tuningSummary,
  stopTuningLine,
  syncTuningForm,
  currentScalePath,
  tuningFields,
  tuningCommand,
  openTuningBrowse,
  closeTuningBrowse,
  tuningBrowse,
} from "./editor/tuning-popover.js";
import {
  wireMidiForm,
  openMidiForm,
  closeMidiForm,
  syncMidiForm,
  wireCompassForm,
  openCompassForm,
  closeCompassForm,
  syncCompassForm,
  wireRoomForm,
  openRoomForm,
  closeRoomForm,
  syncRoomForm,
  wireBindingsForm,
  openBindingsForm,
  closeBindingsForm,
  syncBindingsForm,
  wireSaveForm,
  openSaveForm,
  closeSaveForm,
  syncSaveForm,
  wireSaveAsForm,
  openSaveAsForm,
  closeSaveAsForm,
  syncSaveAsForm,
} from "./editor/settings-popovers.js";
import { wireHexForm, openHexForm, closeHexForm, syncHexForm } from "./editor/hex-popover.js";
import {
  wireTremKnob,
  wireTremForm,
  openTremForm,
  closeTremForm,
  syncTremForm,
} from "./editor/trem-popover.js";
import { PITCH_ACTIONS, emptyNote } from "./wiring.js";

/// The keyboard context menu's "Change type" radio group, in the order
/// they're offered — the same vocabulary the add menu and the server's
/// `kind=` param share.
const KEYBOARD_KINDS = [
  ["manual", "Manual"],
  ["pedal", "Pedalboard"],
  ["microtonal", "Microtonal keyboard"],
];

/// Anything with behavior of its own — a panel drag must never start on
/// these, or a drawknob could not be clicked and a key could not play.
const INTERACTIVE = ".knob, .key, .cheek, .rocker, .shoe, button, input, select, textarea";

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
    this.compassManual = null; // manual idx the compass popover is open for, or null
    this.roomOpen = false; // the Room & noises popover
    this.roomDragging = new Set(); // slider keys mid-drag: the snapshot keeps its hands off
    this.bindingsOpen = false; // the flat Bindings popover
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
    setText(this.el.statusText, snapshot.loading ?? "");

    // A load that failed with the picker closed (an organ picked from
    // the menu's Recent list) would otherwise fail into silence: the
    // picker shows load_error only while it is up, so the error strip
    // carries it here. Warnings ride along — an organ whose file lines
    // were healed over, or whose sample set had holes, loads emptier
    // than intended, and that must say so where the player is looking.
    // Only transitions matter — repainting on every poll would clobber
    // the strip's own transient command errors.
    const warnings = snapshot.load_warnings ?? [];
    const loadError =
      snapshot.load_error ??
      (warnings.length
        ? `the organ loaded with ${warnings.length} warning${
            warnings.length === 1 ? "" : "s"
          } — e.g. ${warnings[0]}`
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
      setText(chip, this.tuningSummary(own));
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
      label.title = `${label.textContent} — Ctrl-drag to the bin to remove this swell box; its stops stay, unenclosed.`;
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

  /// A keyboard's resize: measured once at the start (kb-scale.js —
  /// the same math console.js's scaleKeyboard applies when the stored
  /// size comes back off the file), then solved per move.
  startKeyboardResize(panel, event) {
    const measured = measureKeyboard(panel);
    if (!measured) return;
    const start = { x: event.clientX, w: panel.offsetWidth, ...measured };
    panel.dataset.dragging = "1"; // layoutPanels leaves a mid-gesture panel alone
    const move = (e) => {
      const target = start.w + e.clientX - start.x;
      panel.style.setProperty("--kb-scale", keyboardScale(start, target));
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

    menu.append(
      menuItem("Add a stop…", {
        onClick: async () => await this.showDivisionStops(menu, manual),
      })
    );

    // "No enclosure already": none of this division's stops are boxed
    // and no box carries its name. The box takes the whole division.
    const stops = (snapshot.stops ?? []).filter((s) => s.midx === idx);
    const enclosed = stops.some((s) => (s.enc ?? []).length);
    const named = (snapshot.enclosures ?? []).some((e) => e.name === manual.name);
    if (stops.length && !enclosed && !named) {
      menu.append(
        menuItem("Enclose in a swell box", {
          onClick: () => {
            this.closeDivisionMenu();
            // One rebuild each; runQueue waits each rebuild out.
            this.runQueue([
              commands.organEnclosureAdd(manual.name),
              ...stops.map((stop) => commands.organEnclosureAssign(manual.name, stop.id, true)),
            ]);
          },
        })
      );
    }
  }

  /// Swap the division menu's items for a pick-list of every stop the
  /// sources still offer; clicking one pulls it onto this manual. The
  /// list stays open so a division can be registered in one visit.
  async showDivisionStops(menu, manual) {
    menu.replaceChildren(emptyNote("Reading the sources…"));
    if (!this.offerings) await this.fetchOfferings(false);
    menu.replaceChildren();
    const sources = this.offerings;
    if (sources == null) {
      menu.append(emptyNote("Couldn't read this organ's sources."));
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
          const row = menuItem(stop.name, {
            onClick: () => {
              row.disabled = true; // optimistic: pulled now
              this.organCommand(
                commands.organPull(source.alias, srcManual.name, manual.name, stop.name)
              );
            },
          });
          group.append(row);
        }
        menu.append(group);
      }
    }
    if (!any) {
      menu.append(
        emptyNote("The sources have nothing left to offer — add a sample set first.")
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
      menu.append(
        menuItem(label, {
          radio: true,
          checked: kind === currentKind,
          onClick: () => {
            this.closeKeyboardMenu();
            if (kind !== currentKind) this.organCommand(commands.organManualKind(idx, kind));
          },
        })
      );
    }

    menu.append(document.createElement("hr"));

    // The bin gesture as a menu item — same confirm, same command.
    menu.append(
      menuItem("Remove keyboard…", {
        onClick: () => {
          this.closeKeyboardMenu();
          const stopCount = (this.lastSnapshot?.stops ?? []).filter((s) => s.midx === idx).length;
          this.showRemoveConfirm("manual", { idx, name: manual.name, stopCount });
        },
      })
    );

    // The manual's own wiring and reach, popovers of their own. Both
    // sit above "Change tuning…" so the tuning item stays the menu's
    // last (harness-hooks.js counts on that).
    menu.append(
      menuItem("MIDI input…", {
        onClick: () => {
          const rect = menu.getBoundingClientRect();
          this.closeKeyboardMenu();
          this.openMidiForm(idx, rect.left, rect.top);
        },
      })
    );

    menu.append(
      menuItem("Compass…", {
        onClick: () => {
          const rect = menu.getBoundingClientRect();
          this.closeKeyboardMenu();
          this.openCompassForm(idx, rect.left, rect.top);
        },
      })
    );

    // A hex field is a microtonal-manual fact; the other kinds have
    // no layout to offer.
    if (currentKind === "microtonal") {
      menu.append(
        menuItem("Hex layout…", {
          onClick: () => {
            const rect = menu.getBoundingClientRect();
            this.closeKeyboardMenu();
            this.openHexForm(idx, rect.left, rect.top);
          },
        })
      );
    }

    menu.append(
      menuItem("Change tuning…", {
        onClick: () => {
          const rect = menu.getBoundingClientRect();
          this.closeKeyboardMenu();
          this.openTuningForm({ kind: "division", idx }, rect.left, rect.top);
        },
      })
    );
  }

  closeKeyboardMenu() {
    this.el.keyboardMenu.classList.add("hidden");
    this.el.keyboardMenu.replaceChildren();
  }

  // ---- the tuning popover: this manual's own pitch, apart from the --------
  // See editor/tuning-popover.js (also home of its own file browser,
  // formerly its own section here — the popover and browse together).

  wireTuningForm() {
    wireTuningForm(this);
  }

  openTuningForm(scope, x, y) {
    openTuningForm(this, scope, x, y);
  }

  closeTuningForm() {
    closeTuningForm(this);
  }

  tuningLabel(tuning) {
    return tuningLabel(this, tuning);
  }

  tuningSummary(tuning) {
    return tuningSummary(this, tuning);
  }

  stopTuningLine(stop) {
    return stopTuningLine(this, stop);
  }

  syncTuningForm() {
    syncTuningForm(this);
  }

  currentScalePath() {
    return currentScalePath(this);
  }

  tuningFields(extra) {
    return tuningFields(this, extra);
  }

  async tuningCommand(fields) {
    return tuningCommand(this, fields);
  }

  openTuningBrowse(kind) {
    openTuningBrowse(this, kind);
  }

  closeTuningBrowse() {
    closeTuningBrowse(this);
  }

  async tuningBrowse(dir) {
    await tuningBrowse(this, dir);
  }

  // ---- the MIDI-input, compass, Room & noises, Bindings, save and --------
  // save-as popovers -- see editor/settings-popovers.js.

  wireMidiForm() {
    wireMidiForm(this);
  }

  openMidiForm(idx, x, y) {
    openMidiForm(this, idx, x, y);
  }

  closeMidiForm() {
    closeMidiForm(this);
  }

  syncMidiForm() {
    syncMidiForm(this);
  }

  wireCompassForm() {
    wireCompassForm(this);
  }

  openCompassForm(idx, x, y) {
    openCompassForm(this, idx, x, y);
  }

  closeCompassForm() {
    closeCompassForm(this);
  }

  syncCompassForm() {
    syncCompassForm(this);
  }

  wireRoomForm() {
    wireRoomForm(this);
  }

  openRoomForm(x, y) {
    openRoomForm(this, x, y);
  }

  closeRoomForm() {
    closeRoomForm(this);
  }

  syncRoomForm() {
    syncRoomForm(this);
  }

  wireBindingsForm() {
    wireBindingsForm(this);
  }

  openBindingsForm(x, y) {
    openBindingsForm(this, x, y);
  }

  closeBindingsForm() {
    closeBindingsForm(this);
  }

  syncBindingsForm() {
    syncBindingsForm(this);
  }

  wireSaveForm() {
    wireSaveForm(this);
  }

  openSaveForm(x, y) {
    openSaveForm(this, x, y);
  }

  closeSaveForm() {
    closeSaveForm(this);
  }

  syncSaveForm() {
    syncSaveForm(this);
  }

  wireSaveAsForm() {
    wireSaveAsForm(this);
  }

  openSaveAsForm(pending = null) {
    openSaveAsForm(this, pending);
  }

  closeSaveAsForm() {
    closeSaveAsForm(this);
  }

  syncSaveAsForm() {
    syncSaveAsForm(this);
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

  // ---- the popover registry: one source of truth for what's open ----------
  // See editor/popovers.js: everything about what each popover is and
  // what opening it closes lives there now.

  get popovers() {
    return popovers(this);
  }

  closeQuickMenus() {
    closeQuickMenus(this);
  }

  openingPopover(kind) {
    openingPopover(this, kind);
  }

  closeAllPopovers() {
    closeAllPopovers(this);
  }

  // ---- the tremulant-shape popover: right-click the Tremblant knob --------
  // See editor/trem-popover.js.

  wireTremKnob() {
    wireTremKnob(this);
  }

  wireTremForm() {
    wireTremForm(this);
  }

  openTremForm(x, y) {
    openTremForm(this, x, y);
  }

  closeTremForm() {
    closeTremForm(this);
  }

  syncTremForm() {
    syncTremForm(this);
  }

  // ---- the stop-editor popover: right-click any drawknob ------------------
  // See editor/stop-editor.js.

  wireStopForm() {
    wireStopForm(this);
  }

  openStopForm(id, x, y) {
    openStopForm(this, id, x, y);
  }

  closeStopForm() {
    closeStopForm(this);
  }

  syncStopForm() {
    syncStopForm(this);
  }

  syncPistonRow(container, action) {
    syncPistonRow(this, container, action);
  }

  // ---- the coupler-route popover: right-click any coupler rocker ----------
  // See editor/coupler-routes.js.

  wireCouplerForm() {
    wireCouplerForm(this);
  }

  openCouplerForm(idx, x, y) {
    openCouplerForm(this, idx, x, y);
  }

  closeCouplerForm() {
    closeCouplerForm(this);
  }

  duplicateCouplerOf(excludeIdx, routes) {
    return duplicateCouplerOf(this, excludeIdx, routes);
  }

  syncCouplerForm() {
    syncCouplerForm(this);
  }

  // ---- the hex-layout popover: a microtonal manual's isomorphic grid ------
  // See editor/hex-popover.js.

  wireHexForm() {
    wireHexForm(this);
  }

  openHexForm(idx, x, y) {
    openHexForm(this, idx, x, y);
  }

  closeHexForm() {
    closeHexForm(this);
  }

  syncHexForm() {
    syncHexForm(this);
  }

  // ---- drag controller: plain when unlocked, ctrl-drag always -------------
  // See editor/drag-controller.js: the pointer-drag state machine and
  // its drop-target math live there now.

  wireDragSource(el, getInfo) {
    wireDragSource(this, el, getInfo);
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

  /// A 409 is the sample set's own organ refusing to change its
  /// instrument: not an
  /// error to show, but the save-as dialog to open with the refused
  /// command in hand (see openSaveAsForm). True when handled that way.
  deferToSaveAs(status, query) {
    if (status !== 409) return false;
    this.openSaveAsForm(query);
    return true;
  }

  /// `error` is null when nothing is the caller's to show — the edit
  /// went through, or the save-as dialog has taken it over.
  async organCommandResult(query) {
    this.hideError();
    const { ok, status, error } = await localFetch(this.base, query, { method: "POST" });
    if (!ok) {
      if (this.deferToSaveAs(status, query)) return { ok: false, error: null };
      return { ok: false, error };
    }
    // Any successful edit can change what the sources offer (a new
    // source, a pull claiming a stop) — the cached offerings are
    // stale now whether or not the drawer is up to show it. The
    // division menu reads this cache, so a kept stale [] would keep
    // insisting there are no sources right after one was added.
    this.offerings = null;
    if (this.drawerOpen) this.fetchOfferings();
    return { ok: true, error: null };
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
  // See editor/library-drawer.js.

  wireDrawer() {
    wireDrawer(this);
  }

  openDrawer() {
    openDrawer(this);
  }

  closeDrawer() {
    closeDrawer(this);
  }

  async fetchOfferings(render = true) {
    await fetchOfferings(this, render);
  }

  // ---- adding to the organ: double-click the canvas -----------------------
  // See editor/add-menu.js.

  wireCanvas() {
    wireCanvas(this);
  }

  openAddMenu(x, y) {
    openAddMenu(this, x, y);
  }

  positionPopover(el, x, y) {
    el.style.left = "0px";
    el.style.top = "0px";
    const { width, height } = el.getBoundingClientRect();
    el.style.left = `${Math.max(8, Math.min(x, window.innerWidth - width - 8))}px`;
    el.style.top = `${Math.max(8, Math.min(y, window.innerHeight - height - 8))}px`;
  }

  closeAdd() {
    closeAdd(this);
  }

  wireAdd() {
    wireAdd(this);
  }

  placePending(snapshot) {
    placePending(this, snapshot);
  }

  openCouplerAddForm() {
    openCouplerAddForm(this);
  }
}
