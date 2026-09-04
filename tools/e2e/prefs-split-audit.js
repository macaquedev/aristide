// End-to-end audit of the preferences split, against the REAL server
// and the REAL console UI: user preferences and organ settings live in
// separate surfaces, and every organ fact edited on the console lands
// in the organ's own file.
//
//   bun tools/e2e/prefs-split-audit.js [path-to-aristide-server]
//
// Needs: a built server (default target/release, falls back to debug),
// chromium on PATH, the gitignored demo set at testsets/grandorgue-demo,
// and no listener on the ports below. Runs against a throwaway
// XDG_CONFIG_HOME, so the real library and wiring are never touched.
//
// What it proves:
//   1. MENUS — the Aristide menu is the player's (Preferences, About),
//      the Organ menu carries the organ-settings popovers, View lost
//      Appearance, Help is gone.
//   2. PREFERENCES IS USER-ONLY — the dialog holds the skin and this
//      machine's sample memory; the skin sends no API command at all,
//      and memory edits reach only /api/prefs — they land in the user
//      config, never in the organ's file.
//   3. TUNING — the bar's readout opens the whole-instrument popover;
//      a pitch commit changes the live tuning AND writes [tuning] into
//      the organ's file.
//   4. ROOM — the Organ-menu popover opens and its rows track what the
//      organ actually has (reverb/noises).
//   5. WIRING — an unwired keyboard wears the silent badge; the badge
//      opens the MIDI popover; binding the computer keyboard clears
//      the badge and writes [[midi.input]] into the file.
//   6. BINDINGS — the flat popover learns a computer key and rebinds
//      its action; the stop editor's piston row quick-binds a key to
//      "stop:<name>".

import { connect, launchHarness } from "./cdp.js";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const SERVER_PORT = 9886;
const UI_PORT = 9887;
const CDP_PORT = 9233;
const h = launchHarness({ name: "prefs-audit", serverPort: SERVER_PORT, uiPort: UI_PORT, cdpPort: CDP_PORT });
const { REPO, S, demo, scratch, check, sleep, state, settled, post, waitForServer, done } = h;
const OUT = join(REPO, "target", "prefs-split-audit");
const skip = (what) => console.log(`SKIP  ${what}`);

try {
  // Wait for the server, then load the demo set — adoption writes the
  // wrapper organ file under the throwaway config dir.
  await waitForServer();
  await post(`/api/organ/load?path=${encodeURIComponent(demo)}`);
  let snap = await settled();
  const organFile = snap.setup?.file;
  check(!!organFile, `the adopted demo lives in a file (${organFile})`);
  const fileText = () => readFileSync(organFile, "utf8");

  const drive = await connect(CDP_PORT);
  await drive.navigate(`http://127.0.0.1:${UI_PORT}/?server=${encodeURIComponent(S)}`);
  await sleep(1500);

  const menuLabels = async (openExpr) => {
    await drive.eval(openExpr);
    await sleep(150);
    return drive.eval(`[...document.querySelectorAll(".menu-list:not(.hidden) .menu-item")]
      .map((b) => b.textContent.trim())`);
  };
  const escape = () =>
    drive.eval(`window.dispatchEvent(new KeyboardEvent("keydown", {key: "Escape", bubbles: true}))`);
  const titleButton = (title) => `[...document.querySelectorAll("#menus .menu-title")]
    .find((b) => b.textContent.trim() === ${JSON.stringify(title)})`;

  // ---- 1. menus ------------------------------------------------------

  const appItems = await menuLabels(`document.getElementById("app-menu").click()`);
  check(
    appItems.some((l) => l.startsWith("Preferences")) && appItems.some((l) => l.includes("About")),
    `Aristide menu holds Preferences and About (${appItems.join(" / ")})`
  );
  await escape();

  const organItems = await menuLabels(`${titleButton("Organ")}.click()`);
  for (const wanted of ["Tuning…", "Room & noises…", "Buttons & shortcuts…"]) {
    check(organItems.includes(wanted), `Organ menu offers ${wanted}`);
  }
  check(!organItems.some((l) => l.startsWith("Preferences")), "Organ menu has no Preferences");
  await escape();

  const viewItems = await menuLabels(`${titleButton("View")}.click()`);
  check(!viewItems.some((l) => l.startsWith("Appearance")), "View menu lost Appearance");
  await escape();
  const titles = await drive.eval(`[...document.querySelectorAll("#menus .menu-title")]
    .map((b) => b.textContent.trim())`);
  check(!titles.includes("Help"), `no Help menu (bar: ${titles.join(", ")})`);

  // ---- 2. Preferences is user-only ----------------------------------

  await drive.eval(`window.__apiPosts = [];
    const realFetch = window.fetch;
    window.fetch = (url, opts) => {
      if (String(url).includes("/api/") && opts?.method === "POST") window.__apiPosts.push(String(url));
      return realFetch(url, opts);
    }; true`);
  await drive.eval(`document.getElementById("app-menu").click()`);
  await sleep(120);
  await drive.eval(`[...document.querySelectorAll(".menu-list:not(.hidden) .menu-item")]
    .find((b) => b.textContent.includes("Preferences")).click()`);
  await sleep(200);
  check(
    await drive.eval(`!document.getElementById("prefs").classList.contains("hidden")`),
    "Preferences opens from the Aristide menu"
  );
  const panes = await drive.eval(`[...document.querySelectorAll("#prefs .pane")].map((p) => p.dataset.pane)`);
  check(panes.join(",") === "appearance,memory", `the dialog holds the skin and sample memory (${panes})`);
  const foreign = await drive.eval(`[...document.querySelectorAll("#prefs select, #prefs input")]
    .map((el) => el.id).filter((id) => id !== "ram-budget").length`);
  check(foreign === 0, "no field in the dialog but the RAM budget");
  await drive.shot(join(OUT, "prefs-user-only.png"));
  // The size row: six zoom steps with the native one lit. In a browser
  // the host zooms for itself, so the row is shown but not live.
  const sizes = await drive.eval(`[...document.querySelectorAll("#scale-segment button")]
    .map((b) => b.textContent + (b.classList.contains("on") ? "*" : ""))`);
  check(
    sizes.join(" ") === "50% 60% 70% 80% 90% 100%* 110% 125% 150% 175% 200%",
    `the size row offers the zoom steps with 100% lit (${sizes.join(" ")})`
  );
  check(
    await drive.eval(`document.getElementById("scale-segment").getAttribute("aria-disabled") === "true"`),
    "in a browser the size row defers to the browser's own zoom"
  );
  await drive.eval(`document.querySelectorAll("#accent-swatches .swatch")[2]?.click();
    document.querySelectorAll("#density-segment button")[0]?.click();
    document.querySelectorAll("#scale-segment button")[10]?.click(); true`);
  await sleep(200);
  const posts = await drive.eval(`window.__apiPosts.length`);
  check(posts === 0, `appearance edits sent ${posts} API commands (want 0)`);

  // Sample memory: the chips reflect the user config, editing them
  // posts to /api/prefs only, the config file takes the change and the
  // organ's file does not — and the pane says a reload is due.
  const streamingChips = await drive.eval(`[...document.querySelectorAll("#streaming-segment button")]
    .map((b) => b.textContent + (b.classList.contains("on") ? "*" : ""))`);
  check(
    streamingChips.join(" ") === "Auto* Stream In RAM",
    `release tails offer auto/stream/in-RAM with auto lit (${streamingChips.join(" ")})`
  );
  const status = await drive.eval(`document.getElementById("memory-status").textContent`);
  check(/^This organ: .* resident/.test(status), `the pane reports the loaded organ's memory (${status})`);
  check(
    await drive.eval(`document.getElementById("memory-stale").classList.contains("hidden")`),
    "nothing is waiting on a reload before an edit"
  );
  await drive.eval(`document.querySelectorAll("#streaming-segment button")[1].click(); true`);
  await sleep(300);
  await drive.eval(`{ const b = document.getElementById("ram-budget"); b.value = "3072";
    b.dispatchEvent(new Event("change", { bubbles: true })); } true`);
  await sleep(300);
  const memoryPosts = await drive.eval(`window.__apiPosts`);
  check(
    memoryPosts.length === 2 && memoryPosts.every((u) => u.includes("/api/prefs/samples?")),
    `memory edits reached only /api/prefs (${memoryPosts.join(" ")})`
  );
  snap = await state();
  check(
    snap.prefs?.samples?.streaming === "on" && snap.prefs?.samples?.ram_budget_mb === 3072,
    `the snapshot carries the new preferences (${JSON.stringify(snap.prefs?.samples)})`
  );
  const userConfig = readFileSync(join(scratch, "config", "aristide", "midi.toml"), "utf8");
  check(
    /\[samples\][\s\S]*streaming = "on"/.test(userConfig) && userConfig.includes("ram_budget_mb = 3072"),
    "the user config took [samples]"
  );
  check(!/^\s*\[samples\]/m.test(fileText()), "the organ's file has no [samples] table");
  check(
    !(await drive.eval(`document.getElementById("memory-stale").classList.contains("hidden")`)),
    "the pane says the change waits for a reload"
  );
  await drive.shot(join(OUT, "prefs-memory.png"));
  await drive.eval(`document.querySelectorAll("#streaming-segment button")[0].click(); true`);
  await drive.eval(`{ const b = document.getElementById("ram-budget"); b.value = "";
    b.dispatchEvent(new Event("change", { bubbles: true })); } true`);
  await sleep(300);
  await escape();

  // ---- 3. whole-instrument tuning -----------------------------------

  await drive.eval(`document.getElementById("tuning").click()`);
  await sleep(200);
  check(
    await drive.eval(`!document.getElementById("editor-tuning").classList.contains("hidden")`),
    "the bar's tuning readout opens the tuning popover"
  );
  check(
    (await drive.eval(`document.getElementById("editor-tuning-title").textContent`)) ===
      "Whole instrument",
    "…in whole-instrument mode"
  );
  await drive.shot(join(OUT, "tuning-whole-instrument.png"));
  // Key left at A4 (the default reference key); only the Hz moves.
  await drive.eval(`const refhz = document.getElementById("editor-tuning-ref-hz");
    refhz.value = 415; refhz.dispatchEvent(new Event("change", {bubbles: true})); true`);
  await sleep(400);
  snap = await state();
  check(Math.abs(snap.tuning.reference.hz - 415) < 0.01, `reference Hz committed live (${snap.tuning.reference.hz})`);
  check(snap.tuning.reference.key === 69, `reference key stayed A4 (${snap.tuning.reference.key})`);
  check(/\[tuning\][^[]*reference_hz\s*=\s*415/s.test(fileText()), "…and written to the file's [tuning]");
  await escape();

  // ---- 4. room & noises ---------------------------------------------

  await drive.eval(`${titleButton("Organ")}.click()`);
  await sleep(120);
  await drive.eval(`[...document.querySelectorAll(".menu-list:not(.hidden) .menu-item")]
    .find((b) => b.textContent.includes("Room")).click()`);
  await sleep(200);
  check(
    await drive.eval(`!document.getElementById("editor-room").classList.contains("hidden")`),
    "Room & noises opens from the Organ menu"
  );
  const reverbHidden = await drive.eval(
    `document.getElementById("editor-room-reverb-row").classList.contains("hidden")`
  );
  check(reverbHidden === (snap.reverb == null), "the reverb row tracks whether the organ has one");
  await drive.shot(join(OUT, "room-noises.png"));
  if (snap.noises) {
    await drive.eval(`const on = document.getElementById("editor-room-noises-on");
      on.checked = !on.checked; on.dispatchEvent(new Event("change", {bubbles: true})); true`);
    await sleep(400);
    check(/\[noises\]/.test(fileText()), "a noises edit writes the file's [noises]");
  } else {
    skip("noises — the demo organ reports none");
  }
  await escape();

  // ---- 5. the silent badge and the MIDI popover ---------------------

  const badge = await drive.eval(`document.querySelector(".keyboard[data-manual] .kb-silent")?.textContent ?? null`);
  check(badge != null, `an unwired keyboard wears the silent badge ("${badge}")`);
  await drive.shot(join(OUT, "silent-badges.png"));
  await drive.eval(`document.querySelector(".keyboard[data-manual] .kb-silent").click()`);
  await sleep(250);
  check(
    await drive.eval(`!document.getElementById("editor-midi").classList.contains("hidden")`),
    "the badge opens the MIDI popover"
  );
  await drive.shot(join(OUT, "midi-popover.png"));
  const boundManual = await drive.eval(`(() => {
    const device = document.querySelector("#editor-midi-inputs .input-device");
    device.value = "Computer keyboard";
    device.dispatchEvent(new Event("change", {bubbles: true}));
    return Number(document.querySelector(".keyboard[data-manual] .kb-silent")
      ?.closest(".keyboard").dataset.manual);
  })()`);
  await sleep(600);
  snap = await state();
  const wired = snap.midi.manuals.find((m) => m.idx === boundManual);
  check(wired?.inputs.length === 1, `the computer keyboard now plays ${wired?.name}`);
  check(
    /\[\[midi\.input\]\][^[]*Computer keyboard/s.test(fileText()),
    "…and the wiring landed in the file's [[midi.input]]"
  );
  check(
    await drive.eval(`document.querySelector(
      ".keyboard[data-manual='" + ${boundManual} + "'] .kb-silent") == null`),
    "…and its badge is gone"
  );
  await escape();

  // ---- 6. bindings: the flat list and a stop's piston row -----------

  await drive.eval(`${titleButton("Organ")}.click()`);
  await sleep(120);
  await drive.eval(`[...document.querySelectorAll(".menu-list:not(.hidden) .menu-item")]
    .find((b) => b.textContent.includes("Buttons & shortcuts")).click()`);
  await sleep(200);
  check(
    await drive.eval(`!document.getElementById("editor-bindings").classList.contains("hidden")`),
    "Bindings opens from the Organ menu"
  );
  await drive.eval(`document.getElementById("editor-bindings-add").click()`);
  await sleep(300);
  snap = await state();
  check(snap.control_learning === 0, "+ add binding starts a learn");
  await post("/api/key?code=KeyQ&on=1");
  await post("/api/key?code=KeyQ&on=0");
  await sleep(500);
  snap = await state();
  check(
    snap.controls?.[0]?.trigger === "key:KeyQ",
    `the learned key landed (${JSON.stringify(snap.controls?.[0])})`
  );
  check(
    await drive.eval(`document.querySelectorAll("#editor-bindings-list .control-row").length`) === 1,
    "…and shows as a row in the popover"
  );
  await drive.shot(join(OUT, "bindings.png"));
  await escape();

  // The stop editor's piston row, reached through the lock.
  await drive.eval(`(() => {
    const knob = document.querySelector('.knob[data-key^="stop-"]');
    const rect = knob.getBoundingClientRect();
    knob.dispatchEvent(new MouseEvent("contextmenu", {
      bubbles: true, cancelable: true, ctrlKey: true,
      clientX: rect.left + 8, clientY: rect.top + 8,
    })); return true;
  })()`);
  await sleep(250);
  check(
    await drive.eval(`!document.getElementById("editor-stop").classList.contains("hidden")`),
    "ctrl-right-click opens the stop editor"
  );
  const stopName = await drive.eval(`document.getElementById("editor-stop-title").textContent`);
  await drive.eval(`document.querySelector("#editor-stop-pistons .listen").click()`);
  await sleep(300);
  await post("/api/key?code=KeyW&on=1");
  await post("/api/key?code=KeyW&on=0");
  await sleep(800);
  snap = await state();
  const quick = (snap.controls ?? []).find((c) => c.trigger === "key:KeyW");
  check(
    quick?.action === `stop:${stopName}`,
    `the piston row quick-bound key W to stop:${stopName} (${JSON.stringify(quick)})`
  );
  await drive.shot(join(OUT, "stop-piston.png"));

  console.log(h.failures ? `\n${h.failures} FAILURES` : "\nall green");
  if (h.failures) console.log("server log tail:\n" + h.serverLog.slice(-2000));
  await done(h.failures ? 1 : 0);
} catch (err) {
  console.error("audit crashed:", err);
  console.log("server log tail:\n" + h.serverLog.slice(-2000));
  await done(2);
}
