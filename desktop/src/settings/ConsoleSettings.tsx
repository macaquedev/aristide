import { useEffect, useRef, useState } from 'react';
import { Button, Drawer, Group, Loader, Select, Stack, Table, Text } from '@mantine/core';
import { COMPUTER_KEYBOARD, type Snapshot } from '../api';

type Send = (path: string, values?: Record<string, string | number>) => Promise<Snapshot>;
type Act = (path: string, values: Record<string, string | number>, message: string) => Promise<Snapshot | undefined>;
type Division = Snapshot['manuals'][number];

const channels = [{ value: 'any', label: 'Any' }, ...Array.from({ length: 16 }, (_, i) => ({ value: String(i + 1), label: String(i + 1) }))];

/** Settings › Console: the MIDI keyboard that plays each of the loaded organ's manuals,
 * found by pressing a key on it, as Hauptwerk's auto-detect does. */
export function ConsoleSettings({ state, send }: { state: Snapshot; send: Send }) {
  const [detecting, setDetecting] = useState(false);
  const [failure, setFailure] = useState<string>();
  const act: Act = (path, values, message) => {
    setFailure(undefined);
    return send(path, values).catch(() => { setFailure(message); return undefined; });
  };
  return <Stack>
    <Group justify="space-between">
      <Text fw={600}>{state.organ ?? 'No organ loaded'}</Text>
      <Group>
        <Button variant="default" onClick={() => void act('midi/rescan', {}, 'MIDI devices could not be rescanned.')}>Rescan MIDI</Button>
        {state.organ && <Button onClick={() => setDetecting(true)}>Detect all</Button>}
      </Group>
    </Group>
    {state.organ
      ? <Table.ScrollContainer minWidth={600}><Table verticalSpacing="sm">
          <Table.Thead><Table.Tr><Table.Th>Manual</Table.Th><Table.Th>MIDI device</Table.Th><Table.Th>Channel</Table.Th><Table.Th/></Table.Tr></Table.Thead>
          <Table.Tbody>{state.manuals.map(division => <DivisionRow key={division.idx} division={division} state={state} act={act}/>)}</Table.Tbody>
        </Table></Table.ScrollContainer>
      : <Text c="dimmed">Load an organ to choose the keyboard for each of its manuals.</Text>}
    {failure && <Text c="red">{failure}</Text>}
    <DetectAll opened={detecting} state={state} send={send} close={() => setDetecting(false)}/>
  </Stack>;
}

function DivisionRow({ division, state, act }: { division: Division; state: Snapshot; act: Act }) {
  const input = state.midi.manuals.find(m => m.idx === division.idx)?.inputs[0];
  const listening = state.midi.learning?.manual === division.idx;
  const ports = state.midi.ports.map(p => p.name);
  const devices = [{ value: 'none', label: 'None' }, ...[...new Set([COMPUTER_KEYBOARD, ...ports])].map(name => ({ value: name, label: name })),
    ...(input && !ports.includes(input.device) && input.device !== COMPUTER_KEYBOARD ? [{ value: input.device, label: `${input.device} (not connected)` }] : [])];
  const assign = (device: string, ch: string) => act('midi/assign', { manual: division.idx, device, ...(ch === 'any' ? {} : { ch }) }, `${division.name} could not be changed.`);
  const channel = input?.channel === null || input?.channel === undefined ? 'any' : String(input.channel);
  return <Table.Tr data-learning={listening}>
    <Table.Td><Text fw={600}>{division.name}</Text></Table.Td>
    {listening ? <Table.Td colSpan={2}><Group gap="sm" wrap="nowrap"><Loader size="xs"/>
        <Text>Press any key on the keyboard for {division.name}.</Text></Group></Table.Td>
      : <>
        <Table.Td><Select aria-label={`${division.name} MIDI device`} w={240} value={input?.device ?? 'none'} data={devices} allowDeselect={false}
          onChange={v => v && void assign(v === 'none' ? '' : v, channel)}/></Table.Td>
        <Table.Td><Select aria-label={`${division.name} channel`} w={84} disabled={!input || input.device === COMPUTER_KEYBOARD} value={channel} data={channels} allowDeselect={false}
          onChange={v => v && input && void assign(input.device, v)}/></Table.Td>
      </>}
    <Table.Td>{listening
      ? <Button variant="default" onClick={() => void act('midi/learn', {}, 'Detection could not stop.')}>Cancel</Button>
      : <Button variant="default" onClick={() => void act('midi/learn', { manual: division.idx, detect: 1 }, 'Detection could not start.')}>Detect</Button>}</Table.Td>
  </Table.Tr>;
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
    if (index >= manuals.length) { baseline.current = undefined; void send('midi/learn').catch(() => {}); return; }
    baseline.current = state.midi.detected ?? 0;
    void send('midi/learn', { manual: manuals[index].idx, detect: 1 }).catch(() => {});
  };
  const go = (index: number) => { setStep(index); listen(index); };

  useEffect(() => { if (opened) go(0); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [opened]);
  useEffect(() => {
    if (!opened || baseline.current === undefined) return;
    if ((state.midi.detected ?? 0) > baseline.current) go(step + 1);
    else if (state.midi.learning) seen.current = true;
    else if (seen.current) { baseline.current = undefined; setMissed(true); }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.midi, opened]);

  const finish = () => { baseline.current = undefined; void send('midi/learn').catch(() => {}); close(); };
  return <Drawer opened={opened} onClose={finish} position="right" title="Detect all" closeButtonProps={{ 'aria-label': 'Close' }}>
    <Stack>
      {manuals.map((manual, index) => {
        const input = state.midi.manuals.find(m => m.idx === manual.idx)?.inputs[0];
        return <Group key={manual.idx} justify="space-between" wrap="nowrap">
          <Text fw={index === step ? 600 : undefined}>{manual.name}</Text>
          {index < step && <Text size="sm" c={input ? undefined : 'dimmed'}>{input ? `✓ ${input.device}${input.channel ? ` · channel ${input.channel}` : ''}` : 'None'}</Text>}
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
