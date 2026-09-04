// Delayed requests, keyboard-only Recent actions, and creating an organ on a phone.
// bun tools/e2e/loader-reliability-audit.js [server binary]
import { connect, launchHarness } from './cdp.js';
const h = launchHarness({name:'loader-reliability',serverPort:9930,uiPort:9931,cdpPort:9260});
const {check, post, state, settled, sleep} = h;
let fatal = false;
try {
  await h.waitForServer();
  await post(`/api/organ/load?path=${encodeURIComponent(h.demo)}`); await settled();
  await post('/api/organ/save_as?name=Morning%20practice'); await settled();
  await post('/api/organ/save_as?name=Evening%20practice'); await settled();
  const d = await connect(9260);
  await d.navigate('http://127.0.0.1:9931/?server=http://127.0.0.1:9930');
  await sleep(500);
  await d.eval(`window.__requests=[]; window.__holdPrefix=null; window.__release=null;
    window.__realFetch=window.fetch;
    window.fetch=(url,opts)=>{
      const path=new URL(url,location.href).pathname;
      window.__requests.push({path,method:opts?.method??'GET'});
      if(path===window.__holdPrefix) {
        window.__holdPrefix=null;
        return new Promise(resolve=>{window.__release=async(fail)=>resolve(fail ? new Response(fail,{status:400}) : await window.__realFetch(url,opts));});
      }
      return window.__realFetch(url,opts);
    }; true`);
  const waitState = async (predicate) => { for(let i=0;i<100;i++){const s=await state();if(predicate(s))return s;await sleep(200);}throw new Error('State did not reach expected result'); };
  const visible = (selector) => d.eval(`!!document.querySelector(${JSON.stringify(selector)})?.getClientRects().length`);
  const key = async (name, code=name) => {
    await d.send('Input.dispatchKeyEvent',{type:'keyDown',key:name,code,text:name==='Enter'?'\r':undefined,windowsVirtualKeyCode:name==='Enter'?13:undefined});
    await d.send('Input.dispatchKeyEvent',{type:'keyUp',key:name,code,windowsVirtualKeyCode:name==='Enter'?13:undefined});
  };
  const openPicker = async () => {
    await d.click('#organ-name');
    await d.eval(`[...document.querySelectorAll('#organ-menu-list button')].find(b=>b.textContent==='Load an organ…').click()`);
    await sleep(100);
  };
  const filter = async (text) => d.eval(`(()=>{const input=document.querySelector('#picker-search');input.value=${JSON.stringify(text)};input.dispatchEvent(new Event('input',{bubbles:true}));})()`);

  await openPicker();
  await d.eval(`window.__holdPrefix='/api/organ/load'`);
  await d.eval(`(()=>{const button=[...document.querySelectorAll('.picker-open')].find(b=>b.textContent.includes('Morning practice'));button.focus();button.click();})()`);
  await sleep(450);
  check(await visible('#picker'), 'a delayed load request does not close the loader');
  check(await visible('#picker-loading'), 'pending load has visible progress');
  check(await d.eval(`document.querySelector('#picker-sections').inert`), 'duplicate selections wait for acknowledgement');
  await d.eval(`window.__release('The selected organ is no longer available.')`); await sleep(250);
  check(await visible('#picker'), 'a refused load leaves the loader open');
  check(await d.eval(`document.querySelector('#picker-error').textContent.includes('no longer available')`), 'the failure appears inside the loader');
  check(!(await visible('#editor-error')), 'the load refusal is not hidden in a second error behind the dialog');
  check(await d.eval(`document.activeElement.matches('.picker-open')`), 'a failed load returns focus to the chosen organ');
  check(await d.eval(`!document.querySelector('#picker-sections').inert`), 'loading controls become usable again after failure');

  await filter('morning');
  check(await d.eval(`document.querySelectorAll('.picker-open').length===1`), 'Recent search filters by organ name');
  await filter('no such instrument');
  check(await d.eval(`document.querySelectorAll('.picker-open').length===0 && document.querySelector('#picker-library').textContent.includes('No matching')`), 'search has an informative empty state');
  await filter('morning');
  const original = (await state()).setup.file;
  await d.eval(`window.__requests=[];document.querySelector('.picker-forget').focus()`);
  await key('Enter'); await sleep(500);
  check(!(await state()).library.some(e=>e.name==='Morning practice'), 'Enter removes the chosen Recent entry');
  check((await state()).setup.file===original, 'removing a Recent entry keeps the current organ loaded');
  check(await d.eval(`!window.__requests.some(r=>r.path==='/api/organ/load')`), 'keyboard activation of Remove does not also send a load');

  await filter('');
  await d.eval(`window.__holdPrefix='/api/organ/load'`);
  await d.eval(`[...document.querySelectorAll('.picker-open')].find(b=>b.textContent.includes('Evening practice')).click()`);
  await sleep(200); await d.eval(`window.__release()`); await settled(); await sleep(500);
  check(!(await visible('#picker')), 'successfully reloading the same organ closes the loader');

  await openPicker(); await d.click('#picker-new-blank');
  await d.eval(`document.querySelector('#picker-name').value='Duplicate test';window.__requests=[];window.__holdPrefix='/api/organ/new';document.querySelector('#picker-name-form').requestSubmit();document.querySelector('#picker-name-form').requestSubmit()`);
  await sleep(200);
  check(await d.eval(`window.__requests.filter(r=>r.path==='/api/organ/new').length===1`), 'repeated submit creates only one request');
  await d.eval(`window.__release('An organ with that name already exists.')`); await sleep(200);
  check(await d.eval(`document.querySelector('#picker-error').textContent.includes('already exists')`), 'new-organ validation is shown beside the name form');
  await d.click('#picker-close');

  // A failed native chooser is a visible error rather than a silent no-op.
  await openPicker();
  await d.eval(`window.__TAURI__={core:{invoke:async()=>{throw new Error('Chooser unavailable')}}}`);
  await d.click('#picker-new-set'); await sleep(150);
  check(await d.eval(`document.querySelector('#picker-error').textContent.includes('Chooser unavailable')`), 'native file chooser errors are visible');
  await d.eval('delete window.__TAURI__');
  await d.click('#picker-close');
  await openPicker();

  await d.send('Emulation.setDeviceMetricsOverride',{width:390,height:844,deviceScaleFactor:1,mobile:true});
  await d.send('Emulation.setTouchEmulationEnabled',{enabled:true,maxTouchPoints:5});
  await sleep(200); await d.shot('/tmp/aristide-loader-phone.png');
  check(await d.eval(`(()=>{const r=document.querySelector('#picker .modal-card').getBoundingClientRect();return r.left>=0&&r.right<=innerWidth&&r.bottom<=innerHeight;})()`), 'loader fits the phone viewport');
  check(await d.eval(`(()=>{const r=document.querySelector('.picker-forget').getBoundingClientRect();return r.width>=44&&r.height>=44;})()`), 'Recent removal has a visible full-size touch target');
  await d.click('#picker-new-blank');
  await d.eval(`document.querySelector('#picker-name').value='Touch study';document.querySelector('#picker-name-form').requestSubmit()`);
  await waitState(s=>s.organ==='Touch study'&&!s.loading); await sleep(400);
  check((await state()).organ==='Touch study' && !(await visible('#picker')), 'creating a blank organ opens it successfully');
  await d.click('#editor-add-button'); await d.click('#editor-add-manual');
  await d.eval(`window.__requests=[];document.querySelector('#editor-add-manual-name').value='Practice';document.querySelector('#editor-add-manual-form').requestSubmit()`);
  await waitState(s=>s.manuals.some(m=>m.name==='Practice')); await sleep(400);
  check((await state()).manuals.some(m=>m.name==='Practice'), 'a keyboard can be added in the phone layout');
  check(await d.eval(`!window.__requests.some(r=>r.path==='/api/organ/panel/place')`), 'phone creation does not save desktop panel coordinates');
  check(!Object.keys((await state()).layout??{}).some(k=>k.includes('Practice')), 'new keyboard retains automatic desktop placement');
} catch(e) {fatal=true;console.error(e);}
console.log(`${h.failures} failed${fatal?'; fatal error':''}`);
await h.done(fatal || h.failures ? 1 : 0);
