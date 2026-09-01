// The tuning popover: this manual's own pitch, apart from the
// instrument's, applied live field by field — never a rebuild. A
// Scala scale (and its optional keymap) is just another field on the
// same /api/tuning contract; picking one supersedes the temperament.
//
// Every field goes through `tuningCommand` rather than the plain
// `send()` the rest of the console uses: a scale path can 400 (a bad
// file, an unparseable one), and that reason needs to land in this
// popover, not the app-wide status strip — see `showTuningError`.

import { commands, localFetch } from "../api.js";
import { renderIfChanged, setText } from "../dom.js";
import { keyName, keySpellings, parseKeyName, tidyKeyName } from "../pitch.js";
import { emptyNote, option } from "../wiring.js";

/// Every scientific-pitch temperament `snapshot.home` can *identify* by
/// name — `null` there means an unequal temperament that doesn't match
/// any of these tables, not an absence of measurement (that's `home`
/// itself null).
const HOME_TEMPERAMENT_NAMES = {
  equal: "equal",
  werckmeister3: "Werckmeister III",
  kirnberger3: "Kirnberger III",
  meantone4: "¼-comma meantone",
  pythagorean: "Pythagorean",
};

/// The pitch a sample set was actually recorded at for MIDI key `key`,
/// per the snapshot's `home` block: `home.a4_hz` is the measured A4,
/// `home.offsets_cents` the other eleven pitch classes' deviation from
/// equal temperament under that A4 (C = index 0). Used only to decide
/// whether the "as recorded" reference button has anything to do —
/// the popover otherwise just echoes `home.a4_hz` verbatim.
function homeHz(home, key) {
  const equalCents = 1200 * Math.log2(home.a4_hz / 440);
  const classCents = home.offsets_cents[key % 12];
  return 440 * 2 ** ((key - 69) / 12 + (equalCents + classCents) / 1200);
}

/// What a stop is actually playing right now, cutting straight to the
/// concrete tuning object regardless of how it got there — a pure
/// function of the snapshot, exported for stopTuningLine's cascade.
function stopEffectiveTuning(snap, stop) {
  const info = stop?.tuning ?? { scope: "organ" };
  if (info.scope === "stop") {
    return (snap.stop_tuning ?? []).find((t) => t.stop === stop.id) ?? snap.tuning;
  }
  if (info.scope === "division") {
    return (snap.manual_tuning ?? []).find((t) => t.idx === stop.midx) ?? snap.tuning;
  }
  if (info.scope === "source") {
    return (snap.source_tuning ?? []).find((t) => t.source === stop.src?.from) ?? snap.tuning;
  }
  return snap.tuning;
}

/// The tuning cascade's resolution, as a pure function of the popover's
/// scope and the snapshot: which tuning object governs, whether it's
/// this scope's own or borrowed, and everything the paint step
/// (syncTuningForm) needs to fill the fields and the "Plays X's
/// tuning…" resolved line. Deliberately stops short of formatting text
/// that needs live state syncTuningForm has and this doesn't — a
/// source's display name (the offerings cache) and a temperament's
/// friendly name (the <select>'s own <option> text) — those come back
/// as `{source: alias}` tags or a plain string the paint step (or a
/// caller like stopTuningLine) resolves the rest of the way.
///
/// Returns null for no scope/snapshot yet (nothing to show), or
/// `{closeForm: true}` for a scope naming something the snapshot no
/// longer has (a removed division, stop, or rank) — sync's cue to
/// close the popover, since a later poll can find the same thing gone.
export function resolveTuningView(scope, snap) {
  if (!scope || !snap) return null;

  if (scope.kind === "organ") {
    return {
      closeForm: false,
      title: "Whole instrument",
      showFollowRow: false,
      showTransposeRow: true,
      followOptions: null,
      followValue: null,
      tuning: snap.tuning,
      following: false,
      resolvedLine: null,
    };
  }

  if (scope.kind === "division") {
    const manual = snap.manuals.find((m) => m.idx === scope.idx);
    if (!manual) return { closeForm: true };
    const own = (snap.manual_tuning ?? []).find((t) => t.idx === scope.idx);
    const following = !own;
    return {
      closeForm: false,
      title: manual.name,
      showFollowRow: true,
      showTransposeRow: true,
      followOptions: [["organ", "Whole instrument"], ["own", "Own tuning"]],
      followValue: own ? "own" : "organ",
      tuning: own ?? snap.tuning,
      following,
      resolvedLine: following
        ? { primaryLabel: "the instrument", link: { kind: "organ" }, chainParts: null }
        : null,
    };
  }

  if (scope.kind === "source") {
    // No existence check against the offerings list: it may not have
    // loaded yet (openTuningForm kicks that fetch off but doesn't wait
    // on it), and the alias is otherwise all this scope needs — the
    // display name just falls back to the bare alias until it lands.
    const own = (snap.source_tuning ?? []).find((t) => t.source === scope.alias);
    const following = !own;
    return {
      closeForm: false,
      title: { source: scope.alias, suffix: " · sample set" },
      showFollowRow: true,
      showTransposeRow: false,
      followOptions: [["organ", "Whole instrument"], ["own", "Own tuning"]],
      followValue: own ? "own" : "organ",
      tuning: own ?? snap.tuning,
      following,
      resolvedLine: following
        ? { primaryLabel: "the instrument", link: { kind: "organ" }, chainParts: null }
        : null,
    };
  }

  if (scope.kind === "stop") {
    const stop = snap.stops.find((s) => s.id === scope.id);
    if (!stop) return { closeForm: true };
    const manual = snap.manuals.find((m) => m.idx === stop.midx);
    const info = stop.tuning ?? { scope: "organ", follow: "auto" };
    const divOwn = (snap.manual_tuning ?? []).find((t) => t.idx === stop.midx);
    const srcAlias = stop.src?.from;
    const srcOwn = srcAlias ? (snap.source_tuning ?? []).find((t) => t.source === srcAlias) : null;
    const autoLabel = divOwn ? "division" : srcOwn ? "sample set" : "instrument";
    const followOptions = [
      ["auto", `Automatic (→ ${autoLabel})`],
      ["division", `Division · ${manual?.name ?? "?"}`],
      ["source", srcAlias ? { source: srcAlias, prefix: "Sample set · " } : "Sample set · ?"],
      ["organ", "Whole instrument"],
      ["own", "Own tuning"],
    ];

    let tuning;
    let following = false;
    let resolvedLine = null;
    if (info.scope === "stop") {
      tuning = (snap.stop_tuning ?? []).find((t) => t.stop === stop.id) ?? snap.tuning;
    } else {
      following = true;
      if (info.scope === "division") {
        tuning = divOwn ?? snap.tuning;
        const sourceStatus = srcAlias ? (srcOwn ? "own tuning" : "follows instrument") : "no sample set";
        resolvedLine = {
          primaryLabel: manual?.name ?? "this division",
          link: { kind: "division", idx: stop.midx },
          chainParts: [
            { prefix: `Sample set: ${sourceStatus}` },
            { prefix: "Instrument: ", tuning: snap.tuning },
          ],
        };
      } else if (info.scope === "source") {
        tuning = srcOwn ?? snap.tuning;
        resolvedLine = {
          primaryLabel: { source: srcAlias },
          link: { kind: "source", alias: srcAlias },
          chainParts: [{ prefix: "Instrument: ", tuning: snap.tuning }],
        };
      } else {
        tuning = snap.tuning;
        resolvedLine = { primaryLabel: "the instrument", link: { kind: "organ" }, chainParts: null };
      }
    }

    return {
      closeForm: false,
      title: `${stop.name} · ${manual?.name ?? ""}`,
      showFollowRow: true,
      showTransposeRow: false,
      followOptions,
      followValue: info.follow ?? "auto",
      tuning,
      following,
      resolvedLine,
    };
  }

  if (scope.kind === "rank") {
    const stop = snap.stops.find((s) => s.id === scope.stop);
    const rankInfo = stop?.ranks?.find((r) => r.id === scope.rank);
    if (!stop || !rankInfo) return { closeForm: true };
    let tuning;
    let following = false;
    let resolvedLine = null;
    if (rankInfo.own) {
      tuning = (snap.rank_tuning ?? []).find((t) => t.stop === scope.stop && t.rank === scope.rank);
    } else {
      following = true;
      tuning = stopEffectiveTuning(snap, stop);
      resolvedLine = { primaryLabel: "this stop", link: { kind: "stop", id: scope.stop }, chainParts: null };
    }
    return {
      closeForm: false,
      title: `${rankInfo.name} · ${stop.name}`,
      showFollowRow: true,
      showTransposeRow: false,
      followOptions: [["stop", "This stop"], ["own", "Own tuning"]],
      followValue: rankInfo.own ? "own" : "stop",
      tuning,
      following,
      resolvedLine,
    };
  }

  return null;
}

export function wireTuningForm(editor) {
  // Every MIDI note for the reference-key field's autocomplete —
  // built here rather than hand-written into index.html, in the
  // ASCII spellings a player can type, black keys under both names.
  for (let key = 0; key <= 127; key++) {
    for (const spelling of keySpellings(key)) {
      // A bare value, no label: the datalist offers it as typed text,
      // not the value/label pair wiring.js's option() builds.
      const entry = document.createElement("option");
      entry.value = spelling;
      editor.el.pitchNames.append(entry);
    }
  }

  editor.el.tuningClose.addEventListener("click", () => editor.closeTuningForm());

  // The Follows select: what a division/source/rank falls back to is
  // binary (the parent, or its own), a stop's is the five-way
  // auto/division/source/organ/own vocabulary the server's `follow`
  // param speaks directly — so every kind maps straight onto one
  // /api/tuning call, never a client-side branch on what "own" means.
  editor.el.tuningFollow.addEventListener("change", () => {
    const scope = editor.tuningScope;
    if (!scope) return;
    const value = editor.el.tuningFollow.value;
    if (scope.kind === "stop") {
      tuningCommand(editor, tuningFields(editor, { follow: value }));
    } else if (value === "own") {
      tuningCommand(editor, tuningFields(editor, { follow: "own" }));
    } else {
      tuningCommand(editor, tuningFields(editor, { reset: 1 }));
    }
    editor.el.tuningFollow.blur();
  });

  editor.el.tuningTemperament.addEventListener("change", () => {
    if (!editor.tuningScope) return;
    // Naming a temperament here is allowed even with a scale active —
    // the server reads it as leaving the scale (http.rs's /api/tuning
    // arm clears `tuning.scale` whenever `temperament` is given).
    tuningCommand(editor, tuningFields(editor, { temperament: editor.el.tuningTemperament.value }));
    editor.el.tuningTemperament.blur();
  });

  editor.el.tuningPipes.addEventListener("change", () => {
    if (!editor.tuningScope) return;
    tuningCommand(editor, tuningFields(editor, { pipes: editor.el.tuningPipes.value }));
    editor.el.tuningPipes.blur();
  });

  editor.el.tuningEdo.addEventListener("change", () => {
    if (!editor.tuningScope) return;
    const edo = Math.min(311, Math.max(1, Math.round(Number(editor.el.tuningEdo.value) || 12)));
    editor.el.tuningEdo.value = edo;
    // Like naming a temperament, choosing a division count leaves
    // any active scale (the server clears it on this field).
    tuningCommand(editor, tuningFields(editor, { edo }));
  });

  // The pitch anchor is a key/Hz *pair* — "a′" only names a key in
  // 12-EDO — so either field changing re-sends both: the server keeps
  // whichever one didn't move.
  editor.el.tuningRefKey.addEventListener("change", () => {
    if (!editor.tuningScope) return;
    const key = parseKeyName(editor.el.tuningRefKey.value);
    if (key == null) {
      showTuningError(editor, `"${editor.el.tuningRefKey.value}" doesn't name a key`);
      editor.el.tuningRefKey.blur();
      editor.syncTuningForm(); // restores the last known-good spelling
      return;
    }
    // The player's own spelling stays on screen: D#4 typed is D#4
    // shown, not the E♭4 the canonical printer would pick. The
    // server only ever sees the key number's canonical name.
    editor.tuningRefKeySpelling = { key, text: tidyKeyName(editor.el.tuningRefKey.value) };
    editor.el.tuningRefKey.value = editor.tuningRefKeySpelling.text;
    const hz = Number(editor.el.tuningRefHz.value);
    tuningCommand(editor, tuningFields(editor, { reference_key: keyName(key), reference_hz: hz }));
    editor.el.tuningRefKey.blur();
  });

  editor.el.tuningRefHz.addEventListener("change", () => {
    if (!editor.tuningScope) return;
    const hz = Number(editor.el.tuningRefHz.value);
    // No hard range here — the server clamps so the implied shift
    // stays within a′ 300–500 Hz equivalents and the next snapshot
    // reflects the clamped value; a bad number just reverts.
    if (!Number.isFinite(hz) || hz <= 0) {
      editor.el.tuningRefHz.blur();
      editor.syncTuningForm();
      return;
    }
    const key = parseKeyName(editor.el.tuningRefKey.value);
    tuningCommand(editor, tuningFields(editor, {
      reference_key: key != null ? keyName(key) : editor.el.tuningRefKey.value,
      reference_hz: hz,
    }));
    editor.el.tuningRefHz.blur();
  });

  // "As recorded": put the reference back on whatever the sample set
  // itself sounds on the current reference key — the server reads
  // `reference_hz=home` as that instruction rather than a literal Hz
  // number (see /api/tuning). Only shown when it would move anything;
  // see the visibility check in syncTuningForm.
  editor.el.tuningRefHome.addEventListener("click", () => {
    if (!editor.tuningScope) return;
    tuningCommand(editor, tuningFields(editor, { reference_hz: "home" }));
  });

  editor.el.tuningTranspose.addEventListener("change", () => {
    if (!editor.tuningScope) return;
    const transpose = Math.min(12, Math.max(-12, Math.round(Number(editor.el.tuningTranspose.value) || 0)));
    editor.el.tuningTranspose.value = transpose;
    tuningCommand(editor, tuningFields(editor, { transpose }));
    editor.el.tuningTranspose.blur();
  });

  editor.el.tuningScalePick.addEventListener("click", () => openTuningBrowse(editor, "scale"));
  editor.el.tuningScaleClear.addEventListener("click", () => {
    if (!editor.tuningScope) return;
    tuningCommand(editor, tuningFields(editor, { scale: "off" }));
  });

  editor.el.tuningKeymapPick.addEventListener("click", () => openTuningBrowse(editor, "keymap"));
  editor.el.tuningKeymapClear.addEventListener("click", () => {
    if (!editor.tuningScope) return;
    const scl = currentScalePath(editor);
    if (!scl) return;
    // An empty `keymap` param is indistinguishable, server-side, from
    // an omitted one (http.rs filters both to "no keymap") — sending
    // it explicitly just documents the intent here.
    tuningCommand(editor, tuningFields(editor, { scale: scl, keymap: "" }));
  });

  editor.el.tuningBrowseUp.addEventListener("click", () => {
    if (editor.tuningBrowseParent) tuningBrowse(editor, editor.tuningBrowseParent);
  });
  editor.el.tuningBrowseCancel.addEventListener("click", () => closeTuningBrowse(editor));
}

/// `scope` is one of {kind:"organ"} | {kind:"division", idx} |
/// {kind:"source", alias} | {kind:"stop", id} | {kind:"rank", stop, rank}
/// — see the class-level comment by `editor.tuningScope`. The bare
/// string "organ" (main.js's own two call sites, predating the
/// scope object) is still accepted as shorthand for {kind:"organ"}.
export function openTuningForm(editor, scope, x, y) {
  if (scope === "organ") scope = { kind: "organ" };
  editor.openingPopover("tuning");
  editor.tuningScope = scope;
  hideTuningError(editor);
  closeTuningBrowse(editor);
  editor.syncTuningForm();
  // A bad scope (a stop/division/rank the snapshot doesn't have)
  // closes right back — sync's own job, since a later poll can find
  // the same thing gone. Nothing left to show.
  if (editor.tuningScope == null) return;
  // A source or stop's popover names the governing sample set by its
  // offerings entry (display name, not just the bare alias) — fetch
  // once, quietly, if the drawer never has been opened this session.
  if ((scope.kind === "source" || scope.kind === "stop") && editor.offerings == null) {
    editor.fetchOfferings(false).then(() => editor.syncTuningForm());
  }
  editor.el.tuning.classList.remove("hidden");
  editor.positionPopover(editor.el.tuning, x, y);
}

export function closeTuningForm(editor) {
  editor.tuningScope = null;
  editor.tuningResolved = null;
  editor.el.tuning.classList.add("hidden");
  hideTuningError(editor);
  closeTuningBrowse(editor);
}

/// Replaces the Follows select's own options — each scope kind offers
/// a different vocabulary (division/source: instrument or own; stop:
/// the full auto/division/source/organ/own cascade; rank: the stop or
/// own). `pairs` is `[value, label]`.
function setFollowOptions(editor, pairs) {
  renderIfChanged(editor.el.tuningFollow, JSON.stringify(pairs), () => {
    editor.el.tuningFollow.replaceChildren(...pairs.map(([value, label]) => option(value, label)));
  });
}

/// The short label a tuning reads by — a scale's name, an EDO count,
/// or the temperament select's own friendly text for the value (so
/// renaming an option, like "original" → "As recorded", only has to
/// happen in index.html).
export function tuningLabel(editor, tuning) {
  if (!tuning) return "";
  if (tuning.scale) return `${tuning.scale.name} (${tuning.scale.notes} notes)`;
  const edo = tuning.edo ?? 12;
  if (edo !== 12) return `${edo}-EDO`;
  const opt = editor.el.tuningTemperament.querySelector(`option[value="${tuning.temperament}"]`);
  return opt ? opt.textContent : tuning.temperament;
}

/// The resolved line's "…: <summary>" half — the label above plus
/// where it's anchored, e.g. "¼-comma meantone · A4 = 415.3 Hz".
export function tuningSummary(editor, tuning) {
  if (!tuning?.reference) return "";
  const hz = tuning.reference.hz.toFixed(1).replace(/\.0$/, "");
  return `${tuningLabel(editor, tuning)} · ${keyName(tuning.reference.key)} = ${hz} Hz`;
}

/// A source's display name from the cached offerings list, falling
/// back to its bare alias when the offerings haven't loaded yet.
export function sourceDisplayName(editor, alias) {
  return editor.offerings?.find((s) => s.alias === alias)?.name ?? alias ?? "?";
}

/// The stop editor's compact "Tuning" line — also the accent dot's
/// title attribute on the console (refreshTuningChips): "Automatic →
/// Récit · 19-EDO", "Pinned · sample set", "Own · Pythagorean".
export function stopTuningLine(editor, stop) {
  const snap = editor.lastSnapshot;
  const info = stop.tuning ?? { scope: "organ", follow: "auto" };
  const manual = snap.manuals.find((m) => m.idx === stop.midx);
  if (info.follow === "own") {
    const own = (snap.stop_tuning ?? []).find((t) => t.stop === stop.id);
    return `Own · ${tuningLabel(editor, own)}`;
  }
  if (info.follow && info.follow !== "auto") {
    const target = { division: manual?.name ?? "division", source: "sample set", organ: "instrument" }[info.follow];
    return `Pinned · ${target}`;
  }
  const target =
    info.scope === "division" ? manual?.name ?? "division" : info.scope === "source" ? "sample set" : "instrument";
  return `Automatic → ${target} · ${tuningLabel(editor, stopEffectiveTuning(snap, stop))}`;
}

/// A resolveTuningView label — a plain string, or a `{source: alias}`
/// tag naming a source whose display name needs the offerings cache.
function formatLabel(editor, label) {
  if (typeof label === "string") return label;
  return `${label.prefix ?? ""}${sourceDisplayName(editor, label.source)}${label.suffix ?? ""}`;
}

/// Fills `#editor-tuning-resolved-primary`/`-chain` with "Plays X's
/// tuning: <summary> — open →" and dims the fields below to match —
/// the common tail every following (non-own) scope shares. `link`
/// jumps this same popover to the scope that actually governs.
function setResolvedLines(editor, primaryLabel, link, tuning, chainText) {
  const primary = editor.el.tuningResolvedPrimary;
  const lead = `Plays ${primaryLabel}'s tuning: ${tuningSummary(editor, tuning)} — `;
  renderIfChanged(primary, JSON.stringify([lead, link]), () => {
    primary.replaceChildren(lead);
    const open = document.createElement("button");
    open.type = "button";
    open.className = "tuning-open-link";
    open.textContent = "open →";
    open.addEventListener("click", () => {
      const rect = editor.el.tuning.getBoundingClientRect();
      editor.openTuningForm(link, rect.left, rect.top);
    });
    primary.append(open);
  });
  primary.classList.remove("hidden");
  setText(editor.el.tuningResolvedChain, chainText ?? "");
  editor.el.tuningResolvedChain.classList.toggle("hidden", !chainText);
}

/// Dims (and disables) the spec fields while a scope is following
/// someone else's tuning — item 3 of the tuning-cascade UI: the
/// values still read as what's actually playing, but nothing here is
/// live until "Own tuning" seeds a copy to edit.
function setTuningFollowing(editor, following) {
  for (const row of [
    editor.el.tuningScaleRow, editor.el.tuningKeymapRow, editor.el.tuningEdoRow,
    editor.el.tuningTemperamentRow, editor.el.tuningPipesRow, editor.el.tuningRefRow,
    editor.el.tuningTransposeRow,
  ]) {
    row.classList.toggle("tuning-following", following);
  }
  for (const field of [
    editor.el.tuningScalePick, editor.el.tuningScaleClear, editor.el.tuningKeymapPick, editor.el.tuningKeymapClear,
    editor.el.tuningEdo, editor.el.tuningTemperament, editor.el.tuningPipes,
    editor.el.tuningRefKey, editor.el.tuningRefHz, editor.el.tuningRefHome, editor.el.tuningTranspose,
  ]) {
    field.disabled = following;
  }
}

/// Every scope kind's tuning popover: what governs it right now (its
/// own tuning, or someone else's — organ/division/source/stop cascade
/// down to instrument), reflected into the Follows select, the
/// resolved-line prose, and the spec fields below (which show the
/// governing values, live and editable only when it's this scope's
/// own). Called on open and on every later poll, so a shared value
/// another panel (or session) changes keeps the popover honest. Never
/// touches the file-browser sub-view (see `openTuningBrowse`/
/// `closeTuningBrowse`) — a poll landing mid-navigation must not yank
/// it shut. The cascade resolution itself is resolveTuningView, above
/// — this is the paint step, filling in text and field values.
export function syncTuningForm(editor) {
  const view = resolveTuningView(editor.tuningScope, editor.lastSnapshot);
  if (!view) return;
  if (view.closeForm) {
    editor.closeTuningForm();
    return;
  }

  editor.el.tuningFollowRow.classList.toggle("hidden", !view.showFollowRow);
  editor.el.tuningTransposeRow.classList.toggle("hidden", !view.showTransposeRow);
  setText(editor.el.tuningTitle, formatLabel(editor, view.title));
  if (view.followOptions) {
    setFollowOptions(editor, view.followOptions.map(([value, label]) => [value, formatLabel(editor, label)]));
    if (editor.root.activeElement !== editor.el.tuningFollow) editor.el.tuningFollow.value = view.followValue;
  }
  if (view.resolvedLine) {
    const chainText = view.resolvedLine.chainParts
      ? view.resolvedLine.chainParts
          .map((part) => part.prefix + (part.tuning ? tuningSummary(editor, part.tuning) : ""))
          .join(" · ")
      : undefined;
    setResolvedLines(
      editor,
      formatLabel(editor, view.resolvedLine.primaryLabel),
      view.resolvedLine.link,
      view.tuning,
      chainText
    );
  }

  const tuning = view.tuning;
  if (!tuning) return;
  const snap = editor.lastSnapshot;
  const scope = editor.tuningScope;
  editor.tuningResolved = tuning;
  editor.el.tuningResolvedPrimary.classList.toggle("hidden", !view.following);
  if (!view.following) editor.el.tuningResolvedChain.classList.add("hidden");
  setTuningFollowing(editor, view.following);

  // Temperaments are twelve-class vocabulary: the row shows only
  // while the division count is 12 (absent on an old snapshot = 12).
  const edo = tuning.edo ?? 12;
  editor.el.tuningTemperamentRow.classList.toggle("hidden", edo !== 12);
  if (editor.root.activeElement !== editor.el.tuningTemperament) {
    editor.el.tuningTemperament.value = tuning.temperament;
  }
  if (editor.root.activeElement !== editor.el.tuningEdo) editor.el.tuningEdo.value = edo;
  if (editor.root.activeElement !== editor.el.tuningRefKey) {
    const spelling = editor.tuningRefKeySpelling;
    editor.el.tuningRefKey.value =
      spelling?.key === tuning.reference.key ? spelling.text : keyName(tuning.reference.key);
  }
  if (editor.root.activeElement !== editor.el.tuningRefHz) editor.el.tuningRefHz.value = tuning.reference.hz;
  if (editor.root.activeElement !== editor.el.tuningTranspose) editor.el.tuningTranspose.value = tuning.transpose ?? 0;

  // "Recorded: …" — what the sample set itself sounds, measured at
  // load time. Lives on the snapshot's top level (`home`) for the
  // instrument as a whole; at set scope, `source_home` swaps in that
  // set's own recorded A4 alongside the instrument-wide temperament
  // and spread (every division of one set was recorded together).
  let home = editor.lastSnapshot?.home ?? null;
  if (home && scope.kind === "source") {
    const setHome = (snap.source_home ?? []).find((h) => h.source === scope.alias);
    if (setHome) home = { ...home, a4_hz: setHome.a4_hz };
  }
  if (!home) {
    setText(editor.el.tuningHome, "Recorded: not measured (assuming A4 = 440 equal)");
  } else {
    const name = HOME_TEMPERAMENT_NAMES[home.temperament] ?? "unequal (unnamed)";
    const mixed = home.spread_cents > 8 ? " · mixed pitch standards?" : "";
    setText(
      editor.el.tuningHome,
      `Recorded: A4 = ${home.a4_hz.toFixed(1).replace(/\.0$/, "")} Hz · ${name} · ` +
        `±${home.spread_cents.toFixed(1).replace(/\.0$/, "")} ¢ · ${home.measured} of ${home.pipes} pipes${mixed}`
    );
  }
  // The ↺ button next to Reference: only worth showing when it would
  // actually move something — when the reference isn't already
  // sitting on what the recording itself sounds on that key. A
  // button that silently does nothing on click is worse than none.
  // `.wrap` rides along on the same condition: the row has no spare
  // width for a fourth thing, so the button (and "Hz" alongside it)
  // only wrap to their own line while there's a button to make room
  // for — the row's ordinary layout is untouched the rest of the time.
  const showRefHome =
    !view.following && home != null && Math.abs(tuning.reference.hz - homeHz(home, tuning.reference.key)) > 0.05;
  editor.el.tuningRefHome.classList.toggle("hidden", !showRefHome);
  editor.el.tuningRefStepper.classList.toggle("wrap", showRefHome);

  const scale = tuning.scale ?? null;
  editor.el.tuningScalePick.classList.toggle("hidden", !!scale);
  editor.el.tuningScaleActive.classList.toggle("hidden", !scale);
  if (scale) {
    setText(editor.el.tuningScaleName, `${scale.name} · ${scale.notes} notes`);
    editor.el.tuningScaleName.title = scale.scl;
  }

  editor.el.tuningKeymapRow.classList.toggle("hidden", !scale);
  if (scale) {
    if (scale.kbm) {
      editor.el.tuningKeymapName.textContent = scale.kbm.split("/").pop();
      editor.el.tuningKeymapName.title = scale.kbm;
      editor.el.tuningKeymapClear.classList.remove("hidden");
    } else {
      editor.el.tuningKeymapName.textContent = "linear";
      editor.el.tuningKeymapName.title = "";
      editor.el.tuningKeymapClear.classList.add("hidden");
    }
  }

  // The scale IS the tuning while one is active — the temperament
  // select and the division count stay live (setting either is a
  // valid way back out) but read as superseded rather than in
  // effect.
  editor.el.tuningTemperamentRow.classList.toggle("tuning-dimmed", !!scale);
  editor.el.tuningEdoRow.classList.toggle("tuning-dimmed", !!scale);
  editor.el.tuningTemperament.title = scale
    ? "A scale is active — picking a temperament here leaves it"
    : "";
  editor.el.tuningEdo.title = scale
    ? "A scale is active — setting a division count here leaves it"
    : "";

  // Pipes only mean something under a target: as recorded, every
  // pipe is exactly where it is, so the row stays visible (the
  // choice persists into the next target) but reads dimmed.
  if (editor.root.activeElement !== editor.el.tuningPipes) {
    editor.el.tuningPipes.value = tuning.pipes ?? "original";
  }
  const asRecorded = tuning.temperament === "original" && edo === 12 && !scale;
  editor.el.tuningPipesRow.classList.toggle("tuning-dimmed", asRecorded);
  editor.el.tuningPipes.title = asRecorded
    ? "As recorded, pipes are exactly where they are — this applies under a target tuning"
    : "";
}

/// The scope's effective scale path right now, or null with none —
/// what a keymap pick or clear re-sends alongside, since /api/tuning
/// takes the scale and its keymap together (see http.rs). Reads the
/// same resolved tuning the fields above are already showing, so it
/// agrees with them even while following (fields are disabled then,
/// but the browse-cancel/clear paths still call in).
export function currentScalePath(editor) {
  return editor.tuningResolved?.scale?.scl ?? null;
}

/// The tuning popover's target as /api/tuning fields — the scope
/// selector each kind speaks (see the endpoint contract): none for
/// the instrument, `manual=` for a division, `source=` for a set,
/// `stop=` for a stop, `stop=`+`rank=` for one of its ranks.
export function tuningFields(editor, extra) {
  const scope = editor.tuningScope;
  if (!scope || scope.kind === "organ") return extra;
  if (scope.kind === "division") return { manual: scope.idx, ...extra };
  if (scope.kind === "source") return { source: scope.alias, ...extra };
  if (scope.kind === "stop") return { stop: scope.id, ...extra };
  if (scope.kind === "rank") return { stop: scope.stop, rank: scope.rank, ...extra };
  return extra;
}

/// Sends a tuning field update directly (not through the app-wide
/// `send()`), so a 400's reason can land in this popover instead of
/// the global status strip — the same local-fetch idiom
/// `organCommandResult` uses for structural edits.
export async function tuningCommand(editor, fields) {
  hideTuningError(editor);
  const query = commands.tuning(fields);
  const { ok, status, error } = await localFetch(editor.base, query, { method: "POST" });
  if (!ok) {
    if (editor.deferToSaveAs(status, query)) return false;
    showTuningError(editor, error);
    return false;
  }
  return true;
}

function showTuningError(editor, text) {
  editor.el.tuningError.textContent = text;
  editor.el.tuningError.classList.remove("hidden");
}

function hideTuningError(editor) {
  editor.el.tuningError.classList.add("hidden");
  editor.el.tuningError.textContent = "";
}

// ---- the tuning popover's own file browser: picks a .scl or .kbm --------
// path, the same /api/browse idiom as the add-source browse, filtered
// client-side to the relevant extension (directories stay navigable).

export function openTuningBrowse(editor, kind) {
  editor.tuningBrowseKind = kind;
  editor.tuningBrowseDir = null;
  editor.tuningBrowseParent = null;
  editor.tuningBrowseEntries = null;
  editor.tuningBrowseError = null;
  editor.el.tuningBrowseTitle.textContent = kind === "keymap" ? "Choose a keymap" : "Choose a scale";
  editor.el.tuningForm.classList.add("hidden");
  editor.el.tuningBrowse.classList.remove("hidden");
  tuningBrowse(editor);
}

export function closeTuningBrowse(editor) {
  editor.tuningBrowseKind = null;
  editor.el.tuningBrowse.classList.add("hidden");
  editor.el.tuningForm.classList.remove("hidden");
}

export async function tuningBrowse(editor, dir) {
  const query = dir ? `/api/browse?dir=${encodeURIComponent(dir)}` : "/api/browse";
  const { ok, data, error } = await localFetch(editor.base, query, { json: true });
  if (!ok) {
    editor.tuningBrowseError = error;
  } else {
    editor.tuningBrowseDir = data.dir;
    editor.tuningBrowseParent = data.parent;
    editor.tuningBrowseEntries = data.entries;
    editor.tuningBrowseError = null;
  }
  renderTuningBrowse(editor);
}

function renderTuningBrowse(editor) {
  editor.el.tuningBrowseDir.textContent = editor.tuningBrowseDir ?? "";
  editor.el.tuningBrowseDir.title = editor.tuningBrowseDir ?? "";
  editor.el.tuningBrowseUp.disabled = !editor.tuningBrowseParent;
  editor.el.tuningBrowseError.classList.toggle("hidden", !editor.tuningBrowseError);
  editor.el.tuningBrowseError.textContent = editor.tuningBrowseError ?? "";
  editor.el.tuningBrowseList.replaceChildren();
  if (editor.tuningBrowseError) return;
  const ext = editor.tuningBrowseKind === "keymap" ? ".kbm" : ".scl";
  const entries = (editor.tuningBrowseEntries ?? []).filter(
    (entry) => entry.dir || entry.name.toLowerCase().endsWith(ext)
  );
  if (!entries.length) {
    editor.el.tuningBrowseList.append(emptyNote("Nothing here."));
    return;
  }
  for (const entry of entries) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = entry.dir ? "picker-row picker-browse-dir" : "picker-row";
    row.title = entry.path;
    row.addEventListener("click", () => {
      if (entry.dir) tuningBrowse(editor, entry.path);
      else pickTuningFile(editor, entry.path);
    });
    const name = document.createElement("span");
    name.className = "picker-row-name";
    name.textContent = entry.name;
    row.append(name);
    editor.el.tuningBrowseList.append(row);
  }
}

async function pickTuningFile(editor, path) {
  if (!editor.tuningScope) return;
  const fields =
    editor.tuningBrowseKind === "keymap"
      ? tuningFields(editor, { scale: currentScalePath(editor), keymap: path })
      : tuningFields(editor, { scale: path });
  if (fields.scale == null) return;
  const ok = await tuningCommand(editor, fields);
  if (ok) closeTuningBrowse(editor);
}
