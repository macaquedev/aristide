# GrandOrgue sound engine: a critique

A close reading of GrandOrgue's audio path (shallow clone of
`GrandOrgue/grandorgue` master, August 2026, in `reference/grandorgue/` —
gitignored). File:line references are into that tree. The purpose is not
sport: every weakness here is either already addressed in Aristide or
drives a concrete design decision. GO's loader compatibility and two
decades of field testing deserve respect — its renderer does not.

## 1. The audio callback blocks on mutexes and condition variables

`GOSoundSystem::AudioCallback` (GOSoundSystem.cpp:217) — the code the
sound card interrupt ultimately runs — does all of the following, per
callback:

- takes `GOMutexLocker locker(device.mutex)` (:240);
- **waits on a condition variable in a loop**:
  `while (device.wait && device.waiting) device.condition.Wait();` (:242-243);
- signals other devices' condition variables under their mutexes (:258-262);
- wakes worker threads via `pThread->Wakeup()` → `m_Condition.Signal()`
  (GOSoundOrganEngine.cpp:452-455, GOSoundThread.cpp:66) — a syscall.

This is the canonical list of things a real-time audio callback must
never do. A GUI-priority thread holding `device.mutex` at the wrong
moment, or a scheduler hiccup around the condvar, stalls the callback
past its deadline: an xrun. This isn't theoretical — it's the
architecture behind GO's reputation for clicks under load. The worker
scheduler is mutex-based too (GOSoundScheduler.cpp:21,27,34,66,81),
and engine lifecycle shares a mutex with the GUI thread
(GOSoundOrganEngine.cpp:172,866-868).

**Aristide:** the RT thread never allocates, locks, or syscalls; control
traffic arrives via a lock-free SPSC queue (`rtrb`), sample data is
immutable behind an `Arc`. This invariant is stated at the top of
`aristide-engine` and upheld by construction.

## 2. Interpolation: 8 taps, a Lanczos window, and 1976-style tables

GO's "high quality" polyphase resampler (GOSoundResample.h:36-39):

- **8 taps** per phase (`POLYPHASE_POINTS = 8`);
- windowed with **Lanczos** (`apply_lanczos_window`,
  GOSoundResample.cpp:66-71) — a sinc window, whose stopband floor is
  around −40 dB and decays slowly. Kaiser/Blackman-Harris windows at the
  same tap count buy 30-50 dB more rejection; this was settled DSP
  knowledge decades ago;
- **8192 phase rows with no inter-row interpolation**
  (`r_coefs[resamplingPos.GetFraction()]`, GOSoundResample.h:270-289).
  Storing 8192×8 floats = **256 KiB** of coefficients (plus 64 KiB for
  the linear table) that are indexed in an effectively random order per
  output sample — a guaranteed cache-miss generator on every voice —
  to avoid a 2-FMA-per-tap row interpolation that would have made 256
  rows sufficient;
- the playback rate is **frozen at voice start**: `m_FractionIncrement`
  is computed once in `ResamplingPosition::Init`
  (GOSoundResample.cpp:34-40) and never modulated. Nothing in this
  design can do smooth per-sample pitch modulation — which is why GO
  has no wind-sag model and its tremulants cannot bend pitch (see §4,
  §5). The quantization itself (13-bit) is ~0.1 cent, fine; the
  *immutability* is the wall.

**Aristide:** 16-tap Kaiser β=9 kernels, 512 phases *with* inter-row
interpolation (32 KiB table, cache-resident), measured 90.6 dB SNR at
40 % Nyquist vs 17.1 dB for linear (`resample.rs` tests). `rate` is a
per-voice `f64` that any control- or engine-side modulator will be able
to vary per block — wind and tremulant models plug into it.

## 3. Release alignment: guessing phase from two samples

GO's release phase alignment (GOSoundReleaseAlignTable.h:19-21,
.cpp:114-236) works like this: bucket the last **two output samples**
into 32 amplitude levels × **2 derivative buckets** (i.e. slope sign,
essentially), and look up a release start position in a 64-cell table.
The table itself is built by scanning the first 1/20 s of the release
and recording the *first* sample that happens to land in each
(amplitude, slope) cell (first-hit, `if (!areCellsFilled…)`,
.cpp:174-177); unfilled cells are patched from their nearest neighbour
(.cpp:200-212).

The problems are structural:

- Instantaneous (amplitude, slope-sign) is **not a phase**. Any
  harmonic-rich waveform — i.e. every principal, reed, and mixture —
  passes through the same amplitude with the same slope sign several
  times per cycle. The lookup is ambiguous exactly where alignment
  matters most.
- The table is keyed on amplitudes scaled to the **release's own
  maximum** (.cpp:129-130), but queried with values from the **loop**,
  whose level differs from a decaying release's — a systematic bias.
- First-hit table filling isn't a best match; it's "whatever came
  first", and holes are filled by copying neighbours.

**Aristide:** we know the pipe's fundamental (the model carries
per-pipe pitch), so we track true phase — `(position − loop_start) /
period` — and build the offset table by **normalized cross-correlation
of actual waveform windows** at bank load (`bank.rs::align_release`).
Measured on the adversarial anti-phase release: 0.89 of held level
through the splice vs 0.17 naive (`aligned_release_splice_never_cancels`).
The same correlation machinery extends to separate release files
(pending, M4).

## 4. There is no wind model

Search the entire tree for a blower, reservoir, or pressure state:
there is none. `GOWindchest` (model/GOWindchest.h:26-61) is a routing
group holding enclosure pointers and tremulant IDs with a static
`GetVolume()`. Forty-seven stops drawn and a tutti chord costs exactly
the same "wind" as a single 8' flute — the single most characteristic
dynamic behaviour of the instrument GO simulates is absent.

**Aristide:** DESIGN.md commits to a blower + reservoir + windchest
simulation with per-division pressure that sags pitch and amplitude
under load. §2's mutable per-voice rate is the delivery mechanism; the
wind model is next on the M4 list.

## 5. Tremulants: amplitude wobble rendered as 16-bit shorts

GO's synthesized tremulant (GOSoundProviderSynthedTrem.cpp:17-21)
renders a modulation waveform through `inline short SynthTrem(double
amp, double angle)` — **16-bit integer** control data for a gain
wobble, applied per windchest. No pitch modulation, no spectral tilt:
a real tremulant varies wind pressure, which detunes and re-voices
every pipe on the chest, which is why sampled-trem sets exist at all
and why GO's synth trem convinces nobody.

**Aristide (planned, M4):** tremulant = periodic pressure disturbance
into the §4 wind model — pitch, amplitude, and brightness move
together, per pipe, in float.

## 6. Architectural drag

- The "engine" is not a library. Engine lifecycle is guarded by a
  mutex shared with the GUI thread (GOSoundOrganEngine.cpp:866-868);
  wxWidgets types (`wxString`, `wxLogError`) reach into the sound
  system (GOSoundSystem.cpp:227-229); there is no headless mode and no
  control-plane API. Every consumer of GO's renderer is the GO
  application.
- Legacy 16/24-bit sample storage with templated per-format access
  (`PtrSampleVector<SampleT…>`, GOSoundResample.h:143-188) spreads
  format branching through the hottest loops instead of normalizing
  once at load.
- Voice pool refills allocate under a mutex while running
  (GOSoundSamplerPool.cpp:41-45).

**Aristide:** engine = pure `buffers in → buffers out` library; server
owns devices; GUI is a separate process on an IPC socket (M5). Samples
normalize to f32 once, control-side.

## What GO gets right (and we should respect or steal)

- **ODF compatibility and leniency.** Twenty years of coping with
  broken real-world sets. Our loader's warn-don't-die policy is learned
  from them.
- **The load cache.** Hashed binary cache of decoded/analyzed sample
  data for fast set reloads — worth adopting once bank builds get big.
- **Duration-selected multi-releases** and `ReleaseTail` truncation are
  sensible mechanisms; ours will subsume them.
- **Memory pragmatism**: optional lossless compression of cached
  samples in RAM matters for 30 GB sets; our disk-streaming plan must
  meet the same constraint differently.
- It ships, on three platforms, with real users. Criticism is easy;
  the bar it sets — a complete, free, working VPO — is the bar we have
  to clear *while* sounding better.
