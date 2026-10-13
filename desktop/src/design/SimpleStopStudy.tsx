import { useRef, useState } from 'react';
import { Button, Drawer, Group, Menu, NumberInput, SegmentedControl, Slider, Stack, Text } from '@mantine/core';
import { compass, formatHz, keyCents, keyName, type Tuning } from './limitsModel';
import './simple.css';

/** Design study: a stop as a short list of sounds in organ words. Silent; nothing is saved. */
type Source = { id: string; name: string; division: string; footage: number; keys: [number, number]; keyboard?: boolean };
type Ending = { kind: 'release' } | { kind: 'after'; ms: number } | { kind: 'hold'; ms: number };
type Sound = { id: string; source: string; cents: number; level: number; keys: [number, number] | null; ceiling: number | null; delay: number; ending: Ending };
type Picking = { sound: string; mode: 'keys' | 'ceiling' };

const sources: Source[] = [
  { id: 'bourdon', name: 'Bourdon 8′', division: 'Grand-orgue', footage: 8, keys: [36, 89] },
  { id: 'prestant', name: 'Prestant 4′', division: 'Grand-orgue', footage: 4, keys: [36, 89] },
  { id: 'nasard', name: 'Nasard 2⅔′', division: 'Grand-orgue', footage: 8 / 3, keys: [36, 89] },
  { id: 'doublette', name: 'Doublette 2′', division: 'Grand-orgue', footage: 2, keys: [36, 89] },
  { id: 'tierce', name: 'Tierce 1⅗′', division: 'Grand-orgue', footage: 8 / 5, keys: [36, 89] },
  { id: 'larigot', name: 'Larigot 1⅓′', division: 'Grand-orgue', footage: 4 / 3, keys: [36, 89] },
  { id: 'flute', name: 'Flûte 8′', division: 'Positif', footage: 8, keys: [36, 89] },
  { id: 'pos', name: 'Positif', division: 'Positif', footage: 8, keys: [36, 89], keyboard: true },
  { id: 'pedal-bourdon', name: 'Bourdon 8′', division: 'Pédale', footage: 8, keys: [36, 62] },
  { id: 'ped', name: 'Pédale', division: 'Pédale', footage: 8, keys: [36, 62], keyboard: true },
];
const intervals = [
  { cents: -2400, name: 'two octaves down' }, { cents: -1200, name: 'an octave down' }, { cents: 0, name: 'as it is' },
  { cents: 702, name: 'a fifth up' }, { cents: 1200, name: 'an octave up' }, { cents: 1902, name: 'a twelfth up' },
  { cents: 2400, name: 'two octaves up' }, { cents: 2786, name: 'a seventeenth up' }, { cents: 3102, name: 'a nineteenth up' },
];
const sound = (id: string, source: string, cents: number, keys: [number, number] | null = null): Sound =>
  ({ id, source, cents, level: 0, keys, ceiling: null, delay: 0, ending: { kind: 'release' } });
const pleinJeu: Sound[] = [
  sound('a', 'bourdon', 0), sound('b', 'prestant', 0),
  sound('c', 'doublette', 0, [36, 59]), sound('d', 'doublette', -1200, [60, 89]),
  sound('e', 'larigot', 0, [36, 59]), sound('f', 'larigot', -1200, [60, 89]),
  sound('g', 'ped', 0),
];

const sourceOf = (s: Sound) => sources.find(x => x.id === s.source)!;
const sourceName = (source: Source) => source.keyboard ? `Couple the ${source.name}` : source.division === 'Grand-orgue' ? source.name : `${source.name} from ${source.division}`;
const fraction = (feet: number) => {
  const known: [number, string][] = [[32, '32'], [16, '16'], [8, '8'], [16 / 3, '5⅓'], [4, '4'], [8 / 3, '2⅔'], [2, '2'], [8 / 5, '1⅗'], [4 / 3, '1⅓'], [8 / 7, '1⅐'], [1, '1'], [2 / 3, '⅔'], [1 / 2, '½']];
  const hit = known.find(([value]) => Math.abs(value - feet) / feet < .004);
  return hit ? `${hit[1]}′` : `${Number(feet.toPrecision(3))}′`;
};
const feetOf = (s: Sound) => sourceOf(s).footage * 2 ** (-s.cents / 1200);
const pitchWords = (s: Sound) => {
  const named = intervals.find(i => Math.abs(i.cents - s.cents) < .5)?.name ?? `${s.cents > 0 ? '+' : ''}${Math.round(s.cents)} ¢`;
  return sourceOf(s).keyboard ? named : `${named} · ${fraction(feetOf(s))}`;
};
/** The pitch a sound gives one key, in cents from the reference (8′ basis). */
const soundingAt = (s: Sound, key: number, tuning: Tuning) => keyCents(key, tuning) + 1200 * Math.log2(8 / feetOf(s));
type Status = 'plays' | 'off' | 'ceiling' | 'missing';
function statusAt(s: Sound, key: number, tuning: Tuning): Status {
  if (s.keys && (key < s.keys[0] || key > s.keys[1])) return 'off';
  const pitch = soundingAt(s, key, tuning);
  if (s.ceiling !== null && pitch > s.ceiling + .5) return 'ceiling';
  const source = sourceOf(s);
  // Coupled keyboards stop at their compass; ranks stop where their pipes do.
  if (source.keyboard) return key < source.keys[0] || key > source.keys[1] ? 'missing' : 'plays';
  const ladder = 69 + (pitch - 1200 * Math.log2(8 / source.footage)) / 100;
  return ladder < source.keys[0] - .5 || ladder > source.keys[1] + .5 ? 'missing' : 'plays';
}
const keysWords = (s: Sound, tuning: Tuning) => s.keys ? `${keyName(s.keys[0], tuning)} – ${keyName(s.keys[1], tuning)}` : 'all keys';
const black = (key: number, tuning: Tuning) => tuning === 12 && [1, 3, 6, 8, 10].includes(key % 12);

/** A keyboard drawn across the compass: tap or drag to choose keys. */
function Keyboard({ tuning, chosen, marks, choose, height = 56 }: {
  tuning: Tuning; chosen?: [number, number] | null; marks?: (key: number) => string; choose: (first: number, last: number) => void; height?: number;
}) {
  const host = useRef<HTMLDivElement>(null);
  const drag = useRef<number | null>(null);
  const keyAt = (clientX: number) => {
    const rect = host.current!.getBoundingClientRect();
    return compass[Math.max(0, Math.min(compass.length - 1, Math.floor((clientX - rect.left) / rect.width * compass.length)))];
  };
  return <div ref={host} className="simple-keyboard" style={{ height }} role="group" aria-label="Keyboard"
    onPointerDown={e => { e.currentTarget.setPointerCapture(e.pointerId); drag.current = keyAt(e.clientX); choose(drag.current, drag.current); }}
    onPointerMove={e => { if (drag.current !== null) { const key = keyAt(e.clientX); choose(Math.min(drag.current, key), Math.max(drag.current, key)); } }}
    onPointerUp={() => { drag.current = null; }}>
    {compass.map(key => <div key={key} aria-label={keyName(key, tuning)}
      className={`simple-key ${black(key, tuning) ? 'is-black' : ''} ${chosen && key >= chosen[0] && key <= chosen[1] ? 'is-chosen' : ''} ${marks?.(key) ?? ''}`}/>)}
  </div>;
}

/** Where one sound plays across the keyboard: filled, hatched where it has no pipe, empty where it is off. */
function Coverage({ s, tuning }: { s: Sound; tuning: Tuning }) {
  return <div className="simple-coverage" aria-hidden="true">{compass.map(key => <span key={key} className={`is-${statusAt(s, key, tuning)}`}/>)}</div>;
}

export function SimpleStopStudy() {
  const [sounds, setSounds] = useState<Sound[]>(pleinJeu);
  const [history, setHistory] = useState<Sound[][]>([]);
  const [tuning, setTuning] = useState<Tuning>(12);
  const [tried, setTried] = useState(60);
  const [adding, setAdding] = useState<string | null>(null);
  const [picking, setPicking] = useState<Picking | null>(null);
  const [range, setRange] = useState<[number, number] | null>(null);
  const [timing, setTiming] = useState<string | null>(null);
  const [fine, setFine] = useState<string | null>(null);
  const edit = (next: Sound[]) => { setHistory(h => [...h, sounds]); setSounds(next); };
  const change = (id: string, patch: Partial<Sound>) => edit(sounds.map(s => s.id === id ? { ...s, ...patch } : s));
  const fresh = () => `s${Math.random().toString(36).slice(2, 8)}`;
  const picked = sounds.find(s => s.id === picking?.sound);
  const timed = sounds.find(s => s.id === timing);
  const tuned = sounds.find(s => s.id === fine);
  const divisions = [...new Set(sources.map(s => s.division))];
  const openKeys = (s: Sound, mode: Picking['mode']) => { setPicking({ sound: s.id, mode }); setRange(mode === 'keys' ? s.keys : null); };
  // "Split here": keys from the split point up become a copy of the sound an octave lower — a mixture break.
  const split = (s: Sound, at: number) => {
    const [low, high] = s.keys ?? [compass[0], compass.at(-1)!];
    if (at <= low || at > high) return;
    const above = { ...s, id: fresh(), cents: s.cents - 1200, keys: [at, high] as [number, number] };
    edit(sounds.flatMap(x => x.id === s.id ? [{ ...x, keys: [low, at - 1] as [number, number] }, above] : [x]));
    setPicking(null);
  };
  const playing = sounds.map(s => ({ s, status: statusAt(s, tried, tuning) }));

  return <Stack gap="md" className="simple-stop">
    <Group justify="space-between" align="end">
      <div><Text fw={600} size="lg">Plein jeu</Text><Text c="dimmed" size="sm">Grand-orgue · {sounds.length} sounds</Text></div>
      <Group gap="xs">
        <SegmentedControl aria-label="Tuning" value={String(tuning)} onChange={v => setTuning(Number(v) as Tuning)} data={[{ value: '12', label: '12 equal' }, { value: '19', label: '19 equal' }]}/>
        <Button variant="default" disabled={!history.length} onClick={() => { setSounds(history.at(-1)!); setHistory(history.slice(0, -1)); }}>Undo</Button>
      </Group>
    </Group>

    <section className="simple-try">
      <Group justify="space-between" mb={6}><Text fw={600}>Try a key</Text><Text c="dimmed" size="sm">{keyName(tried, tuning)} · {formatHz(keyCents(tried, tuning))}</Text></Group>
      <Keyboard tuning={tuning} chosen={[tried, tried]} choose={key => setTried(key)} height={44}/>
      <div className="simple-try-list">{playing.map(({ s, status }) => <span key={s.id} className={`is-${status}`}>
        {sourceName(sourceOf(s))}{status === 'plays' ? ` · ${formatHz(soundingAt(s, tried, tuning))}` : status === 'missing' ? ' · no pipe here' : ' · not on this key'}
      </span>)}</div>
    </section>

    <ol className="simple-sounds">{sounds.map(s => <li key={s.id} className="simple-sound">
      <div className="simple-sentence">
        <Button variant="default" className="simple-word" onClick={() => setAdding(s.id)}>{sourceName(sourceOf(s))}</Button>
        <Menu position="bottom-start">
          <Menu.Target><Button variant="default" className="simple-word" aria-label={`Pitch: ${pitchWords(s)}`}>{pitchWords(s)}</Button></Menu.Target>
          <Menu.Dropdown>{intervals.map(i => <Menu.Item key={i.cents} onClick={() => change(s.id, { cents: i.cents })}>
            {i.name}{!sourceOf(s).keyboard && <Text span c="dimmed" size="sm"> · {fraction(sourceOf(s).footage * 2 ** (-i.cents / 1200))}</Text>}</Menu.Item>)}
            <Menu.Divider/><Menu.Item onClick={() => setFine(s.id)}>Fine-tune…</Menu.Item></Menu.Dropdown>
        </Menu>
        <span className="simple-joiner">on</span>
        <Button variant="default" className="simple-word" aria-label={`Keys: ${keysWords(s, tuning)}`} onClick={() => openKeys(s, 'keys')}>{keysWords(s, tuning)}</Button>
        {s.ceiling !== null && <Button variant="subtle" className="simple-word" onClick={() => openKeys(s, 'ceiling')}>never above {formatHz(s.ceiling)}</Button>}
        {(s.delay > 0 || s.ending.kind !== 'release') && <Button variant="subtle" className="simple-word" onClick={() => setTiming(s.id)}>{timingWords(s)}</Button>}
        <div className="simple-level"><Slider aria-label="Level" min={-24} max={6} step={1} value={s.level} label={v => `${v > 0 ? '+' : ''}${v} dB`} onChangeEnd={level => change(s.id, { level })}/></div>
        <Menu position="bottom-end">
          <Menu.Target><Button variant="subtle" aria-label={`More for ${sourceName(sourceOf(s))}`}>⋯</Button></Menu.Target>
          <Menu.Dropdown>
            <Menu.Item onClick={() => openKeys(s, 'keys')}>Split at a key…</Menu.Item>
            <Menu.Item onClick={() => setTiming(s.id)}>Timing…</Menu.Item>
            <Menu.Item onClick={() => setFine(s.id)}>Fine-tune…</Menu.Item>
            <Menu.Item onClick={() => edit([...sounds, { ...s, id: fresh() }])}>Duplicate</Menu.Item>
            <Menu.Item color="red" disabled={sounds.length === 1} onClick={() => edit(sounds.filter(x => x.id !== s.id))}>Remove</Menu.Item>
          </Menu.Dropdown>
        </Menu>
      </div>
      <Coverage s={s} tuning={tuning}/>
    </li>)}</ol>
    <Group><Button onClick={() => setAdding('new')}>+ Add a sound</Button>
      <Text c="dimmed" size="sm"><span className="legend is-plays"/> plays <span className="legend is-missing"/> no pipe there</Text></Group>

    <Drawer opened={adding !== null} onClose={() => setAdding(null)} title={adding === 'new' ? 'Add a sound' : 'Play instead'} position="right" closeButtonProps={{ 'aria-label': 'Close' }}>
      <Stack gap="md">{divisions.map(division => <Stack key={division} gap={4}><Text size="sm" c="dimmed">{division}</Text>
        {sources.filter(x => x.division === division).map(x => <Button key={x.id} variant="default" justify="start" onClick={() => {
          if (adding === 'new') edit([...sounds, sound(fresh(), x.id, 0)]); else if (adding) change(adding, { source: x.id });
          setAdding(null);
        }}>{x.keyboard ? `Couple the ${x.name}` : x.name}</Button>)}
      </Stack>)}</Stack>
    </Drawer>

    <Drawer opened={Boolean(picked)} onClose={() => setPicking(null)} position="bottom" size="auto" closeButtonProps={{ 'aria-label': 'Close' }}
      title={picking?.mode === 'ceiling' ? 'Never sounds higher than…' : `Which keys play ${picked ? sourceName(sourceOf(picked)) : ''}?`}>
      {picked && picking && <Stack gap="sm">
        <Text size="sm" c="dimmed">{picking.mode === 'keys' ? 'Drag across the keys, or tap one key to split there.' : 'Tap the key whose pitch is the highest it may sound.'}</Text>
        <Keyboard tuning={tuning} chosen={picking.mode === 'keys' ? range : null} choose={(first, last) => {
          if (picking.mode === 'ceiling') { change(picked.id, { ceiling: keyCents(first, tuning) }); setPicking(null); } else setRange([first, last]);
        }}/>
        {picking.mode === 'keys' && <Group justify="space-between">
          <Text>{range ? `${keyName(range[0], tuning)} – ${keyName(range[1], tuning)}` : 'All keys'}</Text>
          <Group gap="xs">
            <Button variant="default" onClick={() => { change(picked.id, { keys: null }); setPicking(null); }}>All keys</Button>
            {range && range[0] === range[1] && <Button variant="default" onClick={() => split(picked, range[0])}>Split here: from {keyName(range[0], tuning)} up, an octave lower</Button>}
            <Button disabled={!range} onClick={() => { change(picked.id, { keys: range }); setPicking(null); }}>Done</Button>
          </Group>
        </Group>}
        {picking.mode === 'ceiling' && <Group><Button variant="default" onClick={() => { change(picked.id, { ceiling: null }); setPicking(null); }}>No limit</Button></Group>}
      </Stack>}
    </Drawer>

    <Drawer opened={Boolean(timed)} onClose={() => setTiming(null)} title="Timing" position="right" closeButtonProps={{ 'aria-label': 'Close' }}>
      {timed && <Stack>
        <NumberInput label="Starts after the key, in ms" min={0} max={30000} value={timed.delay} onChange={v => typeof v === 'number' && change(timed.id, { delay: v })}/>
        <SegmentedControl orientation="vertical" value={timed.ending.kind} onChange={kind => change(timed.id, { ending: kind === 'release' ? { kind } : { kind: kind as 'after' | 'hold', ms: 200 } })}
          data={[{ value: 'release', label: 'Stops when you let go' }, { value: 'after', label: 'Stops after a time' }, { value: 'hold', label: 'Keeps sounding after you let go' }]}/>
        {timed.ending.kind !== 'release' && <NumberInput label={timed.ending.kind === 'after' ? 'Stops after, in ms' : 'Keeps sounding for, in ms'} min={1} max={30000} value={timed.ending.ms}
          onChange={v => typeof v === 'number' && change(timed.id, { ending: { kind: timed.ending.kind as 'after' | 'hold', ms: v } })}/>}
        <Text size="sm" c="dimmed">Echoes, canons and sounds after release live in the piano roll.</Text>
        <Button variant="default" component="a" href="/?study=1&panel=build&layout=split">Open the piano roll</Button>
      </Stack>}
    </Drawer>

    <Drawer opened={Boolean(tuned)} onClose={() => setFine(null)} title="Fine-tune" position="right" closeButtonProps={{ 'aria-label': 'Close' }}>
      {tuned && <Stack>
        <NumberInput label="Pitch, in cents from the sound as it is" min={-9600} max={9600} value={Math.round(tuned.cents * 10) / 10} onChange={v => typeof v === 'number' && change(tuned.id, { cents: v })}/>
        <Text size="sm" c="dimmed">{pitchWords(tuned)}</Text>
        <Button variant="default" onClick={() => { setFine(null); openKeys(tuned, 'ceiling'); }}>{tuned.ceiling === null ? 'Never sounds higher than…' : `Never above ${formatHz(tuned.ceiling)} · change`}</Button>
      </Stack>}
    </Drawer>
  </Stack>;
}

function timingWords(s: Sound) {
  const start = s.delay > 0 ? `${s.delay} ms late` : '';
  const end = s.ending.kind === 'after' ? `for ${s.ending.ms} ms` : s.ending.kind === 'hold' ? `+${s.ending.ms} ms after release` : '';
  return [start, end].filter(Boolean).join(', ');
}
