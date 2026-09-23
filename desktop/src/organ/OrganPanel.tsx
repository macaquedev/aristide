import { useCallback, useEffect, useRef, useState } from 'react';
import { Button, Checkbox, Drawer, Group, Loader, Modal, NumberInput, SegmentedControl, Select, Stack, Text } from '@mantine/core';
import { editInstrument, endpoint, request } from '../api';
import { NumberControl } from '../design/NumberControl';
import { RollCanvas, type RollView } from '../design/RollCanvas';
import { anchorName, cents, validNote, type Anchor, type RollModel, type RollNote, type Stamp } from '../design/rollModel';
import '../design/roll.css';
import './organ.css';

type RuleSource = { stop: number; rank: number | null };
type RuleEvent = { source: RuleSource; cents: number; level: number; start: string; end: string | null };
type Rule = { stamps: Stamp[]; events: RuleEvent[] };
type SourceStop = { stop: number; name: string; manual: string; midx: number; ranks: { id: number; name: string }[] };
export type StopRule = Rule & {
  stop: { id: number; name: string; manual: string; midx: number };
  custom: boolean; voices: number; sources: SourceStop[];
};

const home: RollView = { time: 0, center: 0, span: 120, pitchSpan: 3600 };
const sourceId = ({ stop, rank }: RuleSource) => rank === null ? String(stop) : `${stop}:${rank}`;
const parseSource = (id: string): RuleSource => {
  const [stop, rank] = id.split(':').map(Number);
  return { stop, rank: rank === undefined || Number.isNaN(rank) ? null : rank };
};
const noteId = (index: number) => `e${index}`;
const indexOf = (id: string) => Number(id.slice(1));

function toModel(rule: StopRule): RollModel {
  const sources = rule.sources.flatMap(stop => [
    { id: String(stop.stop), name: stop.name, kind: 'Stop' as const, organ: stop.manual, transpose: 0 },
    ...(stop.ranks.length > 1 ? stop.ranks.map(rank => ({ id: `${stop.stop}:${rank.id}`, name: `${stop.name} · ${rank.name}`, kind: 'Rank' as const, organ: stop.manual, transpose: 0 })) : []),
  ]);
  return {
    stamps: rule.stamps,
    notes: rule.events.map((event, index) => ({ id: noteId(index), source: sourceId(event.source), pitch: event.cents, start: event.start, end: event.end, level: event.level })),
    sources,
  };
}
const toRule = (model: RollModel): Rule => ({
  stamps: model.stamps,
  events: model.notes.map(note => ({ source: parseSource(note.source), cents: note.pitch, level: note.level, start: note.start, end: note.end })),
});
const freshStamp = (anchor: Anchor, ms: number): Stamp => ({ id: `${anchor[0]}${crypto.randomUUID().slice(0, 8)}`, anchor, ms });

/** Organ: the organ's stops, and one stop's rule as two piano rolls (key down, key up) with the event inspector beside them.
 * Every edit sounds from the next note, held keys re-speak, and the organ file saves it. */
export function OrganPanel({ organ, stops, stopId, select, offerUndo, openTuning }: {
  organ: string; stops: { id: number; name: string; midx: number; manual: string; custom?: boolean }[];
  stopId?: number; select: (id: number) => void; offerUndo: (undo?: () => void) => void; openTuning: (stop: number) => void;
}) {
  const root = useRef<HTMLDivElement>(null);
  const pointer = useRef<{ x: number; y: number } | null>(null);
  const current = stopId ?? stops[0]?.id;
  const [rule, setRule] = useState<StopRule>();
  const [model, setModel] = useState<RollModel>();
  const [history, setHistory] = useState<Rule[]>([]);
  const [selected, setSelected] = useState(noteId(0));
  const [views, setViews] = useState<Record<Anchor, RollView>>({ down: home, up: home });
  const [snap, setSnap] = useState(false);
  const [grid, setGrid] = useState(false);
  const [steps, setSteps] = useState(12);
  const [tool, setTool] = useState('draw');
  const [sourceOpen, setSourceOpen] = useState(false);
  const [stampEditor, setStampEditor] = useState<{ anchor: Anchor; id?: string; ms: number }>();
  const [stampError, setStampError] = useState('');
  const [failure, setFailure] = useState<string>();
  const [notice, setNotice] = useState(false);
  // Latest edit wins: a number dragged fast sends only what is current when the last request returns.
  const sending = useRef(false);
  const pending = useRef<{ stop: number; rule: Rule | null } | null>(null);

  const load = useCallback((stop: number) => request<StopRule>('GET', endpoint('rule', { stop })).then(value => {
    setRule(value); setModel(toModel(value));
  }, () => setFailure('This stop could not be opened.')), []);
  useEffect(() => {
    if (current === undefined) return;
    setHistory([]); setSelected(noteId(0)); setViews({ down: home, up: home });
    void load(current);
  }, [current, load]);

  const flush = async () => {
    if (sending.current) return;
    sending.current = true;
    try {
      while (pending.current) {
        const { stop, rule: next } = pending.current;
        pending.current = null;
        const params: Record<string, string | number> = next ? { stop, rule: JSON.stringify(next) } : { stop, reset: 1 };
        try {
          const reply = await editInstrument<StopRule>(organ, 'organ/rule', params);
          if (!pending.current && reply.stop.id === current) { setRule(reply); setModel(toModel(reply)); }
        } catch {
          setFailure('That change could not be made.');
          await load(stop);
        }
      }
    } finally { sending.current = false; }
  };
  const send = (next: Rule | null) => {
    if (current === undefined) return;
    pending.current = { stop: current, rule: next };
    void flush();
  };
  const edit = (next: RollModel) => {
    if (!model) return;
    setHistory(h => [...h, toRule(model)]);
    setModel(next);
    send(toRule(next));
  };

  const undo = () => {
    const last = history.at(-1);
    if (!last || !model) return;
    setHistory(history.slice(0, -1));
    setModel({ ...model, stamps: last.stamps, notes: toModel({ ...rule!, ...last }).notes });
    send(last);
  };
  const latestUndo = useRef(undo);
  latestUndo.current = undo;
  const undoable = history.length > 0;
  useEffect(() => { offerUndo(undoable ? () => latestUndo.current() : undefined); }, [undoable, offerUndo]);
  useEffect(() => () => offerUndo(undefined), [offerUndo]);

  const note = model?.notes.find(n => n.id === selected) ?? model?.notes[0];
  const update = (next: RollNote) => { if (model && validNote(next, model.stamps)) edit({ ...model, notes: model.notes.map(n => n.id === next.id ? next : n) }); };
  const remove = (id: string) => {
    if (!model || model.notes.length === 1 || !model.notes.some(n => n.id === id)) return;
    const index = indexOf(id);
    edit({ ...model, notes: model.notes.filter(n => n.id !== id).map((n, i) => ({ ...n, id: noteId(i) })) });
    setSelected(noteId(Math.max(0, index - 1)));
  };
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (event.key !== 'Delete' || event.repeat || event.isComposing || event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey) return;
      if (!root.current?.getClientRects().length) return;
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"]), [role="textbox"], [role="combobox"]')) return;
      if ([...document.querySelectorAll('[role="dialog"]')].some(dialog => dialog.getClientRects().length)) return;
      const hovered = pointer.current && document.elementFromPoint(pointer.current.x, pointer.current.y)?.closest('[data-note]');
      const id = hovered && root.current.contains(hovered) ? hovered.getAttribute('data-note') : note?.id;
      if (!id) return;
      event.preventDefault();
      remove(id);
    };
    document.addEventListener('keydown', keydown);
    return () => document.removeEventListener('keydown', keydown);
  });

  if (current === undefined) return <Stack align="center" p="xl"><Text>This organ has no stops.</Text></Stack>;
  if (!rule || !model) return <Stack align="center" p="xl"><Loader size="sm"/><Text c="dimmed">Opening stop</Text></Stack>;

  const source = model.sources.find(s => s.id === note?.source);
  const start = model.stamps.find(s => s.id === note?.start);
  const end = model.stamps.find(s => s.id === note?.end);
  const add = (anchor: Anchor, stamp: string, pitch: number) => {
    let stamps = model.stamps;
    let ending: string | null = null;
    if (anchor === 'up') {
      const onset = stamps.find(s => s.id === stamp)!;
      let next = stamps.filter(s => s.anchor === 'up' && s.ms > onset.ms).sort((a, b) => a.ms - b.ms)[0];
      if (!next) { next = freshStamp('up', onset.ms + 50); stamps = [...stamps, next]; }
      ending = next.id;
    }
    const added: RollNote = { id: noteId(model.notes.length), source: note?.source ?? String(rule.stop.id), pitch, start: stamp, end: ending, level: 0 };
    edit({ ...model, stamps, notes: [...model.notes, added] });
    setSelected(added.id);
  };
  const endings = note ? model.stamps.filter(s => validNote({ ...note, end: s.id }, model.stamps)).map(s => ({ value: s.id, label: `${anchorName(s.anchor)} + ${s.ms} ms` })) : [];
  const mode = end ? end.anchor === 'up' && start?.anchor === 'down' ? 'after' : 'finite' : 'held';
  const changeMode = (value: string | null) => {
    if (!note || !start || !value) return;
    if (value === 'held') return update({ ...note, end: null });
    const anchor = value === 'after' ? 'up' : start.anchor;
    let stamps = model.stamps;
    let marker = stamps.filter(s => s.anchor === anchor && (anchor !== start.anchor ? s.ms > 0 : s.ms > start.ms)).sort((a, b) => a.ms - b.ms)[0];
    if (!marker) { marker = freshStamp(anchor, (anchor === start.anchor ? start.ms : 0) + 50); stamps = [...stamps, marker]; }
    edit({ ...model, stamps, notes: model.notes.map(n => n.id === note.id ? { ...n, end: marker.id } : n) });
  };
  const inspector = note && source && start ? <Stack gap="md" className="roll-inspector-fields">
    <Text fw={600}>Event</Text>
    <Button variant="default" justify="space-between" onClick={() => setSourceOpen(true)} aria-label={`Source: ${source.name}`}
      rightSection={<Text component="span" size="xs" c="dimmed">{source.organ}</Text>}>{source.name}</Button>
    <Group grow align="start">
      <div><Text c="dimmed" size="xs">Pitch offset</Text><NumberControl label="Pitch" value={note.pitch} unit="¢" min={-9600} max={9600} change={pitch => update({ ...note, pitch })} assign={() => setNotice(true)}/></div>
      <div><Text c="dimmed" size="xs">Level</Text><NumberControl label="Level" value={note.level} unit="dB" step={0.5} min={-60} max={12} change={level => update({ ...note, level })} assign={() => setNotice(true)}/></div>
    </Group>
    <Select label="Starts at" value={note.start} allowDeselect={false} data={model.stamps.filter(s => validNote({ ...note, start: s.id }, model.stamps)).map(s => ({ value: s.id, label: `${anchorName(s.anchor)} + ${s.ms} ms` }))} onChange={value => value && update({ ...note, start: value })}/>
    <Select label="Duration" value={mode} allowDeselect={false} data={start.anchor === 'down' ? [{ value: 'held', label: 'Until release →' }, { value: 'finite', label: 'End at timestamp' }, { value: 'after', label: 'Hold after release → / ←' }] : [{ value: 'finite', label: 'End at timestamp' }]} onChange={changeMode}/>
    {end && <Select label="Ends at" value={end.id} allowDeselect={false} data={endings.filter(option => mode === 'after' ? model.stamps.find(s => s.id === option.value)!.anchor === 'up' : model.stamps.find(s => s.id === option.value)!.anchor === start.anchor)} onChange={value => value && update({ ...note, end: value })}/>}
    {end?.anchor === start.anchor && <Text size="xs" c="dimmed">{end.ms - start.ms} ms</Text>}
    <Group grow>
      <Button variant="default" onClick={() => { const copy = { ...note, id: noteId(model.notes.length) }; edit({ ...model, notes: [...model.notes, copy] }); setSelected(copy.id); }}>Duplicate</Button>
      <Button variant="subtle" color="red" disabled={model.notes.length === 1} onClick={() => remove(note.id)}>Delete</Button>
    </Group>
  </Stack> : <Text c="dimmed">Tap a roll to add an event.</Text>;
  const canvas = (anchor: Anchor) => <RollCanvas key={anchor} anchor={anchor} model={model} selected={note?.id ?? ''} height={400}
    view={views[anchor]} setView={view => setViews(v => ({ ...v, [anchor]: view }))} snap={snap} steps={steps} grid={grid} pan={tool === 'pan'}
    select={setSelected} inspect={setSelected} remove={remove} update={update} add={add}
    editStamp={stamp => { setStampError(''); setStampEditor(stamp); }}
    addStamp={anchor => { setStampError(''); setStampEditor({ anchor, ms: Math.max(...model.stamps.filter(s => s.anchor === anchor).map(s => s.ms)) + 50 }); }}/>;
  const zoom = (axis: 'span' | 'pitchSpan', factor: number) => setViews(v => Object.fromEntries(Object.entries(v).map(([key, value]) => [key, { ...value, [axis]: Math.max(axis === 'span' ? 10 : 8, Math.min(axis === 'span' ? 10000 : 19200, value[axis] * factor)) }])) as Record<Anchor, RollView>);
  const divisions = [...new Map(stops.map(s => [s.midx, s.manual])).entries()];

  return <div className="build-panel">
    <nav className="build-stops" aria-label="Stops">{divisions.map(([midx, manual]) => <div key={midx}>
      <Text size="xs" c="dimmed" className="build-division">{manual}</Text>
      {stops.filter(s => s.midx === midx).map(s => <Button key={s.id} fullWidth justify="space-between" variant={s.id === current ? 'light' : 'subtle'} color={s.id === current ? undefined : 'gray'}
        aria-current={s.id === current ? 'true' : undefined} onClick={() => select(s.id)}>{s.name}{s.custom && <span className="modified" aria-label="custom">◇</span>}</Button>)}
    </div>)}</nav>
    <Stack ref={root} className="piano-study build-editor" gap="md"
      onPointerMoveCapture={e => { pointer.current = e.pointerType === 'touch' ? null : { x: e.clientX, y: e.clientY }; }} onPointerLeave={() => { pointer.current = null; }}>
      <Group justify="space-between" align="center">
        <div>
          <Text className="roll-stop-name" fw={600}>{rule.stop.name} {rule.custom && <span className="modified" aria-label="custom">◇</span>}</Text>
          <Text c="dimmed" size="xs">{rule.stop.manual} · {rule.voices} {rule.voices === 1 ? 'voice' : 'voices'} per key</Text>
        </div>
        <Group gap="xs">
          <Button variant="default" onClick={() => openTuning(rule.stop.id)}>Tuning</Button>
          <Button variant="default" disabled={!rule.custom} onClick={() => { setHistory(h => [...h, toRule(model)]); send(null); }}>Reset</Button>
          <Button onClick={() => add('down', 'down', 0)}>Add event</Button>
        </Group>
      </Group>
      <div className="roll-toolbar">
        <SegmentedControl aria-label="Roll tool" value={tool} onChange={setTool} data={[{ value: 'draw', label: 'Draw' }, { value: 'pan', label: 'Pan' }]}/>
        <div className="roll-zoom"><Text size="xs" c="dimmed">Time</Text><Button variant="default" aria-label="Zoom time out" onClick={() => zoom('span', 1.5)}>−</Button><Button variant="default" aria-label="Zoom time in" onClick={() => zoom('span', 1 / 1.5)}>+</Button></div>
        <div className="roll-zoom"><Text size="xs" c="dimmed">Pitch</Text><Button variant="default" aria-label="Zoom pitch out" onClick={() => zoom('pitchSpan', 1.5)}>−</Button><Button variant="default" aria-label="Zoom pitch in" onClick={() => zoom('pitchSpan', 1 / 1.5)}>+</Button></div>
        <Button variant="subtle" onClick={() => setViews({ down: home, up: home })}>Centre 0 ¢</Button>
        <Checkbox label="Snap pitch" checked={snap} onChange={e => setSnap(e.currentTarget.checked)}/>
        <Checkbox label="Time grid" checked={grid} onChange={e => setGrid(e.currentTarget.checked)}/>
        <Select className="roll-tuning" aria-label="Pitch guide" value={String(steps)} allowDeselect={false} data={[{ value: '12', label: 'Guide · 12 equal' }, { value: '19', label: 'Guide · 19 equal' }, { value: '31', label: 'Guide · 31 equal' }]} onChange={value => value && setSteps(Number(value))}/>
      </div>
      <div className="roll-event-strip" aria-label="Events">{model.notes.map(n => <Button key={n.id} variant={note?.id === n.id ? 'light' : 'subtle'} aria-pressed={note?.id === n.id} onClick={() => setSelected(n.id)}>
        {model.sources.find(s => s.id === n.source)?.name ?? 'Missing'} <span className="event-chip-pitch">{cents(n.pitch)}</span></Button>)}</div>
      <div className="roll-workspace variant-split">
        <div className="roll-pair">{canvas('down')}{canvas('up')}</div>
        <aside className="roll-inspector">{inspector}</aside>
      </div>
      <Group justify="space-between" className="roll-footer"><Text size="xs" c="dimmed">→ Until release · → / ← Hold after release</Text><Text size="xs" c="dimmed">Right-click / Delete to remove · Middle-drag to pan</Text></Group>
    </Stack>
    <Drawer closeButtonProps={{ 'aria-label': 'Close' }} opened={sourceOpen} onClose={() => setSourceOpen(false)} title="Source" position="right" size="md"><Stack gap="xs">
      {divisions.map(([midx, manual]) => <Stack key={midx} gap={4}><Text size="xs" c="dimmed">{manual}</Text>
        {rule.sources.filter(s => s.midx === midx).flatMap(s => [{ id: String(s.stop), label: s.name, rank: false },
          ...(s.ranks.length > 1 ? s.ranks.map(r => ({ id: `${s.stop}:${r.id}`, label: r.name, rank: true })) : [])]).map(option =>
          <Button key={option.id} variant={option.id === note?.source ? 'light' : 'default'} justify="start" pl={option.rank ? 'xl' : undefined}
            onClick={() => { if (note) update({ ...note, source: option.id }); setSourceOpen(false); }}>{option.label}</Button>)}
      </Stack>)}
    </Stack></Drawer>
    <Drawer closeButtonProps={{ 'aria-label': 'Close' }} opened={Boolean(stampEditor)} onClose={() => setStampEditor(undefined)} title={stampEditor?.id ? 'Edit timestamp' : 'Add timestamp'} position="right"><Stack>
      <Text fw={600}>{stampEditor && anchorName(stampEditor.anchor)} +</Text>
      {stampEditor?.id ? <NumberControl label="Timestamp" unit="ms" value={model.stamps.find(s => s.id === stampEditor.id)?.ms ?? 0} min={0} max={30000} disabled={stampEditor.id === 'down' || stampEditor.id === 'up'} change={ms => {
        const stamps = model.stamps.map(s => s.id === stampEditor.id ? { ...s, ms } : s);
        if (stamps.some(s => s.id !== stampEditor.id && s.anchor === stampEditor.anchor && s.ms === ms)) return setStampError('A timestamp already exists at this time.');
        if (!model.notes.every(n => validNote(n, stamps))) return setStampError('An ending must remain after its start.');
        setStampError(''); edit({ ...model, stamps });
      }} assign={() => setNotice(true)}/> : <NumberInput label="Time (ms)" min={0} max={30000} value={stampEditor?.ms ?? 0} onChange={value => { if (stampEditor && typeof value === 'number') setStampEditor({ ...stampEditor, ms: value }); }} data-autofocus/>}
      {stampError && <Text role="alert">{stampError}</Text>}
      <Button onClick={() => {
        if (!stampEditor) return;
        if (!stampEditor.id) {
          if (!Number.isFinite(stampEditor.ms) || stampEditor.ms <= 0) return setStampError('Choose a time after 0 ms.');
          if (model.stamps.some(s => s.anchor === stampEditor.anchor && s.ms === stampEditor.ms)) return setStampError('A timestamp already exists at this time.');
          edit({ ...model, stamps: [...model.stamps, freshStamp(stampEditor.anchor, stampEditor.ms)] });
        }
        const ms = stampEditor.id ? model.stamps.find(s => s.id === stampEditor.id)!.ms : stampEditor.ms;
        const view = views[stampEditor.anchor];
        if (ms < view.time || ms > view.time + view.span * .9) setViews(v => ({ ...v, [stampEditor.anchor]: { ...view, time: Math.max(0, ms - view.span * .65) } }));
        setStampEditor(undefined);
      }}>{stampEditor?.id ? 'Done' : 'Add timestamp'}</Button>
    </Stack></Drawer>
    <Modal opened={notice} onClose={() => setNotice(false)} title="Assign control">
      <Stack><Text>Assigning a MIDI control or LFO to a stop rule is not available yet.</Text><Button onClick={() => setNotice(false)}>Close</Button></Stack>
    </Modal>
    <Modal opened={Boolean(failure)} onClose={() => setFailure(undefined)} title="Stop not changed">
      <Stack><Text>{failure} The stop keeps its last saved rule.</Text><Button onClick={() => setFailure(undefined)}>Close</Button></Stack>
    </Modal>
  </div>;
}
