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

  const prefsParam = params.get("prefs");
  if (prefsParam) prefs.open(prefsParam);

  const scrollParam = params.get("paneScroll");
  if (scrollParam) {
    const pane = document.querySelector(`.pane[data-pane="${prefsParam}"]`);
    // Wait out the first poll so there's real content to scroll through,
    // not just the pane's pre-snapshot shell.
    if (pane) setTimeout(() => { pane.scrollTop = Number(scrollParam); }, 400);
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
}
