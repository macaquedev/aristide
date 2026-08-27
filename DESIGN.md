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
| GUI | Native Rust (wgpu-based; iced/egui TBD) | Fast, light, no webview |
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
Each MIDI input carries the range it was measured at — Preferences → MIDI learns it
from two key presses — and a manual answers to the union of its inputs' ranges,
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
keyboard is and where it currently sits are facts about the hardware.

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
│  control plane: IPC (unix socket / TCP) — GUI, tablet remote, OSC, scripting            │
│        ┌──────────────── aristide-engine (RT core, lib) ────────────────┐               │
│        │ lock-free command queue → voice allocator → streaming voices    │               │
│        │ (RAM attack cache + disk streamer) → node graph (delays, conv   │               │
│        │ reverb, wind model taps) → N-channel output routing, SIMD mix   │               │
│        └────────────────────────────┬────────────────────────────────────┘              │
│                 aristide-model (lib): organ model — divisions, stops, ranks, pipes,     │
│                 couplers, tuning/temperament (Scala), key mappings (MPE/MIDI2/Lumatone) │
│                 aristide-formats (lib): GO loader, HW(unenc) loader, sidecar read/write │
└─────────────────────────────────────────────────────────────────────────────────────────┘
                                   ▲ IPC
                aristide-console (bin): Tauri console UI (HTTP to server)
```

- The **audio thread never allocates, locks, or touches disk**. Control → RT communication
  is lock-free SPSC queues; disk streaming happens on dedicated streamer threads filling
  ring buffers; sample attacks are pre-cached in RAM (Hauptwerk's proven trick).
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
compass. Layout edits are *live*, tuning-style — file line plus in-place
apply, never a reload — because the layout is a console fact throughout:
duplicate hexes share a key number, a bound Lumatone `.ltn` map's key
colours tint the hexes in the same extended-key numbering its notes land
in, and *pitch* still comes only from the tuning layer), and
per-manual tuning (`[[manual]] temperament/a4_hz/transpose`) —
a 415 meantone Positif against a 440 equal Great is one instrument. Tuning is
physical: a coupled copy sounds the destination division's pipes, so it speaks
that division's temperament; a division's transpose moves only its own
keyboard. When several sets load ad hoc, the console opens its Organ setup to
ask how they go together, and saving writes the composite file that from then
on owns the instrument.

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
  command path. Proves device layer + lock-free plumbing. ✅ (QWERTY fallback deferred;
  see docs/PROGRESS.md)
- **M2 — organ model + GO loader**: parse `.organ` sets (test: Grabowski Friesach demo)
  into the model; validate against GrandOrgue's docs. ✅
- **M3 — sampled voices**: attack-cache + disk-streaming playback with loops and basic
  releases. First real organ sound. ✅ code-complete (RAM-resident; disk streaming
  moved to M4 with the rest of the engine quality pass — see docs/PROGRESS.md)
- **M4 — engine quality pass**: phase-aligned multi-releases, sinc resampling, voicing,
  tremulants, wind model. The "better than Hauptwerk" milestone; A/B against GO.
- **M5 — headless split + GUI**: IPC protocol, native GUI console, multi-window.
- **M6 — contemporary layer**: Scala/MPE/MIDI2/Lumatone input, effects graph public,
  multichannel routing, per-pipe delays. ✅ core complete 2026-08-24 (opened ahead of
  M4/M5 by decision the same day): Scala per-division tuning with nearest-pipe
  re-anchoring; ramped SetVoiceRate; live tuning drift on held voices; MPE per-note
  pitch (per-input bend ranges); Lumatone `.ltn` input maps over u16 manual keys;
  output buses with delay inserts + sidecar `[routing]`/`[[voicing.delay]]` +
  N-channel output. See docs/progress/2026-08-24-m6-contemporary-layer.md. Named
  deferrals: MIDI 2.0 UMP parsing (device layer — the cents seam is ready), the full
  effect-node graph and per-single-pipe addressing (overlap M4), multi-device output
  and a full routing matrix, console editors for routing.
- **M7 — HW-unencrypted loader, CLAP wrapper, Windows/macOS CI.**

## Test rig

User's console sends MIDI over USB/DIN; one speaker (stereo MVP is fine). Free GO sets
(Grabowski, Palo) are the test corpus.
