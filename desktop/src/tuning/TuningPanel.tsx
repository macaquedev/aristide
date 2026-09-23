import { useCallback, useEffect, useRef, useState } from 'react';
import { Button, Loader, Modal, SegmentedControl, Stack, Text } from '@mantine/core';
import { editInstrument, request } from '../api';
import { TuningDesk, type DeskScope, type Part } from './TuningDesk';
import { CUSTOM, RECORDED, type Anchor, type Shape } from './model';
import { ScalaSheet } from './ScalaSheet';

type TuningView = {
  temperament: string; edo: number; reference: { key: number; hz: number }; root: number; offsets?: number[]; offset_cents: number;
  system: 'temperament' | 'equal' | 'steps' | 'scale'; period: number | null; steps?: number[]; start_key?: number;
  scale?: { scl: string; kbm: string | null; name: string };
};
type Own = { anchor: boolean; scale: boolean };
type Scopes = {
  instrument: TuningView;
  manuals: { idx: number; name: string; own: Own; tuning: TuningView }[];
  stops: { id: number; name: string; midx: number; own: Own; tuning: TuningView; ranks: { id: number; name: string; own: Own; tuning: TuningView }[] }[];
};
type Params = Record<string, string | number>;

const toAnchor = (view: TuningView): Anchor => ({ key: view.reference.key, hz: view.reference.hz, offset: view.offset_cents });
function toShape(view: TuningView): Shape {
  if (view.system === 'equal') return { system: 'equal', steps: view.edo, period: view.period ?? 1200 };
  if (view.system === 'steps') return { system: 'steps', intervals: view.steps ?? [0], period: view.period, startKey: view.start_key ?? 60 };
  if (view.system === 'scale') return { system: 'scale', name: view.scale?.name ?? 'Scala', scl: view.scale?.scl ?? '', kbm: view.scale?.kbm ?? null,
    intervals: view.steps ?? [0], period: view.period ?? 1200, startKey: view.start_key ?? 60 };
  return { system: 'temperament', temperament: view.temperament, root: view.root, offsets: view.offsets ?? Array(12).fill(0) };
}

function scopeParams(id: string): Params {
  const [kind, stop, rank] = id.split(':');
  return kind === 'manual' ? { manual: stop } : kind === 'stop' ? { stop } : kind === 'rank' ? { stop, rank } : {};
}
const anchorParams = (anchor: Anchor): Params => ({ reference_key: anchor.key, reference_hz: anchor.hz, offset_cents: anchor.offset });
const cents = (values: number[]) => values.map(v => Number(v.toFixed(3))).join(',');
function shapeParams(shape: Shape): Params {
  if (shape.system === 'temperament') {
    if (shape.temperament === CUSTOM) return { temperament: CUSTOM, offsets: cents(shape.offsets) };
    if (shape.temperament === RECORDED) return { temperament: RECORDED };
    return { temperament: shape.temperament, root: shape.root };
  }
  if (shape.system === 'equal') return { edo: shape.steps, period: shape.period };
  if (shape.system === 'steps') return { steps: cents(shape.intervals), period: shape.period ?? 'none', start_key: shape.startKey };
  return { scale: shape.scl, ...(shape.kbm ? { keymap: shape.kbm } : { start_key: shape.startKey }) };
}

/** Tuning on the live engine: every edit sounds and autosaves. The Tuning tab edits the
 * tuning stops share (instrument, divisions); given `stop`, it edits that stop's own
 * tuning and its ranks' inside the stop editor, and `openScope` leads to what it follows. */
export function TuningPanel({ organ, offerUndo, scope, stop, openScope }: {
  organ: string; offerUndo: (undo?: () => void) => void; scope?: string; stop?: number; openScope?: (id: string) => void;
}) {
  const [scopes, setScopes] = useState<Scopes>();
  const [selected, setSelected] = useState(scope ?? (stop === undefined ? 'instrument' : `stop:${stop}`));
  const [failure, setFailure] = useState<string>();
  const [notice, setNotice] = useState(false);
  const [scala, setScala] = useState(false);
  const [scalaFailed, setScalaFailed] = useState(false);
  const [history, setHistory] = useState<{ scope: DeskScope; at: number }[]>([]);
  const queue = useRef<Promise<unknown>>(Promise.resolve());

  const refresh = useCallback(() => request<Scopes>('GET', '/api/tuning').then(setScopes, () => setScopes(undefined)), []);
  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), 1000);
    return () => clearInterval(timer);
  }, [refresh]);
  const send = (params: Params) => editInstrument(organ, 'tuning', params);
  const post = (steps: Params[]) => {
    const task = queue.current.catch(() => {}).then(async () => {
      for (const params of steps) await send(params);
    });
    queue.current = task;
    return task.finally(() => void refresh());
  };

  const all: DeskScope[] = scopes ? [
    { id: 'instrument', name: 'Whole instrument', own: { anchor: true, scale: true }, anchor: toAnchor(scopes.instrument), shape: toShape(scopes.instrument) },
    ...scopes.manuals.flatMap(manual => [
      { id: `manual:${manual.idx}`, name: manual.name, parent: 'instrument', own: manual.own, anchor: toAnchor(manual.tuning), shape: toShape(manual.tuning) },
      ...scopes.stops.filter(stop => stop.midx === manual.idx).flatMap(stop => [
        { id: `stop:${stop.id}`, name: stop.name, parent: `manual:${manual.idx}`, own: stop.own, anchor: toAnchor(stop.tuning), shape: toShape(stop.tuning) },
        // A single-rank stop is its rank: only a mixture's ranks are listed.
        ...(stop.ranks.length > 1 ? stop.ranks.map(rank => ({
          id: `rank:${stop.id}:${rank.id}`, name: rank.name || `Rank ${rank.id}`, parent: `stop:${stop.id}`, own: rank.own, anchor: toAnchor(rank.tuning), shape: toShape(rank.tuning) })) : []),
      ]),
    ]),
  ] : [];
  const own = (id: string) => id === `stop:${stop}` || id.startsWith(`rank:${stop}:`);
  const shared = (id: string) => id === 'instrument' || id.startsWith('manual:');
  const home = scopes?.stops.find(s => s.id === stop);
  // A stop's editor shows the chain it inherits through, so the desk can say where values come from.
  const views = all.filter(s => stop === undefined ? shared(s.id) : s.id === 'instrument' || s.id === `manual:${home?.midx}` || own(s.id));
  const choose = (id: string) => stop === undefined || own(id) ? setSelected(id) : openScope?.(id);
  const remember = (id: string) => {
    const scope = views.find(s => s.id === id);
    if (!scope) return;
    const now = Date.now();
    setHistory(h => h.at(-1)?.scope.id === id && now - h.at(-1)!.at < 800 ? h : [...h, { scope, at: now }]);
  };
  const change = (id: string, params: Params[], message: string) => {
    remember(id);
    post(params.map(p => ({ ...scopeParams(id), ...p }))).catch(() => setFailure(message));
  };
  const undo = () => {
    const last = history.at(-1);
    if (!last) return;
    setHistory(history.slice(0, -1));
    const { scope } = last;
    const at = scopeParams(scope.id);
    const steps: Params[] = scope.id === 'instrument'
      ? [{ ...anchorParams(scope.anchor), ...shapeParams(scope.shape) }]
      : [scope.own.anchor ? { ...at, ...anchorParams(scope.anchor) } : { ...at, follow: 'anchor' },
        scope.own.scale ? { ...at, ...shapeParams(scope.shape) } : { ...at, follow: 'scale' }];
    post(steps).catch(() => setFailure('That change could not be undone.'));
  };

  // The top bar's Undo steps back through this panel's edits while it is open.
  const latestUndo = useRef(undo);
  latestUndo.current = undo;
  const undoable = history.length > 0;
  useEffect(() => { offerUndo(undoable ? () => latestUndo.current() : undefined); }, [undoable, offerUndo]);
  useEffect(() => () => offerUndo(undefined), [offerUndo]);

  if (!scopes) return <Stack align="center" p="xl"><Loader size="sm"/><Text c="dimmed">Loading tuning</Text></Stack>;
  if (stop !== undefined && !home) return <Text c="dimmed" p="md">This stop has no tuning of its own.</Text>;
  const current = views.find(s => s.id === selected) ?? views.find(s => own(s.id)) ?? views[0];
  const ranks = views.filter(s => own(s.id));
  return <div className="tuning-panel">
    {ranks.length > 1 && <SegmentedControl mb="sm" aria-label="Tuning of" value={current.id} onChange={setSelected}
      data={ranks.map(s => ({ value: s.id, label: s.id.startsWith('stop:') ? 'Whole stop' : s.name }))}/>}
    <TuningDesk layout="channel" scopes={views} selected={current.id} select={choose} recorded browser={stop === undefined}
      assign={() => setNotice(true)}
      setAnchor={(id, anchor) => change(id, [anchorParams(anchor)], 'That pitch could not be set.')}
      setShape={(id, shape) => change(id, [shapeParams(shape)], 'That scale could not be set.')}
      setOwn={(id, part: Part, own) => change(id, [own ? { own: part } : { follow: part }], 'That scope could not be changed.')}
      importScala={() => { setScalaFailed(false); setScala(true); }}/>
    <ScalaSheet opened={scala} failed={scalaFailed} close={() => setScala(false)}
      apply={(scl, kbm) => { remember(current.id); post([{ ...scopeParams(current.id), scale: scl, ...(kbm ? { keymap: kbm } : {}) }]).then(() => setScala(false), () => setScalaFailed(true)); }}/>
    <Modal opened={notice} onClose={() => setNotice(false)} title="Assign control">
      <Stack><Text>Assigning a MIDI control or LFO to tuning is not available yet.</Text><Button onClick={() => setNotice(false)}>Close</Button></Stack>
    </Modal>
    <Modal opened={Boolean(failure)} onClose={() => setFailure(undefined)} title="Tuning not changed">
      <Stack><Text>{failure} Check the value and try again.</Text><Button onClick={() => setFailure(undefined)}>Close</Button></Stack>
    </Modal>
  </div>;
}
