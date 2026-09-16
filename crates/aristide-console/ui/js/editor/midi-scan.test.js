import { test, expect } from "bun:test";
import { wireMidiForm, syncMidiScan } from "./settings-popovers.js";

const control = () => Object.assign(new EventTarget(), {disabled:false, textContent:""});
function fixture(send) {
  const editor = {
    el: {midiClose:control(), midiRescan:control(), midiPortsNote:control()},
    closeMidiForm() {}, send,
  };
  wireMidiForm(editor);
  return editor;
}

test("rescan remains busy until the server confirms completion, even with unchanged ports", async () => {
  let requests = 0;
  const editor = fixture(async () => { requests++; return {ok:true}; });
  editor.el.midiRescan.dispatchEvent(new Event("click"));
  editor.el.midiRescan.dispatchEvent(new Event("click"));
  await Promise.resolve();
  expect(requests).toBe(1);
  expect(editor.el.midiRescan.disabled).toBe(true);
  const midi = {ports:[{name:"Keyboard"}, {name:"Computer keyboard", virtual:true}],
    scan:{requested:2, completed:1, error:null}};
  syncMidiScan(editor, midi);
  expect(editor.el.midiPortsNote.textContent).toContain("Scanning");
  syncMidiScan(editor, {...midi, scan:{requested:2, completed:2, error:null}});
  expect(editor.el.midiRescan.disabled).toBe(false);
  expect(editor.el.midiPortsNote.textContent).toContain("1 MIDI input connected");
});

test("rescan reports request and device errors and permits retry", async () => {
  const editor = fixture(async () => ({ok:false, error:"Server unavailable"}));
  editor.el.midiRescan.dispatchEvent(new Event("click"));
  await Promise.resolve();
  expect(editor.el.midiRescan.disabled).toBe(false);
  expect(editor.el.midiPortsNote.textContent).toContain("Server unavailable");
  syncMidiScan(editor, {ports:[], scan:{requested:1, completed:1, error:"MIDI unavailable"}});
  expect(editor.el.midiPortsNote.textContent).toContain("MIDI unavailable");
  expect(editor.el.midiRescan.disabled).toBe(false);
});

test("rescan explicitly reports no hardware inputs", () => {
  const editor = fixture();
  syncMidiScan(editor, {ports:[{virtual:true}], scan:{requested:1, completed:1}});
  expect(editor.el.midiPortsNote.textContent).toContain("0 MIDI inputs connected");
});
