// End-to-end audit of click versus drag on the console's drag sources
// (stop drawknobs, coupler rockers), against the REAL server and the
// REAL console UI: a mouse button's own travel wobbles the pointer a
// few pixels, and a wobbly click on a drawknob in edit mode must still
// be a click.
//
//   bun tools/e2e/click-vs-drag-audit.js [path-to-aristide-server]
//
// Needs: a built server (default target/release, falls back to debug),
// chromium on PATH, the gitignored demo set at testsets/grandorgue-demo,
// and no listener on the ports below. Runs against a throwaway
// XDG_CONFIG_HOME, so the real library and wiring are never touched.
//
// What it proves, with press/move/release as real pointer input:
//   1. WOBBLE — a press that drifts a few pixels before release still
//      toggles the knob, locked and unlocked.
//   2. STAYED — a press that drifts past the drag threshold but is let
//      go still over the knob is a click, not a drag: the knob toggles
//      and the division's rank is untouched.
//   3. DRAG — a press carried onto another knob reorders the rank and
//      does NOT toggle the dragged stop.
//   4. AFTERWARDS — the very next plain click on that knob toggles it:
//      a finished drag leaves no click-swallowing listener behind.

import { connect } from "./cdp.js";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const REPO = new URL("../..", import.meta.url).pathname;
const SERVER_PORT = 9898;
const UI_PORT = 9899;
const CDP_PORT = 9236;
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

const scratch = mkdtempSync(join(tmpdir(), "aristide-click-vs-drag-"));
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
  await settled();

  const drive = await connect(CDP_PORT);
  const mouse = (type, x, y) =>
    drive.send("Input.dispatchMouseEvent", {
      type, x: Math.round(x), y: Math.round(y), button: "left", clickCount: 1,
    });
  // A human press: down, a moment, the pointer carried to (x+dx, y+dy)
  // in a couple of steps while held, a moment, up.
  const press = async (x, y, dx = 0, dy = 0) => {
    await mouse("mouseMoved", x, y);
    await mouse("mousePressed", x, y);
    await sleep(40);
    await mouse("mouseMoved", x + dx / 2, y + dy / 2);
    await mouse("mouseMoved", x + dx, y + dy);
    await sleep(40);
    await mouse("mouseReleased", x + dx, y + dy);
    await sleep(300);
  };
  const knobCenter = (key) =>
    drive.eval(`(() => { const r = document.querySelector('.knob[data-key="${key}"]').getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height }; })()`);
  const isOn = (key) =>
    drive.eval(`document.querySelector('.knob[data-key="${key}"]').classList.contains("on")`);
  const serverOn = async (id) => (await state()).stops.find((s) => s.id === id).on;
  const rankOf = async (midx) => (await state()).manuals.find((m) => m.idx === midx).rank ?? null;

  // Two stops side by side in one division, so a drag has somewhere
  // real to land and a wobble has room to stay inside its knob.
  const snap = await state();
  const first = snap.stops.find((s) => snap.stops.some((o) => o.midx === s.midx && o.id !== s.id));
  const second = snap.stops.find((s) => s.midx === first.midx && s.id !== first.id);
  const key = `stop-${first.id}`;
  console.log(`subject: ${first.name} (id ${first.id}) beside ${second.name} on manual ${first.midx}`);

  for (const mode of ["locked", "unlocked"]) {
    await drive.navigate(
      `http://127.0.0.1:${UI_PORT}/?server=${encodeURIComponent(S)}${mode === "unlocked" ? "&unlock=1" : ""}`
    );
    await sleep(1500);
    check((await drive.eval(`document.body.classList.contains("editing")`)) === (mode === "unlocked"),
      `${mode}: console is ${mode}`);
    const c = await knobCenter(key);

    // 1. WOBBLE
    for (const [dx, dy] of [[0, 0], [3, 1], [5, -2]]) {
      const before = await isOn(key);
      await press(c.x, c.y, dx, dy);
      check((await isOn(key)) !== before && (await serverOn(first.id)) !== before,
        `${mode}: press drifting (${dx},${dy}) toggles the knob`);
    }

    // 2. STAYED — past the threshold, still inside the knob
    const rankBefore = await rankOf(first.midx);
    for (const [dx, dy] of [[12, 0], [-14, 6], [0, 10]]) {
      const before = await isOn(key);
      await press(c.x, c.y, dx, dy);
      check((await isOn(key)) !== before && (await serverOn(first.id)) !== before,
        `${mode}: press drifting (${dx},${dy}) yet released over the knob still toggles it`);
    }
    check(JSON.stringify(await rankOf(first.midx)) === JSON.stringify(rankBefore),
      `${mode}: no reorder came of those releases`);
    check((await drive.eval(`document.querySelectorAll(".organ-drag-ghost").length`)) === 0,
      `${mode}: no drag ghost is left on the page`);
  }

  // 3. DRAG (unlocked): carry the first knob onto the far side of the
  // second. A sample set's own organ refuses structural edits (409 →
  // the save-as card), so the drag lands on a copy with a name of its own.
  {
    const saved = await fetch(S + `/api/organ/save_as?name=${encodeURIComponent("Click vs drag audit")}`, { method: "POST" });
    check(saved.ok, `unlocked: the demo saved as an organ of its own (${saved.status})`);
    await settled();
    await sleep(800);
    const before = await isOn(key);
    const rankBefore = await rankOf(first.midx);
    const c = await knobCenter(key);
    const t = await knobCenter(`stop-${second.id}`);
    await press(c.x, c.y, t.x + t.w * 0.4 - c.x, t.y - c.y);
    await settled();
    await sleep(600);
    check((await isOn(key)) === before && (await serverOn(first.id)) === before,
      `unlocked: a drag onto the neighbouring knob does not toggle the dragged stop`);
    check(JSON.stringify(await rankOf(first.midx)) !== JSON.stringify(rankBefore),
      `unlocked: the drag reordered the rank (${JSON.stringify(rankBefore)} → ${JSON.stringify(await rankOf(first.midx))})`);
  }

  // 4. AFTERWARDS: the next plain click on the dragged knob is honoured.
  {
    const c = await knobCenter(key);
    const before = await isOn(key);
    await press(c.x, c.y);
    check((await isOn(key)) !== before && (await serverOn(first.id)) !== before,
      `unlocked: the first plain click after a drag toggles the knob`);
  }

  console.log(failures ? `\n${failures} check(s) failed` : "\nall checks passed");
  if (failures && serverLog) console.log("--- server log ---\n" + serverLog.split("\n").filter((l) => l.includes("http ")).slice(-30).join("\n"));
  await done(failures ? 1 : 0);
} catch (err) {
  console.error("audit crashed:", err);
  console.log("--- server log ---\n" + serverLog.slice(-2000));
  await done(2);
}
