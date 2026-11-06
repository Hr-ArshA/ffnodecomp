//! Right-hand preview viewport and generated-command inspection panel.

use eframe::egui::{self, Color32, RichText};

use super::App;
use crate::preview::engine::{PREVIEW_H, PREVIEW_W};

impl App {
    pub(crate) fn viewer(&mut self, parent: &mut egui::Ui) {
        egui::Panel::right("preview")
            .default_size(440.0)
            .resizable(true)
            .show(parent, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Preview");
                    ui.label(
                        RichText::new(format!("{PREVIEW_W}x{PREVIEW_H}"))
                            .small()
                            .weak(),
                    );
                });

                let mut timestamp = self.timestamp;
                if ui
                    .add(
                        egui::Slider::new(&mut timestamp, 0.0..=120.0)
                            .text("time (s)")
                            .fixed_decimals(2),
                    )
                    .changed()
                {
                    self.timestamp = timestamp;
                    self.touch();
                }

                ui.add_space(6.0);
                let width = ui.available_width().min(PREVIEW_W as f32).max(120.0);
                let height = width * PREVIEW_H as f32 / PREVIEW_W as f32;
                let size = egui::vec2(width, height);

                if let Some(texture) = &self.texture {
                    ui.add(egui::Image::new(egui::load::SizedTexture::new(
                        texture.id(),
                        size,
                    )));
                } else {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(
                        rect,
                        egui::CornerRadius::same(4),
                        Color32::from_rgb(12, 13, 16),
                    );
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "No preview frame",
                        egui::FontId::proportional(14.0),
                        Color32::from_gray(120),
                    );
                }

                if let Some(note) = &self.preview_note {
                    ui.colored_label(Color32::from_rgb(220, 190, 110), note);
                }
                if let Some(error) = &self.preview_error {
                    ui.add(
                        egui::Label::new(
                            RichText::new(error).color(Color32::from_rgb(235, 120, 120)),
                        )
                        .wrap(),
                    );
                }
                if let Some(error) = &self.compile_error {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("Graph: {error}"))
                                .color(Color32::from_rgb(235, 120, 120)),
                        )
                        .wrap(),
                    );
                }

                ui.separator();
                egui::CollapsingHeader::new("Generated FFmpeg command")
                    .default_open(true)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(240.0)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(self.command_text()).monospace().size(10.0),
                                    )
                                    .wrap(),
                                );
                            });
                    });

                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Preview extracts one frame with -frames:v 1 and reads raw RGBA \
                         from stdout. Old processes are killed when a newer request arrives.",
                    )
                    .small()
                    .weak(),
                );
            });
    }
}
