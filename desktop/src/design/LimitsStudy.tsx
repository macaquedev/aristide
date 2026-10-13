import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Button, Group, NumberInput, Popover, SegmentedControl, Stack, Text } from '@mantine/core';
import { NumberControl } from './NumberControl';
import { RollCanvas, type RollView } from './RollCanvas';
import { cents as rollCents, type PitchWindow } from './rollModel';
import {
  compass, formatCents, formatHz, keyCents, keyName, pipeSources, pleinJeu, referenceName, rollModel, sourceLabel, status, stepCents,
  type LimitNote, type Status, type Tuning,
} from './limitsModel';
import './roll.css';

export const limitViews = [{ value: 'chart', label: 'Pitch chart' }, { value: 'breaks', label: 'Break chart' }, { value: 'strips', label: 'Key strips' }, { value: 'sentence', label: 'Sentence only' }];
export const overlapModes = [{ value: 'offset', label: 'Offset (now)' }, { value: 'stack', label: 'Stack' }, { value: 'merge', label: 'Merge' }, { value: 'lanes', label: 'Lanes' }];
type Field = 'played' | 'sounding';
type Edge = 'low' | 'high';
const home: RollView = { time: 0, center: 1200, span: 120, pitchSpan: 4800 };
const statusWords: Record<Status, string> = { sounds: 'sounds', key: 'outside its keys', sounding: 'outside its sounding range', missing: 'no pipe' };
const sourceOf = (note: LimitNote) => pipeSources.find(s => s.id === note.source)!;

function useWidth() {
  const host = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(800);
  useEffect(() => {
    const observer = new ResizeObserver(entries => setWidth(entries[0].contentRect.width));
    observer.observe(host.current!);
    return () => observer.disconnect();
  }, []);
  return [host, width] as const;
}

function setEdge(window: PitchWindow | null | undefined, edge: Edge, value: number | undefined): PitchWindow | null {
  const next: PitchWindow = { ...window, [edge]: value };
  if (value === undefined) delete next[edge];
  return next.low === undefined && next.high === undefined ? null : next;
}

/** One tappable value inside the sentence: open ("the lowest key", "any pitch") or a pitch in cents and Hz. */
function Chip({ field, edge, note, tuning, update }: { field: Field; edge: Edge; note: LimitNote; tuning: Tuning; update: (note: LimitNote) => void }) {
  const [opened, setOpened] = useState(false);
  const value = note[field]?.[edge];
  const other = note[field]?.[edge === 'low' ? 'high' : 'low'];
  const open = `the ${edge === 'low' ? 'lowest' : 'highest'} ${field === 'played' ? 'key' : 'pitch'}`;
  const set = (next: number | undefined) => update({ ...note, [field]: setEdge(note[field], edge, next) });
  const inside = value === undefined ? [] : compass.filter(key => edge === 'low' ? keyCents(key, tuning) >= value : keyCents(key, tuning) <= value);
  const nearest = field === 'played' && inside.length ? keyName(edge === 'low' ? inside[0] : inside.at(-1)!, tuning) : undefined;
  return <Popover opened={opened} onChange={setOpened} position="bottom" withArrow trapFocus>
    <Popover.Target><button type="button" className={`limit-chip ${value === undefined ? 'is-open' : ''}`} aria-label={`${field === 'played' ? 'Key' : 'Sounding'} ${edge === 'low' ? 'from' : 'to'}: ${value === undefined ? open : formatCents(value)}`} onClick={() => setOpened(true)}>
      {value === undefined ? open : <>{formatCents(value)} <span>{formatHz(value)}</span></>}
    </button></Popover.Target>
    <Popover.Dropdown><Stack gap="xs" w={220}>
      <NumberInput label="Cents" value={value ?? ''} placeholder={open} step={stepCents(tuning)} min={edge === 'high' && other !== undefined ? other : -9600} max={edge === 'low' && other !== undefined ? other : 9600}
        onChange={v => set(typeof v === 'number' ? v : undefined)} data-autofocus/>
      {value !== undefined && <Text size="xs" c="dimmed">{formatHz(value)}{nearest ? ` · ${edge === 'low' ? 'from' : 'up to'} ${nearest}` : ''}</Text>}
      <Group grow><Button variant="default" onClick={() => { set(undefined); setOpened(false); }}>No limit</Button><Button onClick={() => setOpened(false)}>Done</Button></Group>
    </Stack></Popover.Dropdown>
  </Popover>;
}

/** The limits read as one sentence, every value editable in place. */
function LimitSentence({ note, tuning, update, compact = false }: { note: LimitNote; tuning: Tuning; update: (note: LimitNote) => void; compact?: boolean }) {
  const chip = (field: Field, edge: Edge) => <Chip field={field} edge={edge} note={note} tuning={tuning} update={update}/>;
  return <div className={`limit-sentence ${compact ? 'is-compact' : ''}`}>
    <p>Plays when the key is between {chip('played', 'low')} and {chip('played', 'high')},</p>
    <p>and only while it sounds between {chip('sounding', 'low')} and {chip('sounding', 'high')}.</p>
    <p className="limit-reference">Pitches relative to {referenceName(tuning)}.</p>
  </div>;
}

/**
 * Across: the division's keys. Up: the pitch that sounds (`pitch`), or the offset from the key (`offset`, an
 * organ builder's break chart, where a mixture's breaks are steps). Each event is a line over the keys it plays.
 */
function CompassChart({ notes, selected, select, update, tuning, probe, setProbe, axis }: {
  notes: LimitNote[]; selected: string; select: (id: string) => void; update: (note: LimitNote) => void; tuning: Tuning; probe: number; setProbe: (key: number) => void; axis: 'pitch' | 'offset';
}) {
  const [host, width] = useWidth();
  const svg = useRef<SVGSVGElement>(null);
  const [drag, setDrag] = useState<{ field: Field; edge: Edge } | null>(null);
  const [draft, setDraft] = useState<LimitNote>();
  const height = 416, left = 112, right = 16, top = 12, bottom = height - 60;
  const [lowC, highC] = axis === 'pitch' ? [-3700, 5300] : [-1500, 3500];
  const column = (width - left - right) / compass.length;
  const xKey = (index: number) => left + (index + .5) * column;
  const y = (c: number) => top + (highC - c) / (highC - lowC) * (bottom - top);
  const step = stepCents(tuning), first = keyCents(compass[0], tuning);
  const xCents = (c: number) => xKey((c - first) / step);
  const centsAtX = (px: number) => first + ((px - left) / column - .5) * step;
  const valueAtY = (py: number) => highC - (py - top) / (bottom - top) * (highC - lowC);
  const plotted = (key: number, offset: number) => axis === 'pitch' ? keyCents(key, tuning) + offset : offset;
  const shown = notes.map(n => n.id === draft?.id ? draft : n);
  const note = shown.find(n => n.id === selected);
  const point = (event: { clientX: number; clientY: number }) => {
    const rect = svg.current!.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  };
  // Coincident lines run side by side, like a transit map, instead of hiding each other.
  const lanes = new Map<string, number>();
  const lane = (n: LimitNote) => {
    const twins = shown.filter(o => o.pitch === n.pitch && JSON.stringify(o.played) === JSON.stringify(n.played));
    lanes.set(n.id, twins.findIndex(o => o.id === n.id) - (twins.length - 1) / 2);
    return lanes.get(n.id)! * 6;
  };
  const runs = (n: LimitNote) => {
    const out: { kind: Status; points: [number, number][] }[] = [];
    const dy = lane(n);
    compass.forEach((key, i) => {
      const kind = status(n, key, tuning);
      const p: [number, number] = [xKey(i), y(plotted(key, n.pitch)) + dy];
      const last = out.at(-1);
      if (last && last.kind === kind) last.points.push(p); else out.push({ kind, points: [p] });
    });
    return out;
  };
  const path = (points: [number, number][]) => points.length === 1 ? `M${points[0][0] - column / 2} ${points[0][1]} h${column}` : `M${points.map(p => p.join(' ')).join(' L')}`;
  const grid = (axis === 'pitch' ? Array.from({ length: 9 }, (_, i) => -3600 + i * 1200) : Array.from({ length: 9 }, (_, i) => -1200 + i * 600))
    .filter(c => y(c) > top + 16 && y(c) < bottom - 16);
  const edgeX = (edge: Edge) => note?.played?.[edge] !== undefined ? xCents(note.played[edge]!) : edge === 'low' ? left : width - right;
  // Open edges park their tab just inside the plot, clear of the axis labels and the other tabs.
  const parked = (edge: Edge) => edge === 'low' ? bottom - 14 : top + 14;
  const edgeY = (edge: Edge) => note?.sounding?.[edge] !== undefined ? Math.max(top, Math.min(bottom, y(axis === 'pitch' ? note.sounding[edge]! : note.sounding[edge]! - keyCents(compass.at(-1)!, tuning)))) : parked(edge);
  // A sounding limit seen as an offset from the key closes in as the keys rise: a diagonal.
  const soundingLine = (value: number) => axis === 'pitch' ? `M${left} ${y(value)} H${width - right}` : `M${compass.map((key, i) => `${xKey(i)} ${y(value - keyCents(key, tuning))}`).join(' L')}`;
  const move = (event: React.PointerEvent) => {
    if (!drag || !note) return;
    const pos = point(event);
    let value: number | undefined;
    if (drag.field === 'played') {
      const boundary = Math.round((pos.x - left) / column);
      value = boundary <= 0 || boundary >= compass.length ? undefined : first + (boundary - .5) * step;
    } else {
      value = pos.y <= top || pos.y >= bottom ? undefined : Math.round((valueAtY(pos.y) + (axis === 'pitch' ? 0 : centsAtX(pos.x))) / 10) * 10;
    }
    const other = note[drag.field]?.[drag.edge === 'low' ? 'high' : 'low'];
    if (value !== undefined && other !== undefined && (drag.edge === 'low' ? value > other : value < other)) value = other;
    setDraft({ ...note, [drag.field]: setEdge(note[drag.field], drag.edge, value) });
  };
  const handle = (field: Field, edge: Edge) => {
    const value = note?.[field]?.[edge];
    const label = value === undefined ? field === 'played' ? edge === 'low' ? 'lowest' : 'highest' : 'any' : formatCents(value);
    const name = `${field === 'played' ? 'Key' : 'Sounding'} ${edge === 'low' ? 'from' : 'to'}`;
    const grab = (event: React.PointerEvent) => { event.stopPropagation(); svg.current!.setPointerCapture(event.pointerId); setDrag({ field, edge }); };
    if (field === 'played') {
      const x = edgeX(edge), tab = Math.max(left + 32, Math.min(width - right - 32, x));
      return <g className={`limit-handle ${value === undefined ? 'is-open' : ''}`} role="slider" aria-label={name} aria-valuetext={label} onPointerDown={grab}>
        <line x1={x} x2={x} y1={top} y2={bottom}/><rect x={tab - 32} y={bottom + 4} width={64} height={22} rx={4}/><text x={tab} y={bottom + 19} textAnchor="middle">{label}</text>
        <rect className="limit-hit" x={x - 12} y={top} width={24} height={bottom - top + 28}/>
      </g>;
    }
    const yy = edgeY(edge), tabX = axis === 'pitch' ? 4 : width - right - (left - 10);
    return <g className={`limit-handle ${value === undefined ? 'is-open' : ''}`} role="slider" aria-label={name} aria-valuetext={label} onPointerDown={grab}>
      {value !== undefined ? <path d={soundingLine(value)} className="limit-line"/> : <line x1={left} x2={width - right} y1={yy} y2={yy}/>}<rect x={tabX} y={yy - 11} width={left - 10} height={22} rx={4}/><text x={tabX + (left - 10) / 2} y={yy + 4} textAnchor="middle">{edge === 'low' ? '≥ ' : '≤ '}{label}</text>
      <rect className="limit-hit" x={0} y={yy - 12} width={width} height={24}/>
    </g>;
  };
  return <div ref={host} className="compass-chart">
    <svg ref={svg} width="100%" height={height} role="group" aria-label="Compass chart"
      onPointerMove={move}
      onPointerUp={e => { if (draft) update(draft); setDraft(undefined); setDrag(null); if (svg.current!.hasPointerCapture(e.pointerId)) svg.current!.releasePointerCapture(e.pointerId); }}
      onPointerCancel={() => { setDraft(undefined); setDrag(null); }}>
      <rect className="roll-background" width={width} height={height}/>
      {note && <g className="limit-shade">
        {note.played?.low !== undefined && <rect x={left} y={top} width={Math.max(0, edgeX('low') - left)} height={bottom - top}/>}
        {note.played?.high !== undefined && <rect x={edgeX('high')} y={top} width={Math.max(0, width - right - edgeX('high'))} height={bottom - top}/>}
        {note.sounding?.high !== undefined && <path d={`${soundingLine(note.sounding.high)} V${top} H${left} Z`}/>}
        {note.sounding?.low !== undefined && <path d={`${soundingLine(note.sounding.low)} V${bottom} H${left} Z`}/>}
      </g>}
      <rect className="chart-probe" x={left + (probe - compass[0]) * column} y={top} width={column} height={bottom - top + 28}/>
      {grid.map(c => <g key={c}><line className={c === 0 ? 'roll-zero' : 'roll-guide'} x1={left} x2={width - right} y1={y(c)} y2={y(c)}/>
        <text className={c === 0 ? 'roll-zero-label' : 'roll-axis-label'} x={left - 10} y={y(c) + (axis === 'pitch' && c !== 0 ? -2 : 4)} textAnchor="end">{c === 0 ? axis === 'pitch' ? referenceName(tuning) : 'key’s pitch' : formatCents(c)}</text>
        {c !== 0 && axis === 'pitch' && <text className="roll-axis-label" x={left - 10} y={y(c) + 11} textAnchor="end">{formatHz(c)}</text>}</g>)}
      <g className="chart-keys">{compass.map((key, i) => {
        const black = tuning === 12 && [1, 3, 6, 8, 10].includes(key % 12);
        return <rect key={key} className={black ? 'is-black' : ''} x={left + i * column + .5} y={bottom + 30} width={column - 1} height={12} onPointerDown={() => setProbe(key)}/>;
      })}</g>
      {compass.map((key, i) => (tuning === 12 ? key % 12 === 0 : (i % 9 === 0)) && <text key={key} className="roll-axis-label" x={xKey(i)} y={height - 3} textAnchor="middle">{keyName(key, tuning)}</text>)}
      <rect className="chart-hit" x={left} y={top} width={width - left - right} height={bottom - top} onPointerDown={e => { const index = Math.floor((point(e).x - left) / column); if (compass[index]) setProbe(compass[index]); }}/>
      {[...shown].sort((a, b) => Number(a.id === selected) - Number(b.id === selected)).map(n => <g key={n.id} data-note={n.id} className={`chart-line ${n.id === selected ? 'is-selected' : ''}`} onPointerDown={() => select(n.id)}>
        <title>{sourceLabel(sourceOf(n))} {rollCents(n.pitch)}</title>
        {runs(n).map((run, i) => run.kind === 'sounds' ? <path key={i} className="chart-sounds" d={path(run.points)}/>
          : run.kind === 'missing' ? <path key={i} className="chart-missing" d={path(run.points)}/>
          : n.id === selected ? <path key={i} className="chart-limited" d={path(run.points)}/> : null)}
        {n.id === selected && (() => { const firstRun = runs(n).find(r => r.kind === 'sounds'); return firstRun && <text className="chart-label" x={firstRun.points[0][0] + 4} y={firstRun.points[0][1] - 7}>{sourceLabel(sourceOf(n))} {rollCents(n.pitch)}</text>; })()}
      </g>)}
      {note && <>{handle('played', 'low')}{handle('played', 'high')}{handle('sounding', 'low')}{handle('sounding', 'high')}</>}
    </svg>
  </div>;
}

/** One strip per event across the keys: filled where it sounds, hatched where its rank has no pipe. */
function KeyStrips({ notes, selected, select, update, tuning, probe, setProbe }: {
  notes: LimitNote[]; selected: string; select: (id: string) => void; update: (note: LimitNote) => void; tuning: Tuning; probe: number; setProbe: (key: number) => void;
}) {
  const [host, width] = useWidth();
  const svg = useRef<SVGSVGElement>(null);
  const [draft, setDraft] = useState<LimitNote>();
  const [drag, setDrag] = useState<Edge | null>(null);
  const left = 190, right = 12, row = 30, head = 30;
  const height = head + notes.length * row + 8;
  const column = (width - left - right) / compass.length;
  const step = stepCents(tuning), first = keyCents(compass[0], tuning);
  const shown = notes.map(n => n.id === draft?.id ? draft : n);
  const note = shown.find(n => n.id === selected);
  const boundaryX = (c: number | undefined, edge: Edge) => c === undefined ? edge === 'low' ? left : width - right : left + ((c - first) / step + .5) * column;
  return <div ref={host} className="key-strips">
    <svg ref={svg} width="100%" height={height} role="group" aria-label="Key strips"
      onPointerMove={e => {
        if (!drag || !note) return;
        const boundary = Math.round((e.clientX - svg.current!.getBoundingClientRect().left - left) / column);
        let value = boundary <= 0 || boundary >= compass.length ? undefined : first + (boundary - .5) * step;
        const other = note.played?.[drag === 'low' ? 'high' : 'low'];
        if (value !== undefined && other !== undefined && (drag === 'low' ? value > other : value < other)) value = other;
        setDraft({ ...note, played: setEdge(note.played, drag, value) });
      }}
      onPointerUp={e => { if (draft) update(draft); setDraft(undefined); setDrag(null); if (svg.current!.hasPointerCapture(e.pointerId)) svg.current!.releasePointerCapture(e.pointerId); }}>
      <defs><pattern id="strip-hatch" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><line x1="0" y1="0" x2="0" y2="6" className="hatch-line"/></pattern></defs>
      <rect className="roll-background" width={width} height={height}/>
      <rect className="chart-probe" x={left + (probe - compass[0]) * column} y={0} width={column} height={height}/>
      {compass.map((key, i) => {
        const black = tuning === 12 && [1, 3, 6, 8, 10].includes(key % 12);
        return <g key={key} onPointerDown={() => setProbe(key)}>
          <rect className={`strip-key ${black ? 'is-black' : ''}`} x={left + i * column + .5} y={head - 12} width={column - 1} height={10}/>
          {(tuning === 12 ? key % 12 === 0 : i % 9 === 0) && <text className="roll-axis-label" x={left + (i + .5) * column} y={12} textAnchor="middle">{keyName(key, tuning)}</text>}
        </g>;
      })}
      {shown.map((n, r) => {
        const y0 = head + r * row;
        const isSelected = n.id === selected;
        return <g key={n.id} data-note={n.id} className={`strip-row ${isSelected ? 'is-selected' : ''}`} onPointerDown={() => select(n.id)}>
          <rect className="strip-row-hit" x={0} y={y0} width={width} height={row}/>
          <text className="strip-label" x={8} y={y0 + 19}>{sourceLabel(sourceOf(n))}</text>
          <text className="strip-pitch" x={left - 8} y={y0 + 19} textAnchor="end">{rollCents(n.pitch)}</text>
          {compass.map((key, i) => {
            const kind = status(n, key, tuning);
            return <rect key={key} className={`strip-cell is-${kind}`} x={left + i * column + .5} y={y0 + 6} width={column - 1} height={row - 12}
              fill={kind === 'missing' ? 'url(#strip-hatch)' : undefined}><title>{keyName(key, tuning)} · {statusWords[kind]}</title></rect>;
          })}
          {isSelected && (['low', 'high'] as const).map(edge => {
            const x = boundaryX(n.played?.[edge], edge);
            return <g key={edge} className={`limit-handle ${n.played?.[edge] === undefined ? 'is-open' : ''}`} role="slider" aria-label={`Key ${edge === 'low' ? 'from' : 'to'}`}
              onPointerDown={e => { e.stopPropagation(); svg.current!.setPointerCapture(e.pointerId); setDrag(edge); }}>
              <line x1={x} x2={x} y1={y0 + 2} y2={y0 + row - 2}/><rect className="limit-hit" x={x - 12} y={y0} width={24} height={row}/>
            </g>;
          })}
        </g>;
      })}
    </svg>
  </div>;
}

/** What one key plays, and why each other event is silent there. */
function Probe({ notes, tuning, probe, select }: { notes: LimitNote[]; tuning: Tuning; probe: number; select: (id: string) => void }) {
  const played = keyCents(probe, tuning);
  return <div className="limit-probe" aria-label="Key readout">
    <Text size="sm" fw={600}>{keyName(probe, tuning)} · {formatCents(played)} · {formatHz(played)}</Text>
    <div className="limit-probe-list">{notes.map(n => {
      const kind = status(n, probe, tuning);
      return <button type="button" key={n.id} className={`probe-item is-${kind}`} onClick={() => select(n.id)}>
        <span>{sourceLabel(sourceOf(n))} {rollCents(n.pitch)}</span><span>{kind === 'sounds' ? formatHz(played + n.pitch) : statusWords[kind]}</span>
      </button>;
    })}</div>
  </div>;
}

export function LimitsStudy({ view, overlap, assign }: { view: string; overlap: string; assign: (name: string) => void }) {
  const [notes, setNotes] = useState<LimitNote[]>(pleinJeu);
  const [history, setHistory] = useState<LimitNote[][]>([]);
  const [selected, setSelected] = useState('f1-low');
  const [tuning, setTuning] = useState<Tuning>(12);
  const [probe, setProbe] = useState(60);
  const [roll, setRoll] = useState<RollView>(home);
  const note = notes.find(n => n.id === selected) ?? notes[0];
  const update = (next: LimitNote) => { setHistory(h => [...h, notes]); setNotes(notes.map(n => n.id === next.id ? next : n)); };
  const model = rollModel(notes);
  const canvas = (height: number, source?: string) => <RollCanvas key={source ?? 'all'} anchor="down" model={model} selected={note.id} source={source} height={height}
    view={roll} setView={setRoll} snap={false} steps={tuning} grid={false} pan={false} overlap={overlap === 'lanes' ? 'offset' : overlap as 'offset' | 'stack' | 'merge'}
    select={setSelected} inspect={setSelected} remove={() => {}} update={next => update({ ...note, ...next } as LimitNote)} add={() => {}} editStamp={() => {}} addStamp={() => {}}/>;
  const lanes = [...new Set(notes.map(n => n.source))];
  const shared = { notes, selected: note.id, select: setSelected, update, tuning, probe, setProbe };
  let body: ReactNode;
  if (view === 'chart' || view === 'breaks') body = <CompassChart key={view} {...shared} axis={view === 'chart' ? 'pitch' : 'offset'}/>;
  else if (view === 'strips') body = <KeyStrips {...shared}/>;
  else body = <div className="limit-sentences">{notes.map(n => <section key={n.id} className={`limit-sentence-row ${n.id === note.id ? 'is-selected' : ''}`} onPointerDown={() => setSelected(n.id)}>
    <Text fw={600} size="sm">{sourceLabel(sourceOf(n))} {rollCents(n.pitch)}</Text>
    <LimitSentence note={n} tuning={tuning} update={update} compact/>
  </section>)}</div>;
  return <Stack gap="md" className="piano-study limits-study">
    <Group justify="space-between" align="center">
      <div><Text className="roll-stop-name" fw={600}>Plein jeu <span className="modified">◇</span></Text><Text c="dimmed" size="xs">Grand-orgue · {notes.length} events · {referenceName(tuning)}</Text></div>
      <Group gap="xs">
        <SegmentedControl aria-label="Division tuning" value={String(tuning)} onChange={v => setTuning(Number(v) as Tuning)} data={[{ value: '12', label: '12 equal' }, { value: '19', label: '19 equal' }]}/>
        <Button variant="default" disabled={!history.length} onClick={() => { setNotes(history.at(-1)!); setHistory(history.slice(0, -1)); }}>Undo</Button>
      </Group>
    </Group>
    <div className="limits-top">
      <div className="limits-roll">{overlap === 'lanes'
        ? <div className="roll-lanes">{lanes.map(id => <section key={id} className="roll-source-lane"><Text fw={600} size="sm" mb={4}>{sourceLabel(pipeSources.find(s => s.id === id)!)}</Text>{canvas(150, id)}</section>)}</div>
        : canvas(330)}</div>
      <aside className="roll-inspector limits-inspector"><Stack gap="sm">
        <Text fw={600}>{sourceLabel(sourceOf(note))}</Text>
        <div><Text c="dimmed" size="xs">Pitch offset</Text><NumberControl label="Pitch" value={note.pitch} unit="¢" min={-9600} max={9600} change={pitch => update({ ...note, pitch })} assign={() => assign(`stop/plein-jeu/voice/${note.id}/pitch`)}/></div>
        <LimitSentence note={note} tuning={tuning} update={update}/>
      </Stack></aside>
    </div>
    {body}
    <Probe notes={notes} tuning={tuning} probe={probe} select={setSelected}/>
  </Stack>;
}
