//! Lumatone key-mapping files: `.ltn`, saved by the Lumatone Editor (and
//! its Terpstra-keyboard predecessor, TerpstraSysEx) to describe what
//! each physical key of a five-board, 56-key-per-board hex keyboard
//! transmits over MIDI, and what colour it's lit.
//!
//! The format is INI-ish but was never meant to be hand-validated by
//! anything other than its own writer, which always dumps every key of
//! every board (`Key_i`/`Chan_i`/`Col_i`, plus `KTyp_i` only when a key
//! isn't the default note type, plus assorted global settings after the
//! last board). Community-maintained mapping files and partial/manually
//! trimmed ones are common enough that parsing here is line-at-a-time
//! and tolerant: an unrecognized or malformed line is skipped, never
//! fatal. The only failure mode is a file with no `[BoardN]` section at
//! all — a real `.ltn` always has at least one.

use std::collections::{BTreeSet, HashMap};

/// Physical keys per board.
pub const KEYS_PER_BOARD: u16 = 56;
/// Boards making up a keyboard (`[Board0]`..`[Board4]`).
pub const BOARD_COUNT: u16 = 5;

/// What a key transmits (`KTyp_N` in the file). Only
/// [`KeyType::NoteOnNoteOff`] sounds a pitch; the rest are console-side
/// controls this module has no opinion about beyond "not a note" —
/// [`LumatoneMap::key_for`] never returns them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    /// The default when a key's `KTyp_N` entry is absent.
    NoteOnNoteOff,
    ContinuousController,
    LumaTouch,
    Disabled,
}

impl KeyType {
    /// The file's `KTyp_N` integer vocabulary (1-4; 0 is an
    /// editor-internal "invalid channel" marker that's never actually
    /// written, so it's treated the same as any other unrecognized code).
    fn from_code(code: u32) -> Option<KeyType> {
        match code {
            1 => Some(KeyType::NoteOnNoteOff),
            2 => Some(KeyType::ContinuousController),
            3 => Some(KeyType::LumaTouch),
            4 => Some(KeyType::Disabled),
            _ => None,
        }
    }
}

/// Per-key data as parsed, before [`LumatoneMap::parse`] indexes it for
/// lookup. A `None` field simply wasn't present in the file for this key.
#[derive(Debug, Clone, Copy, Default)]
struct RawKey {
    note: Option<u8>,
    /// 0-based; the file's `Chan_N` is 1-based.
    channel: Option<u8>,
    colour: Option<u32>,
    key_type: Option<KeyType>,
}

/// A Lumatone keyboard mapping: what each of the (up to) 280 physical
/// keys — five boards of 56 — transmits, and how it's lit.
#[derive(Debug, Clone, Default)]
pub struct LumatoneMap {
    /// (0-based channel, note) -> contiguous key number; `NoteOnNoteOff`
    /// keys only.
    note_index: HashMap<(u8, u8), u16>,
    colours: HashMap<u16, u32>,
    channels: BTreeSet<u8>,
    /// Physical keys that are `NoteOnNoteOff` with both a note and a
    /// channel set. May exceed `note_index.len()` when two keys claim
    /// the same (channel, note) and the later one loses.
    note_key_count: usize,
    /// Non-fatal issues found while parsing: currently just duplicate
    /// (channel, note) addresses, where the first key found wins.
    pub warnings: Vec<String>,
}

impl LumatoneMap {
    /// Parse a `.ltn` file's text: up to five `[BoardN]` sections (`N` in
    /// `0..5`), each with up to 56 keys' worth of `Key_i`/`Chan_i`/
    /// `Col_i`/`KTyp_i` entries. Lines outside any section, entries this
    /// module doesn't know (`CCInvert_i`, velocity-curve tables, other
    /// global settings), and malformed lines are all skipped rather than
    /// treated as fatal; only the total absence of a board section fails.
    pub fn parse(text: &str) -> Result<LumatoneMap, String> {
        let mut boards = vec![[RawKey::default(); KEYS_PER_BOARD as usize]; BOARD_COUNT as usize];
        let mut any_board = false;
        let mut current_board: Option<usize> = None;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                current_board = parse_board_index(inner);
                any_board |= current_board.is_some();
                continue;
            }

            let Some(board) = current_board else { continue };
            let Some((key, value)) = line.split_once('=') else { continue };
            let key = key.trim();
            let value = value.trim();

            if let Some(index) = key.strip_prefix("Key_").and_then(parse_key_index) {
                if let Some(note) = parse_note_or_cc(value) {
                    boards[board][index].note = Some(note);
                }
            } else if let Some(index) = key.strip_prefix("Chan_").and_then(parse_key_index) {
                if let Ok(chan) = value.parse::<u8>()
                    && (1..=16).contains(&chan)
                {
                    boards[board][index].channel = Some(chan - 1);
                }
            } else if let Some(index) = key.strip_prefix("Col_").and_then(parse_key_index) {
                if let Some(colour) = parse_colour(value) {
                    boards[board][index].colour = Some(colour);
                }
            } else if let Some(index) = key.strip_prefix("KTyp_").and_then(parse_key_index)
                && let Some(kind) = value.parse::<u32>().ok().and_then(KeyType::from_code)
            {
                boards[board][index].key_type = Some(kind);
            }
            // Anything else (CCInvert_i, global settings, unknown
            // entries) is intentionally ignored.
        }

        if !any_board {
            return Err("ltn: no [BoardN] section found".to_string());
        }

        let mut map = LumatoneMap::default();
        for (board_idx, board) in boards.iter().enumerate() {
            for (key_idx, raw) in board.iter().enumerate() {
                let key_number = (board_idx as u16) * KEYS_PER_BOARD + key_idx as u16;

                if let Some(colour) = raw.colour {
                    map.colours.insert(key_number, colour);
                }

                if raw.key_type.unwrap_or(KeyType::NoteOnNoteOff) != KeyType::NoteOnNoteOff {
                    continue;
                }
                let (Some(note), Some(channel)) = (raw.note, raw.channel) else { continue };

                map.note_key_count += 1;
                map.channels.insert(channel);
                match map.note_index.entry((channel, note)) {
                    std::collections::hash_map::Entry::Occupied(existing) => {
                        map.warnings.push(format!(
                            "key {key_number}: channel {channel} note {note} already mapped by key {}; ignoring the duplicate",
                            existing.get()
                        ));
                    }
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        slot.insert(key_number);
                    }
                }
            }
        }

        Ok(map)
    }

    /// Contiguous key number a physical key addresses: `board * 56 +
    /// index` (`0..280`), for the key holding what arrives on the wire —
    /// a 0-based MIDI channel and note number. Only `NoteOnNoteOff` keys
    /// are considered; CC, LumaTouch and disabled keys never match.
    pub fn key_for(&self, channel: u8, note: u8) -> Option<u16> {
        self.note_index.get(&(channel, note)).copied()
    }

    /// Total physical keys that are `NoteOnNoteOff` with both a note and
    /// a channel set — i.e. actually playable. Counts duplicates (see
    /// [`LumatoneMap::warnings`]) even though only one wins the lookup.
    pub fn key_count(&self) -> usize {
        self.note_key_count
    }

    /// The key's colour as `0xRRGGBB`, if the file set one. Independent
    /// of key type: a CC or disabled key's colour is still available,
    /// since the console still needs to draw it.
    pub fn colour(&self, key: u16) -> Option<u32> {
        self.colours.get(&key).copied()
    }

    /// The 0-based MIDI channels used by any `NoteOnNoteOff` key, low to
    /// high; handy for input filtering.
    pub fn channels(&self) -> impl Iterator<Item = u8> + '_ {
        self.channels.iter().copied()
    }
}

/// Parse the inner text of a `[BoardN]` header: exactly `Board` followed
/// by a digit in `0..BOARD_COUNT`.
fn parse_board_index(inner: &str) -> Option<usize> {
    let index: usize = inner.strip_prefix("Board")?.parse().ok()?;
    (index < BOARD_COUNT as usize).then_some(index)
}

/// Parse a `Key_N`/`Chan_N`/`Col_N`/`KTyp_N` suffix into a valid in-board
/// key index.
fn parse_key_index(digits: &str) -> Option<usize> {
    let index: usize = digits.parse().ok()?;
    (index < KEYS_PER_BOARD as usize).then_some(index)
}

/// A `Key_N` value: a MIDI note or CC number, `0..=127`.
fn parse_note_or_cc(value: &str) -> Option<u8> {
    value.parse::<u8>().ok().filter(|&n| n <= 127)
}

/// A `Col_N` value: a bare hex RGB triple, variable width (Lumatone
/// omits leading zero nibbles), case-insensitive, up to 6 digits.
fn parse_colour(value: &str) -> Option<u32> {
    if value.is_empty() || value.len() > 6 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(value, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_BOARD_FIXTURE: &str = "\
[Board0]
Key_0=60
Chan_0=1
Col_0=FF0000
Key_1=61
Chan_1=2
Col_1=00ff00
Key_2=10
Chan_2=1
KTyp_2=2
Col_2=0000FF
this line is garbage and should be skipped
Key_3=62
Chan_3=1
KTyp_3=4
Key_5=not-a-number
Chan_5=1
[Board1]
Key_3=64
Chan_3=3
Col_3=64c8dc
AfterTouchActive=1
LightOnKeyStrokes=1
InvertFootController=0
ExprCtrlSensivity=0
VelocityIntrvlTbl=1 2 3 4 5
";

    #[test]
    fn maps_note_on_note_off_keys_by_channel_and_note() {
        let map = LumatoneMap::parse(TWO_BOARD_FIXTURE).unwrap();
        assert_eq!(map.key_for(0, 60), Some(0));
        assert_eq!(map.key_for(1, 61), Some(1));
    }

    #[test]
    fn board_offset_is_board_times_56_plus_index() {
        let map = LumatoneMap::parse(TWO_BOARD_FIXTURE).unwrap();
        // Board 1, key index 3 -> key number 59.
        assert_eq!(map.key_for(2, 64), Some(59));
    }

    #[test]
    fn cc_typed_key_never_maps_as_a_note() {
        let map = LumatoneMap::parse(TWO_BOARD_FIXTURE).unwrap();
        assert_eq!(map.key_for(0, 10), None);
    }

    #[test]
    fn disabled_key_never_maps_as_a_note() {
        let map = LumatoneMap::parse(TWO_BOARD_FIXTURE).unwrap();
        assert_eq!(map.key_for(0, 62), None);
    }

    #[test]
    fn colour_lookup_is_independent_of_key_type() {
        let map = LumatoneMap::parse(TWO_BOARD_FIXTURE).unwrap();
        assert_eq!(map.colour(0), Some(0xFF0000));
        assert_eq!(map.colour(1), Some(0x00FF00));
        // CC-typed key 2 still has a colour, even though it's not a note.
        assert_eq!(map.colour(2), Some(0x0000FF));
        // Disabled key 3 has no Col_3 line.
        assert_eq!(map.colour(3), None);
        // Board 1 key 3 -> key number 59.
        assert_eq!(map.colour(59), Some(0x64C8DC));
    }

    #[test]
    fn key_count_counts_only_valid_note_on_note_off_keys() {
        let map = LumatoneMap::parse(TWO_BOARD_FIXTURE).unwrap();
        // Board0 key0, board0 key1, board1 key3 (key number 59).
        assert_eq!(map.key_count(), 3);
    }

    #[test]
    fn channels_lists_distinct_zero_based_channels_in_use() {
        let map = LumatoneMap::parse(TWO_BOARD_FIXTURE).unwrap();
        assert_eq!(map.channels().collect::<Vec<_>>(), vec![0, 1, 2]);
    }

    #[test]
    fn malformed_lines_are_skipped_not_fatal() {
        // Key_5's garbage value and the free-text garbage line must not
        // prevent the rest of the file from loading; key 5 simply never
        // gets a note (Chan_5 alone isn't enough to produce a mapping).
        let map = LumatoneMap::parse(TWO_BOARD_FIXTURE).unwrap();
        assert_eq!(map.colour(5), None);
        assert_eq!(map.key_for(0, 62), None); // not accidentally key 5's channel
    }

    #[test]
    fn global_settings_lines_are_tolerated() {
        // Presence alone is the assertion: TWO_BOARD_FIXTURE's trailing
        // AfterTouchActive/VelocityIntrvlTbl/etc. lines must not error.
        assert!(LumatoneMap::parse(TWO_BOARD_FIXTURE).is_ok());
    }

    #[test]
    fn duplicate_channel_note_address_first_wins_and_warns() {
        let text = "\
[Board0]
Key_0=60
Chan_0=1
Col_0=FFFFFF
Key_1=60
Chan_1=1
Col_1=000000
";
        let map = LumatoneMap::parse(text).unwrap();
        assert_eq!(map.key_for(0, 60), Some(0));
        assert_eq!(map.key_count(), 2);
        assert_eq!(map.warnings.len(), 1, "{:?}", map.warnings);
        assert!(map.warnings[0].contains("key 1"), "{}", map.warnings[0]);
    }

    #[test]
    fn rejects_file_with_no_board_section() {
        let err = LumatoneMap::parse("AfterTouchActive=1\nLightOnKeyStrokes=1\n").unwrap_err();
        assert!(err.contains("Board"), "{err}");
    }

    #[test]
    fn empty_input_has_no_board_section() {
        assert!(LumatoneMap::parse("").is_err());
    }

    #[test]
    fn note_values_above_127_are_rejected_per_key() {
        let text = "\
[Board0]
Key_0=200
Chan_0=1
";
        let map = LumatoneMap::parse(text).unwrap();
        assert_eq!(map.key_count(), 0);
    }

    #[test]
    fn channel_out_of_1_to_16_range_is_rejected_per_key() {
        let text = "\
[Board0]
Key_0=60
Chan_0=17
Key_1=61
Chan_1=0
";
        let map = LumatoneMap::parse(text).unwrap();
        assert_eq!(map.key_count(), 0);
    }

    #[test]
    fn unknown_section_header_is_ignored_not_fatal() {
        let text = "\
[Unknown]
Key_0=60
Chan_0=1
[Board0]
Key_0=60
Chan_0=1
";
        let map = LumatoneMap::parse(text).unwrap();
        assert_eq!(map.key_for(0, 60), Some(0));
        assert_eq!(map.key_count(), 1);
    }

    #[test]
    fn parsing_never_panics_on_truncated_input() {
        for (i, _) in TWO_BOARD_FIXTURE.char_indices() {
            let _: Result<LumatoneMap, String> = LumatoneMap::parse(&TWO_BOARD_FIXTURE[..i]);
        }
        let _ = LumatoneMap::parse(TWO_BOARD_FIXTURE);
        let _ = LumatoneMap::parse("");
    }
}
