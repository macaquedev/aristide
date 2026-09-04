// Real touch input, responsive geometry and focus behavior against an isolated organ.
// bun tools/e2e/touch-ui-audit.js [server binary]
import { connect, launchHarness } from './cdp.js';
const h = launchHarness({ name:'touch-ui-audit', serverPort:9920, uiPort:9921, cdpPort:9250 });
const {check, sleep, post, settled, state} = h;
let fatal = false;
try {
  await h.waitForServer();
  await post(`/api/organ/load?path=${encodeURIComponent(h.demo)}`);
  await settled();
  await post('/api/organ/save_as?name=Touch%20UI%20audit');
  const snapshot = await settled();
  const d = await connect(9250);
  await d.navigate('http://127.0.0.1:9921/?server=http://127.0.0.1:9920');
  await sleep(700);
  await d.send('Emulation.setTouchEmulationEnabled', {enabled:true,maxTouchPoints:5});
  const center = async (sel) => d.eval(`(() => { const el = document.querySelector(${JSON.stringify(sel)}); el.scrollIntoView({block:'center'}); const r = el.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; })()`);
  const touch = (type, touchPoints) => d.send('Input.dispatchTouchEvent',{type,touchPoints});
  const tap = async (sel) => {const p=await center(sel); await touch('touchStart',[{...p,id:1}]); await touch('touchEnd',[]); await sleep(180);};
  const visible = (sel) => d.eval(`!!document.querySelector(${JSON.stringify(sel)})?.getClientRects().length`);

  for (const [width,height] of [[320,740],[390,844],[768,1024],[1024,768]]) {
    await d.send('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:true});
    await sleep(250);
    check(await d.eval(`document.documentElement.scrollWidth <= innerWidth`), `${width}px: no page-wide horizontal overflow`);
    check(await d.eval(`(() => {const panels=[...document.querySelectorAll('.panel')].filter(p=>p.getClientRects().length).map(p=>p.getBoundingClientRect()); return panels.every((a,i)=>panels.slice(i+1).every(b=>a.right<=b.left+1 || b.right<=a.left+1 || a.bottom<=b.top+1 || b.bottom<=a.top+1));})()`), `${width}px: panels do not overlap`);
  }
  await d.shot('/tmp/aristide-tablet.png');
  const manual = snapshot.manuals.find(m=>!m.pedal && m.key_count>20) ?? snapshot.manuals[0];
  const key1 = `.keyboard[data-manual="${manual.idx}"] .key.natural[data-midi="${manual.first_key}"]`;
  const key2 = `.keyboard[data-manual="${manual.idx}"] .key.natural[data-midi="${manual.first_key+4}"]`;
  await center(key1);
  // Measure both after scrolling once, so their coordinates share a viewport.
  const points = await d.eval(`[${JSON.stringify(key1)},${JSON.stringify(key2)}].map((s,i)=>{const r=document.querySelector(s).getBoundingClientRect();return {id:i+1,x:r.x+r.width/2,y:r.bottom-10};})`);
  await touch('touchStart',points); await sleep(350);
  let held=(await state()).manuals.find(m=>m.idx===manual.idx).held;
  check(held.includes(manual.first_key) && held.includes(manual.first_key+4), 'two fingers play a chord');
  await touch('touchEnd',[points[0]]); await sleep(350);
  held=(await state()).manuals.find(m=>m.idx===manual.idx).held;
  check(!held.includes(manual.first_key) && held.includes(manual.first_key+4), 'lifting one finger leaves the other note sounding');
  await touch('touchCancel',[]); await sleep(350);
  check(!(await state()).manuals.find(m=>m.idx===manual.idx).held.length, 'cancelled touch releases the remaining note');

  await tap('#editor-lock');
  check(await d.eval(`document.body.classList.contains('editing')`), 'touch opens Edit console');
  await tap('#editor-add-button');
  check(await visible('#editor-add-menu'), '+ Add opens without double-click');
  await tap('#editor-add-manual');
  check(await visible('#editor-add-manual-form'), 'keyboard creation is reachable by touch');
  await tap('#editor-add-manual-cancel');
  const stop=snapshot.stops[0];
  const knob=`.knob[data-key="stop-${stop.id}"]`;
  const before=(await state()).stops.find(s=>s.id===stop.id).on;
  await tap('#editor-inspect');
  await tap(knob);
  check(await visible('#editor-stop'), 'Control settings opens a stop editor without right-click');
  check((await state()).stops.find(s=>s.id===stop.id).on===before, 'opening settings does not toggle the stop');
  await tap('#editor-inspect');
  const stopManual = snapshot.manuals.find(m=>m.idx===stop.midx);
  await tap(`.keyboard[data-manual="${stop.midx}"] .key[data-midi="${stopManual.first_key}"]`);
  check(await visible('#editor-key-voicing'), 'pipe voicing is reachable by touch from a stop editor');
  check(!(await state()).manuals.find(m=>m.idx===stop.midx).held.length, 'choosing a pipe to edit does not play its note');
  await d.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape'});
  const ranks = JSON.stringify((await state()).manuals.map(m=>m.rank));
  const dragPoint = await center(knob);
  await touch('touchStart',[{...dragPoint,id:1}]);
  await touch('touchMove',[{x:dragPoint.x+100,y:dragPoint.y+55,id:1}]);
  check(await visible('.organ-drag-ghost'), 'touch can start a stop drag');
  await touch('touchCancel',[]); await sleep(150);
  check(!(await visible('.organ-drag-ghost')), 'cancelling a drag removes its floating label');
  check(JSON.stringify((await state()).manuals.map(m=>m.rank))===ranks, 'cancelled drag leaves stop order unchanged');
  await tap('#editor-lock');
  await tap(knob);
  check((await state()).stops.find(s=>s.id===stop.id).on!==before, 'normal touch still toggles a stop');

  await d.send('Emulation.setDeviceMetricsOverride',{width:390,height:844,deviceScaleFactor:1,mobile:true});
  await d.eval('window.scrollTo(0,0)'); await sleep(200);
  await d.shot('/tmp/aristide-phone.png');
  await tap('#app-menu');
  await tap('#organ-name');
  check(await visible('#organ-menu-list'), 'touch switches directly between open menus');
  await tap('#app-menu');
  await d.eval(`[...document.querySelectorAll('#app-menu-list button')].find(b=>b.textContent.includes('Preferences')).click()`);
  await sleep(200);
  check(await d.eval(`document.querySelector('#prefs').contains(document.activeElement) && document.querySelector('#console').inert`), 'dialog focuses its controls and makes the console inactive');
  await d.eval(`const b=[...document.querySelectorAll('#prefs button')].filter(b=>!b.disabled && b.getClientRects().length); b.at(-1).focus()`);
  await d.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Tab',code:'Tab'});
  check(await d.eval(`document.querySelector('#prefs').contains(document.activeElement)`), 'Tab stays inside Preferences');
  await d.shot('/tmp/aristide-prefs.png');
  await tap('#prefs .advanced-settings summary');
  await sleep(150);
  check(await d.eval(`(() => {const r=document.querySelector('#prefs .modal-card').getBoundingClientRect();return r.top>=0 && r.bottom<=innerHeight;})()`), 'expanded advanced settings stay inside the viewport');
  await tap('#prefs [data-close].modal-close');
  check(await d.eval(`!document.querySelector('#console').inert && !document.body.classList.contains('modal-open')`), 'closing the dialog restores the console');

  await d.send('Emulation.setTouchEmulationEnabled',{enabled:false});
  await d.send('Emulation.setDeviceMetricsOverride',{width:1500,height:950,deviceScaleFactor:1,mobile:false});
  await d.eval('window.scrollTo(0,0)'); await sleep(250);
  await d.shot('/tmp/aristide-desktop.png');
  check(await d.eval(`getComputedStyle(document.querySelector('.panel')).position === 'absolute'`), 'desktop returns to the saved canvas layout');
  await d.eval(`document.querySelector('#app-menu').focus()`);
  await d.send('Input.dispatchKeyEvent',{type:'keyDown',key:'ArrowDown',code:'ArrowDown'});
  check(await d.eval(`document.querySelector('#app-menu-list').contains(document.activeElement)`), 'arrow keys open menus and focus an item');
  await d.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape'});
  check(await d.eval(`document.activeElement.id==='app-menu' && document.querySelector('#app-menu').getAttribute('aria-expanded')==='false'`), 'Escape restores menu trigger focus');
} catch (e) {fatal=true; console.error(e);}
console.log(`${h.failures} failed${fatal ? '; fatal error' : ''}`);
await h.done(h.failures || fatal ? 1 : 0);
