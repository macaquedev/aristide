# Recording pitch and pipe playback contracts

This replaces the waveform-driven bank tuning and the subsequent Solignac
harmonic-disambiguation patch. Those implementations confused a period suitable
for release alignment with the recording's pitch. Shared files could be analysed
with a lower rank's pitch hint, changing the fitted rank anchor and causing
otherwise correct notes to be transposed by an octave.

## Data and decisions

`pitch.rs` resolves three separate facts on the control thread:

1. **Recording pitch** in Hz, with provenance: an explicit per-attack declaration,
   a pipe-level ODF override (GrandOrgue), or embedded WAV/WavPack pitch metadata.
2. **Intended pipe pitch** from the imported model, including its harmonic ladder.
3. **Authored playback transposition**, resolved from an explicit source mode.

`SamplePitchMode::AsRecorded` preserves `PitchTuning` plus the attack offset.
`SamplePitchMode::Declared` computes the recording-to-pipe interval and adds
those author offsets. The sample-rate conversion is applied separately when
building a voice. Large intervals are not discarded by an arbitrary 1800-cent
ceiling. Missing or invalid declarations keep authored speed and produce an
unknown-pitch diagnostic; they never trigger fundamental detection.

The GrandOrgue adapter defaults to AsRecorded, following its original-based
playback semantics. Choosing a target tuning subtracts the declared sounding
pitch and applies `PitchCorrection` in place of `PitchTuning`. `AcceptsRetuning=N`
disables per-key temperament/scale changes, while the recording-to-pipe mapping
and reference remain applicable. See [the GO source notes](../go-odf-notes.md)
for the underlying source-code references.

The Hauptwerk adapter explicitly requests Declared playback onto its imported
pipe pitch, including the existing base-pitch and layer offsets. Each attack's
method-3 or method-4 declaration is retained in Hz, rather than copied from the
first attack to every variant. Otherwise the existing adapter's file-metadata
fallback applies. This is the supported adapter's playback contract, not a claim
that every Hauptwerk runtime pitch mode is emulated. In particular, the general
`EnablePlayingAtOriginalOrganPitch` capability flag is not treated as a request
to bypass the recording-to-pipe relationship.

Consequently imported Solignac uses its defined destination pitch (base-pitch 0
maps to A440), instead of the previous heuristic's inferred approximately A419
home. GrandOrgue original playback retains the author's offsets. This is an
intentional behavioral correction, not a new temperament inferred from audio.

## Invariants

- Release-period analysis has no input into `PlaybackPitch`, voice rates or the
  descriptive tuning fit. It remains available solely for audio splice alignment.
- The descriptive fit may describe drift, and a player-selected target can retain
  that drift, but the fit never changes source playback transpositions.
- Reordering ranks does not change a shared recording's playback relationships.
  The fallback release-analysis hint is deterministic; declared recording pitch
  takes priority for alignment. Cold/warm caches and residency precision do not
  decide pitch.
- Alternative attack recordings keep their own pitch and author offsets.
  Selection, held tremulant switches and subsequent live retuning use the same
  relationship, including file sample-rate differences.
- The audio thread gets precomputed rates. This change adds no audio-thread I/O,
  locks or allocations, and does not edit source sample files or organ definitions.

## Compatibility and validation

New serialized fields have defaults, so existing native instruments and sidecars
continue to load. The decoded-attack cache signature changes once because the
release-alignment hint policy changed; caches rebuild automatically. The existing
snapshot field `home.measured` remains for API compatibility, but now counts
usable declared playback pitches. The UI calls this “Instrument pitch” and labels
unavailable metadata as unknown.

Regressions cover explicit vs authored playback, metadata precedence and invalid
values, large/non-octave intervals, each attack's declaration, reversed rank order
with cold/warm caches at 16- and 32-bit residency, author target corrections,
held-note attack switching, and actual A415/A440 rendered audio. Both downloaded
Solignac editions check all pipe-rate equations, plus every Positif Flute and Echo
Cornet key through the console under original and equal tuning. Existing GO
rendering and console tests remain in the workspace suite.

The resolver guarantees deterministic handling of the declared facts. It cannot
prove that a producer's plausible pitch tag matches the audio. Such tags must be
corrected in source-independent metadata overrides; guessing from a harmonic-rich
waveform is deliberately not a fallback. No universal sample-set compatibility
claim is implied by passing the fixtures.

Validation on this checkout: 411 Rust functional tests and 31 JavaScript tests
passed. The Solignac regression additionally renders the Positif Flute 4 at played
MIDI 70 and 71 through the engine and checks the sounding B-flat5/B5 pitches
within 25 cents, rejecting the reported octave drop. Both notes pass.
Fourteen existing tests remain intentionally ignored; two pre-existing real-time
performance-budget tests were excluded after failing on the unchanged baseline
on this host. These results establish functional coverage, not a new real-time
performance guarantee.

The release server build and native console check pass. Server Clippy completes
with existing warnings in unrelated audio, noise matching and enum layout code.
