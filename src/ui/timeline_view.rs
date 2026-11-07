//! Docked non-linear timeline: ruler, tracks, clips, playhead and transport.
//!
//! egui has no multi-track widget, so everything is painted by hand (the same
//! approach as the node canvas) and hit-tested with per-clip interact rects.

use std::time::Instant;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Vec2,
};

use super::App;
use crate::timeline::{ClipDragMode, FrameIndex, PlaybackState, Timecode, TrackId, TrackKind};

const HEADER_W: f32 = 104.0;
const RULER_H: f32 = 24.0;
const TRACK_H: f32 = 52.0;
const TRACK_GAP: f32 = 4.0;
const HANDLE_W: f32 = 7.0;
const MIN_ZOOM: f32 = 8.0;
const MAX_ZOOM: f32 = 400.0;

#[derive(Clone, Copy)]
pub(crate) enum TimelineDrag {
    Playhead,
    Clip {
        track: TrackId,
        clip: u64,
        mode: ClipDragMode,
        last_frame: u64,
    },
}

struct ClipDraw {
    track: TrackId,
    clip: u64,
    rect: Rect,
    label: String,
    border: Color32,
    fill: Color32,
    selected: bool,
}

impl App {
    pub(crate) fn timeline(&mut self, parent: &mut egui::Ui) {
        egui::Panel::bottom("timeline_panel")
            .default_size(232.0)
            .resizable(true)
            .show(parent, |ui| {
                self.timeline_toolbar(ui);
                ui.separator();
                self.timeline_body(ui);
            });
    }

    fn timeline_toolbar(&mut self, ui: &mut egui::Ui) {
        let fps = self.graph.timeline.fps();
        let playing = self.graph.timeline.playback_state == PlaybackState::Playing;

        ui.horizontal(|ui| {
            if ui
                .button(if playing { "Pause" } else { "Play" })
                .on_hover_text("Space")
                .clicked()
            {
                self.toggle_playback();
            }
            if ui.button("Start").on_hover_text("Home").clicked() {
                self.go_to_start();
            }
            if ui
                .button("<")
                .on_hover_text("Step back one frame (Left)")
                .clicked()
            {
                self.step_frames(-1);
            }
            if ui
                .button(">")
                .on_hover_text("Step forward one frame (Right)")
                .clicked()
            {
                self.step_frames(1);
            }

            ui.separator();
            if ui.button("Set In").on_hover_text("I").clicked() {
                self.graph.timeline.set_in();
            }
            if ui.button("Set Out").on_hover_text("O").clicked() {
                self.graph.timeline.set_out();
            }
            if self.graph.timeline.loop_range.is_some() && ui.small_button("Clear loop").clicked() {
                self.graph.timeline.loop_range = None;
            }

            ui.separator();
            ui.label(
                egui::RichText::new(self.graph.timeline.timecode().to_string())
                    .monospace()
                    .strong(),
            );
            if let Some((in_frame, out_frame)) = self.graph.timeline.loop_range {
                ui.label(
                    egui::RichText::new(format!(
                        "loop {} - {}",
                        Timecode::from_frames(in_frame, fps),
                        Timecode::from_frames(out_frame, fps)
                    ))
                    .small()
                    .weak(),
                );
            }

            ui.separator();
            egui::ComboBox::from_id_salt("timeline_fps")
                .selected_text(format!("{fps} fps"))
                .show_ui(ui, |ui| {
                    for candidate in [24, 25, 30, 50, 60] {
                        ui.selectable_value(
                            &mut self.graph.timeline.timebase,
                            candidate,
                            format!("{candidate} fps"),
                        );
                    }
                });

            ui.separator();
            if ui.small_button("-").on_hover_text("Zoom out").clicked() {
                self.graph.timeline.zoom = (self.graph.timeline.zoom * 0.8).max(MIN_ZOOM);
            }
            if ui.small_button("+").on_hover_text("Zoom in").clicked() {
                self.graph.timeline.zoom = (self.graph.timeline.zoom * 1.25).min(MAX_ZOOM);
            }
            ui.checkbox(&mut self.graph.timeline.snapping, "Snap");

            if ui
                .button("Sync clips")
                .on_hover_text("Create a timeline clip for every source node")
                .clicked()
            {
                self.sync_timeline_clips();
            }
        });
    }

    fn timeline_body(&mut self, ui: &mut egui::Ui) {
        let fps = self.graph.timeline.fps();
        let track_count = self.graph.timeline.tracks.len().max(1);
        let wanted = RULER_H + track_count as f32 * (TRACK_H + TRACK_GAP) + 6.0;
        let (area, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), wanted), Sense::hover());
        let tracks_left = area.left() + HEADER_W;
        let tracks_right = area.right() - 2.0;
        if tracks_right <= tracks_left + 20.0 {
            return;
        }

        self.timeline_scroll(ui, &response, tracks_left, tracks_right);

        let zoom = self.graph.timeline.zoom;
        let scroll = self.graph.timeline.scroll_offset;
        let px_per_frame = (zoom / fps as f32).max(0.001);
        let x_of = move |frame: u64| tracks_left + (frame as f32 - scroll) * px_per_frame;
        let frame_at =
            move |x: f32| ((((x - tracks_left) / px_per_frame) + scroll).max(0.0)).round() as u64;

        let painter = ui.painter_at(area);
        painter.rect_filled(area, CornerRadius::same(4), Color32::from_rgb(18, 20, 25));

        let ruler = Rect::from_min_max(
            egui::pos2(tracks_left, area.top()),
            egui::pos2(tracks_right, area.top() + RULER_H),
        );
        painter.rect_filled(ruler, CornerRadius::ZERO, Color32::from_rgb(27, 30, 38));
        draw_ruler(&painter, ruler, fps, zoom, scroll, x_of);

        // Track headers and empty lanes.
        let mut lane_top = area.top() + RULER_H + TRACK_GAP;
        for track in &self.graph.timeline.tracks {
            let header = Rect::from_min_size(
                egui::pos2(area.left(), lane_top),
                Vec2::new(HEADER_W, TRACK_H),
            );
            painter.rect_filled(header, CornerRadius::same(3), Color32::from_rgb(31, 34, 42));
            painter.text(
                egui::pos2(header.left() + 8.0, header.top() + 8.0),
                Align2::LEFT_TOP,
                &track.name,
                FontId::proportional(11.0),
                Color32::from_gray(205),
            );
            let kind = match track.kind {
                TrackKind::Video => "video",
                TrackKind::Audio => "audio",
            };
            painter.text(
                egui::pos2(header.left() + 8.0, header.top() + 27.0),
                Align2::LEFT_TOP,
                kind,
                FontId::proportional(9.0),
                Color32::from_gray(115),
            );

            let lane = Rect::from_min_max(
                egui::pos2(tracks_left, lane_top),
                egui::pos2(tracks_right, lane_top + TRACK_H),
            );
            painter.rect_filled(lane, CornerRadius::same(3), Color32::from_rgb(22, 25, 32));
            painter.line_segment(
                [lane.left_bottom(), lane.right_bottom()],
                Stroke::new(1.0, Color32::from_rgb(34, 38, 46)),
            );
            lane_top += TRACK_H + TRACK_GAP;
        }

        // Clip metadata is collected first so interacting with a clip does not
        // fight the immutable borrow used for painting.
        let mut draws = Vec::new();
        let mut lane_top = area.top() + RULER_H + TRACK_GAP;
        for track in &self.graph.timeline.tracks {
            let (border, fill) = match track.kind {
                // Colours match the canvas port types.
                TrackKind::Video => (
                    Color32::from_rgb(228, 150, 70),
                    Color32::from_rgb(66, 45, 24),
                ),
                TrackKind::Audio => (
                    Color32::from_rgb(90, 190, 130),
                    Color32::from_rgb(26, 58, 40),
                ),
            };
            for clip in &track.clips {
                let x0 = x_of(clip.start_frame).max(tracks_left);
                let x1 = x_of(clip.end_frame()).min(tracks_right);
                if x1 <= x0 {
                    continue;
                }
                let rect = Rect::from_min_max(
                    egui::pos2(x0, lane_top + 2.0),
                    egui::pos2(x1, lane_top + TRACK_H - 2.0),
                );
                let title = self
                    .graph
                    .node(clip.node)
                    .map(|node| node.kind.title())
                    .unwrap_or("missing node");
                let seconds = clip.duration_frames as f32 / fps as f32;
                draws.push(ClipDraw {
                    track: track.id,
                    clip: clip.clip_id,
                    rect,
                    label: format!("{title}  {seconds:.1}s"),
                    border,
                    fill,
                    selected: self.graph.timeline.selected == Some((track.id, clip.clip_id)),
                });
            }
            lane_top += TRACK_H + TRACK_GAP;
        }

        let lane_clip = Rect::from_min_max(
            egui::pos2(tracks_left, area.top() + RULER_H),
            egui::pos2(tracks_right, area.bottom()),
        );
        let clip_painter = painter.with_clip_rect(lane_clip);
        for draw in &draws {
            clip_painter.rect_filled(draw.rect, CornerRadius::same(3), draw.fill);
            clip_painter.rect_stroke(
                draw.rect,
                CornerRadius::same(3),
                Stroke::new(
                    if draw.selected { 2.0 } else { 1.0 },
                    if draw.selected {
                        Color32::WHITE
                    } else {
                        draw.border
                    },
                ),
                StrokeKind::Inside,
            );
            // Trim handles.
            clip_painter.rect_filled(
                Rect::from_min_size(draw.rect.min, Vec2::new(HANDLE_W, draw.rect.height())),
                CornerRadius::ZERO,
                draw.border.gamma_multiply(0.55),
            );
            clip_painter.rect_filled(
                Rect::from_min_max(
                    egui::pos2(draw.rect.right() - HANDLE_W, draw.rect.top()),
                    egui::pos2(draw.rect.right(), draw.rect.bottom()),
                ),
                CornerRadius::ZERO,
                draw.border.gamma_multiply(0.55),
            );
            clip_painter.text(
                egui::pos2(draw.rect.left() + HANDLE_W + 4.0, draw.rect.top() + 5.0),
                Align2::LEFT_TOP,
                &draw.label,
                FontId::proportional(10.0),
                Color32::from_gray(235),
            );
        }

        // Clip body drags: move placement.
        for draw in &draws {
            let body = Rect::from_min_max(
                egui::pos2(draw.rect.left() + HANDLE_W, draw.rect.top()),
                egui::pos2(draw.rect.right() - HANDLE_W, draw.rect.bottom()),
            );
            if body.width() < 2.0 {
                continue;
            }
            let resp = ui
                .interact(
                    body,
                    egui::Id::unique(("timeline_body", draw.track, draw.clip)),
                    Sense::click_and_drag(),
                )
                .on_hover_cursor(egui::CursorIcon::Grab);
            if resp.drag_started() {
                self.graph.timeline.selected = Some((draw.track, draw.clip));
                self.begin_clip_drag(ui, &resp, draw, ClipDragMode::Body, &frame_at);
            }
            if resp.dragged() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    self.continue_clip_drag(pos.x, &frame_at);
                }
            }
            if resp.drag_stopped() {
                self.timeline_drag = None;
            }
        }

        // Edge handles: trim in (changes source offset) / trim out.
        for draw in &draws {
            let left = Rect::from_min_size(draw.rect.min, Vec2::new(HANDLE_W, draw.rect.height()));
            let right = Rect::from_min_max(
                egui::pos2(draw.rect.right() - HANDLE_W, draw.rect.top()),
                egui::pos2(draw.rect.right(), draw.rect.bottom()),
            );
            for (rect, mode, id_salt, cursor) in [
                (
                    left,
                    ClipDragMode::TrimIn,
                    "timeline_left",
                    egui::CursorIcon::ResizeHorizontal,
                ),
                (
                    right,
                    ClipDragMode::TrimOut,
                    "timeline_right",
                    egui::CursorIcon::ResizeHorizontal,
                ),
            ] {
                let resp = ui
                    .interact(
                        rect,
                        egui::Id::unique((id_salt, draw.track, draw.clip)),
                        Sense::click_and_drag(),
                    )
                    .on_hover_cursor(cursor);
                if resp.drag_started() {
                    self.graph.timeline.selected = Some((draw.track, draw.clip));
                    self.begin_clip_drag(ui, &resp, draw, mode, &frame_at);
                }
                if resp.dragged() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        self.continue_clip_drag(pos.x, &frame_at);
                    }
                }
                if resp.drag_stopped() {
                    self.timeline_drag = None;
                }
            }
        }

        // Ruler scrubbing. Done after clips so the playhead is painted on top.
        let ruler_resp = ui.interact(
            ruler,
            egui::Id::unique("timeline_ruler"),
            Sense::click_and_drag(),
        );
        if ruler_resp.clicked() || ruler_resp.dragged() {
            if let Some(pos) = ruler_resp.interact_pointer_pos() {
                self.graph.timeline.playback_state = PlaybackState::Scrubbing;
                self.seek_to_frame(frame_at(pos.x));
                self.timeline_drag = Some(TimelineDrag::Playhead);
            }
        }
        if ruler_resp.drag_stopped() {
            self.graph.timeline.playback_state = PlaybackState::Paused;
            self.timeline_drag = None;
        }

        // Playhead. Painted with the unclipped painter so the top handle is
        // visible inside the ruler.
        let playhead_x = x_of(self.graph.timeline.playhead.0);
        if playhead_x >= tracks_left && playhead_x <= tracks_right {
            let head = Color32::from_rgb(238, 92, 82);
            painter.line_segment(
                [
                    egui::pos2(playhead_x, ruler.bottom()),
                    egui::pos2(playhead_x, area.bottom()),
                ],
                Stroke::new(1.5, head),
            );
            painter.rect_filled(
                Rect::from_min_max(
                    egui::pos2(playhead_x - 5.0, ruler.top() + 1.0),
                    egui::pos2(playhead_x + 5.0, ruler.bottom() - 2.0),
                ),
                CornerRadius::same(2),
                head,
            );
        }
    }

    fn timeline_scroll(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        tracks_left: f32,
        tracks_right: f32,
    ) {
        if !response.hovered() {
            return;
        }
        let pointer = ui.input(|i| i.pointer.hover_pos());
        if !pointer.is_some_and(|pos| pos.x >= tracks_left && pos.x <= tracks_right) {
            return;
        }
        let (scroll, command) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.command));
        if command && scroll.y != 0.0 {
            let factor = (1.0 + scroll.y * 0.002).clamp(0.75, 1.33);
            self.graph.timeline.zoom =
                (self.graph.timeline.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
            return;
        }
        let delta = if scroll.x.abs() > f32::EPSILON {
            scroll.x
        } else {
            scroll.y
        };
        if delta.abs() > f32::EPSILON {
            let px_per_frame = self.graph.timeline.px_per_frame();
            self.graph.timeline.scroll_offset = (self.graph.timeline.scroll_offset
                - delta / px_per_frame)
                .clamp(0.0, self.graph.timeline.duration as f32);
        }
    }

    fn begin_clip_drag(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        draw: &ClipDraw,
        mode: ClipDragMode,
        frame_at: &impl Fn(f32) -> u64,
    ) {
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.ctx().pointer_latest_pos());
        let last_frame = pointer.map_or(self.graph.timeline.playhead.0, |pos| frame_at(pos.x));
        self.timeline_drag = Some(TimelineDrag::Clip {
            track: draw.track,
            clip: draw.clip,
            mode,
            last_frame,
        });
    }

    fn continue_clip_drag(&mut self, pointer_x: f32, frame_at: &impl Fn(f32) -> u64) {
        let Some(TimelineDrag::Clip {
            track,
            clip,
            mode,
            last_frame,
        }) = self.timeline_drag
        else {
            return;
        };
        let frame = frame_at(pointer_x);
        let delta = frame as i64 - last_frame as i64;
        if delta == 0 {
            return;
        }
        self.graph.timeline.drag_clip(track, clip, mode, delta);
        self.timeline_drag = Some(TimelineDrag::Clip {
            track,
            clip,
            mode,
            last_frame: frame,
        });
        self.touch_throttled();
    }

    pub(crate) fn handle_timeline_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (space, left, right, home, set_in, set_out) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::I),
                i.key_pressed(egui::Key::O),
            )
        });
        if space {
            self.toggle_playback();
        }
        if home {
            self.go_to_start();
        }
        if left {
            self.step_frames(-1);
        }
        if right {
            self.step_frames(1);
        }
        if set_in {
            self.graph.timeline.set_in();
        }
        if set_out {
            self.graph.timeline.set_out();
        }
    }

    pub(crate) fn advance_playback(&mut self, ctx: &egui::Context) {
        if self.graph.timeline.playback_state != PlaybackState::Playing {
            self.playback_anchor = None;
            return;
        }
        let fps = self.graph.timeline.fps();
        let (started, start_frame) = *self
            .playback_anchor
            .get_or_insert_with(|| (Instant::now(), self.graph.timeline.playhead.0));
        let elapsed = started.elapsed().as_secs_f64();
        let mut frame = start_frame + (elapsed * fps as f64) as u64;
        let duration = self.graph.timeline.duration.max(1);

        if let Some((in_frame, out_frame)) = self.graph.timeline.loop_range {
            if frame >= out_frame.0 {
                frame = in_frame.0;
                self.playback_anchor = Some((Instant::now(), frame));
            }
        } else if frame >= duration {
            frame = duration - 1;
            self.graph.timeline.playback_state = PlaybackState::Paused;
            self.playback_anchor = None;
        }

        if frame != self.graph.timeline.playhead.0 {
            self.graph.timeline.playhead = FrameIndex(frame);
            // Do not cancel here: playback is best-effort real time and a
            // render slower than real time would otherwise never land.
            self.seek_to_secs(frame as f32 / fps as f32, false);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }

    pub(crate) fn toggle_playback(&mut self) {
        if self.graph.timeline.playback_state == PlaybackState::Playing {
            self.graph.timeline.playback_state = PlaybackState::Paused;
            self.playback_anchor = None;
            return;
        }
        if let Some((in_frame, out_frame)) = self.graph.timeline.loop_range {
            if self.graph.timeline.playhead >= out_frame {
                self.graph.timeline.playhead = in_frame;
            }
        }
        self.graph.timeline.playback_state = PlaybackState::Playing;
        self.playback_anchor = Some((Instant::now(), self.graph.timeline.playhead.0));
        self.seek_to_secs(self.graph.timeline.playhead_secs() as f32, true);
    }

    pub(crate) fn go_to_start(&mut self) {
        self.graph.timeline.playback_state = PlaybackState::Paused;
        self.playback_anchor = None;
        self.graph.timeline.playhead = FrameIndex::ZERO;
        self.seek_to_secs(0.0, true);
    }

    pub(crate) fn step_frames(&mut self, delta: i64) {
        self.graph.timeline.playback_state = PlaybackState::Paused;
        self.playback_anchor = None;
        self.graph.timeline.step(delta);
        let secs = self.graph.timeline.playhead_secs() as f32;
        self.seek_to_secs(secs, true);
    }

    pub(crate) fn seek_to_frame(&mut self, frame: u64) {
        self.graph.timeline.set_playhead(frame);
        self.playback_anchor = Some((Instant::now(), self.graph.timeline.playhead.0));
        let secs = self.graph.timeline.playhead_secs() as f32;
        self.seek_to_secs(secs, true);
    }

    pub(crate) fn sync_timeline_clips(&mut self) {
        self.graph.sync_timeline_clips();
        self.touch();
    }
}

fn draw_ruler(
    painter: &egui::Painter,
    ruler: Rect,
    fps: u32,
    zoom: f32,
    scroll: f32,
    x_of: impl Fn(u64) -> f32,
) {
    // Pick the smallest whole-second step that leaves room for a label.
    let mut step_secs = 1.0f32;
    while step_secs * zoom < 72.0 && step_secs < 600.0 {
        step_secs *= 2.0;
    }
    let step_frames = (step_secs * fps as f32).max(1.0);
    let show_timecode = zoom >= 40.0;
    let mut frame = (scroll / step_frames).ceil() * step_frames;

    while x_of(frame.max(0.0) as u64) < ruler.right() {
        let frame_index = frame.max(0.0) as u64;
        let x = x_of(frame_index);
        if x >= ruler.left() {
            painter.line_segment(
                [
                    egui::pos2(x, ruler.bottom() - 6.0),
                    egui::pos2(x, ruler.bottom()),
                ],
                Stroke::new(1.0, Color32::from_gray(95)),
            );
            let label = if show_timecode {
                Timecode::from_frames(FrameIndex(frame_index), fps).to_string()
            } else {
                format!("{}s", frame_index / fps.max(1) as u64)
            };
            painter.text(
                egui::pos2(x + 3.0, ruler.top() + 3.0),
                Align2::LEFT_TOP,
                label,
                FontId::monospace(9.0),
                Color32::from_gray(150),
            );
        }
        frame += step_frames;
    }
}
