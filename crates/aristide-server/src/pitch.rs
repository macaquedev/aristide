//! Recording facts and authored playback relationships. No waveform/period analysis.
use crate::bank::DecodedInfo;
use aristide_model::units::{cents_between, equal_ladder_hz};
use aristide_model::{Pipe, SamplePitchMode};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PitchSource {
    Definition,
    File,
}

#[derive(Debug, Clone, Copy)]
pub struct RecordingPitch {
    pub hz: f64,
    pub source: PitchSource,
}

#[derive(Debug, Clone, Copy)]
pub struct PlaybackPitch {
    pub recording: Option<RecordingPitch>,
    /// Explicit transposition applied to the recording, excluding sample-rate conversion.
    pub transpose_cents: f64,
    /// Known sounding pitch relative to the pipe nominal, after transposition.
    pub sounding_cents: Option<f64>,
    /// Author offset retained under a user-selected target tuning.
    pub target_correction_cents: f64,
}

pub fn recording(
    pipe: &Pipe,
    info: DecodedInfo,
    declared_hz: Option<f64>,
) -> Option<RecordingPitch> {
    if let Some(hz) = declared_hz {
        return (hz.is_finite() && hz > 0.0).then_some(RecordingPitch {
            hz,
            source: PitchSource::Definition,
        });
    }
    let (key, fraction, source) = match (pipe.midi_key_number, pipe.midi_pitch_fraction_cents) {
        (Some(key), fraction) => (key, fraction.unwrap_or(0.0), PitchSource::Definition),
        (None, fraction) => (
            info.unity_note?,
            fraction.unwrap_or(info.unity_fraction_cents),
            if fraction.is_some() {
                PitchSource::Definition
            } else {
                PitchSource::File
            },
        ),
    };
    let hz = equal_ladder_hz(key as f64 + fraction / 100.0);
    (hz.is_finite() && hz > 0.0 && fraction.is_finite()).then_some(RecordingPitch { hz, source })
}

pub fn resolve(
    pipe: &Pipe,
    info: DecodedInfo,
    attack: &aristide_model::AttackSample,
) -> PlaybackPitch {
    let recording = recording(pipe, info, attack.recorded_pitch_hz);
    let attack_offset = attack.pitch_offset_cents;
    let authored = pipe.pitch_tuning_cents + attack_offset;
    let transpose_cents = match (pipe.sample_pitch_mode, recording) {
        (SamplePitchMode::Declared, Some(recorded)) => {
            cents_between(recorded.hz, pipe.nominal_frequency_hz) + authored
        }
        _ => authored,
    };
    let sounding_cents = recording
        .map(|recorded| cents_between(pipe.nominal_frequency_hz, recorded.hz) + transpose_cents);
    let target_correction_cents = if recording.is_none() {
        0.0
    } else {
        match pipe.sample_pitch_mode {
            SamplePitchMode::AsRecorded => pipe.pitch_correction_cents + attack_offset,
            SamplePitchMode::Declared => authored,
        }
    };
    PlaybackPitch {
        recording,
        transpose_cents,
        sounding_cents,
        target_correction_cents,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aristide_model::{AttackSample, PipeSource};

    fn pipe(mode: SamplePitchMode) -> Pipe {
        Pipe {
            nominal_frequency_hz: 440.0,
            pitch_tuning_cents: 25.0,
            pitch_correction_cents: -12.0,
            gain_db: 0.0,
            midi_key_number: None,
            midi_pitch_fraction_cents: None,
            accepts_retuning: true,
            sample_pitch_mode: mode,
            source: PipeSource::Silent,
        }
    }
    fn info() -> DecodedInfo {
        DecodedInfo {
            index: 0,
            sample_rate: 48000.0,
            percussive: false,
            unity_note: Some(57),
            unity_fraction_cents: 0.0,
        }
    }
    #[test]
    fn author_speed_and_declared_destination_are_distinct_contracts() {
        let attack = AttackSample {
            pitch_offset_cents: 7.0,
            ..Default::default()
        };
        let original = resolve(&pipe(SamplePitchMode::AsRecorded), info(), &attack);
        assert_eq!(original.transpose_cents, 32.0);
        assert_eq!(original.sounding_cents, Some(-1168.0));
        assert_eq!(original.target_correction_cents, -5.0);
        let target = resolve(&pipe(SamplePitchMode::Declared), info(), &attack);
        assert_eq!(target.transpose_cents, 1232.0);
        assert_eq!(target.sounding_cents, Some(32.0));
        assert_eq!(target.target_correction_cents, 32.0);
    }
    #[test]
    fn missing_or_invalid_pitch_never_invents_a_transposition() {
        for mode in [SamplePitchMode::AsRecorded, SamplePitchMode::Declared] {
            let unknown = DecodedInfo {
                unity_note: None,
                ..info()
            };
            let pitch = resolve(&pipe(mode), unknown, &AttackSample::default());
            assert_eq!(pitch.transpose_cents, 25.0);
            assert!(pitch.sounding_cents.is_none());
            for hz in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
                let attack = AttackSample {
                    recorded_pitch_hz: Some(hz),
                    ..Default::default()
                };
                assert!(resolve(&pipe(mode), info(), &attack).recording.is_none());
            }
        }
    }
    #[test]
    fn definition_overrides_file_and_keeps_each_attacks_pitch() {
        let mut pipe = pipe(SamplePitchMode::Declared);
        pipe.midi_key_number = Some(69);
        let file = DecodedInfo {
            unity_fraction_cents: 37.0,
            ..info()
        };
        assert_eq!(recording(&pipe, file, None).unwrap().hz, 440.0);
        let attack = AttackSample {
            recorded_pitch_hz: Some(415.0),
            ..Default::default()
        };
        let pitch = resolve(&pipe, file, &attack);
        assert_eq!(pitch.recording.unwrap().hz, 415.0);
        assert_eq!(pitch.recording.unwrap().source, PitchSource::Definition);
        assert!((pitch.transpose_cents - (cents_between(415.0, 440.0) + 25.0)).abs() < 1e-9);
    }
    #[test]
    fn large_transpositions_and_non_octave_pitches_are_explicit() {
        let mut pipe = pipe(SamplePitchMode::Declared);
        pipe.nominal_frequency_hz = 55.0 * 3.0 / 2.0;
        pipe.pitch_tuning_cents = 0.0;
        let attack = AttackSample {
            recorded_pitch_hz: Some(1760.0),
            ..Default::default()
        };
        let pitch = resolve(&pipe, info(), &attack);
        assert!(pitch.transpose_cents < -5000.0);
        assert!(pitch.sounding_cents.unwrap().abs() < 1e-9);
    }
}
