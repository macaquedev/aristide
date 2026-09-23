import { useState, type ReactNode } from 'react';
import { ActionIcon, Badge, Button, Card, Group, Select, Stack, Switch, Tabs, Text, TextInput } from '@mantine/core';
import { ChevronDown, ChevronRight, Search } from 'lucide-react';
import { NumberControl } from '../design/NumberControl';
import { CUSTOM, RECORDED, formatCents, keyHz, noteName, pitchNames, presetOffsets, ratioToCents, relativeOffsets, repeatInterval,
  stepCount, stepIntervals, temperamentName, temperaments, widestFifth, type Anchor, type Shape } from './model';
import { TuningGraph } from './TuningGraph';
import './tuning-desk.css';

export type Part = 'anchor' | 'scale';
export type DeskScope = { id: string; name: string; parent?: string; own: Record<Part, boolean>; anchor: Anchor; shape: Shape };
export type DeskProps = {
  layout: string; scopes: DeskScope[]; selected: string; select: (id: string) => void;
  setAnchor: (id: string, anchor: Anchor) => void; setShape: (id: string, shape: Shape) => void; setOwn: (id: string, part: Part, own: boolean) => void;
  assign: (address: string) => void; recorded?: boolean; undo?: () => void; canUndo?: boolean; importScala?: () => void; readOnly?: boolean;
  /** Hide the scope list when the scope is chosen elsewhere (a stop's own editor). */
  browser?: boolean;
};

export const tuningDeskLayouts = [
  { value: 'channel', label: '1 · Channel strip' }, { value: 'rack', label: '2 · Device rack' },
  { value: 'inspector', label: '3 · Editor + inspector' }, { value: 'tabbed', label: '4 · Tabbed device' },
];

const defaultCollection = [0, 137, 311, 523, 887, 1460, 2107];

export const shapeLabel = (shape: Shape) =>
  shape.system === 'temperament' ? temperamentName(shape.temperament)
    : shape.system === 'equal' ? `${shape.steps} equal` : shape.system === 'steps' ? `${shape.intervals.length} steps` : shape.name;

const keyLabel = (key: number) => `(${noteName(key)})`;

export function TuningDesk({ layout, scopes, selected, select, setAnchor, setShape, setOwn, assign, recorded = false, undo, canUndo = false, importScala, readOnly = false, browser = true }: DeskProps) {
  const [selectedStep, setSelectedStep] = useState(0);
  const [query, setQuery] = useState('');
  const [collapsed, setCollapsed] = useState(() => scopes.filter(s => s.parent && scopes.some(child => child.parent === s.id)).map(s => s.id));
  const [tab, setTab] = useState<string | null>('pitch');
  const scope = scopes.find(s => s.id === selected) ?? scopes[0];
  const { anchor, shape } = scope;
  const parent = scopes.find(s => s.id === scope.parent);
  const anchorLocked = readOnly || (Boolean(parent) && !scope.own.anchor);
  const scaleLocked = readOnly || (Boolean(parent) && !scope.own.scale);
  const twelve = shape.system === 'temperament';
  const period = repeatInterval(shape);
  const count = stepCount(shape);
  const step = Math.min(selectedStep, count - 1);
  const referenceClass = ((anchor.key % 12) + 12) % 12;
  const indices = Array.from({ length: count }, (_, i) => twelve ? (i + shape.root) % 12 : i);
  const labels = indices.map(i => twelve ? pitchNames[i] : `${i + 1}`);
  const values = twelve ? indices.map(i => Number(relativeOffsets(shape.offsets, anchor)[i].toFixed(1))) : stepIntervals(shape);
  const editableNotes = !scaleLocked && (twelve ? shape.temperament === CUSTOM : shape.system === 'steps');
  const fixedStep = twelve ? indices.indexOf(referenceClass) : 0;
  const lower = twelve ? -50 : Math.min(0, ...values) - (shape.system === 'steps' ? 100 : 0);
  const upper = twelve ? 50 : Math.max(period ?? 0, ...values, 100) + (shape.system === 'steps' ? 100 : 0);
  const finite = shape.system === 'steps' && shape.period === null;
  const keyRange = shape.system === 'steps' || shape.system === 'scale' ? [shape.startKey, shape.startKey + count - 1] : undefined;

  const governing = (part: Part) => {
    let current: DeskScope | undefined = scope;
    while (current?.parent && !current.own[part]) current = scopes.find(s => s.id === current!.parent);
    return current ?? scope;
  };
  const changeAnchor = (change: Partial<Anchor>) => { if (!anchorLocked) setAnchor(scope.id, { ...anchor, ...change }); };
  const changeShape = (next: Shape) => { if (!scaleLocked) setShape(scope.id, next); };
  const changeNote = (index: number, value: number) => {
    if (!editableNotes || index === fixedStep) return;
    if (shape.system === 'steps') changeShape({ ...shape, intervals: shape.intervals.map((cents, i) => i === index ? value : cents) });
    if (shape.system === 'temperament') changeShape({ ...shape, offsets: shape.offsets.map((cents, i) => i === indices[index] ? value + shape.offsets[referenceClass] : cents) });
  };
  const chooseSystem = (system: string) => {
    setSelectedStep(0);
    if (system === 'temperament') changeShape({ system, temperament: 'equal', root: 0, offsets: Array(12).fill(0) });
    if (system === 'equal') changeShape({ system, steps: 12, period: 1200 });
    if (system === 'steps') changeShape({ system, intervals: defaultCollection, period: null, startKey: anchor.key });
  };
  const chooseTemperament = (temperament: string) => {
    if (shape.system !== 'temperament') return;
    const preset = temperaments.some(t => t.id === temperament);
    changeShape({ ...shape, temperament, offsets: preset ? presetOffsets(temperament, shape.root) : shape.offsets });
  };

  const followSwitch = (part: Part) => parent && <Switch size="xs" label={`Follow ${parent.name}`} aria-label={`${part === 'anchor' ? 'Pitch' : 'Scale'} follows ${parent.name}`}
    checked={!scope.own[part]} disabled={readOnly} onChange={e => setOwn(scope.id, part, !e.currentTarget.checked)}/>;
  const module = (name: string, content: ReactNode, className = '', part?: Part) => <section className={`tuning-module ${className}`} aria-label={name}>
    <Group className="tuning-module-title" justify="space-between" gap="xs"><Text size="xs" c="dimmed">{name}</Text>{part && followSwitch(part)}</Group>{content}</section>;
  const referenceKey = <div><Text size="xs" c="dimmed">Reference key</Text><NumberControl label="Reference key" value={anchor.key} unit={keyLabel(anchor.key)}
    min={finite ? keyRange![0] : 0} max={finite ? keyRange![1] : 127} disabled={anchorLocked}
    change={value => changeAnchor({ key: Math.round(value) })} assign={() => assign(`tuning/${scope.id}/referenceKey`)}/></div>;
  const reference = module('Reference', <Stack gap="xs">
    <div className="tuning-reference-number"><NumberControl label="Reference pitch" value={Number(anchor.hz.toFixed(2))} unit="Hz" min={1} disabled={anchorLocked} change={hz => changeAnchor({ hz })} assign={() => assign(`tuning/${scope.id}/hz`)}/></div>
    <Group gap={4} grow>{[392, 415, 440, 466].map(hz => <Button key={hz} size="compact-sm" variant={anchor.hz === hz ? 'light' : 'default'} disabled={anchorLocked} onClick={() => changeAnchor({ hz })}>{hz}</Button>)}</Group>
    {referenceKey}</Stack>, 'reference-module', 'anchor');
  const fine = module('Fine offset', <Stack gap="xs"><NumberControl label="Fine offset" value={anchor.offset} unit="¢" step={.1} disabled={anchorLocked} change={offset => changeAnchor({ offset })} assign={() => assign(`tuning/${scope.id}/fine`)}/>
    <Button size="compact-sm" variant="subtle" disabled={anchorLocked || anchor.offset === 0} onClick={() => changeAnchor({ offset: 0 })}>Reset offset</Button></Stack>, 'fine-module');

  const periodChoice = period === null ? 'none' : Math.abs(period - 1200) < 1e-6 ? 'octave' : Math.abs(period - ratioToCents(3)) < 1e-6 ? 'triple' : 'custom';
  const startKey = (shape.system === 'steps' || (shape.system === 'scale' && !shape.kbm)) && <div><Text size="xs" c="dimmed">Step 1 key</Text>
    <NumberControl label="Step 1 key" value={shape.startKey} unit={keyLabel(shape.startKey)} min={finite ? Math.max(0, anchor.key - count + 1) : 0} max={finite ? anchor.key : 127} disabled={scaleLocked}
      change={value => changeShape({ ...shape, startKey: Math.round(value) } as Shape)} assign={() => assign(`tuning/${scope.id}/startKey`)}/></div>;
  const fifths = twelve && widestFifth(shape);
  const systemChoices = [{ value: 'temperament', label: 'Twelve-note' }, { value: 'equal', label: 'Equal division' }, { value: 'steps', label: 'Pitch collection' }, ...(shape.system === 'scale' ? [{ value: 'scale', label: 'Scala file' }] : [])];
  const scale = module('Scale', <Stack gap="xs">
    <Group gap="xs" align="end" className="tuning-system-row"><Select label="Tuning system" value={shape.system} disabled={scaleLocked} allowDeselect={false} data={systemChoices} onChange={value => value && value !== shape.system && chooseSystem(value)}/>
      {importScala && <Button variant="default" disabled={scaleLocked} onClick={importScala}>Import Scala</Button>}</Group>
    {shape.system === 'temperament' && <>
      <Select label="Temperament" disabled={scaleLocked} allowDeselect={false} value={shape.temperament} onChange={value => value && chooseTemperament(value)}
        data={[...(recorded ? [{ value: RECORDED, label: temperamentName(RECORDED) }] : []), ...temperaments.map(t => ({ value: t.id, label: t.name })), { value: CUSTOM, label: temperamentName(CUSTOM) }]}/>
      {temperaments.some(t => t.id === shape.temperament) && shape.temperament !== 'equal'
        ? <Select label="Root note" data={pitchNames} value={pitchNames[shape.root]} disabled={scaleLocked} allowDeselect={false}
          onChange={value => { if (value) { const root = pitchNames.indexOf(value); setSelectedStep(0); changeShape({ ...shape, root, offsets: presetOffsets(shape.temperament, root) }); } }}/>
        : <div/>}
      {fifths && <Text size="xs" c="dimmed" className="tuning-fifths">{fifths.wolf ? `Wolf ${fifths.from}–${fifths.to} · ${Number(fifths.cents.toFixed(1))} ¢`
        : Math.abs(fifths.largest - fifths.smallest) < .05 ? `Fifths ${Number(fifths.largest.toFixed(1))} ¢` : `Fifths ${Number(fifths.smallest.toFixed(1))}–${Number(fifths.largest.toFixed(1))} ¢`}</Text>}
    </>}
    {(shape.system === 'equal' || shape.system === 'steps') && <>
      <Select label="Repeat interval" value={periodChoice} disabled={scaleLocked} allowDeselect={false}
        data={[...(shape.system === 'steps' ? [{ value: 'none', label: 'None · no repetition' }] : []), { value: 'octave', label: '2:1 · 1200 ¢' }, { value: 'triple', label: '3:1 · 1901.96 ¢' }, { value: 'custom', label: 'Custom interval' }]}
        onChange={value => { if (!value) return; const next = value === 'none' ? null : value === 'octave' ? 1200 : value === 'triple' ? ratioToCents(3) : 1300; changeShape({ ...shape, period: next } as Shape); }}/>
      {shape.system === 'equal' ? <div><Text size="xs" c="dimmed">Steps per repeat</Text><NumberControl label="Steps per repeat" value={shape.steps} min={1} max={311} disabled={scaleLocked}
        change={value => changeShape({ ...shape, steps: Math.round(value) })} assign={() => assign(`tuning/${scope.id}/steps`)}/></div>
        : <Group gap="xs"><Text size="xs">{count} steps</Text>
          <Button size="compact-sm" variant="default" disabled={scaleLocked || count >= 128} onClick={() => { changeShape({ ...shape, intervals: [...shape.intervals, shape.intervals.at(-1)! + 100] }); setSelectedStep(count); }}>Add step</Button>
          <Button size="compact-sm" variant="subtle" color="red" disabled={scaleLocked || count <= 1 || (finite && anchor.key === keyRange![1])} onClick={() => { changeShape({ ...shape, intervals: shape.intervals.slice(0, -1) }); setSelectedStep(Math.min(step, count - 2)); }}>Remove last</Button></Group>}
      {periodChoice === 'custom' && <div><Text size="xs" c="dimmed">Repeat size</Text><NumberControl label="Repeat size" value={period!} unit="¢" min={.01} step={.1} disabled={scaleLocked} change={value => changeShape({ ...shape, period: value } as Shape)} assign={() => assign(`tuning/${scope.id}/period`)}/></div>}
      {startKey}
    </>}
    {shape.system === 'scale' && <><Text fw={600}>{shape.name}</Text><Text size="xs" c="dimmed">{[shape.scl, shape.kbm].filter(Boolean).map(path => path!.split(/[\\/]/).at(-1)).join(' · ')}</Text>
      <Text size="xs">{count} steps · Repeat {Number(shape.period.toFixed(2))} ¢</Text>{startKey}</>}
  </Stack>, 'scale-module', 'scale');

  const noteEditor = module('Selected note', <Stack gap="xs"><Group justify="space-between"><Text className="tuning-selected-note" fw={600}>{twelve ? labels[step] : `Step ${step + 1}`}</Text><Group gap={4}>
    <ActionIcon variant="default" size="lg" aria-label="Previous note" onClick={() => setSelectedStep((step + count - 1) % count)}><ChevronRight size={16} style={{ transform: 'rotate(180deg)' }}/></ActionIcon>
    <ActionIcon variant="default" size="lg" aria-label="Next note" onClick={() => setSelectedStep((step + 1) % count)}><ChevronRight size={16}/></ActionIcon></Group></Group>
    {twelve ? <>
      <NumberControl label={`${labels[step]} deviation`} value={values[step]} unit="¢" min={-50} max={50} step={.1} disabled={!editableNotes || step === fixedStep} change={value => changeNote(step, value)} assign={() => assign(`tuning/${scope.id}/deviation/${indices[step]}`)}/>
      <Text size="xs" c="dimmed">{step === fixedStep ? 'Reference note' : `From ${pitchNames[referenceClass]}`}</Text></>
      : shape.system === 'steps' ? <><NumberControl label={`Step ${step + 1} interval`} value={values[step]} unit="¢" step={.1} disabled={!editableNotes || step === 0} change={value => changeNote(step, value)} assign={() => assign(`tuning/${scope.id}/interval/${step}`)}/>
        <Text size="xs" c="dimmed">{step === 0 ? 'Step 1 · 0 ¢' : 'From step 1'}</Text></>
      : <><Text>{Number(values[step].toFixed(2))} ¢</Text><Text size="xs" c="dimmed">from step 1{shape.system === 'equal' ? ` · ${Number((period! / count).toFixed(2))} ¢ spacing` : ''}</Text></>}
  </Stack>, 'note-module');

  const graph = <TuningGraph key={`${scope.id}-${shape.system}-${twelve ? shape.root : ''}-${period}`} values={values} labels={labels} selected={step} select={setSelectedStep}
    intervals={!twelve} lower={lower} upper={upper} fixed={fixedStep} editable={editableNotes} change={changeNote}
    caption={twelve ? `±50 ¢ from ${pitchNames[referenceClass]}` : undefined}/>;
  const pitchOf = (key: number) => keyHz(shape, anchor, key);
  const mappingKey = keyRange ? keyRange[0] + step : anchor.key + step;
  const mappingHz = pitchOf(mappingKey);
  const mapping = !twelve && <Group className="tuning-mapping" justify="space-between"><Text size="xs" c="dimmed">Key {mappingKey} {keyLabel(mappingKey)} → step {step + 1} · {mappingHz === undefined ? 'silent' : `${Number(mappingHz.toFixed(2))} Hz`}</Text>
    <Text size="xs" c="dimmed">{finite ? `Keys ${keyRange![0]}–${keyRange![1]} only · outside unmapped` : shape.system === 'scale' && shape.kbm ? 'Mapped by keyboard file' : `Consecutive keys · repeat every ${count} steps`}</Text></Group>;
  const summary = <Group className="tuning-status" justify="space-between"><Text size="xs" c="dimmed">{twelve ? `${temperamentName(shape.temperament)}${shape.temperament === RECORDED || shape.temperament === CUSTOM ? '' : ` · root ${pitchNames[shape.root]}`}` : `${count} ${shape.system === 'equal' ? 'equal ' : ''}steps · ${period === null ? 'No repetition' : `Repeat ${Number(period.toFixed(2))} ¢`}`}</Text>
    <Text size="xs" c="dimmed">{noteName(anchor.key)} = {Number(anchor.hz.toFixed(2))} Hz · {formatCents(anchor.offset)} ¢ offset</Text></Group>;

  const visible = (s: DeskScope): boolean => {
    if (query) return s.name.toLocaleLowerCase().includes(query.toLocaleLowerCase());
    const up = scopes.find(p => p.id === s.parent);
    return !up || (!collapsed.includes(up.id) && visible(up));
  };
  const depth = (s: DeskScope): number => s.parent ? 1 + depth(scopes.find(p => p.id === s.parent)!) : 0;
  const origin = (part: Part, label: string) => {
    const from = governing(part);
    return from.id === scope.id ? <Text size="xs" className="modified">Own {label} ◇</Text>
      : <Button variant="subtle" size="compact-xs" onClick={() => select(from.id)}>{label[0].toUpperCase() + label.slice(1)} · {from.name} ↗</Button>;
  };

  return <div className={`tuning-desk tuning-${layout}${browser ? '' : ' tuning-no-browser'}`}>
    {browser && <nav className="tuning-browser" aria-label="Tuning scopes">
      <Text fw={600} mb="sm">Scopes</Text><TextInput aria-label="Find scope" placeholder="Find scope" leftSection={<Search size={14}/>} value={query} onChange={e => setQuery(e.currentTarget.value)} mb="sm"/>
      <div className="tuning-scope-list">{scopes.filter(visible).map(s => {
        const children = scopes.some(child => child.parent === s.id);
        return <div key={s.id} className="tuning-scope-row" style={{ paddingLeft: query ? 0 : depth(s) * 10 }}>
          {children ? <ActionIcon size="sm" variant="subtle" color="gray" aria-label={`${collapsed.includes(s.id) ? 'Expand' : 'Collapse'} ${s.name}`} onClick={() => setCollapsed(ids => ids.includes(s.id) ? ids.filter(id => id !== s.id) : [...ids, s.id])}>{collapsed.includes(s.id) ? <ChevronRight size={14}/> : <ChevronDown size={14}/>}</ActionIcon> : <span className="scope-indent"/>}
          <Button variant={scope.id === s.id ? 'light' : 'subtle'} color={scope.id === s.id ? undefined : 'gray'} className="tuning-scope-button" aria-label={s.name} aria-pressed={scope.id === s.id} onClick={() => { select(s.id); setSelectedStep(0); }}>
            <span>{s.name}{s.parent && (s.own.anchor || s.own.scale) && <span className="modified" aria-label="Tuning override"> ◇</span>}</span><span className="scope-detail">{Number(s.anchor.hz.toFixed(2))} Hz · {shapeLabel(s.shape)}</span>
          </Button></div>;
      })}{!scopes.some(visible) && <Text c="dimmed" size="xs">No matching scopes</Text>}</div>
    </nav>}
    <Card withBorder padding={0} className="tuning-device">
      <div className="tuning-device-header"><Group justify="space-between" gap="sm"><div><Group gap={4} className="tuning-path">{ancestors(scopes, scope).map(s => <Button key={s.id} variant="subtle" color="gray" size="compact-xs" onClick={() => select(s.id)}>{s.name} ›</Button>)}</Group><Text fw={600} className="tuning-scope-name">{scope.name}</Text></div>
        {undo && <Button variant="default" disabled={readOnly || !canUndo} onClick={undo}>Undo</Button>}</Group>
        <Group mt="xs" gap="xs">{parent ? <>{origin('anchor', 'pitch')}{origin('scale', 'scale')}</> : <Badge variant="outline" color="gray">Instrument tuning</Badge>}</Group>
      </div>
      {layout === 'channel' && <><div className="tuning-channel-controls">{reference}{scale}{fine}</div>{graph}<div className="tuning-note-dock">{noteEditor}</div></>}
      {layout === 'rack' && <><div className="tuning-rack-controls">{reference}{scale}<div className="tuning-rack-trims">{fine}{noteEditor}</div></div>{graph}</>}
      {layout === 'inspector' && <div className="tuning-editor-inspector"><div className="tuning-editor-main">{graph}<div className="tuning-note-dock">{noteEditor}</div></div><aside className="tuning-properties">{reference}{scale}{fine}</aside></div>}
      {layout === 'tabbed' && <Tabs value={tab} onChange={setTab} keepMounted={false}><Tabs.List grow><Tabs.Tab value="pitch">Pitch</Tabs.Tab><Tabs.Tab value="scale">Scale</Tabs.Tab><Tabs.Tab value="note">Note</Tabs.Tab></Tabs.List>
        <Tabs.Panel value="pitch"><div className="tuning-pitch-page">{reference}{fine}</div>{graph}</Tabs.Panel>
        <Tabs.Panel value="scale"><div className="tuning-scale-page">{scale}{graph}</div></Tabs.Panel>
        <Tabs.Panel value="note">{graph}<div className="tuning-note-dock">{noteEditor}</div></Tabs.Panel>
      </Tabs>}
      {mapping}{summary}
    </Card>
  </div>;
}

function ancestors(scopes: DeskScope[], scope: DeskScope) {
  const path: DeskScope[] = [];
  for (let current = scopes.find(s => s.id === scope.parent); current; current = scopes.find(s => s.id === current!.parent)) path.unshift(current);
  return path;
}
