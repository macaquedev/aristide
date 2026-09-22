import { useEffect, useId, useRef, useState } from 'react';
import { Button, Group, Text } from '@mantine/core';
import { anchorName, cents, nearestStamp, snapPitch, validNote } from './rollModel';
import type { Anchor, RollModel, RollNote, Stamp } from './rollModel';

export type RollView = { time: number; center: number; span: number; pitchSpan: number };
type Props = {
  anchor: Anchor; model: RollModel; selected: string; source?: string; height: number;
  view: RollView; setView: (view: RollView) => void; snap: boolean; steps: number; grid: boolean; pan: boolean;
  select: (id: string) => void; inspect: (id: string) => void; remove: (id: string) => void; update: (note: RollNote) => void;
  add: (anchor: Anchor, stamp: string, pitch: number, source?: string) => void;
  editStamp: (stamp: Stamp) => void; addStamp: (anchor: Anchor) => void;
};

/** Time is always constrained by marker identity; pitch guides are independent of snapping. */
export function RollCanvas(p: Props) {
  const host = useRef<HTMLDivElement>(null);
  const svg = useRef<SVGSVGElement>(null);
  const [width, setWidth] = useState(500);
  const [draft, setDraft] = useState<RollNote>();
  const clip = useId().replace(/:/g, '');
  const gesture = useRef<{ x: number; y: number; view: RollView; note?: RollNote; resize?: boolean; moved: boolean; pan: boolean } | null>(null);
  const latest = useRef(p); latest.current = p;
  const left = 70, top = 38, bottom = p.height - 12;
  const plotWidth = Math.max(1, width - left - 14), plotHeight = bottom - top;
  const scaleX = plotWidth / p.view.span, scaleY = plotHeight / p.view.pitchSpan;
  const x = (ms: number) => left + (ms - p.view.time) * scaleX;
  const y = (pitch: number) => top + plotHeight / 2 - (pitch - p.view.center) * scaleY;
  const pitchAt = (py: number) => snapPitch(p.view.center + (top + plotHeight / 2 - py) / scaleY, p.steps, p.snap);
  const stampAt = (px: number) => nearestStamp(p.model.stamps, p.anchor, p.view.time + (px - left) / scaleX);
  const point = (event: { clientX: number; clientY: number }) => {
    const rect = svg.current!.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  };
  useEffect(() => {
    const observer = new ResizeObserver(entries => setWidth(entries[0].contentRect.width));
    observer.observe(host.current!);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    const node = svg.current!;
    const wheel = (e: WheelEvent) => {
      e.preventDefault();
      const current = latest.current;
      const v = current.view;
      if (e.ctrlKey || e.metaKey || e.altKey) {
        const factor = Math.exp(Math.max(-100, Math.min(100, e.deltaY)) / 300);
        const rect = node.getBoundingClientRect();
        const ratio = Math.max(0, Math.min(1, (e.clientX - rect.left - left) / Math.max(1, rect.width - left - 14)));
        if (e.altKey) {
          const pitchSpan = Math.max(8, Math.min(19200, v.pitchSpan * factor));
          const ry = .5 - (e.clientY - rect.top - top) / (current.height - 12 - top);
          current.setView({ ...v, pitchSpan, center: v.center + ry * (v.pitchSpan - pitchSpan) });
        } else {
          const span = Math.max(10, Math.min(10000, v.span * factor));
          current.setView({ ...v, span, time: Math.max(0, v.time + ratio * (v.span - span)) });
        }
      } else current.setView({ ...v,
        time: Math.max(0, v.time + (e.shiftKey ? e.deltaY : e.deltaX) * v.span / 600),
        center: v.center - (e.shiftKey ? 0 : e.deltaY) * v.pitchSpan / 600,
      });
    };
    node.addEventListener('wheel', wheel, { passive: false });
    return () => node.removeEventListener('wheel', wheel);
  }, []);

  const notes = p.model.notes.map(n => draft?.id === n.id ? draft : n).filter(n => !p.source || n.source === p.source);
  // Close microtonal pitches can overlap at overview zoom. Keep the true pitch dot,
  // offset the label/bar only, and connect it with a leader rather than quantizing.
  const bars: { note: RollNote; start: Stamp; end?: Stamp; from: number; to: number; py: number; by: number; continuation: boolean; arrow: boolean }[] = [];
  for (const note of [...notes].sort((a, b) => b.pitch - a.pitch)) {
    const start = p.model.stamps.find(s => s.id === note.start)!;
    const end = p.model.stamps.find(s => s.id === note.end);
    const continuation = start.anchor === 'down' && end?.anchor === 'up' && p.anchor === 'up';
    if (start.anchor !== p.anchor && !continuation) continue;
    const arrow = p.anchor === 'down' && (!end || end.anchor === 'up');
    const from = continuation ? left : x(start.ms);
    const to = arrow ? width - 14 : x(end!.ms);
    if (to < left || from > width - 14) continue;
    const py = y(note.pitch);
    let by = py - 12;
    for (const other of bars) {
      if (from < other.to && to > other.from && Math.abs(by - other.by) < 28) by = other.by + 30;
    }
    bars.push({ note, start, end, from, to, py, by, continuation, arrow });
  }
  const firstStep = Math.ceil((p.view.center - p.view.pitchSpan / 2) / (1200 / p.steps));
  const count = Math.min(1200, Math.ceil(p.view.pitchSpan / (1200 / p.steps)) + 1);
  const guides = Array.from({ length: count }, (_, i) => (firstStep + i) * 1200 / p.steps);
  const sparse = Math.max(1, Math.ceil(24 / ((1200 / p.steps) * scaleY)));

  return <section className="roll-panel" aria-label={`${anchorName(p.anchor)}${p.source ? ` ${p.model.sources.find(s => s.id === p.source)?.name}` : ''} timeline`}>
    <Group className="roll-panel-heading" justify="space-between" gap="xs">
      <Group gap="xs"><Text fw={600}>{anchorName(p.anchor)}</Text><Text size="xs" c="dimmed">+ ms</Text></Group>
      <Button variant="subtle" size="compact-sm" onClick={() => p.addStamp(p.anchor)}>+ Timestamp</Button>
    </Group>
    <div ref={host} className="roll-surface">
      <svg ref={svg} width="100%" height={p.height} className={p.pan ? 'roll-canvas panning' : 'roll-canvas'}
        aria-label={`${anchorName(p.anchor)} piano roll`} role="group"
        onContextMenu={e => {
          e.preventDefault();
          const id = (e.target as Element).closest('[data-note]')?.getAttribute('data-note');
          if (id) { gesture.current = null; setDraft(undefined); p.remove(id); }
        }}
        onPointerDown={e => {
          if (e.button !== 0 && e.button !== 1) return;
          const pos = point(e);
          const target = (e.target as Element).closest('[data-note]');
          const note = notes.find(n => n.id === target?.getAttribute('data-note'));
          if (pos.x < left || pos.y < top) return;
          e.preventDefault(); e.currentTarget.setPointerCapture(e.pointerId);
          const pan = p.pan || e.button === 1;
          if (note && !pan) p.select(note.id);
          gesture.current = { ...pos, view: p.view, note, resize: Boolean((e.target as Element).closest('[data-resize]')), moved: false, pan };
        }}
        onPointerMove={e => {
          const g = gesture.current;
          if (!g || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
          const pos = point(e), dx = pos.x - g.x, dy = pos.y - g.y;
          if (Math.abs(dx) + Math.abs(dy) > 4) g.moved = true;
          if (!g.moved) return;
          if (g.pan) p.setView({ ...g.view, time: Math.max(0, g.view.time - dx / scaleX), center: g.view.center + dy / scaleY });
          else if (g.note) {
            const next = { ...g.note };
            const start = p.model.stamps.find(s => s.id === next.start)!;
            if (g.resize) next.end = stampAt(pos.x).id;
            else {
              next.pitch = snapPitch(g.note.pitch - dy / scaleY, p.steps, p.snap);
              if (start.anchor === p.anchor && Math.abs(dx) > 4) next.start = stampAt(x(start.ms) + dx).id;
            }
            if (validNote(next, p.model.stamps)) setDraft(next);
          }
        }}
        onPointerUp={e => {
          const g = gesture.current;
          if (!g) return;
          if (draft && g.moved) p.update(draft);
          else if (g.note && !g.pan && !g.moved) p.inspect(g.note.id);
          else if (!g.note && !g.pan && !g.moved) { const pos = point(e); p.add(p.anchor, stampAt(pos.x).id, pitchAt(pos.y), p.source); }
          gesture.current = null; setDraft(undefined);
          if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
        }}
        onPointerCancel={() => { gesture.current = null; setDraft(undefined); }}
        onLostPointerCapture={() => { gesture.current = null; setDraft(undefined); }}>
        <defs><clipPath id={clip}><rect x={left} y={top} width={plotWidth} height={plotHeight}/></clipPath></defs>
        <rect className="roll-background" width={width} height={p.height}/>
        <g clipPath={`url(#${clip})`}>
          {guides.map((pitch, i) => <line key={i} className={Math.abs(pitch) < .01 ? 'roll-zero' : 'roll-guide'} x1={left} x2={width} y1={y(pitch)} y2={y(pitch)}/>)}
          {p.grid && Array.from({ length: Math.min(300, Math.ceil(p.view.span / 10) + 1) }, (_, i) => Math.ceil(p.view.time / 10) * 10 + i * 10).map(ms => <line key={ms} className="roll-time-grid" x1={x(ms)} x2={x(ms)} y1={top} y2={bottom}/>)}
          {p.model.stamps.filter(s => s.anchor === p.anchor).map(stamp => <line key={stamp.id} className="roll-stamp-line" x1={x(stamp.ms)} x2={x(stamp.ms)} y1={top} y2={bottom}/>)}
        </g>
        <rect className="roll-axis" x={0} y={0} width={left} height={p.height}/>
        <text className="roll-unit" x={left - 12} y={24} textAnchor="end">cents</text>
        {guides.filter((pitch, i) => (firstStep + i) % sparse === 0 || Math.abs(pitch) < .01).map(pitch => <text key={pitch} className={Math.abs(pitch) < .01 ? 'roll-zero-label' : 'roll-axis-label'} x={left - 12} y={y(pitch) + 4} textAnchor="end">{Number(pitch.toFixed(1))}</text>)}
        <g clipPath={`url(#${clip})`}>
          {bars.map(bar => {
            const { note, from, to, by, py, arrow, continuation } = bar;
            const source = p.model.sources.find(s => s.id === note.source)!;
            const sx = Math.max(left, from), ex = Math.min(width - 14, to);
            const label = `${source.name} · ${cents(note.pitch)}`;
            const selected = p.selected === note.id;
            return <g key={note.id} data-note={note.id} className={`event-note ${selected ? 'is-selected' : ''}`}
              role="button" tabIndex={0} aria-label={`${source.name}, ${cents(note.pitch)}, ${continuation ? 'held after release' : `${bar.start.ms} ms`}, ${arrow ? note.end ? 'hold after release' : 'until release' : `ends ${bar.end!.ms} ms`}`}
              onKeyDown={e => {
                if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); p.inspect(note.id); }
                if (['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].includes(e.key)) {
                  e.preventDefault(); p.select(note.id);
                  const next = { ...note };
                  if (e.key === 'ArrowUp' || e.key === 'ArrowDown') next.pitch = snapPitch(note.pitch + (e.key === 'ArrowUp' ? 1 : -1) * (p.snap ? 1200 / p.steps : e.shiftKey ? 10 : 1), p.steps, p.snap);
                  else if (!continuation) {
                    const stamps = p.model.stamps.filter(s => s.anchor === p.anchor).sort((a, b) => a.ms - b.ms);
                    const index = stamps.findIndex(s => s.id === note.start) + (e.key === 'ArrowRight' ? 1 : -1);
                    if (stamps[index]) next.start = stamps[index].id;
                  }
                  if (validNote(next, p.model.stamps)) p.update(next);
                }
              }}>
              <title>{label} · {source.kind}{continuation ? ' · continuation from key down' : ''}</title>
              <line className="note-leader" x1={sx + 2} x2={sx + 2} y1={py} y2={by + 12}/>
              <circle className="note-pitch-dot" cx={sx + 2} cy={py} r={3}/>
              <rect className="note-body" x={sx} y={by} width={Math.max(1, ex - sx)} height={24} rx={3}/>
              <svg x={sx + (continuation ? 22 : 8)} y={by} width={Math.max(1, ex - sx - (continuation ? 44 : 30))} height={24} overflow="hidden">
                <text className="note-label" x={0} y={16}>{label}</text>
              </svg>
              {continuation && <path className="note-arrow" data-testid="continuation-arrow" d={`M${sx + 16} ${by + 6} l-7 6 7 6 M${sx + 9} ${by + 12} h12`}/>}
              {arrow ? <path className="note-arrow" data-testid="hold-arrow" d={`M${ex - 17} ${by + 6} l7 6 -7 6 M${ex - 23} ${by + 12} h12`}/> : to <= width - 14 && <line className="note-end" x1={ex - 1} x2={ex - 1} y1={by + 3} y2={by + 21}/>}
              {(arrow || to <= width - 14) && <rect data-resize="true" className="note-resize" x={ex - 12} y={by - 5} width={16} height={34}><title>Drag ending to a timestamp</title></rect>}
            </g>;
          })}
        </g>
        <rect className="roll-ruler" x={left} y={0} width={plotWidth + 14} height={top}/>
        {p.model.stamps.filter(s => s.anchor === p.anchor && x(s.ms) >= left && x(s.ms) < width - 14).map(stamp => <g key={stamp.id} className="stamp-marker" role="button" tabIndex={0} aria-label={`Edit ${anchorName(p.anchor)} timestamp ${stamp.ms} ms`}
          onClick={() => p.editStamp(stamp)} onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); p.editStamp(stamp); } }}>
          <rect x={x(stamp.ms)} y={0} width={Math.min(66, width - x(stamp.ms))} height={36}/>
          <text x={x(stamp.ms) + 5} y={23}>{stamp.ms}</text><path d={`M${x(stamp.ms)} 32 l5 6 -5 0 Z`}/>
        </g>)}
      </svg>
      <input className="roll-scroll vertical" type="range" aria-label={`${anchorName(p.anchor)} pitch scroll`} min={Math.min(-9600, p.view.center - p.view.pitchSpan)} max={Math.max(9600, p.view.center + p.view.pitchSpan)} step="1" value={p.view.center} onChange={e => p.setView({ ...p.view, center: Number(e.target.value) })}/>
    </div>
    <div className="roll-scroll-row"><Text size="xs" c="dimmed">{Math.round(p.view.time)} ms</Text><input className="roll-scroll" type="range" aria-label={`${anchorName(p.anchor)} time scroll`} min="0" max={Math.max(1000, p.view.time + p.view.span, ...p.model.stamps.map(s => s.ms))} step="1" value={p.view.time} onChange={e => p.setView({ ...p.view, time: Number(e.target.value) })}/><Text size="xs" c="dimmed">{Math.round(p.view.time + p.view.span)}</Text></div>
  </section>;
}
