// A playable 200-stop composite checks density without needing a huge sample set.
// bun tools/e2e/stop-density-audit.js [server binary]
import { join } from 'node:path';
import { connect, launchHarness } from './cdp.js';
const h=launchHarness({name:'stop-density',serverPort:9970,uiPort:9971,cdpPort:9294});
try {
  const names=['PEDAL','GREAT','SWELL','CHOIR','SOLO'];
  const labels=['Principal','Bourdon','Viola da gamba','Flûte harmonique','Salicional','Quintaton','Trompette','Clarinette','Voix céleste','Dulciana','Flauto traverso','Cor anglais','Contrebombarde','Flûte octaviante','Sesquialtera','Plein jeu'];
  let text=`name = "Large console — 200 stops"\n[sources]\ndemo = ${JSON.stringify(h.demo)}\n`;
  for(const name of names) text+=`\n[[manual]]\nname = "${name}"\nlow = "C2"\nhigh = "C7"\n`;
  for(const name of names) for(let i=0;i<40;i++) {
    text+=`\n[[stop]]\nfrom = "demo"\nmanual = "First Manual"\nstop = "Montre 8'"\non = "${name}"\nrename = ${JSON.stringify(labels[i%labels.length]+(i>=labels.length?' '+['doux','fort'][Math.floor(i/labels.length)-1]:''))}\n`;
    if(i===14) text+='pitch_label = "2 2/3′"\n';
  }
  const file=join(h.scratch,'large.toml');await Bun.write(file,text);
  await h.waitForServer();await h.post(`/api/organ/load?path=${encodeURIComponent(file)}`);
  const snapshot=await h.settled();
  h.check(snapshot.stops.length===200,'load a real 200-stop instrument');
  const d=await connect(9294);
  await d.navigate('http://127.0.0.1:9971/?server=http://127.0.0.1:9970');
  const measure=()=>d.eval(`(()=>{
    const stops=[...document.querySelectorAll('.panel-jamb .knob[data-key^="stop-"]')];
    const rects=stops.map(e=>e.getBoundingClientRect());
    const panels=[...document.querySelectorAll('.panel')].filter(e=>e.offsetHeight).map(e=>e.getBoundingClientRect());
    const heights=rects.map(r=>r.height).sort((a,b)=>a-b);
    return {count:stops.length,median:heights[Math.floor(heights.length/2)],min:heights[0],
      last:Math.max(...rects.map(r=>r.bottom)),
      readable:stops.every(e=>[...e.querySelectorAll('.stop-name,.stop-pitch')].every(label=>parseFloat(getComputedStyle(label).fontSize)>=13 && label.scrollWidth<=label.clientWidth+1 && label.getBoundingClientRect().bottom<=e.getBoundingClientRect().bottom)),
      fits:document.documentElement.scrollWidth<=innerWidth,
      overlaps:panels.some((a,i)=>panels.slice(i+1).some(b=>a.left<b.right-1 && b.left<a.right-1 && a.top<b.bottom-1 && b.top<a.bottom-1)),
      controls:Math.max(...panels.map(r=>r.bottom))};
  })()`);
  for(const [width,height] of [[1919,1080],[2560,1440],[3838,1999]]) {
    await d.send('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:false});
    for(const density of ['compact','regular','spacious']) {
      await d.eval(`document.body.dataset.density=${JSON.stringify(density)}`);await d.sleep(180);
      const g=await measure();
      h.check(g.count===200 && g.readable && g.fits && !g.overlaps,`${width}px ${density}: every stop remains readable and reachable`);
      h.check(g.median<={compact:28,regular:32,spacious:40}[density],`${width}px ${density}: compact controls, median ${g.median}px`);
      if(density==='regular') h.check(g.last<=height,`${width}px: all 200 stop controls fit vertically`);
      // Spacious deliberately trades capacity for breathing room; the full
      // console must fit in the default and compact desktop presets.
      if(width>=2560 && density!=='spacious') h.check(g.controls<=height,`${width}px ${density}: the complete console fits on screen`);
    }
  }
  await d.send('Emulation.setDeviceMetricsOverride',{width:1919,height:1080,deviceScaleFactor:1,mobile:false});
  await d.eval(`document.body.dataset.density='regular'`);await d.sleep(200);
  const target=snapshot.stops[89], neighbor=snapshot.stops[90];
  const selector=`.knob[data-key="stop-${target.id}"]`;
  const p=await d.eval(`(()=>{const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2}})()`);
  await d.send('Input.dispatchMouseEvent',{type:'mousePressed',...p,button:'left',clickCount:1});
  await d.send('Input.dispatchMouseEvent',{type:'mouseReleased',...p,button:'left',clickCount:1});await d.sleep(300);
  const drawn=await h.state();
  h.check(drawn.stops.find(s=>s.id===target.id).on && !drawn.stops.find(s=>s.id===neighbor.id).on,'clicking a dense stop toggles only that stop');
  const longName='Contrebombarde harmonique extraordinaire';
  await h.post(`/api/organ/stop/rename?stop=${target.id}&name=${encodeURIComponent(longName)}`);await d.sleep(300);
  const long=await d.eval(`(()=>{const e=document.querySelector(${JSON.stringify(selector)}),label=e.querySelector('.stop-name');return {name:label.textContent,font:parseFloat(getComputedStyle(label).fontSize),fits:label.scrollWidth<=label.clientWidth+1 && label.getBoundingClientRect().bottom<=e.getBoundingClientRect().bottom};})()`);
  h.check(long.name===longName && long.font>=13 && long.fits,'long stop names wrap in full without tiny type');
  await d.shot('/tmp/aristide-200-stops-desktop.png');
  await d.send('Emulation.setDeviceMetricsOverride',{width:2560,height:1440,deviceScaleFactor:1,mobile:false});await d.sleep(250);
  await d.shot('/tmp/aristide-200-stops-wide.png');
  await d.send('Emulation.setTouchEmulationEnabled',{enabled:true});
  for(const width of [320,390,768,1024]) {
    await d.send('Emulation.setDeviceMetricsOverride',{width,height:900,deviceScaleFactor:1,mobile:true});await d.sleep(200);
    const g=await measure();
    h.check(g.min>=44 && g.readable && g.fits && !g.overlaps,`${width}px touch: compact layout retains readable labels and 44px tap targets`);
  }
  await d.send('Emulation.setDeviceMetricsOverride',{width:390,height:844,deviceScaleFactor:1,mobile:true});await d.sleep(200);
  await d.shot('/tmp/aristide-200-stops-phone.png');
} catch(e) {h.check(false,e.stack || String(e));}
finally {await h.done(h.failures?1:0);}
