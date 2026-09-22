# Tuning desk: shape, placement and anchor

The Tuning desk moved to `desktop/src/tuning/` as one component shared by
the design study and the coming connected panel. Its model keeps three facts
apart: the shape (twelve-class temperament, equal division, pitch collection
or Scala file), its placement (temperament root, or the key that plays step
1) and the anchor (reference key, Hz and fine offset). Each scope owns or
follows the anchor and the shape separately, with a Follow switch in the
Reference and Scale modules; the header links to whichever scope supplies
each part.

Alex found "Root note C" beside "440 Hz" confusing: the root places the
temperament and the reference pins the pitch, and they are independent. The
reference key is now editable in every system and shown with its note name.
The deviation graph is zeroed at the reference note, so A reads 0 at A4 =
440, and that column is fixed. A readout names the wolf along the
temperament's chain (G♯–E♭ on C, A♯–F on D) or the range of fifth sizes.

Temperament tables come from the engine's generated
`desktop/src/tuning/temperaments.json`; there is no second table in the UI.
The study stays silent and unsaved; Scala import appears only when connected.

Validation: unit tests for repeat intervals, finite collections, a fixed
reference under any root, fine offset and wolf spelling; browser tests for
split inheritance, reference-relative custom editing (mouse, keyboard and
touch), root rotation and all four arrangements; screenshots reviewed.
