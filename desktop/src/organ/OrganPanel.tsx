import { useEffect, useRef, useState, type KeyboardEvent, type MouseEvent, type PointerEvent } from 'react';
import { Button, Drawer, Group, Loader, Menu, Modal, SegmentedControl, Select, Stack, Switch, Text, TextInput } from '@mantine/core';
import { Folder, GripVertical, Pencil, Plus, Trash2 } from 'lucide-react';
import { canPickFiles, editInstrument, organFiles, pickFile, request, type Snapshot, type Stop } from '../api';
import { StopEditor } from './StopEditor';
import './organ.css';

type Selection = { kind: 'stop'; id: number } | { kind: 'division'; idx: number } | { kind: 'coupler'; idx: number };
/** What to select again once a structural edit has rebuilt the organ: ids change, names don't. */
type Reselect = { kind: 'stop'; name: string; manual: string } | { kind: 'division'; name: string } | { kind: 'coupler'; name: string };
type Offerings = { sources: { alias: string; path: string; name?: string; error?: string; manuals?: { name: string; pedal: boolean; pulled: boolean; stops: { name: string; pulled: boolean }[] }[] }[] };
type Params = Record<string, string | number>;
/** Where a dragged stop would land: before the stop at `index` in division `midx`. */
type Drop = { midx: number; index: number };

/** The drop position under a point in the organ tree, by row midpoints. */
function dropAt(x: number, y: number): Drop | undefined {
  const section = document.elementFromPoint(x, y)?.closest<HTMLElement>('[data-midx]');
  if (!section) return undefined;
  const rows = [...section.querySelectorAll<HTMLElement>('[data-stop]')];
  const index = rows.filter(row => { const box = row.getBoundingClientRect(); return box.top + box.height / 2 < y; }).length;
  return { midx: Number(section.dataset.midx), index };
}

const pitches = [{ value: '0', label: 'Unison' }, { value: '12', label: 'Octave up' }, { value: '-12', label: 'Octave down' }];
const pitchName = (shift: number) => pitches.find(p => Number(p.value) === shift)?.label ?? `${shift > 0 ? '+' : ''}${shift} keys`;

/** Organ: this organ's divisions, stops and couplers down the left, the selected one's editor on the right.
 * Renames land live; adding, moving and removing rebuild the organ from its file. */
export function OrganPanel({ organ, state, stopId, select, offerUndo, openScope }: {
  organ: string; state: Snapshot; stopId?: number; select: (id: number) => void;
  offerUndo: (undo?: () => void) => void; openScope: (id: string) => void;
}) {
  const [selection, setSelection] = useState<Selection | undefined>(stopId === undefined ? undefined : { kind: 'stop', id: stopId });
  const [failure, setFailure] = useState<string>();
  const [confirm, setConfirm] = useState<{ title: string; message: string; run: () => void }>();
  const [menu, setMenu] = useState<{ x: number; y: number; target: Selection }>();
  const [renaming, setRenaming] = useState<Selection>();
  const [drag, setDrag] = useState<{ id: number; to?: Drop }>();
  const dragged = useRef(false);
  const [adding, setAdding] = useState<{ kind: 'stop'; manual: string } | { kind: 'division' } | { kind: 'coupler' }>();
  const reselect = useRef<Reselect>(undefined);
  const tree = useRef<HTMLElement>(null);
  const busy = Boolean(state.loading);
  const { stops, manuals, couplers } = state;

  useEffect(() => { if (stopId !== undefined) setSelection({ kind: 'stop', id: stopId }); }, [stopId]);
  // After a rebuild, find what was being edited by name.
  useEffect(() => {
    const wanted = reselect.current;
    if (busy || !wanted) return;
    reselect.current = undefined;
    if (wanted.kind === 'stop') {
      const found = stops.find(s => s.name === wanted.name && s.manual === wanted.manual) ?? stops.find(s => s.name === wanted.name);
      if (found) { setSelection({ kind: 'stop', id: found.id }); select(found.id); }
    } else if (wanted.kind === 'division') {
      const found = manuals.find(m => m.name === wanted.name);
      setSelection(found ? { kind: 'division', idx: found.idx } : undefined);
    } else {
      const found = couplers.find(c => c.name === wanted.name);
      setSelection(found ? { kind: 'coupler', idx: found.idx } : undefined);
    }
  }, [busy, stops, manuals, couplers, select]);

  const stillThere = (s: Selection) => s.kind === 'stop' ? stops.some(x => x.id === s.id)
    : s.kind === 'division' ? manuals.some(m => m.idx === s.idx) : couplers.some(c => c.idx === s.idx);
  const current: Selection | undefined = selection && stillThere(selection) ? selection : stops[0] ? { kind: 'stop', id: stops[0].id } : undefined;
  const choose = (next: Selection) => { setSelection(next); if (next.kind === 'stop') select(next.id); };
  const change = async (path: string, params: Params, message: string, then?: Reselect) => {
    try {
      await editInstrument(organ, path, params);
      if (then) reselect.current = then;
      return true;
    } catch { setFailure(message); return false; }
  };
  const ask = (title: string, message: string, run: () => void) => setConfirm({ title, message, run });
  // A division holds one stop of each name: the organ file finds stops by division and name.
  const named = (manual: string) => stops.filter(s => s.manual === manual).map(s => s.name.toLowerCase());
  const nameOf = (s: Selection) => s.kind === 'stop' ? stops.find(x => x.id === s.id)?.name
    : s.kind === 'division' ? manuals.find(m => m.idx === s.idx)?.name : couplers.find(c => c.idx === s.idx)?.name;
  const rename = (s: Selection, name: string) => {
    if (s.kind === 'stop') {
      const manual = stops.find(x => x.id === s.id)?.manual ?? '';
      return change('organ/stop/rename', { stop: s.id, name }, 'The stop could not be renamed. Use a name no other stop has.', { kind: 'stop', name, manual });
    }
    if (s.kind === 'division') return change('organ/manual/rename', { manual: s.idx, name }, 'The division could not be renamed. Use a name no other division has.', { kind: 'division', name });
    return change('organ/coupler/rename', { idx: s.idx, name }, 'The coupler could not be renamed. Use a name no other coupler has.', { kind: 'coupler', name });
  };
  const remove = (s: Selection) => {
    const name = nameOf(s);
    if (name === undefined) return;
    if (s.kind === 'stop') ask(`Delete ${name}?`, 'Its samples stay in the sample set, so it can be added again.',
      () => void change('organ/unpull', { stop: s.id }, 'This stop could not be deleted.'));
    else if (s.kind === 'division') ask(`Delete ${name}?`, 'Its stops leave the organ with it. Their samples stay in the sample set.',
      () => void change('organ/manual/remove', { manual: s.idx }, 'This division could not be deleted. Divisions that came with the sample set stay; delete their stops instead.'));
    else ask(`Delete ${name}?`, 'A coupler that came with the sample set is taken off the console instead, and can be shown again.',
      () => void change('organ/coupler/remove', { idx: s.idx }, 'This coupler could not be deleted.'));
  };
  const openMenu = (target: Selection) => (e: MouseEvent) => { e.preventDefault(); choose(target); setMenu({ x: e.clientX, y: e.clientY, target }); };
  const drop = (s: Stop, at: Drop) => {
    const target = manuals.find(m => m.idx === at.midx);
    if (!target) return;
    const ids = stops.filter(x => x.midx === at.midx).map(x => x.id);
    const from = ids.indexOf(s.id);
    const next = ids.filter(id => id !== s.id);
    next.splice(from >= 0 && from < at.index ? at.index - 1 : at.index, 0, s.id);
    const order = () => change('organ/stop/order', { manual: at.midx, stops: next.join(',') }, `${s.name} could not be placed there.`);
    if (at.midx === s.midx) {
      if (next.some((id, i) => id !== ids[i])) void order();
    } else if (named(target.name).includes(s.name.toLowerCase())) {
      setFailure(`${target.name} already has a stop named ${s.name}. Rename one of them first.`);
    } else {
      void change('organ/move', { stop: s.id, manual: at.midx }, `${s.name} could not be moved to ${target.name}.`).then(ok => { if (ok) void order(); });
    }
  };
  // Mouse drags from anywhere on the row; touch from the grip, so the list still scrolls.
  const startDrag = (s: Stop) => (e: PointerEvent<HTMLButtonElement>) => {
    dragged.current = false;
    if (busy || e.button !== 0) return;
    if (e.pointerType !== 'mouse' && !(e.target as HTMLElement).closest('.organ-grip')) return;
    const [x0, y0] = [e.clientX, e.clientY];
    let to: Drop | undefined;
    const move = (ev: globalThis.PointerEvent) => {
      if (!dragged.current && Math.hypot(ev.clientX - x0, ev.clientY - y0) < 6) return;
      dragged.current = true;
      const box = tree.current?.getBoundingClientRect();
      if (box && ev.clientY < box.top + 40) tree.current?.scrollBy(0, -12);
      if (box && ev.clientY > box.bottom - 40) tree.current?.scrollBy(0, 12);
      to = dropAt(ev.clientX, ev.clientY);
      setDrag({ id: s.id, to });
    };
    const finish = (commit: boolean) => () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', cancel);
      window.removeEventListener('keydown', escape);
      setDrag(undefined);
      if (commit && dragged.current && to) drop(s, to);
    };
    const up = finish(true);
    const cancel = finish(false);
    const escape = (ev: globalThis.KeyboardEvent) => { if (ev.key === 'Escape') cancel(); };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', cancel);
    window.addEventListener('keydown', escape);
  };
  const dropMark = (midx: number, index: number, count: number) => drag?.to?.midx !== midx ? undefined
    : drag.to.index === index ? 'before' : index === count - 1 && drag.to.index === count ? 'after' : undefined;

  const onTreeKey = (e: KeyboardEvent) => {
    if ((e.key === 'Delete' || e.key === 'Backspace') && current && !busy) { e.preventDefault(); remove(current); }
    if (e.key === 'F2' && current && !busy) { e.preventDefault(); setRenaming(current); }
  };

  const stop = current?.kind === 'stop' ? stops.find(s => s.id === current.id) : undefined;
  const shown = current && (current.kind === 'stop' ? `s${current.id}` : current.kind === 'division' ? `d${current.idx}` : `c${current.idx}`);
  useEffect(() => { tree.current?.querySelector('[data-selected="true"]')?.scrollIntoView({ block: 'nearest' }); }, [shown]);
  const division = current?.kind === 'division' ? manuals.find(m => m.idx === current.idx) : undefined;
  const coupler = current?.kind === 'coupler' ? couplers.find(c => c.idx === current.idx) : undefined;

  const stopActions = stop && <>
    <Rename label="Stop name" value={stop.name} disabled={busy} save={name => rename({ kind: 'stop', id: stop.id }, name)}/>
    <Select aria-label="Division" w={170} value={String(stop.midx)} allowDeselect={false} disabled={busy}
      data={manuals.map(m => ({ value: String(m.idx), label: m.name, disabled: m.idx !== stop.midx && named(m.name).includes(stop.name.toLowerCase()) }))}
      onChange={value => { const to = manuals.find(m => String(m.idx) === value); if (to && to.idx !== stop.midx) void change('organ/move', { stop: stop.id, manual: to.idx }, 'The stop could not be moved.', { kind: 'stop', name: stop.name, manual: to.name }); }}/>
    <Button variant="subtle" color="red" disabled={busy} onClick={() => remove({ kind: 'stop', id: stop.id })}>Delete stop</Button>
  </>;

  return <div className="organ-panel">
    <nav ref={tree} className="organ-tree" aria-label="Organ" onKeyDown={onTreeKey} data-dragging={drag ? true : undefined}>
      {manuals.map(manual => { const own = stops.filter(s => s.midx === manual.idx); return <section key={manual.idx} aria-label={manual.name} data-midx={manual.idx}>
        <Button className="organ-division" fullWidth justify="space-between" variant={division?.idx === manual.idx ? 'light' : 'subtle'} color={division?.idx === manual.idx ? undefined : 'gray'}
          data-selected={division?.idx === manual.idx} data-drop={!own.length && drag?.to?.midx === manual.idx ? 'after' : undefined}
          onClick={() => choose({ kind: 'division', idx: manual.idx })} onContextMenu={openMenu({ kind: 'division', idx: manual.idx })}>{manual.name}</Button>
        <nav aria-label={`${manual.name} stops`}>{own.map((s, i) => <Button key={s.id} fullWidth justify="space-between" variant={stop?.id === s.id ? 'light' : 'subtle'} color={stop?.id === s.id ? undefined : 'gray'}
          aria-current={stop?.id === s.id ? 'true' : undefined} data-selected={stop?.id === s.id} data-stop={s.id} data-drop={dropMark(manual.idx, i, own.length)} data-lifted={drag?.id === s.id ? true : undefined}
          rightSection={<GripVertical className="organ-grip" size={16} aria-hidden/>} onPointerDown={startDrag(s)}
          onClick={() => { if (dragged.current) { dragged.current = false; return; } choose({ kind: 'stop', id: s.id }); }} onContextMenu={openMenu({ kind: 'stop', id: s.id })}>
          {s.name}{s.custom && <span className="modified" aria-label="custom">◇</span>}</Button>)}</nav>
        <Button className="organ-add" variant="subtle" color="gray" size="compact-sm" leftSection={<Plus size={14}/>} disabled={busy} onClick={() => setAdding({ kind: 'stop', manual: manual.name })}>Add stop</Button>
      </section>; })}
      <section aria-label="Couplers">
        <Text size="xs" c="dimmed" className="organ-heading">Couplers</Text>
        {couplers.map(c => <Button key={c.idx} fullWidth justify="space-between" variant={coupler?.idx === c.idx ? 'light' : 'subtle'} color={coupler?.idx === c.idx ? undefined : 'gray'}
          data-selected={coupler?.idx === c.idx} onClick={() => choose({ kind: 'coupler', idx: c.idx })} onContextMenu={openMenu({ kind: 'coupler', idx: c.idx })} rightSection={c.hidden ? <Text component="span" size="xs" c="dimmed">Hidden</Text> : undefined}>{c.name}</Button>)}
        <Button className="organ-add" variant="subtle" color="gray" size="compact-sm" leftSection={<Plus size={14}/>} disabled={busy || manuals.length < 2} onClick={() => setAdding({ kind: 'coupler' })}>Add coupler</Button>
      </section>
      <Button variant="default" leftSection={<Plus size={16}/>} disabled={busy} onClick={() => setAdding({ kind: 'division' })}>Add division</Button>
    </nav>

    <div className="organ-detail">
      {busy && <Group className="organ-busy" gap="xs" role="status"><Loader size="xs"/><Text size="sm">Rebuilding the organ</Text></Group>}
      {stop && <StopEditor key={stop.id} organ={organ} stops={stops} stopId={stop.id} actions={stopActions} offerUndo={offerUndo} openScope={openScope}/>}
      {division && <DivisionEditor division={division} count={manuals.length} stops={stops.filter(s => s.midx === division.idx).length} busy={busy}
        rename={name => rename({ kind: 'division', idx: division.idx }, name)}
        kind={kind => void change('organ/manual/kind', { manual: division.idx, kind }, 'The keyboard type could not be changed.', { kind: 'division', name: division.name })}
        order={to => void change('organ/manual/order', { manual: division.idx, to }, 'The division could not be moved.', { kind: 'division', name: division.name })}
        remove={() => remove({ kind: 'division', idx: division.idx })}/>}
      {coupler && <CouplerEditor coupler={coupler} manuals={manuals} busy={busy}
        rename={name => rename({ kind: 'coupler', idx: coupler.idx }, name)}
        route={route => void change('organ/coupler/routes', { idx: coupler.idx, routes: JSON.stringify([route]) }, 'The coupler could not be changed.', { kind: 'coupler', name: coupler.name })}
        keep={keep => void change('organ/coupler', { idx: coupler.idx, keep: keep ? 1 : 0 }, 'The coupler could not be changed.')}
        remove={() => remove({ kind: 'coupler', idx: coupler.idx })}/>}
      {!current && !busy && <Stack align="center" p="xl"><Text>This organ has no stops yet.</Text><Button disabled={!manuals.length} onClick={() => manuals[0] && setAdding({ kind: 'stop', manual: manuals[0].name })}>Add stop</Button></Stack>}
    </div>

    <AddStop opened={adding?.kind === 'stop'} manual={adding?.kind === 'stop' ? adding.manual : ''} taken={adding?.kind === 'stop' ? named(adding.manual) : []} close={() => setAdding(undefined)}
      pull={(from, source, name) => change('organ/pull', { from, manual: source, on: adding?.kind === 'stop' ? adding.manual : '', ...(name ? { stop: name } : {}) },
        'That stop could not be added.', name && adding?.kind === 'stop' ? { kind: 'stop', name, manual: adding.manual } : undefined)}
      blank={name => change('organ/stop/blank', { on: adding?.kind === 'stop' ? adding.manual : '', name }, 'The blank stop could not be added.',
        adding?.kind === 'stop' ? { kind: 'stop', name, manual: adding.manual } : undefined)}
      addSource={path => change('organ/source/add', { path }, 'That sample set could not be added. Choose a GrandOrgue or unencrypted Hauptwerk organ.')}/>
    <AddDivision opened={adding?.kind === 'division'} close={() => setAdding(undefined)}
      add={(name, pedal) => change('organ/manual/add', { name, kind: pedal ? 'pedal' : 'manual', low: 36, high: pedal ? 67 : 96 }, 'The division could not be added. Use a name no other division has.', { kind: 'division', name })}/>
    <AddCoupler opened={adding?.kind === 'coupler'} manuals={manuals} close={() => setAdding(undefined)}
      add={(name, route) => change('organ/coupler/add', { name, routes: JSON.stringify([route]) }, 'The coupler could not be added. Use a name no other coupler has.', { kind: 'coupler', name })}/>

    <Modal opened={Boolean(confirm)} onClose={() => setConfirm(undefined)} title={confirm?.title}>
      <Stack><Text>{confirm?.message}</Text><Group justify="end"><Button variant="default" onClick={() => setConfirm(undefined)}>Cancel</Button>
        <Button color="red" data-autofocus onClick={() => { confirm?.run(); setConfirm(undefined); }}>Delete</Button></Group></Stack>
    </Modal>
    <Menu opened={Boolean(menu)} onChange={open => { if (!open) setMenu(undefined); }} position="bottom-start" withinPortal>
      <Menu.Target><div className="organ-menu-anchor" style={{ left: menu?.x ?? 0, top: menu?.y ?? 0 }}/></Menu.Target>
      <Menu.Dropdown>
        <Menu.Item leftSection={<Pencil size={16}/>} disabled={busy} onClick={() => menu && setRenaming(menu.target)}>Rename</Menu.Item>
        <Menu.Item leftSection={<Trash2 size={16}/>} color="red" disabled={busy} onClick={() => menu && remove(menu.target)}>Delete</Menu.Item>
      </Menu.Dropdown>
    </Menu>
    <RenameModal target={renaming && nameOf(renaming)} close={() => setRenaming(undefined)} save={name => { if (renaming) void rename(renaming, name); setRenaming(undefined); }}/>
    <Modal opened={Boolean(failure)} onClose={() => setFailure(undefined)} title="Organ not changed">
      <Stack><Text>{failure}</Text><Button onClick={() => setFailure(undefined)}>Close</Button></Stack>
    </Modal>
  </div>;
}

/** A name that saves on Enter or when the field loses focus. */
function Rename({ label, value, disabled, save, visible }: { label: string; value: string; disabled?: boolean; save: (name: string) => Promise<boolean>; visible?: string }) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = () => { const name = draft.trim(); if (!name) setDraft(value); else if (name !== value) void save(name).then(ok => { if (!ok) setDraft(value); }); };
  return <TextInput aria-label={label} label={visible} w={190} value={draft} disabled={disabled} onChange={e => setDraft(e.currentTarget.value)}
    onBlur={commit} onKeyDown={e => { if (e.key === 'Enter') e.currentTarget.blur(); if (e.key === 'Escape') { setDraft(value); e.currentTarget.blur(); } }}/>;
}

function RenameModal({ target, close, save }: { target?: string; close: () => void; save: (name: string) => void }) {
  const [name, setName] = useState('');
  useEffect(() => { if (target !== undefined) setName(target); }, [target]);
  const trimmed = name.trim();
  return <Modal opened={target !== undefined} onClose={close} title={`Rename ${target ?? ''}`}>
    <form onSubmit={e => { e.preventDefault(); if (trimmed && trimmed !== target) save(trimmed); else close(); }}><Stack>
      <TextInput label="Name" value={name} onChange={e => setName(e.currentTarget.value)} data-autofocus onFocus={e => e.currentTarget.select()}/>
      <Group justify="end"><Button variant="default" onClick={close}>Cancel</Button><Button type="submit" disabled={!trimmed}>Rename</Button></Group>
    </Stack></form>
  </Modal>;
}

function DivisionEditor({ division, count, stops, busy, rename, kind, order, remove }: {
  division: Snapshot['manuals'][number]; count: number; stops: number; busy: boolean;
  rename: (name: string) => Promise<boolean>; kind: (kind: string) => void; order: (to: number) => void; remove: () => void;
}) {
  const last = division.first_key !== undefined && division.key_count !== undefined ? division.first_key + division.key_count - 1 : undefined;
  return <Stack gap="md" className="organ-card">
    <div><Text className="roll-stop-name" fw={600}>{division.name}</Text><Text size="xs" c="dimmed">{stops} {stops === 1 ? 'stop' : 'stops'}{last !== undefined && ` · keys ${division.first_key}–${last}`}</Text></div>
    <Group align="end"><Rename label="Division name" visible="Name" value={division.name} disabled={busy} save={rename}/>
      <Stack gap={4}><Text size="sm" fw={500}>Keyboard</Text><SegmentedControl aria-label="Keyboard" disabled={busy} value={division.pedal ? 'pedal' : 'manual'} onChange={kind}
        data={[{ value: 'manual', label: 'Manual' }, { value: 'pedal', label: 'Pedal' }]}/></Stack></Group>
    <Group><Button variant="default" disabled={busy || division.idx === 0} onClick={() => order(division.idx - 1)}>Move up</Button>
      <Button variant="default" disabled={busy || division.idx === count - 1} onClick={() => order(division.idx + 1)}>Move down</Button>
      <Button variant="subtle" color="red" disabled={busy} onClick={remove}>Delete division</Button></Group>
  </Stack>;
}

type Route = { from: number; to: number; shift: number };
function CouplerEditor({ coupler, manuals, busy, rename, route, keep, remove }: {
  coupler: Snapshot['couplers'][number]; manuals: Snapshot['manuals']; busy: boolean;
  rename: (name: string) => Promise<boolean>; route: (route: Route) => void; keep: (keep: boolean) => void; remove: () => void;
}) {
  const first = coupler.routes[0];
  const simple = coupler.routes.length === 1 && first?.from !== undefined && first.to !== undefined;
  const now: Route | undefined = simple ? { from: first.from!, to: first.to!, shift: first.shift ?? 0 } : undefined;
  const division = manuals.map(m => ({ value: String(m.idx), label: m.name }));
  return <Stack gap="md" className="organ-card">
    <div><Text className="roll-stop-name" fw={600}>{coupler.name}</Text>
      <Text size="xs" c="dimmed">{coupler.routes.map(r => `${manuals.find(m => m.idx === r.from)?.name ?? 'Any'} → ${manuals.find(m => m.idx === r.to)?.name ?? 'Any'}${r.shift ? ` · ${pitchName(r.shift)}` : ''}`).join(' · ')}</Text></div>
    <Rename label="Coupler name" visible="Name" value={coupler.name} disabled={busy} save={rename}/>
    {now ? <Group align="end">
      <Select label="Play from" w={170} value={String(now.from)} allowDeselect={false} disabled={busy} data={division} onChange={v => v && route({ ...now, from: Number(v) })}/>
      <Select label="Also sounds" w={170} value={String(now.to)} allowDeselect={false} disabled={busy} data={division} onChange={v => v && route({ ...now, to: Number(v) })}/>
      <Select label="Pitch" w={150} value={String(now.shift)} allowDeselect={false} disabled={busy} data={pitches.some(p => Number(p.value) === now.shift) ? pitches : [...pitches, { value: String(now.shift), label: pitchName(now.shift) }]}
        onChange={v => v !== null && route({ ...now, shift: Number(v) })}/>
    </Group> : <Text size="sm" c="dimmed">This coupler has several routes; they are kept as the sample set defines them.</Text>}
    <Switch label="On the console" checked={!coupler.hidden} disabled={busy} onChange={e => keep(e.currentTarget.checked)}/>
    <Group><Button variant="subtle" color="red" disabled={busy} onClick={remove}>Delete coupler</Button></Group>
  </Stack>;
}

function AddStop({ opened, manual, taken, close, pull, blank, addSource }: {
  opened: boolean; manual: string; taken: string[]; close: () => void;
  pull: (from: string, sourceManual: string, stop?: string) => Promise<boolean>; blank: (name: string) => Promise<boolean>; addSource: (path: string) => Promise<boolean>;
}) {
  const [offerings, setOfferings] = useState<Offerings>();
  const [failed, setFailed] = useState(false);
  const [name, setName] = useState('');
  const load = () => { setFailed(false); request<Offerings>('GET', '/api/organ/offerings').then(setOfferings, () => setFailed(true)); };
  useEffect(() => {
    if (!opened) return;
    load();
    let n = 1;
    while (taken.includes((n === 1 ? 'new stop' : `new stop ${n}`))) n++;
    setName(n === 1 ? 'New stop' : `New stop ${n}`);
  }, [opened]);
  const done = (ok: boolean) => { if (ok) close(); };
  const clash = taken.includes(name.trim().toLowerCase());
  return <Drawer closeButtonProps={{ 'aria-label': 'Close' }} opened={opened} onClose={close} title={`Add stop to ${manual}`} position="right" size="md">
    <Stack gap="md">
      <form onSubmit={e => { e.preventDefault(); if (name.trim() && !clash) void blank(name.trim()).then(done); }}>
        <Stack gap={6}><Text fw={600}>Blank stop</Text>
          <Group align="start" wrap="nowrap">
            <TextInput aria-label="Blank stop name" style={{ flex: 1 }} value={name} onChange={e => setName(e.currentTarget.value)} error={clash ? `In ${manual}` : undefined}/>
            <Button type="submit" variant="default" disabled={!name.trim() || clash}>Add blank stop</Button>
          </Group>
        </Stack>
      </form>
      {failed && <Text>The sample sets could not be read. Close this and try again.</Text>}
      {!offerings && !failed && <Group><Loader size="xs"/><Text size="sm" c="dimmed">Reading sample sets</Text></Group>}
      {offerings?.sources.map(source => <Stack key={source.alias} gap={6}>
        <Text fw={600}>{source.name ?? source.alias}</Text>
        {source.error && <Text size="sm" c="dimmed">This sample set could not be read.</Text>}
        {source.manuals?.map(sm => <Stack key={sm.name} gap={4}>
          <Group justify="space-between"><Text size="xs" c="dimmed">{sm.name}</Text>
            <Button size="compact-xs" variant="subtle" disabled={sm.stops.some(s => taken.includes(s.name.toLowerCase()))} onClick={() => void pull(source.alias, sm.name).then(done)}>Add whole division</Button></Group>
          {sm.stops.map(s => { const here = taken.includes(s.name.toLowerCase()); return <Button key={s.name} justify="space-between" variant="default" disabled={here} onClick={() => void pull(source.alias, sm.name, s.name).then(done)}
            rightSection={here ? <Text component="span" size="xs" c="dimmed">In {manual}</Text> : s.pulled ? <Text component="span" size="xs" c="dimmed">In organ</Text> : undefined}>{s.name}</Button>; })}
        </Stack>)}
      </Stack>)}
      <Button variant="default" leftSection={<Folder size={16}/>} disabled={!canPickFiles}
        onClick={() => void pickFile('Add a sample set', organFiles.slice(0, 1)).then(path => { if (path) void addSource(path).then(ok => { if (ok) load(); }); })}>Add a sample set</Button>
    </Stack>
  </Drawer>;
}

function AddDivision({ opened, close, add }: { opened: boolean; close: () => void; add: (name: string, pedal: boolean) => Promise<boolean> }) {
  const [name, setName] = useState('');
  const [pedal, setPedal] = useState(false);
  useEffect(() => { if (opened) { setName(''); setPedal(false); } }, [opened]);
  return <Drawer closeButtonProps={{ 'aria-label': 'Close' }} opened={opened} onClose={close} title="Add division" position="right">
    <form onSubmit={e => { e.preventDefault(); if (name.trim()) void add(name.trim(), pedal).then(ok => { if (ok) close(); }); }}><Stack>
      <TextInput label="Name" value={name} onChange={e => setName(e.currentTarget.value)} data-autofocus/>
      <SegmentedControl aria-label="Keyboard" value={pedal ? 'pedal' : 'manual'} onChange={v => setPedal(v === 'pedal')} data={[{ value: 'manual', label: 'Manual' }, { value: 'pedal', label: 'Pedal' }]}/>
      <Button type="submit" disabled={!name.trim()}>Add division</Button>
    </Stack></form>
  </Drawer>;
}

function AddCoupler({ opened, manuals, close, add }: { opened: boolean; manuals: Snapshot['manuals']; close: () => void; add: (name: string, route: Route) => Promise<boolean> }) {
  const [route, setRoute] = useState<Route>({ from: 0, to: 1, shift: 0 });
  const [name, setName] = useState('');
  const division = manuals.map(m => ({ value: String(m.idx), label: m.name }));
  const suggested = `${manuals.find(m => m.idx === route.from)?.name ?? ''} to ${manuals.find(m => m.idx === route.to)?.name ?? ''}${route.shift ? ` ${route.shift > 0 ? '4′' : '16′'}` : ''}`;
  useEffect(() => { if (opened) { setRoute({ from: 0, to: Math.min(1, manuals.length - 1), shift: 0 }); setName(''); } }, [opened, manuals.length]);
  return <Drawer closeButtonProps={{ 'aria-label': 'Close' }} opened={opened} onClose={close} title="Add coupler" position="right">
    <form onSubmit={e => { e.preventDefault(); void add(name.trim() || suggested, route).then(ok => { if (ok) close(); }); }}><Stack>
      <Select label="Play from" value={String(route.from)} allowDeselect={false} data={division} onChange={v => v && setRoute({ ...route, from: Number(v) })}/>
      <Select label="Also sounds" value={String(route.to)} allowDeselect={false} data={division} onChange={v => v && setRoute({ ...route, to: Number(v) })}/>
      <Select label="Pitch" value={String(route.shift)} allowDeselect={false} data={pitches} onChange={v => v !== null && setRoute({ ...route, shift: Number(v) })}/>
      <TextInput label="Name" placeholder={suggested} value={name} onChange={e => setName(e.currentTarget.value)}/>
      <Button type="submit" disabled={route.from === route.to}>Add coupler</Button>
    </Stack></form>
  </Drawer>;
}
