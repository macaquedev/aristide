import { useEffect, useState } from 'react';
import { ActionIcon, Button, Group, NumberInput, SegmentedControl, Stack, Tabs, Text, TextInput, useMantineColorScheme } from '@mantine/core';
import { ArrowLeft, Trash2 } from 'lucide-react';
import { endpoint, request, type Snapshot } from '../api';
import type { Routing } from '../sound/SoundPanel';
import { ConsoleSettings } from './ConsoleSettings';

type Send = (path: string, values?: Record<string, string | number>) => Promise<Snapshot>;

/** Settings: Aristide itself and the room, whatever organ is loaded. */
export function SettingsPanel({ state, send, back, density, changeDensity }: {
  state?: Snapshot; send: Send; back: () => void; density: string; changeDensity: (value: string) => void;
}) {
  const [tab, setTab] = useState<string | null>(() => { try { return localStorage.getItem('aristide-settings-tab') ?? 'console'; } catch { return 'console'; } });
  const choose = (value: string | null) => { setTab(value); try { if (value) localStorage.setItem('aristide-settings-tab', value); } catch { /* per-viewer convenience only */ } };
  return <Stack p="lg">
    <Group><Button variant="subtle" leftSection={<ArrowLeft size={18}/>} onClick={back}>Play</Button><Text fw={600}>Settings</Text></Group>
    <Tabs value={tab} onChange={choose} keepMounted={false}>
      <Tabs.List><Tabs.Tab value="console">Console</Tabs.Tab><Tabs.Tab value="speakers">Speakers</Tabs.Tab><Tabs.Tab value="appearance">Appearance</Tabs.Tab></Tabs.List>
      <Tabs.Panel value="console" pt="md">{state ? <ConsoleSettings state={state} send={send}/> : <Text c="dimmed">Connecting to the sound engine…</Text>}</Tabs.Panel>
      <Tabs.Panel value="speakers" pt="md">{state ? <SpeakerSettings/> : <Text c="dimmed">Connecting to the sound engine…</Text>}</Tabs.Panel>
      <Tabs.Panel value="appearance" pt="md"><Appearance density={density} changeDensity={changeDensity}/></Tabs.Panel>
    </Tabs>
  </Stack>;
}

function Appearance({ density, changeDensity }: { density: string; changeDensity: (value: string) => void }) {
  const { colorScheme, setColorScheme } = useMantineColorScheme();
  return <Stack><Group><Text>Colour</Text><SegmentedControl value={colorScheme} onChange={value => setColorScheme(value as 'dark' | 'light')} data={[{ value: 'dark', label: 'Dark' }, { value: 'light', label: 'Light' }]}/></Group>
    <Group><Text>Density</Text><SegmentedControl value={density} onChange={changeDensity} data={[{ value: 'comfortable', label: 'Comfortable' }, { value: 'compact', label: 'Compact' }]}/></Group></Stack>;
}

function SpeakerSettings() {
  const [routing, setRouting] = useState<Routing>();
  const [name, setName] = useState('');
  const [output, setOutput] = useState<[number, number]>([3, 4]);
  const [failure, setFailure] = useState(false);
  useEffect(() => { request<Routing>('GET', '/api/routing').then(setRouting, () => setRouting(undefined)); }, []);
  const change = (values: Record<string, string | number>) => {
    setFailure(false);
    return request<Routing>('POST', endpoint('speakers', values)).then(setRouting, () => { setFailure(true); throw new Error('speakers'); });
  };
  const channel = (value: string | number) => Math.max(1, Math.min(64, Math.round(Number(value) || 1)));
  const defined = routing?.speakers.filter(s => s.defined && s.output) ?? [];
  return <Stack>
    <Group justify="space-between"><Text fw={600}>Speakers</Text>{routing && <Text size="xs" c="dimmed">Device outputs: {routing.channels}</Text>}</Group>
    {defined.map(speaker => <Group key={speaker.name} wrap="nowrap">
      <Stack gap={0} style={{ flex: 1 }}><Text>{speaker.name}</Text>{!speaker.available && <Text size="xs" c="dimmed">Not on this device · plays through Main</Text>}</Stack>
      {speaker.name === 'Main' ? <Text c="dimmed">1 / 2</Text> : <>
        <NumberInput w={84} aria-label={`${speaker.name} left channel`} min={1} max={64} value={speaker.output![0]}
          onChange={v => void change({ name: speaker.name, left: channel(v), right: speaker.output![1] }).catch(() => {})}/>
        <NumberInput w={84} aria-label={`${speaker.name} right channel`} min={1} max={64} value={speaker.output![1]}
          onChange={v => void change({ name: speaker.name, left: speaker.output![0], right: channel(v) }).catch(() => {})}/>
        <ActionIcon size="lg" variant="subtle" color="red" aria-label={`Remove ${speaker.name}`} onClick={() => void change({ name: speaker.name, remove: 1 }).catch(() => {})}><Trash2 size={18}/></ActionIcon>
      </>}
    </Group>)}
    <form onSubmit={e => { e.preventDefault(); if (name.trim()) void change({ name: name.trim(), left: output[0], right: output[1] }).then(() => setName(''), () => {}); }}>
      <Group wrap="nowrap" align="end">
        <TextInput style={{ flex: 1 }} label="New group" placeholder="Name" value={name} onChange={e => setName(e.currentTarget.value)}/>
        <NumberInput w={84} label="Left" min={1} max={64} value={output[0]} onChange={v => setOutput([channel(v), output[1]])}/>
        <NumberInput w={84} label="Right" min={1} max={64} value={output[1]} onChange={v => setOutput([output[0], channel(v)])}/>
        <Button type="submit" disabled={!name.trim()}>Add</Button>
      </Group>
    </form>
    {failure && <Text>That speaker group could not be saved. Use a new name without commas or colons.</Text>}
  </Stack>;
}
