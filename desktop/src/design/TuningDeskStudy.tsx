import { useState } from 'react';
import { TuningDesk, type DeskScope, type Part } from '../tuning/TuningDesk';
import { defaultAnchor, equalTemperament, type Anchor, type Shape } from '../tuning/model';

export { tuningDeskLayouts } from '../tuning/TuningDesk';

type StudyScope = { id: string; name: string; parent?: string; anchor?: Anchor; shape?: Shape };

const initialScopes: StudyScope[] = [
  { id: 'instrument', name: 'Whole instrument', anchor: defaultAnchor, shape: equalTemperament },
  { id: 'great', name: 'Grand-orgue', parent: 'instrument' },
  { id: 'bourdon', name: 'Bourdon', parent: 'great' },
  { id: 'bourdon-c4', name: 'Bourdon · C4 pipe', parent: 'bourdon' },
  { id: 'recit', name: 'Récit', parent: 'instrument', anchor: { ...defaultAnchor, hz: 415 },
    shape: { system: 'temperament', temperament: 'custom', root: 0, offsets: [0, -8, 4, -4, 8, 0, -8, 4, -4, 8, 0, -8] } },
  { id: 'flute', name: 'Flûte', parent: 'recit' },
];

function resolved(scopes: StudyScope[], scope: StudyScope): DeskScope {
  const inherit = <K extends 'anchor' | 'shape'>(key: K): NonNullable<StudyScope[K]> => {
    let current: StudyScope | undefined = scope;
    while (current && !current[key]) current = scopes.find(s => s.id === current!.parent);
    return current![key]!;
  };
  return { id: scope.id, name: scope.name, parent: scope.parent, own: { anchor: Boolean(scope.anchor), scale: Boolean(scope.shape) }, anchor: inherit('anchor'), shape: inherit('shape') };
}

/** Silent, unsaved study data behind the same desk the connected panel uses. */
export function TuningDeskStudy({ layout, assign }: { layout: string; assign: (name: string) => void }) {
  const [scopes, setScopes] = useState(initialScopes);
  const [history, setHistory] = useState<StudyScope[][]>([]);
  const [selected, setSelected] = useState('instrument');
  const views = scopes.map(s => resolved(scopes, s));
  const edit = (id: string, change: (scope: StudyScope, view: DeskScope) => StudyScope) => {
    setHistory(h => [...h, scopes]);
    setScopes(scopes.map((s, i) => s.id === id ? change(s, views[i]) : s));
  };
  const own = (id: string, part: Part, owned: boolean) => edit(id, (s, view) => part === 'anchor'
    ? { ...s, anchor: owned ? view.anchor : undefined } : { ...s, shape: owned ? view.shape : undefined });
  return <TuningDesk layout={layout} scopes={views} selected={selected} select={setSelected} assign={assign}
    setAnchor={(id, anchor) => edit(id, s => ({ ...s, anchor }))} setShape={(id, shape) => edit(id, s => ({ ...s, shape }))} setOwn={own}
    canUndo={history.length > 0} undo={() => { if (!history.length) return; setScopes(history.at(-1)!); setHistory(history.slice(0, -1)); }}/>;
}
