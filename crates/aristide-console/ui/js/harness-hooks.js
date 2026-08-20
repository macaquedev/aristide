// Screenshot-harness hooks only: a handful of `?param` query switches that
// let the screenshot script (~/tmp/ui-shots/stub.py + chromium --headless)
// drive the console into a state a single static screenshot couldn't reach
// on its own — a dialog open to a tab, mid-drag chrome forced visible, a
// pane scrolled past the fold. Every one of them is a no-op unless a
// screenshot script sets the param, so leaving this wired up in production
// is harmless.

/// Call once at startup with the pieces a hook needs to reach. `prefs` is
/// the Preferences instance — hooks that just need `document` reach it
/// directly.
export function applyHarnessHooks({ prefs }) {
  const params = new URLSearchParams(location.search);

  const prefsParam = params.get("prefs");
  if (prefsParam) prefs.open(prefsParam);

  if (params.has("forceBin")) {
    document.getElementById("organ-bin")?.classList.add("visible");
  }

  const scrollParam = params.get("paneScroll");
  if (scrollParam) {
    const pane = document.querySelector(`.pane[data-pane="${prefsParam}"]`);
    // Wait out the first poll (and the Organ pane's own offerings fetch)
    // so there's real content to scroll through, not just the pane's
    // pre-snapshot shell.
    if (pane) setTimeout(() => { pane.scrollTop = Number(scrollParam); }, 400);
  }

  if (params.has("openSources")) {
    setTimeout(() => {
      for (const d of document.querySelectorAll(".organ-offerings-source")) d.open = true;
    }, 400);
  }

  if (params.has("forceRemoveConfirm")) {
    setTimeout(() => {
      document.getElementById("organ-remove-confirm-text").textContent =
        "Remove Grand-Orgue and its 6 stops? Sources still offer everything.";
      document.getElementById("organ-remove-confirm").classList.remove("hidden");
    }, 400);
  }

  if (params.has("forceRemoveEncConfirm")) {
    setTimeout(() => {
      document.getElementById("organ-remove-confirm-text").textContent =
        "Remove the Solo box? Its stops stay, unenclosed.";
      document.getElementById("organ-remove-confirm").classList.remove("hidden");
    }, 400);
  }

  if (params.has("fabMenu")) {
    setTimeout(() => document.getElementById("organ-fab-menu").classList.remove("hidden"), 400);
  }

  const fabFormParam = params.get("fabForm"); // "manual" | "pedal" | "enc" | "source"
  if (fabFormParam) {
    // A real click, not just unhiding the form — "source" also needs the
    // form's own openSourceForm() to run so its directory listing fetches.
    const buttonId = {
      manual: "organ-fab-add-manual", pedal: "organ-fab-add-pedal",
      enc: "organ-fab-add-enc", source: "organ-fab-add-source",
    }[fabFormParam];
    setTimeout(() => {
      document.getElementById("organ-fab").click();
      document.getElementById(buttonId)?.click();
    }, 400);
  }

  const renameManualParam = params.get("renameManual");
  if (renameManualParam) {
    setTimeout(() => {
      const header = [...document.querySelectorAll(".organ-manual-header")][Number(renameManualParam)];
      header?.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    }, 400);
  }
}
