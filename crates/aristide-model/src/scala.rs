//! Scala tuning-file format: `.scl` scales and `.kbm` keyboard mappings.
//!
//! Scala (huygens-fokker.org) is the de facto interchange format for
//! microtonal tunings — thousands of historical and experimental scales
//! already exist as `.scl`/`.kbm` pairs, so reading them is how Aristide
//! gets tuning content without inventing its own scale library. This
//! module is pure parsing: no I/O, no dependency on how the organ model
//! or engine consumes the result, so it can be exercised (and trusted)
//! in isolation from everything downstream. In keeping with the
//! project's no-12-EDO-assumption rule, a [`Scale`] carries cents, not
//! semitones, and a degree number can be anything a mapping produces —
//! nothing here assumes 12 steps to the period or even that the period
//! is a 2/1 octave (Bohlen–Pierce's 3/1 "tritave" parses the same way).

use crate::units::{cents_to_ratio, ratio_to_cents};

/// Non-comment lines of a Scala file, trimmed, paired with their
/// 1-based line number for error messages. A line is a comment iff its
/// trimmed text starts with `!` — that's the whole rule; blank lines
/// are ordinary content (a scale's description line is allowed to be
/// exactly one, and is exactly one blank line, not "no line").
fn non_comment_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line.trim()))
        .filter(|(_, line)| !line.starts_with('!'))
}

/// Pull the next non-comment line and parse its first whitespace-
/// delimited token as `T`. Used for the single-value header fields of
/// both file kinds; `name` only feeds error messages.
fn next_field<'a, I, T>(lines: &mut I, name: &str) -> Result<T, String>
where
    I: Iterator<Item = (usize, &'a str)>,
    T: std::str::FromStr,
{
    let (line_no, line) = lines
        .next()
        .ok_or_else(|| format!("scala: missing {name}"))?;
    let token = line
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("line {line_no}: missing {name}"))?;
    token
        .parse()
        .map_err(|_| format!("line {line_no}: invalid {name} {token:?}"))
}

/// A microtonal scale loaded from a `.scl` file: a period-repeating set
/// of pitches, each expressed as cents above the (implicit, unlisted)
/// tonic degree 0.
#[derive(Debug, Clone, PartialEq)]
pub struct Scale {
    pub description: String,
    /// Degrees 1..=N from the tonic, in cents; the last is the period.
    pub degrees: Vec<f64>,
}

impl Scale {
    /// Parse a `.scl` file's text. Format: comment lines (`!...`)
    /// anywhere; then one description line (may be empty); then a
    /// note-count line; then that many value lines, each a cents value
    /// (contains `.`) or a ratio (`p/q` or bare `p`, meaning `p/1`).
    pub fn parse(text: &str) -> Result<Scale, String> {
        let mut lines = non_comment_lines(text);

        let (_, description) = lines
            .next()
            .ok_or_else(|| "scl: missing description line".to_string())?;
        let description = description.to_string();

        let note_count: i64 = next_field(&mut lines, "note count")?;
        if note_count < 0 {
            return Err("scl: note count must not be negative".to_string());
        }
        let note_count = note_count as usize;

        let mut degrees = Vec::with_capacity(note_count);
        for seen in 0..note_count {
            let (line_no, line) = lines.next().ok_or_else(|| {
                format!("scl: expected {note_count} note values, found only {seen}")
            })?;
            degrees.push(parse_scale_value(line_no, line)?);
        }

        Ok(Scale { description, degrees })
    }

    /// Cents above the tonic of an arbitrary (possibly negative) degree
    /// number. `degree = period_count * period_cents + degrees[within]`,
    /// where `within = degree.rem_euclid(N)` and degree 0 within a
    /// period is 0¢ (the implicit unison, never stored). A scale with
    /// `N = 0` has no period to repeat, so every degree is 0¢.
    pub fn degree_cents(&self, degree: i64) -> f64 {
        let n = self.degrees.len();
        if n == 0 {
            return 0.0;
        }
        let n = n as i64;
        let period_count = degree.div_euclid(n);
        let within = degree.rem_euclid(n);
        let base = period_count as f64 * self.period_cents();
        if within == 0 {
            base
        } else {
            base + self.degrees[(within - 1) as usize]
        }
    }

    /// The last degree's cents value — the formal period of the scale
    /// (usually, but not necessarily, a 2/1 octave). `0.0` for an empty
    /// scale.
    pub fn period_cents(&self) -> f64 {
        self.degrees.last().copied().unwrap_or(0.0)
    }

    pub fn len(&self) -> usize {
        self.degrees.len()
    }

    pub fn is_empty(&self) -> bool {
        self.degrees.is_empty()
    }
}

/// Parse one `.scl` value line: the first whitespace-delimited token is
/// the value, everything after it is free-text comment (Scala allows
/// annotating e.g. `700.0  fifth` without a leading `!`).
fn parse_scale_value(line_no: usize, line: &str) -> Result<f64, String> {
    let token = line
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("line {line_no}: empty scale value"))?;

    if token.contains('.') {
        return token
            .parse::<f64>()
            .map_err(|_| format!("line {line_no}: invalid cents value {token:?}"));
    }

    let (numerator, denominator) = match token.split_once('/') {
        Some((p, q)) => {
            let p: i64 = p
                .parse()
                .map_err(|_| format!("line {line_no}: invalid ratio numerator in {token:?}"))?;
            let q: i64 = q
                .parse()
                .map_err(|_| format!("line {line_no}: invalid ratio denominator in {token:?}"))?;
            (p, q)
        }
        None => {
            let p: i64 = token
                .parse()
                .map_err(|_| format!("line {line_no}: invalid scale value {token:?}"))?;
            (p, 1)
        }
    };
    if numerator <= 0 || denominator <= 0 {
        return Err(format!("line {line_no}: ratio must be positive in {token:?}"));
    }
    Ok(ratio_to_cents(numerator as f64 / denominator as f64))
}

/// A `.kbm` keyboard mapping: how MIDI key numbers correspond to a
/// [`Scale`]'s degrees. Scala mappings repeat a short pattern of `M`
/// keys (typically one 2/1 octave's worth of physical keys, however
/// many scale degrees that octave actually holds) across the whole
/// keyboard.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyboardMapping {
    pub first_key: i32,
    pub last_key: i32,
    pub middle_key: i32,
    pub reference_key: i32,
    pub reference_hz: f64,
    pub octave_degrees: i64,
    /// Pattern entries; empty = 1:1 linear (map size 0).
    pub mapping: Vec<Option<i64>>,
}

impl KeyboardMapping {
    /// Parse a `.kbm` file's text: seven header fields (map size, first
    /// key, last key, middle key, reference key, reference Hz, octave
    /// degree span), one per non-comment line, then up to `map size`
    /// mapping lines (a scale-degree number, or `x` for unmapped).
    /// Fewer mapping lines than the header promises is accepted — the
    /// rest of the pattern is treated as unmapped, per the Scala spec.
    pub fn parse(text: &str) -> Result<KeyboardMapping, String> {
        let mut lines = non_comment_lines(text);

        let map_size: i64 = next_field(&mut lines, "map size")?;
        if map_size < 0 {
            return Err("kbm: map size must not be negative".to_string());
        }
        let map_size = map_size as usize;

        let first_key: i32 = next_field(&mut lines, "first key")?;
        let last_key: i32 = next_field(&mut lines, "last key")?;
        let middle_key: i32 = next_field(&mut lines, "middle key")?;
        let reference_key: i32 = next_field(&mut lines, "reference key")?;
        let reference_hz: f64 = next_field(&mut lines, "reference frequency")?;
        if reference_hz <= 0.0 || reference_hz.is_nan() {
            return Err("kbm: reference frequency must be positive".to_string());
        }
        let octave_degrees: i64 = next_field(&mut lines, "octave degree span")?;

        let mut mapping = Vec::with_capacity(map_size);
        for _ in 0..map_size {
            match lines.next() {
                Some((line_no, line)) => mapping.push(parse_mapping_entry(line_no, line)?),
                None => mapping.push(None),
            }
        }

        Ok(KeyboardMapping {
            first_key,
            last_key,
            middle_key,
            reference_key,
            reference_hz,
            octave_degrees,
            mapping,
        })
    }

    /// The linear default used when an organ names a scale but no
    /// `.kbm`: every key maps 1:1 to successive degrees, middle key 60
    /// on degree 0, and `reference_key` sounding `reference_hz` — the
    /// tuning's own anchor, whichever piano key it names. Modeled as
    /// an empty pattern, which [`degree_of`] already treats as `key -
    /// middle_key`.
    ///
    /// [`degree_of`]: KeyboardMapping::degree_of
    pub fn linear(reference_key: i32, reference_hz: f64) -> KeyboardMapping {
        KeyboardMapping {
            first_key: 0,
            last_key: 127,
            middle_key: 60,
            reference_key,
            reference_hz,
            octave_degrees: 0,
            mapping: Vec::new(),
        }
    }

    /// The scale degree a key means, or `None` when the key is
    /// unmapped or outside `[first_key, last_key]`. With an empty
    /// mapping this is `key - middle_key`. Otherwise: `offset = key -
    /// middle_key`, pattern index = `offset.rem_euclid(M)`, pattern
    /// number = `offset.div_euclid(M)`; the degree is `pattern_number *
    /// octave_degrees + mapping[pattern index]` (an `x` entry ->
    /// `None`).
    pub fn degree_of(&self, key: i32) -> Option<i64> {
        if key < self.first_key || key > self.last_key {
            return None;
        }
        if self.mapping.is_empty() {
            return Some(key as i64 - self.middle_key as i64);
        }
        let pattern_len = self.mapping.len() as i64;
        let offset = key as i64 - self.middle_key as i64;
        let index = offset.rem_euclid(pattern_len);
        let pattern_number = offset.div_euclid(pattern_len);
        self.mapping[index as usize].map(|degree| pattern_number * self.octave_degrees + degree)
    }

    /// Same arithmetic as [`degree_of`], but never `None`: an `x`
    /// pattern entry falls back to "linear-through" — the pattern
    /// index itself, as if that slot had mapped straight to its own
    /// position. Used only to anchor the reference key in
    /// [`key_frequency`], which needs *some* pitch for the reference
    /// key even when a `.kbm` author left it unmapped (this happens in
    /// mapping files written for a different tonic than they get
    /// reused with — the reference key is a physical key on the
    /// keyboard, not a scale-degree choice, so it must always resolve).
    ///
    /// [`degree_of`]: KeyboardMapping::degree_of
    fn reference_degree(&self) -> i64 {
        let key = self.reference_key;
        if self.mapping.is_empty() {
            return key as i64 - self.middle_key as i64;
        }
        let pattern_len = self.mapping.len() as i64;
        let offset = key as i64 - self.middle_key as i64;
        let index = offset.rem_euclid(pattern_len);
        let pattern_number = offset.div_euclid(pattern_len);
        let local = self.mapping[index as usize].unwrap_or(index);
        pattern_number * self.octave_degrees + local
    }
}

/// Parse one `.kbm` mapping-pattern line: a non-negative scale-degree
/// number, or the letter `x` (case-insensitive) for an unmapped key.
fn parse_mapping_entry(line_no: usize, line: &str) -> Result<Option<i64>, String> {
    let token = line
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("line {line_no}: empty mapping entry"))?;
    if token.eq_ignore_ascii_case("x") {
        return Ok(None);
    }
    let degree: i64 = token
        .parse()
        .map_err(|_| format!("line {line_no}: invalid mapping entry {token:?}"))?;
    if degree < 0 {
        return Err(format!(
            "line {line_no}: mapping entry must not be negative in {token:?}"
        ));
    }
    Ok(Some(degree))
}

/// The frequency a key sounds under `scale` retuned via `mapping`:
/// everything is anchored to the reference key's frequency, so a
/// mapping's `reference_hz` is exact at the reference key regardless of
/// what tonic the scale itself was written around. `None` when `key`
/// is unmapped or outside the mapping's retuned range.
pub fn key_frequency(scale: &Scale, mapping: &KeyboardMapping, key: i32) -> Option<f64> {
    let key_degree = mapping.degree_of(key)?;
    let reference_degree = mapping.reference_degree();
    let cents = scale.degree_cents(key_degree) - scale.degree_cents(reference_degree);
    Some(mapping.reference_hz * cents_to_ratio(cents))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDO_12_SCL: &str = "\
! 12-EDO test scale
! comments may appear anywhere, including between the description and
! the note count
12-tone equal temperament
! this comment sits right before the note count on purpose
12
100.0
200.0
300.0
400.0
500.0
600.0
700.0  perfect fifth, trailing comment text
800.0
900.0
1000.0
1100.0
1200.0
";

    const JUST_INTONATION_SCL: &str = "\
! 5-limit just intonation, six-note
Just intonation with ratios and a bare-integer period
6
9/8
5/4
4/3
3/2
5/3
2
";

    const WHITE_KEY_KBM: &str = "\
! 12-in-pattern mapping onto a 7-degree diatonic scale
12
0
127
60
69
440.0
7
0
x
1
x
2
3
x
4
x
5
x
6
";

    #[test]
    fn parses_12edo_cents_scale() {
        let scale = Scale::parse(EDO_12_SCL).unwrap();
        assert_eq!(scale.description, "12-tone equal temperament");
        assert_eq!(scale.len(), 12);
        assert_eq!(scale.degrees[0], 100.0);
        assert_eq!(scale.degrees[6], 700.0);
        assert_eq!(scale.degrees[11], 1200.0);
        assert_eq!(scale.period_cents(), 1200.0);
    }

    #[test]
    fn parses_just_intonation_ratios_and_bare_integer_period() {
        let scale = Scale::parse(JUST_INTONATION_SCL).unwrap();
        assert_eq!(scale.len(), 6);
        // 9/8 major second
        assert!((scale.degrees[0] - 203.910_002).abs() < 1e-4);
        // 5/4 major third
        assert!((scale.degrees[1] - 386.313_714).abs() < 1e-4);
        // bare "2" => 2/1 => exactly 1200 cents
        assert_eq!(scale.degrees[5], 1200.0);
        assert_eq!(scale.period_cents(), 1200.0);
    }

    #[test]
    fn blank_description_line_is_legal() {
        let text = "\
! leading comment
! another
\n\
3
100.0
200.0
300.0
";
        let scale = Scale::parse(text).unwrap();
        assert_eq!(scale.description, "");
        assert_eq!(scale.len(), 3);
    }

    #[test]
    fn trailing_comment_text_on_value_line_is_ignored() {
        let text = "\
desc
2
100.0 this is not a real comment marker but is ignored anyway
200.0\tsome\ttabs\ttoo
";
        let scale = Scale::parse(text).unwrap();
        assert_eq!(scale.degrees, vec![100.0, 200.0]);
    }

    #[test]
    fn zero_note_scale_is_legal_and_always_zero_cents() {
        let text = "empty scale\n0\n";
        let scale = Scale::parse(text).unwrap();
        assert!(scale.is_empty());
        assert_eq!(scale.period_cents(), 0.0);
        for d in [-5i64, -1, 0, 1, 5] {
            assert_eq!(scale.degree_cents(d), 0.0);
        }
    }

    #[test]
    fn edo_19_degree_cents_across_multiple_periods() {
        let step = 1200.0 / 19.0;
        let mut text = String::from("19-EDO\n19\n");
        for i in 1..=19 {
            text.push_str(&format!("{:.10}\n", step * i as f64));
        }
        let scale = Scale::parse(&text).unwrap();
        assert_eq!(scale.len(), 19);
        assert!((scale.period_cents() - 1200.0).abs() < 1e-6);

        // Within the first period, degree d is just d*step.
        for d in 0..19i64 {
            assert!((scale.degree_cents(d) - step * d as f64).abs() < 1e-6);
        }
        // Two periods up: an extra 2*1200 cents on top of the in-period value.
        for d in 0..19i64 {
            let expected = 2.0 * 1200.0 + step * d as f64;
            assert!((scale.degree_cents(d + 19 * 2) - expected).abs() < 1e-6);
        }
        // Negative degrees via rem_euclid: -1 is one step below the
        // tonic, i.e. one period down plus the 18th (last-but-one) degree.
        let expected_minus_one = -1200.0 + step * 18.0;
        assert!((scale.degree_cents(-1) - expected_minus_one).abs() < 1e-6);
        // Two periods down from degree 5.
        let expected = -2.0 * 1200.0 + step * 5.0;
        assert!((scale.degree_cents(5 - 19 * 2) - expected).abs() < 1e-6);
    }

    #[test]
    fn bohlen_pierce_period_is_3_over_1() {
        // A minimal Bohlen-Pierce-style scale: single degree whose
        // period is the 3/1 "tritave" rather than a 2/1 octave.
        let text = "Bohlen-Pierce tritave\n1\n3/1\n";
        let scale = Scale::parse(text).unwrap();
        let expected_period = 1200.0 * 3f64.log2(); // 1901.955...
        assert!((scale.period_cents() - expected_period).abs() < 1e-9);
        assert!((scale.degree_cents(1) - expected_period).abs() < 1e-9);
        assert!((scale.degree_cents(2) - 2.0 * expected_period).abs() < 1e-9);
        assert!((scale.degree_cents(-1) - (-expected_period)).abs() < 1e-9);
    }

    #[test]
    fn kbm_white_key_mapping_header_fields() {
        let kbm = KeyboardMapping::parse(WHITE_KEY_KBM).unwrap();
        assert_eq!(kbm.first_key, 0);
        assert_eq!(kbm.last_key, 127);
        assert_eq!(kbm.middle_key, 60);
        assert_eq!(kbm.reference_key, 69);
        assert_eq!(kbm.reference_hz, 440.0);
        assert_eq!(kbm.octave_degrees, 7);
        assert_eq!(kbm.mapping.len(), 12);
    }

    #[test]
    fn kbm_degree_of_pattern_boundaries_and_negative_offsets() {
        let kbm = KeyboardMapping::parse(WHITE_KEY_KBM).unwrap();

        // Middle key itself: pattern index 0, degree 0.
        assert_eq!(kbm.degree_of(60), Some(0));
        // One pattern repetition up (C an octave above middle C).
        assert_eq!(kbm.degree_of(72), Some(7));
        // Two repetitions up.
        assert_eq!(kbm.degree_of(84), Some(14));
        // One repetition down (offset -12): index 0, pattern number -1.
        assert_eq!(kbm.degree_of(48), Some(-7));
        // Offset -13 crosses a pattern boundary: index 11, pattern -2.
        assert_eq!(kbm.degree_of(47), Some(-2 * 7 + 6));

        // Unmapped (black) keys are None.
        assert_eq!(kbm.degree_of(61), None); // C#
        assert_eq!(kbm.degree_of(66), None); // F#

        // Out of the retuned range.
        assert_eq!(kbm.degree_of(-1), None);
        assert_eq!(kbm.degree_of(128), None);
    }

    #[test]
    fn key_frequency_linear_12edo_reproduces_standard_midi_pitch() {
        let scale = Scale::parse(EDO_12_SCL).unwrap();
        let mapping = KeyboardMapping::linear(69, 440.0);

        assert_eq!(key_frequency(&scale, &mapping, 69), Some(440.0));

        let middle_c = key_frequency(&scale, &mapping, 60).unwrap();
        assert!((middle_c - 261.6256).abs() < 1e-3, "got {middle_c}");

        let a3 = key_frequency(&scale, &mapping, 57).unwrap();
        assert!((a3 - 220.0).abs() < 1e-9, "got {a3}");
    }

    #[test]
    fn key_frequency_with_baroque_reference_pitch() {
        let scale = Scale::parse(EDO_12_SCL).unwrap();
        let mapping = KeyboardMapping::linear(69, 415.0);
        assert_eq!(key_frequency(&scale, &mapping, 69), Some(415.0));
    }

    /// The linear mapping anchors whichever key the tuning names:
    /// middle C at 256 Hz puts a′ nine equal steps above it, not at
    /// 440.
    #[test]
    fn key_frequency_linear_anchors_any_reference_key() {
        let scale = Scale::parse(EDO_12_SCL).unwrap();
        let mapping = KeyboardMapping::linear(60, 256.0);
        assert_eq!(key_frequency(&scale, &mapping, 60), Some(256.0));
        let a = key_frequency(&scale, &mapping, 69).unwrap();
        assert!((a - 256.0 * 2f64.powf(9.0 / 12.0)).abs() < 1e-9, "got {a}");
    }

    #[test]
    fn key_frequency_none_when_key_unmapped_or_out_of_range() {
        let scale = Scale::parse(EDO_12_SCL).unwrap();
        let kbm = KeyboardMapping::parse(WHITE_KEY_KBM).unwrap();
        assert_eq!(key_frequency(&scale, &kbm, 61), None); // C#, unmapped
        assert_eq!(key_frequency(&scale, &kbm, 200), None); // out of range
    }

    #[test]
    fn rejects_negative_note_count() {
        let err = Scale::parse("desc\n-1\n").unwrap_err();
        assert!(err.contains("negative"), "{err}");
    }

    #[test]
    fn rejects_zero_numerator_ratio() {
        let err = Scale::parse("desc\n1\n0/3\n").unwrap_err();
        assert!(err.contains("positive"), "{err}");
    }

    #[test]
    fn rejects_zero_denominator_ratio() {
        let err = Scale::parse("desc\n1\n3/0\n").unwrap_err();
        assert!(err.contains("positive"), "{err}");
    }

    #[test]
    fn rejects_garbage_value_token() {
        let err = Scale::parse("desc\n1\nnot-a-number\n").unwrap_err();
        assert!(err.contains("line 3"), "{err}");
    }

    #[test]
    fn rejects_too_few_value_lines() {
        let err = Scale::parse("desc\n3\n100.0\n200.0\n").unwrap_err();
        assert!(err.contains("expected 3"), "{err}");
    }

    #[test]
    fn rejects_kbm_with_non_numeric_header() {
        let text = "not-a-number\n0\n127\n60\n69\n440.0\n12\n";
        let err = KeyboardMapping::parse(text).unwrap_err();
        assert!(err.contains("map size"), "{err}");
    }

    #[test]
    fn parsing_never_panics_on_truncated_input() {
        for (text, is_scale) in [(EDO_12_SCL, true), (WHITE_KEY_KBM, false)] {
            for (i, _) in text.char_indices() {
                let prefix = &text[..i];
                if is_scale {
                    let _: Result<Scale, String> = Scale::parse(prefix);
                } else {
                    let _: Result<KeyboardMapping, String> = KeyboardMapping::parse(prefix);
                }
            }
            // Also the full text and the empty string.
            if is_scale {
                let _ = Scale::parse(text);
                let _ = Scale::parse("");
            } else {
                let _ = KeyboardMapping::parse(text);
                let _ = KeyboardMapping::parse("");
            }
        }
    }
}
