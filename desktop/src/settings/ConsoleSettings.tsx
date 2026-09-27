import { useEffect, useRef, useState } from 'react';
import { ActionIcon, Badge, Button, Drawer, Group, Loader, Select, Stack, Table, Text, TextInput, Tooltip } from '@mantine/core';
import { RotateCcw, Trash2 } from 'lucide-react';
import { COMPUTER_KEYBOARD, type ConsoleKeyboard, type Snapshot } from '../api';
import { noteName } from '../tuning/model';

type Send = (path: string, values?: Record<string, string | number>) => Promise<Snapshot>;
type Act = (path: string, values: Record<string, string | number>, message: string) => Promise<Snapshot | undefined>;
type Division = Snapshot['manuals'][number];

const channels = [{ value: 'any', label: 'Any' }, ...Array.from({ length: 16 }, (_, i) => ({ value: String(i + 1), label: String(i + 1) }))];
const describe = (keyboard?: ConsoleKeyboard) => !keyboard ? '' : !keyboard.device ? 'No device'
  : `${keyboard.device}${keyboard.channel ? ` · channel ${keyboard.channel}` : ''}${keyboard.connected ? '' : ' (not connected)'}`;

/** Settings › Console: which of the player's keyboards plays each manual of the loaded organ,
 * found by pressing a key on it, as Hauptwerk's auto-detect does. */
export function ConsoleSettings({ state, send }: { state: Snapshot; send: Send }) {
  const [detecting, setDetecting] = useState(false);
  const [failure, setFailure] = useState<string>();
  const keyboards = state.console?.keyboards ?? [];
  const act: Act = (path, values, message) => {
    setFailure(undefined);
    return send(path, values).catch(() => { setFailure(message); return undefined; });
  };
  return <Stack gap="xl">
    <Stack>
      <Group justify="space-between">
        <Text fw={600}>{state.organ ?? 'No organ loaded'}</Text>
        <Group>
          <Button variant="default" onClick={() => void act('midi/rescan', {}, 'MIDI devices could not be rescanned.')}>Rescan MIDI</Button>
          {state.organ && <Button onClick={() => setDetecting(true)}>Detect all</Button>}
        </Group>
      </Group>
      {state.organ
        ? <Table.ScrollContainer minWidth={640}><Table verticalSpacing="sm">
            <Table.Thead><Table.Tr><Table.Th>Manual</Table.Th><Table.Th>Played from</Table.Th><Table.Th>Device</Table.Th><Table.Th/></Table.Tr></Table.Thead>
            <Table.Tbody>{state.manuals.map(division => <DivisionRow key={division.idx} division={division} state={state} act={act}/>)}</Table.Tbody>
          </Table></Table.ScrollContainer>
        : <Text c="dimmed">Load an organ to choose the keyboard for each of its manuals.</Text>}
    </Stack>
    {keyboards.length > 0 && <Stack>
      <Text fw={600}>Your keyboards</Text>
      <Table.ScrollContainer minWidth={640}><Table verticalSpacing="sm">
        <Table.Thead><Table.Tr><Table.Th>Name</Table.Th><Table.Th>Device</Table.Th><Table.Th>Channel</Table.Th><Table.Th>Range</Table.Th><Table.Th/></Table.Tr></Table.Thead>
        <Table.Tbody>{keyboards.map(keyboard => <KeyboardRow key={`${keyboard.idx}-${keyboard.name}`} keyboard={keyboard} state={state} act={act}
          learning={state.console?.learning?.keyboard === keyboard.idx ? state.console.learning : undefined}/>)}</Table.Tbody>
      </Table></Table.ScrollContainer>
    </Stack>}
    {failure && <Text c="red">{failure}</Text>}
    <DetectAll opened={detecting} state={state} send={send} close={() => setDetecting(false)}/>
  </Stack>;
}

function DivisionRow({ division, state, act }: { division: Division; state: Snapshot; act: Act }) {
  const keyboards = state.console?.keyboards ?? [];
  const midi = state.midi.manuals.find(m => m.idx === division.idx);
  const current = midi?.keyboard ?? undefined;
  const own = midi?.inputs ?? [];
  const listening = state.console?.learning?.manual === division.idx;
  const value = midi?.automatic ? 'auto' : current === undefined ? 'none' : keyboards[current]?.name ?? 'auto';
  const automatic = current !== undefined ? `${keyboards[current]?.name} (automatic)` : own.length ? 'This organ’s own' : 'Nothing (automatic)';
  const choose = (keyboard: string) => act('console/map', { manual: division.idx, keyboard }, `${division.name} could not be changed.`);
  return <Table.Tr data-learning={listening}>
    <Table.Td><Group gap="xs" wrap="nowrap"><Text fw={600}>{division.name}</Text>{division.pedal && !/pedal/i.test(division.name) && <Badge variant="default" size="sm">Pedal</Badge>}</Group></Table.Td>
    {listening ? <Table.Td colSpan={2}><Group gap="sm" wrap="nowrap"><Loader size="xs"/>
        <Text>Press any key on the keyboard for {division.name}.</Text>
        <Button size="xs" variant="default" onClick={() => void act('console/learn', {}, 'Detection could not stop.')}>Cancel</Button></Group></Table.Td>
      : <>
        <Table.Td><Select aria-label={`${division.name} played from`} w={220} value={value} allowDeselect={false} disabled={!keyboards.length && !own.length}
          data={[{ value: 'auto', label: automatic }, ...keyboards.map(k => ({ value: k.name, label: k.name })), { value: 'none', label: 'Nothing' }]}
          onChange={v => v && void choose(v)}/></Table.Td>
        <Table.Td><Stack gap={2}>
          <Text size="sm" c={current === undefined ? 'dimmed' : undefined}>{current === undefined ? (own.length ? '' : '—') : describe(keyboards[current])}</Text>
          {own.map(input => <Group key={input.slot} gap={4} wrap="nowrap"><Text size="sm">{input.device}{input.channel ? ` · channel ${input.channel}` : ''}</Text>
            <Button size="compact-xs" variant="subtle" color="gray" onClick={() => void act('midi/unbind', { manual: division.idx, slot: input.slot }, 'That input could not be removed.')}>Remove</Button></Group>)}
        </Stack></Table.Td>
      </>}
    <Table.Td>{!listening && <Button variant="default" onClick={() => void act('console/learn', { manual: division.idx }, 'Detection could not start.')}>Detect</Button>}</Table.Td>
  </Table.Tr>;
}

type Learning = NonNullable<NonNullable<Snapshot['console']>['learning']>;

function KeyboardRow({ keyboard, state, learning, act }: { keyboard: ConsoleKeyboard; state: Snapshot; learning?: Learning; act: Act }) {
  const computer = keyboard.device === COMPUTER_KEYBOARD;
  const ports = state.midi.ports.map(p => p.name);
  const devices = [{ value: 'none', label: 'None' }, ...[...new Set([COMPUTER_KEYBOARD, ...ports])].map(name => ({ value: name, label: name })),
    ...(keyboard.device && !ports.includes(keyboard.device) && !computer ? [{ value: keyboard.device, label: `${keyboard.device} (not connected)` }] : [])];
  const set = (values: Record<string, string | number>) => act('console/set', { keyboard: keyboard.idx, ...values }, `${keyboard.name} could not be changed.`);
  const range = keyboard.low !== null && keyboard.high !== null ? `${noteName(keyboard.low)}–${noteName(keyboard.high)}` : computer ? '—' : 'Organ’s own';
  return <Table.Tr>
    <Table.Td><Group gap="xs" wrap="nowrap"><KeyboardName keyboard={keyboard} rename={name => set({ name })}/>{keyboard.pedal && <Badge variant="default" size="sm">Pedal</Badge>}</Group></Table.Td>
    <Table.Td><Select aria-label={`${keyboard.name} device`} w={220} value={keyboard.device || 'none'} data={devices} allowDeselect={false} onChange={v => v && void set({ device: v === 'none' ? '' : v })}/></Table.Td>
    <Table.Td><Select aria-label={`${keyboard.name} channel`} w={84} disabled={computer || !keyboard.device} value={keyboard.channel === null ? 'any' : String(keyboard.channel)} data={channels} allowDeselect={false}
      onChange={v => v && void set({ ch: v })}/></Table.Td>
    <Table.Td>{learning ? <Group gap="sm" wrap="nowrap"><Loader size="xs"/><Text size="sm">{learning.step === 'low' ? 'Press the lowest key.' : 'Now the highest key.'}</Text>
        <Button size="xs" variant="default" onClick={() => void act('console/learn', {}, 'Detection could not stop.')}>Cancel</Button></Group>
      : <Group gap={4} wrap="nowrap"><Text size="sm" style={{ whiteSpace: 'nowrap' }}>{range}</Text>
        {!computer && keyboard.device && <Button size="xs" variant="subtle" onClick={() => void act('console/learn', { keyboard: keyboard.idx, range: 1 }, 'Detection could not start.')}>Detect</Button>}
        {keyboard.low !== null && <Tooltip label="Use the organ’s range"><ActionIcon variant="subtle" color="gray" aria-label={`Reset ${keyboard.name} range`} onClick={() => void set({ range: 'organ' })}><RotateCcw size={16}/></ActionIcon></Tooltip>}
      </Group>}</Table.Td>
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

/** Each of the organ's manuals in turn: press a key on the keyboard that should play it. */
function DetectAll({ opened, state, send, close }: { opened: boolean; state: Snapshot; send: Send; close: () => void }) {
  const [step, setStep] = useState(0);
  const [missed, setMissed] = useState(false);
  const baseline = useRef<number>(undefined);
  // A poll answered before the learn request can show no wait yet; only a wait seen to end is a miss.
  const seen = useRef(false);
  const manuals = state.manuals;
  const division = manuals[step];

  const listen = (index: number) => {
    setMissed(false);
    seen.current = false;
    if (index >= manuals.length) { baseline.current = undefined; void send('console/learn').catch(() => {}); return; }
    baseline.current = state.console?.heard ?? 0;
    void send('console/learn', { manual: manuals[index].idx }).catch(() => {});
  };
  const go = (index: number) => { setStep(index); listen(index); };

  useEffect(() => { if (opened) go(0); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [opened]);
  useEffect(() => {
    const console_ = state.console;
    if (!opened || baseline.current === undefined || !console_) return;
    if (console_.heard > baseline.current) go(step + 1);
    else if (console_.learning) seen.current = true;
    else if (seen.current) { baseline.current = undefined; setMissed(true); }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.console, opened]);

  const finish = () => { baseline.current = undefined; void send('console/learn').catch(() => {}); close(); };
  const keyboards = state.console?.keyboards ?? [];
  return <Drawer opened={opened} onClose={finish} position="right" title="Detect all" closeButtonProps={{ 'aria-label': 'Close' }}>
    <Stack>
      {manuals.map((manual, index) => {
        const keyboard = state.midi.manuals.find(m => m.idx === manual.idx)?.keyboard;
        const found = keyboard === null || keyboard === undefined ? undefined : keyboards[keyboard];
        return <Group key={manual.idx} justify="space-between" wrap="nowrap">
          <Text fw={index === step ? 600 : undefined}>{manual.name}</Text>
          {index < step && <Text size="sm" c={found ? undefined : 'dimmed'}>{found ? `✓ ${found.name}` : 'Nothing'}</Text>}
        </Group>;
      })}
      {division ? <>
        <Group gap="sm" wrap="nowrap"><Loader size="sm"/><Text fw={600}>{missed ? 'Nothing heard.' : `Press any key on the keyboard for ${division.name}.`}</Text></Group>
        <Text size="sm" c="dimmed">A letter key picks the computer keyboard.</Text>
        <Group>{missed && <Button variant="default" onClick={() => listen(step)}>Try again</Button>}
          <Button variant="default" onClick={() => go(step + 1)}>Skip</Button></Group>
      </> : <Text fw={600}>Every manual has its keyboard.</Text>}
      <Group justify="flex-end"><Button onClick={finish}>{division ? 'Stop' : 'Done'}</Button></Group>
    </Stack>
  </Drawer>;
}
