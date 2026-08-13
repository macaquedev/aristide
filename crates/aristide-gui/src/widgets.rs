//! Hand-drawn console furniture: drawknobs, coupler rockers and the
//! swell shoe. Pure egui painting — no textures — so the console stays
//! crisp at any DPI and costs nothing to render.

use eframe::egui::{
    self, Align, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui,
    text::LayoutJob, vec2,
};

use crate::theme;

/// A stop name as engraved on the knob: the ODF marks intended line
/// breaks with "- " ("Contre- basse 16'"), and the footage reads best
/// on its own line.
pub fn engrave_stop_name(name: &str) -> String {
    name.replace("- ", "-\n")
}

/// A round drawknob: pulled out (ivory face, gold rim) when the stop is
/// drawn, pushed into its socket when silent.
pub fn drawknob(ui: &mut Ui, name: &str, on: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(76.0, 76.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let center = rect.center();
    let socket_r = rect.width() * 0.5 - 4.0;
    painter.circle_filled(center, socket_r, theme::KNOB_SOCKET);

    let (face_r, face, rim, lettering) = if on {
        (socket_r - 1.0, theme::IVORY, theme::GOLD, theme::ENGRAVE)
    } else {
        (socket_r - 5.0, theme::KNOB_OFF, theme::KNOB_OFF_RIM, theme::ENGRAVE.gamma_multiply(0.8))
    };
    painter.circle_filled(center, face_r, face);
    painter.circle_stroke(center, face_r, Stroke::new(2.0, rim));
    // Turned ring near the edge, like a lathed knob.
    painter.circle_stroke(center, face_r - 4.0, Stroke::new(1.0, rim.gamma_multiply(0.55)));
    if response.hovered() || response.has_focus() {
        painter.circle_stroke(center, socket_r + 1.0, Stroke::new(1.5, theme::GOLD));
    }

    let mut job = LayoutJob::simple(
        engrave_stop_name(name),
        FontId::proportional(10.0),
        lettering,
        face_r * 1.6,
    );
    job.halign = Align::Center;
    let galley = painter.layout_job(job);
    let anchor = Pos2::new(center.x, center.y - galley.size().y * 0.5);
    painter.galley(anchor, galley, lettering);
    response
}

/// A coupler rocker tab: a small plate whose top edge lights up when
/// the coupler is engaged.
pub fn rocker(ui: &mut Ui, name: &str, on: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(66.0, 40.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let body = rect.shrink(2.0);
    let rounding = egui::CornerRadius::same(4);
    painter.rect_filled(body, rounding, if on { theme::GOLD_DIM } else { theme::PANEL });
    let edge = if response.hovered() { theme::GOLD } else { theme::PANEL_EDGE };
    painter.rect_stroke(body, rounding, Stroke::new(1.0, edge), StrokeKind::Inside);
    // Indicator bar along the top edge.
    let bar = Rect::from_min_max(
        body.min + vec2(6.0, 4.0),
        Pos2::new(body.max.x - 6.0, body.min.y + 7.0),
    );
    painter.rect_filled(
        bar,
        egui::CornerRadius::same(2),
        if on { theme::GOLD } else { theme::KNOB_SOCKET },
    );
    painter.text(
        body.center() + vec2(0.0, 4.0),
        egui::Align2::CENTER_CENTER,
        name,
        FontId::proportional(11.0),
        if on { theme::IVORY } else { theme::TEXT },
    );
    response
}

/// A balanced swell shoe: vertical, open at the top. Returns the new
/// position (0 = closed, 1 = open) while the pointer works it.
pub fn swell_shoe(ui: &mut Ui, value: f32) -> (Response, Option<f32>) {
    let (rect, response) = ui.allocate_exact_size(vec2(44.0, 132.0), Sense::click_and_drag());
    let track = rect.shrink2(vec2(10.0, 6.0));

    let mut new_value = None;
    if response.is_pointer_button_down_on() {
        if let Some(pos) = response.interact_pointer_pos() {
            new_value = Some((1.0 - (pos.y - track.top()) / track.height()).clamp(0.0, 1.0));
        }
    }
    let shown = new_value.unwrap_or(value).clamp(0.0, 1.0);

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let rounding = egui::CornerRadius::same(4);
        painter.rect_filled(track.expand(3.0), rounding, theme::KNOB_SOCKET);
        // Openness fills with gold from the bottom.
        let fill_top = track.top() + track.height() * (1.0 - shown);
        let fill = Rect::from_min_max(Pos2::new(track.left(), fill_top), track.max);
        painter.rect_filled(fill, egui::CornerRadius::same(2), theme::GOLD_DIM);
        // The shoe itself: a wide bar at the current position.
        let shoe = Rect::from_center_size(
            Pos2::new(track.center().x, fill_top),
            vec2(track.width() + 12.0, 9.0),
        );
        painter.rect_filled(shoe, egui::CornerRadius::same(3), theme::IVORY);
        painter.rect_stroke(
            shoe,
            egui::CornerRadius::same(3),
            Stroke::new(1.0, if response.hovered() { theme::GOLD } else { theme::ENGRAVE }),
            StrokeKind::Inside,
        );
    }
    (response, new_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_names_break_where_the_odf_marks_them() {
        assert_eq!(engrave_stop_name("Contre- basse 16'"), "Contre-\nbasse 16'");
        assert_eq!(engrave_stop_name("Montre 8'"), "Montre 8'");
    }
}
