import { test, expect } from "bun:test";
import { wireSaveAsForm, closeSaveAsForm, syncSaveAsForm } from "./settings-popovers.js";

const control = () => Object.assign(new EventTarget(), {
  value:"", textContent:"", disabled:false,
  classList:{add(){},remove(){}}, querySelectorAll:()=>[],
});
function fixture() {
  const el = Object.fromEntries(["saveAs","saveAsCancel","saveAsBtn","saveAsName","saveAsError"].map(name=>[name,control()]));
  el.saveAsName.value = "My organ";
  const sent = [];
  const editor = {
    el, base:"", root:{body:control()}, saveAsOpen:true, saveAsSession:1,
    saveAsFor:"Original", saveAsPending:"/api/organ/remove?stop=1",
    lastSnapshot:{organ:"Original",setup:{file:"original.toml"}},
    send:query=>sent.push(query),
    closeSaveAsForm:()=>closeSaveAsForm(editor),
  };
  wireSaveAsForm(editor);
  return {editor, sent, click:()=>el.saveAsBtn.dispatchEvent(new Event("click"))};
}
const tick = () => new Promise(resolve=>setTimeout(resolve,0));

test("save-copy submits once and replays the protected edit even if its poll arrives first", async () => {
  const original = globalThis.fetch, {editor,sent,click} = fixture();
  let resolve, requests = 0;
  try {
    globalThis.fetch = () => {requests++;return new Promise(done=>{resolve=done;});};
    click(); click();
    expect(requests).toBe(1);
    expect(editor.el.saveAsBtn.disabled).toBe(true);
    editor.lastSnapshot = {organ:"My organ",setup:{file:"my-organ.toml"}};
    syncSaveAsForm(editor);
    expect(editor.saveAsOpen).toBe(true);
    resolve(new Response("saved")); await tick();
    expect(editor.saveAsOpen).toBe(false);
    expect(sent).toEqual(["/api/organ/remove?stop=1"]);
    expect(editor.el.saveAsBtn.disabled).toBe(false);
  } finally {globalThis.fetch=original;}
});

test("closing a pending save-copy prevents a delayed response from replaying the edit", async () => {
  const original = globalThis.fetch, {editor,sent,click} = fixture();
  let resolve;
  try {
    globalThis.fetch = () => new Promise(done=>{resolve=done;});
    click(); editor.closeSaveAsForm();
    resolve(new Response("saved")); await tick();
    expect(sent).toEqual([]);
    expect(editor.saveAsOpen).toBe(false);
    expect(editor.el.saveAsBtn.disabled).toBe(false);
  } finally {globalThis.fetch=original;}
});
