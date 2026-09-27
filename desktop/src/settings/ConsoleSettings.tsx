import { useEffect, useRef, useState } from 'react';
import { ActionIcon, Badge, Button, Drawer, Group, Loader, Select, Stack, Table, Text, TextInput, Tooltip } from '@mantine/core';
import { Plus, RotateCcw, Trash2 } from 'lucide-react';
import { COMPUTER_KEYBOARD, type ConsoleKeyboard, type Snapshot } from '../api';
import { noteName } from '../tuning/model';

type Send = (path: string, values?: Record<string, string | number>) => Promise<Snapshot>;

const channels = [{ value: 'any', label: 'Any' }, ...Array.from({ length: 16 }, (_, i) => ({ value: String(i + 1), label: String(i + 1) }))];

/** Settings › Console: the player's own keyboards, taught once for every organ. */
export function ConsoleSettings({ state, send }: { state: Snapshot; send: Send }) {
  const [detecting, setDetecting] = useState(false);
  const [failure, setFailure] = useState<string>();
  const keyboards = state.console?.keyboards ?? [];
  const learning = detecting ? undefined : state.console?.learning;
  const act = (path: string, values: Record<string, string | number>, message: string) => {
    setFailure(undefined);
    return send(path, values).catch(() => { setFailure(message); return undefined; });
  };
  const useComputer = async () => {
    const added = await act('console/add', {}, 'The keyboard could not be added.');
    if (added?.console) await act('console/set', { keyboard: added.console.keyboards.length - 1, device: COMPUTER_KEYBOARD }, 'The computer keyboard could not be chosen.');
  };
  return <Stack>
    <Group justify="space-between">
      <Text fw={600}>Keyboards</Text>
      <Group>
        <Button variant="default" onClick={() => void act('midi/rescan', {}, 'MIDI devices could not be rescanned.')}>Rescan MIDI</Button>
        <Button onClick={() => setDetecting(true)}>Detect console</Button>
      </Group>
    </Group>
    {keyboards.length === 0
      ? <Stack align="flex-start" gap="xs"><Text c="dimmed">No keyboards yet.</Text>
          <Group><Button onClick={() => setDetecting(true)}>Detect console</Button><Button variant="default" onClick={() => void useComputer()}>Use computer keyboard</Button></Group></Stack>
      : <Table.ScrollContainer minWidth={760}><Table verticalSpacing="sm">
          <Table.Thead><Table.Tr><Table.Th>Keyboard</Table.Th><Table.Th>Device</Table.Th><Table.Th>Channel</Table.Th><Table.Th>Range</Table.Th>{state.organ && <Table.Th>Plays</Table.Th>}<Table.Th/></Table.Tr></Table.Thead>
          <Table.Tbody>{keyboards.map(keyboard => <KeyboardRow key={`${keyboard.idx}-${keyboard.name}`} keyboard={keyboard} state={state} act={act}
            learning={learning?.keyboard === keyboard.idx ? learning : undefined}/>)}</Table.Tbody>
        </Table></Table.ScrollContainer>}
    {keyboards.length > 0 && <Group>
      <Button variant="default" leftSection={<Plus size={16}/>} onClick={() => void act('console/add', {}, 'The keyboard could not be added.')}>Manual</Button>
      <Button variant="default" leftSection={<Plus size={16}/>} onClick={() => void act('console/add', { pedal: 1 }, 'The keyboard could not be added.')}>Pedalboard</Button>
    </Group>}
    {failure && <Text c="red">{failure}</Text>}
    <DetectConsole opened={detecting} state={state} send={send} close={() => setDetecting(false)}/>
  </Stack>;
}

type Act = (path: string, values: Record<string, string | number>, message: string) => Promise<Snapshot | undefined>;
type Learning = NonNullable<NonNullable<Snapshot['console']>['learning']>;

function KeyboardRow({ keyboard, state, learning, act }: { keyboard: ConsoleKeyboard; state: Snapshot; learning?: Learning; act: Act }) {
  const computer = keyboard.device === COMPUTER_KEYBOARD;
  const ports = state.midi.ports.map(p => p.name);
  const devices = [{ value: 'none', label: 'None' }, ...[...new Set([COMPUTER_KEYBOARD, ...ports])].map(name => ({ value: name, label: name })),
    ...(keyboard.device && !ports.includes(keyboard.device) && !computer ? [{ value: keyboard.device, label: `${keyboard.device} (not connected)` }] : [])];
  const set = (values: Record<string, string | number>) => act('console/set', { keyboard: keyboard.idx, ...values }, `${keyboard.name} could not be changed.`);
  const listen = (range: boolean) => act('console/learn', { keyboard: keyboard.idx, ...(range ? { range: 1 } : {}) }, 'Detection could not start.');
  const plays = keyboard.plays.map(i => state.manuals.find(m => m.idx === i)?.name).filter(Boolean).join(', ');
  const range = keyboard.low !== null && keyboard.high !== null ? `${noteName(keyboard.low)}–${noteName(keyboard.high)}` : computer ? '—' : 'Organ’s own';
  return <Table.Tr data-learning={Boolean(learning)}>
    <Table.Td><Group gap="xs" wrap="nowrap"><KeyboardName keyboard={keyboard} rename={name => set({ name })}/>{keyboard.pedal && <Badge variant="default" size="sm">Pedal</Badge>}</Group></Table.Td>
    {learning ? <Table.Td colSpan={state.organ ? 4 : 3}>
      <Group gap="sm" wrap="nowrap"><Loader size="xs"/>
        <Text>{learning.repeat ? `That key is on ${learning.repeat}. ` : ''}{learning.range ? (learning.step === 'low' ? 'Press the lowest key.' : 'Now the highest key.') : `Press any key on ${keyboard.name}.`}</Text>
        <Button size="xs" variant="default" onClick={() => void act('console/learn', {}, 'Detection could not stop.')}>Cancel</Button></Group>
    </Table.Td> : <>
      <Table.Td><Group gap="xs" wrap="nowrap">
        <Select aria-label={`${keyboard.name} device`} w={220} value={keyboard.device || 'none'} data={devices} allowDeselect={false} onChange={v => v && void set({ device: v === 'none' ? '' : v })}/>
        <Button size="xs" variant="default" onClick={() => void listen(false)}>Detect</Button></Group></Table.Td>
      <Table.Td><Select aria-label={`${keyboard.name} channel`} w={84} disabled={computer || !keyboard.device} value={keyboard.channel === null ? 'any' : String(keyboard.channel)} data={channels} allowDeselect={false}
        onChange={v => v && void set({ ch: v })}/></Table.Td>
      <Table.Td><Group gap={4} wrap="nowrap"><Text size="sm" style={{ whiteSpace: 'nowrap' }}>{range}</Text>
        {!computer && keyboard.device && <Button size="xs" variant="subtle" onClick={() => void listen(true)}>Detect</Button>}
        {keyboard.low !== null && <Tooltip label="Use the organ’s range"><ActionIcon variant="subtle" color="gray" aria-label={`Reset ${keyboard.name} range`} onClick={() => void set({ range: 'organ' })}><RotateCcw size={16}/></ActionIcon></Tooltip>}
      </Group></Table.Td>
      {state.organ && <Table.Td><Text size="sm" c={plays ? undefined : 'dimmed'}>{plays || 'Nothing'}</Text></Table.Td>}
    </>}
    <Table.Td><ActionIcon variant="subtle" color="red" aria-label={`Remove ${keyboard.name}`} onClick={() => void act('console/remove', { keyboard: keyboard.idx }, `${keyboard.name} could not be removed.`)}><Trash2 size={18}/></ActionIcon></Table.Td>
  </Table.Tr>;
}

function KeyboardName({ keyboard, rename }: { keyboard: ConsoleKeyboard; rename: (name: string) => Promise<unknown> }) {
  const [name, setName] = useState(keyboard.name);
  useEffect(() => setName(keyboard.name), [keyboard.name]);
  const commit = () => { const next = name.trim(); if (next && next !== keyboard.name) void rename(next).then(result => { if (!result) setName(keyboard.name); }); else setName(keyboard.name); };
  return <TextInput aria-label="Keyboard name" w={150} variant="unstyled" value={name} onChange={e => setName(e.currentTarget.value)}
    onBlur={commit} onKeyDown={e => { if (e.key === 'Enter') e.currentTarget.blur(); if (e.key === 'Escape') { setName(keyboard.name); e.currentTarget.blur(); } }}/>;
}

type Phase = 'hand' | 'pedal' | 'done';

/** Press a key on each manual, bottom to top, then a pedal. Detected keyboards
 * take the console's places in order; Done removes any the console no longer has. */
function DetectConsole({ opened, state, send, close }: { opened: boolean; state: Snapshot; send: Send; close: () => void }) {
  const [phase, setPhase] = useState<Phase>('hand');
  const [heard, setHeard] = useState<string[]>([]);
  const [missed, setMissed] = useState(false);
  const counts = useRef({ hand: 0, pedal: 0 });
  const baseline = useRef<number>(undefined);
  // A poll answered before the learn request can show no wait yet; only a wait seen to end is a miss.
  const seen = useRef(false);
  const keyboards = state.console?.keyboards ?? [];
  const console_ = state.console;

  const listen = (next: Exclude<Phase, 'done'>) => {
    const places = keyboards.filter(k => k.pedal === (next === 'pedal'));
    const target = places[counts.current[next]]?.idx ?? keyboards.length;
    setMissed(false);
    seen.current = false;
    baseline.current = console_?.heard ?? 0;
    void send('console/learn', { keyboard: target, ...(next === 'pedal' ? { pedal: 1 } : {}) });
  };
  const stop = () => { baseline.current = undefined; void send('console/learn').catch(() => {}); };

  useEffect(() => {
    if (!opened) return;
    counts.current = { hand: 0, pedal: 0 };
    setHeard([]); setPhase('hand'); listen('hand');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opened]);

  useEffect(() => {
    if (!opened || baseline.current === undefined || !console_) return;
    if (console_.heard > baseline.current) {
      baseline.current = undefined;
      const newest = phase === 'pedal'
        ? console_.keyboards.filter(k => k.pedal)[counts.current.pedal]
        : console_.keyboards.filter(k => !k.pedal)[counts.current.hand];
      setHeard(list => [...list, newest ? `${newest.name}: ${newest.device}${newest.channel ? `, channel ${newest.channel}` : ''}` : '']);
      if (phase === 'pedal') { counts.current.pedal += 1; setPhase('done'); }
      else { counts.current.hand += 1; listen('hand'); }
    } else if (console_.learning) seen.current = true;
    else if (seen.current) { baseline.current = undefined; setMissed(true); }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [console_, opened]);

  const toPedal = () => { stop(); setPhase('pedal'); listen('pedal'); };
  const finish = async () => {
    stop();
    const latest = await send('console/learn');
    const all = latest.console?.keyboards ?? [];
    const extra = [...all.filter(k => !k.pedal).slice(counts.current.hand), ...all.filter(k => k.pedal).slice(counts.current.pedal)]
      .map(k => k.idx).sort((a, b) => b - a);
    for (const keyboard of extra) await send('console/remove', { keyboard });
    close();
  };
  const cancel = () => { stop(); close(); };
  const repeat = console_?.learning?.repeat;
  const prompt = phase === 'pedal' ? 'Press any pedal.'
    : counts.current.hand === 0 ? 'Press any key on your lowest manual.' : 'Press any key on the next manual up.';

  return <Drawer opened={opened} onClose={cancel} position="right" title="Detect console" closeButtonProps={{ 'aria-label': 'Close' }}>
    <Stack>
      {heard.map((line, i) => <Text key={i} size="sm">✓ {line}</Text>)}
      {phase !== 'done' && <Group gap="sm"><Loader size="sm"/><Text fw={600}>{missed ? 'Nothing heard.' : repeat ? `That was ${repeat}. ${prompt}` : prompt}</Text></Group>}
      {phase !== 'done' && <Text size="sm" c="dimmed">A letter key picks the computer keyboard.</Text>}
      {missed && <Button variant="default" onClick={() => listen(phase === 'pedal' ? 'pedal' : 'hand')}>Try again</Button>}
      {phase === 'hand' && counts.current.hand > 0 && <Button variant="default" onClick={toPedal}>That was the top manual</Button>}
      {phase === 'pedal' && <Button variant="default" onClick={() => { stop(); setPhase('done'); }}>No pedalboard</Button>}
      {phase === 'done' && <Text fw={600}>Console detected.</Text>}
      <Group justify="flex-end"><Button variant="default" onClick={cancel}>Cancel</Button>
        <Button disabled={phase !== 'done'} onClick={() => void finish()}>Done</Button></Group>
    </Stack>
  </Drawer>;
}
