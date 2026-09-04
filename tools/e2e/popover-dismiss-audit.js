// End-to-end audit of popover dismissal, against the REAL server and
// the REAL console UI, with REAL pointer and keyboard input over CDP:
// a popover closes on a press outside it, and whatever was being typed
// in it commits first.
//
//   bun tools/e2e/popover-dismiss-audit.js [path-to-aristide-server]
//
// Needs: a built server (default target/release, falls back to debug),
// chromium on PATH, the gitignored demo set at testsets/grandorgue-demo,
// and no listener on the ports below. Runs against a throwaway
// XDG_CONFIG_HOME on a saved copy of the demo, so nothing real is touched.
//
// Why real input: the bug this guards only shows with it. A press on a
// key or a panel cancels the browser's own focus move, so the field's
// change event arrives AFTER the click — and a popover that closed on
// the click had already forgotten which stop it was editing, dropping
// the edit on the floor (type a brightness, click off, it snaps back).
// Synthetic change events never see that ordering.
//
// What it proves, for the stop editor on Trompette 8':
//   1. COMMIT — a brightness typed without Tab or Enter lands on the
//      server when the click-off is on a panel body, the panel chrome,
//      or a key: the targets whose press handlers cancel the default.
//   2. DRAG — a drag that starts inside the popover (selecting the
//      name) and ends outside it leaves the popover open.
//   3. PRESS — pressing another drawknob commits the field, closes the
//      popover, and still pulls that stop.

import { connect, launchHarness } from "./cdp.js";

const SERVER_PORT = 9902;
const UI_PORT = 9903;
const CDP_PORT = 9238;
const h = launchHarness({ name: "popover-dismiss-audit", serverPort: SERVER_PORT, uiPort: UI_PORT, cdpPort: CDP_PORT });
const { S, demo, check, sleep, state, settled, post, done } = h;

try {
  await h.waitForServer();
  await post(`/api/organ/load?path=${encodeURIComponent(demo)}`);
  await settled();
  await post(`/api/organ/save_as?name=${encodeURIComponent("Popover dismiss audit")}`);
  const snap = await settled();
  const stop = snap.stops.find((s) => /Trompette/.test(s.name)) ?? snap.stops.find((s) => s.pitch?.native != null);
  check(!!stop, `an audit subject (${stop?.name})`);

  const drive = await connect(CDP_PORT);
  await drive.navigate(`http://127.0.0.1:${UI_PORT}/?server=${encodeURIComponent(S)}`);
  await sleep(400);
  await drive.eval(`[...document.querySelectorAll('.keyboard-toggle')].forEach(b=>b.click())`);
  await sleep(1500);

  const mouse = (type, x, y, button = "left") =>
    drive.send("Input.dispatchMouseEvent", { type, x, y, button, clickCount: 1 });
  const center = (sel) =>
    drive.eval(`(() => { const r = document.querySelector(${JSON.stringify(sel)}).getBoundingClientRect();
      return [r.left + r.width / 2, r.top + r.height / 2]; })()`);
  const clickAt = async (sel, button = "left") => {
    const [x, y] = await center(sel);
    await mouse("mouseMoved", x, y);
    await mouse("mousePressed", x, y, button);
    await mouse("mouseReleased", x, y, button);
    await sleep(300);
  };
  const dragFromTo = async (a, b) => {
    const [x1, y1] = await center(a);
    const [x2, y2] = await center(b);
    await mouse("mouseMoved", x1, y1);
    await mouse("mousePressed", x1, y1);
    for (let i = 1; i <= 5; i++) await mouse("mouseMoved", x1 + ((x2 - x1) * i) / 5, y1 + ((y2 - y1) * i) / 5);
    await mouse("mouseReleased", x2, y2);
    await sleep(300);
  };
  const type = async (text) => {
    for (const ch of text) {
      await drive.send("Input.dispatchKeyEvent", { type: "keyDown", text: ch, key: ch });
      await drive.send("Input.dispatchKeyEvent", { type: "keyUp", key: ch });
    }
  };
  const selectAll = async () => {
    await drive.send("Input.dispatchKeyEvent", { type: "keyDown", key: "a", code: "KeyA", modifiers: 2, windowsVirtualKeyCode: 65 });
    await drive.send("Input.dispatchKeyEvent", { type: "keyUp", key: "a", code: "KeyA", modifiers: 2 });
  };
  const visible = (sel) => drive.eval(`!document.querySelector(${JSON.stringify(sel)}).classList.contains("hidden")`);
  const brightness = async () => (await state()).stops.find((s) => s.id === stop.id).pitch.brightness;
  const knob = `.knob[data-key="stop-${stop.id}"]`;
  const openEditor = async () => {
    await clickAt(knob, "right");
    return visible("#editor-stop");
  };
  const typeBrightness = async (value) => {
    await clickAt("#editor-stop-brightness");
    await selectAll();
    await type(String(value));
  };

  await clickAt("#editor-lock");
  check(await drive.eval(`document.body.classList.contains("editing")`), "the padlock unlocks the console");

  // ---- 1. COMMIT on a click-off ------------------------------------

  const targets = [
    ["a jamb panel", ".panel"],
    ["the panel chrome", ".panel-chrome"],
    ["a key", '.keyboard[data-manual="0"] .key[data-midi]'],
  ];
  let value = 1;
  for (const [name, sel] of targets) {
    check(await openEditor(), `right-click opens the editor on ${stop.name}`);
    await typeBrightness(value);
    await clickAt(sel);
    await sleep(700);
    check((await brightness()) === value, `brightness ${value} typed without Enter commits when clicking off on ${name}`);
    check(!(await visible("#editor-stop")), `…and the popover closes`);
    await drive.eval(`document.querySelectorAll(".key.pressed").forEach((k) => k.classList.remove("pressed"))`);
    value += 1;
  }

  // ---- 2. DRAG out of the popover ----------------------------------

  check(await openEditor(), "the editor opens again");
  await dragFromTo("#editor-stop-name", ".panel-chrome");
  check(await visible("#editor-stop"), "a drag from inside the popover to outside leaves it open");

  // ---- 3. PRESS on another drawknob --------------------------------

  const other = await drive.eval(
    `[...document.querySelectorAll('.knob[data-key^="stop-"]')].map((k) => k.dataset.key).find((k) => k !== "stop-${stop.id}")`
  );
  await typeBrightness(value);
  await clickAt(`.knob[data-key="${other}"]`);
  await sleep(700);
  check((await brightness()) === value, "pressing another drawknob commits the field first");
  check(!(await visible("#editor-stop")), "…closes the popover");
  check(
    await drive.eval(`document.querySelector('.knob[data-key="${other}"]').classList.contains("on")`),
    "…and still pulls that stop"
  );
} catch (e) {
  check(false, "audit completed without an exception");
  console.error("ERR", e);
}
console.log(h.failures ? `\n${h.failures} FAILED` : "\nall green");
await done(h.failures ? 1 : 0);
