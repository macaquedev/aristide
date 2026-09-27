# Console keyboards, taught once for every organ

Alex, 2026-09-27: "Use computer keyboard" did nothing when another manual
already had the computer keyboard, and the console settings were hacky. Redo
them as Hauptwerk does, as friendly as possible, and split console settings
from the rest of Settings.

The cause of the bug: binding a device that already played another manual
parked a keep-both / replace / cancel question on the server
(`Pending::Input`), and the new UI never showed it.

## Model (Hauptwerk's two layers)

- **Console keyboards** are the machine's: `[[console]]` in `midi.toml`, bottom
  manual first, each a name, `pedal`, and the usual device / channel / low /
  high / transpose fields. The computer keyboard is one device among the rest.
  A device and channel belong to one console keyboard; choosing them for
  another moves them there, so no question needs asking.
- **Each organ's keyboard map** (`[organs."…".keyboards]`, manual → keyboard
  name, `""` for none) is optional. An unmapped manual takes the console in
  order: the n-th hand manual plays from the n-th hand keyboard, the n-th pedal
  from the n-th pedalboard. A newly loaded organ therefore plays at once.
- **Migration**: none needed. A manual with inputs of its own in the old
  per-organ table keeps playing from exactly those and does not also take a
  console keyboard. They show in Organ as "Also …" with Remove. A composite
  organ file still owns its `[midi]` rows; the keyboard map stays in the user
  config because the file cannot know this player's console.
- Octave and transpose actions shift the console keyboard that plays the manual.

## API

`/api/console/add?pedal=`, `/set?keyboard=&name=&pedal=&device=&ch=&low=&high=`
(`range=organ` forgets a learned compass), `/remove?keyboard=`,
`/learn?keyboard=&pedal=&range=` (no keyboard stops), and
`/map?manual=&keyboard=<name>|none|auto`. The snapshot gains `console`
(keyboards with the manuals each `plays`, `heard`, `learning` with `repeat`)
and `keyboard` / `automatic` on each `midi.manuals` row.

Detection needs one press. A key from a keyboard the console already has is a
slip: the wait continues and names that keyboard. A letter pressed during
detection picks the computer keyboard. `range=1` waits for the lowest and then
the highest key.

## UI

Alex, same day: every virtual organ is different, so show the organ's own
manuals and discover them the Hauptwerk way.

- Settings has tabs: Console, Speakers, Appearance.
- Console lists the loaded organ's manuals. Each has Detect ("Press any key
  on the keyboard for Great"), Played from (the console keyboard, Automatic,
  or Nothing) and the device it resolves to. The organ's own old inputs show
  as extra lines with Remove.
- Detect all is a sheet that walks the organ's manuals in order, with Skip.
- A press on a keyboard the console lacks adds it (a pedalboard for a pedal
  division); a keyboard already playing another of this organ's manuals moves
  to the one pressed (`/api/console/learn?manual=`).
- Your keyboards, below, holds what was found: name, device, channel, range
  (Detect lowest and highest, or the organ's), Remove. The next organ plays
  from them in order until it is detected itself.
- The Organ tab no longer has Played from; this is its one home.

## Validation

`cargo test -p aristide-server` (console order, detection, slip, move, map,
rename, removal, legacy precedence, config round trip);
`ARISTIDE_LIVE=1 bunx playwright test console-live` against a server with its
own `XDG_CONFIG_HOME` drives per-manual detection with the computer keyboard
and Detect all, and holds a note throughout.

## Gaps

- The first-run "Learn the console" step does not exist yet; it should reuse
  Detect all.
- Pistons, swell pedals and toe studs (`/api/control/*`) have no new UI.
  They stay per organ.
- Detection with real MIDI hardware has only been exercised through unit
  tests here.
