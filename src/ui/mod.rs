mod canvas;
mod viewer;

use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::graph::compiler::{self, command_line, Compiled};
use crate::graph::node::{demo_graph, hex_color, Graph, KeyMode, NodeId, NodeKind, TextPos};
use crate::preview::engine::{self, Accel, PreviewEngine, PreviewMsg};

const DEBOUNCE: Duration = Duration::from_millis(80);
const GRAPH_FILE: &str = "graph.json";
const EXPORT_FILE: &str = "export.mp4";

pub struct App {
    graph: Graph,
    accel: Accel,
    timestamp: f32,

    engine: Option<PreviewEngine>,
    texture: Option<egui::TextureHandle>,
    preview_error: Option<String>,
    preview_note: Option<String>,
    /// Monotonic graph version. Bumped on every edit and reused as the preview
    /// request generation, so in-flight frames from older topologies are
    /// discarded without a second counter.
    graph_version: u64,
    /// True while a worker is expected to answer. egui is otherwise idle and
    /// would not repaint to pick the frame out of the channel.
    preview_pending: bool,

    /// Set on every user edit. The UI waits DEBOUNCE after the last edit
    /// before recompiling, so dragging a slider spawns one FFmpeg, not fifty.
    dirty_at: Option<Instant>,
    compiled: Option<Compiled>,
    compile_error: Option<String>,

    selected: Option<NodeId>,
    /// Index into `graph.links`; invalidated by any structural edit.
    selected_edge: Option<usize>,
    pending_link: Option<(NodeId, usize)>,
    /// True while an input port is being dragged off its wire. The pending
    /// link is then resolved on pointer release, including over empty space
    /// (which leaves the port disconnected).
    wire_drag: bool,
    dragging: Option<(NodeId, egui::Vec2)>,

    status: Option<(String, bool)>,
    export_rx: Option<Receiver<String>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let graph = std::fs::read_to_string(GRAPH_FILE)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(demo_graph);

        let (engine, status) = match PreviewEngine::new() {
            Ok(engine) => (Some(engine), None),
            Err(error) => (None, Some((error, false))),
        };

        Self {
            graph,
            accel: Accel::Cpu,
            timestamp: 0.0,
            engine,
            texture: None,
            preview_error: None,
            preview_note: None,
            graph_version: 0,
            preview_pending: false,
            dirty_at: Some(Instant::now()),
            compiled: None,
            compile_error: None,
            selected: None,
            selected_edge: None,
            pending_link: None,
            wire_drag: false,
            dragging: None,
            status,
            export_rx: None,
        }
    }

    fn touch(&mut self) {
        self.graph_version += 1;
        self.dirty_at = Some(Instant::now());
        self.preview_pending = false;
        // Kill the in-flight FFmpeg immediately; the debounced re-plan below
        // starts a fresh extraction for the new topology.
        if let Some(engine) = &mut self.engine {
            engine.cancel();
        }
    }

    fn notify(&mut self, message: impl Into<String>, ok: bool) {
        self.status = Some((message.into(), ok));
    }

    fn refresh_preview(&mut self) {
        match compiler::compile(&self.graph) {
            Ok(compiled) => {
                // None means "do not even try GPU". Otherwise keep the CPU
                // command around for the fallback path.
                let gpu = (self.accel != Accel::Cpu)
                    .then(|| compiled.preview_args(self.timestamp, self.accel));
                let cpu = compiled.preview_args(self.timestamp, Accel::Cpu);

                self.compiled = Some(compiled);
                self.compile_error = None;
                self.preview_error = None;
                self.preview_note = None;

                if let Some(engine) = &mut self.engine {
                    self.preview_pending = true;
                    engine.request(self.graph_version, gpu, cpu);
                }
            }
            Err(error) => {
                // Invalidate any in-flight frame: its generation predates the
                // broken graph and must not be shown as the current preview.
                self.graph_version += 1;
                self.preview_pending = false;
                if let Some(engine) = &mut self.engine {
                    engine.cancel();
                }
                self.compiled = None;
                self.compile_error = Some(error);
                self.preview_error = None;
                self.preview_note = None;
                self.texture = None;
            }
        }
    }

    fn poll_preview(&mut self, ctx: &egui::Context) {
        let mut messages = Vec::new();
        if let Some(engine) = &mut self.engine {
            while let Some(message) = engine.poll() {
                messages.push(message);
            }
        }

        for message in messages {
            match message {
                PreviewMsg::Frame {
                    generation,
                    pixels,
                    note,
                } => {
                    if generation != self.graph_version {
                        continue; // result of a request we already replaced
                    }
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [
                            crate::preview::engine::PREVIEW_W as usize,
                            crate::preview::engine::PREVIEW_H as usize,
                        ],
                        &pixels,
                    );
                    self.texture = Some(ctx.load_texture(
                        "preview_frame",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                    self.preview_note = note;
                    self.preview_error = None;
                    self.preview_pending = false;
                }
                PreviewMsg::Error {
                    generation,
                    message,
                } => {
                    if generation == self.graph_version {
                        self.preview_error = Some(message);
                        self.preview_pending = false;
                    }
                }
            }
        }
    }

    fn add_node(&mut self, kind: NodeKind) {
        let index = self.graph.nodes.len();
        let pos = [
            60.0 + (index % 4) as f32 * 34.0,
            60.0 + (index % 5) as f32 * 40.0,
        ];
        self.selected = Some(self.graph.add_node(kind, pos));
        self.selected_edge = None;
        self.touch();
    }

    fn top_bar(&mut self, parent: &mut egui::Ui) {
        egui::Panel::top("top_bar").show(parent, |ui| {
            ui.horizontal(|ui| {
                ui.heading("FFNodeComp");
                ui.separator();

                egui::ComboBox::from_label("Acceleration")
                    .selected_text(self.accel.label())
                    .show_ui(ui, |ui| {
                        for accel in Accel::ALL {
                            if ui
                                .selectable_value(&mut self.accel, accel, accel.label())
                                .clicked()
                            {
                                if let Some(engine) = &mut self.engine {
                                    engine.reset_fallback();
                                }
                                self.touch();
                            }
                        }
                    });

                ui.separator();
                if ui.button("Save graph").clicked() {
                    self.save_graph();
                }
                if ui.button("Load graph").clicked() {
                    self.load_graph();
                }

                ui.separator();
                let can_export = self.compiled.is_some() && self.export_rx.is_none();
                if ui
                    .add_enabled(can_export, egui::Button::new("Export MP4"))
                    .on_hover_text("Renders the graph to export.mp4")
                    .clicked()
                {
                    self.start_export();
                }
                if self.export_rx.is_some() {
                    ui.spinner();
                }
            });

            if let Some((message, ok)) = &self.status {
                ui.colored_label(
                    if *ok {
                        egui::Color32::from_rgb(120, 210, 140)
                    } else {
                        egui::Color32::from_rgb(235, 120, 120)
                    },
                    message,
                );
            }
        });
    }

    fn left_panel(&mut self, parent: &mut egui::Ui) {
        egui::Panel::left("nodes")
            .default_size(260.0)
            .resizable(true)
            .show(parent, |ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Add node").strong());
                egui::Grid::new("add_node_grid")
                    .num_columns(2)
                    .spacing([6.0, 6.0])
                    .show(ui, |ui| {
                        if ui.button("Input").clicked() {
                            self.add_node(NodeKind::Input {
                                path: String::new(),
                            });
                        }
                        if ui.button("Transform").clicked() {
                            self.add_node(NodeKind::Transform {
                                width: 1280,
                                height: 720,
                                x: 0,
                                y: 0,
                            });
                        }
                        ui.end_row();
                        if ui.button("Color").clicked() {
                            self.add_node(NodeKind::Color {
                                brightness: 0.0,
                                contrast: 1.0,
                                saturation: 1.0,
                            });
                        }
                        if ui.button("Overlay").clicked() {
                            self.add_node(NodeKind::Overlay { x: 0, y: 0 });
                        }
                        ui.end_row();
                        if ui.button("Output").clicked() {
                            self.add_node(NodeKind::Output {
                                width: 1280,
                                height: 720,
                            });
                        }
                        ui.end_row();
                        if ui.button("Chroma Key").clicked() {
                            self.add_node(NodeKind::ColorKey {
                                color: [0, 255, 0],
                                similarity: 0.3,
                                blend: 0.1,
                            });
                        }
                        if ui.button("Alpha Key").clicked() {
                            self.add_node(NodeKind::AlphaKey {
                                mode: KeyMode::LumaKey,
                                threshold: 0.5,
                                tolerance: 0.1,
                                softness: 0.1,
                            });
                        }
                        ui.end_row();
                        if ui.button("Text").clicked() {
                            self.add_node(NodeKind::Text {
                                text: "Title".into(),
                                font_size: 36.0,
                                font_color: [255, 255, 255],
                                font_file: String::new(),
                                position: TextPos::Center,
                                custom_x: String::new(),
                                custom_y: String::new(),
                                duration: 5.0,
                            });
                        }
                        if ui.button("Image").clicked() {
                            self.add_node(NodeKind::Image {
                                path: String::new(),
                                duration: 5.0,
                                framerate: 30,
                            });
                        }
                        ui.end_row();
                        if ui.button("Solid Color").clicked() {
                            self.add_node(NodeKind::SolidColor {
                                color: [32, 32, 32],
                                width: 1280,
                                height: 720,
                                duration: 5.0,
                                framerate: 30,
                            });
                        }
                        ui.end_row();
                    });

                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Click an output port, then an input port, to connect.\n\
                         Drag a header to move. Drag a connected input away to detach.\n\
                         Right-click a wire to delete it.\n\
                         Delete/Backspace removes the selected node or wire.",
                    )
                    .small()
                    .weak(),
                );

                ui.separator();
                self.inspector(ui);
            });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.selected else {
            ui.label("Select a node to edit it.");
            return;
        };
        let Some(node) = self.graph.node(id) else {
            self.selected = None;
            return;
        };

        ui.label(egui::RichText::new(format!("{}  #{}", node.kind.title(), id)).strong());
        ui.add_space(4.0);

        let mut changed = false;
        {
            let kind = &mut self.graph.nodes[id].kind;
            match kind {
                NodeKind::Input { path } => {
                    ui.label("Video file");
                    changed |= ui.text_edit_singleline(path).changed();
                    ui.label(
                        egui::RichText::new("Paste an absolute path; media stays in place.")
                            .small()
                            .weak(),
                    );
                }
                NodeKind::Transform {
                    width,
                    height,
                    x,
                    y,
                } => {
                    egui::Grid::new("transform_grid")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Width");
                            changed |= ui
                                .add(egui::DragValue::new(width).range(2..=7680).speed(2))
                                .changed();
                            ui.end_row();
                            ui.label("Height");
                            changed |= ui
                                .add(egui::DragValue::new(height).range(2..=7680).speed(2))
                                .changed();
                            ui.end_row();
                            ui.label("X");
                            changed |= ui.add(egui::DragValue::new(x).range(0..=7680)).changed();
                            ui.end_row();
                            ui.label("Y");
                            changed |= ui.add(egui::DragValue::new(y).range(0..=7680)).changed();
                            ui.end_row();
                        });
                    ui.label(
                        egui::RichText::new("Position is measured on the Output canvas.")
                            .small()
                            .weak(),
                    );
                }
                NodeKind::Color {
                    brightness,
                    contrast,
                    saturation,
                } => {
                    changed |= ui
                        .add(egui::Slider::new(brightness, -1.0..=1.0).text("Brightness"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(contrast, 0.0..=2.0).text("Contrast"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(saturation, 0.0..=3.0).text("Saturation"))
                        .changed();
                    ui.label(
                        egui::RichText::new("Maps to FFmpeg eq=brightness:contrast:saturation")
                            .small()
                            .weak(),
                    );
                }
                NodeKind::Overlay { x, y } => {
                    egui::Grid::new("overlay_grid")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("X");
                            changed |= ui.add(egui::DragValue::new(x)).changed();
                            ui.end_row();
                            ui.label("Y");
                            changed |= ui.add(egui::DragValue::new(y)).changed();
                            ui.end_row();
                        });
                    ui.label(
                        egui::RichText::new(
                            "Foreground is placed at this offset over the background.",
                        )
                        .small()
                        .weak(),
                    );
                }
                NodeKind::ColorKey {
                    color,
                    similarity,
                    blend,
                } => {
                    ui.horizontal(|ui| {
                        changed |= ui.color_edit_button_srgb(color).changed();
                        ui.monospace(hex_color(*color));
                    });
                    ui.horizontal(|ui| {
                        if ui.small_button("Green screen").clicked() {
                            *color = [0, 255, 0];
                            changed = true;
                        }
                        if ui.small_button("Blue screen").clicked() {
                            *color = [0, 0, 255];
                            changed = true;
                        }
                    });
                    changed |= ui
                        .add(egui::Slider::new(similarity, 0.01..=1.0).text("Similarity"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(blend, 0.0..=1.0).text("Blend"))
                        .changed();
                    ui.label(
                        egui::RichText::new("Maps to colorkey=color:similarity:blend")
                            .small()
                            .weak(),
                    );
                }
                NodeKind::AlphaKey {
                    mode,
                    threshold,
                    tolerance,
                    softness,
                } => {
                    egui::ComboBox::from_label("Mode")
                        .selected_text(mode.label())
                        .show_ui(ui, |ui| {
                            for option in KeyMode::ALL {
                                if ui.selectable_value(mode, option, option.label()).clicked() {
                                    changed = true;
                                }
                            }
                        });
                    changed |= ui
                        .add(egui::Slider::new(threshold, 0.0..=1.0).text("Threshold"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(tolerance, 0.0..=1.0).text("Tolerance"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(softness, 0.0..=1.0).text("Softness"))
                        .changed();
                    ui.label(
                        egui::RichText::new(
                            "Luma key maps to lumakey. A connected mask overrides it via alphamerge.",
                        )
                        .small()
                        .weak(),
                    );
                }
                NodeKind::Text {
                    text,
                    font_size,
                    font_color,
                    font_file,
                    position,
                    custom_x,
                    custom_y,
                    duration,
                } => {
                    ui.label("Text");
                    changed |= ui.text_edit_multiline(text).changed();
                    ui.horizontal(|ui| {
                        ui.label("Size");
                        changed |= ui
                            .add(egui::DragValue::new(font_size).range(6.0..=400.0).speed(1))
                            .changed();
                        changed |= ui.color_edit_button_srgb(font_color).changed();
                    });
                    changed |= egui::ComboBox::from_id_salt("text_pos")
                        .selected_text(position.label())
                        .show_ui(ui, |ui| {
                            let mut hit = false;
                            for option in TextPos::ALL {
                                hit |= ui
                                    .selectable_value(position, option, option.label())
                                    .clicked();
                            }
                            hit
                        })
                        .inner
                        .unwrap_or(false);
                    if *position == TextPos::CustomXY {
                        egui::Grid::new("text_xy_grid")
                            .num_columns(2)
                            .show(ui, |ui| {
                                ui.label("X");
                                changed |= ui.text_edit_singleline(custom_x).changed();
                                ui.end_row();
                                ui.label("Y");
                                changed |= ui.text_edit_singleline(custom_y).changed();
                                ui.end_row();
                            });
                        ui.label(
                            egui::RichText::new("Expressions, e.g. (w-text_w)/2")
                                .small()
                                .weak(),
                        );
                    }
                    ui.label("Font file (optional)");
                    changed |= ui.text_edit_singleline(font_file).changed();
                    ui.horizontal(|ui| {
                        ui.label("Duration");
                        changed |= ui
                            .add(
                                egui::DragValue::new(duration)
                                    .range(0.1..=600.0)
                                    .suffix("s"),
                            )
                            .changed();
                    });
                    ui.label(
                        egui::RichText::new(
                            "Without a video input Text draws on a transparent canvas; \
                             duration ends that source.",
                        )
                        .small()
                        .weak(),
                    );
                }
                NodeKind::Image {
                    path,
                    duration,
                    framerate,
                } => {
                    ui.label("Image file");
                    changed |= ui.text_edit_singleline(path).changed();
                    egui::Grid::new("image_grid").num_columns(2).show(ui, |ui| {
                        ui.label("Duration");
                        changed |= ui
                            .add(
                                egui::DragValue::new(duration)
                                    .range(0.1..=600.0)
                                    .suffix("s"),
                            )
                            .changed();
                        ui.end_row();
                        ui.label("Framerate");
                        changed |= ui
                            .add(egui::DragValue::new(framerate).range(1..=120))
                            .changed();
                        ui.end_row();
                    });
                    ui.label(
                        egui::RichText::new("Looped with -loop 1 -framerate -t.")
                            .small()
                            .weak(),
                    );
                }
                NodeKind::SolidColor {
                    color,
                    width,
                    height,
                    duration,
                    framerate,
                } => {
                    ui.horizontal(|ui| {
                        changed |= ui.color_edit_button_srgb(color).changed();
                        ui.monospace(hex_color(*color));
                    });
                    egui::Grid::new("solid_grid").num_columns(2).show(ui, |ui| {
                        ui.label("Width");
                        changed |= ui
                            .add(egui::DragValue::new(width).range(2..=7680).speed(2))
                            .changed();
                        ui.end_row();
                        ui.label("Height");
                        changed |= ui
                            .add(egui::DragValue::new(height).range(2..=7680).speed(2))
                            .changed();
                        ui.end_row();
                        ui.label("Duration");
                        changed |= ui
                            .add(
                                egui::DragValue::new(duration)
                                    .range(0.1..=600.0)
                                    .suffix("s"),
                            )
                            .changed();
                        ui.end_row();
                        ui.label("Framerate");
                        changed |= ui
                            .add(egui::DragValue::new(framerate).range(1..=120))
                            .changed();
                        ui.end_row();
                    });
                    ui.label(
                        egui::RichText::new("Generated by the lavfi color source.")
                            .small()
                            .weak(),
                    );
                }
                NodeKind::Output { width, height } => {
                    egui::Grid::new("output_grid")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Canvas width");
                            changed |= ui
                                .add(egui::DragValue::new(width).range(2..=7680).speed(2))
                                .changed();
                            ui.end_row();
                            ui.label("Canvas height");
                            changed |= ui
                                .add(egui::DragValue::new(height).range(2..=7680).speed(2))
                                .changed();
                            ui.end_row();
                        });
                    ui.label(
                        egui::RichText::new("Odd sizes are rounded down to even for H.264.")
                            .small()
                            .weak(),
                    );
                }
            }
        }

        if changed {
            self.touch();
        }

        ui.add_space(8.0);
        if ui
            .button(egui::RichText::new("Delete node").color(egui::Color32::LIGHT_RED))
            .clicked()
        {
            self.graph.remove_node(id);
            self.selected = None;
            self.selected_edge = None;
            self.pending_link = None;
            self.dragging = None;
            self.texture = None;
            self.touch();
        }
    }

    fn save_graph(&mut self) {
        let result = serde_json::to_string_pretty(&self.graph)
            .map_err(|error| error.to_string())
            .and_then(|json| std::fs::write(GRAPH_FILE, json).map_err(|error| error.to_string()));
        match result {
            Ok(()) => self.notify(format!("Saved {GRAPH_FILE}"), true),
            Err(error) => self.notify(format!("Save failed: {error}"), false),
        }
    }

    fn load_graph(&mut self) {
        let result = std::fs::read_to_string(GRAPH_FILE)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                serde_json::from_str::<Graph>(&text).map_err(|error| error.to_string())
            });
        match result {
            Ok(mut graph) => {
                // Ids are indexes; repair hand-edited JSON so the canvas
                // cannot index out of bounds.
                for (index, node) in graph.nodes.iter_mut().enumerate() {
                    node.id = index;
                }
                graph
                    .links
                    .retain(|link| link.from < graph.nodes.len() && link.to < graph.nodes.len());

                self.graph = graph;
                self.selected = None;
                self.selected_edge = None;
                self.pending_link = None;
                self.dragging = None;
                self.texture = None;
                self.touch();
                self.notify(format!("Loaded {GRAPH_FILE}"), true);
            }
            Err(error) => self.notify(format!("Load failed: {error}"), false),
        }
    }

    fn start_export(&mut self) {
        let Some(compiled) = &self.compiled else {
            self.notify("Fix the compile error before exporting", false);
            return;
        };

        let accel = self.accel;
        let gpu_args = compiled.export_args(EXPORT_FILE, accel);
        let cpu_args = compiled.export_args(EXPORT_FILE, Accel::Cpu);
        let (tx, rx) = channel();

        std::thread::spawn(move || {
            let message = if accel == Accel::Cpu {
                match engine::run_blocking(&gpu_args) {
                    Ok(()) => format!("Export finished: {EXPORT_FILE}"),
                    Err(error) => format!("Export failed: {error}"),
                }
            } else {
                match engine::run_blocking(&gpu_args) {
                    Ok(()) => format!("Export finished: {EXPORT_FILE}"),
                    Err(gpu_error) => match engine::run_blocking(&cpu_args) {
                        Ok(()) => format!("Export finished: {EXPORT_FILE} (CPU fallback)"),
                        Err(cpu_error) => {
                            format!("Export failed: {cpu_error} (GPU: {gpu_error})")
                        }
                    },
                }
            };
            let _ = tx.send(message);
        });

        self.export_rx = Some(rx);
        self.notify(format!("Exporting {EXPORT_FILE}..."), true);
    }

    fn poll_export(&mut self) {
        let Some(rx) = &self.export_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(message) => {
                let ok = message.starts_with("Export finished");
                self.status = Some((message, ok));
                self.export_rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.export_rx = None;
                self.notify("Export worker stopped unexpectedly", false);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    fn command_text(&self) -> String {
        match &self.compiled {
            Some(compiled) => command_line(&compiled.preview_args(self.timestamp, self.accel)),
            None => match &self.compile_error {
                Some(error) => format!("Compile error: {error}"),
                None => "Waiting for the graph...".into(),
            },
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_preview(&ctx);
        self.poll_export();

        if let Some(changed_at) = self.dirty_at {
            let elapsed = changed_at.elapsed();
            if elapsed >= DEBOUNCE {
                self.dirty_at = None;
                self.refresh_preview();
            } else {
                ctx.request_repaint_after(DEBOUNCE - elapsed);
            }
        }

        self.top_bar(ui);
        self.left_panel(ui);
        self.viewer(ui);
        egui::CentralPanel::default().show(ui, |ui| {
            self.canvas(ui);
        });

        if self.preview_pending {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        if self.export_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_quotes_paths_with_spaces() {
        let line = command_line(&["-i".into(), "/tmp/my clip.mp4".into()]);
        assert_eq!(line, "ffmpeg -i '/tmp/my clip.mp4'");
    }
}
