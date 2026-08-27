// Minimal CDP driver: node/bun script, drives a headless chromium that
// was launched with --remote-debugging-port=9222.
//   bun cdp.js <url> <outdir> <script.js-with-steps>
// The steps file exports async (drive) => {} where drive has:
//   eval(expr) -> value, shot(name), sleep(ms), click(selector), set(selector, value)

export async function connect(port = 9222) {
  let list;
  for (let i = 0; i < 50; i++) {
    try {
      list = await fetch(`http://127.0.0.1:${port}/json`).then((r) => r.json());
      if (list.some((t) => t.type === "page")) break;
    } catch {}
    await new Promise((r) => setTimeout(r, 200));
  }
  const target = list.find((t) => t.type === "page");
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  let id = 0;
  const pending = new Map();
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.id && pending.has(m.id)) {
      const { res, rej } = pending.get(m.id);
      pending.delete(m.id);
      m.error ? rej(new Error(JSON.stringify(m.error))) : res(m.result);
    }
  };
  await new Promise((r) => (ws.onopen = r));
  const send = (method, params = {}) =>
    new Promise((res, rej) => {
      const i = ++id;
      pending.set(i, { res, rej });
      ws.send(JSON.stringify({ id: i, method, params }));
    });
  await send("Page.enable");
  await send("Runtime.enable");

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const evaluate = async (expression) => {
    const out = await send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (out.exceptionDetails) throw new Error(JSON.stringify(out.exceptionDetails));
    return out.result?.value;
  };
  return {
    send,
    sleep,
    eval: evaluate,
    navigate: async (url) => {
      await send("Page.navigate", { url });
      await sleep(1200);
    },
    shot: async (path) => {
      const { data } = await send("Page.captureScreenshot", { format: "png" });
      await Bun.write(path, Buffer.from(data, "base64"));
      return path;
    },
    click: (selector) =>
      evaluate(`(() => { const el = document.querySelector(${JSON.stringify(selector)});
        if (!el) return "MISSING " + ${JSON.stringify(selector)};
        el.click(); return "ok"; })()`),
    set: (selector, value) =>
      evaluate(`(() => { const el = document.querySelector(${JSON.stringify(selector)});
        if (!el) return "MISSING " + ${JSON.stringify(selector)};
        el.value = ${JSON.stringify(value)};
        el.dispatchEvent(new Event("change", { bubbles: true })); return "ok"; })()`),
  };
}
