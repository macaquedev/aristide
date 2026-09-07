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
    for (const width of [320, 390, 680, 768, 1000, 1100, ...(touch ? [1280, 1600] : [])]) {
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
            tiles: tiles.length > 0 && tiles.every(e => e.clientWidth >= 90 && e.clientWidth <= 96 && e.clientHeight >= 46 && e.clientHeight <= 72),
            fills: grids.every(e => Math.abs(e.clientWidth - e.parentElement.clientWidth) <= 2),
            controls: controls.every((a,i) => a.left >= 0 && a.right <= innerWidth && controls.slice(i+1).every(b => a.right <= b.left+1 || b.right <= a.left+1 || a.bottom <= b.top+1 || b.bottom <= a.top+1))
          };
        })()`);
        h.check(Object.values(result).every(Boolean), `${touch ? 'touch' : 'mouse'} ${width}px ${density}: ${JSON.stringify(result)}`);
      }
    }
  }
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
