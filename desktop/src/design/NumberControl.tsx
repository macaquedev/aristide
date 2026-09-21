import { useEffect, useRef, useState } from 'react';
import { Button, Group, NumberInput, Popover, Stack, Text } from '@mantine/core';

/** Shared number gesture prototype: drag, tap/step, type, hold to assign. */
export function NumberControl({ label, value, unit = '', step = 1, min = -Infinity, max = Infinity, disabled = false, change, assign }: {
  label: string; value: number; unit?: string; step?: number; min?: number; max?: number;
  disabled?: boolean; change: (value: number) => void; assign: () => void;
}) {
  const [opened, setOpened] = useState(false);
  const hold = useRef<ReturnType<typeof setTimeout>>(undefined);
  const gesture = useRef({ y: 0, value, moved: false, held: false });
  const clear = () => clearTimeout(hold.current);
  useEffect(() => clear, []);
  const set = (next: number) => { if (Number.isFinite(next)) change(Math.max(min, Math.min(max, Number(next.toFixed(4))))); };
  return <Popover opened={opened} onChange={setOpened} position="bottom" withArrow trapFocus>
    <Popover.Target><Button className="number-control" variant="default" disabled={disabled} aria-label={`${label}: ${value} ${unit}`}
      onPointerDown={e => {
        if (e.button !== 0) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        gesture.current = { y: e.clientY, value, moved: false, held: false };
        hold.current = setTimeout(() => { gesture.current.held = true; assign(); }, 600);
      }}
      onPointerMove={e => {
        if (!e.currentTarget.hasPointerCapture(e.pointerId) || gesture.current.held) return;
        const distance = gesture.current.y - e.clientY;
        if (Math.abs(distance) > 5) { clear(); gesture.current.moved = true; set(gesture.current.value + Math.round(distance / 5) * step); }
      }}
      onPointerUp={e => { clear(); if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId); }}
      onPointerCancel={() => { clear(); gesture.current.moved = true; }}
      onClick={e => { if (e.detail === 0 || (!gesture.current.held && !gesture.current.moved)) setOpened(true); }}
      onContextMenu={e => { e.preventDefault(); assign(); }}>
      <span>{value}{unit && ` ${unit}`}</span>
    </Button></Popover.Target>
    <Popover.Dropdown><Stack gap="xs"><Text>{label}</Text><NumberInput aria-label={label} value={value} step={step} min={Number.isFinite(min) ? min : undefined} max={Number.isFinite(max) ? max : undefined}
      onChange={v => { if (typeof v === 'number') set(v); }} data-autofocus/>
      <Group grow><Button variant="default" aria-label={`Decrease ${label}`} onClick={() => set(value - step)}>−</Button><Button variant="default" aria-label={`Increase ${label}`} onClick={() => set(value + step)}>+</Button></Group>
      <Button onClick={() => setOpened(false)}>Done</Button></Stack></Popover.Dropdown>
  </Popover>;
}
