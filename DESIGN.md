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

**Legal boundary:** encrypted Hauptwerk sample sets (HW5+-era commercial sets) are
off-limits — no decryption, ever. We read only the open GrandOrgue format and
unencrypted HW v1/v2-era packages, and we say so clearly in user-facing docs.

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
                        aristide-gui (bin): native console UI
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

## Milestones

- **M0** — repo, workspace, this document. ✅
- **M1 — first sound**: server opens audio+MIDI devices, sine-per-note through the RT
  command path. Proves device layer + lock-free plumbing. ✅ (QWERTY fallback deferred;
  see docs/PROGRESS.md)
- **M2 — organ model + GO loader**: parse `.organ` sets (test: Grabowski Friesach demo)
  into the model; validate against GrandOrgue's docs.
- **M3 — sampled voices**: attack-cache + disk-streaming playback with loops and basic
  releases. First real organ sound.
- **M4 — engine quality pass**: phase-aligned multi-releases, sinc resampling, voicing,
  tremulants, wind model. The "better than Hauptwerk" milestone; A/B against GO.
- **M5 — headless split + GUI**: IPC protocol, native GUI console, multi-window.
- **M6 — contemporary layer**: Scala/MPE/MIDI2/Lumatone input, effects graph public,
  multichannel routing, per-pipe delays.
- **M7 — HW-unencrypted loader, CLAP wrapper, Windows/macOS CI.**

## Test rig

User's console sends MIDI over USB/DIN; one speaker (stereo MVP is fine). Free GO sets
(Grabowski, Palo) are the test corpus.
