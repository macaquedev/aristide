// Responsive stop layout must survive every desktop spacing preset.
// bun tools/e2e/homepage-layout-audit.js [server binary]
import { connect, launchHarness } from './cdp.js';
const h = launchHarness({ name: 'homepage-layout', serverPort: 9940, uiPort: 9941, cdpPort: 9270 });
try {
  await h.waitForServer();
  await h.post(`/api/organ/load?path=${encodeURIComponent(h.demo)}`);
  await h.settled();
  const d = await connect(9270);
  await d.navigate('http://127.0.0.1:9941/?server=http://127.0.0.1:9940');
  for (const touch of [false, true]) {
    await d.send('Emulation.setTouchEmulationEnabled', { enabled: touch });
    for (const width of [320, 390, 680, 768, 1000, 1100, 1280, 1500, 1920]) {
      await d.send('Emulation.setDeviceMetricsOverride', { width, height: 900, deviceScaleFactor: 1, mobile: touch });
      for (const density of ['compact', 'regular', 'spacious']) {
        await d.eval(`document.body.dataset.density = ${JSON.stringify(density)}`);
        await d.sleep(80);
        const result = await d.eval(`(() => {
          const tiles = [...document.querySelectorAll('.panel-jamb .knob')];
          const grids = [...document.querySelectorAll('.panel-jamb:not(.empty) .division-knobs')];
          const controls = [...document.querySelectorAll('.menubar button, #gain')].filter(e => e.getClientRects().length).map(e => e.getBoundingClientRect());
          return {
            fits: document.documentElement.scrollWidth <= innerWidth,
            tiles: tiles.length > 0 && tiles.every(e => e.clientWidth >= 90 && e.offsetHeight >= 54),
            fills: grids.every(e => Math.abs(e.clientWidth - e.parentElement.clientWidth) <= 2),
            panels: (() => { const panels = [...document.querySelectorAll('.panel')].filter(e => e.getClientRects().length).map(e => e.getBoundingClientRect()); return panels.every((a,i) => panels.slice(i+1).every(b => a.right <= b.left+1 || b.right <= a.left+1 || a.bottom <= b.top+1 || b.bottom <= a.top+1)); })(),
            controls: controls.every((a,i) => a.left >= 0 && a.right <= innerWidth && controls.slice(i+1).every(b => a.right <= b.left+1 || b.right <= a.left+1 || a.bottom <= b.top+1 || b.bottom <= a.top+1))
          };
        })()`);
        h.check(Object.values(result).every(Boolean), `${touch ? 'touch' : 'mouse'} ${width}px ${density}: ${JSON.stringify(result)}`);
      }
    }
  }
  // A saved panel must return to the exact pixel position after the server
  // echoes its normalized coordinates and after reopening the console.
  await d.send('Emulation.setTouchEmulationEnabled', { enabled: false });
  await d.send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
  await d.eval(`document.body.dataset.density='regular'`); await d.sleep(200);
  await d.click('#editor-lock');
  const grip = await d.eval(`(()=>{const e=document.querySelector('.panel-jamb .panel-chrome');e.scrollIntoView({block:'center'});const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
  const mouse = (type,x,y)=>d.send('Input.dispatchMouseEvent',{type,x,y,button:'left',clickCount:1});
  await mouse('mousePressed',grip.x,grip.y);
  await mouse('mouseMoved',grip.x+10,grip.y+20);
  await mouse('mouseReleased',grip.x+10,grip.y+20); await d.sleep(500);
  const position = () => d.eval(`(()=>{const e=document.querySelector('.panel-jamb');return {id:e.dataset.panel,x:parseFloat(e.style.left),y:parseFloat(e.style.top),w:document.querySelector('#console-canvas').clientWidth,h:document.querySelector('#console-canvas').clientHeight};})()`);
  const before = await position(), saved = (await h.state()).layout?.[before.id];
  h.check(saved && Math.abs(before.x-saved.x*before.w)<=1 && Math.abs(before.y-saved.y*before.h)<=1,'dragged panel uses the same coordinates as persistence');
  await d.navigate('http://127.0.0.1:9941/?server=http://127.0.0.1:9940'); await d.sleep(250);
  const after = await position();
  h.check(Math.abs(before.x-after.x)<=1 && Math.abs(before.y-after.y)<=1,'saved panel keeps its position after reopening');
  await d.send('Emulation.setDeviceMetricsOverride', { width: 1000, height: 1000, deviceScaleFactor: 1, mobile: true });
  await d.sleep(200);
  await d.shot('/tmp/aristide-homepage-tablet.png');
  await d.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  await d.sleep(200);
  await d.shot('/tmp/aristide-homepage-phone.png');
  await h.done(h.failures ? 1 : 0);
} catch (e) {
  console.error(e);
  await h.done(1);
}
