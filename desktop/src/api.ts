import { invoke, isTauri } from '@tauri-apps/api/core';

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
  manuals: { idx: number; name: string; pedal: boolean; held: number[] }[];
  couplers: { idx: number; name: string; on: boolean; hidden?: boolean; routes: { from: number; to: number }[] }[];
  trems: { idx: number; name: string; on: boolean }[];
  generals: number[]; setter: boolean; gain: number;
  combinations?: { matching_generals: number[]; divisionals: Record<string, number[]>; matching_divisionals: Record<string, number[]>; frame: number; frames: number };
  library: { name: string; path: string }[];
  memory?: { resident_mb: number; samples: number };
  manual_tuning?: { idx: number; temperament: string; reference: { hz: number } }[];
  midi: { ports: { id: number; name: string }[]; manuals: { idx: number; name: string; inputs: { device: string; connected: boolean }[] }[]; learning?: { manual: number; slot: number; step: string } };
  keyboard?: { manual: number };
};

export type Browse = { dir: string; parent: string | null; entries: { name: string; path: string; dir: boolean }[] };
export const native = isTauri();
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
