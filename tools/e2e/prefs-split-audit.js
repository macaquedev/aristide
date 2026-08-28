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
//   2. PREFERENCES IS USER-ONLY — the dialog holds only appearance
//      controls, and using them sends not one API command.
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

import { connect } from "./cdp.js";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const REPO = new URL("../..", import.meta.url).pathname;
const SERVER_PORT = 9886;
const UI_PORT = 9887;
const CDP_PORT = 9233;
const S = `http://127.0.0.1:${SERVER_PORT}`;
const OUT = join(REPO, "target", "prefs-split-audit");

let failures = 0;
const check = (ok, what) => {
  console.log(`${ok ? "PASS" : "FAIL"}  ${what}`);
  if (!ok) failures++;
};
const skip = (what) => console.log(`SKIP  ${what}`);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const post = async (path) => {
  const res = await fetch(S + path, { method: "POST" });
  return { ok: res.ok, body: await res.text() };
};
const state = async () => (await fetch(S + "/api/state")).json();
const settled = async () => {
  for (let i = 0; i < 100; i++) {
    const s = await state();
    if (s.organ && !s.loading) return s;
    await sleep(300);
  }
  throw new Error("organ never settled");
};

// ---- processes -------------------------------------------------------

const serverBin = process.argv[2] ??
  [join(REPO, "target/release/aristide-server"), join(REPO, "target/debug/aristide-server")]
    .find(existsSync);
if (!serverBin) {
  console.error("no server binary — cargo build -p aristide-server first");
  process.exit(2);
}
const demo = join(REPO, "testsets/grandorgue-demo/demo.organ");
if (!existsSync(demo)) {
  console.error("no demo set — see CLAUDE.md's testsets note");
  process.exit(2);
}

const scratch = mkdtempSync(join(tmpdir(), "aristide-prefs-audit-"));
const server = spawn(serverBin, ["--http-port", String(SERVER_PORT)], {
  stdio: ["ignore", "ignore", "pipe"],
  env: { ...process.env, XDG_CONFIG_HOME: join(scratch, "config") },
});
let serverLog = "";
server.stderr.on("data", (d) => (serverLog += d));

const ui = Bun.serve({
  port: UI_PORT,
  fetch(req) {
    const path = new URL(req.url).pathname;
    const file = Bun.file(join(REPO, "crates/aristide-console/ui", path === "/" ? "index.html" : path));
    return file.exists().then((ok) => (ok ? new Response(file) : new Response("nope", { status: 404 })));
  },
});

const chrome = spawn("chromium", [
  "--headless", "--disable-gpu", `--remote-debugging-port=${CDP_PORT}`,
  `--window-size=1500,950`,
  `--user-data-dir=${join(scratch, "chrome")}`, "about:blank",
], { stdio: "ignore" });

const done = async (code) => {
  try { chrome.kill(); } catch {}
  try { server.kill(); } catch {}
  try { ui.stop(true); } catch {}
  try { rmSync(scratch, { recursive: true, force: true }); } catch {}
  process.exit(code);
};

try {
  // Wait for the server, then load the demo set — adoption writes the
  // wrapper organ file under the throwaway config dir.
  for (let i = 0; i < 100; i++) {
    try { await state(); break; } catch { await sleep(200); }
  }
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
  for (const wanted of ["Tuning…", "Room & noises…", "Bindings…"]) {
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

  await drive.eval(`window.__apiPosts = 0;
    const realFetch = window.fetch;
    window.fetch = (url, opts) => {
      if (String(url).includes("/api/") && opts?.method === "POST") window.__apiPosts++;
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
  const foreign = await drive.eval(`[...document.querySelectorAll("#prefs select, #prefs input")].length`);
  check(foreign === 0, "the dialog holds no selects or inputs — appearance buttons only");
  await drive.shot(join(OUT, "prefs-user-only.png"));
  await drive.eval(`document.querySelectorAll("#accent-swatches .swatch")[2]?.click();
    document.querySelectorAll("#density-segment button")[0]?.click(); true`);
  await sleep(200);
  const posts = await drive.eval(`window.__apiPosts`);
  check(posts === 0, `appearance edits sent ${posts} API commands (want 0)`);
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
  await drive.eval(`const a4 = document.getElementById("editor-tuning-a4");
    a4.value = 415; a4.dispatchEvent(new Event("change", {bubbles: true})); true`);
  await sleep(400);
  snap = await state();
  check(Math.abs(snap.tuning.a4 - 415) < 0.01, `a′ committed live (${snap.tuning.a4})`);
  check(/\[tuning\][^[]*a4_hz\s*=\s*415/s.test(fileText()), "…and written to the file's [tuning]");
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
    .find((b) => b.textContent.includes("Bindings")).click()`);
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

  console.log(failures ? `\n${failures} FAILURES` : "\nall green");
  if (failures) console.log("server log tail:\n" + serverLog.slice(-2000));
  await done(failures ? 1 : 0);
} catch (err) {
  console.error("audit crashed:", err);
  console.log("server log tail:\n" + serverLog.slice(-2000));
  await done(2);
}
