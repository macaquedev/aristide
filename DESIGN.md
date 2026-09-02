# Aristide — Design Document

*An open-source virtual pipe organ. Named for Aristide Cavaillé-Coll.*

Aristide aims to (a) render existing sample sets **better than Hauptwerk** and far better
than GrandOrgue, and (b) be the first VPO built for **contemporary music**: microtonality,
per-pipe addressing, delays and live processing (Orgelpark-style), arbitrary many-to-many
MIDI/audio routing. Free forever, GPLv3.

## Core decisions (2026-08-08)

| Decision | Choice | Why |
|---|---|---|
| Language | Rust | RT-safe concurrency without GC, SIMD, modern tooling |
| Shape | Headless engine + clients | Console PCs, tablet remotes, future CLAP wrapper |
| GUI | Tauri 2 shell over a zero-build HTML/CSS/JS console (revised 2026-08-13; the egui GUI was removed) | One UI for the desktop shell and any browser; the server stays headless |
| License | GPLv3 | Nobody takes it proprietary; ecosystem norm |
| Formats | GO `.organ` + unencrypted Hauptwerk read **directly**; Aristide features live in **sidecar files** | Sound quality is engine-side; sidecars add superpowers to every existing free set with no conversion |
| Native standalone format | Deferred | Only needed for future multi-mic/spatial recordings |
| Effects | Internal RT-safe modular node graph | Taps at pipe/stop/division/output level; per-pipe delays |
| UI style | Modern-first; photoreal console skins supported | 2026 UI by default, keep the charm when sets ship artwork |
| Multi-organ | **Compose at load** (`aristide-formats::instrument`): composite TOML files and multi-set launches assemble N sources into one `Organ`; engine/console/UI stay single-instrument | One playable instrument from any sources is the organ-native abstraction (a console with three builders' ranks is one organ); cross-set couplers become plain routes; no organ tag threads through every API forever |

**Legal boundary:** encrypted Hauptwerk sample sets (HW5+-era commercial sets) are
off-limits — no decryption, ever. We read only the open GrandOrgue format and
unencrypted HW v1/v2-era packages, and we say so clearly in user-facing docs.

**Keyboard compass (locked):** a stop sounds only its own manual's accessible keys.
A manual 8′ speaks from the keyboard's bottom key to its top and nowhere else, and
that holds after coupling: an octave coupler that lands outside the compass sounds
nothing rather than reaching past it (unless its route opts into repitching — see
"Couplers are routes" below).

Sample sets routinely define ranks longer than the keyboard — GO's demo set gives
Montre 8′ 85 pipes starting twelve keys below middle-C-2, so its extension is there
for *other* stops to borrow (a pedal 16′, an extended unit rank). Those pipes are
loaded and addressable through whatever stop legitimately covers them; they are not
reachable from a manual stop whose compass ends above them. The loader therefore
clips each stop's key range to the keyboard (range starts at key 0, skipping the
sub-compass pipes via `first_pipe`) and caps it at `NumberOfAccessiblePipes` at the
top.

This is a deliberate divergence from GrandOrgue, which lets a sub-octave coupler
drive logical keys below the accessible range and speak the extension pipes. We
treat the keyboard's compass as the instrument's compass; borrowing is how a rank
gets played outside it.

**Couplers are routes (locked, 2026-08-18):** a coupler is a named, engageable
bundle of *routes*: `from manual + source key range → to manual + key shift`,
each route optionally `unison_off` (the source keys' own division is silenced
in the range, so the note *moves* instead of doubling) and `repitch` (below).
The classic couplers are single full-compass routes; GO's `UnisonOff` is a
route with no target; a fourths coupler that only speaks from tenor C, or a
16' that transposes the bottom octave down instead of doubling it, are the
same vocabulary with ranges. Users deploy their own in the sidecar
(`[[couplers.define]]` — manuals by name pattern, keys as MIDI numbers or
note names like `"C3"`), resolved like bindings: a definition naming what the
loaded organ hasn't got is reported and ignored, never fatal. Engaging or
releasing a coupler lands on held notes immediately, as an electric-action
console does (and as drawing a stop mid-hold always has): the console
re-derives what each held key should sound and diffs. GO's Bass/Melody picks
and cent-offset routes are deferred — the latter breaks pipe-speaks-once and
belongs with per-pipe addressing (M6).

**A pipe speaks once (locked, default; 2026-08-27):** the refcount identity of
a voice is the *physical* pipe — borrow chains followed to the pipe that
actually sounds — plus the pitch it is asked to sound, at cent resolution.
However many stops, unit-organ borrows, duplexes and couplers demand the same
pipe at the same pitch, they hold one voice between them, as the real action
would (a second voice would sum the identical recording coherently, +6 dB).
The same physical pipe *repitched* two ways is two virtual pipes and both
speak — an out-of-range C# and D both stood in for by top C still make a
second. The opt-out is per coupler route (`own_pipes = true` on a
`[[couplers.define.route]]`) and per stop (`own_pipes` on a `[[stop]]` pull or
a `[[division]]` pull's per-stop map): an own-pipes route or stop speaks an
independent virtual set of pipes and deliberately doubles — which also makes
a unison self-coupler a doubler instead of a no-op. Both toggles are in the
console editors and land live under held keys.

**Couplers never repitch (locked, default):** repitching fills in what the
*player's keyboard* can reach; a coupler is not a keyboard. A coupled note
sounds only a pipe the division it lands on actually has — a 16' coupler
running off the bottom of a rank, a coupler into a shorter compass, or a hole
in a rank all stay silent in the coupled copy, while the played key itself is
filled in as usual. Letting couplers invent pipes would change the instrument
rather than reach it. Sets or pieces that want the other behaviour set
`[couplers] repitch = true` in the sidecar (or POST `/api/couplers?repitch=1`
live), or a single route sets `repitch = true`. A repitching route is
explicitly asked to synthesize tone the instrument hasn't got, so it also
reaches past the destination's compass — that is the whole point of a 16'
route over an 8' rank's bottom octave; non-repitching couplers stay bounded
by the compass rule above.

**Whose keyboard (locked):** the compass is the *player's* keyboard, not the set's.
Each MIDI input carries the range it was measured at — a keyboard's MIDI popover
learns it from two key presses — and a manual answers to the union of its inputs' ranges,
defaulting to the set's own compass until something is measured. Notes outside are
silent, as above; keys *inside* it that the set has no pipe for are filled by
repitching the nearest pipe the rank does have, with no ceiling on the interval. A
rank range that reaches the set's own edge carries on past it; one that stopped
earlier stays stopped, because a half-compass stop is a musical decision. Holes
inside a rank (a sample that failed to load) are filled the same way — those are
defects, not decisions.

**Bindings (locked):** any input message — a MIDI note, controller or program
change, or a computer key — can be bound to any console action, as text
(`note:36` -> `stop:Montre 8'`). Bindings live per organ beside the input
assignments; a bound message does its job and does not also play. Actions
resolve their subject by name through the same matcher manual names use, so a
binding that names something the loaded organ hasn't got is reported and
ignored rather than dropped from the file. `State::run` is the only place an
action becomes an effect: a piston, a menu item and the HTTP API cannot mean
different things by "cancel". The text vocabulary is deliberate groundwork for
the scripting layer that will eventually write bindings (M6).

The computer keyboard is an input like any other under this scheme — a device
named `Computer keyboard`, assigned to a manual, with its own transpose and
bindings. Octave shift belongs to the *input*, not the manual: how wide a
keyboard is and where it currently sits are facts about the hardware. What a
key *means* depends on the manual it addresses: on a hand keyboard the two
letter rows are the usual DAW piano; on a microtonal manual all four rows
become a window onto the manual's own hex layout, read in the slanted
stagger the physical rows actually have (each row up sits half a key left,
no re-centering — `control::KEYBOARD_GRID`, `HexLayout::key_at_slanted`).
The cap up-right of another sounds +upright, so isomorphic shapes lie under
the fingers exactly as they lie on the board, and the legend redraws as the
slanted grid with each cap's key number and its Lumatone map colour.

## Why we will sound better (engine-side quality)

Fidelity comes from the renderer, not the file format. The plan, roughly in order of
audible impact:

1. **Phase-aligned release crossfades.** Select among multiple release samples by held
   duration; align phase at the splice point so releases never click or double. This is
   the single biggest realism win.
2. **High-quality resampling.** Windowed-sinc / polyphase interpolation for all pitch
   shifts (tuning, temperament, random detune) instead of linear interpolation.
3. **Wind supply model.** Blower + reservoir + windchest simulation so big chords sag
   pressure (pitch/amplitude dip and recovery) per division.
4. **Tremulant modeling** on untremmed samples (pitch/amplitude/spectral modulation),
   alongside sampled-tremulant playback.
5. **Per-pipe voicing**: user-adjustable level/EQ/detune/speech per pipe, stored in sidecars.
6. **64-bit float mix bus**, convolution reverb, per-note random variation, action/blower noises.

## Architecture

```
┌───────────────────────────── aristide-server (daemon, bin) ─────────────────────────────┐
│  device I/O: audio (cpal→PipeWire/JACK/WASAPI/CoreAudio), MIDI (midir; MIDI 2.0 later)  │
│  control plane: HTTP/JSON on localhost today (`--http-port`, 9669) — console, browser,  │
│  scripting; a richer IPC (unix socket / TCP, OSC, tablet remote) is M5                  │
│        ┌──────────────── aristide-engine (RT core, lib) ────────────────┐               │
│        │ lock-free command queue → voice allocator → voices (attacks    │               │
│        │ and loops in RAM, 16-bit; tails stream) → node graph           │               │
│        │ (delays, conv reverb, wind taps) → N-channel routing, SIMD mix │               │
│        └────────────────────────────┬────────────────────────────────────┘              │
│                 aristide-model (lib): organ model — divisions, stops, ranks, pipes,     │
│                 couplers, tuning/temperament (Scala), key mappings (MPE/MIDI2/Lumatone) │
│                 aristide-formats (lib): GO loader, HW(unenc) loader, sidecar read/write │
└─────────────────────────────────────────────────────────────────────────────────────────┘
                                   ▲ HTTP
                aristide-console (bin): Tauri console UI (HTTP to server)
```

- The **audio thread never allocates, locks, or touches disk**. Control → RT communication
  is lock-free SPSC queues. Attacks and sustain loops are RAM-resident (16-bit by default,
  see docs/progress/2026-08-26-memory-wall.md) — a held note never waits for a disk; the
  release tails behind them stream (2026-09-02), keeping a resident head of each tail
  (Hauptwerk's proven trick, inverted) and filling ring buffers from streamer threads. See
  docs/progress/2026-09-02-disk-streaming.md.
- Polyphony target: **≥2000 streaming voices** on a mid-range desktop; latency target
  sub-10 ms end-to-end at 48 kHz.
- The engine is a pure library (buffers in/out) — the server owns devices. This is what
  makes a later CLAP/VST3 wrapper nearly free.

## Contemporary-music layer

- **Tuning**: every stop/division retunable via Scala `.scl`/`.kbm`; per-pipe cent offsets;
  live tuning drift. Input side: plain MIDI, MPE, MIDI 2.0 per-note pitch, generalized
  keyboards (Lumatone) via a flexible key→pitch mapping layer (no 12-EDO assumptions
  anywhere in the model).
- **Per-pipe addressing**: any pipe individually triggerable/processable — delays, echoes,
  granular tricks as graph nodes tapped at pipe/stop/division level (Orgelpark Utopa as
  the reference point).
- **Routing**: many MIDI sources → any divisions; division/stop/pipe audio → arbitrary
  speaker groups on multichannel interfaces. Sidecar-stored per-set routing configs.

## Sidecar files

Human-readable (TOML) files next to a loaded sample set, never modifying it:
voicing, tuning/temperament, audio routing & speaker groups, per-pipe effects/delays,
input mappings, console-skin overrides. A set + its sidecars = a reproducible instrument.

## Multi-organ composition

Aristide is organ-agnostic all the way down, and "one loaded organ" is a
composition-time fact, not an architectural one. `aristide-formats::instrument`
is the one composition engine: it assembles any number of sources into a single
`Organ` — ids renumbered into one namespace, sample paths absolutized per
source, windchests/enclosures carried and renumbered, colliding console names
suffixed with their source. Everything downstream — bank, engine, console,
HTTP, UI — plays exactly one instrument and must never grow a "which organ"
concept.

An organ *is* a small TOML file (sidecar philosophy — sources never modified,
samples stay where they are): `[sources]` names sets by alias, `[[manual]]`
declares any number of manuals, `[[division]]`/`[[stop]]` pull whole divisions
or single stops onto any manual (loading only the ranks actually used, borrows
followed), `[[couplers.define]]` couples across all of it, and the sidecar
sections (tuning, wind, reverb…) apply instrument-wide. Stops anchor by pitch
when they move between manuals. A file that declares nothing — no manuals,
no pulls — is those organs whole, each manual and coupler exactly as its set
provides (locked default), so wrapping a GO/HW set is three lines, and
launching the server with several set paths builds the same implicit
composite. The moment a file declares any shape of its own, sources
contribute only what is pulled: adding a source to an organ being edited
offers its material without dumping the whole set onto the console. The composite file
also owns its rig's `[midi]` wiring (locked 2026-08-18): device/control
bindings load from it and interactively learned bindings are written back into
it (comment-preserving), while plain sets keep wiring in the user config.

Every organ IS a composite with an organ file (locked 2026-08-19): a GO/HW
package is only ever a *source*, never the organ itself. Loading a raw set
path adopts it — the library's organ file that already wraps the set is
loaded instead when one exists, else one is created under the config
`organs/` dir.

Adoption writes an inventory, not a pointer (locked 2026-08-20, revising
the above): the organ file *snapshots* the set's console — every manual
declared with its name and compass, every stop an explicit `[[stop]]` pull
(`manual`-filtered so same-named stops on different divisions stay distinct),
every coupler a `[[couplers.define]]` route bundle — plus the set's sidecar
sections verbatim and any wiring the user config already held. From then on
the file is the sole authority on the instrument's shape; the set is
consulted only for what the pulls read: ranks, pipes, samples and their
recording-level facts (loop points, pitches — facts about the sounds, not
opinions about the console). `layout = true` on the source keeps its
windchest/enclosure numbering whole so `[tremulant] chests` etc. keep
meaning what the set meant. Adoption is proven byte-equivalent to the
direct load by test — but a later set update no longer flows through, by
design: the organ cannot change shape behind the player's back, and every
console-level fact is a line the editor (and the player) can change or
delete. Renames, wiring and every per-organ edit have a durable home from
the first load. The only un-adopted loads are multi-set CLI launches (the
implicit combination, until saved) and the no-config-directory fallback.

**The set's own organ keeps the set's instrument (locked 2026-08-30,
revised 2026-09-01):** the adopted file carries `adopted = true`. The line
runs between the *instrument* and the *player's settings*. Anything that
changes what the set defines — its keyboards, stops, couplers, enclosures,
sources, voicing, a tuning of any scope below the whole instrument, the
tremulant's shape, the organ's name — is refused with 409 until the organ is
saved under a different name. Anything about how this player uses it — MIDI
wiring (and the learns and conflicts that end in a bind), the room, the
whole-instrument pitch, panel placement and knob order — lands in the set's
own file straight away, so loading the raw set again brings it back wired
and pitched as they left it; nobody should have to name a copy just to bind
their keyboards. (The first cut, 2026-08-30, refused everything; that made
the plain "load a set, wire it up" path a save-as detour.) The console
answers the refusal with a save-as dialog rather than an error; saving
(`/api/organ/save_as`) copies the file line for line beside the original
with the mark dropped, switches to the copy without a rebuild, and sends the
refused change again, so the player's gesture lands after all — on an organ
that is theirs. A sample set's organ therefore always loads with the set's
instrument, the player's *own* instrument is a named copy, and browsing to
the raw set again means the marked original, never a copy (the older
`layout = true` / bare-file signs of adoption only count when no marked
file is in reach). "Save as…" on the organ-name menu makes the same copy of
any organ that has a file.

The console edits the instrument live, and every edit lands in its file:
declared compasses (`[[manual]] low/high`), stops moved between manuals
(`[[move]]` — pitch-anchored, replayed after the pulls), couplers taken off the
console (`[couplers] drop` — hidden and disengaged, never deleted, so always
restorable), swell boxes of the file's own (`[[enclosure]]` — a name plus
member stops/manuals; enclosure is physical, so a box holds the ranks its
member stops actually sound, splitting a windchest shared with outsiders,
while borrowed pipes stand with their own rank and stay outside), the organ's
whole structure (manuals declared/renamed/reordered/removed, sources added,
stops pulled and unpulled — the pane's editor, each edit a line in the file
followed by a reload), each keyboard's declared kind
(`[[manual]] kind = manual/pedal/microtonal` — never deduced; "microtonal"
draws a Terpstra/Lumatone-style hex key field whose isomorphic layout is the
manual's `hex = { rows, cols, right, upright, anchor }` — the classic
two-step-vector parameterization, editable from the keyboard's context menu
with Bosanquet/Wicki–Hayden/harmonic-table presets derived against the
manual's own steps-per-octave; absent, a Bosanquet default is fitted to the
compass. The synth tremulant's shape is editable the same live way: right-click the
Tremblant knob → rate (Hz), depth (pitch cents — gain and timbre follow
pressure physically), spin-up, unevenness; engine first, then the file's
`[tremulant]` section (which, once declared, supersedes a set's own ODF
tremulants at load — that is what declaring one means). Wave tremulants
are recorded in their samples and refuse shaping. Every stop is editable in
place the same way: right-click its drawknob → its name (kept on the pull
that brought it in — a `[[stop]]` line's `rename`, or a `[[division]]`
pull's per-stop `rename` map; exact file references follow), its pitch and
gain (footage + cents + dB, the stop's own exact-name `[[voicing.adjust]]`
rule — footage re-anchors each key to the pipe really sounding there,
unit-organ style, with only the sub-semitone remainder bent and rank ends
filled by repitching; a mixture has no single footage and voices in cents
alone), and its source (the pull rewritten — a division stop leaves its
pull via `except` and gets a `[[stop]]` line of its own — label kept;
structural, a reload). Name and voicing land live, addressed by recorded
provenance rather than by guessable names. The drawknob's engraved footage
line comes from the pitch data, never from parsing the name — per stop it
can be hidden or replaced with custom text (`pitch_label` beside `rename`
on the pull lines; adoption keeps the labels the set's names implied only
insofar as the pipes agree). Couplers get the same treatment: right-click
a rocker → its name (a `[[couplers.define]]`'s own line, or the
`[couplers.rename]` map for one a source carries in — keyed by the
original, so drop entries and `coupler:` control bindings follow) and its
full routes (from/to/shift/scope/unison-off). Editing a carried coupler's
routes materializes it as this organ's own define, the original renamed
out of the way and dropped (restorable); route edits and the add menu's
new-coupler form are structural, renames land live. The coupler editor
speaks the coupler's own grammar — "Sounds [Swell] on [Great] at
[Sub-octave (16′)]" — never the wire's from/to. Jamb layout is the
player's too: stops drag-reorder within (or into) a division with an
insertion seam, kept as `[console.order]` (per manual, console stop
names, display-only — ids, voicing and combinations never move); and a
jamb's corner grip drags it wider, wrapping the knob rank into columns
(`w`/`h` on its `[console.layout]` entry — width is what wraps; height
follows content so nothing ever clips). Both live, the panel-placement
contract.
Layout edits are *live*, tuning-style — file line plus in-place
apply, never a reload — because the layout is a console fact throughout:
duplicate hexes share a key number, a bound Lumatone `.ltn` map's key
colours tint the hexes in the same extended-key numbering its notes land
in, and *pitch* still comes only from the tuning layer), and
per-manual tuning (`[[manual]] temperament/edo/reference_key/reference_hz/
transpose` — `edo` is divisions per octave, default 12 and written as absence;
away from 12 keys walk equal steps of 1200/edo cents from the reference key and
the temperament — twelve-class vocabulary — is dormant, so the UIs offer it
only at 12. The pitch anchor is a *pair*, the way Scala's `.kbm` anchors: one
piano key named in scientific pitch notation plus the Hz it sounds, default
`A4` = 440. "a′ = 415" is one choice of anchor; it presumes a tuning that has
an a′, which 15-EDO or Bohlen–Pierce does not, whereas "the key labelled C4
sounds 261.6 Hz" always means something. `a4_hz` is the older single-field
spelling and still reads) —
a 415 meantone Positif against a 440 equal Great is one instrument. Tuning is
physical: a coupled copy sounds the destination division's pipes, so it speaks
that division's temperament; a division's transpose moves only its own
keyboard. When several sets load ad hoc, the console offers its save-as popover
once — the combination has no file yet, and saving writes the composite file
that from then on owns the instrument.

**Home pitch is measured, never assumed (locked 2026-08-31):** no set is
taken to sit on the 12-EDO/A440 ladder. At load the engine measures every
looped pipe's fundamental (a coarse one-cycle autocorrelation scan ±600 cents
around where the set's own voicing says the recording should be, refined over
up to 24 cycles — the same number the release aligner tracks phase with), and
the bank fits the organ's *home tuning* from it: per-rank pitch anchor (a rank
comes from one set, and a composite may hold a 415 Positif beside a 440
Great), an instrument-wide a-referenced 12-class table from the octave-class
ranks (mutations are tuned pure to their unison and would smear the class they
land on), the named temperament the table matches within 1.75 ¢ RMS if any,
and the spread. Each pipe carries `home_cents` — how far it really sounds from
its nominal — with the fitted model standing in for pipes that could not
measure. `temperament = "original"` (the new default; GO's "Original
temperament" is the same idea) plays every pipe as recorded, and its reference
reads the organ's *own* pitch on the reference key (a 415 set says
"A4 = 415.3", not a 440 it never sounded); pulling that reference moves the
whole instrument as one, intervals and drift intact. Every other tuning is a
*target* — a table, a division count, a Scala scale — and bends each pipe from
its measured pitch, so "440 equal" on a 415 meantone set is exact per pipe,
Hauptwerk-style, not a guess applied on top of an unknown. Naming `original`
returns the reference to the organ's own; naming a target keeps the reference
it had (a temperament change never jumps the pitch a semitone on its own);
`reference_hz = home` releases a pulled reference. What a target does with
each pipe's own *drift* — the few cents every real pipe sits from where its
tuner meant it, left once pitch standard and temperament are accounted for —
is `pipes = "original" | "exact"` (default `original`): each pipe moved by
what the fitted model moves by, drift kept (the same instrument, retuned by a
tuner exactly as good as the first), or each pipe landed on the target from
its measured pitch (clinically in tune). Measurement also decides
key placement: a pipe more than 50 ¢ from where its rank's anchor plus the
class table puts it is a file at another key (a borrowed neighbour, a mis-keyed
sample), moved onto the model from its measured pitch; the `smpl`/ODF metadata
path survives only as the fallback for pipes that cannot be measured
(percussives, noises, loops too short). The unsaid `reference_hz` in a file
means "the organ's own" under `original` and 440-ladder under a target; the
console writes whatever is live.

**Tuning is a cascade of scopes (locked 2026-09-01):** the whole
instrument's `[tuning]` is the root — the organ's global preference, `original`
(*as recorded*) by default — and everything that joins the organ later, a set,
a stop, a rank, plays it until told otherwise, by absence: nothing is written
for a scope that merely follows. Four scopes hang off it. A *division* and a
*sample set* (`[sources.<alias>.tuning]`) each either follow the instrument or
carry a whole tuning of their own. A *stop* plays its own tuning if it has one;
else what its pin names (`[[tuning.stop]] follow = "division" | "source" |
"organ"`); else — automatically, the default — its division's own, its set's
own, the instrument's, in that order. Division and set are two axes that only
meet at the stop, so the order is a decision: a division's tuning is a
*performance* fact ("the Terpstra keyboard plays 31-EDO") and a keyboard
silently playing the wrong scale on some of its stops is the worse failure; a
set's tuning is a *material* fact, and the set that must not be retuned
whatever keyboard it lands on is what the stop's `source` pin is for. A *rank
within a stop* (a mixture's tierce, `rank = "…"` on the row) tunes apart from
the stop, within that stop only — the drawknob is the unit the player sees, so
a rank's tuning is keyed by the stop it is heard through. Inheritance is
all-or-nothing per scope, never per field: switching a scope to its own tuning
seeds it with what it resolved to, so the switch itself changes nothing
audible and the file never needs "inherit the temperament but override the
reference". Transposition is a keyboard's — instrument and division only.
Each set has a home of its own (the instrument's class table at the median
anchor of the set's ranks; a rank likewise at its own anchor), so *as
recorded* at set scope means the 415 the Positif was sampled at inside a 440
instrument, and a set-scoped reference pull moves that set as one. On the
console one popover serves every scope: its first row says what the scope
follows, the spec below it is live under *Own tuning* and shown dimmed and
disabled — the resolved values, with an "open →" to the governing scope —
while following. Disabled, not edit-to-detach: the common mistake is
right-clicking the nearest keyboard meaning to retune the organ, and a live
field would silently fork the division. A stop's row lives on its editor
(with one row per rank on a mixture), a set's on its Library row; keyboards
and drawknobs tuned apart wear a chip or a dot, and nothing shows for a scope
that just follows.

**Two scopes, two surfaces (locked 2026-08-28):** user preferences and organ
settings never share a surface. The Preferences dialog (Aristide menu, Ctrl+,)
is the *player's* — appearance today, per-machine audio settings tomorrow —
local to the installation and sending no organ command, ever. Every organ fact
is edited on the console and lands in the organ's file: panel-anchored facts by
right-click on their panel (a keyboard's MIDI input, compass, kind, tuning, hex
layout; a stop's or coupler's editor, each with a quick piston row), organ-wide
anchorless facts as popovers off the Organ menu (whole-instrument Tuning — also
the bar's tuning readout — Room & noises, the flat Bindings list). Wiring is
organ-scoped throughout: an unwired keyboard wears a "silent — no input" badge
that opens its MIDI popover; hidden couplers restore from the add menu. The
menu bar reads as the scopes read — Aristide = the app and the player, the
organ-name menu = this organ's file, Organ = the instrument. Instrument-wide
tuning, reverb wet and noises persist into the file (`[tuning]`, `[reverb]
wet`, `[noises]` — sliders live-apply while dragging and persist on release),
so an organ sounds tomorrow as it was left today.

Organs load at runtime, never implicitly (locked 2026-08-18): the server
starts organ-less and the console opens on a picker — the `[[library]]` in the
user config (every organ this machine has loaded, most recent first) plus a
file browser; CLI paths are just a pre-queued pick. The engine's bank is fixed
at construction (the RT path never swaps pointers), so a load builds the new
console and bank off-thread, then replaces engine and stream together on the
main thread; a failed load reports and leaves the running organ untouched.

## Milestones

- **M0** — repo, workspace, this document. ✅
- **M1 — first sound**: server opens audio+MIDI devices, sine-per-note through the RT
  command path. Proves device layer + lock-free plumbing. ✅ (QWERTY fallback landed
  2026-08-18 as the `Computer keyboard` input; hex manuals 2026-08-27)
- **M2 — organ model + GO loader**: parse `.organ` sets (test: Grabowski Friesach demo)
  into the model; validate against GrandOrgue's docs. ✅
- **M3 — sampled voices**: attack-cache + disk-streaming playback with loops and basic
  releases. First real organ sound. ✅ (RAM-resident; disk streaming moved to M4 and
  landed there 2026-09-02)
- **M4 — engine quality pass**: phase-aligned multi-releases, sinc resampling, voicing,
  tremulants, wind model. The "better than Hauptwerk" milestone; A/B against GO.
  Shipped: sinc resampling + phase-aligned releases (2026-08-08), wind supply model
  (2026-08-09), convolution reverb, enclosures (2026-08-12), multi-loop/multi-release
  and attack selection, sounding tremulants (2026-08-26; wind-valve physics
  2026-08-27), voicing trims + generals with a setter, 16-bit residency + load cache
  (2026-08-26), closed-box pressure rise + nested (multi-box) windchests
  (2026-09-02), stereo release alignment (2026-09-02), disk streaming of
  release tails (2026-09-02), a recorded A/B against GrandOrgue (2026-09-02,
  headless rig + analysis in `tools/ab/`, see
  docs/progress/2026-09-02-ab-grandorgue.md), the whole combination action —
  divisionals honouring GO's `DivisionalsStore*` flags, the stepper, an
  additive crescendo, and the console's piston rail, all stored by name in the
  organ file (2026-09-02), wave-trem switch on held notes (2026-09-02). Still
  open: pipe-scope voicing, a brightness/EQ leg and live voicing edits.
- **M5 — headless split + GUI**: IPC protocol, native GUI console, multi-window. The
  console shipped 2026-08-13 as a Tauri 2 shell over the web console (replacing the
  egui GUI) and edits the organ in place (see "The console edits the instrument"
  above). Still open: a control-plane protocol beyond the localhost HTTP API (tablet
  remote, OSC, scripting), multi-window.
- **M6 — contemporary layer**: Scala/MPE/MIDI2/Lumatone input, effects graph public,
  multichannel routing, per-pipe delays. ✅ core complete 2026-08-24 (opened ahead of
  M4/M5 by decision the same day): Scala per-division tuning with nearest-pipe
  re-anchoring; ramped SetVoiceRate; live tuning drift on held voices; MPE per-note
  pitch (per-input bend ranges); Lumatone `.ltn` input maps over u16 manual keys;
  output buses with delay inserts + sidecar `[routing]`/`[[voicing.delay]]` +
  N-channel output. See docs/progress/2026-08-24-m6-contemporary-layer.md. Named
  deferrals: MIDI 2.0 UMP parsing (device layer — the cents seam is ready), the full
  effect-node graph and per-single-pipe addressing (overlap M4), multi-device output
  and a full routing matrix, console editors for routing. 2026-09-01: tuning
  scopes — sets, stops, ranks within stops — see
  docs/progress/2026-09-01-tuning-scopes.md.
- **M7 — HW-unencrypted loader, CLAP wrapper, Windows/macOS CI.** 2026-09-02: the
  Hauptwerk reader shipped (`aristide-formats::hauptwerk`, format reference
  `docs/hw-odf-notes.md`, test set AVO Solignac); `load_set` dispatches on extension
  and everything downstream stays single-`Organ`. Deferred inside it: noise ranks,
  second-layer tremmed samples, temperament files, the wind-physics tables. See
  docs/progress/2026-09-02-hauptwerk-loader.md. Still open: the CLAP wrapper and
  Windows/macOS CI (the repo has no CI at all yet).

## Test rig

User's console sends MIDI over USB/DIN; one speaker (stereo MVP is fine). Test corpus:
GrandOrgue's bundled Friesach demo (`testsets/grandorgue-demo/`) and the free AVO
Solignac Hauptwerk set (`testsets/avo-solignac/`); see CLAUDE.md for how to fetch both.
