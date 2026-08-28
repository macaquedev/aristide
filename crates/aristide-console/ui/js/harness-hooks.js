// Screenshot-harness hooks only: a handful of `?param` query switches that
// let the screenshot script (~/tmp/ui-shots/stub.py + chromium --headless)
// drive the console into a state a single static screenshot couldn't reach
// on its own — the editor unlocked, the drawer open, a drag's bin forced
// visible, a form or menu popped without a real click sequence. Every one
// of them is a no-op unless a screenshot script sets the param, so leaving
// this wired up in production is harmless.

/// Call once at startup with the pieces a hook needs to reach. `prefs` is
/// the Preferences instance, `editor` the console editor (see editor.js);
/// hooks that just need `document` reach it directly.
export function applyHarnessHooks({ prefs, editor }) {
  const params = new URLSearchParams(location.search);

  // Preferences is user-only now (a single appearance pane), so the
  // param takes no tab; any value opens the dialog.
  if (params.has("prefs")) prefs.open();

  // The organ-scoped settings popovers, opened as their menu items
  // would open them.
  if (params.has("organTuning")) {
    setTimeout(() => editor.openTuningForm("organ", 120, 40), 400);
  }
  if (params.has("roomForm")) {
    setTimeout(() => editor.openRoomForm(120, 40), 400);
  }
  if (params.has("bindingsForm")) {
    setTimeout(() => editor.openBindingsForm(120, 40), 400);
  }
  if (params.has("saveForm")) {
    setTimeout(() => editor.openSaveForm(), 400);
  }

  if (params.has("openSources")) {
    setTimeout(() => {
      for (const d of document.querySelectorAll(".organ-offerings-source")) d.open = true;
    }, 400);
  }

  if (params.has("unlock")) {
    setTimeout(() => editor.unlock(), 400);
  }

  if (params.has("drawer")) {
    setTimeout(() => {
      editor.unlock();
      editor.openDrawer();
    }, 400);
  }

  if (params.has("forceBin")) {
    setTimeout(() => document.getElementById("editor-bin")?.classList.add("visible"), 400);
  }

  if (params.has("forceRemoveConfirm")) {
    setTimeout(() => {
      document.getElementById("editor-remove-confirm-text").textContent =
        "Remove Grand-Orgue and its 6 stops? Sources still offer everything.";
      document.getElementById("editor-remove-confirm").classList.remove("hidden");
    }, 400);
  }

  if (params.has("forceRemoveEncConfirm")) {
    setTimeout(() => {
      document.getElementById("editor-remove-confirm-text").textContent =
        "Remove the Solo box? Its stops stay, unenclosed.";
      document.getElementById("editor-remove-confirm").classList.remove("hidden");
    }, 400);
  }

  if (params.has("addMenu")) {
    setTimeout(() => {
      editor.unlock();
      editor.openAddMenu(window.innerWidth * 0.55, window.innerHeight * 0.4);
    }, 400);
  }

  const addFormParam = params.get("addForm"); // "manual" | "pedal" | "microtonal" | "enc" | "source"
  if (addFormParam) {
    // A real click, not just unhiding the form — "source" also needs the
    // form's own openSourceForm() to run so its directory listing fetches.
    const buttonId = {
      manual: "editor-add-manual", pedal: "editor-add-pedal", microtonal: "editor-add-microtonal",
      enc: "editor-add-enc", source: "editor-add-source",
    }[addFormParam];
    setTimeout(() => {
      editor.unlock();
      editor.openAddMenu(window.innerWidth * 0.55, window.innerHeight * 0.4);
      document.getElementById(buttonId)?.click();
    }, 400);
  }

  const divisionMenuParam = params.get("divisionMenu"); // a manual idx
  if (divisionMenuParam != null) {
    setTimeout(() => {
      editor.unlock();
      const division = document.querySelector(
        `.division[data-division="${divisionMenuParam}"] .division-add`
      );
      division?.click();
      if (params.has("divisionStops")) {
        setTimeout(() => {
          document.querySelector("#editor-division-menu .menu-item")?.click();
        }, 200);
      }
    }, 400);
  }

  const renameManualParam = params.get("renameManual");
  if (renameManualParam) {
    setTimeout(() => {
      editor.unlock();
      const cheeks = [...document.querySelectorAll(".keyboard .cheek")];
      cheeks[Number(renameManualParam)]?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    }, 400);
  }

  // A manual's own board, found by its snapshot idx (the dataset value)
  // or its name (the cheek's text) — either is handy from a screenshot
  // script that only knows one of them.
  function findKeyboardBoard(ref) {
    const boards = [...document.querySelectorAll(".keyboard[data-manual]")];
    return (
      boards.find((b) => b.dataset.manual === String(ref)) ??
      boards.find((b) => b.querySelector(".cheek")?.textContent === ref)
    );
  }

  function rightClick(board) {
    const rect = board.getBoundingClientRect();
    board.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, clientX: rect.left + 60, clientY: rect.top + 20 })
    );
  }

  const kbdMenuParam = params.get("kbdMenu"); // a manual name or idx
  if (kbdMenuParam != null) {
    setTimeout(() => {
      editor.unlock();
      const board = findKeyboardBoard(kbdMenuParam);
      if (board) rightClick(board);
    }, 400);
  }

  // One step into the keyboard menu: the manual's MIDI-input popover
  // or its compass popover, found by their labels.
  for (const [param, label] of [
    ["kbdMidi", "MIDI input"],
    ["kbdCompass", "Compass"],
  ]) {
    const ref = params.get(param); // a manual name or idx
    if (ref == null) continue;
    setTimeout(() => {
      editor.unlock();
      const board = findKeyboardBoard(ref);
      if (board) rightClick(board);
      setTimeout(() => {
        [...document.querySelectorAll("#editor-keyboard-menu .menu-item")]
          .find((item) => item.textContent.includes(label))
          ?.click();
      }, 200);
    }, 400);
  }

  // The tremulant-shape popover, via a right-click on the trem knob.
  if (params.has("tremForm")) {
    setTimeout(() => {
      editor.unlock();
      const knob = document.querySelector('[data-key="trem"]');
      if (!knob) return;
      const rect = knob.getBoundingClientRect();
      knob.dispatchEvent(
        new MouseEvent("contextmenu", {
          bubbles: true, cancelable: true,
          clientX: rect.left + 10, clientY: rect.top + 10,
        })
      );
    }, 600);
  }

  // One step into the keyboard menu: the microtonal hex-layout form,
  // found by its label — it only exists on microtonal manuals.
  const kbdHexParam = params.get("kbdHexForm"); // a manual name or idx
  if (kbdHexParam != null) {
    setTimeout(() => {
      editor.unlock();
      const board = findKeyboardBoard(kbdHexParam);
      if (board) rightClick(board);
      setTimeout(() => {
        [...document.querySelectorAll("#editor-keyboard-menu .menu-item")]
          .find((item) => item.textContent.includes("Hex layout"))
          ?.click();
      }, 200);
    }, 400);
  }

  const kbdTuningParam = params.get("kbdTuning"); // a manual name or idx
  if (kbdTuningParam != null) {
    setTimeout(() => {
      editor.unlock();
      const board = findKeyboardBoard(kbdTuningParam);
      if (board) rightClick(board);
      // "Change tuning…" is always the menu's last item, after the
      // three-kind radio group and its divider.
      setTimeout(() => {
        const items = [...document.querySelectorAll("#editor-keyboard-menu .menu-item")];
        items[items.length - 1]?.click();
      }, 200);
    }, 400);
  }

  // Same as kbdTuning, one step further in: the tuning popover's own
  // Scala-scale file browser, open — for a screenshot of real .scl rows.
  const kbdTuningScaleParam = params.get("kbdTuningScale"); // a manual name or idx
  if (kbdTuningScaleParam != null) {
    setTimeout(() => {
      editor.unlock();
      const board = findKeyboardBoard(kbdTuningScaleParam);
      if (board) rightClick(board);
      setTimeout(() => {
        const items = [...document.querySelectorAll("#editor-keyboard-menu .menu-item")];
        items[items.length - 1]?.click();
        setTimeout(() => {
          document.getElementById("editor-tuning-scale-pick")?.click();
        }, 200);
      }, 200);
    }, 400);
  }

  // A stop drawknob, found by its numeric id (the snapshot's `stops[].id`,
  // matching its data-key) or by the label text the console built its
  // knob face from — either is handy from a screenshot script that only
  // knows one of them.
  function findStopKnob(ref) {
    const knobs = [...document.querySelectorAll('.knob[data-key^="stop-"]')];
    const byId = knobs.find((k) => k.dataset.key === `stop-${ref}`);
    if (byId) return byId;
    return knobs.find((k) => {
      const name = k.querySelector(".stop-name")?.textContent ?? "";
      const pitch = k.querySelector(".stop-pitch")?.textContent ?? "";
      return name === ref || (pitch ? `${name} ${pitch}` : name) === ref;
    });
  }

  // The stop-editor popover, via a right-click on its drawknob.
  const stopFormParam = params.get("stopForm"); // a stop id or name
  if (stopFormParam != null) {
    setTimeout(() => {
      editor.unlock();
      const knob = findStopKnob(stopFormParam);
      if (!knob) return;
      const rect = knob.getBoundingClientRect();
      knob.dispatchEvent(
        new MouseEvent("contextmenu", {
          bubbles: true, cancelable: true,
          clientX: rect.left + 10, clientY: rect.top + 10,
        })
      );
      // One step further in: the stop popover's own source-picker
      // subview, open — for a screenshot of the real offerings list.
      if (params.has("stopSrc")) {
        setTimeout(() => {
          document.getElementById("editor-stop-src-change")?.click();
        }, 200);
      }
    }, 400);
  }

  // A coupler rocker, found by its numeric idx (matching its data-key)
  // or by its rail text — either is handy from a screenshot script that
  // only knows one of them.
  function findCouplerRocker(ref) {
    const rockers = [...document.querySelectorAll('.rocker[data-key^="coupler-"]')];
    const byIdx = rockers.find((r) => r.dataset.key === `coupler-${ref}`);
    if (byIdx) return byIdx;
    return rockers.find((r) => r.querySelector(".tab")?.textContent === ref);
  }

  // The coupler-route popover, via a right-click on its rocker.
  const couplerFormParam = params.get("couplerForm"); // a coupler idx or name
  if (couplerFormParam != null) {
    setTimeout(() => {
      editor.unlock();
      const rocker = findCouplerRocker(couplerFormParam);
      if (!rocker) return;
      const rect = rocker.getBoundingClientRect();
      rocker.dispatchEvent(
        new MouseEvent("contextmenu", {
          bubbles: true, cancelable: true,
          clientX: rect.left + 10, clientY: rect.top + 10,
        })
      );
    }, 400);
  }
}
