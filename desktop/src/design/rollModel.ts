export type Anchor = 'down' | 'up';
export type Stamp = { id: string; anchor: Anchor; ms: number };
export type RollNote = { id: string; source: string; pitch: number; start: string; end: string | null; level: number };
export type Source = { id: string; name: string; kind: 'Rank' | 'Stop'; organ: string; transpose: number };
export type RollModel = { stamps: Stamp[]; notes: RollNote[]; sources: Source[] };
export const anchorName = (anchor: Anchor) => anchor === 'down' ? 'Key down' : 'Key up';
export const cents = (pitch: number) => `${pitch > 0 ? '+' : ''}${Number(pitch.toFixed(2))} ¢`;
export const initialRoll: RollModel = {
  stamps: [
    { id: 'd0', anchor: 'down', ms: 0 }, { id: 'd50', anchor: 'down', ms: 50 },
    { id: 'u0', anchor: 'up', ms: 0 }, { id: 'u50', anchor: 'up', ms: 50 }, { id: 'u150', anchor: 'up', ms: 150 },
  ],
  notes: [
    { id: 'theorbe', source: 'theorbe', pitch: 0, start: 'd0', end: null, level: 0 },
    { id: 'flute', source: 'flute', pitch: 1250, start: 'd0', end: 'd50', level: 0 },
    { id: 'ophicleide', source: 'ophicleide', pitch: -2, start: 'd50', end: null, level: 0 },
  ],
  sources: [
    { id: 'theorbe', name: 'Théorbe', kind: 'Stop', organ: 'This organ', transpose: 0 },
    { id: 'flute', name: 'Flute 4′', kind: 'Stop', organ: 'This organ', transpose: 0 },
    { id: 'ophicleide', name: 'Ophicleide 32′', kind: 'Stop', organ: 'This organ', transpose: 0 },
    { id: 'bourdon', name: 'Bourdon 8′', kind: 'Rank', organ: 'This organ', transpose: 0 },
    { id: 'string', name: 'Salicional 8′', kind: 'Rank', organ: 'Second organ', transpose: 0 },
  ],
};
export function validNote(note: RollNote, stamps: Stamp[]) {
  const start = stamps.find(s => s.id === note.start);
  const end = stamps.find(s => s.id === note.end);
  if (!start || !Number.isFinite(note.pitch)) return false;
  if (note.end === null) return start.anchor === 'down';
  if (!end) return false;
  return start.anchor === end.anchor ? end.ms > start.ms : start.anchor === 'down' && end.anchor === 'up';
}
export function nearestStamp(stamps: Stamp[], anchor: Anchor, ms: number) {
  return stamps.filter(s => s.anchor === anchor).reduce((a, b) => Math.abs(a.ms - ms) <= Math.abs(b.ms - ms) ? a : b);
}
export function snapPitch(pitch: number, steps: number, snap: boolean) {
  return Number((snap ? Math.round(pitch / (1200 / steps)) * (1200 / steps) : pitch).toFixed(2));
}
