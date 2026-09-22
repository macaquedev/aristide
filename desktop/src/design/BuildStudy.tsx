import { useState } from 'react';
import { Button, Card, Checkbox, Drawer, Group, Select, Stack, Table, Text } from '@mantine/core';
import { NumberControl } from './NumberControl';
import { initialBuild, noteName, outputs, sources, type BuildState, type Voice } from './model';

export const buildLayouts = [
  { value: 'rows', label: '1 · Voice rows' }, { value: 'steps', label: '2 · Step grid' },
  { value: 'roll', label: '3 · Piano roll' }, { value: 'keys', label: '4 · Per-key view' },
];
export function BuildStudy({ layout, compact, assign }: { layout: string; compact: boolean; assign: (name: string) => void }) {
  const [model, setModel] = useState<BuildState>(initialBuild);
  const [history, setHistory] = useState<BuildState[]>([]);
  const [held, setHeld] = useState<number>();
  const [selected, setSelected] = useState('one');
  const [selection, setSelection] = useState<string[]>([]);
  const [sourceFor, setSourceFor] = useState<string>();
  const edit = (next: BuildState) => { setHistory([...history, model]); setModel(next); };
  const update = (id: string, change: Partial<Voice>) => {
    const ids = selection.includes(id) ? selection : [id];
    if (held !== undefined) {
      const keys = { ...model.keys, [held]: { ...model.keys[held] } };
      for (const selected of ids) keys[held][selected] = { ...keys[held][selected], ...change };
      edit({ ...model, keys });
    } else edit({ ...model, voices: model.voices.map(voice => ids.includes(voice.id) ? { ...voice, ...change } : voice) });
  };
  const voices = model.voices.map(voice => ({ ...voice, ...(held === undefined ? {} : model.keys[held]?.[voice.id]) }));
  const voice = voices.find(voice => voice.id === selected) ?? voices[0];
  const number = (v: Voice, field: 'pitch' | 'delay' | 'level' | 'low' | 'high', label: string, unit: string) =>
    <NumberControl label={`Voice ${voices.indexOf(v) + 1} ${label}`} value={v[field]} unit={unit} step={field === 'pitch' ? 100 : 1}
      min={field === 'delay' ? 0 : field === 'low' ? 0 : field === 'high' ? v.low : -Infinity}
      max={field === 'low' ? v.high : field === 'high' ? 4095 : Infinity}
      change={value => update(v.id, { [field]: value })} assign={() => assign(`stop/study/voice/${v.id}/${field}${held === undefined ? '' : `/pipe/${held}`}`)}/>;
  const fields = (v: Voice) => <>
    <div><Text c="dimmed">Source</Text><Button variant="default" onClick={() => setSourceFor(v.id)}>{v.source}</Button></div>
    <div><Text c="dimmed">Pitch</Text>{number(v, 'pitch', 'pitch', '¢')}</div>
    <div><Text c="dimmed">Delay</Text>{number(v, 'delay', 'delay', 'ms')}</div>
    <div><Text c="dimmed">Level</Text>{number(v, 'level', 'level', 'dB')}</div>
    <Select label="Output" aria-label={`Voice ${voices.indexOf(v) + 1} output`} data={outputs} value={v.output} onChange={value => value && update(v.id, { output: value })}/>
    <div><Text c="dimmed">Key range</Text><Group gap="xs">{number(v, 'low', 'low key', '')}<Text>–</Text>{number(v, 'high', 'high key', '')}</Group></div>
    <Select label="Tuning" aria-label={`Voice ${voices.indexOf(v) + 1} tuning`} data={['Follow stop', 'Keep source']} value={v.tuning} onChange={value => value && update(v.id, { tuning: value })}/>
  </>;
  const inspector = voice && <Card withBorder><Group mb="md" justify="space-between"><Text fw={600}>Voice {voices.indexOf(voice) + 1}</Text><Button color="red" variant="subtle" disabled={voices.length === 1} onClick={() => { edit({ ...model, voices: model.voices.filter(v => v.id !== voice.id) }); setSelected(voices.find(v => v.id !== voice.id)!.id); }}>Remove voice</Button></Group><div className="voice-fields">{fields(voice)}</div></Card>;
  return <Stack className={compact ? 'study-compact' : ''}>
    <Group justify="space-between"><div><Text fw={600}>Bourdon variation <span className="modified">◇</span></Text><Text c="dimmed">Grand-orgue · {voices.length} voices per key</Text></div><Group><Select aria-label="Stop tuning" defaultValue="Follow division" data={['Follow division', 'Override']}/>
      <Button variant="default" disabled={!history.length} onClick={() => { setModel(history.at(-1)!); setHistory(history.slice(0, -1)); }}>Undo</Button>
      <Button onClick={() => { const added = { ...voice, id: crypto.randomUUID() }; edit({ ...model, voices: [...model.voices, added] }); setSelected(added.id); }}>Add voice</Button></Group></Group>
    <Group justify="space-between"><Text c={held === undefined ? 'dimmed' : 'violet'}>{held === undefined ? 'All keys' : `${noteName(held)} only`}</Text><Text c="dimmed" size="sm">Hold key to isolate</Text></Group>
    <Keyboard held={held} overrides={Object.keys(model.keys).map(Number)} change={setHeld}/>
    {layout === 'rows' && (compact ? <Table.ScrollContainer minWidth={1100}><Table withTableBorder withColumnBorders><Table.Thead><Table.Tr>{['Select', 'Source', 'Pitch', 'Delay', 'Level', 'Output', 'Keys', 'Tuning'].map(label => <Table.Th key={label}>{label}</Table.Th>)}</Table.Tr></Table.Thead>
      <Table.Tbody>{voices.map(v => <Table.Tr key={v.id}><Table.Td><Checkbox aria-label={`Select voice ${voices.indexOf(v) + 1}`} checked={selection.includes(v.id)} onChange={e => setSelection(e.currentTarget.checked ? [...selection, v.id] : selection.filter(id => id !== v.id))}/></Table.Td>
        <Table.Td><Button variant="default" onClick={() => setSourceFor(v.id)}>{v.source}</Button></Table.Td><Table.Td>{number(v, 'pitch', 'pitch', '¢')}</Table.Td><Table.Td>{number(v, 'delay', 'delay', 'ms')}</Table.Td><Table.Td>{number(v, 'level', 'level', 'dB')}</Table.Td>
        <Table.Td><Select aria-label="Output" data={outputs} value={v.output} onChange={output => output && update(v.id, { output })}/></Table.Td><Table.Td>{noteName(v.low)}–{noteName(v.high)}</Table.Td><Table.Td><Select aria-label="Tuning" data={['Follow stop', 'Keep source']} value={v.tuning} onChange={tuning => tuning && update(v.id, { tuning })}/></Table.Td></Table.Tr>)}</Table.Tbody></Table></Table.ScrollContainer>
      : voices.map((v, index) => <Card withBorder key={v.id}><Text fw={600} mb="sm">Voice {index + 1}</Text><div className="voice-fields">{fields(v)}</div></Card>))}
    {layout === 'steps' && <><div className="step-grid">{['Voice', ...Array.from({ length: 9 }, (_, i) => `${i * 120} ms`)].map((label, index) => <Text key={index} c="dimmed">{label}</Text>)}
      {voices.map((v, index) => <div className="step-row" key={v.id}><Button variant={voice.id === v.id ? 'filled' : 'default'} onClick={() => setSelected(v.id)}>{index + 1}</Button>{Array.from({ length: 9 }, (_, step) => <Button key={step} aria-label={`Voice ${index + 1} at ${step * 120} ms`} variant={Math.round(v.delay / 120) === step ? 'filled' : 'default'} onClick={() => { setSelected(v.id); update(v.id, { delay: step * 120 }); }}>{Math.round(v.delay / 120) === step ? `${v.pitch} ¢` : '·'}</Button>)}</div>)}</div>{inspector}</>}
    {layout === 'roll' && <><div className="piano-roll" role="group" aria-label="Pitch and delay piano roll">
      {Array.from({ length: 13 }, (_, i) => 1200 - i * 100).map(pitch => <div className="roll-row" key={pitch}><Text c="dimmed">{pitch} ¢</Text>{Array.from({ length: 9 }, (_, delay) => <button key={delay} aria-label={`Set selected voice to ${pitch} cents at ${delay * 120} milliseconds`} onClick={() => update(voice.id, { pitch, delay: delay * 120 })}>{voices.filter(v => Math.round(v.pitch / 100) * 100 === pitch && Math.round(v.delay / 120) === delay).map(v => <span key={v.id} className={voice.id === v.id ? 'roll-note selected' : 'roll-note'} onClick={e => { e.stopPropagation(); setSelected(v.id); }}>{voices.indexOf(v) + 1}</span>)}</button>)}</div>)}
      <div className="roll-row"><span/>{Array.from({ length: 9 }, (_, i) => <Text c="dimmed" key={i}>{i * 120} ms</Text>)}</div>
    </div>{inspector}</>}
    {layout === 'keys' && <div className="key-editor"><Card withBorder><Text fw={600} mb="md">{held === undefined ? 'Whole-stop rule' : `${noteName(held)} pipe rule`}</Text><Stack>{voices.map((v, index) => <Button key={v.id} variant={voice.id === v.id ? 'filled' : 'default'} onClick={() => setSelected(v.id)}>Voice {index + 1} · {v.pitch} ¢ · {v.delay} ms</Button>)}</Stack></Card>{inspector}</div>}
    <Drawer opened={Boolean(sourceFor)} onClose={() => setSourceFor(undefined)} title="Source" position="right" size="lg"><Stack>{sources.map(source => <Button variant="default" key={source} justify="start" onClick={() => { if (sourceFor) update(sourceFor, { source }); setSourceFor(undefined); }}>{source}</Button>)}</Stack></Drawer>
  </Stack>;
}

export function Keyboard({ held, overrides, change }: { held?: number; overrides: number[]; change: (key?: number) => void }) {
  return <div className="keyboard-strip" aria-label="Hold a key for per-pipe editing">{Array.from({ length: 25 }, (_, index) => index + 48).map(key => <button key={key} className={held === key ? 'held' : ''} aria-label={`Hold ${noteName(key)}`} aria-pressed={held === key}
    onPointerDown={e => { e.preventDefault(); e.currentTarget.setPointerCapture(e.pointerId); change(key); }} onPointerUp={() => change(undefined)} onPointerCancel={() => change(undefined)} onLostPointerCapture={() => change(undefined)}
    onKeyDown={e => { if (e.key === ' ' || e.key === 'Enter') { e.preventDefault(); change(key); } }} onKeyUp={() => change(undefined)} onBlur={() => change(undefined)}>
    {noteName(key)}{overrides.includes(key) && <span className="modified" aria-label="Pipe override">◇</span>}
  </button>)}</div>;
}
