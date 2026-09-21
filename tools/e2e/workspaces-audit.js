// Real-server interactions, then explicitly synthetic layout extremes using
// real stop names. No fixture behavior is shipped in the application.
// bun tools/e2e/workspaces-audit.js [server binary]
import { connect, launchHarness } from './cdp.js';
const h = launchHarness({ name: 'workspaces', serverPort: 19950, uiPort: 19951, cdpPort: 19280 });
try {
  await h.waitForServer();
  const d = await connect(19280);
  await d.navigate(h.S);
  for(let i=0;i<100 && !await d.eval(`document.readyState === 'complete' && document.body.dataset.ready === 'true'`);i++) await d.sleep(50);
  h.check(await d.eval('document.body.dataset.workspace') === 'library', 'an empty server opens Library');
  await d.click('[data-workspace="setup"]'); await d.sleep(300);
  h.check(await d.eval('document.body.dataset.workspace') === 'setup', 'Setup remains available before any organ is loaded');
  if(process.env.ARISTIDE_STARTUP_ONLY) await h.done(h.failures ? 1 : 0);
  await d.click('[data-workspace="library"]');
  await h.post(`/api/organ/load?path=${encodeURIComponent(h.demo)}`); await h.settled();
  await h.post('/api/organ/save_as?name=Workspace%20audit'); await h.settled();
  await d.send('Page.addScriptToEvaluateOnNewDocument', { source: `window.__errors=[];addEventListener('error',e=>__errors.push(e.message));addEventListener('unhandledrejection',e=>__errors.push(String(e.reason)));` });
  await d.send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
  await d.navigate('http://127.0.0.1:19951/?server=http://127.0.0.1:19950');
  const capture = async path => { await d.eval('document.fonts.ready.then(() => true)'); await d.sleep(250); await d.shot(path); };
  const mode = () => d.eval('document.body.dataset.workspace');
  const visible = selector => d.eval(`!!document.querySelector(${JSON.stringify(selector)})?.getClientRects().length`);
  const action = async label => { await d.eval(`[...document.querySelectorAll('#organ-prefs button')].find(b=>b.textContent.startsWith(${JSON.stringify(label)})).click()`); await d.sleep(150); };
  h.check(await mode() === 'play', 'fully loaded organ enters Play');
  h.check(await d.eval(`(()=>{const b=document.querySelector('#panic'),r=b.getBoundingClientRect();return r.height>=56&&document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)===b})()`), 'Silence remains a visible, reachable touch target');
  h.check(!(await visible('#workspace-nav')), 'configuration navigation is absent in Play');
  const first = (await h.state()).stops[0];
  await d.click(`[data-key="stop-${first.id}"]`); await d.sleep(300);
  h.check((await h.state()).stops[0].on !== first.on, 'stop toggle reaches the real engine');
  await d.eval(`document.querySelector('.knob').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true,ctrlKey:true}))`);
  h.check(await mode() === 'play' && !(await visible('#organ-prefs')), 'Ctrl context-click cannot edit through the Play lock');
  for (const [width,height] of [[900,600],[390,844],[320,740]]) {
    await d.send('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:width<700}); await d.sleep(150);
    h.check(await d.eval(`(()=>{const rs=[...document.querySelectorAll('.panel')].filter(e=>e.getClientRects().length).map(e=>e.getBoundingClientRect());return rs.every((a,i)=>a.bottom<=innerHeight+1&&rs.slice(i+1).every(b=>a.right<=b.left+1||b.right<=a.left+1||a.bottom<=b.top+1||b.bottom<=a.top+1));})()`), `real organ ${width}×${height}: divisions, couplers and expression do not overlap`);
  }
  await d.send('Emulation.setDeviceMetricsOverride',{width:1500,height:950,deviceScaleFactor:1,mobile:false}); await d.sleep(150);
  await capture('/tmp/aristide-new-play.png');
  await d.click('#editor-lock'); await d.sleep(150);
  h.check(await mode() === 'build' && await visible('#organ-prefs'), 'unlock enters the Build workspace');
  await capture('/tmp/aristide-new-build.png');
  await action('Pedal'); await action('Connect keyboard / MIDI input');
  h.check(await visible('#editor-midi'), 'keyboard MIDI editor remains reachable');
  await d.click('[data-workspace="voice"]'); await action('Tuning & pitch');
  h.check(await visible('#editor-tuning'), 'Voice hosts the existing live tuning form');
  await d.set('#editor-tuning-ref-hz', '415'); await d.sleep(400);
  h.check(Math.abs((await h.state()).tuning.reference.hz - 415) < .01, 'live tuning persists through the real API');
  await capture('/tmp/aristide-new-voice.png');
  await d.click('[data-workspace="setup"]');
  await capture('/tmp/aristide-new-setup.png');
  h.check(await visible('#scale-row') && !(await visible('#editor-tuning')), 'Setup replaces Voice with machine preferences');
  await d.click('#command-open'); await d.set('#command-search', 'room');
  await d.eval(`document.querySelector('#command-search').dispatchEvent(new Event('input'))`);
  await d.click('#command-results button'); await d.sleep(200);
  h.check(await visible('#editor-room') && await mode() === 'voice', 'command palette navigates to the live room editor');
  await d.click('[data-workspace="build"]'); await d.click('#build-layout'); await d.sleep(200);
  h.check(await mode() === 'layout' && await visible('.keyboard-toggle'), 'Build retains console arrangement and keyboard audition');
  await d.click('.keyboard-toggle'); h.check(await visible('.keyboard:not(.keys-collapsed)'), 'desk keyboard audition can be revealed');
  await d.click('#editor-lock'); await d.sleep(200);
  h.check(await mode() === 'play', 'one lock returns to Play');
  await d.click('#editor-lock'); await d.click('[data-workspace="library"]');
  await capture('/tmp/aristide-new-library.png');
  h.check(await visible('#picker-new-set') && await visible('.picker-open'), 'Library offers loading and recent instruments');
  await d.click('#editor-lock');
  h.check(await mode() === 'play', 'Library returns to the running instrument');
  h.check((await d.eval('__errors')).length === 0, 'real-server navigation produces no browser errors');

  // The binary serves the very same assets as the desktop frontend.
  await d.navigate(h.S);
  h.check(await mode() === 'play' && await visible('.knob'), 'server URL serves the complete shared console without a separate static server');
  await d.click('#editor-lock'); await d.click('[data-workspace="library"]');
  await d.eval(`window.__fetch=window.fetch;window.fetch=async(...args)=>{if(String(args[0]).includes('/api/organ/load'))await new Promise(r=>setTimeout(r,600));return __fetch(...args)}`);
  await d.click('.picker-open'); await d.sleep(100);
  h.check(await mode() === 'library' && !(await visible('#console')), 'a pending organ load never reveals a playable partial instrument');
  await h.settled();
  for (let i=0;i<200 && await mode() !== 'play';i++) await d.sleep(100);
  h.check(await mode() === 'play', 'completed library load returns to Play');
  if (process.env.ARISTIDE_INTERACTIONS_ONLY) await h.done(h.failures ? 1 : 0);
  const base = await h.settled();
  const source = `
    const originalFetch=window.fetch.bind(window);
    window.fetch=async (...args)=> {
      const url=String(args[0]);
      if (!url.includes('/api/state')) return originalFetch(...args);
      const s=${JSON.stringify(base)};
      const count=Number(new URL(location.href).searchParams.get('fixture'));
      if(!count)return originalFetch(...args);
      const divisions=count===5?['Positif']:['Grand-Orgue','Positif','Récit','Hauptwerk','Pedal'];
      const names=['Montre','Bourdon','Prestant','Doublette','Fourniture','Flûte harmonique','Trompette','Clairon','Salicional','Voix céleste','Nasard','Tierce','Larigot','Cornet','Cromorne','Principal','Gedackt','Quintade','Rohrflöte','Spitzflöte','Octave','Mixtur','Sesquialtera','Trompete','Open diapason','Stopped diapason','Dulciana','Claribel flute','Oboe','Violone'];
      const footage=[8,16,4,2,null,8,8,4,8,8,8/3,8/5,4/3,null,8,8,8,16,4,4,4,null,null,8,8,8,8,8,8,16];
      s.organ=count===5?'Positif de chambre':'Orgue de concert';
      s.manuals=divisions.map((name,i)=>({...s.manuals[0],idx:i,name,rank:[],held:[],coupled:[]}));
      s.stops=Array.from({length:count},(_,i)=>({...s.stops[0],id:i,midx:Math.floor(i/(count/divisions.length)),manual:divisions[Math.floor(i/(count/divisions.length))],name:names[i%names.length]+" "+(footage[i%30]===null?"IV":footage[i%30]+"'"),label:footage[i%30]===null?"IV":null,on:i%3===0,hand:i%3===0,pitch:{...s.stops[0].pitch,footage:footage[i%30],native:footage[i%30]},tuning:{...s.stops[0].tuning}}));
      for(const m of s.manuals)m.rank=s.stops.filter(t=>t.midx===m.idx).map(t=>'s'+t.id);
      s.couplers=[];s.enclosures=[];s.layout={};s.setup.sources=[];
      return new Response(JSON.stringify(s),{status:200,headers:{'Content-Type':'application/json'}});
    };`;
  await d.send('Page.addScriptToEvaluateOnNewDocument', { source });
  for (const count of [5, 150]) for (const [width, height] of [[1500,950],[1080,1920],[900,600],[768,1024],[390,844],[320,740]]) {
    await d.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: width < 1100 });
    await d.navigate(`http://127.0.0.1:19951/?server=http://127.0.0.1:19950&fixture=${count}`);
    const visited = new Set(); let fits = true, targets = true, overlap = false;
    for(let page=0;page<100;page++) {
      const result = await d.eval(`(() => {
        const visible=e=>e.getClientRects().length && !e.closest('[data-page-hidden]');
        const panels=[...document.querySelectorAll('.panel')].filter(visible).map(e=>e.getBoundingClientRect());
        const controls=[...document.querySelectorAll('#console button')].filter(visible);
        return {
          ids:controls.filter(e=>e.dataset.key?.startsWith('stop-')).map(e=>e.dataset.key),
          fits:panels.every(r=>r.left>=-1&&r.right<=innerWidth+1&&r.top>=0&&r.bottom<=innerHeight+1),
          targets:controls.every(e=>{const r=e.getBoundingClientRect();return r.width>=55&&r.height>=55&&(!e.classList.contains('knob')||(r.width>=95&&r.height>=63))}),
          overlap:panels.some((a,i)=>panels.slice(i+1).some(b=>a.left<b.right-1&&a.right>b.left+1&&a.top<b.bottom-1&&a.bottom>b.top+1)),
          labels:controls.filter(e=>e.classList.contains('knob')).every(e=>[...e.querySelectorAll('.stop-name,.stop-pitch')].every(l=>{const r=l.getBoundingClientRect(),b=e.getBoundingClientRect();return r.left>=b.left&&r.right<=b.right&&r.top>=b.top&&r.bottom<=b.bottom})),
          next:!document.querySelector('.play-pages button:last-child').disabled,
        };
      })()`);
      for(const id of result.ids)visited.add(id);
      fits &&= result.fits && result.labels; targets &&= result.targets; overlap ||= result.overlap;
      if(!result.next)break;
      await d.click('.play-pages button:last-child'); await d.sleep(40);
    }
    h.check(visited.size === count, `${count} stops ${width}×${height}: every stop is reachable by page`);
    h.check(fits && targets && !overlap, `${count} stops ${width}×${height}: fits=${fits}, targets=${targets}, overlap=${overlap}`);
    await d.navigate(`http://127.0.0.1:19951/?server=http://127.0.0.1:19950&fixture=${count}`);
    await capture(`/tmp/aristide-new-${count}-${width}.png`);
    h.check((await d.eval('__errors')).length === 0, `${count} stops ${width}×${height}: no browser errors`);
  }
  await h.done(h.failures ? 1 : 0);
} catch (e) { console.error(e); await h.done(1); }
