export type Voice = { id: string; source: string; pitch: number; delay: number; level: number; output: string; low: number; high: number; tuning: string };
export type BuildState = { voices: Voice[]; keys: Record<number, Record<string, Partial<Voice>>> };
export const initialBuild: BuildState = {
  voices: [
    { id: 'one', source: 'Study organ · Bourdon', pitch: 0, delay: 0, level: 0, output: 'Follow stop', low: 36, high: 96, tuning: 'Follow stop' },
    { id: 'two', source: 'Study organ · Flute', pitch: 700, delay: 120, level: -6, output: 'Follow stop', low: 36, high: 96, tuning: 'Follow stop' },
    { id: 'three', source: 'Second organ · Principal', pitch: 1200, delay: 240, level: -12, output: 'Rear', low: 36, high: 96, tuning: 'Keep source' },
  ], keys: {},
};
export const noteName = (key: number) => `${['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'][((key % 12) + 12) % 12]}${Math.floor(key / 12) - 1}`;
export const sources = ['Study organ · Bourdon', 'Study organ · Flute', 'Study organ · Trompette', 'Second organ · Principal', 'Second organ · String'];
export const outputs = ['Follow stop', 'Front L+R', 'Rear', 'Sub'];
export type Tuning = { hz: number; temperament: string; system: string; steps: number; root: string; fine: number; deviations: number[]; period: number | null; intervals: number[]; referenceKey: number };
export type Scope = { id: string; name: string; parent?: string; own?: Tuning };
export const defaultTuning: Tuning = { hz: 440, temperament: 'Equal', system: 'Twelve-note', steps: 12, root: 'C', fine: 0, deviations: Array(12).fill(0), period: 1200, intervals: [0, 137, 311, 523, 887, 1460, 2107], referenceKey: 69 };
export const initialScopes: Scope[] = [
  { id: 'instrument', name: 'Whole instrument', own: defaultTuning },
  { id: 'great', name: 'Grand-orgue', parent: 'instrument' },
  { id: 'bourdon', name: 'Bourdon', parent: 'great' },
  { id: 'bourdon-c4', name: 'Bourdon · C4 pipe', parent: 'bourdon' },
  { id: 'recit', name: 'Récit', parent: 'instrument', own: { ...defaultTuning, hz: 415, temperament: 'Custom', deviations: [0, -8, 4, -4, 8, 0, -8, 4, -4, 8, 0, -8] } },
  { id: 'flute', name: 'Flûte', parent: 'recit' },
];
export function resolve(scopes: Scope[], id: string): Tuning {
  const scope = scopes.find(scope => scope.id === id)!;
  return scope.own ?? resolve(scopes, scope.parent!);
}
