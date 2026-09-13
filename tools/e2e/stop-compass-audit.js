// bun tools/e2e/stop-compass-audit.js target/debug/aristide-server
// Uses an isolated copy of the demo organ; verifies editing and reload.
import { connect, launchHarness } from "./cdp.js";

const h = launchHarness({ name: "stop-compass-audit", serverPort: 9908, uiPort: 9909, cdpPort: 9240 });
const { S, demo, check, sleep, state, settled, post, waitForServer, done } = h;
try {
  await waitForServer();
  await post(`/api/organ/load?path=${encodeURIComponent(demo)}`);
  await settled();
  const adopted = await state();
  check((await post(`/api/organ/stop/compass?stop=${adopted.stops[0].id}&keys=C4..C5`)).status === 409, "source instrument protects its compass");
  check((await post("/api/organ/save_as?name=Stop%20compass%20audit")).ok, "save an editable copy");
  let snap = await settled();
  const file = snap.setup.file;
  const stop = snap.stops.find(s => s.native_compass?.[0] < 60 && s.native_compass?.[1] > 72);
  if (!stop) throw new Error("demo has no suitable stop");
  const drive = await connect(9240);
  await drive.navigate(`http://127.0.0.1:9909/?server=${encodeURIComponent(S)}`);
  await sleep(1500);
  await drive.eval(`(() => {
    const knob = document.querySelector('.knob[data-key="stop-${stop.id}"]');
    const rect = knob.getBoundingClientRect();
    knob.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true,
      ctrlKey: true, clientX: rect.left + 8, clientY: rect.top + 8 }));
    return true;
  })()`);
  await sleep(250);
  check(await drive.eval(`!document.getElementById("editor-stop").classList.contains("hidden")`), "open selected stop editor");
  const enter = async text => {
    await drive.eval(`(() => {
      const field = document.getElementById("editor-stop-compass");
      field.value = ${JSON.stringify(text)};
      field.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    })()`);
    await sleep(500);
  };
  await enter("C4..C5");
  check(JSON.stringify((await state()).stops.find(s => s.id === stop.id).compass) === "[60,72]", "editor saves inclusive note bounds");
  for (const keys of ["invalid", "-1..60", "0..128"]) {
    check((await post(`/api/organ/stop/compass?stop=${stop.id}&keys=${encodeURIComponent(keys)}`)).status === 400, `reject invalid bounds: ${keys}`);
  }
  await enter("invalid");
  check(await drive.eval(`!document.getElementById("editor-stop-error").classList.contains("hidden")`), "invalid compass shows a local error");
  check(JSON.stringify((await state()).stops.find(s => s.id === stop.id).compass) === "[60,72]", "invalid edit preserves the saved range");
  await drive.eval(`document.getElementById("editor-stop-compass-reset").click()`);
  await sleep(500);
  check((await state()).stops.find(s => s.id === stop.id).compass == null, "reset restores source compass");
  await enter("C4..C5");
  await post(`/api/organ/load?path=${encodeURIComponent(file)}`);
  snap = await settled();
  check(JSON.stringify(snap.stops.find(s => s.name === stop.name).compass) === "[60,72]", "compass survives reload");
} catch (err) {
  check(false, err.stack || String(err));
} finally {
  await done(h.failures ? 1 : 0);
}
