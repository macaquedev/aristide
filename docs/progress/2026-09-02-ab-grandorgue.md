# 2026-09-02 — a recorded A/B against GrandOrgue (M4 closes)

M4's ledger has carried "a recorded A/B against GrandOrgue" as the last
open item since the milestone opened. This ships the rig
(`tools/ab/`), the actual recordings, and the analysis — headlessly, on
this box, with no audio hardware, which turned out to be most of the
work.

## The passage

`tools/ab/passage.py` is the one source of truth for both halves: a
plein jeu chord progression (First Manual: Montre 8', Prestant 4',
Plein jeu III), a Second-Manual Trompette 8' solo run — staccato then
legato, the release-splice probe — and a 12-second held chord (First
Manual chorus + Pedal Contre- basse 16'/Sous- basse 16') for wind sag.
36.0s of events, ~40.5s rendered end to end. Same registration, same
key/velocity/timing data feeds GO's MIDI Player (`make_midi.py`) and
Aristide's HTTP console API (`drive_aristide.py`) — see `tools/ab/
README.md` for the exact commands and every GUI click.

Both on `testsets/grandorgue-demo/demo.organ` (GrandOrgue's bundled
Friesach demo), GrandOrgue 3.13.1-1build2 (Ubuntu 24.04 apt), Aristide
commit `0f54d3f`.

## Two headless-audio findings, before any of this worked

Neither engine can simply run against an ALSA `null` device the way the
existing e2e rig runs `aristide-server` for UI testing (see the
`console-e2e-repro-rig` memory) — that rig only needs *a* device to open
successfully, never listens to its output, and doesn't record. Recording
needs the device to actually **pace** playback to real time, and `null`
doesn't: it accepts writes instantly, so the render loop free-runs at
whatever the CPU allows.

- **GrandOrgue**, 64 real seconds against `null`: a **2.7 GB** WAV whose
  header claims **7,671 nominal seconds** (128 minutes) — roughly 120x
  real time. The RMS-vs-nominal-second probe in the fix commit's history
  shows why: GO's own console (MIDI Player highlighting, drawstop
  clicks) stays wall-clock-paced via a separate timer, but the audio
  thread free-runs, so long stretches of "held note, nothing changed"
  render into a hugely inflated nominal-sample count for the same real
  seconds.
- **`aristide-server`**, 5 real seconds against `null`: a **2.6 GB** WAV
  claiming **~14,710 seconds** (245 minutes) — cpal's ALSA backend hits
  the identical free-run problem, worse.

The fix for both: JACK's `dummy` backend paces itself with a real sleep
between periods, so route each engine through a running `jackd -d dummy`
instead of `null`. GO has a native JACK output ("Jack: Native Output" in
Audio/Midi → Organ settings → Audio); `aristide-server` doesn't build
cpal with the `jack` feature, so it goes through the ALSA→JACK bridge
plugin (`libasound2-plugins`' `pcm_jack`) instead — plain ALSA from
`aristide-server`'s point of view, routed into the same `jackd` graph.
Full commands in `tools/ab/README.md`.

A third path, ALSA loopback (`snd-aloop`, a real ring-buffer device, not
a discard sink), also paces correctly but threw intermittent
`alsa::poll() returned POLLERR` mid-recording on this box — flaky enough
to drop in favor of the JACK-bridge route.

One more trap: `aristide-server` only finalizes the WAV header (and
flushes the recorder thread) on `SIGINT`; a plain `kill` (`SIGTERM`)
leaves the `data` chunk's declared size at 0 even though the audio is on
disk. `analyze.py`'s reader falls back to EOF when it sees that, but
`kill -INT` avoids needing the fallback.

## Recordings

`/home/macaque/aristide-ab/` (outside the repo, never committed):

| file | size | duration | format |
|---|---|---|---|
| `go-take.wav` | 23,420,972 bytes (22.3 MiB) | 66.39s | 44100 Hz stereo IEEE float32, GO's native JACK client |
| `aristide-take.wav` | 7,143,424 bytes (6.8 MiB) | 40.50s | 44100 Hz stereo PCM16, ALSA→JACK bridge |

GO's take is longer because GO's Audio Recorder was started, then
stopped by hand a few seconds after the MIDI Player visibly finished
(no programmatic "done" signal from the panel); Aristide's driver script
knows exactly when its last note-off fired. Both were spot-checked with
a sliding-window RMS probe against the passage's own section timeline
(quiet through 0–2s startup, the plein jeu chorus, a dip into the
Trompette solo, then the loudest stretch of the whole take during the
Section-C held chord, then silence) — both land where designed; see the
commit history for the exact probe.

## Analysis

```
uv run tools/ab/analyze.py /home/macaque/aristide-ab/go-take.wav /home/macaque/aristide-ab/aristide-take.wav
```

| metric | go-take.wav | aristide-take.wav |
|---|---|---|
| Duration (s) | 66.39 | 40.50 |
| Loudness-match gain (dB) | +11.24 | +11.19 |
| Peak (dBFS, matched) | -2.99 | -1.03 |
| RMS (dBFS, matched) | -21.77 | -19.04 |
| Spectral centroid mean (Hz) | 1053 | 1391 |
| Spectral centroid std (Hz) | 658 | 443 |
| Spectral centroid p10–p90 (Hz) | 399–1592 | 859–2022 |
| Noise floor (dBFS, matched) | -119.52 | -73.62 |
| Discontinuity spikes, ch0 (>10x local level) | 388 (5.84/s) | 2 (0.05/s) |
| Discontinuity spikes, ch1 | 226 (3.40/s) | 0 (0.00/s) |

Both takes get loudness-matched to the same broadband RMS target before
any of these numbers are compared (GO's own default master volume is
-15 dB, Aristide's `DEFAULT_MASTER_GAIN` is 0.178 ≈ -15.0 dB — a
coincidence worth noting, not something the analysis relies on; it
matches on measured RMS, not on either engine's nominal setting).

**Discontinuity spikes** — the standout number. Aristide's take is
essentially clean (2 spikes on one channel, zero on the other, out of
1.79M samples); GO's has roughly two orders of magnitude more per
second. This is the metric M4 was chased for: phase-aligned multi-
release splicing (shipped 2026-08-08) is exactly the claim this number
is checking. It should not be over-read from one take, though — see
caveats below.

**Noise floor** — Aristide's is ~46 dB higher (worse) than GO's. Some
of this is very likely the recording *path*, not the engine: GO's take
is 32-bit float via a native JACK client; Aristide's is 16-bit PCM
through the ALSA→JACK bridge plugin, an extra software layer neither
engine's normal (non-headless) path goes through. 16-bit quantization
noise alone has a theoretical floor around -96 dBFS; -73.6 is worse than
that, so something in the bridge or in genuine engine noise (the wind
model's continuous pressure-sag process runs even on a held chord's
sustain, unlike a pure sample-loop engine) is contributing. Not
resolved here — flagged for a follow-up with a same-bit-depth, same-
backend re-take.

**Spectral centroid** — Aristide's passage reads brighter (centroid
higher, narrower spread). Plausible (sinc resampling and phase-aligned
releases preserve high-frequency content GO's plain interpolation and
splice discontinuities can smear or duplicate-cancel) but, like the
above, not isolated from the recording-path difference here.

## Honest caveats

- **Not the same recording path.** GO: 32-bit float, GO's own native
  JACK client. Aristide: 16-bit PCM (this set's sidecar default —
  `[samples] bits = 16` is the documented default in
  `demo.organ.aristide.toml`), through the ALSA→JACK bridge plugin
  because `aristide-server` has no built-in JACK backend. The
  discontinuity-count and noise-floor gaps are large enough that they're
  very unlikely to be *entirely* explained by this, but a same-format
  re-take (`[samples] bits = 32` in the sidecar, and building
  `aristide-server` with cpal's `jack` feature) would separate "engine
  quality" from "recording path" cleanly — named as follow-up, not done
  here.
- **GO's recorder start/stop was manual**, so its take runs ~26s longer
  than Aristide's (trailing near-silence after the passage ends,
  confirmed by the RMS probe — this doesn't affect the loudness-matched
  peak/RMS/centroid numbers, which are computed over the whole file
  including that silence, diluting them very slightly toward silence,
  but does inflate GO's spike *count*; the reported *rate* (spikes/s)
  corrects for it).
- **Neither engine's headless render path is its normal path.** GO
  normally talks to a real PortAudio/ALSA device; here it talks to a
  JACK dummy backend. Aristide normally talks to ALSA directly; here
  that ALSA call is bridged into the same JACK graph. Both were
  necessary to get *any* correctly-paced recording at all on a box with
  no audio hardware — see the desktop fallback below for a same-path
  re-take.
- **One take each**, not a statistical sample. The passage is
  deterministic (same MIDI/HTTP event list every run), so a re-run
  should reproduce closely, but only one pass was recorded per engine
  here.
- **GO's own signal path** (resampler quality, its convolution reverb —
  not engaged here since neither engine has reverb on for this
  registration, its master gain handling) wasn't independently
  characterized beyond what the analysis measures; the gap-analysis doc
  (`docs/gap-analysis-go-hw.md`) is the place for a feature-by-feature
  writeup, not this note.

## Desktop re-take (real hardware, no bridges)

The rig works unchanged with real audio devices — `tools/ab/README.md`'s
GO section and the plain (non-`--record`-to-null) `aristide-server`
invocation don't need JACK at all when a real device is present. Given
the caveats above, the user re-running this on their desktop rig (a
normal ALSA/PortAudio device on both sides, same bit depth) would settle
whether the discontinuity and noise-floor gaps are recording-path
artifacts or real. What to listen for either way: Section B's Trompette
8' solo (release clicks would be audible on the staccato repeats) and
the Section-C held chord's release (a splice click at the very end, all
pipes at once).

## Deferred

- Same-bit-depth (32-bit), same-backend (native device on both, no
  bridge) re-take to isolate engine quality from recording path.
- A statistical pass (several takes, or a longer/more varied passage) —
  this note reports one recording per engine.
- GO's reverb/resampler settings weren't varied or characterized here.
