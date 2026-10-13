import type { PitchWindow, RollModel, RollNote } from './rollModel';

/** Design-study fixture: a Plein jeu whose mixture breaks, two unison 8′s, and a pedal rank that runs out. */
export type LimitNote = RollNote & { played?: PitchWindow | null; sounding?: PitchWindow | null };
export type PipeSource = { id: string; name: string; division: string; low: number; high: number };
export type Tuning = 12 | 19;
export type Status = 'sounds' | 'key' | 'sounding' | 'missing';

export const compass = Array.from({ length: 54 }, (_, i) => 36 + i);
export const reference = { key: 69, hz: 415 };
const span = (first: number, last: number, offset = 0) => ({ low: (first - 69) * 100 + offset, high: (last - 69) * 100 + offset });

export const pipeSources: PipeSource[] = [
  { id: 'bourdon', name: 'Bourdon 8′', division: 'Grand-orgue', ...span(36, 89) },
  { id: 'flute', name: 'Flûte 8′', division: 'Positif', ...span(36, 89) },
  { id: 'pedal', name: 'Bourdon 8′', division: 'Pédale', ...span(36, 62) },
  { id: 'fourniture-1', name: 'Fourniture I', division: 'Grand-orgue', ...span(36, 89, 1900) },
  { id: 'fourniture-2', name: 'Fourniture II', division: 'Grand-orgue', ...span(36, 89, 2400) },
  { id: 'tierce', name: 'Tierce 1⅗′', division: 'Grand-orgue', ...span(36, 89, 2786) },
];

const held = (id: string, source: string, pitch: number, limits: Partial<LimitNote> = {}): LimitNote =>
  ({ id, source, pitch, start: 'down', end: null, level: 0, played: null, sounding: null, ...limits });
export const pleinJeu: LimitNote[] = [
  held('bourdon', 'bourdon', 0),
  held('flute', 'flute', 0),
  held('pedal', 'pedal', 0),
  held('f1-low', 'fourniture-1', 1900, { played: { high: -950 } }),
  held('f1-high', 'fourniture-1', 700, { played: { low: -950 } }),
  held('f2-low', 'fourniture-2', 2400, { played: { high: -950 } }),
  held('f2-high', 'fourniture-2', 1200, { played: { low: -950 } }),
  held('tierce', 'tierce', 2786, { sounding: { high: 3600 } }),
];

export const stepCents = (tuning: Tuning) => 1200 / tuning;
/** The pitch a key plays, in cents from the reference key's pitch. */
export const keyCents = (key: number, tuning: Tuning) => (key - reference.key) * stepCents(tuning);
export const hz = (cents: number) => reference.hz * 2 ** (cents / 1200);
export const formatHz = (cents: number) => `${hz(cents) < 100 ? hz(cents).toFixed(1) : Math.round(hz(cents))} Hz`;
export const formatCents = (cents: number) => `${cents > 0 ? '+' : cents < 0 ? '−' : ''}${Math.abs(Math.round(cents))} ¢`;
const names = ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'];
/** Conventional names only where the keyboard is a conventional twelve; neutral key numbers otherwise. */
export const keyName = (key: number, tuning: Tuning) => tuning === 12 ? `${names[key % 12]}${Math.floor(key / 12) - 1}` : `key ${key - compass[0] + 1}`;
export const referenceName = (tuning: Tuning) => `${keyName(reference.key, tuning)} = ${reference.hz} Hz`;
export const contains = (window: PitchWindow | null | undefined, cents: number) =>
  !window || ((window.low === undefined || cents >= window.low) && (window.high === undefined || cents <= window.high));

/** Why an event does or doesn't sound on one key: its key limits, its sounding limits, or no pipe there. */
export function status(note: LimitNote, key: number, tuning: Tuning): Status {
  const played = keyCents(key, tuning);
  if (!contains(note.played, played)) return 'key';
  const sounding = played + note.pitch;
  if (!contains(note.sounding, sounding)) return 'sounding';
  const source = pipeSources.find(s => s.id === note.source)!;
  return sounding < source.low - 50 || sounding > source.high + 50 ? 'missing' : 'sounds';
}

export const sourceLabel = (source: PipeSource) => source.division === 'Grand-orgue' ? source.name : `${source.name} · ${source.division}`;
export const rollModel = (notes: LimitNote[]): RollModel => ({
  stamps: [{ id: 'down', anchor: 'down', ms: 0 }, { id: 'up', anchor: 'up', ms: 0 }],
  notes,
  sources: pipeSources.map(s => ({ id: s.id, name: sourceLabel(s), kind: 'Rank', organ: s.division, transpose: 0 })),
});
