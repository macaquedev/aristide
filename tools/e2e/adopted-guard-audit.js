// End-to-end audit of the adoption guard, against the REAL server and
// the REAL console UI: a sample set's own organ takes the player's
// settings (wiring, room, whole-instrument pitch) into its own file,
// refuses every change to the instrument itself until it is saved
// under a different name, and the console answers that refusal with
// the save-as dialog.
//
//   bun tools/e2e/adopted-guard-audit.js [path-to-aristide-server]
//
// Needs: a built server (default target/release, falls back to debug),
// chromium on PATH, the gitignored demo set at testsets/grandorgue-demo,
// and no listener on the ports below. Runs against a throwaway
// XDG_CONFIG_HOME, so the real library and wiring are never touched.
//
// What it proves:
//   1. ADOPTION MARKS — loading the raw demo set writes its own organ
//      file with `adopted = true`, and the snapshot says so.
//   2. SETTINGS LAND — a whole-instrument pitch, a wiring bind and a
//      persisted room setting all answer 200 and are written into the
//      set's own file; it stays the set's own organ.
//   3. THE INSTRUMENT REFUSES — a division's tuning, a structural edit,
//      a rename and a tremulant shape all answer 409; playing answers
//      200; the file and the live state stay as they were.
//   4. THE DIALOG — giving a stop a tuning of its own in its popover
//      opens the save-as dialog (not the error strip), naming the organ.
//   5. SAVE AS — accepting the dialog switches the console to a copy
//      under the new name, the refused tuning lands on the copy (live
//      and in its file), and the original file is byte-identical.
//   6. RELOAD — browsing to the raw set again loads the marked
//      original, not the copy — wired and pitched as the player left
//      it, its stops untouched; the copy loads by its own path.
//   7. MENU — the organ-name menu offers "Save as…", and a plain copy
//      from it makes another organ with nothing replayed.

import { connect, launchHarness } from "./cdp.js";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

const SERVER_PORT = 9896;
const UI_PORT = 9897;
const CDP_PORT = 9236;
const h = launchHarness({ name: "adopted-audit", serverPort: SERVER_PORT, uiPort: UI_PORT, cdpPort: CDP_PORT });
const { REPO, S, demo, check, sleep, state, settled, post, waitForServer, done } = h;
const OUT = join(REPO, "target", "adopted-guard-audit");

try {
  await waitForServer();

  // ---- 1. adoption marks the set's own organ -------------------------

  await post(`/api/organ/load?path=${encodeURIComponent(demo)}`);
  let snap = await settled();
  const organFile = snap.setup?.file;
  const setName = snap.organ;
  check(!!organFile, `the adopted demo lives in a file (${organFile})`);
  check(snap.setup?.adopted === true, "the snapshot says it is the set's own organ");
  const original = readFileSync(organFile, "utf8");
  check(/^adopted = true$/m.test(original), "the file carries adopted = true");

  // ---- 2. the player's settings land on the set's own organ -----------

  const settings = [
    ["/api/tuning?reference_hz=415", "the whole-instrument pitch"],
    ["/api/midi/bind?manual=0&slot=0&device=Computer%20keyboard", "a wiring bind"],
    ["/api/noises?on=1&vol=0.5&persist=1", "a persisted room setting"],
  ];
  for (const [query, what] of settings) {
    const r = await post(query);
    check(r.ok, `${what} answers 200 (${r.status}: ${r.body.slice(0, 60)})`);
  }
  snap = await state();
  check(snap.organ === setName && snap.setup.adopted === true, "the organ is still the set's own");
  check(Math.abs(snap.tuning.reference.hz - 415) < 0.01, `the pitch changed live (${snap.tuning.reference.hz})`);
  check(snap.midi.manuals[0].inputs.some((i) => i.device === "Computer keyboard"), "the binding is live");
  const settled2 = readFileSync(organFile, "utf8");
  check(settled2 !== original, "the set's own file took the settings");
  check(/\[tuning\][^[]*reference_hz\s*=\s*415/s.test(settled2), "…the pitch under [tuning]");
  check(/Computer keyboard/.test(settled2), "…the binding");
  check(/\[noises\][^[]*volume\s*=\s*0\.5/s.test(settled2), "…and the room");

  // ---- 3. the API refuses the instrument itself, allows playing -------

  const kept = settled2;
  const refusals = [
    ["/api/tuning?manual=0&reference_hz=430", "a division's own tuning"],
    ["/api/tuning?stop=1&follow=own", "a stop's own tuning"],
    ["/api/organ/manual/add?name=Solo&low=48&high=84", "a structural edit"],
    ["/api/organ/rename?name=Other", "a rename"],
    ["/api/trem/params?rate=4", "a tremulant shape"],
  ];
  for (const [query, what] of refusals) {
    const r = await post(query);
    check(r.status === 409, `${what} answers 409 (${r.status}: ${r.body.slice(0, 60)})`);
  }
  const played = await post("/api/stop?id=0&on=1");
  check(played.ok, "drawing a stop is playing, not changing — 200");
  snap = await state();
  check(snap.organ === setName && snap.setup.adopted === true, "the organ is still the set's own");
  check(Math.abs(snap.tuning.reference.hz - 415) < 0.01, `pitch untouched live (${snap.tuning.reference.hz})`);
  check(snap.stops.every((s) => s.tuning.follow !== "own"), "no stop took a tuning of its own");
  check(readFileSync(organFile, "utf8") === kept, "the file is byte-identical");

  // ---- 4. the console answers the refusal with the dialog -------------

  const stop = snap.stops[0];
  const drive = await connect(CDP_PORT);
  await drive.navigate(`http://127.0.0.1:${UI_PORT}/?server=${encodeURIComponent(S)}`);
  await sleep(1500);
  await drive.eval(`(() => { const el = document.querySelector('.knob[data-key="stop-${stop.id}"]');
    const r = el.getBoundingClientRect();
    el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, ctrlKey: true,
      clientX: r.left + 8, clientY: r.top + 8 })); return true; })()`);
  await sleep(400);
  check(
    await drive.eval(`!document.getElementById("editor-stop").classList.contains("hidden")`),
    `the stop editor opened for ${stop.name}`
  );
  await drive.eval(`document.getElementById("editor-stop-tuning-edit").click()`);
  await sleep(300);
  check(
    await drive.eval(`!document.getElementById("editor-tuning").classList.contains("hidden")`),
    "…and its tuning popover from there"
  );
  await drive.eval(`const follow = document.getElementById("editor-tuning-follow");
    follow.value = "own"; follow.dispatchEvent(new Event("change", {bubbles: true})); true`);
  await sleep(600);
  const dialogUp = await drive.eval(`!document.getElementById("save-as").classList.contains("hidden")`);
  check(dialogUp, "giving the stop its own tuning opens the save-as dialog");
  const stripUp = await drive.eval(`!document.getElementById("editor-error").classList.contains("hidden")`);
  check(!stripUp, "…and not the error strip");
  const note = await drive.eval(`document.getElementById("save-as-note").textContent`);
  check(note.startsWith(setName) && note.includes("sample set's own organ"), `the note names the organ ("${note.slice(0, 70)}…")`);
  const prefilled = await drive.eval(`document.getElementById("save-as-name").value`);
  check(prefilled === `My ${setName}`, `a name is proposed ("${prefilled}")`);
  await drive.shot(join(OUT, "save-as-dialog.png"));

  // ---- 5. saving switches to the copy and replays the tuning ----------

  await drive.eval(`document.getElementById("save-as-name").value = "My Demo";
    document.getElementById("save-as-btn").click(); true`);
  await sleep(1200);
  snap = await settled();
  const copyFile = snap.setup?.file;
  check(snap.organ === "My Demo", `the console now plays the copy (${snap.organ})`);
  check(snap.setup.adopted === false, "…which takes edits");
  check(copyFile && copyFile !== organFile, `…from its own file (${copyFile})`);
  check(copyFile && dirname(copyFile) === dirname(organFile), "beside the original");
  check(
    await drive.eval(`document.getElementById("save-as").classList.contains("hidden")`),
    "the dialog closed itself"
  );
  const ownStop = snap.stops.find((s) => s.id === stop.id);
  check(ownStop?.tuning.follow === "own", `the refused tuning landed live (${stop.name} follows ${ownStop?.tuning.follow})`);
  const copyText = readFileSync(copyFile, "utf8");
  check(/^name = "My Demo"$/m.test(copyText), "the copy is named");
  check(!/^adopted/m.test(copyText), "the copy carries no adopted mark");
  check(/\[\[tuning\.stop\]\]/.test(copyText), "…and the stop's tuning is written into it");
  check(/Computer keyboard/.test(copyText), "…along with the wiring it inherited");
  check(readFileSync(organFile, "utf8") === kept, "the original is still byte-identical");
  check(
    (snap.library ?? []).some((entry) => entry.name === "My Demo" && entry.path === copyFile),
    "Recent lists the copy by its name"
  );
  await drive.shot(join(OUT, "after-save-as.png"));

  // ---- 6. the raw set still means the marked original -----------------

  await post(`/api/organ/load?path=${encodeURIComponent(demo)}`);
  snap = await settled();
  check(snap.setup.file === organFile && snap.setup.adopted === true, `browsing to the set loads its own organ (${snap.organ})`);
  check(Math.abs(snap.tuning.reference.hz - 415) < 0.01, "…at the pitch the player gave it");
  check(snap.midi.manuals[0].inputs.some((i) => i.device === "Computer keyboard"), "…wired as they left it");
  check(snap.stops.every((s) => s.tuning.follow !== "own"), "…its stops as the set defines them");
  await post(`/api/organ/load?path=${encodeURIComponent(copyFile)}`);
  snap = await settled();
  check(
    snap.organ === "My Demo" && snap.stops.find((s) => s.id === stop.id)?.tuning.follow === "own",
    "the copy loads by its path, with its own tuning"
  );

  // ---- 7. "Save as…" on the organ-name menu ---------------------------

  await sleep(800);
  await drive.eval(`document.getElementById("organ-name").click()`);
  await sleep(200);
  const items = await drive.eval(`[...document.querySelectorAll(".menu-list:not(.hidden) .menu-item")]
    .map((b) => b.textContent.trim())`);
  check(items.includes("Save as…"), `the organ-name menu offers Save as… (${items.join(" / ")})`);
  await drive.eval(`[...document.querySelectorAll(".menu-list:not(.hidden) .menu-item")]
    .find((b) => b.textContent.trim() === "Save as…").click()`);
  await sleep(250);
  const plainNote = await drive.eval(`document.getElementById("save-as-note").textContent`);
  check(plainNote.startsWith("Save a copy of My Demo"), `a plain copy's note ("${plainNote.slice(0, 50)}…")`);
  await drive.eval(`document.getElementById("save-as-name").value = "Second Demo";
    document.getElementById("save-as-btn").click(); true`);
  await sleep(1200);
  snap = await settled();
  check(snap.organ === "Second Demo" && snap.setup.file !== copyFile, `a second copy, switched to (${snap.setup.file})`);
  check(readFileSync(copyFile, "utf8") === copyText, "the first copy is untouched");

  console.log(h.failures ? `\n${h.failures} FAILURES` : "\nall green");
  if (h.failures) console.log("server log tail:\n" + h.serverLog.slice(-2000));
  await done(h.failures ? 1 : 0);
} catch (err) {
  console.error("audit crashed:", err);
  console.log("server log tail:\n" + h.serverLog.slice(-2000));
  await done(2);
}
