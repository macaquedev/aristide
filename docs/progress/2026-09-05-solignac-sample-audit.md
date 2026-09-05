# Solignac sample and playback audit

The local `testsets/avo-solignac` download contains 1,794 WAV files
(2,123,935,208 bytes). Both supplied organ definitions reference 1,739 distinct
sample paths; all exist. A RIFF/chunk scan found no truncated chunks or invalid
loop boundaries. Decoding every file through Aristide's own WAV reader produced
353,918,784 frames with no decode errors, non-finite audio, invalid loops, or
entirely silent files. Both the original and extended instruments loaded in the
server. These checks establish that the local download is readable; they do not
verify the user's audio device or rule out every possible playback problem.

## Confirmed playback fault

The bank loader treated the engine's release-alignment period as an authoritative
fundamental. Compound stops contain multiple sounding pipes, so their strongest
period can represent another partial or a beat. This polluted both individual
pitch corrections and the rank's fitted tuning anchor.

Before the fix, 108 of the original instrument's 407 sampled pipes received a
speed correction exceeding one cent in original-pitch mode. Some corrections are
legitimate borrowed or neighbouring recordings, but the following are not:

| Recording | Before: automatic shift | After |
| --- | ---: | --- |
| Cornet `068-g#.wav` | +819.8 cents | Recorded speed |
| Cornet `064-e.wav` | +498.8 cents | Recorded speed |
| Plein-Jeu `077-f.wav` | −704.1 cents | Recorded speed |

A separate fallback pulled recordings without a stable period toward nominal
A440 instead of the fitted organ pitch. The supplier describes this set as
approximately A419, and its embedded pitch declarations agree. That fallback
introduced near-semitone steps between neighbouring notes.

The loader now reconciles small-integer period ambiguities against trusted pitch
metadata. Repeated ambiguities across a rank cause its declarations to be used
consistently when estimating its tuning. The evidence threshold considers only
recordings with both a period and a declaration. This is a heuristic for compound
recordings, not a new sample-format field. Existing guards against editor-default
metadata remain in place. Unmeasured recordings are compared with the organ's
fitted pitch before any correction is applied.

Original WAVs and definitions are unchanged. Release-alignment measurements in
the audio engine are unchanged. Voice rates are rebuilt when an instrument loads,
so deleting the sample cache is unnecessary; restart the updated server and
reload the instrument.

## Reproduction and regression checks

Decode the entire local sample tree without modifying it:

```sh
cargo run --release -p aristide-formats --example audit_samples -- testsets/avo-solignac
```

`bank::compound_pitch_tests` covers harmonic ambiguity, preservation of a
neighbouring-key mismatch, and an unmeasured A419 recording. Its optional downloaded
fixture regression loads both Solignac definitions, checks every sampled pipe,
and rejects the accidental multi-semitone shifts. The fixture test skips when
the download is absent.

Two apparent missing-note cases are intentional in the supplied original organ:
the Cornet starts at MIDI 61 (C-sharp), and the pedal relies on the manual-to-pedal
coupler. These are documented in the supplier's included HTML disposition.

The release workspace run passed 404 tests, with two timing-budget failures and
14 intentional ignores. Running the timing tests serially still exceeded their
headroom budgets. An isolated copy of unchanged commit `6067af4`, using the same
GrandOrgue fixture, also failed both: coupled tutti used 72.2% of real time
(budget <70%), and spam stress used 64.0% (budget <50%). The changed build measured
75.6% and 65.7%, respectively. These host performance failures predate this fix;
audio-device playback has not been verified here.

The final serial functional run passed all 404 tests plus documentation tests,
with the same 14 intentional ignores and only the two separately investigated
timing tests excluded. No test assertions or performance thresholds were relaxed.
