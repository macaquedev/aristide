// Real pointer/keyboard input verifies that leaving a shared editor commits a
// typed field before clearing its subject. The workspace also keeps a gesture
// from activating the playing surface behind it.
import { connect, launchHarness } from "./cdp.js";

const h = launchHarness({ name: "popover-dismiss", serverPort: 9902, uiPort: 9903, cdpPort: 9238 });
const { check, sleep, state, post, settled } = h;
try {
  await h.waitForServer();
  await post(`/api/organ/load?path=${encodeURIComponent(h.demo)}`); await settled();
  await post('/api/organ/save_as?name=Editor%20navigation');
  const s = await settled();
  const stop = s.stops.find(stop => /Trompette/.test(stop.name));
  const d = await connect(9238);
  await d.send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
  await d.navigate('http://127.0.0.1:9903/?server=http://127.0.0.1:9902');
  const visible = sel => d.eval(`!!document.querySelector(${JSON.stringify(sel)})?.getClientRects().length`);
  const center = sel => d.eval(`(()=>{const e=document.querySelector(${JSON.stringify(sel)});e.scrollIntoView({block:'center'});const r=e.getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2]})()`);
  const mouse = (type, x, y, button = 'left') => d.send('Input.dispatchMouseEvent', { type, x, y, button, clickCount: 1 });
  const click = async (sel, button = 'left') => {
    const [x,y] = await center(sel);
    await mouse('mouseMoved',x,y); await mouse('mousePressed',x,y,button); await mouse('mouseReleased',x,y,button); await sleep(180);
  };
  const knob = `.knob[data-key="stop-${stop.id}"]`;
  const open = async () => {
    if (await visible('#organ-prefs')) await click('#organ-prefs .modal-close');
    await click(knob,'right');
    check(await visible('#editor-stop'), 'context shortcut opens the shared stop editor');
  };
  const typeBrightness = async value => {
    await click('#editor-stop-brightness');
    await d.send('Input.dispatchKeyEvent', { type:'keyDown', key:'a', code:'KeyA', modifiers:2, windowsVirtualKeyCode:65 });
    await d.send('Input.dispatchKeyEvent', { type:'keyUp', key:'a', code:'KeyA', modifiers:2 });
    await d.send('Input.insertText', { text: String(value) });
  };
  const brightness = async () => (await state()).stops.find(s=>s.id===stop.id).pitch.brightness;
  await click('#editor-lock');
  let value = 1;
  for (const [label, target] of [
    ['Back', '#organ-prefs-back'],
    ['another section', '[data-category="keyboards"]'],
    ['Close', '#organ-prefs .modal-close'],
  ]) {
    await open(); await typeBrightness(value); await click(target); await sleep(500);
    check(await brightness() === value, `typed brightness commits through ${label} without Enter`);
    check(!(await visible('#editor-stop')), `${label} leaves the stop editor`);
    value++;
  }
  await open();
  const [x,y] = await center('#editor-stop-name');
  await mouse('mouseMoved',x,y); await mouse('mousePressed',x,y);
  await mouse('mouseMoved',10,100); await mouse('mouseReleased',10,100); await sleep(150);
  check(await visible('#editor-stop'), 'a drag beginning inside the editor does not dismiss it');

  const other = s.stops[0];
  const otherKnob = `.knob[data-key="stop-${other.id}"]`;
  const before = (await state()).stops.find(s=>s.id===other.id).on;
  await typeBrightness(value);
  // This knob is under the backdrop, left of the workspace. A press closes the
  // workspace; a second, deliberate press can play it.
  const r = await d.eval(`(()=>{const r=document.querySelector(${JSON.stringify(otherKnob)}).getBoundingClientRect();return {x:r.left+20,y:r.top+20}})()`);
  await mouse('mousePressed',r.x,r.y); await mouse('mouseReleased',r.x,r.y); await sleep(500);
  check(await brightness() === value, 'backdrop dismissal commits the typed field');
  check(!(await visible('#organ-prefs')), 'backdrop dismisses the workspace');
  check((await state()).stops.find(s=>s.id===other.id).on === before, 'dismissal does not pull a stop behind the dialog');
  await click(otherKnob); await sleep(200);
  check((await state()).stops.find(s=>s.id===other.id).on !== before, 'the next deliberate press plays the stop');
} catch (error) { check(false,'audit completes'); console.error(error); }
console.log(`${h.failures} failed`);
await h.done(h.failures ? 1 : 0);
