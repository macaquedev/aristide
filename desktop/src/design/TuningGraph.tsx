import { useRef, useState } from 'react';
import { Group, Text } from '@mantine/core';

export const pitchNames = ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'];
export const formatCents = (value: number) => `${value > 0 ? '+' : ''}${Number(value.toFixed(2))}`;

/** A gesture commits once, so Undo restores a complete drag. */
export function TuningGraph({ values, labels, selected, select, editable, equalDivision, change }: {
  values: number[]; labels: string[]; selected: number; select: (index: number) => void;
  editable: boolean; equalDivision: boolean; change: (index: number, value: number) => void;
}) {
  const [draft, setDraft] = useState<{ index: number; value: number }>();
  const drag = useRef<{ index: number; y: number; value: number; height: number; next: number; moved: boolean } | null>(null);
  return <section className="tuning-graph" aria-label={equalDivision ? 'Scale intervals' : 'Pitch deviations'}>
    <Group justify="space-between" className="tuning-graph-heading"><Text fw={600}>{equalDivision ? 'Scale intervals' : 'Pitch deviations'}</Text><Text size="xs" c="dimmed">{equalDivision ? '0–1200 ¢' : '±50 ¢'} · {editable ? 'Drag to tune' : 'Select a note'}</Text></Group>
    <div className="tuning-graph-body">
      <div className="tuning-ruler" aria-hidden="true"><span>{equalDivision ? '1200' : '+50'}</span><span>{equalDivision ? '600' : '0'}</span><span>{equalDivision ? '0' : '−50'}</span></div>
      <div className="tuning-graph-scroll"><div className="tuning-columns" style={{ minWidth: values.length * 44 }}>
        {values.map((value, index) => {
          const cents = draft?.index === index ? draft.value : value;
          const percent = equalDivision ? cents / 1200 * 100 : 50 - cents;
          return <div key={index} className={`tuning-column ${selected === index ? 'selected' : ''}`}>
            <button className="tuning-note-track" style={{ cursor: editable ? 'ns-resize' : 'pointer', touchAction: editable ? 'none' : 'pan-x' }} aria-label={`Select ${labels[index]}`} aria-pressed={selected === index}
              aria-description={`${formatCents(cents)} cents${editable ? '; drag or use up and down arrows to tune' : ''}`}
              onClick={() => select(index)}
              onPointerDown={e => {
                if (e.button !== 0) return;
                select(index);
                if (!editable) return;
                e.preventDefault(); e.currentTarget.setPointerCapture(e.pointerId);
                drag.current = { index, y: e.clientY, value, height: e.currentTarget.clientHeight, next: value, moved: false };
              }}
              onPointerMove={e => {
                const gesture = drag.current;
                if (!gesture || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
                const dy = gesture.y - e.clientY;
                if (Math.abs(dy) < 3 && !gesture.moved) return;
                gesture.moved = true;
                gesture.next = Math.max(-50, Math.min(50, Math.round((gesture.value + dy / gesture.height * 100) * 10) / 10));
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
                if (editable && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
                  e.preventDefault(); change(index, Math.max(-50, Math.min(50, Number((value + (e.key === 'ArrowUp' ? 1 : -1) * (e.shiftKey ? .1 : 1)).toFixed(1)))));
                }
              }}>
              <span className="tuning-track-zero" style={{ top: equalDivision ? '100%' : '50%' }}/>
              <span className="tuning-track-bar" style={equalDivision ? { bottom: 0, height: `${percent}%` } : { top: `${Math.min(50, percent)}%`, height: `${Math.abs(cents)}%` }}/>
              <span className="tuning-track-handle" style={{ top: `${equalDivision ? 100 - percent : percent}%` }}/>
            </button>
            <Text className="tuning-note-label" size="xs">{labels[index]}</Text><Text className="tuning-note-value" size="xs" c="dimmed">{equalDivision ? Number(cents.toFixed(1)) : formatCents(cents)}</Text>
          </div>;
        })}
      </div></div>
    </div>
  </section>;
}
