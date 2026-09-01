// Minimal CDP driver: node/bun script, drives a headless chromium that
// was launched with --remote-debugging-port=9222.
//   bun cdp.js <url> <outdir> <script.js-with-steps>
// The steps file exports async (drive) => {} where drive has:
//   eval(expr) -> value, shot(name), sleep(ms), click(selector), set(selector, value)

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const REPO = new URL("../..", import.meta.url).pathname;

/// Boots the rig every tools/e2e/*-audit.js drives against the REAL
/// server and console UI: finds the server binary (an explicit
/// `serverArg`, else release falling back to debug), a scratch dir,
/// the static file server for ui/, and headless chromium — then hands
/// back the small vocabulary every audit's own checks are written in.
///
/// `name` becomes the scratch dir's prefix ("aristide-<name>-").
/// `needsDemo` (default true) requires and exposes the gitignored demo
/// set's path as `demo`, erroring out first if it's missing.
/// `isolateConfig` (default true) points the server at a throwaway
/// XDG_CONFIG_HOME under the scratch dir, so the player's real library
/// and wiring are never touched; an audit that means to exercise the
/// real library (hex-audit) passes false.
/// `windowSize` (default true) sets chromium's window to 1500×950 —
/// off for an audit with no pixel geometry to get right.
export function launchHarness({
  name,
  serverPort,
  uiPort,
  cdpPort,
  serverArg = process.argv[2],
  needsDemo = true,
  isolateConfig = true,
  windowSize = true,
} = {}) {
  const S = `http://127.0.0.1:${serverPort}`;

  let failures = 0;
  const check = (ok, what) => {
    console.log(`${ok ? "PASS" : "FAIL"}  ${what}`);
    if (!ok) failures++;
    return ok;
  };
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const state = async () => (await fetch(S + "/api/state")).json();
  // Waits out a rebuild (a fresh load, a structural edit) rather than
  // reading a snapshot mid-flight.
  const settled = async () => {
    for (let i = 0; i < 100; i++) {
      const s = await state();
      if (s.organ && !s.loading) return s;
      await sleep(300);
    }
    throw new Error("organ never settled");
  };
  // A one-off POST outside the audit's own drive — loading an organ,
  // provoking a refusal, feeding a computer key — not a UI gesture.
  const post = async (path) => {
    const res = await fetch(S + path, { method: "POST" });
    return { ok: res.ok, status: res.status, body: await res.text() };
  };
  // Retries `state()` until the server answers at all, freshly spawned.
  const waitForServer = async (attempts = 100) => {
    for (let i = 0; i < attempts; i++) {
      try {
        await state();
        return;
      } catch {
        await sleep(200);
      }
    }
  };

  const serverBin = serverArg ??
    [join(REPO, "target/release/aristide-server"), join(REPO, "target/debug/aristide-server")]
      .find(existsSync);
  if (!serverBin) {
    console.error("no server binary — cargo build -p aristide-server first");
    process.exit(2);
  }
  const demo = join(REPO, "testsets/grandorgue-demo/demo.organ");
  if (needsDemo && !existsSync(demo)) {
    console.error("no demo set — see CLAUDE.md's testsets note");
    process.exit(2);
  }

  const scratch = mkdtempSync(join(tmpdir(), `aristide-${name}-`));
  const server = spawn(serverBin, ["--http-port", String(serverPort)], {
    stdio: ["ignore", "ignore", "pipe"],
    env: isolateConfig ? { ...process.env, XDG_CONFIG_HOME: join(scratch, "config") } : process.env,
  });
  let serverLog = "";
  server.stderr.on("data", (d) => (serverLog += d));

  const ui = Bun.serve({
    port: uiPort,
    fetch(req) {
      const path = new URL(req.url).pathname;
      const file = Bun.file(join(REPO, "crates/aristide-console/ui", path === "/" ? "index.html" : path));
      return file.exists().then((ok) => (ok ? new Response(file) : new Response("nope", { status: 404 })));
    },
  });

  const chrome = spawn("chromium", [
    "--headless", "--disable-gpu", `--remote-debugging-port=${cdpPort}`,
    ...(windowSize ? ["--window-size=1500,950"] : []),
    `--user-data-dir=${join(scratch, "chrome")}`, "about:blank",
  ], { stdio: "ignore" });

  // Kills every process this spawned and removes the scratch dir —
  // without exiting, for an audit whose own cleanup needs the server
  // alive a moment longer first (see hex-audit's throwaway organ).
  const cleanup = async () => {
    try { chrome.kill(); } catch {}
    try { server.kill(); } catch {}
    try { ui.stop(true); } catch {}
    try { rmSync(scratch, { recursive: true, force: true }); } catch {}
  };
  const done = async (code) => {
    await cleanup();
    process.exit(code);
  };

  return {
    REPO, S, demo, scratch,
    check, sleep, state, settled, post, waitForServer, cleanup, done,
    get failures() { return failures; },
    get serverLog() { return serverLog; },
  };
}

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
