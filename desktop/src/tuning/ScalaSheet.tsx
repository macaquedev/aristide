import { useEffect, useState } from 'react';
import { ActionIcon, Button, Drawer, Group, Stack, Text } from '@mantine/core';
import { FolderOpen, X } from 'lucide-react';
import { canPickFiles, mappingFiles, pickFile, scaleFiles } from '../api';

const fileName = (path: string) => path.split(/[\\/]/).at(-1) ?? path;

/** Picks a Scala scale and, optionally, its keyboard mapping from disk. */
export function ScalaSheet({ opened, close, apply, failed }: { opened: boolean; close: () => void; apply: (scl: string, kbm?: string) => void; failed?: boolean }) {
  const [scl, setScl] = useState<string>();
  const [kbm, setKbm] = useState<string>();
  useEffect(() => { if (opened) { setScl(undefined); setKbm(undefined); } }, [opened]);
  const choose = (title: string, filters: typeof scaleFiles, set: (path: string) => void) =>
    void pickFile(title, filters).then(path => { if (path) set(path); });
  return <Drawer opened={opened} onClose={close} title="Import Scala" position="right" size="md"><Stack>
    <Group justify="space-between" wrap="nowrap">
      <Stack gap={0} miw={0}><Text size="sm" c="dimmed">Scale</Text><Text truncate="end">{scl ? fileName(scl) : 'None'}</Text></Stack>
      <Button variant="default" leftSection={<FolderOpen size={18}/>} disabled={!canPickFiles} onClick={() => choose('Choose a Scala scale', scaleFiles, setScl)}>Choose .scl</Button>
    </Group>
    <Group justify="space-between" wrap="nowrap">
      <Stack gap={0} miw={0}><Text size="sm" c="dimmed">Keyboard mapping</Text><Text truncate="end">{kbm ? fileName(kbm) : 'Consecutive keys'}</Text></Stack>
      <Group gap="xs" wrap="nowrap">
        {kbm && <ActionIcon variant="subtle" color="gray" size="lg" aria-label="Use consecutive keys" onClick={() => setKbm(undefined)}><X size={18}/></ActionIcon>}
        <Button variant="default" leftSection={<FolderOpen size={18}/>} disabled={!canPickFiles} onClick={() => choose('Choose a keyboard mapping', mappingFiles, setKbm)}>Choose .kbm</Button>
      </Group>
    </Group>
    {failed && <Text>That scale could not be read. Choose another file.</Text>}
    <Group grow><Button variant="default" onClick={close}>Cancel</Button><Button disabled={!scl} onClick={() => scl && apply(scl, kbm)}>Use scale</Button></Group>
  </Stack></Drawer>;
}
