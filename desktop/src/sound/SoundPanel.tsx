import { Fragment, useCallback, useEffect, useRef, useState } from 'react';
import { Button, Group, Loader, Modal, Stack, Text } from '@mantine/core';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { editInstrument, request } from '../api';
import { SoundCell } from './SoundCell';
import './sound.css';

export type Sends = Record<string, number>;
export type Speaker = { name: string; output: [number, number] | null; defined: boolean; available: boolean };
export type Routing = {
  channels: number;
  speakers: Speaker[];
  divisions: { idx: number; name: string; own: boolean; sends: Sends }[];
  stops: { id: number; name: string; midx: number; own: boolean; file: string | null; sends: Sends | null }[];
};
type Source = { manual: number; stop?: number };
type Params = Record<string, string | number>;

const sourceParams = ({ manual, stop }: Source): Params => stop === undefined ? { manual } : { manual, stop };
const sendsParam = (sends: Sends) => Object.entries(sends).map(([group, db]) => `${group}:${db}`).join(',');
const levelOf = (sends: Sends | null, group: string) =>
  sends ? Object.entries(sends).find(([name]) => name.toLowerCase() === group.toLowerCase())?.[1] : undefined;

/** Sound: sources down the side, speaker groups across the top. Every edit sounds and autosaves. */
export function SoundPanel({ organ, offerUndo, openSettings }: { organ: string; offerUndo: (undo?: () => void) => void; openSettings: () => void }) {
  const [routing, setRouting] = useState<Routing>();
  const [open, setOpen] = useState<number[]>();
  const [failure, setFailure] = useState<string>();
  const [notice, setNotice] = useState(false);
  const [history, setHistory] = useState<{ source: Source; own: boolean; sends: Sends; at: number }[]>([]);
  const queue = useRef<Promise<unknown>>(Promise.resolve());

  const refresh = useCallback(() => request<Routing>('GET', '/api/routing').then(setRouting, () => setRouting(undefined)), []);
  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), 1000);
    return () => clearInterval(timer);
  }, [refresh]);
  // A division opens by itself when one of its stops has its own routing.
  useEffect(() => {
    if (routing && !open) setOpen([...new Set(routing.stops.filter(s => s.own || s.file).map(s => s.midx))]);
  }, [routing, open]);

  const post = (params: Params) => {
    const task = queue.current.catch(() => {}).then(() => editInstrument<Routing>(organ, 'routing', params));
    queue.current = task;
    return task.then(setRouting);
  };
  const current = (source: Source) => {
    if (!routing) return undefined;
    if (source.stop === undefined) {
      const division = routing.divisions.find(d => d.idx === source.manual);
      return division && { own: division.own, sends: division.sends };
    }
    const stop = routing.stops.find(s => s.id === source.stop);
    return stop && { own: stop.own, sends: stop.sends ?? {} };
  };
  const change = (source: Source, params: Params, message: string) => {
    const before = current(source);
    if (before) {
      const now = Date.now();
      setHistory(h => {
        const last = h.at(-1);
        const same = last && last.source.manual === source.manual && last.source.stop === source.stop && now - last.at < 800;
        return same ? h : [...h, { source, ...before, at: now }];
      });
    }
    post({ ...sourceParams(source), ...params }).catch(() => setFailure(message));
  };
  const undo = () => {
    const last = history.at(-1);
    if (!last) return;
    setHistory(history.slice(0, -1));
    post({ ...sourceParams(last.source), ...(last.own ? { sends: sendsParam(last.sends) } : { follow: 1 }) })
      .catch(() => setFailure('That change could not be undone.'));
  };

  // The top bar's Undo steps back through this panel's edits while it is open.
  const latestUndo = useRef(undo);
  latestUndo.current = undo;
  const undoable = history.length > 0;
  useEffect(() => { offerUndo(undoable ? () => latestUndo.current() : undefined); }, [undoable, offerUndo]);
  useEffect(() => () => offerUndo(undefined), [offerUndo]);

  if (!routing) return <Stack align="center" p="xl"><Loader size="sm"/><Text c="dimmed">Loading routing</Text></Stack>;
  const expanded = open ?? [];
  const cells = (source: Source, name: string, sends: Sends | null, own: boolean) => routing.speakers.map(speaker => {
    const level = levelOf(sends, speaker.name);
    return <td key={speaker.name}>
      <SoundCell label={`${name} to ${speaker.name}`} level={level} inherited={!own} assign={() => setNotice(true)}
        connect={() => change(source, { speakers: speaker.name, level_db: 0 }, 'That connection could not be made.')}
        setLevel={db => change(source, { speakers: speaker.name, level_db: db }, 'That level could not be set.')}
        disconnect={() => change(source, { speakers: speaker.name, off: 1 }, 'That connection could not be removed.')}/>
    </td>;
  });
  const follow = (source: Source, label: string, name: string) =>
    <Button size="compact-sm" variant="subtle" color="gray" aria-label={`${name} ${label.toLowerCase()}`}
      onClick={() => change(source, { follow: 1 }, 'That routing could not be reset.')}>{label}</Button>;

  return <div className="route-panel">
    <Group className="route-header" justify="space-between">
      <Text fw={600}>Sound</Text>
      <Button variant="default" onClick={openSettings}>Speakers</Button>
    </Group>
    <div className="route-scroll">
      <table className="route-matrix">
        <thead><tr>
          <th className="route-corner"><Text size="xs" c="dimmed">dB</Text></th>
          {routing.speakers.map(speaker => <th key={speaker.name} scope="col">
            <Text fw={600}>{speaker.name}{!speaker.defined && <span className="modified" aria-label="Not in Settings"> ◇</span>}</Text>
            <Text size="xs" c="dimmed">{speaker.output ? `${speaker.output[0]}/${speaker.output[1]}${speaker.available ? '' : ' · not on device'}` : 'Plays through Main'}</Text>
          </th>)}
        </tr></thead>
        <tbody>{routing.divisions.map(division => {
          const isOpen = expanded.includes(division.idx);
          const stops = routing.stops.filter(stop => stop.midx === division.idx);
          const source = { manual: division.idx };
          return <Fragment key={division.idx}>
            <tr className="route-division">
              <th scope="row"><div className="route-source">
                <Button variant="subtle" color="gray" className="route-expand" aria-expanded={isOpen} disabled={!stops.length}
                  leftSection={isOpen ? <ChevronDown size={18}/> : <ChevronRight size={18}/>}
                  onClick={() => setOpen(isOpen ? expanded.filter(i => i !== division.idx) : [...expanded, division.idx])}>
                  <Text fw={600} span>{division.name}{division.own && <span className="modified" aria-label="Routing override"> ◇</span>}</Text>
                </Button>
                {division.own && follow(source, 'Reset', division.name)}
              </div></th>
              {cells(source, division.name, division.sends, true)}
            </tr>
            {isOpen && stops.map(stop => {
              const source = { manual: division.idx, stop: stop.id };
              return <tr key={stop.id} className="route-stop">
                <th scope="row"><div className="route-source">
                  <div className="route-stop-name"><span>{stop.name}{(stop.own || stop.file) && <span className="modified" aria-label="Routing override"> ◇</span>}</span>
                    {stop.file && !stop.own && <Text size="xs" c="dimmed">Organ file · {stop.file}</Text>}</div>
                  {stop.own && follow(source, 'Follow', stop.name)}
                </div></th>
                {cells(source, stop.name, stop.sends, stop.own)}
              </tr>;
            })}
          </Fragment>;
        })}</tbody>
      </table>
    </div>
    <Modal opened={notice} onClose={() => setNotice(false)} title="Assign control">
      <Stack><Text>Assigning a MIDI control or LFO to a level is not available yet.</Text><Button onClick={() => setNotice(false)}>Close</Button></Stack>
    </Modal>
    <Modal opened={Boolean(failure)} onClose={() => setFailure(undefined)} title="Routing not changed">
      <Stack><Text>{failure} Check the setting and try again.</Text><Button onClick={() => setFailure(undefined)}>Close</Button></Stack>
    </Modal>
  </div>;
}
