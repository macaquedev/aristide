//! The built-in additive test tone (no-sample-set mode).

pub(crate) const TONE_ATTACK_SECONDS: f32 = 0.006;
pub(crate) const TONE_RELEASE_SECONDS: f32 = 0.09;
pub(crate) const TONE_GAIN: f32 = 0.08;

/// Principal-chorus-flavoured partials: 8', 4', 2 2/3', 2', 1 1/3'.
const HARMONICS: [(f32, f32); 5] = [
    (1.0, 0.50),
    (2.0, 0.24),
    (3.0, 0.09),
    (4.0, 0.12),
    (6.0, 0.04),
];

#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) enum ToneStage {
    #[default]
    Idle,
    Attack,
    Sustain,
    Release,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ToneVoice {
    pub(crate) key: u8,
    pub(crate) phase: f32,
    pub(crate) phase_increment: f32,
    pub(crate) envelope: f32,
    pub(crate) stage: ToneStage,
}

impl ToneVoice {
    #[inline]
    pub(crate) fn tick(&mut self, attack_step: f32, release_step: f32) -> f32 {
        match self.stage {
            ToneStage::Idle => return 0.0,
            ToneStage::Attack => {
                self.envelope += attack_step;
                if self.envelope >= 1.0 {
                    self.envelope = 1.0;
                    self.stage = ToneStage::Sustain;
                }
            }
            ToneStage::Sustain => {}
            ToneStage::Release => {
                self.envelope -= release_step;
                if self.envelope <= 0.0 {
                    self.envelope = 0.0;
                    self.stage = ToneStage::Idle;
                    return 0.0;
                }
            }
        }
        let mut sample = 0.0;
        for (multiple, amplitude) in HARMONICS {
            sample += amplitude * (core::f32::consts::TAU * self.phase * multiple).sin();
        }
        self.phase += self.phase_increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        sample * self.envelope
    }
}
