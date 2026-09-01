// Unit tests for the pure half of the tuning popover: the cascade
// resolution (resolveTuningView), kept separate from the DOM-painting
// half (syncTuningForm) precisely so it can be checked like this,
// without a browser.
//
//   bun test crates/aristide-console/ui/js/editor/tuning-popover.test.js

import { test, expect } from "bun:test";
import { resolveTuningView } from "./tuning-popover.js";

const organTuning = { temperament: "original", reference: { key: 69, hz: 440 } };

test("organ scope always plays its own tuning, unconditionally", () => {
  const view = resolveTuningView({ kind: "organ" }, { tuning: organTuning });
  expect(view.closeForm).toBe(false);
  expect(view.title).toBe("Whole instrument");
  expect(view.showFollowRow).toBe(false);
  expect(view.following).toBe(false);
  expect(view.tuning).toBe(organTuning);
  expect(view.resolvedLine).toBeNull();
});

test("division with no tuning of its own follows the instrument", () => {
  const snap = { tuning: organTuning, manuals: [{ idx: 0, name: "Great" }], manual_tuning: [] };
  const view = resolveTuningView({ kind: "division", idx: 0 }, snap);
  expect(view.title).toBe("Great");
  expect(view.following).toBe(true);
  expect(view.followValue).toBe("organ");
  expect(view.tuning).toBe(organTuning);
  expect(view.resolvedLine).toEqual({
    primaryLabel: "the instrument",
    link: { kind: "organ" },
    chainParts: null,
  });
});

test("division with its own tuning is never following", () => {
  const own = { temperament: "kirnberger3", reference: { key: 69, hz: 415 } };
  const snap = {
    tuning: organTuning,
    manuals: [{ idx: 0, name: "Great" }],
    manual_tuning: [{ idx: 0, ...own }],
  };
  const view = resolveTuningView({ kind: "division", idx: 0 }, snap);
  expect(view.following).toBe(false);
  expect(view.followValue).toBe("own");
  expect(view.tuning).toEqual({ idx: 0, ...own });
  expect(view.resolvedLine).toBeNull();
});

test("a division the snapshot no longer has closes the popover", () => {
  const snap = { tuning: organTuning, manuals: [], manual_tuning: [] };
  expect(resolveTuningView({ kind: "division", idx: 0 }, snap)).toEqual({ closeForm: true });
});

test("a stop with follow=auto and no division/source tuning defaults to the instrument", () => {
  const snap = {
    tuning: organTuning,
    manuals: [{ idx: 0, name: "Great" }],
    manual_tuning: [],
    source_tuning: [],
    stops: [{ id: 1, midx: 0, tuning: { scope: "organ", follow: "auto" } }],
  };
  const view = resolveTuningView({ kind: "stop", id: 1 }, snap);
  expect(view.following).toBe(true);
  expect(view.followValue).toBe("auto");
  expect(view.tuning).toBe(organTuning);
  expect(view.resolvedLine.primaryLabel).toBe("the instrument");
});

test("a stop automatically following its division reports the division's chain", () => {
  const divTuning = { idx: 0, temperament: "meantone4", reference: { key: 69, hz: 440 } };
  const snap = {
    tuning: organTuning,
    manuals: [{ idx: 0, name: "Great" }],
    manual_tuning: [divTuning],
    source_tuning: [],
    stops: [{ id: 1, midx: 0, tuning: { scope: "division", follow: "auto" } }],
  };
  const view = resolveTuningView({ kind: "stop", id: 1 }, snap);
  expect(view.following).toBe(true);
  expect(view.tuning).toBe(divTuning);
  expect(view.resolvedLine.primaryLabel).toBe("Great");
  expect(view.resolvedLine.link).toEqual({ kind: "division", idx: 0 });
  expect(view.resolvedLine.chainParts).toEqual([
    { prefix: "Sample set: no sample set" },
    { prefix: "Instrument: ", tuning: organTuning },
  ]);
});

test("a stop following a source names the source in the resolved line", () => {
  const srcTuning = { source: "demo", temperament: "pythagorean", reference: { key: 69, hz: 440 } };
  const snap = {
    tuning: organTuning,
    manuals: [{ idx: 0, name: "Great" }],
    manual_tuning: [],
    source_tuning: [srcTuning],
    stops: [{ id: 1, midx: 0, src: { from: "demo" }, tuning: { scope: "source", follow: "auto" } }],
  };
  const view = resolveTuningView({ kind: "stop", id: 1 }, snap);
  expect(view.tuning).toBe(srcTuning);
  expect(view.resolvedLine.primaryLabel).toEqual({ source: "demo" });
  expect(view.resolvedLine.link).toEqual({ kind: "source", alias: "demo" });
});

test("a stop pinned to its own tuning is never following, whatever else changed", () => {
  const own = { stop: 1, temperament: "equal", reference: { key: 69, hz: 442 } };
  const snap = {
    tuning: organTuning,
    manuals: [{ idx: 0, name: "Great" }],
    manual_tuning: [],
    source_tuning: [],
    stop_tuning: [own],
    stops: [{ id: 1, midx: 0, tuning: { scope: "stop", follow: "own" } }],
  };
  const view = resolveTuningView({ kind: "stop", id: 1 }, snap);
  expect(view.following).toBe(false);
  expect(view.followValue).toBe("own");
  expect(view.tuning).toBe(own);
});

test("a stop the snapshot no longer has closes the popover", () => {
  const snap = { tuning: organTuning, manuals: [], stops: [] };
  expect(resolveTuningView({ kind: "stop", id: 99 }, snap)).toEqual({ closeForm: true });
});

test("a rank without its own tuning falls back to its stop's effective tuning", () => {
  const snap = {
    tuning: organTuning,
    manuals: [{ idx: 0, name: "Great" }],
    manual_tuning: [],
    source_tuning: [],
    stops: [{ id: 1, midx: 0, tuning: { scope: "organ" }, ranks: [{ id: 0, own: false }] }],
  };
  const view = resolveTuningView({ kind: "rank", stop: 1, rank: 0 }, snap);
  expect(view.following).toBe(true);
  expect(view.tuning).toBe(organTuning);
  expect(view.resolvedLine).toEqual({
    primaryLabel: "this stop",
    link: { kind: "stop", id: 1 },
    chainParts: null,
  });
});

test("a rank the stop no longer has closes the popover", () => {
  const snap = { tuning: organTuning, stops: [{ id: 1, ranks: [] }] };
  expect(resolveTuningView({ kind: "rank", stop: 1, rank: 0 }, snap)).toEqual({ closeForm: true });
});

test("no scope or no snapshot yet resolves to nothing", () => {
  expect(resolveTuningView(null, { tuning: organTuning })).toBeNull();
  expect(resolveTuningView({ kind: "organ" }, null)).toBeNull();
});
