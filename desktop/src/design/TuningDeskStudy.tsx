import { useState, type ReactNode } from 'react';
import { ActionIcon, Badge, Button, Card, Group, Select, Stack, Switch, Tabs, Text, TextInput } from '@mantine/core';
import { ChevronDown, ChevronRight, Search } from 'lucide-react';
import { NumberControl } from './NumberControl';
import { initialScopes, resolve, type Scope, type Tuning } from './model';
import { formatCents, pitchNames, TuningGraph } from './TuningGraph';
import './tuning-desk.css';

export const tuningDeskLayouts = [
  { value: 'channel', label: '1 · Channel strip' }, { value: 'rack', label: '2 · Device rack' },
  { value: 'inspector', label: '3 · Editor + inspector' }, { value: 'tabbed', label: '4 · Tabbed device' },
];

export function TuningDeskStudy({ layout, assign }: { layout: string; assign: (name: string) => void }) {
  const [scopes, setScopes] = useState<Scope[]>(initialScopes);
  const [history, setHistory] = useState<Scope[][]>([]);
  const [selected, setSelected] = useState('instrument');
  const [selectedStep, setSelectedStep] = useState(0);
  const [query, setQuery] = useState('');
  const [collapsed, setCollapsed] = useState<string[]>(['bourdon']);
  const [tab, setTab] = useState<string | null>('pitch');
  const scope = scopes.find(s => s.id === selected)!;
  const tuning = resolve(scopes, selected);
  const inherited = !scope.own;
  const parent = scopes.find(s => s.id === scope.parent);
  const equalDivision = tuning.system === 'Equal division';
  const count = equalDivision ? tuning.steps : 12;
  const step = Math.min(selectedStep, count - 1);
  const root = pitchNames.indexOf(tuning.root);
  const indices = Array.from({ length: count }, (_, i) => equalDivision ? i : (i + root) % 12);
  const labels = indices.map(i => equalDivision ? `${i + 1}` : pitchNames[i]);
  const values = indices.map(i => equalDivision ? 1200 * i / count : tuning.deviations[i]);
  const path: Scope[] = [];
  for (let current: Scope | undefined = scope; current; current = scopes.find(s => s.id === current?.parent)) path.unshift(current);
  const governing = [...path].reverse().find(s => s.own)!;
  const edit = (next: Scope[]) => { setHistory(h => [...h, scopes]); setScopes(next); };
  const update = (change: Partial<Tuning>) => {
    if (inherited) return;
    edit(scopes.map(s => s.id === selected ? { ...s, own: { ...tuning, ...change } } : s));
  };
  const changeNote = (index: number, value: number) => {
    if (inherited || equalDivision || tuning.temperament !== 'Custom') return;
    const deviations = [...tuning.deviations]; deviations[indices[index]] = value;
    update({ deviations });
  };
  const number = (field: 'hz' | 'fine' | 'steps', label: string, unit: string) => <NumberControl label={label} value={tuning[field]} unit={unit}
    min={field === 'fine' ? -Infinity : 1} max={field === 'steps' ? 128 : Infinity} step={field === 'fine' ? .1 : 1} disabled={inherited}
    change={value => update({ [field]: field === 'steps' ? Math.round(value) : value })} assign={() => assign(`tuning/${selected}/${field}`)}/>;
  const module = (name: string, content: ReactNode, className = '') => <section className={`tuning-module ${className}`} aria-label={name}><Text className="tuning-module-title" size="xs" c="dimmed">{name}</Text>{content}</section>;
  const reference = module('Reference', <Stack gap="xs"><div className="tuning-reference-number">{number('hz', 'Reference pitch', 'Hz')}</div>
    <Group gap={4} grow>{[392, 415, 440, 466].map(hz => <Button key={hz} size="compact-sm" variant={tuning.hz === hz ? 'light' : 'default'} disabled={inherited} onClick={() => update({ hz })}>{hz}</Button>)}</Group>
    <Text size="xs" c="dimmed">{equalDivision ? 'Reference key · A4 position' : 'Reference note · A4'}</Text></Stack>, 'reference-module');
  const fine = module('Fine offset', <Stack gap="xs">{number('fine', 'Fine offset', '¢')}<Button size="compact-sm" variant="subtle" disabled={inherited || tuning.fine === 0} onClick={() => update({ fine: 0 })}>Reset offset</Button></Stack>, 'fine-module');
  const scale = module('Scale', <Stack gap="xs">
    <Select label="Tuning system" value={tuning.system} disabled={inherited} allowDeselect={false} data={['Twelve-note', 'Equal division']} onChange={value => { if (value) { setSelectedStep(0); update({ system: value }); } }}/>
    {equalDivision ? <div><Text size="xs" c="dimmed">Steps per octave</Text>{number('steps', 'Steps per octave', '')}</div> : <Select label="Temperament" data={['Equal', 'Custom']} value={tuning.temperament} disabled={inherited} allowDeselect={false} onChange={value => value && update({ temperament: value, deviations: value === 'Equal' ? Array(12).fill(0) : tuning.deviations })}/>}
    {!equalDivision && <Select label="Root note" data={pitchNames} value={tuning.root} disabled={inherited} allowDeselect={false} onChange={value => { if (value) { setSelectedStep(0); update({ root: value }); } }}/>}
  </Stack>, 'scale-module');
  const noteEditor = module('Selected note', <Stack gap="xs"><Group justify="space-between"><Text className="tuning-selected-note" fw={600}>{equalDivision ? `Step ${step + 1}` : labels[step]}</Text><Group gap={4}>
    <ActionIcon variant="default" size="lg" aria-label="Previous note" onClick={() => setSelectedStep((step + count - 1) % count)}><ChevronRight size={16} style={{ transform: 'rotate(180deg)' }}/></ActionIcon>
    <ActionIcon variant="default" size="lg" aria-label="Next note" onClick={() => setSelectedStep((step + 1) % count)}><ChevronRight size={16}/></ActionIcon></Group></Group>
    {equalDivision ? <><Text>{Number(values[step].toFixed(2))} ¢</Text><Text size="xs" c="dimmed">from first step · {Number((1200 / count).toFixed(2))} ¢ spacing</Text></> : <>
      <NumberControl label={`${labels[step]} deviation`} value={values[step]} unit="¢" min={-50} max={50} step={.1} disabled={inherited || tuning.temperament !== 'Custom'} change={value => changeNote(step, value)} assign={() => assign(`tuning/${selected}/deviation/${indices[step]}`)}/>
      <Button variant="subtle" size="compact-sm" disabled={inherited || tuning.temperament !== 'Custom' || values[step] === 0} onClick={() => changeNote(step, 0)}>Reset note</Button></>}
  </Stack>, 'note-module');
  const graph = <TuningGraph key={`${selected}-${tuning.system}-${tuning.root}`} values={values} labels={labels} selected={step} select={setSelectedStep}
    equalDivision={equalDivision} editable={!inherited && !equalDivision && tuning.temperament === 'Custom'} change={changeNote}/>;
  const summary = <Group className="tuning-status" justify="space-between"><Text size="xs" c="dimmed">{equalDivision ? `${count} equal steps · ${Number((1200 / count).toFixed(2))} ¢ apart` : `${tuning.temperament} · root ${tuning.root}`}</Text><Text size="xs" c="dimmed">{tuning.hz} Hz · {formatCents(tuning.fine)} ¢ offset</Text></Group>;
  const visible = (s: Scope): boolean => {
    if (query) return s.name.toLocaleLowerCase().includes(query.toLocaleLowerCase());
    const parent = scopes.find(p => p.id === s.parent);
    return !parent || (!collapsed.includes(parent.id) && visible(parent));
  };
  const depth = (s: Scope): number => s.parent ? 1 + depth(scopes.find(p => p.id === s.parent)!) : 0;

  return <div className={`tuning-desk tuning-${layout}`}>
    <nav className="tuning-browser" aria-label="Tuning scopes">
      <Text fw={600} mb="sm">Scopes</Text><TextInput aria-label="Find scope" placeholder="Find scope" leftSection={<Search size={14}/>} value={query} onChange={e => setQuery(e.currentTarget.value)} mb="sm"/>
      <div className="tuning-scope-list">{scopes.filter(visible).map(s => {
        const children = scopes.some(child => child.parent === s.id);
        const resolved = resolve(scopes, s.id);
        return <div key={s.id} className="tuning-scope-row" style={{ paddingLeft: query ? 0 : depth(s) * 10 }}>
          {children ? <ActionIcon size="sm" variant="subtle" color="gray" aria-label={`${collapsed.includes(s.id) ? 'Expand' : 'Collapse'} ${s.name}`} onClick={() => setCollapsed(ids => ids.includes(s.id) ? ids.filter(id => id !== s.id) : [...ids, s.id])}>{collapsed.includes(s.id) ? <ChevronRight size={14}/> : <ChevronDown size={14}/>}</ActionIcon> : <span className="scope-indent"/>}
          <Button variant={selected === s.id ? 'light' : 'subtle'} color={selected === s.id ? undefined : 'gray'} className="tuning-scope-button" aria-label={s.name} aria-pressed={selected === s.id} onClick={() => { setSelected(s.id); setSelectedStep(0); }}>
            <span>{s.name}{s.parent && s.own && <span className="modified" aria-label="Tuning override"> ◇</span>}</span><span className="scope-detail">{resolved.hz} Hz · {s.parent ? s.own ? 'Own tuning' : 'Following' : 'Instrument'}</span>
          </Button></div>;
      })}{!scopes.some(visible) && <Text c="dimmed" size="xs">No matching scopes</Text>}</div>
    </nav>
    <Card withBorder padding={0} className="tuning-device">
      <div className="tuning-device-header"><Group justify="space-between" gap="sm"><div><Group gap={4} className="tuning-path">{path.slice(0, -1).map(s => <Button key={s.id} variant="subtle" color="gray" size="compact-xs" onClick={() => setSelected(s.id)}>{s.name} ›</Button>)}</Group><Text fw={600} className="tuning-scope-name">{scope.name}</Text></div><Button variant="default" disabled={!history.length} onClick={() => { setScopes(history.at(-1)!); setHistory(history.slice(0, -1)); }}>Undo</Button></Group>
        <Group justify="space-between" mt="xs" gap="xs">{parent ? <Switch label={`Follow ${parent.name}`} checked={inherited} onChange={e => { const follow = e.currentTarget.checked; edit(scopes.map(s => s.id === selected ? { ...s, own: follow ? undefined : structuredClone(tuning) } : s)); }}/> : <Badge variant="outline" color="gray">Instrument tuning</Badge>}
          {inherited ? <Button variant="subtle" size="compact-sm" onClick={() => setSelected(governing.id)}>Edit {governing.name} ↗</Button> : parent && <Text size="xs" className="modified">Own tuning ◇</Text>}</Group>
      </div>
      {layout === 'channel' && <><div className="tuning-channel-controls">{reference}{scale}{fine}</div>{graph}<div className="tuning-note-dock">{noteEditor}</div></>}
      {layout === 'rack' && <><div className="tuning-rack-controls">{reference}{scale}<div className="tuning-rack-trims">{fine}{noteEditor}</div></div>{graph}</>}
      {layout === 'inspector' && <div className="tuning-editor-inspector"><div className="tuning-editor-main">{graph}<div className="tuning-note-dock">{noteEditor}</div></div><aside className="tuning-properties">{reference}{scale}{fine}</aside></div>}
      {layout === 'tabbed' && <Tabs value={tab} onChange={setTab} keepMounted={false}><Tabs.List grow><Tabs.Tab value="pitch">Pitch</Tabs.Tab><Tabs.Tab value="scale">Scale</Tabs.Tab><Tabs.Tab value="note">Note</Tabs.Tab></Tabs.List>
        <Tabs.Panel value="pitch"><div className="tuning-pitch-page">{reference}{fine}</div>{graph}</Tabs.Panel>
        <Tabs.Panel value="scale"><div className="tuning-scale-page">{scale}{graph}</div></Tabs.Panel>
        <Tabs.Panel value="note">{graph}<div className="tuning-note-dock">{noteEditor}</div></Tabs.Panel>
      </Tabs>}
      {summary}
    </Card>
  </div>;
}
