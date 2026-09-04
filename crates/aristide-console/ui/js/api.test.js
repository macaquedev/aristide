import { test, expect } from "bun:test";
import { connect } from "./api.js";

const sleep = (ms) => new Promise(resolve => setTimeout(resolve, ms));
const deferred = () => {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
};

test("a pending command pauses new polls and returns its acknowledged state", async () => {
  const original = globalThis.fetch;
  const post = deferred();
  const calls = [], states = [];
  let send;
  try {
    globalThis.fetch = async (url, options) => {
      calls.push(options.method);
      return options.method === "POST" ? post.promise : Response.json({ organ: "Old" });
    };
    send = connect("", state => states.push(state), () => {});
    await sleep(10);
    const pending = send("/api/organ/load?path=next");
    await sleep(300);
    expect(calls).toEqual(["GET", "POST"]);
    post.resolve(Response.json({ organ: "New" }));
    const result = await pending;
    expect(result.ok).toBe(true);
    expect(states.at(-1)).toEqual({ organ: "New" });
    expect(result.data).toEqual(states.at(-1));
  } finally { send?.disconnect(); globalThis.fetch = original; }
});

test("a poll dispatched before a command cannot overwrite its newer response", async () => {
  const original = globalThis.fetch, poll = deferred(), states = [];
  let send;
  try {
    globalThis.fetch = async (_, options) => options.method === "GET" ? poll.promise : Response.json({ organ: "New" });
    send = connect("", state => states.push(state), () => {});
    await send("/api/organ/load?path=next");
    poll.resolve(Response.json({ organ: "Old" }));
    await sleep(10);
    expect(states).toEqual([{ organ: "New" }]);
  } finally { send?.disconnect(); globalThis.fetch = original; }
});

test("dialog-owned errors return their reason without showing a second global error", async () => {
  const original = globalThis.fetch, errors = [], refusals = [];
  let send;
  try {
    globalThis.fetch = async (_, options) => options.method === "GET"
      ? Response.json({}) : new Response("That name already exists", { status: 400 });
    send = connect("", () => {}, e => errors.push(e), e => refusals.push(e));
    const result = await send("/api/organ/new?name=Taken", { reportErrors: false });
    expect(result).toEqual({ ok: false, status: 400, data: null, error: "That name already exists" });
    expect(errors).toEqual([]);
    expect(refusals).toEqual([]);
    await send("/api/organ/new?name=Taken");
    expect(refusals).toEqual(["That name already exists"]);
  } finally { send?.disconnect(); globalThis.fetch = original; }
});

test("overlapping commands resume just one poll loop after both complete", async () => {
  const original = globalThis.fetch, a = deferred(), b = deferred();
  let send, polls = 0;
  try {
    globalThis.fetch = async (url, options) => {
      if (options.method === "GET") { polls++; return Response.json({}); }
      return url === "/a" ? a.promise : b.promise;
    };
    send = connect("", () => {}, () => {});
    const first = send("/a"), second = send("/b");
    a.resolve(Response.json({})); await first;
    await sleep(180); expect(polls).toBe(1);
    b.resolve(Response.json({})); await second;
    await sleep(180); expect(polls).toBe(2);
  } finally { send?.disconnect(); globalThis.fetch = original; }
});
