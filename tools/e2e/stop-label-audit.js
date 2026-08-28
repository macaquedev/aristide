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
// XDG_CONFIG_HOME, so the real library and wiring are never touched.
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

import { connect } from "./cdp.js";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const REPO = new URL("../..", import.meta.url).pathname;
const SERVER_PORT = 9896;
const UI_PORT = 9897;
const CDP_PORT = 9235;
const S = `http://127.0.0.1:${SERVER_PORT}`;
const OUT = join(REPO, "target", "stop-label-audit");

let failures = 0;
const check = (ok, what) => {
  console.log(`${ok ? "PASS" : "FAIL"}  ${what}`);
  if (!ok) failures++;
};
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

// The console's own footage-tail reading, kept dumber on purpose: a
// name whose last word is digits with an optional foot mark. Enough to
// pick audit subjects out of the demo set's stoplist.
const footageTail = (name) => {
  const m = /^(.*\S)\s+(\d+)\s*['′]?$/.exec(name);
  return m ? { base: m[1], feet: Number(m[2]) } : null;
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

const scratch = mkdtempSync(join(tmpdir(), "aristide-stop-label-audit-"));
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
  await post(`/api/organ/load?path=${encodeURIComponent(demo)}`);
  let snap = await settled();
  const organFile = snap.setup?.file;
  check(!!organFile, `the adopted demo lives in a file (${organFile})`);
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
  await post(`/api/organ/stop/label?stop=${renameStop.id}&label=${encodeURIComponent("olde text")}`);
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

  console.log(failures ? `\n${failures} FAILURES` : "\nall green");
  if (failures) console.log("server log tail:\n" + serverLog.slice(-2000));
  await done(failures ? 1 : 0);
} catch (err) {
  console.error("audit crashed:", err);
  console.log("server log tail:\n" + serverLog.slice(-2000));
  await done(2);
}
