// Reproduce a saved GREAT panel obscuring PEDAL in the four-manual Solignac set.
// bun tools/e2e/large-screen-layout-audit.js [server binary]
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { connect, launchHarness } from './cdp.js';
const h = launchHarness({ name: 'large-screen-layout', serverPort: 9960, uiPort: 9961, cdpPort: 9292, needsDemo: false });
const url = 'http://127.0.0.1:9961/?server=http://127.0.0.1:9960';
const settle = async () => {
  for (let i = 0; i < 600; i++) {
    const s = await h.state();
    if (s.organ && !s.loading) return s;
    await h.sleep(500);
  }
  throw new Error('Solignac did not finish loading');
};
try {
  const source = join(h.REPO, 'testsets/avo-solignac/OrganDefinitions/Solignac extend.Organ_Hauptwerk_xml');
  if (!existsSync(source)) throw new Error('Solignac fixture required; see CLAUDE.md');
  await h.waitForServer();
  await h.post(`/api/organ/load?path=${encodeURIComponent(source)}`);
  const initial = await settle();
  h.check(initial.manuals.length === 4, 'exercise all four Solignac keyboards');
  // Layout is a player preference, so even the adopted instrument accepts it.
  h.check((await h.post('/api/organ/panel/place?panel=jamb%3AGREAT&x=0&y=0&w=0.305&h=0.2')).ok, 'load the overlapping saved position from the reported case');
  const saved = JSON.stringify((await h.state()).layout);
  const d = await connect(9292);
  await d.send('Page.addScriptToEvaluateOnNewDocument', { source: `window.auditErrors=[];addEventListener('error',e=>auditErrors.push(e.message));addEventListener('unhandledrejection',e=>auditErrors.push(String(e.reason)));` });
  await d.navigate(url);
  const geometry = () => d.eval(`(() => {
    const panels=[...document.querySelectorAll('.panel')].filter(e=>e.offsetHeight).map(e=>({id:e.dataset.panel,...Object.fromEntries(['x','y','width','height','right','bottom'].map(k=>[k,e.getBoundingClientRect()[k]]))}));
    const overlaps=panels.flatMap((a,i)=>panels.slice(i+1).filter(b=>a.x<b.right-1 && b.x<a.right-1 && a.y<b.bottom-1 && b.y<a.bottom-1).map(b=>a.id+' / '+b.id));
    const stops=[...document.querySelectorAll('.panel-jamb .knob')];
    return { panels, overlaps, fits:document.documentElement.scrollWidth<=innerWidth,
      readable:stops.every(e=>e.clientWidth>=100 && e.offsetHeight>=(matchMedia('(pointer: coarse)').matches?44:28) && e.querySelector('.stop-name').scrollWidth<=e.querySelector('.stop-name').clientWidth+1),
      mode:document.querySelector('#console-canvas').dataset.layout, errors:window.auditErrors };
  })()`);
  for (const [width,height,scale,touch] of [[1440,900,1,false],[1919,976,1,false],[1919,1000,2,false],[2560,1440,1,false],[3838,1999,1,false],[1366,768,1,false],[1024,768,1,true],[390,844,1,true]]) {
    await d.send('Emulation.setTouchEmulationEnabled',{enabled:touch});
    await d.send('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:scale,mobile:touch});
    for (const density of ['compact','regular','spacious']) {
      await d.eval(`document.body.dataset.density=${JSON.stringify(density)}`);
      await d.sleep(180);
      const g=await geometry();
      h.check(g.fits && g.readable && !g.overlaps.length && !g.errors.length, `${width}×${height} @${scale} ${density}: no overlaps, clipped labels or errors ${JSON.stringify(g.overlaps)}`);
      if (width>=1800 && height>=900) {
        const jambs=g.panels.filter(p=>p.id.startsWith('jamb:'));
        const boards=g.panels.filter(p=>p.id.startsWith('keyboard:'));
        h.check(new Set(jambs.map(p=>p.y)).size===1 && new Set(boards.map(p=>p.y)).size===1 && g.panels.every(p=>p.bottom<=height), `${width}px ${density}: four aligned divisions and all playing controls fit on screen`);
      }
    }
  }
  h.check(JSON.stringify((await h.state()).layout)===saved,'responsive repair never overwrites saved coordinates');
  await d.send('Emulation.setTouchEmulationEnabled',{enabled:false});
  await d.send('Emulation.setDeviceMetricsOverride',{width:1919,height:976,deviceScaleFactor:1,mobile:false});
  await d.eval(`document.body.dataset.density='regular'`);
  await d.sleep(200);
  h.check((await geometry()).mode==='automatic','overlapping custom positions automatically fall back to the grid');
  await d.click('#editor-lock');
  await d.click('#editor-auto-arrange');
  await d.sleep(500);
  h.check(Object.keys((await h.state()).layout).length===0,'Auto arrange clears saved panel positions');
  await d.click('#editor-lock');
  await d.navigate(url);
  h.check((await geometry()).mode==='automatic','automatic arrangement survives reopening');

  // An ordinary, non-overlapping custom adjustment must still persist exactly.
  const pos=await d.eval(`(()=>{const e=document.querySelector('[data-panel="jamb:PEDAL"]'),c=document.querySelector('#console-canvas');return {x:(e.offsetLeft+8)/c.clientWidth,y:(e.offsetTop+8)/c.clientHeight};})()`);
  await h.post(`/api/organ/panel/place?panel=jamb%3APEDAL&x=${pos.x}&y=${pos.y}`);
  await d.sleep(300);
  const before=(await geometry()).panels.find(p=>p.id==='jamb:PEDAL');
  h.check((await geometry()).mode==='custom','a valid custom arrangement remains available');
  await d.navigate(url);
  const after=(await geometry()).panels.find(p=>p.id==='jamb:PEDAL');
  h.check(Math.abs(before.x-after.x)<=1 && Math.abs(before.y-after.y)<=1,'valid saved positions survive reopening exactly');

  // Expanding a keyboard changes panel height and must reflow the lower controls.
  await d.click('.panel-keyboard .keyboard-toggle');
  await d.sleep(300);
  h.check(!(await geometry()).overlaps.length,'expanded keyboard does not overlap playing controls');
  await d.click('.panel-keyboard .keyboard-toggle');
  await h.post('/api/organ/panel/place?reset=1');
  const file=(await h.state()).setup.file;
  await h.post(`/api/organ/load?path=${encodeURIComponent(file)}`);
  h.check(Object.keys((await settle()).layout).length===0,'automatic arrangement survives reloading the organ file');
  await d.navigate(url);
  await d.shot('/tmp/aristide-solignac-desktop-final.png');
  await d.send('Emulation.setDeviceMetricsOverride',{width:1919,height:1000,deviceScaleFactor:2,mobile:false});
  await d.sleep(250); await d.shot('/tmp/aristide-solignac-hidpi-final.png');
  await d.send('Emulation.setTouchEmulationEnabled',{enabled:true});
  await d.send('Emulation.setDeviceMetricsOverride',{width:390,height:844,deviceScaleFactor:1,mobile:true});
  await d.sleep(250); await d.shot('/tmp/aristide-solignac-phone-final.png');
} catch(e) { h.check(false,e.stack || String(e)); }
finally { await h.done(h.failures ? 1 : 0); }
