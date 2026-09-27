# As recorded plays the recorded pitch

Alex, 2026-09-27: "As recorded should be the pitch of the pipes as they are
recorded, not retuning everything to equal and 440." Hauptwerk organs loaded
through the declared pitch contract (2026-09-06) were transposed pipe by pipe
onto their written pitch, so "As recorded" reported a′ = 440.00 Hz, equal
temperament, 0 ¢ spread, and sounded the same as Equal.

## Loader

`bank::keep_declared_recordings` runs after staging and before the home fit.
For every `SamplePitchMode::Declared` pipe with a recording pitch it takes the
recording-to-pipe interval, finds the set's own pitch (the middle interval
over all declared pipes, folded to within half an octave of the ladder), and
keeps each recording's offset from its pipe except the whole semitones that
put it under its note. A 416 Hz organ therefore plays at 416 Hz in its own
temperament; a sample reused a semitone or an octave away still sounds its
pipe's note. Alternative attacks follow their pipe's primary recording, as
before. Target temperaments subtract the kept offset (`home_cents`) and land
exactly, through the existing GrandOrgue as-recorded path.

Solignac (both editions) now fits a′ = 416.05 Hz, matching the pitch earlier
Aristide builds wrote into its organ files; the 419 Hz in the Hauptwerk notes
was not what the fit finds and is corrected.

Limit: a composite organ built from Hauptwerk sets at pitches a semitone or
more apart shares one set pitch, so the minority set is moved by whole
semitones toward the majority's. Ranks do not record their source set yet.
The author's offsets (`PitchLvl_DetuningPercentSemitones`, base pitch folded
into `pitch_tuning_cents`) still apply, as they do for GrandOrgue.

## Tuning panel

`GET /api/tuning` gives every scope `recorded_hz`: what the recording sounds
on that scope's reference key. The Reference card's four pitch presets are
replaced by **As recorded (n)** and **440**; n follows the reference key.
Changing the reference key now keeps the organ where it is, giving the new key
the pitch it already sounds (it used to carry the old Hz to the new key, which
the engine then clamped).

## Validation

`cargo test --workspace`; the Solignac fixture test checks both editions for
whole-semitone placements, every declared pipe within 50 ¢ of the set pitch
and a′ ≈ 416 Hz; the baroque render test plays a declared 415 Hz recording
at 415 Hz as recorded and 440 Hz under Equal. Playwright tuning and study
suites pass, including the live test against the demo organ. Real-time tests
fail only when run in parallel with the rest of the suite on this loaded
machine and pass on their own. Needs Alex's ear on Solignac.

Migration: none. Existing organ files that name `reference_hz = 440` under
`original` now play Solignac's recorded temperament pulled to 440; choose
As recorded to hear it at its own pitch.
