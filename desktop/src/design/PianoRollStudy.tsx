import { useState } from 'react';
import { Badge, Button, Checkbox, Drawer, Group, NumberInput, SegmentedControl, Select, Stack, Text } from '@mantine/core';
import { NumberControl } from './NumberControl';
import { RollCanvas } from './RollCanvas';
import type { RollView } from './RollCanvas';
import { anchorName, cents, initialRoll, validNote } from './rollModel';
import type { Anchor, RollModel, RollNote, Stamp } from './rollModel';
import './roll.css';

export const rollLayouts = [
  { value: 'split', label: '1 · Split desk' }, { value: 'stacked', label: '2 · Stacked' },
  { value: 'focus', label: '3 · Focus' }, { value: 'lanes', label: '4 · Source lanes' },
];
const home: RollView = { time: 0, center: 0, span: 120, pitchSpan: 3600 };

export function PianoRollStudy({ layout, compact, assign }: { layout: string; compact: boolean; assign: (name: string) => void }) {
  const [model, setModel] = useState<RollModel>(initialRoll);
  const [history, setHistory] = useState<RollModel[]>([]);
  const [selected, setSelected] = useState('flute');
  const [focus, setFocus] = useState<Anchor>('down');
  const [views, setViews] = useState<Record<Anchor, RollView>>({ down: home, up: home });
  const [snap, setSnap] = useState(false);
  const [grid, setGrid] = useState(false);
  const [steps, setSteps] = useState(12);
  const [tool, setTool] = useState('draw');
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [sourceOpen, setSourceOpen] = useState(false);
  const [sourceKind, setSourceKind] = useState('Stop');
  const [stampEditor, setStampEditor] = useState<{ anchor: Anchor; id?: string; ms: number }>();
  const [stampError, setStampError] = useState('');
  const note = model.notes.find(n => n.id === selected) ?? model.notes[0];
  const source = model.sources.find(s => s.id === note?.source);
  const start = model.stamps.find(s => s.id === note?.start);
  const end = model.stamps.find(s => s.id === note?.end);
  const edit = (next: RollModel) => { setHistory(h => [...h, model]); setModel(next); };
  const update = (next: RollNote) => { if (validNote(next, model.stamps)) edit({ ...model, notes: model.notes.map(n => n.id === next.id ? next : n) }); };
  const select = (id: string) => { setSelected(id); if (layout === 'focus' || layout === 'lanes') setInspectorOpen(true); };
  const editStamp = (stamp: Stamp) => { setStampError(''); setStampEditor(stamp); };
  const addStamp = (anchor: Anchor) => {
    setStampError('');
    setStampEditor({ anchor, ms: Math.max(...model.stamps.filter(s => s.anchor === anchor).map(s => s.ms)) + 50 });
  };
  const add = (anchor: Anchor, stamp: string, pitch: number, sourceId?: string) => {
    let stamps = model.stamps;
    let ending: string | null = null;
    if (anchor === 'up') {
      const onset = stamps.find(s => s.id === stamp)!;
      let next = stamps.filter(s => s.anchor === 'up' && s.ms > onset.ms).sort((a, b) => a.ms - b.ms)[0];
      if (!next) { next = { id: crypto.randomUUID(), anchor: 'up', ms: onset.ms + 50 }; stamps = [...stamps, next]; }
      ending = next.id;
    }
    const added: RollNote = { id: crypto.randomUUID(), source: sourceId ?? note?.source ?? 'theorbe', pitch, start: stamp, end: ending, level: 0 };
    edit({ ...model, stamps, notes: [...model.notes, added] });
    select(added.id);
  };
  const endings = note ? model.stamps.filter(s => validNote({ ...note, end: s.id }, model.stamps)).map(s => ({ value: s.id, label: `${anchorName(s.anchor)} + ${s.ms} ms` })) : [];
  const mode = end ? end.anchor === 'up' && start?.anchor === 'down' ? 'after' : 'finite' : 'held';
  const changeMode = (value: string | null) => {
    if (!note || !start || !value) return;
    if (value === 'held') return update({ ...note, end: null });
    const anchor = value === 'after' ? 'up' : start.anchor;
    let stamps = model.stamps;
    let marker = stamps.filter(s => s.anchor === anchor && (anchor !== start.anchor ? s.ms > 0 : s.ms > start.ms)).sort((a, b) => a.ms - b.ms)[0];
    if (!marker) { marker = { id: crypto.randomUUID(), anchor, ms: (anchor === start.anchor ? start.ms : 0) + 50 }; stamps = [...stamps, marker]; }
    edit({ ...model, stamps, notes: model.notes.map(n => n.id === note.id ? { ...n, end: marker.id } : n) });
  };
  const number = (label: string, value: number, unit: string, change: (value: number) => void) => <NumberControl label={label} value={value} unit={unit} change={change} assign={() => assign(`stop/titanique/voice/${note?.id}/${label === 'Pitch' ? 'pitch' : 'level'}`)}/>;
  const inspector = note && source && start ? <Stack gap="md" className="roll-inspector-fields">
    <Group justify="space-between"><Text fw={600}>Event</Text><Badge variant="outline" color="gray">{source.kind}</Badge></Group>
    <Button variant="default" justify="space-between" onClick={() => { setSourceKind(source.kind); setSourceOpen(true); }}>{source.name} <span>↗</span></Button>
    <Group grow align="start"><div><Text c="dimmed" size="xs">Pitch offset</Text>{number('Pitch', note.pitch, '¢', pitch => update({ ...note, pitch }))}</div><div><Text c="dimmed" size="xs">Level</Text>{number('Level', note.level, 'dB', level => update({ ...note, level }))}</div></Group>
    <Select label="Starts at" value={note.start} allowDeselect={false} data={model.stamps.filter(s => validNote({ ...note, start: s.id }, model.stamps)).map(s => ({ value: s.id, label: `${anchorName(s.anchor)} + ${s.ms} ms` }))} onChange={value => value && update({ ...note, start: value })}/>
    <Select label="Duration" value={mode} allowDeselect={false} data={start.anchor === 'down' ? [{ value: 'held', label: 'Until release →' }, { value: 'finite', label: 'End at timestamp' }, { value: 'after', label: 'Hold after release → / ←' }] : [{ value: 'finite', label: 'End at timestamp' }]} onChange={changeMode}/>
    {end && <Select label="Ends at" value={end.id} allowDeselect={false} data={endings.filter(option => mode === 'after' ? model.stamps.find(s => s.id === option.value)!.anchor === 'up' : model.stamps.find(s => s.id === option.value)!.anchor === start.anchor)} onChange={value => value && update({ ...note, end: value })}/>}
    {end?.anchor === start.anchor && <Text size="xs" c="dimmed">{end.ms - start.ms} ms duration</Text>}
    {source.kind === 'Stop' && <div className="reference-summary"><Text size="xs" c="dimmed">Live stop reference · {source.organ}</Text><Text size="xs">Source offset {cents(source.transpose)} · combined {cents(note.pitch + source.transpose)}</Text></div>}
    <Group grow><Button variant="default" onClick={() => { const copy = { ...note, id: crypto.randomUUID() }; edit({ ...model, notes: [...model.notes, copy] }); setSelected(copy.id); }}>Duplicate</Button><Button variant="subtle" color="red" onClick={() => { edit({ ...model, notes: model.notes.filter(n => n.id !== note.id) }); setInspectorOpen(false); }}>Delete</Button></Group>
  </Stack> : <Text c="dimmed">Click the roll to add an event.</Text>;
  const canvas = (anchor: Anchor, height: number, sourceId?: string) => <RollCanvas key={`${anchor}-${sourceId ?? 'all'}`} anchor={anchor} model={model} selected={note?.id ?? ''} source={sourceId} height={height}
    view={views[anchor]} setView={view => setViews(v => ({ ...v, [anchor]: view }))} snap={snap} steps={steps} grid={grid} pan={tool === 'pan'}
    select={setSelected} inspect={select} update={update} add={add} editStamp={editStamp} addStamp={addStamp}/>;
  const zoom = (axis: 'span' | 'pitchSpan', factor: number) => setViews(v => Object.fromEntries(Object.entries(v).map(([key, value]) => [key, { ...value, [axis]: Math.max(axis === 'span' ? 10 : 8, Math.min(axis === 'span' ? 10000 : 19200, value[axis] * factor)) }])) as Record<Anchor, RollView>);
  const sourceIds = [...new Set(model.notes.map(n => n.source))];

  return <Stack className={`piano-study ${compact ? 'compact-roll' : ''}`} gap="md">
    <Group justify="space-between" align="center"><div><Text className="roll-stop-name" fw={600}>Titanique <span className="modified">◇</span></Text><Text c="dimmed" size="xs">Grand-orgue · {model.notes.length} events · Follow division</Text></div>
      <Group gap="xs"><Button variant="subtle" onClick={() => {
        const example = { ...initialRoll, notes: [...initialRoll.notes.map(n => n.id === 'theorbe' ? { ...n, end: 'u50' } : n), { id: 'release-example', source: 'bourdon', pitch: 700, start: 'u50', end: 'u150', level: -6 }] };
        edit(example); setSelected('theorbe'); setViews({ down: { ...home, span: 180 }, up: { ...home, span: 180 } });
      }}>Try release example</Button><Button variant="default" disabled={!history.length} onClick={() => { setModel(history.at(-1)!); setHistory(history.slice(0, -1)); }}>Undo</Button><Button onClick={() => add(focus, model.stamps.find(s => s.anchor === focus && s.ms === 0)!.id, 0)}>Add event</Button></Group></Group>
    <div className="roll-toolbar">
      <SegmentedControl aria-label="Roll tool" value={tool} onChange={setTool} data={[{ value: 'draw', label: 'Draw' }, { value: 'pan', label: 'Pan' }]}/>
      <div className="roll-zoom"><Text size="xs" c="dimmed">Time</Text><Button variant="default" aria-label="Zoom time out" onClick={() => zoom('span', 1.5)}>−</Button><Button variant="default" aria-label="Zoom time in" onClick={() => zoom('span', 1 / 1.5)}>+</Button></div>
      <div className="roll-zoom"><Text size="xs" c="dimmed">Pitch</Text><Button variant="default" aria-label="Zoom pitch out" onClick={() => zoom('pitchSpan', 1.5)}>−</Button><Button variant="default" aria-label="Zoom pitch in" onClick={() => zoom('pitchSpan', 1 / 1.5)}>+</Button></div>
      <Button variant="subtle" onClick={() => setViews({ down: home, up: home })}>Centre 0 ¢</Button>
      <Checkbox label="Snap pitch" checked={snap} onChange={e => setSnap(e.currentTarget.checked)}/>
      <Checkbox label="Time grid" checked={grid} onChange={e => setGrid(e.currentTarget.checked)}/>
      <Select className="roll-tuning" aria-label="Global tuning guide" value={String(steps)} allowDeselect={false} data={[{ value: '12', label: 'Global · 12 equal' }, { value: '19', label: 'Global · 19 equal' }, { value: '31', label: 'Global · 31 equal' }]} onChange={value => value && setSteps(Number(value))}/>
    </div>
    <div className="roll-event-strip" aria-label="Events">{model.notes.map(n => { const s = model.sources.find(s => s.id === n.source)!; return <Button key={n.id} variant={note?.id === n.id ? 'light' : 'subtle'} aria-pressed={note?.id === n.id} onClick={() => select(n.id)}>{s.name} <span className="event-chip-pitch">{cents(n.pitch)}</span></Button>; })}</div>
    <div className={`roll-workspace variant-${layout}`}>
      {layout === 'split' && <><div className="roll-pair">{canvas('down', 400)}{canvas('up', 400)}</div><aside className="roll-inspector">{inspector}</aside></>}
      {layout === 'stacked' && <><div className="roll-stack">{canvas('down', 290)}{canvas('up', 290)}</div><aside className="roll-inspector">{inspector}</aside></>}
      {layout === 'focus' && <div className="roll-focus"><Group justify="space-between" mb="sm"><SegmentedControl aria-label="Timeline" value={focus} onChange={v => setFocus(v as Anchor)} data={[{ value: 'down', label: 'Key down' }, { value: 'up', label: 'Key up' }]}/><Button variant="default" disabled={!note} onClick={() => setInspectorOpen(true)}>Edit event</Button></Group>{canvas(focus, 480)}<div className="roll-other-events">{model.notes.filter(n => { const s = model.stamps.find(s => s.id === n.start)!; const e = model.stamps.find(s => s.id === n.end); return s.anchor !== focus || (e !== undefined && e.anchor !== focus); }).map(n => <Button key={n.id} variant="subtle" onClick={() => { setSelected(n.id); setFocus(focus === 'down' ? 'up' : 'down'); }}>{model.sources.find(s => s.id === n.source)!.name} · {focus === 'down' ? 'Key up' : 'Key down'} ↗</Button>)}</div></div>}
      {layout === 'lanes' && <div className="roll-lanes">{sourceIds.map(id => <section key={id} className="roll-source-lane"><Group justify="space-between" mb="xs"><Text fw={600}>{model.sources.find(s => s.id === id)!.name}</Text><Badge color="gray" variant="outline">{model.sources.find(s => s.id === id)!.kind}</Badge></Group><div className="roll-pair">{canvas('down', 230, id)}{canvas('up', 230, id)}</div></section>)}{!sourceIds.length && canvas('down', 400)}</div>}
    </div>
    <Group justify="space-between" className="roll-footer"><Text size="xs" c="dimmed">→ Until release · → / ← Hold after release</Text><Text size="xs" c="dimmed">Click to draw · Drag to move · Middle-drag to pan</Text></Group>
    <Drawer closeButtonProps={{ 'aria-label': 'Close' }} opened={inspectorOpen && (layout === 'focus' || layout === 'lanes')} onClose={() => setInspectorOpen(false)} title="Event" position="right">{inspector}</Drawer>
    <Drawer closeButtonProps={{ 'aria-label': 'Close' }} opened={sourceOpen} onClose={() => setSourceOpen(false)} title="Source · prototype" position="right" size="md"><Stack>
      <SegmentedControl value={sourceKind} onChange={setSourceKind} data={['Stop', 'Rank']}/>
      {model.sources.filter(s => s.kind === sourceKind).map(s => <Button key={s.id} variant={s.id === note?.source ? 'light' : 'default'} justify="space-between" onClick={() => { if (note) update({ ...note, source: s.id }); setSourceOpen(false); }}>{s.name}<Text component="span" size="xs" c="dimmed">{s.organ}</Text></Button>)}
      {sourceKind === 'Stop' && <><Text c="dimmed" size="xs">Live references · independent of console on/off</Text><Text c="dimmed" size="xs">Titanique is excluded to prevent self-reference.</Text></>}
      {source?.kind === 'Stop' && <div className="reference-editor"><Text fw={600} mb="xs">{source.name} · source rule</Text><Text size="xs" c="dimmed" mb="sm">Prototype pitch offset, shared by every reference</Text><NumberControl label="Source pitch" value={source.transpose} unit="¢" change={transpose => edit({ ...model, sources: model.sources.map(s => s.id === source.id ? { ...s, transpose } : s) })} assign={() => assign(`stop/${source.id}/voice/source/pitch`)}/></div>}
    </Stack></Drawer>
    <Drawer closeButtonProps={{ 'aria-label': 'Close' }} opened={Boolean(stampEditor)} onClose={() => setStampEditor(undefined)} title={stampEditor?.id ? 'Edit timestamp' : 'Add timestamp'} position="right"><Stack>
      <Text fw={600}>{stampEditor && anchorName(stampEditor.anchor)} +</Text>
      {stampEditor?.id ? <NumberControl label="Timestamp" unit="ms" value={model.stamps.find(s => s.id === stampEditor.id)?.ms ?? 0} min={0} disabled={model.stamps.find(s => s.id === stampEditor.id)?.ms === 0} change={ms => {
        const stamps = model.stamps.map(s => s.id === stampEditor.id ? { ...s, ms } : s);
        if (stamps.some(s => s.id !== stampEditor.id && s.anchor === stampEditor.anchor && s.ms === ms)) return setStampError('A timestamp already exists at this time.');
        if (!model.notes.every(n => validNote(n, stamps))) return setStampError('An ending must remain after its start.');
        setStampError(''); edit({ ...model, stamps });
      }} assign={() => assign('stop/titanique/voice/timestamp/delay')}/> : <NumberInput label="Time (ms)" min={0} value={stampEditor?.ms ?? 0} onChange={value => { if (stampEditor && typeof value === 'number') setStampEditor({ ...stampEditor, ms: value }); }} data-autofocus/>}
      {stampError && <Text role="alert">{stampError}</Text>}
      <Text size="xs" c="dimmed">Attached starts and endings move together.</Text>
      <Button onClick={() => {
        if (!stampEditor) return;
        if (!stampEditor.id) {
          if (!Number.isFinite(stampEditor.ms) || stampEditor.ms < 0) return;
          if (model.stamps.some(s => s.anchor === stampEditor.anchor && s.ms === stampEditor.ms)) return setStampError('A timestamp already exists at this time.');
          edit({ ...model, stamps: [...model.stamps, { ...stampEditor, id: crypto.randomUUID() }] });
        }
        const ms = stampEditor.id ? model.stamps.find(s => s.id === stampEditor.id)!.ms : stampEditor.ms;
        const view = views[stampEditor.anchor];
        if (ms < view.time || ms > view.time + view.span * .9) setViews(v => ({ ...v, [stampEditor.anchor]: { ...view, time: Math.max(0, ms - view.span * .65) } }));
        setStampEditor(undefined);
      }}>{stampEditor?.id ? 'Done' : 'Add timestamp'}</Button>
    </Stack></Drawer>
  </Stack>;
}
