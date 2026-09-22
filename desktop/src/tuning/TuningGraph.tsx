import { useRef, useState } from 'react';
import { Group, Text } from '@mantine/core';
import { formatCents } from './model';

/** A gesture commits once, so Undo restores a complete drag. */
export function TuningGraph({ values, labels, selected, select, editable, intervals = false, lower = -50, upper = 50, fixed, caption, change }: {
  values: number[]; labels: string[]; selected: number; select: (index: number) => void;
  editable: boolean; intervals?: boolean; lower?: number; upper?: number; fixed?: number; caption?: string; change: (index: number, value: number) => void;
}) {
  const [draft, setDraft] = useState<{ index: number; value: number }>();
  const drag = useRef<{ index: number; y: number; value: number; height: number; next: number; moved: boolean } | null>(null);
  return <section className={`tuning-graph ${intervals ? 'tuning-intervals' : ''}`} aria-label={intervals ? 'Scale intervals' : 'Pitch deviations'}>
    <Group justify="space-between" className="tuning-graph-heading"><Text fw={600}>{intervals ? 'Scale intervals' : 'Pitch deviations'}</Text><Text size="xs" c="dimmed">{caption ?? (intervals ? `${Number(lower.toFixed(1))}–${Number(upper.toFixed(1))} ¢` : '±50 ¢')} · {editable ? 'Drag to tune' : 'Select a note'}</Text></Group>
    <div className="tuning-graph-body">
      <div className="tuning-ruler" aria-hidden="true"><span>{Number(upper.toFixed(1))}</span><span>{Number(((lower + upper) / 2).toFixed(1))}</span><span>{lower}</span></div>
      <div className="tuning-graph-scroll"><div className="tuning-columns" style={{ minWidth: values.length * 44 }}>
        {values.map((value, index) => {
          const cents = draft?.index === index ? draft.value : value;
          const percent = (upper - cents) / (upper - lower) * 100;
          const zero = upper / (upper - lower) * 100;
          const canEdit = editable && index !== fixed;
          return <div key={index} className={`tuning-column ${selected === index ? 'selected' : ''}`}>
            <button className="tuning-note-track" style={{ cursor: canEdit ? 'ns-resize' : 'pointer', touchAction: canEdit ? 'none' : 'pan-x' }} aria-label={`Select ${labels[index]}`} aria-pressed={selected === index}
              aria-description={`${formatCents(cents)} cents${canEdit ? '; drag or use up and down arrows to tune' : ''}`}
              onClick={() => select(index)}
              onPointerDown={e => {
                if (e.button !== 0) return;
                select(index);
                if (!canEdit) return;
                e.preventDefault(); e.currentTarget.setPointerCapture(e.pointerId);
                drag.current = { index, y: e.clientY, value, height: e.currentTarget.clientHeight, next: value, moved: false };
              }}
              onPointerMove={e => {
                const gesture = drag.current;
                if (!gesture || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
                const dy = gesture.y - e.clientY;
                if (Math.abs(dy) < 3 && !gesture.moved) return;
                gesture.moved = true;
                gesture.next = Math.max(lower, Math.min(upper, Math.round((gesture.value + dy / gesture.height * (upper - lower)) * 10) / 10));
                setDraft({ index, value: gesture.next });
              }}
              onPointerUp={e => {
                const gesture = drag.current;
                if (gesture?.moved && gesture.next !== gesture.value) change(index, gesture.next);
                drag.current = null; setDraft(undefined);
                if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
              }}
              onPointerCancel={() => { drag.current = null; setDraft(undefined); }}
              onLostPointerCapture={() => { drag.current = null; setDraft(undefined); }}
              onKeyDown={e => {
                if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
                  e.preventDefault(); const next = (index + (e.key === 'ArrowRight' ? 1 : -1) + values.length) % values.length;
                  select(next);
                  (e.currentTarget.closest('.tuning-columns')?.querySelectorAll('button')[next] as HTMLButtonElement)?.focus();
                }
                if (canEdit && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
                  e.preventDefault(); change(index, Math.max(lower, Math.min(upper, Number((value + (e.key === 'ArrowUp' ? 1 : -1) * (e.shiftKey ? .1 : 1)).toFixed(1)))));
                }
              }}>
              <span className="tuning-track-zero" style={{ top: `${zero}%` }}/>
              <span className="tuning-track-bar" style={{ top: `${Math.min(zero, percent)}%`, height: `${Math.abs(zero - percent)}%` }}/>
              <span className="tuning-track-handle" style={{ top: `${percent}%` }}/>
            </button>
            <Text className="tuning-note-label" size="xs">{labels[index]}</Text><Text className="tuning-note-value" size="xs" c="dimmed">{intervals ? Number(cents.toFixed(1)) : formatCents(cents)}</Text>
          </div>;
        })}
      </div></div>
    </div>
  </section>;
}
