// Every shared editor must fit small screens, including short landscape views.
import { connect, launchHarness } from './cdp.js';
const h = launchHarness({name:'settings-layout',serverPort:9950,uiPort:9951,cdpPort:9280});
try {
  await h.waitForServer();
  await h.post(`/api/organ/load?path=${encodeURIComponent(h.demo)}`); await h.settled();
  await h.post('/api/organ/save_as?name=Responsive%20instrument'); const snapshot = await h.settled();
  const d = await connect(9280);
  await d.navigate('http://127.0.0.1:9951/?server=http://127.0.0.1:9950');
  const action = async label => {
    await d.eval(`[...document.querySelectorAll('#organ-prefs-index .organ-pref-action')].find(b=>b.textContent.startsWith(${JSON.stringify(label)})).click()`);
    await h.sleep(150);
  };
  const sections = [
    ['tuning','general',['Tuning & pitch']],
    ['room','general',['Room & noises']],
    ['tremulant','general',['Tremulant']],
    ['MIDI','keyboards',[snapshot.manuals[0].name,'Connect keyboard']],
    ['key range','keyboards',[snapshot.manuals[0].name,'Key range']],
    ['keyboard','keyboards',[snapshot.manuals[0].name]],
    ['stop','stops',[snapshot.stops[0].name]],
    ['coupler','couplers',[snapshot.couplers[0].name]],
    ['bindings','bindings',['Edit buttons & shortcuts']],
    ['sample set tuning','sources',['Sample set tuning']],
  ];
  for (const [name,category,actions] of sections) {
    await d.click('#instrument-settings');
    await d.set('#organ-prefs-section',category); await h.sleep(300);
    for (const label of actions) await action(label);
    for (const [width,height] of [[320,568],[390,844],[768,1024],[1024,640],[1500,900],[844,390]]) {
      await d.send('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:width<1000});
      await d.send('Emulation.setTouchEmulationEnabled',{enabled:width<1000,maxTouchPoints:5});
      await h.sleep(100);
      const result = await d.eval(`(()=>{
        const host=document.querySelector('.organ-prefs-content'), modal=document.querySelector('#organ-prefs .modal-card');
        const box=modal.getBoundingClientRect();
        const active=[...document.querySelectorAll('#organ-prefs .editor-add')].filter(e=>e.getClientRects().length);
        const fields=[...host.querySelectorAll('input,select,button')].filter(e=>e.getClientRects().length && !e.closest('.hidden, .settings-suspended') && getComputedStyle(e).visibility!=='hidden');
        const bounds=host.getBoundingClientRect();
        return {
          viewport:box.left>=0 && box.right<=innerWidth+1 && box.top>=0 && box.bottom<=innerHeight+1,
          scroll:host.scrollWidth<=host.clientWidth+1,
          fields:fields.every(e=>{const r=e.getBoundingClientRect();return r.left>=bounds.left-1 && r.right<=bounds.right+1}),
          single:active.length<=1,
        };
      })()`);
      h.check(Object.values(result).every(Boolean),`${name} at ${width}×${height}: ${JSON.stringify(result)}`);
    }
    await d.click('#organ-prefs .modal-close');
  }
} catch(error) {h.check(false,'audit completes');console.error(error);}
console.log(`${h.failures} failed`);
await h.done(h.failures?1:0);
