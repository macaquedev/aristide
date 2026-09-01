// End-to-end audit of the console's poll-time DOM writes, against the
// REAL server and the REAL console UI. Anything repainted on every
// 120 ms poll must repaint only what changed: WebKit (the Tauri engine
// on Linux and macOS) drops a click when the text node a press landed
// on has been replaced by the release, and every engine loses a
// button's own listener when the button itself was rebuilt mid-press
// (see ui/js/dom.js). A human press lasts about as long as a poll
// interval, so a control rewritten per poll fails most real clicks.
//
//   bun tools/e2e/poll-churn-audit.js [path-to-aristide-server]
//
// Needs: a built server (default target/release, falls back to debug),
// chromium on PATH, the gitignored demo set at testsets/grandorgue-demo,
// and no listener on the ports below. Runs against a throwaway
// XDG_CONFIG_HOME, so the real library and wiring are never touched.
//
// What it proves:
//   1. QUIET — with the organ idle, a second of polling makes no
//      childList or text mutation in the tuning readout, the whole-
//      instrument tuning popover, a mixture stop's editor, or that
//      stop's tuning popover with its "open →" cascade link.
//   2. PRESSED — human-length presses (held across several polls) on
//      the readout, a rank's "Edit…" and the "open →" link each land.
//
// Chromium is immune to the text-node case by itself, so QUIET is
// what guards the WebKit symptom here; PRESSED guards the rebuilt-
// button case, which every engine shows.

import { connect } from "./cdp.js";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const REPO = new URL("../..", import.meta.url).pathname;
const SERVER_PORT = 9900;
const UI_PORT = 9901;
const CDP_PORT = 9237;
const S = `http://127.0.0.1:${SERVER_PORT}`;

let failures = 0;
const check = (ok, what) => {
  console.log(`${ok ? "PASS" : "FAIL"}  ${what}`);
  if (!ok) failures++;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
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

const scratch = mkdtempSync(join(tmpdir(), "aristide-poll-churn-"));
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
  for (let i = 0; i < 100; i++) {
    try { await state(); break; } catch { await sleep(200); }
  }
  await fetch(S + `/api/organ/load?path=${encodeURIComponent(demo)}`, { method: "POST" });
  const snap = await settled();

  const drive = await connect(CDP_PORT);
  await drive.navigate(`http://127.0.0.1:${UI_PORT}/?server=${encodeURIComponent(S)}`);
  await sleep(1500);

  const mouse = (type, x, y) =>
    drive.send("Input.dispatchMouseEvent", {
      type, x: Math.round(x), y: Math.round(y), button: "left", clickCount: 1,
    });
  // A human press on a control's own text, held across several polls.
  const press = async (selector, holdMs = 300) => {
    const c = await drive.eval(`(() => { const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) return null;
      const range = document.createRange(); range.selectNodeContents(el);
      const r = range.getBoundingClientRect().width ? range.getBoundingClientRect() : el.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
    if (!c) return false;
    await mouse("mouseMoved", c.x, c.y);
    await mouse("mousePressed", c.x, c.y);
    await sleep(holdMs);
    await mouse("mouseReleased", c.x, c.y);
    await sleep(250);
    return true;
  };
  const visible = (selector) =>
    drive.eval(`(() => { const el = document.querySelector(${JSON.stringify(selector)});
      return !!el && !el.classList.contains("hidden") && el.offsetWidth > 0; })()`);
  const contextMenu = (selector) =>
    drive.eval(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); const r = el.getBoundingClientRect();
      el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, ctrlKey: true,
        clientX: r.left + 8, clientY: r.top + 8 })); return true; })()`);
  // Counts every childList/characterData mutation under `selector`
  // over `ms` of idle polling.
  const churn = (selector, ms = 1000) =>
    drive.eval(`new Promise((resolve) => {
      const el = document.querySelector(${JSON.stringify(selector)});
      const seen = [];
      const mo = new MutationObserver((records) => {
        for (const r of records) seen.push(r.type + ":" + (r.target.id || r.target.className || r.target.nodeName));
      });
      mo.observe(el, { childList: true, characterData: true, subtree: true });
      setTimeout(() => { mo.disconnect(); resolve(seen); }, ${ms});
    })`);
  const quiet = async (label, selector) => {
    const seen = await churn(selector);
    check(seen.length === 0, `${label}: no DOM churn over a second of idle polling${seen.length ? ` (${seen.length}: ${[...new Set(seen)].slice(0, 5).join(", ")})` : ""}`);
  };

  // A mixture: a stop with several ranks, so the editor lists them
  // with their "Edit…" buttons.
  const mixture = snap.stops.find((s) => (s.ranks ?? []).length > 1);
  check(!!mixture, `the demo offers a mixture (${mixture?.name})`);

  // 1. QUIET
  await quiet("tuning readout", "#tuning");
  await press("#tuning");
  check(await visible("#editor-tuning"), "readout: a 300 ms press on its text opens the tuning popover");
  await quiet("whole-instrument tuning popover", "#editor-tuning");
  await drive.eval(`document.body.click()`);
  await sleep(200);

  await contextMenu(`.knob[data-key="stop-${mixture.id}"]`);
  await sleep(400);
  check(await visible("#editor-stop"), `stop editor opened for ${mixture.name}`);
  check((await drive.eval(`document.querySelectorAll("#editor-stop-ranks .stop-rank-row").length`)) === mixture.ranks.length,
    `stop editor lists its ${mixture.ranks.length} ranks`);
  await quiet("stop editor", "#editor-stop");

  // 2. PRESSED — rank Edit… (a rebuilt-per-poll button before the fix)
  await press("#editor-stop-ranks .stop-rank-row button");
  check(await visible("#editor-tuning") && !(await visible("#editor-stop")),
    "rank Edit…: a 300 ms press opens that rank's tuning popover");
  check((await drive.eval(`document.getElementById("editor-tuning-title").textContent`)).startsWith(mixture.ranks[0].name),
    `…titled for the rank (${await drive.eval(`document.getElementById("editor-tuning-title").textContent`)})`);
  // A rank follows its stop by default: the "open →" cascade link shows.
  check(await visible("#editor-tuning-resolved-primary .tuning-open-link"),
    "rank popover shows the cascade's open → link");
  await quiet("rank tuning popover", "#editor-tuning");
  await press("#editor-tuning-resolved-primary .tuning-open-link");
  const title = await drive.eval(`document.getElementById("editor-tuning-title").textContent`);
  check(await visible("#editor-tuning") && title.startsWith(mixture.name),
    `open →: a 300 ms press walks up to the stop's popover (${title})`);
  await press("#editor-tuning-resolved-primary .tuning-open-link");
  check((await drive.eval(`document.getElementById("editor-tuning-title").textContent`)) === "Whole instrument",
    "open → again: up to the whole instrument");

  console.log(failures ? `\n${failures} check(s) failed` : "\nall checks passed");
  if (failures && serverLog) console.log("--- server log ---\n" + serverLog.slice(-1500));
  await done(failures ? 1 : 0);
} catch (err) {
  console.error("audit crashed:", err);
  console.log("--- server log ---\n" + serverLog.slice(-1500));
  await done(2);
}
