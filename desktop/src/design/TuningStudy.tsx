import { useState } from 'react';
import { Button, Card, Group, Select, Slider, Stack, Switch, Table, Text } from '@mantine/core';
import { NumberControl } from './NumberControl';
import { Keyboard } from './BuildStudy';
import { initialScopes, resolve, type Scope, type Tuning } from './model';

export const tuningLayouts = [
  { value: 'tree', label: '1 · Scope and card' }, { value: 'table', label: '2 · Scope table' },
  { value: 'cascade', label: '3 · Inheritance columns' }, { value: 'keyboard', label: '4 · Keyboard focus' },
];

export function TuningStudy({ layout, assign }: { layout: string; assign: (name: string) => void }) {
  const [scopes, setScopes] = useState<Scope[]>(initialScopes);
  const [history, setHistory] = useState<Scope[][]>([]);
  const [selected, setSelected] = useState('instrument');
  const [advanced, setAdvanced] = useState(false);
  const [held, setHeld] = useState<number>();
  const scope = scopes.find(s => s.id === selected)!;
  const tuning = resolve(scopes, selected);
  const inherited = !scope.own;
  const parent = scopes.find(s => s.id === scope.parent);
  const edit = (next: Scope[]) => { setHistory([...history, scopes]); setScopes(next); };
  const update = (change: Partial<Tuning>, id = selected) => edit(scopes.map(s => s.id === id ? { ...s, own: { ...resolve(scopes, id), ...change } } : s));
  const number = (id: string, field: 'hz' | 'fine' | 'steps', label: string, unit: string, disabled = false) => <NumberControl
    label={label} value={resolve(scopes, id)[field]} unit={unit} min={field === 'hz' ? 1 : field === 'steps' ? 1 : -Infinity}
    max={field === 'steps' ? 128 : Infinity} step={field === 'fine' ? .1 : 1} disabled={disabled}
    change={value => update({ [field]: value }, id)} assign={() => assign(`tuning/${id}/${field}`)}/>;
  const scopeButton = (s: Scope) => <Button key={s.id} justify="start" variant={selected === s.id ? 'filled' : 'default'} onClick={() => setSelected(s.id)}>
    {s.name}{s.parent && s.own && <span className="modified" aria-label="Tuning override"> ◇</span>}</Button>;
  const fields = <Stack>
    <Group justify="space-between"><Text fw={600}>{scope.name}</Text>{parent && <Switch label={`Follow ${parent.name}`} checked={inherited} onChange={e => {
      edit(scopes.map(s => s.id === selected ? { ...s, own: e.currentTarget.checked ? undefined : structuredClone(tuning) } : s));
    }}/>}</Group>
    <Group align="start"><div><Text>Reference pitch</Text>{number(selected, 'hz', 'Reference pitch', 'Hz', inherited)}</div>
      <Select label="Temperament" data={['Equal', 'As recorded', 'Custom']} value={tuning.temperament} disabled={inherited} onChange={value => value && update({ temperament: value, deviations: value === 'Equal' ? Array(12).fill(0) : tuning.deviations })}/></Group>
    <Button variant="subtle" w="fit-content" onClick={() => setAdvanced(!advanced)}>{advanced ? 'Less' : 'Advanced'}</Button>
    {advanced && <Group align="end"><Select label="Tuning system" value={tuning.system} disabled={inherited} data={['Twelve-note', 'Equal division']} onChange={value => value && update({ system: value })}/>
      {tuning.system === 'Equal division' && <div><Text>Steps per octave</Text>{number(selected, 'steps', 'Steps per octave', '', inherited)}</div>}
      <Select label="Root note" disabled={inherited} value={tuning.root} data={['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B']} onChange={value => value && update({ root: value })}/>
      <div><Text>Fine offset</Text>{number(selected, 'fine', 'Fine offset', '¢', inherited)}</div></Group>}
  </Stack>;
  const bars = <Card withBorder><Text mb="md">{tuning.system === 'Equal division' ? `${tuning.steps} steps` : 'Deviation (¢)'}</Text>
    <div className="tuning-bars">{Array.from({ length: tuning.system === 'Equal division' ? tuning.steps : 12 }, (_, i) => {
      const cents = tuning.system === 'Equal division' ? Number((1200 * i / tuning.steps).toFixed(1)) : tuning.deviations[i];
      const active = held !== undefined && held % (tuning.system === 'Equal division' ? tuning.steps : 12) === i;
      return <div className={`tuning-bar ${active ? 'sounding' : ''}`} key={i}>
        <div className="bar-track" onPointerDown={e => {
          if (inherited || tuning.temperament !== 'Custom' || tuning.system === 'Equal division') return;
          e.currentTarget.setPointerCapture(e.pointerId);
          const rect = e.currentTarget.getBoundingClientRect();
          const deviations = [...tuning.deviations]; deviations[i] = Math.round(50 - (e.clientY - rect.top) / rect.height * 100);
          update({ deviations });
        }} onPointerMove={e => {
          if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
          const rect = e.currentTarget.getBoundingClientRect(); const deviations = [...tuning.deviations];
          deviations[i] = Math.max(-50, Math.min(50, Math.round(50 - (e.clientY - rect.top) / rect.height * 100))); update({ deviations });
        }}><div className="bar-zero"/><div className="bar-fill" style={tuning.system === 'Equal division' ? { bottom: 0, height: `${cents / 1200 * 100}%` } : { bottom: `${Math.min(50, 50 + cents)}%`, height: `${Math.max(.7, Math.abs(cents))}%` }}/></div>
        <Text>{tuning.system === 'Equal division' ? i + 1 : ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'][i]}</Text><Text c="dimmed">{cents}</Text>
        {tuning.temperament === 'Custom' && tuning.system === 'Twelve-note' && <Slider aria-label={`Step ${i + 1} cents`} value={cents} min={-50} max={50} disabled={inherited} onChange={value => { const deviations = [...tuning.deviations]; deviations[i] = value; update({ deviations }); }}/>}</div>;
    })}</div>
  </Card>;
  return <Stack><Group justify="space-between"><Text fw={600}>Tuning</Text><Button variant="default" disabled={!history.length} onClick={() => { setScopes(history.at(-1)!); setHistory(history.slice(0, -1)); }}>Undo</Button></Group>
    {layout === 'tree' && <div className="scope-and-card"><Stack>{scopes.map(s => <div key={s.id} style={{ paddingLeft: s.parent ? s.parent === 'instrument' ? 12 : s.parent === 'great' || s.parent === 'recit' ? 24 : 36 : 0 }}>{scopeButton(s)}</div>)}</Stack><Stack><Card withBorder>{fields}</Card>{bars}</Stack></div>}
    {layout === 'table' && <><Table.ScrollContainer minWidth={760}><Table withTableBorder withColumnBorders><Table.Thead><Table.Tr>{['Scope', 'Follows', 'Reference', 'Temperament', 'Fine offset'].map(label => <Table.Th key={label}>{label}</Table.Th>)}</Table.Tr></Table.Thead><Table.Tbody>{scopes.map(s => <Table.Tr key={s.id}><Table.Td>{scopeButton(s)}</Table.Td><Table.Td>{s.parent ? s.own ? 'Own tuning ◇' : scopes.find(p => p.id === s.parent)?.name : 'Instrument'}</Table.Td><Table.Td>{number(s.id, 'hz', `${s.name} reference pitch`, 'Hz', !s.own)}</Table.Td><Table.Td>{resolve(scopes, s.id).temperament}</Table.Td><Table.Td>{number(s.id, 'fine', `${s.name} fine offset`, '¢', !s.own)}</Table.Td></Table.Tr>)}</Table.Tbody></Table></Table.ScrollContainer><Card withBorder>{fields}</Card>{bars}</>}
    {layout === 'cascade' && <><div className="inheritance-columns">{[
      { name: 'Instrument', list: scopes.filter(s => !s.parent) },
      { name: 'Divisions', list: scopes.filter(s => s.parent === 'instrument') },
      { name: 'Stops', list: scopes.filter(s => ['great', 'recit'].includes(s.parent ?? '')) },
      { name: 'Pipes', list: scopes.filter(s => s.parent === 'bourdon') },
    ].map(column => <Card withBorder key={column.name}><Text c="dimmed" mb="md">{column.name}</Text><Stack>{column.list.map(scopeButton)}</Stack></Card>)}</div><div className="key-editor"><Card withBorder>{fields}</Card>{bars}</div></>}
    {layout === 'keyboard' && <><Group>{scopes.map(scopeButton)}</Group>{bars}<Keyboard held={held} overrides={[]} change={setHeld}/><Card withBorder>{fields}</Card></>}
    {layout !== 'keyboard' && <Keyboard held={held} overrides={[]} change={setHeld}/>}
  </Stack>;
}
