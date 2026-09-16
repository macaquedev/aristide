import { test, expect } from "bun:test";
import { rescanMidi } from "./settings-popovers.js";

const control = () => ({disabled:false, textContent:""});
const fixture = () => ({base:"http://server", el:{
  midiRescan:control(), midiPortsNote:control(),
}});
const midi = (requested, completed, error = null) => ({
  ports:[{name:"Keyboard"}, {name:"Computer keyboard", virtual:true}],
  scan:{requested, completed, error},
});
const response = data => new Response(JSON.stringify({midi:data}));
async function withFetch(fetch, run) {
  const original = globalThis.fetch;
  globalThis.fetch = fetch;
  try { await run(); } finally { globalThis.fetch = original; }
}
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

test("rescan polls its own completion even without console polls or changed ports", async () => {
  const editor = fixture(), calls = [];
  await withFetch(async (url, opts) => {
    calls.push([url, opts.method]);
    return response(midi(2, calls.length === 1 ? 1 : 2));
  }, async () => {
    const pending = rescanMidi(editor, {pollMs:1});
    await rescanMidi(editor);
    expect(editor.el.midiRescan.disabled).toBe(true);
    await pending;
    expect(calls).toEqual([
      ["http://server/api/midi/rescan", "POST"],
      ["http://server/api/state", "GET"],
    ]);
    expect(editor.el.midiRescan.disabled).toBe(false);
    expect(editor.el.midiPortsNote.textContent).toContain("1 MIDI input connected");
  });
});

test("older servers cannot leave the button stuck or claim completion", async () => {
  const editor = fixture();
  await withFetch(async () => response({ports:[]}), async () => {
    await rescanMidi(editor);
    expect(editor.el.midiRescan.disabled).toBe(false);
    expect(editor.el.midiPortsNote.textContent).toContain("cannot report completion");
    expect(editor.el.midiPortsNote.textContent).not.toContain("Rescan complete.");
  });
});

test("a stalled supervisor times out and allows another request", async () => {
  const editor = fixture();
  await withFetch(async () => response(midi(1, 0)), async () => {
    await rescanMidi(editor, {timeoutMs:15, pollMs:1});
    expect(editor.el.midiRescan.disabled).toBe(false);
    expect(editor.el.midiPortsNote.textContent).toContain("could not be confirmed");
    await tick();
    expect(editor.el.midiPortsNote.textContent).toContain("could not be confirmed");
  });
});

test("a hung HTTP request is aborted and releases the button", async () => {
  const editor = fixture();
  let aborted = false;
  await withFetch((_url, {signal}) => new Promise((_resolve, reject) => {
    signal.addEventListener("abort", () => {aborted = true; reject(new Error("aborted"));});
  }), async () => {
    await rescanMidi(editor, {timeoutMs:15});
    expect(aborted).toBe(true);
    expect(editor.el.midiRescan.disabled).toBe(false);
    expect(editor.el.midiPortsNote.textContent).toContain("could not be confirmed");
  });
});

test("a failed completion poll and a server restart both permit retry", async () => {
  for (const next of [() => {throw new Error("Connection lost");}, () => response(midi(0, 0))]) {
    const editor = fixture();
    let calls = 0;
    await withFetch(async () => ++calls === 1 ? response(midi(1, 0)) : next(), async () => {
      await rescanMidi(editor, {pollMs:1});
      expect(editor.el.midiRescan.disabled).toBe(false);
      expect(editor.el.midiPortsNote.textContent).toContain("Rescan failed:");
    });
  }
});

test("scan completion reports device errors or zero hardware inputs", async () => {
  for (const [data, message] of [
    [midi(1, 1, "MIDI unavailable"), "Rescan failed: MIDI unavailable"],
    [{...midi(1, 1), ports:[{virtual:true}]}, "0 MIDI inputs connected"],
  ]) {
    const editor = fixture();
    await withFetch(async () => response(data), async () => {
      await rescanMidi(editor);
      expect(editor.el.midiRescan.disabled).toBe(false);
      expect(editor.el.midiPortsNote.textContent).toContain(message);
    });
  }
});


test("a later client request cannot hide completion of this client's scan", async () => {
  const editor = fixture();
  let calls = 0;
  await withFetch(async () => response(++calls === 1 ? midi(1, 0) : midi(2, 1)), async () => {
    await rescanMidi(editor, {pollMs:1});
    expect(editor.el.midiRescan.disabled).toBe(false);
    expect(editor.el.midiPortsNote.textContent).toContain("Rescan complete.");
  });
});
