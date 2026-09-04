// End-to-end audit of the microtonal hex field, against the REAL
// server and the REAL console UI — the CSS/JS geometry has no unit
// tests, so this is its regression net.
//
//   bun tools/e2e/hex-audit.js [path-to-aristide-server]
//
// Needs: a built server (default target/release/aristide-server, falls
// back to target/debug), chromium on PATH, and no listener on the
// ports below. Creates a throwaway organ named "HexAudit" in the
// isolated organ library and removes it again on the way out.
//
// What it proves:
//   1. GEOMETRY — every hex sits exactly where the layout's two
//      step-vectors put it, and carries exactly the key number the
//      isomorphism implies (screen position and data-midi are checked
//      independently against the snapshot's own layout numbers).
//   2. LIVE EDITS — a burst of layout edits faster than any reload
//      could settle all land, with no error and no loading strip:
//      the popover's numbers, the file, and the board agree.
//   3. COLOURS — a bound Lumatone .ltn's key colours reach the hexes
//      (skipped, with a note, when the machine has no real MIDI port).

import { connect, launchHarness } from "./cdp.js";
import { writeFileSync, rmSync } from "node:fs";
import { join } from "node:path";

const SERVER_PORT = 9876;
const UI_PORT = 9877;
const CDP_PORT = 9223;
// Use an isolated configuration, like the other audits. No player files are touched.
const h = launchHarness({
  name: "hex-audit", serverPort: SERVER_PORT, uiPort: UI_PORT, cdpPort: CDP_PORT,
  needsDemo: false, isolateConfig: true, windowSize: false,
});
const { S, scratch, check, sleep, state, post, waitForServer } = h;

const cleanup = async () => {
  try {
    const organs = (await state()).library ?? [];
    const mine = organs.find((o) => o.name === "HexAudit");
    if (mine) {
      await post(`/api/library/forget?path=${encodeURIComponent(mine.path)}`);
      rmSync(mine.path, { force: true });
    }
  } catch {}
  await h.cleanup();
};

try {
  // ---- a throwaway microtonal organ ---------------------------------
  await waitForServer(50);
  await post("/api/organ/new?name=HexAudit");
  await sleep(1500);
  await post("/api/organ/manual/add?name=Hex&kind=microtonal&low=36&high=96");
  await sleep(1500);
  let snap = await state();
  const hex = snap.manuals[0]?.hex;
  check(!!hex, `microtonal manual carries an effective layout (${JSON.stringify(hex)})`);

  // ---- geometry ------------------------------------------------------
  const d = await connect(CDP_PORT);
  await d.send("Emulation.setDeviceMetricsOverride", {
    width: 1500, height: 950, deviceScaleFactor: 1, mobile: false,
  });
  await d.navigate(`http://127.0.0.1:${UI_PORT}/index.html?server=${S}`);
  await sleep(500);
  await d.eval(`[...document.querySelectorAll(".keyboard-toggle")].forEach(b=>b.click())`);
  await d.sleep(2000);

  const audit = await d.eval(`(() => {
    const manual = ${JSON.stringify(snap.manuals[0])};
    const hex = manual.hex;
    const keys = [...document.querySelectorAll('.keyboard.microtonal .key.hex')];
    if (keys.length !== hex.rows * hex.cols) {
      return { fail: 'hex count ' + keys.length + ' != rows*cols ' + hex.rows * hex.cols };
    }
    const style = getComputedStyle(document.querySelector('.keyboard.microtonal'));
    const w = parseFloat(style.getPropertyValue('--hex-w'));
    const h = parseFloat(style.getPropertyValue('--hex-h'));
    const board = document.querySelector('.keyboard.microtonal .keys').getBoundingClientRect();
    const last = manual.first_key + manual.key_count;
    const bad = [];
    keys.forEach((k, i) => {
      // hexKeys appends row-major from the bottom row up.
      const row = Math.floor(i / hex.cols);
      const col = i % hex.cols;
      const r = k.getBoundingClientRect();
      // The bezel gap is a transform:scale — the untransformed cell is
      // what the lattice positions, so measure centers, which scaling
      // about the center preserves.
      const cx = r.x + r.width / 2 - board.x;
      const cy = r.y + r.height / 2 - board.y;
      const ex = (col + (row % 2) * 0.5) * w + w / 2;
      const ey = (hex.rows - 1 - row) * h * 0.75 + h / 2;
      const key = hex.anchor + (col - Math.floor(row / 2)) * hex.right + row * hex.upright;
      const dead = key < manual.first_key || key >= last;
      if (Math.abs(cx - ex) > 0.6 || Math.abs(cy - ey) > 0.6) {
        bad.push({ i, why: 'position', cx, cy, ex, ey });
      } else if (dead !== k.classList.contains('dead')) {
        bad.push({ i, why: 'deadness', key });
      } else if (!dead && Number(k.dataset.midi) !== key) {
        bad.push({ i, why: 'key number', got: k.dataset.midi, key });
      }
    });
    return { checked: keys.length, bad: bad.slice(0, 5) };
  })()`);
  check(
    !audit.fail && audit.bad.length === 0,
    `geometry: ${audit.checked ?? 0} hexes on the isomorphic lattice` +
      (audit.fail ? ` — ${audit.fail}` : audit.bad?.length ? ` — ${JSON.stringify(audit.bad)}` : "")
  );

  // ---- live edits ----------------------------------------------------
  await d.navigate(`http://127.0.0.1:${UI_PORT}/index.html?server=${S}&kbdHexForm=Hex`);
  await sleep(500);
  await d.eval(`[...document.querySelectorAll(".keyboard-toggle")].forEach(b=>b.click())`);
  await d.sleep(2200);
  await d.click('[data-preset="wicki-hayden"]');
  await d.sleep(120);
  await d.set("#editor-hex-rows", "6");
  await d.sleep(120);
  await d.set("#editor-hex-cols", "21");
  await d.sleep(1200);
  const live = await d.eval(`(() => ({
    upright: document.getElementById('editor-hex-upright').value,
    rows: document.getElementById('editor-hex-rows').value,
    cols: document.getElementById('editor-hex-cols').value,
    err: document.getElementById('editor-hex-error').textContent,
    hexes: document.querySelectorAll('.keyboard.microtonal .key.hex').length,
    loading: !document.getElementById('editor-status').classList.contains('hidden'),
  }))()`);
  check(
    live.upright === "7" && live.rows === "6" && live.cols === "21" &&
      live.hexes === 126 && !live.err && !live.loading,
    `live edits: rapid preset+rows+cols all landed without a reload (${JSON.stringify(live)})`
  );
  snap = await state();
  check(
    snap.manuals[0].hex.upright === 7 && snap.manuals[0].hex.rows === 6,
    "the snapshot agrees with the form"
  );

  // ---- computer keyboard as a hex surface ----------------------------
  await post(`/api/midi/bind?manual=0&slot=0&device=${encodeURIComponent("Computer keyboard")}`);
  await d.navigate(`http://127.0.0.1:${UI_PORT}/index.html?server=${S}`);
  await sleep(500);
  await d.eval(`[...document.querySelectorAll(".keyboard-toggle")].forEach(b=>b.click())`);
  await d.sleep(2000);
  snap = await state();
  const hx = snap.manuals[0].hex;
  for (const code of ["KeyZ", "KeyS"]) {
    await d.eval(`window.dispatchEvent(new KeyboardEvent('keydown', { code: ${JSON.stringify(code)} }))`);
  }
  await d.sleep(800);
  const heldNow = (await state()).manuals[0].held;
  // The slanted reading: S sits physically up-right of Z, so it sounds
  // one up-right step (+upright), nothing more.
  const expected = [hx.anchor, hx.anchor + hx.upright].sort((a, b) => a - b);
  check(
    JSON.stringify(heldNow) === JSON.stringify(expected),
    `computer keyboard: Z and S land on the layout's own lattice (${JSON.stringify(heldNow)} vs ${JSON.stringify(expected)})`
  );
  const legendRows = await d.eval(`(() => {
    document.getElementById('keys-legend').classList.remove('hidden');
    return document.querySelectorAll('#keys-legend .legend-row').length;
  })()`);
  check(legendRows === 4, `legend shows the four grid rows (${legendRows})`);
  for (const code of ["KeyZ", "KeyS"]) {
    await d.eval(`window.dispatchEvent(new KeyboardEvent('keyup', { code: ${JSON.stringify(code)} }))`);
  }

  // ---- colours -------------------------------------------------------
  const port = (snap.midi?.ports ?? []).find((p) => !p.virtual);
  if (!port) {
    console.log("SKIP  colours: no real MIDI port on this machine");
  } else {
    const palette = ["FF5555", "FFE04A", "5FD98A", "4FB6F0", "9A6CF0", "E85FA8"];
    const lines = ["[Board0]"];
    for (let i = 0; i < 20; i++) {
      lines.push(`Key_${i}=${36 + i}`, `Chan_${i}=1`, `Col_${i}=${palette[i % 6]}`);
    }
    const ltn = join(scratch, "audit.ltn");
    writeFileSync(ltn, lines.join("\n") + "\n");
    await post(
      `/api/midi/bind?manual=0&slot=0&device=${encodeURIComponent(port.name)}&map=${encodeURIComponent(ltn)}`
    );
    await d.sleep(1200);
    const colours = await d.eval(`(() => {
      const k = document.querySelector('.keyboard.microtonal .key.hex[data-midi="36"]');
      return k && getComputedStyle(k).backgroundColor;
    })()`);
    check(colours === "rgb(255, 85, 85)", `colours: key 36 wears its .ltn colour (${colours})`);
  }
} finally {
  await cleanup();
}
console.log(h.failures ? `\n${h.failures} FAILURE(S)` : "\nall green");
process.exit(h.failures ? 1 : 0);
