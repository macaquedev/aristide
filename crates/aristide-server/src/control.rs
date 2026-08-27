//! What an input *does* when it isn't playing a note: the binding layer.
//!
//! A console is more than keyboards. Thumb pistons, toe studs, a
//! transposer, the tremulant switch, an expression shoe — all of them
//! arrive as ordinary MIDI, and none of them means anything until
//! someone says what it is bound to. The same is true of a computer
//! keyboard, which is why key presses come through here too: `=` is not
//! special-cased anywhere, it is a binding like any other.
//!
//! Both halves are **text** on the wire and in the config file
//! (`"note:36"` → `"stop:Montre 8'"`). That is deliberate. It reads as
//! English in a hand-edited file, it survives an organ being renamed,
//! and it is the shape a rule has to be in before a scripting layer can
//! generate or rewrite one. When bindings grow conditions and sequences,
//! this vocabulary is what they will be written in.

use std::fmt;

/// What arrived. Sources (which device, which channel) are matched
/// separately — the same trigger on two consoles is two bindings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// A key press. Note-offs are ignored: a piston is a moment, not a
    /// held key, and the actions here are all instantaneous.
    Note(u8),
    /// A continuous controller. Switch-like actions fire on the upper
    /// half of its travel; the continuous ones use the value itself.
    Control(u8),
    Program(u8),
    /// A computer key, by physical position (`"Equal"`, `"KeyZ"`) so
    /// the binding means the same thing on QWERTZ and AZERTY.
    Key(String),
}

impl Trigger {
    pub fn parse(text: &str) -> Option<Trigger> {
        let (kind, rest) = text.split_once(':')?;
        let number = || rest.parse::<u8>().ok().filter(|n| *n < 128);
        Some(match kind.trim() {
            "note" => Trigger::Note(number()?),
            "cc" | "control" => Trigger::Control(number()?),
            "program" | "pc" => Trigger::Program(number()?),
            "key" => {
                let code = rest.trim();
                if code.is_empty() {
                    return None;
                }
                Trigger::Key(code.to_string())
            }
            _ => return None,
        })
    }

    /// Whether this trigger carries a position (an expression shoe)
    /// rather than an instant (a piston).
    pub fn is_continuous(&self) -> bool {
        matches!(self, Trigger::Control(_))
    }
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trigger::Note(note) => write!(f, "note:{note}"),
            Trigger::Control(cc) => write!(f, "cc:{cc}"),
            Trigger::Program(program) => write!(f, "program:{program}"),
            Trigger::Key(code) => write!(f, "key:{code}"),
        }
    }
}

/// What it does. Names of organ things (a stop, a coupler, an
/// enclosure) are carried as written and matched against the loaded set
/// later, the same way manual names are: a binding made on one organ
/// says something honest about another, or nothing at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Shift the keyboard the trigger came from. Octaves are the coarse
    /// gesture (a transposer's two buttons), semitones the fine one.
    Transpose(i8),
    /// Back to concert pitch, whatever it was shifted to.
    TransposeReset,
    /// Draw or retire a stop by name — a thumb piston's usual job.
    Stop(String),
    Coupler(String),
    /// Toggle a tremulant by name — or every tremulant the organ has,
    /// the way one switch serves the whole console (`None`).
    Tremulant(Option<String>),
    /// Recall the numbered general combination — or store it, when the
    /// setter is armed. The organist's thumb pistons.
    General(u8),
    /// Arm/disarm the setter: while armed, the next general press
    /// stores the current registration instead of recalling.
    Set,
    /// Retire everything, as the cancel piston does.
    Cancel,
    /// Silence: every voice killed, however it got there.
    Panic,
    /// Drive a named swell box directly, rather than through whichever
    /// division the shoe's channel happens to belong to.
    Enclosure(String),
}

impl Action {
    pub fn parse(text: &str) -> Option<Action> {
        let text = text.trim();
        let (verb, argument) = match text.split_once(':') {
            Some((verb, argument)) => (verb.trim(), argument.trim()),
            None => (text, ""),
        };
        let named = |name: &str| (!name.is_empty()).then(|| name.to_string());
        Some(match verb {
            "octave-up" => Action::Transpose(12),
            "octave-down" => Action::Transpose(-12),
            "transpose-up" => Action::Transpose(1),
            "transpose-down" => Action::Transpose(-1),
            "transpose" => Action::Transpose(argument.parse().ok()?),
            "transpose-reset" => Action::TransposeReset,
            "stop" => Action::Stop(named(argument)?),
            "coupler" => Action::Coupler(named(argument)?),
            "tremulant" => Action::Tremulant(named(argument)),
            "general" => Action::General(argument.parse().ok()?),
            "set" => Action::Set,
            "cancel" => Action::Cancel,
            "panic" => Action::Panic,
            "enclosure" => Action::Enclosure(named(argument)?),
            _ => return None,
        })
    }

    /// Whether the action wants a position rather than a nudge — the
    /// one case where a controller's value means something.
    pub fn is_continuous(&self) -> bool {
        matches!(self, Action::Enclosure(_))
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Transpose(12) => write!(f, "octave-up"),
            Action::Transpose(-12) => write!(f, "octave-down"),
            Action::Transpose(1) => write!(f, "transpose-up"),
            Action::Transpose(-1) => write!(f, "transpose-down"),
            Action::Transpose(semitones) => write!(f, "transpose:{semitones}"),
            Action::TransposeReset => write!(f, "transpose-reset"),
            Action::Stop(name) => write!(f, "stop:{name}"),
            Action::Coupler(name) => write!(f, "coupler:{name}"),
            Action::Tremulant(None) => write!(f, "tremulant"),
            Action::Tremulant(Some(name)) => write!(f, "tremulant:{name}"),
            Action::General(slot) => write!(f, "general:{slot}"),
            Action::Set => write!(f, "set"),
            Action::Cancel => write!(f, "cancel"),
            Action::Panic => write!(f, "panic"),
            Action::Enclosure(name) => write!(f, "enclosure:{name}"),
        }
    }
}

/// Every action a UI can offer, in the order it reads best.
pub const CATALOGUE: [&str; 12] = [
    "octave-up",
    "octave-down",
    "transpose-up",
    "transpose-down",
    "transpose-reset",
    "tremulant",
    "general:",
    "set",
    "cancel",
    "panic",
    "stop:",
    "coupler:",
];

/// The two QWERTY letter rows as a keyboard, in the mapping every DAW
/// uses: the bottom row from C, the row above it holding that row's
/// sharps, and the same again an octave up. Keys are named by physical
/// position (`event.code`), so the shape is identical on QWERTZ and
/// AZERTY — the player's fingers find the same notes.
///
/// The console UI draws this same table as its legend; the copy there
/// labels caps, the copy here decides what sounds. Keep them in step.
pub const KEYBOARD_ROWS: [(&str, u8); 34] = [
    ("KeyZ", 48), ("KeyS", 49), ("KeyX", 50), ("KeyD", 51), ("KeyC", 52),
    ("KeyV", 53), ("KeyG", 54), ("KeyB", 55), ("KeyH", 56), ("KeyN", 57),
    ("KeyJ", 58), ("KeyM", 59), ("Comma", 60), ("KeyL", 61), ("Period", 62),
    ("Semicolon", 63), ("Slash", 64),
    ("KeyQ", 60), ("Digit2", 61), ("KeyW", 62), ("Digit3", 63), ("KeyE", 64),
    ("KeyR", 65), ("Digit5", 66), ("KeyT", 67), ("Digit6", 68), ("KeyY", 69),
    ("Digit7", 70), ("KeyU", 71), ("KeyI", 72), ("Digit9", 73), ("KeyO", 74),
    ("Digit0", 75), ("KeyP", 76),
];

/// The note a computer key plays before any shift, if it plays one.
pub fn key_note(code: &str) -> Option<u8> {
    KEYBOARD_ROWS
        .iter()
        .find(|(key, _)| *key == code)
        .map(|(_, note)| *note)
}

/// The four QWERTY rows as a hex-field surface — how the computer
/// keyboard plays a *microtonal* manual, where the piano mapping above
/// (naturals and sharps) is the wrong vocabulary. Entries are
/// `(code, col, row)`: `col` counts within the row, `row` bottom-up
/// (Z row 0, A row 1, Q row 2, digits row 3).
///
/// The key a cap sounds is `HexLayout::key_at_slanted` + the
/// keyboard's shift — the *left-leaning* reading, because that is the
/// physical geometry of a keyboard: each row up sits about half a key
/// left of the one below, with no re-centering. So the cap physically
/// up-right of another (S from Z) sounds +upright, up-left (A from Z)
/// sounds upright − right, and isomorphic shapes lie under the
/// fingers exactly as they lie on the board — under 12-EDO Bosanquet,
/// W (straight above Z) duplicates it.
///
/// The console UI draws this same table as its legend; keep in step.
pub const KEYBOARD_GRID: [(&str, u8, u8); 45] = [
    ("KeyZ", 0, 0), ("KeyX", 1, 0), ("KeyC", 2, 0), ("KeyV", 3, 0),
    ("KeyB", 4, 0), ("KeyN", 5, 0), ("KeyM", 6, 0), ("Comma", 7, 0),
    ("Period", 8, 0), ("Slash", 9, 0),
    ("KeyA", 0, 1), ("KeyS", 1, 1), ("KeyD", 2, 1), ("KeyF", 3, 1),
    ("KeyG", 4, 1), ("KeyH", 5, 1), ("KeyJ", 6, 1), ("KeyK", 7, 1),
    ("KeyL", 8, 1), ("Semicolon", 9, 1), ("Quote", 10, 1),
    ("KeyQ", 0, 2), ("KeyW", 1, 2), ("KeyE", 2, 2), ("KeyR", 3, 2),
    ("KeyT", 4, 2), ("KeyY", 5, 2), ("KeyU", 6, 2), ("KeyI", 7, 2),
    ("KeyO", 8, 2), ("KeyP", 9, 2), ("BracketLeft", 10, 2), ("BracketRight", 11, 2),
    ("Digit1", 0, 3), ("Digit2", 1, 3), ("Digit3", 2, 3), ("Digit4", 3, 3),
    ("Digit5", 4, 3), ("Digit6", 5, 3), ("Digit7", 6, 3), ("Digit8", 7, 3),
    ("Digit9", 8, 3), ("Digit0", 9, 3), ("Minus", 10, 3), ("Equal", 11, 3),
];

/// The grid position a computer key holds on a hex-field manual, if
/// any. Bindings still win before this is ever asked (`=` bound to
/// octave-up never plays a note).
pub fn key_grid(code: &str) -> Option<(u8, u8)> {
    KEYBOARD_GRID
        .iter()
        .find(|(key, _, _)| *key == code)
        .map(|(_, col, row)| (*col, *row))
}

/// The span the rows cover — the computer keyboard's own compass, the
/// way a MIDI keyboard's is the width of its keys.
pub fn keyboard_compass() -> (u8, u8) {
    let notes = KEYBOARD_ROWS.iter().map(|(_, note)| *note);
    (notes.clone().min().unwrap_or(0), notes.max().unwrap_or(127))
}

/// A switch-like trigger's threshold: the upper half of a controller's
/// travel is "pressed", which is what every organ console and every
/// sustain pedal already assumes.
pub const SWITCH_ON: u8 = 64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers_and_actions_round_trip_through_their_text() {
        let cases = [
            "note:36",
            "cc:64",
            "program:5",
            "key:Equal",
        ];
        for text in cases {
            let trigger = Trigger::parse(text).expect("parses");
            assert_eq!(trigger.to_string(), text, "round trip");
        }
        assert_eq!(Trigger::parse("note:200"), None, "not a MIDI note");
        assert_eq!(Trigger::parse("nonsense"), None);
        assert_eq!(Trigger::parse("key:"), None);

        for text in [
            "octave-up",
            "octave-down",
            "transpose-up",
            "transpose-down",
            "transpose:7",
            "transpose-reset",
            "stop:Montre 8'",
            "coupler:II/I",
            "tremulant",
            "tremulant:Tremblant",
            "general:3",
            "set",
            "cancel",
            "panic",
            "enclosure:Récit",
        ] {
            let action = Action::parse(text).expect("parses");
            assert_eq!(action.to_string(), text, "round trip");
        }
        assert_eq!(Action::parse("stop:"), None, "a stop needs a name");
        assert_eq!(Action::parse("fly-to-the-moon"), None);
    }

    #[test]
    fn the_computer_keyboard_is_two_unbroken_octaves() {
        assert_eq!(key_note("KeyZ"), Some(48), "bottom row starts at C3");
        assert_eq!(key_note("KeyQ"), Some(60), "top row an octave above");
        assert_eq!(key_note("Enter"), None);
        assert_eq!(keyboard_compass(), (48, 76));
        // Every semitone between the ends is reachable, or the legend
        // would have a hole in it.
        for note in 48..=76u8 {
            assert!(
                KEYBOARD_ROWS.iter().any(|(_, n)| *n == note),
                "no key plays {note}"
            );
        }
    }

    #[test]
    fn the_two_octave_buttons_are_a_transpose_underneath() {
        assert_eq!(Action::parse("octave-up"), Some(Action::Transpose(12)));
        assert_eq!(Action::parse("transpose:-5"), Some(Action::Transpose(-5)));
        assert!(Action::parse("enclosure:Récit").expect("parses").is_continuous());
        assert!(!Action::parse("tremulant").expect("parses").is_continuous());
    }
}
