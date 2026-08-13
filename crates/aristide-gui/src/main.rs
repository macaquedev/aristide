//! Aristide's native console: an eframe/egui client of the local
//! server, laid out like a real drawknob console (GrandOrgue-style) —
//! stop jamb per division, coupler rockers, clickable manuals and
//! pedalboard, a swell shoe, and a settings drawer. All I/O stays on
//! the client thread; every control toggles optimistically so the
//! 250 ms poll never makes the console feel spongy.

mod client;
mod keyboard;
mod theme;
mod widgets;

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use client::{Command, Snapshot, Update};
use eframe::egui;

/// French console names for the demo's plain division labels.
fn division_display(name: &str) -> &str {
    match name {
        "Pedal" => "Pédale",
        "First Manual" => "Grand Orgue",
        "Second Manual" => "Récit",
        other => other,
    }
}

/// A control the user just flipped; shown as flipped until the server
/// confirms (or `PENDING_TTL` passes and we believe the server again).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ToggleKey {
    Stop(u32),
    Coupler(usize),
    Tremulant,
}

const PENDING_TTL: Duration = Duration::from_millis(2000);

fn main() -> eframe::Result {
    let server = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:9669".into());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 800.0])
            .with_min_inner_size([900.0, 620.0])
            .with_title("Aristide"),
        ..Default::default()
    };
    eframe::run_native(
        "Aristide",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, server)))),
    )
}

struct App {
    commands: mpsc::Sender<Command>,
    updates: mpsc::Receiver<Update>,
    snapshot: Option<Snapshot>,
    error: Option<String>,
    server: String,
    /// Local gain while the slider is held, so polling doesn't fight
    /// the drag.
    gain_edit: Option<f32>,
    /// Optimistic toggle states awaiting server confirmation.
    pending: BTreeMap<u32, (ToggleKey, bool, Instant)>,
    pending_seq: u32,
    /// The key the mouse is holding down, as (manual idx, midi).
    mouse_note: Option<(usize, u8)>,
    /// Swell shoe position while (and shortly after) the pointer works
    /// it — the server value follows with inertia.
    shoe_edit: Option<(usize, f32, Instant)>,
    settings_open: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, server: String) -> App {
        let (command_tx, command_rx) = mpsc::channel();
        let (update_tx, update_rx) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        client::spawn(server.clone(), command_rx, update_tx, move || {
            ctx.request_repaint();
        });

        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        cc.egui_ctx.all_styles_mut(|style| {
            style.visuals.selection.bg_fill = theme::GOLD;
            style.visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::BLACK);
            style.visuals.override_text_color = Some(theme::TEXT);
            style.visuals.widgets.inactive.bg_fill = theme::PANEL;
            style.visuals.widgets.hovered.bg_fill = theme::PANEL_EDGE;
        });

        App {
            commands: command_tx,
            updates: update_rx,
            snapshot: None,
            error: None,
            server,
            gain_edit: None,
            pending: BTreeMap::new(),
            pending_seq: 0,
            mouse_note: None,
            shoe_edit: None,
            settings_open: false,
        }
    }

    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Flip a toggle optimistically: remember the desired state and
    /// tell the server.
    fn flip(&mut self, key: ToggleKey, on: bool) {
        self.pending.retain(|_, (k, _, _)| *k != key);
        self.pending_seq += 1;
        self.pending.insert(self.pending_seq, (key, on, Instant::now()));
        self.send(match key {
            ToggleKey::Stop(id) => Command::SetStop { id, on },
            ToggleKey::Coupler(idx) => Command::SetCoupler { idx, on },
            ToggleKey::Tremulant => Command::SetTremulant(on),
        });
    }

    /// The state to display for a toggle: the pending flip if one is in
    /// flight, otherwise what the server last said.
    fn effective(&self, key: ToggleKey, server_says: bool) -> bool {
        self.pending
            .values()
            .find(|(k, _, _)| *k == key)
            .map(|(_, on, _)| *on)
            .unwrap_or(server_says)
    }

    /// Drop pending flips the server has confirmed (or that timed out).
    fn prune_pending(&mut self, snapshot: &Snapshot) {
        self.pending.retain(|_, (key, on, at)| {
            if at.elapsed() > PENDING_TTL {
                return false;
            }
            let confirmed = match key {
                ToggleKey::Stop(id) => {
                    snapshot.stops.iter().find(|s| s.id == *id).map(|s| s.on)
                }
                ToggleKey::Coupler(idx) => {
                    snapshot.couplers.iter().find(|c| c.idx == *idx).map(|c| c.on)
                }
                ToggleKey::Tremulant => Some(snapshot.tremulant),
            };
            confirmed != Some(*on)
        });
        if let Some((idx, value, at)) = self.shoe_edit {
            let settled = snapshot
                .enclosures
                .iter()
                .find(|e| e.idx == idx)
                .is_some_and(|e| (e.value - value).abs() < 0.02);
            if at.elapsed() > PENDING_TTL || settled {
                self.shoe_edit = None;
            }
        }
    }

    // ----- panels -------------------------------------------------------

    fn header(&mut self, ui: &mut egui::Ui, snapshot: Option<&Snapshot>) {
        ui.horizontal(|ui| {
            let (dot, tip) = if self.error.is_some() {
                (theme::ERR, "server unreachable")
            } else if self.snapshot.is_some() {
                (theme::OK, "connected")
            } else {
                (theme::TEXT_DIM, "connecting…")
            };
            let (dot_rect, dot_resp) =
                ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
            ui.painter().circle_filled(dot_rect.center(), 5.0, dot);
            dot_resp.on_hover_text(tip);

            ui.heading(egui::RichText::new("ARISTIDE").color(theme::GOLD).strong());
            if let Some(name) = snapshot.and_then(|s| s.organ.as_deref()) {
                ui.label(egui::RichText::new(name).color(theme::TEXT_DIM).italics());
            }
            if let Some(error) = &self.error {
                ui.label(egui::RichText::new(error).color(theme::ERR).small());
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⚙ Settings").clicked() {
                    self.settings_open = !self.settings_open;
                }
                if ui
                    .button(egui::RichText::new("Panic").color(theme::ERR))
                    .on_hover_text("silence everything (all notes off)")
                    .clicked()
                {
                    self.send(Command::Panic);
                }
                ui.add_space(10.0);

                if let Some(snapshot) = snapshot {
                    let trem = self.effective(ToggleKey::Tremulant, snapshot.tremulant);
                    if widgets::rocker(ui, "Trem", trem).clicked() {
                        self.flip(ToggleKey::Tremulant, !trem);
                    }
                    ui.add_space(10.0);

                    let mut gain = self.gain_edit.unwrap_or(snapshot.gain);
                    let slider = ui.add_sized(
                        [150.0, 18.0],
                        egui::Slider::new(&mut gain, 0.05..=1.2)
                            .fixed_decimals(2)
                            .trailing_fill(true),
                    );
                    if slider.changed() {
                        self.gain_edit = Some(gain);
                    }
                    if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
                        self.send(Command::SetGain(gain));
                        self.gain_edit = None;
                    }
                    ui.label(egui::RichText::new("Gain").color(theme::TEXT_DIM));
                }
            });
        });
    }

    /// The stop jamb: one column of drawknobs per division, couplers
    /// beneath.
    fn jamb(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        // Column order follows the server's manual order (Pédale, Grand
        // Orgue, Récit); stops from divisions without a keyboard still
        // get a column at the end.
        let mut order: Vec<&str> = snapshot.manuals.iter().map(|m| m.name.as_str()).collect();
        let mut by_division: BTreeMap<&str, Vec<&client::StopState>> = BTreeMap::new();
        for stop in &snapshot.stops {
            by_division.entry(stop.manual.as_str()).or_default().push(stop);
        }
        for division in by_division.keys() {
            if !order.contains(division) {
                order.push(division);
            }
        }
        order.retain(|division| by_division.contains_key(division));
        if order.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new("no stops — tone mode").color(theme::TEXT_DIM));
            });
            return;
        }

        let mut flips: Vec<(ToggleKey, bool)> = Vec::new();
        ui.add_space(8.0);
        ui.columns(order.len(), |columns| {
            for (column, division) in columns.iter_mut().zip(&order) {
                column.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(division_display(division).to_uppercase())
                            .color(theme::GOLD)
                            .size(14.0)
                            .strong(),
                    );
                });
                column.add_space(6.0);
                column.horizontal_wrapped(|ui| {
                    for stop in &by_division[*division] {
                        let key = ToggleKey::Stop(stop.id);
                        let on = self.effective(key, stop.on);
                        if widgets::drawknob(ui, &stop.name, on).clicked() {
                            flips.push((key, !on));
                        }
                    }
                });
            }
        });

        if !snapshot.couplers.is_empty() {
            ui.add_space(12.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("COUPLERS")
                        .color(theme::TEXT_DIM)
                        .size(11.0)
                        .strong(),
                );
                for coupler in &snapshot.couplers {
                    let key = ToggleKey::Coupler(coupler.idx);
                    let on = self.effective(key, coupler.on);
                    if widgets::rocker(ui, &coupler.name, on).clicked() {
                        flips.push((key, !on));
                    }
                }
            });
        }
        for (key, on) in flips {
            self.flip(key, on);
        }
    }

    /// The playing console: manuals stacked top-manual-first, the
    /// pedalboard under them, swell shoes to the right.
    fn console(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        if snapshot.manuals.is_empty() {
            return;
        }
        let mut down: Option<(usize, u8)> = None;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                // Highest manual on top, pedalboard last — as seated.
                let mut rows: Vec<&client::ManualState> = snapshot.manuals.iter().collect();
                rows.sort_by_key(|m| std::cmp::Reverse(m.idx));
                let is_pedal = |m: &client::ManualState| m.name == "Pedal" || m.idx == 0;
                rows.sort_by_key(|m| is_pedal(m));
                for manual in rows {
                    let dims = if is_pedal(manual) { &keyboard::PEDAL } else { &keyboard::MANUAL };
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [92.0, dims.white_h],
                            egui::Label::new(
                                egui::RichText::new(division_display(&manual.name))
                                    .color(theme::GOLD)
                                    .size(12.0),
                            ),
                        );
                        let mouse_note = self.mouse_note;
                        let held = |key: u8| {
                            manual.held.contains(&key)
                                || mouse_note == Some((manual.idx, key))
                        };
                        let (_, strip_down) = keyboard::strip(
                            ui,
                            manual.first_key,
                            manual.key_count,
                            dims,
                            &held,
                        );
                        if let Some(key) = strip_down {
                            down = Some((manual.idx, key));
                        }
                    });
                    ui.add_space(4.0);
                }
            });

            // Swell shoes for the enclosures the organ displays.
            for enclosure in snapshot.enclosures.iter().filter(|e| e.displayed) {
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    let shown = match self.shoe_edit {
                        Some((idx, value, _)) if idx == enclosure.idx => value,
                        _ => enclosure.value,
                    };
                    let (_, new_value) = widgets::swell_shoe(ui, shown);
                    if let Some(value) = new_value {
                        if (value - shown).abs() > 0.004 {
                            self.send(Command::SetEnclosure { idx: enclosure.idx, value });
                        }
                        self.shoe_edit = Some((enclosure.idx, value, Instant::now()));
                    }
                    ui.label(
                        egui::RichText::new(division_display(&enclosure.name))
                            .color(theme::TEXT_DIM)
                            .size(11.0),
                    );
                });
            }
        });

        // One mouse, one note: release the old key, press the new.
        if down != self.mouse_note {
            if let Some((manual, key)) = self.mouse_note {
                self.send(Command::Note { manual, key, on: false });
            }
            if let Some((manual, key)) = down {
                self.send(Command::Note { manual, key, on: true });
            }
            self.mouse_note = down;
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        ui.add_space(4.0);
        ui.label(egui::RichText::new("SETTINGS").color(theme::GOLD).strong());
        ui.separator();

        if let Some(tuning) = &snapshot.tuning {
            ui.label(egui::RichText::new("Tuning").color(theme::TEXT_DIM).small());
            let mut temperament = tuning.temperament.clone();
            let mut changed = false;
            egui::ComboBox::from_id_salt("temperament")
                .selected_text(&temperament)
                .show_ui(ui, |ui| {
                    for option in [
                        "equal",
                        "werckmeister3",
                        "kirnberger3",
                        "meantone4",
                        "pythagorean",
                    ] {
                        changed |= ui
                            .selectable_value(&mut temperament, option.to_string(), option)
                            .changed();
                    }
                });

            let mut a4 = tuning.a4;
            changed |= ui
                .add(
                    egui::DragValue::new(&mut a4)
                        .range(300.0..=500.0)
                        .speed(0.5)
                        .prefix("a′ ")
                        .suffix(" Hz"),
                )
                .changed();

            let mut transpose = tuning.transpose;
            changed |= ui
                .add(
                    egui::DragValue::new(&mut transpose)
                        .range(-12..=12)
                        .prefix("transpose "),
                )
                .changed();

            if changed {
                self.send(Command::SetTuning { temperament, a4, transpose });
            }
            ui.add_space(8.0);
        }

        if let Some(current) = snapshot.reverb {
            ui.label(egui::RichText::new("Reverb").color(theme::TEXT_DIM).small());
            let mut wet = current;
            let slider = ui.add(
                egui::Slider::new(&mut wet, 0.0..=1.0)
                    .fixed_decimals(2)
                    .trailing_fill(true),
            );
            if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
                self.send(Command::SetReverb(wet));
            }
            ui.add_space(8.0);
        }

        if let Some(noises) = &snapshot.noises {
            ui.label(
                egui::RichText::new("Action noises")
                    .color(theme::TEXT_DIM)
                    .small(),
            );
            let mut on = noises.on;
            if ui.toggle_value(&mut on, "enabled").changed() {
                self.send(Command::SetNoises { on, vol: noises.vol });
            }
            let mut vol = noises.vol;
            let slider = ui.add(egui::Slider::new(&mut vol, 0.0..=1.5).fixed_decimals(2));
            if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
                self.send(Command::SetNoises { on: noises.on, vol });
            }
            ui.add_space(8.0);
        }

        ui.separator();
        ui.label(
            egui::RichText::new(&self.server)
                .color(theme::TEXT_DIM)
                .small(),
        );
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        while let Ok(update) = self.updates.try_recv() {
            match update {
                Update::State(snapshot) => {
                    self.prune_pending(&snapshot);
                    self.snapshot = Some(snapshot);
                    self.error = None;
                }
                Update::Error(message) => self.error = Some(message),
            }
        }

        root.painter()
            .rect_filled(root.max_rect(), egui::CornerRadius::ZERO, theme::BG);

        let snapshot = self.snapshot.clone();
        let panel_frame = egui::Frame::default()
            .fill(theme::PANEL)
            .inner_margin(egui::Margin::same(8));

        egui::Panel::top("header")
            .frame(panel_frame)
            .show(root, |ui| self.header(ui, snapshot.as_ref()));

        let Some(snapshot) = snapshot else {
            root.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new(format!("waiting for aristide-server at {}…", self.server))
                        .color(theme::TEXT_DIM),
                );
            });
            return;
        };

        egui::Panel::bottom("console")
            .frame(panel_frame)
            .show(root, |ui| self.console(ui, &snapshot));

        let mut settings_open = self.settings_open;
        egui::Panel::right("settings")
            .frame(panel_frame)
            .resizable(false)
            .default_size(230.0)
            .show_collapsible(root, &mut settings_open, |ui| self.settings(ui, &snapshot));
        self.settings_open = settings_open;

        egui::CentralPanel::no_frame()
            .frame(egui::Frame::default().inner_margin(egui::Margin::same(10)))
            .show(root, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.jamb(ui, &snapshot));
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn division_names_read_like_a_console() {
        assert_eq!(division_display("Pedal"), "Pédale");
        assert_eq!(division_display("First Manual"), "Grand Orgue");
        assert_eq!(division_display("Second Manual"), "Récit");
        assert_eq!(division_display("Echo"), "Echo");
    }
}
