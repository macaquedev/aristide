//! The console's palette: dark walnut case, gold trim, ivory keys —
//! the classic GrandOrgue look, kept in one place so the widgets stay
//! consistent.

use eframe::egui::Color32;

/// Window background: near-black walnut.
pub const BG: Color32 = Color32::from_rgb(0x1e, 0x16, 0x0f);
/// Panel faces (header, console rail, drawers).
pub const PANEL: Color32 = Color32::from_rgb(0x2a, 0x1f, 0x15);
/// Thin trim line between panels.
pub const PANEL_EDGE: Color32 = Color32::from_rgb(0x4d, 0x3b, 0x25);

/// Brass/gold accents: lit indicators, rims, headings.
pub const GOLD: Color32 = Color32::from_rgb(0xc8, 0xa2, 0x4a);
/// Dimmed gold for fills that must not shout.
pub const GOLD_DIM: Color32 = Color32::from_rgb(0x8a, 0x70, 0x35);

/// Drawn stop face and white keys.
pub const IVORY: Color32 = Color32::from_rgb(0xf0, 0xe6, 0xce);
/// Engraved lettering on an ivory face.
pub const ENGRAVE: Color32 = Color32::from_rgb(0x41, 0x30, 0x1a);

/// A pushed-in (off) drawknob face and its rim.
pub const KNOB_OFF: Color32 = Color32::from_rgb(0x6e, 0x5e, 0x41);
pub const KNOB_OFF_RIM: Color32 = Color32::from_rgb(0x51, 0x43, 0x2c);
/// The dark socket a knob sits in.
pub const KNOB_SOCKET: Color32 = Color32::from_rgb(0x14, 0x0e, 0x08);

/// Black keys.
pub const KEY_BLACK: Color32 = Color32::from_rgb(0x16, 0x11, 0x0b);
/// Outline between keys.
pub const KEY_EDGE: Color32 = Color32::from_rgb(0x30, 0x28, 0x1c);

/// Ordinary labels on the case.
pub const TEXT: Color32 = Color32::from_rgb(0xd9, 0xce, 0xb5);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8f, 0x83, 0x69);

pub const OK: Color32 = Color32::from_rgb(0x5f, 0xb8, 0x6a);
pub const ERR: Color32 = Color32::from_rgb(0xd0, 0x60, 0x50);
