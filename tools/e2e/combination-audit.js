// The combination action, driven end to end: a real server on a null
// ALSA device, the real console UI, and real CDP pointer input on the
// piston rail — the same rig as the other audits here (see
// tools/e2e/cdp.js and the console-e2e-repro-rig note).
//
//   bun tools/e2e/combination-audit.js [path/to/aristide-server]
//
// What it proves:
//   1. RAIL — the "Combinations" panel exists on the canvas with the
//      general pistons, Set, Cancel, the stepper's readout and the
//      crescendo, and each keyboard panel carries its division's own
//      pistons.
//   2. GENERAL — Set, then a general piston, stores; Cancel wipes the
//      console; the same piston brings the registration back. Every
//      press is a human-length press through Input.dispatchMouseEvent,
//      because a synthetic el.click() cannot reproduce the press/release
//      drift that the poll-churn invariant is about.
//   3. DIVISIONAL — a divisional piston under one keyboard recalls that
//      division's stops and leaves another division alone.
//   4. CRESCENDO — a stored stage, recalled by dragging the pedal, adds
//      its stop *over* the hand: the knob lights as crescendo-held (lit,
//      not drawn), and rolling back to the heel takes away only what the
//      pedal added.
//   5. QUIET — a second of polling makes no mutation inside the rail,
//      and a 300 ms press on a piston's own numeral still lands.

import { connect, launchHarness } from "./cdp.js";

const SERVER_PORT = 9910;
const UI_PORT = 9911;
const CDP_PORT = 9241;
const h = launchHarness({
  name: "combination",
  serverPort: SERVER_PORT,
  uiPort: UI_PORT,
  cdpPort: CDP_PORT,
});
const { S, demo, check, sleep, state, settled, post, waitForServer, done } = h;

try {
  await waitForServer();
  await fetch(S + `/api/organ/load?path=${encodeURIComponent(demo)}`, { method: "POST" });
  const snap = await settled();

  const drive = await connect(CDP_PORT);
  await drive.navigate(`http://127.0.0.1:${UI_PORT}/?server=${encodeURIComponent(S)}`);
  await sleep(1500);

  const mouse = (type, x, y) =>
    drive.send("Input.dispatchMouseEvent", {
      type, x: Math.round(x), y: Math.round(y), button: "left", clickCount: 1,
    });
  /// A human press — held across several polls — on the centre of a
  /// control's own text, which is the case WebKit drops when a poll
  /// replaced the text node under the pointer.
  const press = async (selector, ms = 300) => {
    const box = await drive.eval(`(() => {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) return null;
      const inner = el.querySelector("span") ?? el;
      const r = inner.getBoundingClientRect();
      return JSON.stringify({ x: r.x + r.width / 2, y: r.y + r.height / 2 });
    })()`);
    if (!box) return check(false, `press: no ${selector}`);
    const { x, y } = JSON.parse(box);
    await mouse("mousePressed", x, y);
    await sleep(ms);
    await mouse("mouseReleased", x, y);
    await sleep(350);
    return true;
  };
  const count = (selector) =>
    drive.eval(`document.querySelectorAll(${JSON.stringify(selector)}).length`);
  const text = (selector) =>
    drive.eval(`document.querySelector(${JSON.stringify(selector)})?.textContent ?? null`);
  const drawn = async (name) => {
    const s = await state();
    return s.stops.find((stop) => stop.name === name);
  };

  // 1. RAIL
  check(await count(`[data-panel="pistons"]`) === 1, "the Combinations panel is on the canvas");
  check(await count(`[data-panel="pistons"] .piston`) >= 8, "it carries a bank of general pistons");
  check(await count(`[data-key="setter"]`) === 1, "…with Set");
  check(await count(`[data-key="cancel-rail"]`) === 1, "…with Cancel");
  check((await text(".stepper-frame")) !== null, "…with the stepper's frame readout");
  check(await count(".crescendo-track") === 1, "…with the crescendo pedal");
  const manuals = snap.manuals.length;
  check(await count(".divisional-rail") === manuals,
    `every keyboard panel has its division's pistons (${manuals})`);

  // 2. GENERAL — the demo's Montre 8' stands in for "the registration".
  const montre = snap.stops.find((s) => s.name === "Montre 8'");
  const hautbois = snap.stops.find((s) => s.name === "Hautbois 8'");
  check(!!montre && !!hautbois, "the demo offers Montre 8' (First) and Hautbois 8' (Second)");
  await press(`.knob[data-key="stop-${montre.id}"]`);
  check((await drawn("Montre 8'")).on, "a press on the drawknob draws Montre 8'");
  await press(`[data-key="setter"]`);
  check((await state()).setter, "Set arms the setter");
  await press(`[data-key="general-1"]`);
  check(!(await state()).setter, "a general press with Set armed stores, and disarms");
  await press(`[data-key="cancel-rail"]`);
  check(!(await drawn("Montre 8'")).on, "Cancel pushes the drawknob in again");
  await press(`[data-key="general-1"]`);
  check((await drawn("Montre 8'")).on, "general 1 brings the registration back");

  // 3. DIVISIONAL — scoped to the manual its rail sits under.
  const first = montre.midx;
  const second = hautbois.midx;
  check(first !== second, "the two stops sit on different divisions");
  await press(`.knob[data-key="stop-${hautbois.id}"]`);
  await press(`[data-key="setter"]`);
  await press(`[data-key="divisional-${first}-1"]`);
  await press(`.knob[data-key="stop-${montre.id}"]`); // retire it again
  check(!(await drawn("Montre 8'")).on, "…and Montre 8' is off before the recall");
  await press(`[data-key="divisional-${first}-1"]`);
  check((await drawn("Montre 8'")).on, "the divisional brings its own division's stop back");
  check((await drawn("Hautbois 8'")).on, "…and leaves the other division exactly as it was");

  // 4. CRESCENDO — stored through the API (a stage is set up, not
  // played), then recalled by dragging the pedal itself.
  await post(`/api/cancel`);
  await press(`.knob[data-key="stop-${hautbois.id}"]`);
  await post(`/api/crescendo?stage=1&store=1`);
  await post(`/api/cancel`);
  await press(`.knob[data-key="stop-${montre.id}"]`); // the hand's own stop
  const track = JSON.parse(await drive.eval(`(() => {
    const r = document.querySelector(".crescendo-track").getBoundingClientRect();
    return JSON.stringify({ x: r.x, y: r.y + r.height / 2, w: r.width });
  })()`));
  await mouse("mousePressed", track.x + track.w * 0.5, track.y);
  await mouse("mouseReleased", track.x + track.w * 0.5, track.y);
  await sleep(400);
  check((await state()).combinations.crescendo > 0, "dragging the pedal moves it off the heel");
  check((await drawn("Hautbois 8'")).on, "the stage's stop sounds without the hand drawing it");
  check((await drawn("Hautbois 8'")).hand === false, "…and the snapshot says the hand has it in");
  check(await count(`.knob[data-key="stop-${hautbois.id}"].crescendo-held`) === 1,
    "…and the console draws it lit-but-not-drawn");
  check((await drawn("Montre 8'")).on, "the hand's own stop is untouched by the pedal");
  await mouse("mousePressed", track.x, track.y);
  await mouse("mouseReleased", track.x, track.y);
  await sleep(400);
  check((await state()).combinations.crescendo === 0, "back to the heel");
  check(!(await drawn("Hautbois 8'")).on, "what the pedal added, the pedal took away");
  check((await drawn("Montre 8'")).on, "what the hand drew survives");

  // 5. QUIET — the rail must not be rewritten under the pointer.
  await drive.eval(`(() => {
    window.__churn = 0;
    const target = document.querySelector('[data-panel="pistons"]');
    window.__obs = new MutationObserver((records) => { window.__churn += records.length; });
    window.__obs.observe(target, { childList: true, characterData: true, subtree: true });
  })()`);
  await sleep(1000);
  const churn = await drive.eval(`(() => { window.__obs.disconnect(); return window.__churn; })()`);
  check(churn === 0, `the rail is mutation-free over a second of polling (${churn})`);
  // A press has to *land*, not merely not crash: general 1 holds Montre
  // and nothing else, so recalling it must retire the stop just drawn.
  await press(`.knob[data-key="stop-${hautbois.id}"]`);
  check((await drawn("Hautbois 8'")).on, "…with a stop drawn that general 1 does not hold");
  await press(`[data-key="general-1"]`, 300);
  check(!(await drawn("Hautbois 8'")).on,
    "a 300 ms press on a piston's own numeral recalls (the general retires what it hasn't got)");
  check((await state()).combinations != null, "…and the snapshot still carries the combination state");

  console.log(h.failures ? `\n${h.failures} check(s) failed` : "\nall checks passed");
  if (h.failures && h.serverLog) console.log("--- server log ---\n" + h.serverLog.slice(-2000));
  await done(h.failures ? 1 : 0);
} catch (err) {
  console.error("audit crashed:", err);
  console.log("--- server log ---\n" + h.serverLog.slice(-2000));
  await done(2);
}
