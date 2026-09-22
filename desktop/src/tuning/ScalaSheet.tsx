import { useEffect, useState } from 'react';
import { Button, Drawer, Group, Stack, Text, TextInput } from '@mantine/core';
import { ArrowLeft, Folder } from 'lucide-react';
import { endpoint, request, type Browse } from '../api';

const fileName = (path: string) => path.split(/[\\/]/).at(-1) ?? path;

/** Picks a Scala scale and, optionally, its keyboard mapping from disk. */
export function ScalaSheet({ opened, close, apply, failed }: { opened: boolean; close: () => void; apply: (scl: string, kbm?: string) => void; failed?: boolean }) {
  const [browse, setBrowse] = useState<Browse>();
  const [path, setPath] = useState('');
  const [unreadable, setUnreadable] = useState(false);
  const [scl, setScl] = useState<string>();
  const [kbm, setKbm] = useState<string>();
  const visit = async (dir: string) => {
    setUnreadable(false);
    try { const value = await request<Browse>('GET', endpoint('browse', { dir, kind: 'scala' })); setBrowse(value); setPath(value.dir); }
    catch { setUnreadable(true); }
  };
  useEffect(() => { if (opened) { setScl(undefined); setKbm(undefined); void visit(browse?.dir ?? ''); } }, [opened]);
  const files = browse?.entries.filter(e => !e.dir) ?? [];
  return <Drawer opened={opened} onClose={close} title="Import Scala" position="right" size="lg"><Stack>
    <form onSubmit={e => { e.preventDefault(); void visit(path); }}><Group wrap="nowrap"><TextInput aria-label="Folder" value={path} onChange={e => setPath(e.currentTarget.value)} style={{ flex: 1 }}/><Button type="submit">Go</Button></Group></form>
    {unreadable && <Text>That folder could not be opened. Choose another folder.</Text>}
    {browse?.parent && <Button variant="default" leftSection={<ArrowLeft size={18}/>} onClick={() => void visit(browse.parent!)}>Parent folder</Button>}
    {browse?.entries.filter(e => e.dir).map(entry => <Button key={entry.path} justify="start" variant="default" leftSection={<Folder size={18}/>} onClick={() => void visit(entry.path)}>{entry.name}</Button>)}
    {files.map(entry => {
      const mapping = /\.kbm$/i.test(entry.name);
      const chosen = entry.path === (mapping ? kbm : scl);
      return <Button key={entry.path} justify="start" variant={chosen ? 'light' : 'default'} aria-pressed={chosen}
        onClick={() => mapping ? setKbm(chosen ? undefined : entry.path) : setScl(entry.path)}>{entry.name}</Button>;
    })}
    {browse && !files.length && <Text c="dimmed">No .scl or .kbm files here</Text>}
    <Text size="sm">{scl ? fileName(scl) : 'Choose a .scl scale'}{kbm ? ` · ${fileName(kbm)}` : scl ? ' · consecutive keys' : ''}</Text>
    {failed && <Text>That scale could not be read. Choose another file.</Text>}
    <Group grow><Button variant="default" onClick={close}>Cancel</Button><Button disabled={!scl} onClick={() => scl && apply(scl, kbm)}>Use scale</Button></Group>
  </Stack></Drawer>;
}
