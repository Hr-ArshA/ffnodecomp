//! Minimal node canvas. Hand-rolled because egui_node_graph2 is still pinned
//! to egui 0.29 while this app runs on eframe 0.37.

use std::collections::HashMap;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};

use super::App;
use crate::graph::node::{compatible, Node, NodeId, NodeKind, PortKind};

const NODE_W: f32 = 190.0;
const HEADER_H: f32 = 28.0;
const ROW_H: f32 = 22.0;
const PORT_R: f32 = 5.5;
const GRID: f32 = 40.0;

impl App {
    pub(crate) fn canvas(&mut self, ui: &mut egui::Ui) {
        let (area, canvas_response) = ui.allocate_exact_size(ui.available_size(), Sense::click());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, CornerRadius::ZERO, Color32::from_rgb(20, 22, 27));

        let mut x = area.left();
        while x < area.right() {
            painter.line_segment(
                [egui::pos2(x, area.top()), egui::pos2(x, area.bottom())],
                Stroke::new(1.0, Color32::from_rgb(27, 30, 37)),
            );
            x += GRID;
        }
        let mut y = area.top();
        while y < area.bottom() {
            painter.line_segment(
                [egui::pos2(area.left(), y), egui::pos2(area.right(), y)],
                Stroke::new(1.0, Color32::from_rgb(27, 30, 37)),
            );
            y += GRID;
        }

        // Cache port positions first: links are drawn before node bodies and
        // need both endpoints.
        let mut inputs: HashMap<(NodeId, usize), Pos2> = HashMap::new();
        let mut outputs: HashMap<(NodeId, usize), Pos2> = HashMap::new();
        for node in &self.graph.nodes {
            let rect = rect_for(node, area.min);
            for (index, _) in node.kind.inputs().iter().enumerate() {
                inputs.insert((node.id, index), port_center(rect, true, index));
            }
            for (index, _) in node.kind.outputs().iter().enumerate() {
                outputs.insert((node.id, index), port_center(rect, false, index));
            }
        }

        // Hit test wires against pointer click or right-click.
        let pointer_pos = ui.ctx().pointer_latest_pos();
        let mut clicked_edge = None;
        let mut right_clicked_edge = None;

        let over_node = pointer_pos.is_some_and(|pos| {
            self.graph
                .nodes
                .iter()
                .any(|node| rect_for(node, area.min).contains(pos))
        });

        if let Some(pos) = pointer_pos {
            if !over_node {
                let mut best_dist = 10.0; // 10px hit radius
                for (index, link) in self.graph.links.iter().enumerate() {
                    if let (Some(from), Some(to)) = (
                        outputs.get(&(link.from, link.from_port)),
                        inputs.get(&(link.to, link.to_port)),
                    ) {
                        let d = bezier_distance(*from, *to, pos);
                        if d < best_dist {
                            best_dist = d;
                            if ui.input(|i| i.pointer.primary_clicked()) {
                                clicked_edge = Some(index);
                            }
                            if ui.input(|i| i.pointer.secondary_clicked()) {
                                right_clicked_edge = Some(index);
                            }
                        }
                    }
                }
            }
        }

        if let Some(edge) = clicked_edge {
            self.selected_edge = Some(edge);
            self.selected = None;
        }

        // Right-click context menu for edge deletion
        if let Some(edge) = right_clicked_edge {
            self.selected_edge = Some(edge);
            canvas_response.context_menu(|ui| {
                if ui.button("Delete Connection").clicked() {
                    let _ = self.graph.disconnect_edge(edge);
                    self.selected_edge = None;
                    self.touch();
                    ui.close();
                }
            });
        }

        // Draw links
        for (index, link) in self.graph.links.iter().enumerate() {
            if let (Some(from), Some(to)) = (
                outputs.get(&(link.from, link.from_port)),
                inputs.get(&(link.to, link.to_port)),
            ) {
                let port_kind = self
                    .graph
                    .node(link.from)
                    .and_then(|n| n.kind.outputs().get(link.from_port).map(|p| p.kind))
                    .unwrap_or(PortKind::Video);

                let is_selected = self.selected_edge == Some(index);

                if is_selected {
                    draw_bezier(
                        &painter,
                        *from,
                        *to,
                        Stroke::new(5.0, Color32::from_rgb(255, 220, 100)),
                    );
                }
                draw_bezier(
                    &painter,
                    *from,
                    *to,
                    Stroke::new(2.5, wire_color(port_kind)),
                );
            }
        }

        // Draw pending wire while user is connecting or dragging an edge
        if let Some((id, port)) = self.pending_link {
            if let (Some(from), Some(pointer)) = (outputs.get(&(id, port)), pointer_pos) {
                let port_kind = self
                    .graph
                    .node(id)
                    .and_then(|n| n.kind.outputs().get(port).map(|p| p.kind))
                    .unwrap_or(PortKind::Video);

                draw_bezier(
                    &painter,
                    *from,
                    pointer,
                    Stroke::new(2.5, wire_color(port_kind)),
                );
            }
        }

        // Get source port info for port validity tinting
        let pending_source = self.pending_link.and_then(|(src_id, src_port)| {
            self.graph
                .node(src_id)
                .and_then(|n| n.kind.outputs().get(src_port).map(|p| (src_id, p.kind)))
        });

        let ids: Vec<NodeId> = self.graph.nodes.iter().map(|node| node.id).collect();
        let mut clicked_input = None;
        let mut clicked_output = None;

        for id in ids {
            let node_rect = rect_for(&self.graph.nodes[id], area.min);
            let kind = self.graph.nodes[id].kind.clone();
            let inline_h = inline_height(&kind);
            let inline_top = kind.inputs().len().max(kind.outputs().len()).max(1) as f32 * ROW_H;
            let content = Rect::from_min_size(
                egui::pos2(
                    node_rect.left() + 8.0,
                    node_rect.top() + HEADER_H + inline_top + 4.0,
                ),
                egui::vec2(NODE_W - 16.0, inline_h),
            );

            let accent = match kind {
                NodeKind::Input { .. } => Color32::from_rgb(70, 160, 165),
                NodeKind::Transform { .. } => Color32::from_rgb(130, 110, 200),
                NodeKind::Color { .. } => Color32::from_rgb(190, 135, 70),
                NodeKind::Overlay { .. } => Color32::from_rgb(185, 95, 145),
                NodeKind::ColorKey { .. } => Color32::from_rgb(80, 175, 120),
                NodeKind::AlphaKey { .. } => Color32::from_rgb(140, 150, 200),
                NodeKind::Text { .. } => Color32::from_rgb(210, 120, 160),
                NodeKind::Image { .. } => Color32::from_rgb(90, 180, 140),
                NodeKind::SolidColor { .. } => Color32::from_rgb(180, 160, 80),
                NodeKind::Blur { .. } => Color32::from_rgb(100, 140, 190),
                NodeKind::Sharpen { .. } => Color32::from_rgb(195, 120, 80),
                NodeKind::Lut3d { .. } => Color32::from_rgb(150, 120, 200),
                NodeKind::ColorBalance { .. } => Color32::from_rgb(200, 150, 90),
                NodeKind::Levels { .. } => Color32::from_rgb(120, 175, 200),
                NodeKind::InvertSolarize { .. } => Color32::from_rgb(200, 100, 130),
                NodeKind::Output { .. } => Color32::from_rgb(90, 170, 110),
            };

            painter.rect_filled(
                node_rect,
                CornerRadius::same(6),
                Color32::from_rgb(36, 40, 48),
            );
            painter.rect_filled(
                Rect::from_min_size(node_rect.min, egui::vec2(NODE_W, HEADER_H)),
                CornerRadius::same(6),
                accent.gamma_multiply(0.35),
            );
            painter.rect_stroke(
                node_rect,
                CornerRadius::same(6),
                Stroke::new(
                    if self.selected == Some(id) { 2.0 } else { 1.0 },
                    if self.selected == Some(id) {
                        Color32::from_rgb(245, 205, 110)
                    } else {
                        Color32::from_rgb(68, 74, 88)
                    },
                ),
                StrokeKind::Inside,
            );

            painter.text(
                node_rect.left_top() + egui::vec2(10.0, 6.0),
                Align2::LEFT_TOP,
                kind.title(),
                FontId::proportional(14.0),
                Color32::WHITE,
            );
            painter.text(
                node_rect.left_bottom() + egui::vec2(10.0, -22.0),
                Align2::LEFT_TOP,
                kind.summary(),
                FontId::proportional(10.5),
                Color32::from_gray(165),
            );

            // Body click selects node
            let body = ui.interact(node_rect, egui::Id::unique(("node", id)), Sense::click());
            if body.clicked() {
                self.selected = Some(id);
                self.selected_edge = None;
            }

            let header_rect = Rect::from_min_size(node_rect.min, egui::vec2(NODE_W, HEADER_H));
            let header = ui.interact(
                header_rect,
                egui::Id::unique(("header", id)),
                Sense::click_and_drag(),
            );
            if header.drag_started() {
                if let Some(pointer) = header.interact_pointer_pos() {
                    self.dragging = Some((id, pointer - node_rect.min));
                    self.selected = Some(id);
                    self.selected_edge = None;
                }
            }
            if header.dragged() {
                if let Some((drag_id, offset)) = self.dragging {
                    if drag_id == id {
                        if let Some(pointer) = header.interact_pointer_pos() {
                            let position = pointer - offset - area.min;
                            self.graph.nodes[id].pos = [position.x.max(0.0), position.y.max(0.0)];
                            self.touch();
                        }
                    }
                }
            }
            if header.drag_stopped() {
                self.dragging = None;
            }

            // Input Ports
            for (index, port) in kind.inputs().iter().enumerate() {
                let center = port_center(node_rect, true, index);

                // Port validity tinting during pending link drag/wire creation
                let (fill, r) = if let Some((src_id, src_kind)) = pending_source {
                    let is_valid =
                        compatible(src_kind, port.kind) && !self.graph.would_cycle(src_id, id);
                    if is_valid {
                        (Color32::from_rgb(100, 230, 130), PORT_R + 1.5)
                    } else {
                        (Color32::from_rgb(230, 80, 80), PORT_R)
                    }
                } else {
                    (port_color(port.kind), PORT_R)
                };

                painter.circle_filled(center, r, fill);
                painter.circle_stroke(center, r, Stroke::new(1.0, Color32::BLACK));
                painter.text(
                    center + egui::vec2(11.0, -7.0),
                    Align2::LEFT_TOP,
                    port.name,
                    FontId::proportional(10.0),
                    Color32::from_gray(175),
                );

                let response = ui
                    .interact(
                        Rect::from_center_size(center, Vec2::splat(20.0)),
                        egui::Id::unique(("in", id, index)),
                        Sense::click_and_drag(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);

                // Drag away from connected input port: disconnect & drag wire
                if response.drag_started() {
                    if let Some(link_pos) = self
                        .graph
                        .links
                        .iter()
                        .position(|l| l.to == id && l.to_port == index)
                    {
                        let link = self.graph.links[link_pos].clone();
                        let _ = self.graph.disconnect_edge(link_pos);
                        self.selected_edge = None;
                        self.pending_link = Some((link.from, link.from_port));
                        self.wire_drag = true;
                        self.touch();
                    }
                }

                if response.clicked() {
                    clicked_input = Some((id, index));
                }
            }

            // Output Ports
            for (index, port) in kind.outputs().iter().enumerate() {
                let center = port_center(node_rect, false, index);
                painter.circle_filled(center, PORT_R, port_color(port.kind));
                painter.circle_stroke(center, PORT_R, Stroke::new(1.0, Color32::BLACK));
                let response = ui
                    .interact(
                        Rect::from_center_size(center, Vec2::splat(20.0)),
                        egui::Id::unique(("out", id, index)),
                        Sense::click(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if response.clicked() {
                    clicked_output = Some((id, index));
                }
            }

            // Inline controls
            if inline_h > 0.0 {
                let mut changed = false;
                ui.push_id(("inline", id), |ui| {
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(content)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                        |ui| {
                            ui.spacing_mut().slider_width = 70.0;
                            ui.spacing_mut().item_spacing.y = 2.0;
                            changed |= inline_controls(ui, id, &mut self.graph.nodes[id].kind);
                        },
                    );
                });
                if changed {
                    self.touch();
                }
            }
        }

        // Output port click toggles pending link
        if let Some((id, port)) = clicked_output {
            self.pending_link = if self.pending_link == Some((id, port)) {
                None
            } else {
                Some((id, port))
            };
            self.wire_drag = false;
        }

        // Input port click connects
        if let Some((id, port)) = clicked_input {
            if let Some((from, from_port)) = self.pending_link.take() {
                match self.graph.connect(from, from_port, id, port) {
                    Ok(()) => {
                        self.selected_edge = None;
                        self.touch();
                        self.notify("Connected", true);
                    }
                    Err(error) => self.notify(error, false),
                }
            } else {
                self.notify("Click an output port first", false);
            }
            self.wire_drag = false;
        }

        // Pointer released handling for wire drag-away lifecycle
        if self.wire_drag && ui.input(|i| i.pointer.any_released()) {
            if let (Some((from, from_port)), Some(pos)) = (self.pending_link, pointer_pos) {
                // Check if released over an input port
                let mut reconnected = false;
                for (&(target_id, target_port), &center) in &inputs {
                    if center.distance(pos) <= 14.0 {
                        match self.graph.connect(from, from_port, target_id, target_port) {
                            Ok(()) => {
                                self.notify("Connected", true);
                            }
                            Err(error) => self.notify(error, false),
                        }
                        reconnected = true;
                        break;
                    }
                }
                if !reconnected {
                    self.notify("Disconnected", true);
                }
                self.touch();
            }
            self.pending_link = None;
            self.wire_drag = false;
        }

        // Delete / Backspace key handler removes selected edge or node
        if !ui.ctx().egui_wants_keyboard_input()
            && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        {
            if let Some(edge) = self.selected_edge.take() {
                let _ = self.graph.disconnect_edge(edge);
                self.touch();
                self.notify("Deleted edge", true);
            } else if let Some(id) = self.selected {
                self.graph.remove_node(id);
                self.selected = None;
                self.selected_edge = None;
                self.pending_link = None;
                self.wire_drag = false;
                self.dragging = None;
                self.touch();
                self.notify("Deleted node", true);
            }
        }
    }
}

fn rect_for(node: &Node, origin: Pos2) -> Rect {
    let rows = node
        .kind
        .inputs()
        .len()
        .max(node.kind.outputs().len())
        .max(1) as f32;
    let height = HEADER_H + rows * ROW_H + inline_height(&node.kind) + 30.0;
    Rect::from_min_size(
        origin + egui::vec2(node.pos[0], node.pos[1]),
        egui::vec2(NODE_W, height),
    )
}

fn inline_height(kind: &NodeKind) -> f32 {
    match kind {
        NodeKind::Input { .. } => 24.0,
        NodeKind::Image { .. } => 24.0,
        NodeKind::ColorKey { .. } => 64.0,
        NodeKind::AlphaKey { .. } => 84.0,
        _ => 0.0,
    }
}

fn inline_controls(ui: &mut egui::Ui, id: NodeId, kind: &mut NodeKind) -> bool {
    use crate::graph::node::{hex_color, KeyMode};
    let mut changed = false;

    match kind {
        NodeKind::Input { path } => {
            let label = if path.trim().is_empty() {
                "Select video..."
            } else {
                "Change video..."
            };
            if ui.button(egui::RichText::new(label).size(10.0)).clicked() {
                let mut dialog = rfd::FileDialog::new()
                    .add_filter("Video", &["mp4", "mkv", "mov", "avi", "webm", "flv", "m4v"]);
                if let Some(parent) = std::path::Path::new(path).parent() {
                    if parent.exists() && parent != std::path::Path::new("") {
                        dialog = dialog.set_directory(parent);
                    }
                }
                if let Some(file) = dialog.pick_file() {
                    *path = file.to_string_lossy().into_owned();
                    changed = true;
                }
            }
        }
        NodeKind::Image { path, .. } => {
            let label = if path.trim().is_empty() {
                "Select image..."
            } else {
                "Change image..."
            };
            if ui.button(egui::RichText::new(label).size(10.0)).clicked() {
                let mut dialog = rfd::FileDialog::new()
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp", "gif"]);
                if let Some(parent) = std::path::Path::new(path).parent() {
                    if parent.exists() && parent != std::path::Path::new("") {
                        dialog = dialog.set_directory(parent);
                    }
                }
                if let Some(file) = dialog.pick_file() {
                    *path = file.to_string_lossy().into_owned();
                    changed = true;
                }
            }
        }
        NodeKind::ColorKey {
            color,
            similarity,
            blend,
        } => {
            ui.horizontal(|ui| {
                changed |= ui.color_edit_button_srgb(color).changed();
                ui.label(egui::RichText::new(hex_color(*color)).monospace().size(9.0));
                if ui.small_button("G").on_hover_text("Green screen").clicked() {
                    *color = [0, 255, 0];
                    changed = true;
                }
                if ui.small_button("B").on_hover_text("Blue screen").clicked() {
                    *color = [0, 0, 255];
                    changed = true;
                }
            });
            changed |= slider_row(ui, "Sim", similarity, 0.01..=1.0);
            changed |= slider_row(ui, "Blend", blend, 0.0..=1.0);
        }
        NodeKind::AlphaKey {
            mode,
            threshold,
            tolerance,
            softness,
        } => {
            egui::ComboBox::from_id_salt(("keymode", id))
                .selected_text(mode.label())
                .width(120.0)
                .show_ui(ui, |ui| {
                    for option in KeyMode::ALL {
                        if ui.selectable_value(mode, option, option.label()).clicked() {
                            changed = true;
                        }
                    }
                });
            changed |= slider_row(ui, "Thr", threshold, 0.0..=1.0);
            changed |= slider_row(ui, "Tol", tolerance, 0.0..=1.0);
            changed |= slider_row(ui, "Soft", softness, 0.0..=1.0);
        }
        _ => {}
    }

    changed
}

fn slider_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(9.0));
        ui.add(egui::Slider::new(value, range).fixed_decimals(2))
            .changed()
    })
    .inner
}

fn port_center(rect: Rect, input: bool, index: usize) -> Pos2 {
    let y = rect.top() + HEADER_H + 14.0 + index as f32 * ROW_H;
    let x = if input { rect.left() } else { rect.right() };
    egui::pos2(x, y)
}

fn port_color(kind: PortKind) -> Color32 {
    match kind {
        PortKind::Video => Color32::from_rgb(240, 140, 50), // Orange for Video
        PortKind::Audio => Color32::from_rgb(100, 210, 120), // Green for Audio
        PortKind::Mask => Color32::from_rgb(220, 90, 200),  // Magenta for Mask
    }
}

fn wire_color(kind: PortKind) -> Color32 {
    port_color(kind)
}

fn draw_bezier(painter: &egui::Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    let bend = ((to.x - from.x).abs() * 0.5).max(60.0);
    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
        [
            from,
            from + egui::vec2(bend, 0.0),
            to - egui::vec2(bend, 0.0),
            to,
        ],
        false,
        Color32::TRANSPARENT,
        stroke,
    ));
}

fn bezier_distance(from: Pos2, to: Pos2, p: Pos2) -> f32 {
    let bend = ((to.x - from.x).abs() * 0.5).max(60.0);
    let p0 = from;
    let p1 = from + egui::vec2(bend, 0.0);
    let p2 = to - egui::vec2(bend, 0.0);
    let p3 = to;

    let mut min_dist = f32::INFINITY;
    let mut prev = p0;

    for i in 1..=20 {
        let t = i as f32 / 20.0;
        let mt = 1.0 - t;
        let cur = egui::pos2(
            p0.x * (mt * mt * mt)
                + p1.x * (3.0 * mt * mt * t)
                + p2.x * (3.0 * mt * t * t)
                + p3.x * (t * t * t),
            p0.y * (mt * mt * mt)
                + p1.y * (3.0 * mt * mt * t)
                + p2.y * (3.0 * mt * t * t)
                + p3.y * (t * t * t),
        );

        let dist = dist_to_segment(p, prev, cur);
        if dist < min_dist {
            min_dist = dist;
        }
        prev = cur;
    }

    min_dist
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let len_sq = ab.length_sq();
    if len_sq == 0.0 {
        return ap.length();
    }
    let t = (ap.dot(ab) / len_sq).clamp(0.0, 1.0);
    let proj = a + ab * t;
    (p - proj).length()
}
