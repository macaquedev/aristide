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

  if (params.has("fabMenu")) {
    setTimeout(() => {
      editor.unlock();
      document.getElementById("editor-fab-menu").classList.remove("hidden");
    }, 400);
  }

  const fabFormParam = params.get("fabForm"); // "manual" | "pedal" | "enc" | "source"
  if (fabFormParam) {
    // A real click, not just unhiding the form — "source" also needs the
    // form's own openSourceForm() to run so its directory listing fetches.
    const buttonId = {
      manual: "editor-fab-add-manual", pedal: "editor-fab-add-pedal",
      enc: "editor-fab-add-enc", source: "editor-fab-add-source",
    }[fabFormParam];
    setTimeout(() => {
      editor.unlock();
      document.getElementById("editor-fab").click();
      document.getElementById(buttonId)?.click();
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
}
