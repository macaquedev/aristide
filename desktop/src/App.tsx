import { useEffect, useRef, useState } from 'react';
import { ActionIcon, Button, Card, Drawer, Group, Loader, Modal, SegmentedControl, Stack, Text, TextInput, Tooltip, useMantineColorScheme } from '@mantine/core';
import { ArrowLeft, ChevronLeft, ChevronRight, Folder, LockKeyhole, Settings, Undo2, UnlockKeyhole } from 'lucide-react';
import { endpoint, request, type Browse, type Snapshot, type Stop } from './api';
import { useEngine } from './engine';

const panels = ['Play', 'Build', 'Route', 'Tuning', 'Library'] as const;
type Panel = typeof panels[number] | 'Setup';
type Command = ReturnType<typeof useEngine>['command'];

export function App() {
  const engine = useEngine();
  const { snapshot: state, command } = engine;
  const [panel, setPanel] = useState<Panel>('Play');
  const [edit, setEdit] = useState(false);
  const [selected, setSelected] = useState<Stop>();
  const [density, setDensity] = useState(() => localStorage.getItem('aristide-density') ?? 'comfortable');
  const [dismissedLoadError, setDismissedLoadError] = useState<string>();
  const loading = useRef(false);
  useEffect(() => {
    if (state?.loading) loading.current = true;
    else if (loading.current) {
      loading.current = false;
      if (!state?.load_error) { setPanel('Play'); setEdit(false); }
    }
  }, [state?.loading, state?.load_error]);
  const error = engine.error ?? (state?.load_error !== dismissedLoadError ? state?.load_error : undefined);
  return <div className="app" data-density={density}>
    <header className="topbar">
      <Text className="instrument" fw={600} title={state?.organ}>{state?.organ ?? 'Aristide'}</Text>
      <nav aria-label="Panels">{panels.map(name => <Button key={name} variant={panel === name ? 'filled' : 'subtle'} color={panel === name ? undefined : 'gray'}
        disabled={name === 'Build' && !edit} onClick={() => setPanel(name)} aria-current={panel === name ? 'page' : undefined}>{name}</Button>)}</nav>
      <Tooltip label={edit ? 'Lock for performance' : 'Unlock to edit'}><Button variant="default" leftSection={edit ? <UnlockKeyhole size={18}/> : <LockKeyhole size={18}/>}
        onClick={() => { setEdit(!edit); if (edit && panel === 'Build') setPanel('Play'); }}>{edit ? 'Edit' : 'Perform'}</Button></Tooltip>
      <Tooltip label="Undo will be available with layer editing"><ActionIcon variant="default" size="lg" disabled aria-label="Undo"><Undo2 size={18}/></ActionIcon></Tooltip>
      <Text className="meters" c="dimmed" size="xs">CPU —<br/>{state?.memory ? `${state.memory.resident_mb} MB` : 'Memory —'}</Text>
      <Button color="red" variant="filled" disabled={!engine.ready} onClick={() => command('panic')}>Panic</Button>
      <ActionIcon size="lg" variant={panel === 'Setup' ? 'filled' : 'default'} aria-label="Setup" onClick={() => setPanel('Setup')}><Settings size={20}/></ActionIcon>
    </header>
    <main>
      {!state && <Group justify="center" p="xl"><Loader size="sm"/><Text>{engine.error ? 'Waiting for audio' : 'Starting audio…'}</Text></Group>}
      {panel === 'Play' && state && <Play state={state} command={command} edit={edit} openLibrary={() => setPanel('Library')} openStop={stop => { setSelected(stop); setPanel('Build'); }}/>} 
      {panel === 'Library' && <Library state={state} command={command}/>}
      {panel === 'Setup' && <Stack p="lg"><Group><Button variant="subtle" leftSection={<ArrowLeft size={18}/>} onClick={() => setPanel('Play')}>Play</Button><Text fw={600}>Setup</Text></Group>
        <Appearance edit={edit} density={density} changeDensity={value => { setDensity(value); localStorage.setItem('aristide-density', value); }}/>
        {state && <ConsoleSetup state={state} command={command} edit={edit}/>}</Stack>}
      {['Build', 'Route', 'Tuning'].includes(panel) && <Stack p="lg">
        <Text fw={600}>{panel === 'Build' && selected ? selected.name : panel}</Text>
        <Text c="dimmed">{panel === 'Build' ? 'The new voice editor is being designed. Editing will become available after the layout studies and audio integration.' : `${panel} is not connected in this first increment.`}</Text>
        {(panel === 'Build' || panel === 'Tuning') && <Button component="a" href={`/?study=1&panel=${panel.toLowerCase()}`} variant="default" w="fit-content">Compare four design studies</Button>}
        <Button variant="default" w="fit-content" onClick={() => setPanel('Play')}>Back to Play</Button>
      </Stack>}
    </main>
    <Modal opened={Boolean(error)} title={engine.error ? 'Audio is unavailable' : 'The organ could not be opened'} onClose={() => { engine.dismissError(); setDismissedLoadError(state?.load_error); }}>
      <Stack><Text>{engine.error ? 'Check that your audio device is connected, then reopen Aristide.' : 'Check that the organ and its samples are still in their original folder, then choose it again.'}</Text>
        <Button onClick={() => { engine.dismissError(); setDismissedLoadError(state?.load_error); }}>Close</Button></Stack>
    </Modal>
  </div>;
}

function Play({ state, command, edit, openStop, openLibrary }: { state: Snapshot; command: Command; edit: boolean; openStop: (stop: Stop) => void; openLibrary: () => void }) {
  if (!state.organ) return <Stack align="center" p="xl"><Text>Choose an organ to begin.</Text><Button onClick={openLibrary}>Open Library</Button></Stack>;
  return <div className="play">
    <div className="divisions">{state.manuals.map(manual => <section className="division" key={manual.idx} aria-label={manual.name}>
      <header className="division-heading"><Text fw={600}>{manual.name}</Text>{state.manual_tuning?.filter(t => t.idx === manual.idx).map(t => <Text key={t.idx} size="xs" c="violet">{t.temperament}, {t.reference.hz}</Text>)}</header>
      <div className="stops">{state.stops.filter(stop => stop.midx === manual.idx).map(stop => <StopButton key={stop.id} stop={stop} edit={edit} open={() => openStop(stop)} toggle={() => command('stop', { id: stop.id, on: stop.on ? 0 : 1 })}/>)}</div>
      {state.couplers.filter(c => !c.hidden && c.routes.some(r => r.from === manual.idx)).map(c => <Button className="coupler" key={c.idx} variant={c.on ? 'filled' : 'default'} aria-pressed={c.on} onClick={() => command('coupler', { idx: c.idx, on: c.on ? 0 : 1 })}>{c.name}</Button>)}
      <Group className="divisionals" gap="xs">{(state.combinations?.divisionals[String(manual.idx)] ?? []).map(n => <Button key={n} variant={state.combinations?.matching_divisionals[String(manual.idx)]?.includes(n) ? 'filled' : 'default'} aria-label={`${manual.name} piston ${n}`} onClick={() => command('divisional', { manual: manual.idx, n })}>{n}</Button>)}</Group>
    </section>)}</div>
    <footer className="pistons"><Button variant={state.setter ? 'filled' : 'default'} aria-pressed={state.setter} onClick={() => command('setter', { on: state.setter ? 0 : 1 })}>Set</Button>
      {Array.from({ length: Math.max(8, ...state.generals) }, (_, i) => i + 1).map(n => <Button key={n} aria-label={`General ${n}`} variant={state.combinations?.matching_generals.includes(n) ? 'filled' : 'default'} onClick={() => command('general', { n })}>{n}</Button>)}
      <Button variant="default" onClick={() => command('cancel')}>Cancel</Button><span className="spacer"/>
      <Button variant="default" leftSection={<ChevronLeft size={20}/>} onClick={() => command('stepper', { go: 'prev' })}>Prev</Button>
      <Button variant="default" rightSection={<ChevronRight size={20}/>} onClick={() => command('stepper', { go: 'next' })}>Next</Button>
    </footer>
  </div>;
}

function StopButton({ stop, edit, toggle, open }: { stop: Stop; edit: boolean; toggle: () => void; open: () => void }) {
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const origin = useRef({ x: 0, y: 0 });
  const consumed = useRef(false);
  const clear = () => { clearTimeout(timer.current); };
  useEffect(() => clear, [edit]);
  const footage = stop.pitch.footage ?? stop.pitch.native;
  return <button className="stop" aria-pressed={stop.on} onContextMenu={e => { e.preventDefault(); if (edit) open(); }}
    onPointerDown={e => { consumed.current = false; origin.current = { x: e.clientX, y: e.clientY }; if (e.button === 0) timer.current = setTimeout(() => { consumed.current = true; if (edit) open(); }, 600); }}
    onPointerMove={e => { if (Math.hypot(e.clientX - origin.current.x, e.clientY - origin.current.y) > 10) { clear(); consumed.current = true; } }}
    onPointerUp={clear} onPointerCancel={() => { clear(); consumed.current = true; }} onPointerLeave={clear}
    onClick={() => { if (!consumed.current) toggle(); consumed.current = false; }}>
    <span className="stop-name">{stop.name}</span><span className="stop-pitch">{footage ? `${Number(footage.toFixed(2))}′` : `${stop.ranks.length} ranks`}</span>
  </button>;
}

function Library({ state, command }: { state?: Snapshot; command: Command }) {
  const [browse, setBrowse] = useState<Browse>();
  const [opened, setOpened] = useState(false);
  const [path, setPath] = useState('');
  const [failure, setFailure] = useState(false);
  const visit = async (dir = '') => {
    setOpened(true); setFailure(false);
    try { const value = await request<Browse>('GET', endpoint('browse', { dir })); setBrowse(value); setPath(value.dir); }
    catch { setFailure(true); }
  };
  return <Stack p="lg"><Group justify="space-between"><Text fw={600}>Library</Text><Button onClick={() => void visit()}>Add organ</Button></Group>
    {state?.loading && <Group><Loader size="sm"/><Text>{state.loading}</Text></Group>}
    <div className="library-grid">{state?.library.map(organ => <Card key={organ.path} withBorder component="button" className="library-card" onClick={() => command('organ/load', { path: organ.path })}>
      <Text fw={600}>{organ.name}</Text><Text c="dimmed">Open organ</Text></Card>)}</div>
    {!state?.library.length && <Text c="dimmed">Add a GrandOrgue or unencrypted Hauptwerk organ from your computer.</Text>}
    <Drawer opened={opened} onClose={() => setOpened(false)} title="Add an organ" position="right" size="lg"><Stack>
      <form onSubmit={e => { e.preventDefault(); void visit(path); }}><Group wrap="nowrap"><TextInput aria-label="Folder" value={path} onChange={e => setPath(e.currentTarget.value)} style={{ flex: 1 }}/><Button type="submit">Go</Button></Group></form>
      {failure && <Text>That folder could not be opened. Choose another folder.</Text>}
      {browse?.parent && <Button variant="default" leftSection={<ArrowLeft size={18}/>} onClick={() => void visit(browse.parent!)}>Parent folder</Button>}
      {browse?.entries.filter(e => e.dir || !/\.(scl|kbm)$/i.test(e.name)).map(entry => <Button key={entry.path} justify="start" variant="default" leftSection={entry.dir ? <Folder size={18}/> : undefined} onClick={() => {
        if (entry.dir) void visit(entry.path); else { command('organ/load', { path: entry.path }); setOpened(false); }
      }}>{entry.name}</Button>)}
    </Stack></Drawer>
  </Stack>;
}

function Appearance({ edit, density, changeDensity }: { edit: boolean; density: string; changeDensity: (value: string) => void }) {
  const { colorScheme, setColorScheme } = useMantineColorScheme();
  return <Card withBorder><Stack><Text fw={600}>Appearance</Text><Group><Text>Colour</Text><SegmentedControl disabled={!edit} value={colorScheme} onChange={value => setColorScheme(value as 'dark' | 'light')} data={[{ value: 'dark', label: 'Dark' }, { value: 'light', label: 'Light' }]}/></Group>
    <Group><Text>Density</Text><SegmentedControl disabled={!edit} value={density} onChange={changeDensity} data={[{ value: 'comfortable', label: 'Comfortable' }, { value: 'compact', label: 'Compact' }]}/></Group></Stack></Card>;
}

function ConsoleSetup({ state, command, edit }: { state: Snapshot; command: Command; edit: boolean }) {
  return <Card withBorder><Stack><Group justify="space-between"><Text fw={600}>Console</Text><Button variant="default" disabled={!edit} onClick={() => command('midi/rescan')}>Rescan MIDI</Button></Group>
    {!edit && <Text c="dimmed">Unlock Edit to change your console.</Text>}
    {state.midi.manuals.map(manual => <Group key={manual.idx} justify="space-between"><Stack gap={0}><Text>{manual.name}</Text><Text size="xs" c="dimmed">{manual.inputs.map(i => `${i.device}${i.connected ? '' : ' (disconnected)'}`).join(', ') || 'No console assigned'}</Text></Stack>
      <Group><Button disabled={!edit} variant="default" onClick={() => command('midi/learn', { manual: manual.idx, slot: 0 })}>Learn keys</Button>
        <Button disabled={!edit} variant="default" onClick={() => command('midi/bind', { manual: manual.idx, slot: manual.inputs.length, device: 'Computer keyboard' })}>Use computer keyboard</Button></Group>
    </Group>)}
    {state.midi.learning && <Group><Text>Press a key on your console.</Text><Button variant="default" onClick={() => command('midi/learn')}>Cancel learning</Button></Group>}
  </Stack></Card>;
}
