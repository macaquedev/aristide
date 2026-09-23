import { useEffect, useRef, useState } from 'react';
import { Button, Group, NumberInput, Popover, Stack, Text } from '@mantine/core';

const MIN = -60;
const MAX = 12;
const clamp = (db: number) => Math.max(MIN, Math.min(MAX, Math.round(db * 10) / 10));
const format = (db: number) => (db > 0 ? `+${db}` : `${db}`).replace('-', '−');

/** One matrix cell: tap to connect, then the shared number gesture — drag, tap for a stepper, type, hold to assign. */
export function RouteCell({ label, level, inherited, disabled, connect, setLevel, disconnect, assign }: {
  label: string; level?: number; inherited: boolean; disabled: boolean;
  connect: () => void; setLevel: (db: number) => void; disconnect: () => void; assign: () => void;
}) {
  const [opened, setOpened] = useState(false);
  const [draft, setDraft] = useState<number>();
  const hold = useRef<ReturnType<typeof setTimeout>>(undefined);
  const throttle = useRef<ReturnType<typeof setTimeout>>(undefined);
  const latest = useRef<number>(undefined);
  const gesture = useRef({ y: 0, level: 0, moved: false, held: false });
  const clear = () => clearTimeout(hold.current);
  useEffect(() => () => { clear(); clearTimeout(throttle.current); }, []);
  const connected = level !== undefined;
  const shown = draft ?? level;

  // A drag is heard as it moves, a request at most every 100 ms, and its last value always lands.
  const flush = () => {
    clearTimeout(throttle.current);
    throttle.current = undefined;
    if (latest.current !== undefined) setLevel(latest.current);
    latest.current = undefined;
  };
  const drag = (db: number) => {
    setDraft(db);
    latest.current = db;
    throttle.current ??= setTimeout(flush, 100);
  };
  const endDrag = () => { flush(); setDraft(undefined); };

  return <Popover opened={opened} onChange={setOpened} position="bottom" withArrow trapFocus>
    <Popover.Target>
      <button type="button" className="route-cell" data-connected={connected || undefined} data-inherited={(connected && inherited) || undefined}
        disabled={disabled} aria-label={connected ? `${label}: ${shown} dB` : `Connect ${label}`} aria-pressed={connected}
        onPointerDown={e => {
          if (e.button !== 0) return;
          gesture.current = { y: e.clientY, level: level ?? 0, moved: false, held: false };
          if (!connected) return;
          e.currentTarget.setPointerCapture(e.pointerId);
          hold.current = setTimeout(() => { gesture.current.held = true; assign(); }, 600);
        }}
        onPointerMove={e => {
          if (!e.currentTarget.hasPointerCapture(e.pointerId) || gesture.current.held) return;
          const distance = gesture.current.y - e.clientY;
          if (Math.abs(distance) > 5) { clear(); gesture.current.moved = true; drag(clamp(gesture.current.level + Math.round(distance / 4))); }
        }}
        onPointerUp={e => {
          clear();
          if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
          if (gesture.current.moved) endDrag();
        }}
        onPointerCancel={() => { clear(); if (gesture.current.moved) endDrag(); gesture.current.moved = true; }}
        onClick={e => {
          if (gesture.current.held || (gesture.current.moved && e.detail !== 0)) return;
          if (connected) setOpened(true); else connect();
        }}
        onContextMenu={e => { e.preventDefault(); if (connected && !disabled) assign(); }}>
        {connected ? format(shown!) : ''}
      </button>
    </Popover.Target>
    <Popover.Dropdown><Stack gap="xs">
      <Text>{label}</Text>
      <NumberInput aria-label={`${label} level`} value={level ?? 0} step={0.5} min={MIN} max={MAX} suffix=" dB" data-autofocus
        onChange={v => { if (typeof v === 'number') setLevel(clamp(v)); }}/>
      <Group grow>
        <Button variant="default" aria-label={`Lower ${label}`} onClick={() => setLevel(clamp((level ?? 0) - 1))}>−</Button>
        <Button variant="default" aria-label={`Raise ${label}`} onClick={() => setLevel(clamp((level ?? 0) + 1))}>+</Button>
      </Group>
      <Group grow>
        <Button variant="default" onClick={() => { disconnect(); setOpened(false); }}>Disconnect</Button>
        <Button onClick={() => setOpened(false)}>Done</Button>
      </Group>
    </Stack></Popover.Dropdown>
  </Popover>;
}
