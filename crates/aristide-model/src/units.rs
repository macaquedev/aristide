//! Unit conversions every layer needs — cents, ratios, decibels, and
//! the 12-EDO/A440 ladder that MIDI metadata is defined against.
//!
//! One home, so each formula has one place to change and callers say
//! *why* they convert rather than re-spelling *how*. The ladder in
//! particular is a fact about file metadata (`smpl` unity notes, ODF
//! `MIDIKeyNumber`s, the tone generator's default), never a statement
//! about how a key should sound: live key→pitch policy belongs to the
//! tuning layer, which merely starts from this ladder.

/// The frequency ratio a cent offset denotes.
#[inline]
pub fn cents_to_ratio(cents: f64) -> f64 {
    (cents / 1200.0).exp2()
}

/// The cent offset a frequency ratio denotes.
#[inline]
pub fn ratio_to_cents(ratio: f64) -> f64 {
    1200.0 * ratio.log2()
}

/// Cents from `from_hz` up to `to_hz` (negative when `to_hz` is lower).
#[inline]
pub fn cents_between(from_hz: f64, to_hz: f64) -> f64 {
    ratio_to_cents(to_hz / from_hz)
}

/// The 12-EDO, a′ = 440 Hz frequency of a (possibly fractional) MIDI
/// key number.
#[inline]
pub fn equal_ladder_hz(key: f64) -> f64 {
    440.0 * ((key - 69.0) / 12.0).exp2()
}

/// Amplitude gain for a level in decibels.
#[inline]
pub fn db_to_linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Level in decibels for an amplitude gain.
#[inline]
pub fn linear_to_db(gain: f64) -> f64 {
    20.0 * gain.log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cents_and_ratios_invert() {
        for cents in [-1200.0, -386.31, 0.0, 1.0, 701.955, 1901.955] {
            assert!((ratio_to_cents(cents_to_ratio(cents)) - cents).abs() < 1e-9);
        }
        assert_eq!(cents_to_ratio(1200.0), 2.0);
        assert!((ratio_to_cents(1.5) - 701.955).abs() < 1e-3);
        assert!((cents_between(440.0, 415.0) + 101.27).abs() < 0.01);
    }

    #[test]
    fn ladder_is_a440_twelve_edo() {
        assert_eq!(equal_ladder_hz(69.0), 440.0);
        assert_eq!(equal_ladder_hz(81.0), 880.0);
        assert!((equal_ladder_hz(60.0) - 261.6256).abs() < 1e-4);
    }

    #[test]
    fn decibels_invert() {
        assert!((db_to_linear(-6.0206) - 0.5).abs() < 1e-4);
        assert!((linear_to_db(db_to_linear(-12.5)) + 12.5).abs() < 1e-9);
        assert_eq!(db_to_linear(0.0), 1.0);
    }
}
