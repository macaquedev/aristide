//! Aristide's native console: an eframe/egui client of the local
//! server. Deliberately conservative widgets for v1 (this box is
//! headless, so the first visual run happens on a user machine); the
//! interesting parts — protocol, threading, state flow — are tested.

mod client;

use std::collections::BTreeMap;
use std::sync::mpsc;

use client::{Command, Snapshot, Update};
use eframe::egui;

const GOLD: egui::Color32 = egui::Color32::from_rgb(0xC8, 0xA2, 0x4A);

fn main() -> eframe::Result {
    let server = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:9669".into());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 640.0])
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
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, server: String) -> App {
        let (command_tx, command_rx) = mpsc::channel();
        let (update_tx, update_rx) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        client::spawn(server.clone(), command_rx, update_tx, move || {
            ctx.request_repaint();
        });

        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.selection.bg_fill = GOLD;
        style.visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::BLACK);
        cc.egui_ctx.set_style(style);

        App {
            commands: command_tx,
            updates: update_rx,
            snapshot: None,
            error: None,
            server,
            gain_edit: None,
        }
    }

    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(update) = self.updates.try_recv() {
            match update {
                Update::State(snapshot) => {
                    self.snapshot = Some(snapshot);
                    self.error = None;
                }
                Update::Error(message) => self.error = Some(message),
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("ARISTIDE").color(GOLD).strong());
                ui.label(egui::RichText::new(&self.server).weak().small());
            });
            ui.separator();

            if let Some(error) = &self.error {
                ui.colored_label(
                    egui::Color32::from_rgb(0xD0, 0x60, 0x50),
                    format!("server unreachable: {error}"),
                );
            }
            let Some(snapshot) = self.snapshot.clone() else {
                ui.label("waiting for aristide-server…");
                return;
            };

            egui::ScrollArea::vertical().show(ui, |ui| {
                // Stops, grouped by manual (BTreeMap: stable order).
                let mut by_manual: BTreeMap<&str, Vec<&client::StopState>> = BTreeMap::new();
                for stop in &snapshot.stops {
                    by_manual.entry(stop.manual.as_str()).or_default().push(stop);
                }
                for (manual, stops) in by_manual {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(manual).color(GOLD).small().strong());
                    ui.horizontal_wrapped(|ui| {
                        for stop in stops {
                            let mut on = stop.on;
                            if ui.toggle_value(&mut on, &stop.name).changed() {
                                self.send(Command::SetStop { id: stop.id, on });
                            }
                        }
                    });
                }

                if !snapshot.couplers.is_empty() {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Couplers").color(GOLD).small().strong());
                    ui.horizontal_wrapped(|ui| {
                        for coupler in &snapshot.couplers {
                            let mut on = coupler.on;
                            if ui.toggle_value(&mut on, &coupler.name).changed() {
                                self.send(Command::SetCoupler { idx: coupler.idx, on });
                            }
                        }
                    });
                }

                ui.add_space(10.0);
                ui.separator();
                ui.horizontal(|ui| {
                    let mut trem = snapshot.tremulant;
                    if ui.toggle_value(&mut trem, "Tremulant").changed() {
                        self.send(Command::SetTremulant(trem));
                    }

                    ui.add_space(16.0);
                    ui.label("Gain");
                    let mut gain = self.gain_edit.unwrap_or(snapshot.gain);
                    let slider = ui.add(
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
                });

                if let Some(tuning) = &snapshot.tuning {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.label(egui::RichText::new("Tuning").color(GOLD).small().strong());
                    ui.horizontal(|ui| {
                        let mut temperament = tuning.temperament.clone();
                        let mut changed = false;
                        egui::ComboBox::from_label("temperament")
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
                                        .selectable_value(
                                            &mut temperament,
                                            option.to_string(),
                                            option,
                                        )
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
                            self.send(Command::SetTuning {
                                temperament,
                                a4,
                                transpose,
                            });
                        }
                    });
                }
            });
        });
    }
}
