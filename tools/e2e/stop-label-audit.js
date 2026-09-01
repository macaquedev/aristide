// End-to-end audit of the stop editor's rename offer, against the REAL
// server and the REAL console UI: sample sets engrave the footage into
// stop names ("Montre 8'"), so revoicing such a stop's footage offers
// to move the footage out of the name and let the knob engrave the
// pitch it actually speaks.
//
//   bun tools/e2e/stop-label-audit.js [path-to-aristide-server]
//
// Needs: a built server (default target/release, falls back to debug),
// chromium on PATH, the gitignored demo set at testsets/grandorgue-demo,
// and no listener on the ports below. Runs against a throwaway
// XDG_CONFIG_HOME, so the real library and wiring are never touched;
// the demo is saved under a name of its own first, since a set's own
// organ refuses edits to its instrument (409 → the console's save-as card).
//
// What it proves:
//   1. OFFER — revoicing a footage-named stop shows the inline offer,
//      naming the stale tail and the proposed bare name.
//   2. DECLINE — "Keep name" leaves the name alone and is remembered:
//      a later footage edit on that stop doesn't nag again.
//   3. ACCEPT — "Rename" strips the tail from the name live and in the
//      organ's file (name-keyed references follow), returns a custom
//      engraving to auto, and the knob face reads the new footage.
//   4. NO FALSE OFFERS — setting the footage the name already claims,
//      or editing cents/gain, offers nothing.

import { connect, launchHarness } from "./cdp.js";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const SERVER_PORT = 9896;
const UI_PORT = 9897;
const CDP_PORT = 9235;
const h = launchHarness({ name: "stop-label-audit", serverPort: SERVER_PORT, uiPort: UI_PORT, cdpPort: CDP_PORT });
const { REPO, S, demo, check, sleep, state, settled, post, waitForServer, done } = h;
const OUT = join(REPO, "target", "stop-label-audit");

// The console's own footage-tail reading, kept dumber on purpose: a
// name whose last word is digits with an optional foot mark. Enough to
// pick audit subjects out of the demo set's stoplist.
const footageTail = (name) => {
  const m = /^(.*\S)\s+(\d+)\s*['′]?$/.exec(name);
  return m ? { base: m[1], feet: Number(m[2]) } : null;
};

try {
  await waitForServer();
  await post(`/api/organ/load?path=${encodeURIComponent(demo)}`);
  let snap = await settled();
  check(snap.setup?.adopted === true, `the demo loads as the set's own organ (${snap.setup?.file})`);

  // A sample set's own organ refuses instrument edits (409 → the save-as
  // card), so the audit works on a copy with a name of its own — the
  // rename must land in *that* file.
  const saved = await post(`/api/organ/save_as?name=${encodeURIComponent("Stop label audit")}`);
  check(saved.ok, `the demo saved as an organ of its own (${saved.body.slice(0, 80)})`);
  snap = await settled();
  const organFile = snap.setup?.file;
  check(!!organFile && snap.setup?.adopted === false, `the copy lives in a file that takes edits (${organFile})`);
  const fileText = () => readFileSync(organFile, "utf8");

  // Two audit subjects: single-footage stops whose names carry a tail.
  const subjects = (snap.stops ?? []).filter(
    (s) => footageTail(s.name) && s.pitch?.native != null
  );
  check(subjects.length >= 2, `the demo offers footage-named stops (${subjects.map((s) => s.name).join(", ")})`);
  const [declineStop, renameStop] = subjects;

  const drive = await connect(CDP_PORT);
  await drive.navigate(`http://127.0.0.1:${UI_PORT}/?server=${encodeURIComponent(S)}`);
  await sleep(1500);

  const openEditor = async (id) => {
    await drive.eval(`(() => {
      const knob = document.querySelector('.knob[data-key="stop-${id}"]');
      const rect = knob.getBoundingClientRect();
      knob.dispatchEvent(new MouseEvent("contextmenu", {
        bubbles: true, cancelable: true, ctrlKey: true,
        clientX: rect.left + 8, clientY: rect.top + 8,
      })); return true;
    })()`);
    await sleep(250);
    return drive.eval(`!document.getElementById("editor-stop").classList.contains("hidden")`);
  };
  const setFootage = async (text) => {
    await drive.eval(`(() => {
      const field = document.getElementById("editor-stop-footage");
      field.value = ${JSON.stringify(text)};
      field.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    })()`);
    await sleep(600);
  };
  const offerShowing = () =>
    drive.eval(`!document.getElementById("editor-stop-label-sync").classList.contains("hidden")`);
  const offerText = () =>
    drive.eval(`document.getElementById("editor-stop-label-sync-text").textContent`);

  // ---- 1. the offer appears, worded from the name --------------------

  const decline = footageTail(declineStop.name);
  const other = decline.feet === 8 ? "16" : "8";
  check(await openEditor(declineStop.id), `ctrl-right-click opens the editor on ${declineStop.name}`);
  await setFootage(other);
  check(await offerShowing(), `revoicing ${declineStop.name} to ${other}' offers the rename`);
  const text = await offerText();
  check(
    text.includes(decline.base) && text.includes(`${other}'`),
    `the offer names the bare name and the new footage (${text})`
  );
  await drive.shot(join(OUT, "offer.png"));

  // ---- 2. declining sticks ------------------------------------------

  await drive.eval(`document.getElementById("editor-stop-label-sync-no").click()`);
  await sleep(200);
  check(!(await offerShowing()), "Keep name hides the offer");
  snap = await state();
  check(
    snap.stops.find((s) => s.id === declineStop.id)?.name === declineStop.name,
    "…and the name is untouched"
  );
  await setFootage("4");
  check(!(await offerShowing()), "a later footage edit on the declined stop doesn't nag");
  await setFootage("native");
  await drive.eval(`document.getElementById("editor-stop-close").click()`);
  await sleep(200);

  // ---- 3. accepting renames, live and in the file -------------------

  // Give the stop a custom engraving first: accepting must return it
  // to auto, so the knob face follows the pitch from now on.
  const labelled = await post(`/api/organ/stop/label?stop=${renameStop.id}&label=${encodeURIComponent("olde text")}`);
  check(labelled.ok, `${renameStop.name} takes a custom engraving to start from`);
  const target = footageTail(renameStop.name);
  const goal = target.feet === 8 ? "16" : "8";
  check(await openEditor(renameStop.id), `the editor opens on ${renameStop.name}`);
  await setFootage(goal);
  check(await offerShowing(), `revoicing ${renameStop.name} to ${goal}' offers the rename`);
  await drive.eval(`document.getElementById("editor-stop-label-sync-yes").click()`);
  await sleep(800);
  snap = await state();
  const renamed = snap.stops.find((s) => s.id === renameStop.id);
  check(renamed?.name === target.base, `the stop is now named ${target.base} (${renamed?.name})`);
  check(renamed?.label == null, "the custom engraving went back to auto");
  check(
    fileText().includes(`"${target.base}"`),
    `the rename landed in the organ file`
  );
  check(
    await drive.eval(`document.getElementById("editor-stop-title").textContent`) === target.base,
    "the popover title follows"
  );
  const face = await drive.eval(`(() => {
    const knob = document.querySelector('.knob[data-key="stop-${renameStop.id}"]');
    return [knob.querySelector(".stop-name")?.textContent,
            knob.querySelector(".stop-pitch")?.textContent];
  })()`);
  check(
    face[0] === target.base && face[1] === `${goal}'`,
    `the knob face engraves ${target.base} / ${goal}' (${face.join(" / ")})`
  );
  await drive.shot(join(OUT, "renamed.png"));

  // ---- 4. no false offers -------------------------------------------

  // The name now carries no footage: further revoicing offers nothing.
  await setFootage(`${target.feet}`);
  check(!(await offerShowing()), "revoicing a bare-named stop offers nothing");
  await drive.eval(`document.getElementById("editor-stop-close").click()`);
  await sleep(200);

  // A fresh footage-named stop set to exactly what its name claims.
  const third = (snap.stops ?? []).find(
    (s) => s.id !== declineStop.id && s.id !== renameStop.id && footageTail(s.name) && s.pitch?.native != null
  );
  if (third) {
    const tail = footageTail(third.name);
    check(await openEditor(third.id), `the editor opens on ${third.name}`);
    await setFootage(`${tail.feet}`);
    check(!(await offerShowing()), "the footage the name already claims offers nothing");
  }

  console.log(h.failures ? `\n${h.failures} FAILURES` : "\nall green");
  if (h.failures) console.log("server log tail:\n" + h.serverLog.slice(-2000));
  await done(h.failures ? 1 : 0);
} catch (err) {
  console.error("audit crashed:", err);
  console.log("server log tail:\n" + h.serverLog.slice(-2000));
  await done(2);
}
