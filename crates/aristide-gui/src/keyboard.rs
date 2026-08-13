//! Clickable keyboards: manual strips and the pedalboard. Geometry and
//! hit-testing are plain functions (tested); drawing and pointer state
//! live in `strip`.

use eframe::egui::{self, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::theme;

/// Key sizes for one keyboard flavour.
#[derive(Clone, Copy)]
pub struct KeyDims {
    pub white_w: f32,
    pub white_h: f32,
    pub black_w: f32,
    pub black_h: f32,
}

/// A 61-note manual strip.
pub const MANUAL: KeyDims = KeyDims { white_w: 14.0, white_h: 62.0, black_w: 9.0, black_h: 38.0 };
/// The pedalboard: fewer, chunkier keys.
pub const PEDAL: KeyDims = KeyDims { white_w: 19.0, white_h: 46.0, black_w: 12.0, black_h: 27.0 };

/// Is this MIDI note a natural (white key)?
pub fn is_white(midi: u8) -> bool {
    matches!(midi % 12, 0 | 2 | 4 | 5 | 7 | 9 | 11)
}

/// How many naturals a keyboard of `count` keys from `first` contains —
/// i.e. its width in white keys.
pub fn white_count(first: u8, count: u8) -> usize {
    (first..first.saturating_add(count)).filter(|&k| is_white(k)).count()
}

/// Pixel width of the whole keyboard.
pub fn strip_width(first: u8, count: u8, dims: &KeyDims) -> f32 {
    white_count(first, count) as f32 * dims.white_w
}

/// One key's rectangle within the strip.
struct Key {
    midi: u8,
    rect: Rect,
    white: bool,
}

fn layout(origin: Pos2, first: u8, count: u8, dims: &KeyDims) -> Vec<Key> {
    let mut keys = Vec::with_capacity(count as usize);
    let mut whites = 0usize;
    for midi in first..first.saturating_add(count) {
        if is_white(midi) {
            let x = origin.x + whites as f32 * dims.white_w;
            keys.push(Key {
                midi,
                rect: Rect::from_min_size(pos2(x, origin.y), vec2(dims.white_w, dims.white_h)),
                white: true,
            });
            whites += 1;
        } else {
            // A sharp straddles the boundary after the previous natural.
            let x = origin.x + whites as f32 * dims.white_w - dims.black_w * 0.5;
            keys.push(Key {
                midi,
                rect: Rect::from_min_size(pos2(x, origin.y), vec2(dims.black_w, dims.black_h)),
                white: false,
            });
        }
    }
    keys
}

/// The key under `pos`, sharps first (they sit on top of the naturals).
fn hit(keys: &[Key], pos: Pos2) -> Option<u8> {
    keys.iter()
        .filter(|k| !k.white)
        .chain(keys.iter().filter(|k| k.white))
        .find(|k| k.rect.contains(pos))
        .map(|k| k.midi)
}

/// Draw one keyboard and report which key the pointer is holding down,
/// if any. `held` lights keys the server reports as sounding; the
/// caller overlays its own optimistic press.
pub fn strip(
    ui: &mut Ui,
    first: u8,
    count: u8,
    dims: &KeyDims,
    held: &dyn Fn(u8) -> bool,
) -> (Response, Option<u8>) {
    let size = vec2(strip_width(first, count, dims), dims.white_h);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let keys = layout(rect.min, first, count, dims);

    let mut down = None;
    if response.is_pointer_button_down_on() {
        if let Some(pos) = response.interact_pointer_pos() {
            down = hit(&keys, pos);
        }
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect.expand(2.0), egui::CornerRadius::same(2), theme::KNOB_SOCKET);
        for key in keys.iter().filter(|k| k.white) {
            let lit = held(key.midi) || down == Some(key.midi);
            painter.rect_filled(
                key.rect.shrink(0.5),
                egui::CornerRadius::ZERO,
                if lit { theme::GOLD } else { theme::IVORY },
            );
            painter.rect_stroke(
                key.rect,
                egui::CornerRadius::ZERO,
                Stroke::new(1.0, theme::KEY_EDGE),
                StrokeKind::Inside,
            );
        }
        for key in keys.iter().filter(|k| !k.white) {
            let lit = held(key.midi) || down == Some(key.midi);
            painter.rect_filled(
                key.rect,
                egui::CornerRadius::same(1),
                if lit { theme::GOLD } else { theme::KEY_BLACK },
            );
        }
    }
    (response, down)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_counts_match_real_keyboards() {
        // 61-key manual from C: 36 naturals.
        assert_eq!(white_count(36, 61), 36);
        // 32-note pedalboard from C: 19 naturals.
        assert_eq!(white_count(36, 32), 19);
    }

    #[test]
    fn sharps_win_the_hit_test_where_they_overlap() {
        let dims = MANUAL;
        let keys = layout(pos2(0.0, 0.0), 36, 61, &dims);
        // Just right of the C/C# boundary, high up: that's C#37's turf.
        let on_sharp = pos2(dims.white_w + 1.0, dims.black_h - 5.0);
        assert_eq!(hit(&keys, on_sharp), Some(37));
        // Same x below the sharp's reach: back to a natural (D38).
        let below_sharp = pos2(dims.white_w + 1.0, dims.black_h + 5.0);
        assert_eq!(hit(&keys, below_sharp), Some(38));
        // Far left edge is C36 itself.
        assert_eq!(hit(&keys, pos2(2.0, 30.0)), Some(36));
        // Off the end of the strip: nothing.
        assert_eq!(hit(&keys, pos2(10_000.0, 30.0)), None);
    }

    #[test]
    fn keys_span_the_reported_strip_width() {
        let keys = layout(pos2(0.0, 0.0), 36, 32, &PEDAL);
        let right = keys.iter().map(|k| k.rect.max.x).fold(0.0f32, f32::max);
        assert_eq!(right, strip_width(36, 32, &PEDAL));
    }
}
