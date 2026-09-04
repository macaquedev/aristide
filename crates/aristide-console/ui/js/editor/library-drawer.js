// The library drawer: what each source offers, and what's pulled.

import { commands, localFetch } from "../api.js";
import { emptyNote } from "../wiring.js";

export function wireDrawer(editor) {
  editor.el.drawerTab.addEventListener("click", () => toggleDrawer(editor));
  editor.el.drawerClose.addEventListener("click", () => closeDrawer(editor));
}

function toggleDrawer(editor) {
  if (editor.drawerOpen) closeDrawer(editor);
  else openDrawer(editor);
}

// The tab is a pull, not a launcher: open, it rides the drawer's
// edge lit, and pressing it again puts the drawer away.
export function openDrawer(editor) {
  editor.drawerOpen = true;
  editor.el.drawer.classList.remove("hidden");
  editor.el.drawerTab.classList.add("on");
  editor.el.drawerTab.setAttribute("aria-expanded", "true");
  editor.el.drawerTab.setAttribute("aria-label", "Close the library drawer");
  fetchOfferings(editor);
}

export function closeDrawer(editor) {
  editor.drawerOpen = false;
  editor.el.drawer.classList.add("hidden");
  editor.el.drawerTab.classList.remove("on");
  editor.el.drawerTab.setAttribute("aria-expanded", "false");
  editor.el.drawerTab.setAttribute("aria-label", "Open the library drawer");
}

export async function fetchOfferings(editor, render = true) {
  const request = editor.offeringsRequest = (editor.offeringsRequest ?? 0) + 1;
  const file = editor.lastSnapshot?.setup?.file;
  const { ok, data } = await localFetch(editor.base, commands.organOfferings(), { json: true });
  if (request !== editor.offeringsRequest || file !== editor.lastSnapshot?.setup?.file) return;
  editor.offerings = ok ? (data.sources ?? []) : null;
  if (render) buildOfferings(editor, editor.offerings);
}

function buildOfferings(editor, sources) {
  const container = editor.el.offerings;
  container.replaceChildren();
  if (sources == null) {
    container.append(emptyNote("Couldn't read this organ's sources."));
    return;
  }
  if (!sources.length) {
    container.append(
      emptyNote("No sample sets yet. Choose + Add, then Sample set.")
    );
    return;
  }
  for (const source of sources) container.append(offeringSourceRow(editor, source));
}

function offeringSourceRow(editor, source) {
  const details = document.createElement("details");
  details.className = "organ-offerings-source";
  details.open = true;

  const summary = document.createElement("summary");
  const alias = document.createElement("span");
  alias.className = "organ-offerings-alias";
  alias.textContent = source.alias;
  const name = document.createElement("span");
  name.className = "organ-offerings-name";
  name.textContent = source.name ?? "(unreadable)";
  const path = document.createElement("span");
  path.className = "organ-offerings-path";
  path.textContent = source.path;
  path.title = source.path;
  summary.append(alias, name, path);

  // This set's own tuning, or "follows instrument" — a small mono
  // chip through to its tuning popover (the stop/division chips' own
  // idiom). A native <summary> toggles its <details> on any click, so
  // the chip has to swallow its own.
  const own = (editor.lastSnapshot?.source_tuning ?? []).find((t) => t.source === source.alias);
  const tuningChip = document.createElement("button");
  tuningChip.type = "button";
  tuningChip.className = "organ-offerings-tuning";
  // The drawer is narrow: the chip names the tuning, the tooltip
  // carries the anchor.
  tuningChip.textContent = own ? editor.tuningLabel(own) : "follows instrument";
  tuningChip.title = own ? editor.tuningSummary(own) : "Follows the instrument's tuning";
  tuningChip.addEventListener("click", (event) => {
    event.preventDefault();
    event.stopPropagation();
    const rect = tuningChip.getBoundingClientRect();
    editor.openTuningForm({ kind: "source", alias: source.alias }, rect.left, rect.bottom + 6);
  });
  summary.append(tuningChip);
  details.append(summary);

  if (source.error) {
    const error = document.createElement("p");
    error.className = "organ-offerings-error";
    error.textContent = source.error;
    details.append(error);
    return details;
  }

  const body = document.createElement("div");
  body.className = "organ-offerings-body";
  for (const manual of source.manuals ?? []) body.append(offeringDivision(editor, source.alias, manual));
  details.append(body);
  return details;
}

function offeringDivision(editor, alias, manual) {
  const div = document.createElement("div");
  div.className = "organ-offerings-division";

  const head = document.createElement("div");
  head.className = "organ-offerings-division-head";
  if (!manual.pulled) {
    editor.wireDragSource(head, () => ({
      kind: "offering-division",
      payload: { alias, manualName: manual.name },
      label: `${manual.name} (whole division)`,
    }));
  }
  const title = document.createElement("span");
  title.className = "organ-stop-group-title";
  title.textContent = manual.name;
  head.append(title);
  if (manual.pedal) {
    const tag = document.createElement("span");
    tag.className = "organ-manual-pedal-tag";
    tag.textContent = "pedal";
    head.append(tag);
  }
  if (manual.kind === "microtonal") {
    const tag = document.createElement("span");
    tag.className = "organ-manual-pedal-tag";
    tag.textContent = "microtonal";
    head.append(tag);
  }
  if (manual.pulled) {
    const tag = document.createElement("span");
    tag.className = "organ-manual-pedal-tag";
    tag.textContent = "pulled";
    head.append(tag);
  }
  div.append(head);

  for (const stop of manual.stops ?? []) div.append(offeringStop(editor, alias, manual.name, stop));
  return div;
}

function offeringStop(editor, alias, manualName, stop) {
  const row = document.createElement("div");
  row.className = "organ-offerings-stop";
  row.classList.toggle("pulled", !!stop.pulled);
  if (!stop.pulled) {
    editor.wireDragSource(row, () => ({
      kind: "offering-stop",
      payload: { alias, manualName, stopName: stop.name },
      label: stop.name,
    }));
  }
  const check = document.createElement("span");
  check.className = "organ-offerings-stop-check";
  check.textContent = stop.pulled ? "✓" : "";
  const name = document.createElement("span");
  name.textContent = stop.name;
  row.append(check, name);
  return row;
}
