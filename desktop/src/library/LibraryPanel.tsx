import { useEffect, useMemo, useRef, useState } from 'react';
import { ActionIcon, Badge, Button, Group, Loader, Menu, Modal, Paper, Stack, Text, TextInput, UnstyledButton } from '@mantine/core';
import { EllipsisVertical, FilePlus, FolderOpen, Pencil, Search, Trash2 } from 'lucide-react';
import { canPickFiles, organFiles, pickFile, request, type LibraryEntry, type Snapshot } from '../api';
import './library.css';

type Details = { path: string; format: string; location: string | null };
type Send = (path: string, values?: Record<string, string | number>) => Promise<Snapshot>;

export function LibraryPanel({ state, send, load, create, play }: { state?: Snapshot; send: Send; load: (path: string) => void; create: (name: string) => Promise<unknown>; play: () => void }) {
  const library = state?.library ?? [];
  const [details, setDetails] = useState<Record<string, Details>>({});
  const [query, setQuery] = useState('');
  const [naming, setNaming] = useState(false);
  const [renaming, setRenaming] = useState<string>();
  const [deleting, setDeleting] = useState<LibraryEntry>();
  const [failure, setFailure] = useState<string>();
  const listed = library.map(entry => `${entry.path}\n${entry.name}`).join('\n');
  useEffect(() => {
    request<Details[]>('GET', '/api/library').then(rows => { if (Array.isArray(rows)) setDetails(Object.fromEntries(rows.map(row => [row.path, row]))); }, () => {});
  }, [listed]);
  const shown = useMemo(() => {
    const words = query.trim().toLowerCase();
    if (!words) return library;
    return library.filter(entry => `${entry.name} ${details[entry.path]?.format ?? ''} ${details[entry.path]?.location ?? ''}`.toLowerCase().includes(words));
  }, [library, details, query]);

  const remove = (entry: LibraryEntry) => {
    setDeleting(undefined);
    send('library/delete', { path: entry.path }).catch(() => setFailure(`${entry.name} could not be deleted. Check that its file is not read-only, then try again.`));
  };

  return <Stack className="library" gap="md">
    <Group justify="space-between">
      <Text fw={600} size="lg">Library</Text>
      <Group gap="xs">
        {library.length > 6 && <TextInput aria-label="Search organs" placeholder="Search" leftSection={<Search size={16}/>} value={query} onChange={e => setQuery(e.currentTarget.value)}/>}
        <Button variant="default" leftSection={<FilePlus size={18}/>} onClick={() => setNaming(true)} disabled={naming || Boolean(state?.loading)}>New organ</Button>
        <Button leftSection={<FolderOpen size={18}/>} disabled={!canPickFiles} title={canPickFiles ? undefined : 'Open the Aristide desktop app to choose a file'}
          onClick={() => void pickFile('Load from GrandOrgue/Hauptwerk', organFiles).then(path => path && load(path))}>Load from GrandOrgue/Hauptwerk</Button>
      </Group>
    </Group>
    {state?.loading && <Paper withBorder p="md"><Group><Loader size="sm"/><Text>{state.loading}</Text></Group></Paper>}
    {naming && <Paper withBorder className="library-row">
      <NameForm name="" action="Create" failure="The organ could not be created. Try another name." submit={create} done={() => setNaming(false)}/>
    </Paper>}
    {library.length > 0 && <Paper withBorder className="library-list" role="list">
      {shown.map(entry => <LibraryRow key={entry.path} entry={entry} details={details[entry.path]} busy={Boolean(state?.loading)}
        renaming={renaming === entry.path} startRename={() => setRenaming(entry.path)} stopRename={() => setRenaming(undefined)}
        rename={name => send('library/rename', { path: entry.path, name })}
        open={() => entry.loaded ? play() : load(entry.path)} askDelete={() => setDeleting(entry)}/>)}
      {!shown.length && <Text c="dimmed" p="md">No organ matches “{query.trim()}”.</Text>}
    </Paper>}
    {!library.length && !state?.loading && !naming && <Stack align="center" gap="xs" py="xl">
      <Text fw={600}>No organs yet</Text>
      <Text c="dimmed" size="sm">GrandOrgue and unencrypted Hauptwerk organs</Text>
    </Stack>}
    <Modal opened={Boolean(deleting)} onClose={() => setDeleting(undefined)} title={`Are you sure you want to delete ${deleting?.name}?`}>
      <Stack>
        <Text>{deleting?.owned
          ? 'Its settings, combinations and edits are deleted. The sample files stay on disk.'
          : 'It is removed from the Library. Its files stay on disk, and it can be loaded again from GrandOrgue/Hauptwerk.'}</Text>
        <Group justify="flex-end">
          <Button variant="default" onClick={() => setDeleting(undefined)} data-autofocus>Cancel</Button>
          <Button color="red" onClick={() => deleting && remove(deleting)}>Delete</Button>
        </Group>
      </Stack>
    </Modal>
    <Modal opened={Boolean(failure)} onClose={() => setFailure(undefined)} title="Organ not deleted">
      <Stack><Text>{failure}</Text><Button onClick={() => setFailure(undefined)}>Close</Button></Stack>
    </Modal>
  </Stack>;
}

function LibraryRow({ entry, details, busy, renaming, startRename, stopRename, rename, open, askDelete }: {
  entry: LibraryEntry; details?: Details; busy: boolean; renaming: boolean;
  startRename: () => void; stopRename: () => void; rename: (name: string) => Promise<unknown>; open: () => void; askDelete: () => void;
}) {
  const meta = [details?.format, details?.location, played(entry.played)].filter(Boolean) as string[];
  return <div className="library-row" role="listitem" data-loaded={entry.loaded || undefined}>
    {renaming ? <NameForm name={entry.name} action="Save" failure="Another organ has this name, or its file is read-only." submit={rename} done={stopRename}/>
      : <UnstyledButton className="library-open" onClick={open} disabled={busy && !entry.loaded} aria-label={entry.loaded ? `${entry.name}, playing` : `Load ${entry.name}`}>
        <Text fw={600} truncate="end">{entry.name}</Text>
        {meta.length > 0 && <Text component="div" size="sm" c="dimmed" className="library-meta" title={details?.location ?? undefined}>
          {meta.map((part, i) => <span key={i} data-location={part === details?.location || undefined}>{part}</span>)}
        </Text>}
      </UnstyledButton>}
    {!renaming && entry.loaded && <Badge variant="light" size="lg">Playing</Badge>}
    {!renaming && <Menu position="bottom-end" withinPortal>
      <Menu.Target><ActionIcon variant="subtle" color="gray" size={48} aria-label={`${entry.name} actions`}><EllipsisVertical size={20}/></ActionIcon></Menu.Target>
      <Menu.Dropdown>
        <Menu.Item leftSection={<Pencil size={16}/>} onClick={startRename}>Rename</Menu.Item>
        <Menu.Item leftSection={<Trash2 size={16}/>} color="red" disabled={entry.loaded} onClick={askDelete}>{entry.loaded ? 'Delete (playing now)' : 'Delete'}</Menu.Item>
      </Menu.Dropdown>
    </Menu>}
  </div>;
}

function NameForm({ name, action, failure, submit: save, done }: { name: string; action: string; failure: string; submit: (name: string) => Promise<unknown>; done: () => void }) {
  const [value, setValue] = useState(name);
  const [error, setError] = useState<string>();
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => { input.current?.select(); }, []);
  const submit = () => {
    const next = value.trim();
    if (!next || next === name) { done(); return; }
    save(next).then(done, () => setError(failure));
  };
  return <form className="library-rename" onSubmit={e => { e.preventDefault(); submit(); }}>
    <TextInput ref={input} aria-label="Organ name" placeholder="Organ name" value={value} error={error} autoFocus
      onChange={e => { setValue(e.currentTarget.value); setError(undefined); }}
      onKeyDown={e => { if (e.key === 'Escape') { e.stopPropagation(); done(); } }}/>
    <Button type="submit" disabled={!value.trim()}>{action}</Button>
    <Button variant="default" onClick={done}>Cancel</Button>
  </form>;
}

function played(seconds?: number) {
  if (!seconds) return undefined;
  const then = new Date(seconds * 1000);
  const days = Math.round((startOfDay(new Date()) - startOfDay(then)) / 86_400_000);
  if (days <= 0) return 'Played today';
  if (days === 1) return 'Played yesterday';
  if (days < 7) return `Played ${days} days ago`;
  const sameYear = then.getFullYear() === new Date().getFullYear();
  return `Played ${then.toLocaleDateString(undefined, { day: 'numeric', month: 'short', ...(sameYear ? {} : { year: 'numeric' }) })}`;
}

function startOfDay(date: Date) {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}
