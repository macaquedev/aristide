import { invoke, isTauri } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';

export type Stop = {
  id: number; name: string; midx: number; manual: string; on: boolean;
  pitch: { native: number | null; footage: number | null; cents: number; gain: number; own: boolean };
  ranks: { id: number; name: string }[];
  custom?: boolean;
  tuning?: { scope: string; follow: string };
};
export type Snapshot = {
  organ?: string; loading?: string; load_error?: string;
  stops: Stop[];
  manuals: { idx: number; name: string; pedal: boolean; held: number[]; first_key?: number; key_count?: number; kind?: string }[];
  couplers: { idx: number; name: string; on: boolean; hidden?: boolean; routes: { from?: number; to?: number; shift?: number }[] }[];
  trems: { idx: number; name: string; on: boolean }[];
  generals: number[]; setter: boolean; gain: number;
  combinations?: { matching_generals: number[]; divisionals: Record<string, number[]>; matching_divisionals: Record<string, number[]>; frame: number; frames: number };
  library: LibraryEntry[];
  memory?: { resident_mb: number; samples: number };
  manual_tuning?: { idx: number; temperament: string; reference: { hz: number } }[];
  midi: { ports: { id: number; name: string }[]; manuals: { idx: number; name: string; inputs: { device: string; connected: boolean }[] }[]; learning?: { manual: number; slot: number; step: string } };
  keyboard?: { manual: number };
};

export type LibraryEntry = { name: string; path: string; played?: number; loaded?: boolean; owned?: boolean };
export const native = isTauri();
// GTK matches extensions case-sensitively; sample sets ship in either case.
const extensions = (...names: string[]) => names.flatMap(name => [...new Set([name, name.toLowerCase(), name.toUpperCase()])]);
export const organFiles = [
  { name: 'GrandOrgue and Hauptwerk organs', extensions: extensions('organ', 'Organ_Hauptwerk_xml') },
  { name: 'Aristide organs', extensions: ['toml'] },
];
export const scaleFiles = [{ name: 'Scala scales', extensions: extensions('scl') }];
export const mappingFiles = [{ name: 'Scala keyboard mappings', extensions: extensions('kbm') }];

type Filters = { name: string; extensions: string[] }[];
// Browser tests stand in for the system dialog, which they cannot drive.
const testPicker = (globalThis as { aristidePickFile?: (title: string, filters: Filters) => Promise<string | undefined> }).aristidePickFile;
export const canPickFiles = native || Boolean(testPicker);

/** The operating system's file picker; only the desktop app has one. */
export async function pickFile(title: string, filters: Filters) {
  if (testPicker) return testPicker(title, filters);
  if (!native) return undefined;
  const path = await open({ title, filters, multiple: false, directory: false });
  return typeof path === 'string' ? path : undefined;
}

export const endpoint = (path: string, values: Record<string, string | number> = {}) =>
  `/api/${path}${Object.keys(values).length ? `?${new URLSearchParams(Object.entries(values).map(([k, v]) => [k, String(v)]))}` : ''}`;

export async function request<T>(method: 'GET' | 'POST', url: string): Promise<T> {
  const reply = native
    ? await invoke<{ status: number; body: T }>('api_request', { method, url })
    : await fetch(url, { method, signal: AbortSignal.timeout(10_000) }).then(async r => ({ status: r.status, body: (r.ok ? await r.json() : undefined) as T }));
  if (reply.status >= 400) throw new Error(`request-${reply.status}`);
  return reply.body;
}

/** An edit to the instrument itself. A sample set's own organ stays as the
 * set defines it: the first such edit saves the player's own copy, then retries. */
export async function editInstrument<T>(organ: string, path: string, values: Record<string, string | number>): Promise<T> {
  try { return await request<T>('POST', endpoint(path, values)); }
  catch (error) {
    if ((error as Error).message !== 'request-409') throw error;
    await request('POST', endpoint('organ/save_as', { name: `${organ} (edited)` }));
    return request<T>('POST', endpoint(path, values));
  }
}

export async function status() {
  return native ? invoke<{ ready: boolean; error: string | null }>('runtime_status') : { ready: true, error: null };
}
