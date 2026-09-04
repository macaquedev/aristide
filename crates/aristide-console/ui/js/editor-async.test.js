import { test, expect } from "bun:test";
import { Editor } from "./editor.js";
import { commands } from "./api.js";
import { fetchOfferings } from "./editor/library-drawer.js";

const deferred = () => {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
};
function quickEditor() {
  const ack = deferred(), sent = [];
  const editor = Object.create(Editor.prototype);
  editor.lastSnapshot = { controls: [], control_learning: null };
  editor.send = query => { sent.push(query); return sent.length === 1 ? ack.promise : Promise.resolve({ok:true}); };
  return {editor, ack, sent};
}

test("a stale poll before learn acknowledgement preserves the requested piston action", async () => {
  const {editor, ack, sent} = quickEditor();
  editor.quickBindListen("general:1", null, false);
  editor.pumpQuickBind(editor.lastSnapshot);
  expect(editor.quickBind).not.toBeNull();
  editor.lastSnapshot = {controls:[], control_learning:0};
  ack.resolve({ok:true}); await ack.promise;
  editor.pumpQuickBind({controls:[{slot:0,trigger:"key:KeyQ"}],control_learning:null});
  expect(sent).toEqual([commands.controlLearn(0),commands.controlBind(0,"general:1")]);
});

test("a cancelled quick learn cannot be revived by its late acknowledgement", async () => {
  const {editor, ack, sent} = quickEditor();
  editor.quickBindListen("general:1", null, false);
  editor.quickBindListen("general:1", null, true);
  editor.lastSnapshot = {controls:[{slot:0,trigger:"key:KeyQ"}],control_learning:null};
  ack.resolve({ok:true}); await ack.promise;
  expect(editor.quickBind).toBeNull();
  expect(sent).toEqual([commands.controlLearn(0), commands.controlLearn(null)]);
});

test("a refused quick learn clears the pending assignment", async () => {
  const {editor, ack, sent} = quickEditor();
  editor.quickBindListen("general:1", null, false);
  ack.resolve({ok:false}); await ack.promise;
  expect(editor.quickBind).toBeNull();
  expect(sent).toHaveLength(1);
});

test("an older source response cannot replace a newer source list", async () => {
  const original = globalThis.fetch, first = deferred();
  const editor = {base:"",lastSnapshot:{setup:{file:"organ.toml"}},offerings:null};
  let calls = 0;
  try {
    globalThis.fetch = () => ++calls === 1 ? first.promise : Promise.resolve(Response.json({sources:[{alias:"new"}]}));
    const pending = fetchOfferings(editor, false);
    await fetchOfferings(editor, false);
    first.resolve(Response.json({sources:[{alias:"old"}]})); await pending;
    expect(editor.offerings).toEqual([{alias:"new"}]);
  } finally { globalThis.fetch = original; }
});

test("a source response for the previous organ is ignored", async () => {
  const original = globalThis.fetch, response = deferred();
  const editor = {base:"",lastSnapshot:{setup:{file:"first.toml"}},offerings:null};
  try {
    globalThis.fetch = () => response.promise;
    const pending = fetchOfferings(editor, false);
    editor.lastSnapshot = {setup:{file:"second.toml"}};
    response.resolve(Response.json({sources:[{alias:"first"}]})); await pending;
    expect(editor.offerings).toBeNull();
  } finally { globalThis.fetch = original; }
});
