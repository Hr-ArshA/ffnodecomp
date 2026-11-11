mod canvas;
mod files;
mod storage;
mod timeline_view;
mod viewer;

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use eframe::egui;
use web_time::Instant;

use crate::graph::compiler::{self, command_line, Compiled};
use crate::graph::node::{
    demo_graph, hex_color, Graph, KeyMode, MaskMode, MaskShape, NodeId, NodeKind, TextPos,
};
use crate::preview::{
    self, Accel, ExportJob, ExportMsg, PreviewMsg, VideoEngineBackend, PREVIEW_H, PREVIEW_W,
};
use files::{PickKind, PickMsg, PickTarget};
use timeline_view::TimelineDrag;

const DEBOUNCE: Duration = Duration::from_millis(80);
/// Scrubbing and clip drags should feel live; the extraction itself is the
/// real frame budget.
const SCRUB_DEBOUNCE: Duration = Duration::from_millis(33);
pub(crate) const GRAPH_FILE: &str = "graph.json";
const EXPORT_FILE: &str = "export.mp4";

pub struct App {
    graph: Graph,
    accel: Accel,
    timestamp: f32,

    engine: Option<Box<dyn VideoEngineBackend>>,
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
    export_path: String,
    export_progress: f32,
    export_rx: Option<Receiver<ExportMsg>>,
    pick_tx: Sender<PickMsg>,
    pick_rx: Receiver<PickMsg>,
    picking: bool,

    /// Debounce length is switched by `seek`/`touch`: scrubs fire faster than
    /// graph edits.
    debounce: Duration,
    timeline_drag: Option<TimelineDrag>,
    /// Wall-clock anchor `(started_at, frame_at_start)` for playback, so the
    /// playhead does not drift when extraction is slower than real time.
    playback_anchor: Option<(Instant, u64)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let mut graph = storage::load()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(demo_graph);
        graph.timeline.repair(graph.nodes.len());

        let (engine, status) = match preview::create_engine() {
            Ok(engine) => (Some(engine), None),
            Err(error) => (None, Some((error, false))),
        };
        let (pick_tx, pick_rx) = channel();

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
            export_path: EXPORT_FILE.to_string(),
            export_progress: 0.0,
            export_rx: None,
            pick_tx,
            pick_rx,
            picking: false,
            debounce: DEBOUNCE,
            timeline_drag: None,
            playback_anchor: None,
        }
    }

    fn touch(&mut self) {
        self.graph_version += 1;
        self.debounce = DEBOUNCE;
        self.dirty_at = Some(Instant::now());
        self.preview_pending = false;
        // Kill the in-flight FFmpeg immediately; the debounced re-plan below
        // starts a fresh extraction for the new topology.
        if let Some(engine) = &mut self.engine {
            engine.cancel();
        }
    }

    /// Timeline edits and scrubbing change the requested timestamp too, so the
    /// generation must move or a late frame for the old time would be shown.
    /// Only the first event of a burst schedules a re-plan; later events ride
    /// the 33 ms window instead of pushing it back.
    fn touch_throttled(&mut self) {
        self.graph_version += 1;
        self.preview_pending = false;
        if let Some(engine) = &mut self.engine {
            engine.cancel();
        }
        if self.dirty_at.is_none() {
            self.debounce = SCRUB_DEBOUNCE;
            self.dirty_at = Some(Instant::now());
        }
    }

    /// Seek the preview clock. Scrubbing cancels the in-flight frame; playback
    /// does not, otherwise a render slower than real time would never finish.
    fn seek_to_secs(&mut self, secs: f32, cancel: bool) {
        self.timestamp = secs.max(0.0);
        if self.dirty_at.is_some() && !cancel {
            return;
        }
        self.debounce = SCRUB_DEBOUNCE;
        if self.dirty_at.is_none() {
            self.dirty_at = Some(Instant::now());
        }
        if cancel {
            self.graph_version += 1;
            self.preview_pending = false;
            if let Some(engine) = &mut self.engine {
                engine.cancel();
            }
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
                        [PREVIEW_W as usize, PREVIEW_H as usize],
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

    fn poll_files(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.pick_rx.try_recv() {
            self.picking = false;
            match message {
                PickMsg::Error(error) => self.notify(error, false),
                PickMsg::Done(_, None) => {}
                PickMsg::Done(target, Some(file)) => match target {
                    PickTarget::Export => {
                        if !file.path.trim().is_empty() {
                            self.export_path = file.path;
                        }
                    }
                    PickTarget::Node(id, kind) => {
                        let Some(node) = self.graph.nodes.get_mut(id) else {
                            continue;
                        };
                        let changed = match (&mut node.kind, kind) {
                            (NodeKind::Input { path }, PickKind::Video)
                            | (NodeKind::Image { path, .. }, PickKind::Image)
                            | (NodeKind::Lut3d { path }, PickKind::Lut) => {
                                *path = file.path;
                                true
                            }
                            (NodeKind::Text { font_file, .. }, PickKind::Font) => {
                                *font_file = file.path;
                                true
                            }
                            _ => false,
                        };
                        if changed {
                            self.touch();
                        }
                    }
                },
            }
        }
        if self.picking {
            ctx.request_repaint_after(Duration::from_millis(100));
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
        self.graph.sync_timeline_clips();
        self.touch();
    }

    fn top_bar(&mut self, parent: &mut egui::Ui) {
        egui::Panel::top("top_bar").show(parent, |ui| {
            ui.horizontal(|ui| {
                ui.heading("FFNodeComp");
                ui.separator();

                if !cfg!(target_arch = "wasm32") {
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
                }
                if ui.button("Save graph").clicked() {
                    self.save_graph();
                }
                if ui.button("Load graph").clicked() {
                    self.load_graph();
                }

                ui.separator();
                ui.label("Export:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.export_path)
                        .desired_width(120.0)
                        .hint_text("export.mp4"),
                );
                if ui
                    .button("Browse...")
                    .on_hover_text("Choose export destination")
                    .clicked()
                {
                    self.picking = true;
                    files::pick(
                        PickTarget::Export,
                        PickKind::Video,
                        &self.export_path,
                        self.pick_tx.clone(),
                    );
                }

                let can_export = self.compiled.is_some()
                    && self.export_rx.is_none()
                    && !self.export_path.trim().is_empty();
                if ui
                    .add_enabled(can_export, egui::Button::new("Export MP4"))
                    .on_hover_text(format!("Renders the graph to {}", self.export_path))
                    .clicked()
                {
                    self.start_export();
                }
                if self.export_rx.is_some() {
                    ui.add(
                        egui::ProgressBar::new(self.export_progress)
                            .show_percentage()
                            .desired_width(100.0),
                    );
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
                        let has_output = self
                            .graph
                            .nodes
                            .iter()
                            .any(|n| matches!(n.kind, NodeKind::Output { .. }));
                        if ui
                            .add_enabled(!has_output, egui::Button::new("Output"))
                            .on_disabled_hover_text("Graph already has an Output node")
                            .clicked()
                        {
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
                        if ui.button("Blur").clicked() {
                            self.add_node(NodeKind::Blur {
                                sigma: 3.0,
                                sigma_v: 3.0,
                                steps: 1,
                            });
                        }
                        ui.end_row();
                        if ui.button("Sharpen").clicked() {
                            self.add_node(NodeKind::Sharpen {
                                amount: 1.0,
                                clarity: 0.0,
                                msize_x: 5,
                                msize_y: 5,
                            });
                        }
                        if ui.button("3D LUT").clicked() {
                            self.add_node(NodeKind::Lut3d {
                                path: String::new(),
                            });
                        }
                        ui.end_row();
                        if ui.button("Color Balance").clicked() {
                            self.add_node(NodeKind::ColorBalance {
                                shadows: [0.0; 3],
                                midtones: [0.0; 3],
                                highlights: [0.0; 3],
                            });
                        }
                        if ui.button("Levels").clicked() {
                            self.add_node(NodeKind::Levels {
                                black: [0.0; 3],
                                white: [1.0; 3],
                                gamma: [1.0; 3],
                            });
                        }
                        ui.end_row();
                        if ui.button("Invert / Solarize").clicked() {
                            self.add_node(NodeKind::InvertSolarize {
                                invert: 0.0,
                                solarize: 0.0,
                                threshold: 0.5,
                            });
                        }
                        if ui.button("Matte Blur").clicked() {
                            self.add_node(NodeKind::MatteBlur {
                                blur: 3.0,
                                erode: 0,
                                dilate: 0,
                            });
                        }
                        ui.end_row();
                        if ui.button("Mask Combine").clicked() {
                            self.add_node(NodeKind::MaskCombine {
                                mode: MaskMode::Multiply,
                            });
                        }
                        if ui.button("Shape Mask").clicked() {
                            self.add_node(NodeKind::ShapeMask {
                                shape: MaskShape::Rectangle,
                                width: 640,
                                height: 360,
                                x: 0,
                                y: 0,
                                feather: 20.0,
                                invert: false,
                                duration: 5.0,
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
        let pick_tx = self.pick_tx.clone();
        {
            let kind = &mut self.graph.nodes[id].kind;
            match kind {
                NodeKind::Input { path } => {
                    ui.label("Video file");
                    ui.horizontal(|ui| {
                        changed |= ui.text_edit_singleline(path).changed();
                        if ui.button("Browse...").clicked() {
                            self.picking = true;
                            files::pick(
                                PickTarget::Node(id, PickKind::Video),
                                PickKind::Video,
                                path,
                                pick_tx.clone(),
                            );
                        }
                    });
                    ui.label(
                        egui::RichText::new("Choose a video file or paste an absolute path.")
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
                    ui.horizontal(|ui| {
                        changed |= ui.text_edit_singleline(font_file).changed();
                        if ui.button("Browse...").clicked() {
                            self.picking = true;
                            files::pick(
                                PickTarget::Node(id, PickKind::Font),
                                PickKind::Font,
                                font_file,
                                pick_tx.clone(),
                            );
                        }
                    });
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
                    ui.horizontal(|ui| {
                        changed |= ui.text_edit_singleline(path).changed();
                        if ui.button("Browse...").clicked() {
                            self.picking = true;
                            files::pick(
                                PickTarget::Node(id, PickKind::Image),
                                PickKind::Image,
                                path,
                                pick_tx.clone(),
                            );
                        }
                    });
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
                NodeKind::Blur {
                    sigma,
                    sigma_v,
                    steps,
                } => {
                    changed |= ui
                        .add(egui::Slider::new(sigma, 0.0..=100.0).text("Horizontal radius"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(sigma_v, 0.0..=100.0).text("Vertical radius"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(steps, 1..=6).text("Steps"))
                        .changed();
                    ui.label(
                        egui::RichText::new("Gaussian blur via gblur=sigma:sigmaV:steps")
                            .small()
                            .weak(),
                    );
                }
                NodeKind::Sharpen {
                    amount,
                    clarity,
                    msize_x,
                    msize_y,
                } => {
                    changed |= ui
                        .add(egui::Slider::new(amount, -2.0..=5.0).text("Edge sharpness"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(clarity, -2.0..=5.0).text("Clarity amount"))
                        .changed();
                    egui::Grid::new("sharpen_matrix_grid")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Matrix size X");
                            let mut x = *msize_x;
                            if ui
                                .add(egui::DragValue::new(&mut x).range(3..=23).speed(2))
                                .changed()
                            {
                                *msize_x = crate::graph::node::odd_matrix(x);
                                changed = true;
                            }
                            ui.end_row();
                            ui.label("Matrix size Y");
                            let mut y = *msize_y;
                            if ui
                                .add(egui::DragValue::new(&mut y).range(3..=23).speed(2))
                                .changed()
                            {
                                *msize_y = crate::graph::node::odd_matrix(y);
                                changed = true;
                            }
                            ui.end_row();
                        });
                    ui.label(
                        egui::RichText::new(
                            "Sharpening via unsharp=lx:ly:la (matrix sizes must be odd 3..23)",
                        )
                        .small()
                        .weak(),
                    );
                }
                NodeKind::Lut3d { path } => {
                    ui.label("3D LUT file (.cube / .3dl)");
                    ui.horizontal(|ui| {
                        changed |= ui.text_edit_singleline(path).changed();
                        if ui.button("Browse...").clicked() {
                            self.picking = true;
                            files::pick(
                                PickTarget::Node(id, PickKind::Lut),
                                PickKind::Lut,
                                path,
                                pick_tx.clone(),
                            );
                        }
                    });
                    ui.label(
                        egui::RichText::new(
                            "Cinematic preset via lut3d=file=...; an empty path is an identity pass.",
                        )
                        .small()
                        .weak(),
                    );
                }
                NodeKind::ColorBalance {
                    shadows,
                    midtones,
                    highlights,
                } => {
                    for (label, values) in [
                        ("Shadows (Lift)", shadows),
                        ("Midtones (Gamma)", midtones),
                        ("Highlights (Gain)", highlights),
                    ] {
                        ui.label(egui::RichText::new(label).strong());
                        changed |= rgb_sliders(ui, label, values, -1.0..=1.0, 2);
                        ui.add_space(4.0);
                    }
                    ui.label(
                        egui::RichText::new("colorbalance=rs:gs:bs:rm:gm:bm:rh:gh:bh")
                            .small()
                            .weak(),
                    );
                }
                NodeKind::Levels {
                    black,
                    white,
                    gamma,
                } => {
                    ui.label(egui::RichText::new("Black point").strong());
                    changed |= rgb_sliders(ui, "levels_black", black, 0.0..=1.0, 3);
                    ui.label(egui::RichText::new("White point").strong());
                    changed |= rgb_sliders(ui, "levels_white", white, 0.0..=1.0, 3);
                    ui.label(egui::RichText::new("Gamma").strong());
                    changed |= rgb_sliders(ui, "levels_gamma", gamma, 0.1..=10.0, 2);
                    ui.label(
                        egui::RichText::new(
                            "Histogram placeholder (not sampled yet). Maps to colorlevels + eq gamma.",
                        )
                        .small()
                        .weak(),
                    );
                }
                NodeKind::InvertSolarize {
                    invert,
                    solarize,
                    threshold,
                } => {
                    changed |= ui
                        .add(egui::Slider::new(invert, 0.0..=1.0).text("Invert amount"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(solarize, 0.0..=1.0).text("Solarize amount"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(threshold, 0.0..=1.0).text("Solarize threshold"))
                        .changed();
                    ui.label(
                        egui::RichText::new(
                            "Full invert uses negate; partial amounts use lutrgb blends.",
                        )
                        .small()
                        .weak(),
                    );
                }
                NodeKind::MatteBlur {
                    blur,
                    erode,
                    dilate,
                } => {
                    changed |= ui
                        .add(egui::Slider::new(blur, 0.0..=50.0).text("Blur radius"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(erode, 0..=4).text("Erode passes"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(dilate, 0..=4).text("Dilate passes"))
                        .changed();
                    ui.label(
                        egui::RichText::new(
                            "Mask-only: boxblur + repeated erosion/dilation, normalised to gray.",
                        )
                        .small()
                        .weak(),
                    );
                }
                NodeKind::MaskCombine { mode } => {
                    egui::ComboBox::from_label("Mode")
                        .selected_text(mode.label())
                        .show_ui(ui, |ui| {
                            for option in MaskMode::ALL {
                                if ui.selectable_value(mode, option, option.label()).clicked() {
                                    changed = true;
                                }
                            }
                        });
                    ui.label(
                        egui::RichText::new(
                            "blend=all_mode=multiply/addition/difference on two mask ports.",
                        )
                        .small()
                        .weak(),
                    );
                }
                NodeKind::ShapeMask {
                    shape,
                    width,
                    height,
                    x,
                    y,
                    feather,
                    invert,
                    duration,
                } => {
                    egui::ComboBox::from_label("Shape")
                        .selected_text(shape.label())
                        .show_ui(ui, |ui| {
                            for option in MaskShape::ALL {
                                if ui.selectable_value(shape, option, option.label()).clicked() {
                                    changed = true;
                                }
                            }
                        });
                    egui::Grid::new("shape_mask_grid")
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
                            changed |= ui
                                .add(egui::DragValue::new(x).range(-7680..=7680))
                                .changed();
                            ui.end_row();
                            ui.label("Y");
                            changed |= ui
                                .add(egui::DragValue::new(y).range(-7680..=7680))
                                .changed();
                            ui.end_row();
                            ui.label("Feather");
                            changed |= ui
                                .add(egui::Slider::new(feather, 0.0..=200.0).text("px"))
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
                        });
                    changed |= ui.checkbox(invert, "Invert mask").changed();
                    ui.label(
                        egui::RichText::new(
                            "Generated at Output canvas size with drawbox/geq and a feathered edge.",
                        )
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
            .and_then(|json| storage::save(&json));
        match result {
            Ok(()) => self.notify(format!("Saved {GRAPH_FILE}"), true),
            Err(error) => self.notify(format!("Save failed: {error}"), false),
        }
    }

    fn load_graph(&mut self) {
        let result = storage::load()
            .ok_or_else(|| format!("no {GRAPH_FILE} found"))
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
                graph.timeline.repair(graph.nodes.len());
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

        let out_path = self.export_path.clone();
        // ffmpeg.wasm writes into MEMFS, so the written path is fixed and only
        // the browser download name comes from the picker.
        let out = if cfg!(target_arch = "wasm32") {
            EXPORT_FILE.to_string()
        } else {
            out_path.clone()
        };
        let total_duration = preview::estimated_duration(&self.graph);
        let accel = self.accel;
        let gpu_args = compiled.export_args(&out, accel);
        let cpu_args = compiled.export_args(&out, Accel::Cpu);
        let download_name = std::path::Path::new(&out_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(EXPORT_FILE)
            .to_string();
        let (tx, rx) = channel();

        preview::spawn_export(
            ExportJob {
                args: gpu_args,
                fallback: (accel != Accel::Cpu).then_some(cpu_args),
                total: total_duration,
                out,
                download_name,
            },
            tx,
        );

        self.export_progress = 0.0;
        self.export_rx = Some(rx);
        self.notify(format!("Exporting {}...", self.export_path), true);
    }

    fn poll_export(&mut self) {
        let Some(rx) = &self.export_rx else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(ExportMsg::Progress(frac)) => {
                    self.export_progress = frac;
                }
                Ok(ExportMsg::Finished(res)) => {
                    match res {
                        Ok(msg) => {
                            self.export_progress = 1.0;
                            self.status = Some((msg, true));
                        }
                        Err(err) => {
                            self.status = Some((err, false));
                        }
                    }
                    self.export_rx = None;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.export_rx = None;
                    self.notify("Export worker stopped unexpectedly", false);
                    break;
                }
            }
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

/// R/G/B slider grid shared by Color Balance and Levels. `id` must be unique
/// per visible grid, otherwise egui reuses the wrong layout state.
fn rgb_sliders(
    ui: &mut egui::Ui,
    id: &str,
    values: &mut [f32; 3],
    range: std::ops::RangeInclusive<f32>,
    decimals: usize,
) -> bool {
    let mut changed = false;
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([8.0, 2.0])
        .show(ui, |ui| {
            for (label, value) in ["R", "G", "B"].into_iter().zip(values.iter_mut()) {
                ui.label(label);
                changed |= ui
                    .add(egui::Slider::new(value, range.clone()).fixed_decimals(decimals))
                    .changed();
                ui.end_row();
            }
        });
    changed
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_files(&ctx);
        self.poll_preview(&ctx);
        self.poll_export();
        self.handle_timeline_keys(&ctx);
        self.advance_playback(&ctx);

        if let Some(changed_at) = self.dirty_at {
            let elapsed = changed_at.elapsed();
            if elapsed >= self.debounce {
                self.dirty_at = None;
                self.refresh_preview();
            } else {
                ctx.request_repaint_after(self.debounce - elapsed);
            }
        }

        self.top_bar(ui);
        self.left_panel(ui);
        self.viewer(ui);
        self.timeline(ui);
        egui::CentralPanel::default().show(ui, |ui| {
            self.canvas(ui);
        });

        if self.preview_pending {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        // A scrub handled while drawing the timeline schedules a refresh after
        // the debounce check has already run this frame.
        if self.dirty_at.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if self.export_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
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
