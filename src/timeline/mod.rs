//! Frame-accurate non-linear timeline state.
//!
//! Everything is counted in whole frames (`FrameIndex`) so scrubbing, trimming
//! and looping never drift the way accumulated seconds would. A clip points at
//! a graph source node by `NodeId`; the compiler turns the clip window into
//! `trim` + `setpts` directives.

use serde::{Deserialize, Serialize};

use crate::graph::node::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FrameIndex(pub u64);

impl FrameIndex {
    pub const ZERO: FrameIndex = FrameIndex(0);

    pub fn secs(self, fps: u32) -> f64 {
        self.0 as f64 / fps.max(1) as f64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub fps: u32,
}

impl Timecode {
    pub fn from_frames(frame: FrameIndex, fps: u32) -> Self {
        let fps = fps.max(1);
        let total_seconds = frame.0 / fps as u64;
        Self {
            hours: ((total_seconds / 3600) % 24) as u8,
            minutes: ((total_seconds / 60) % 60) as u8,
            seconds: (total_seconds % 60) as u8,
            frames: (frame.0 % fps as u64) as u8,
            fps,
        }
    }
}

impl std::fmt::Display for Timecode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:02}:{:02}:{:02}:{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PlaybackState {
    #[default]
    Paused,
    Playing,
    Scrubbing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackKind {
    Video,
    Audio,
}

pub type TrackId = usize;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub clip_id: u64,
    /// Source node this clip places on the timeline.
    pub node: NodeId,
    pub start_frame: u64,
    pub duration_frames: u64,
    /// Offset into the source asset, in frames.
    pub source_trim_in: u64,
    /// Playback rate multiplier. `1.0` is real time.
    pub speed: f32,
}

impl Clip {
    pub fn end_frame(&self) -> u64 {
        self.start_frame + self.duration_frames
    }

    /// Source frames consumed by this clip, after `speed` is applied.
    pub fn source_frames(&self) -> u64 {
        ((self.duration_frames as f64) * self.speed.clamp(0.1, 10.0) as f64).round() as u64
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub kind: TrackKind,
    pub clips: Vec<Clip>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipDragMode {
    Body,
    TrimIn,
    TrimOut,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineState {
    pub playhead: FrameIndex,
    /// Master framerate of the sequence.
    pub timebase: u32,
    /// Total sequence length in frames.
    pub duration: u64,
    /// Horizontal scale in pixels per second.
    pub zoom: f32,
    /// Horizontal viewport pan, measured in frames.
    pub scroll_offset: f32,
    pub playback_state: PlaybackState,
    pub loop_range: Option<(FrameIndex, FrameIndex)>,
    pub tracks: Vec<Track>,
    pub next_clip_id: u64,
    pub selected: Option<(TrackId, u64)>,
    pub snapping: bool,
}

pub const DEFAULT_ZOOM: f32 = 60.0;

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            playhead: FrameIndex::ZERO,
            timebase: 30,
            duration: 300,
            zoom: DEFAULT_ZOOM,
            scroll_offset: 0.0,
            playback_state: PlaybackState::Paused,
            loop_range: None,
            tracks: vec![
                Track {
                    id: 0,
                    name: "Video 1".into(),
                    kind: TrackKind::Video,
                    clips: Vec::new(),
                },
                Track {
                    id: 1,
                    name: "Audio 1".into(),
                    kind: TrackKind::Audio,
                    clips: Vec::new(),
                },
            ],
            next_clip_id: 1,
            selected: None,
            snapping: true,
        }
    }
}

impl TimelineState {
    pub fn fps(&self) -> u32 {
        self.timebase.max(1)
    }

    pub fn playhead_secs(&self) -> f64 {
        self.playhead.secs(self.fps())
    }

    pub fn px_per_frame(&self) -> f32 {
        (self.zoom / self.fps() as f32).max(0.001)
    }

    pub fn timecode(&self) -> Timecode {
        Timecode::from_frames(self.playhead, self.fps())
    }

    pub fn set_playhead(&mut self, frame: u64) {
        self.playhead = FrameIndex(frame.min(self.duration.saturating_sub(1)));
    }

    pub fn step(&mut self, delta: i64) {
        let frame = self.playhead.0 as i64 + delta;
        self.set_playhead(frame.max(0) as u64);
    }

    /// Loop in point at the playhead, keeping a later out point if one exists.
    pub fn set_in(&mut self) {
        let out = match self.loop_range {
            Some((_, out)) if out > self.playhead => out,
            _ => FrameIndex(self.duration.max(self.playhead.0 + 1)),
        };
        self.loop_range = Some((self.playhead, out));
    }

    pub fn set_out(&mut self) {
        let in_frame = match self.loop_range {
            Some((inf, _)) if inf < self.playhead => inf,
            _ => FrameIndex::ZERO,
        };
        // The out point is exclusive, so it must stay past the in point.
        if in_frame >= self.playhead {
            return;
        }
        self.loop_range = Some((in_frame, self.playhead));
    }

    pub fn clip_for_node(&self, node: NodeId) -> Option<&Clip> {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .find(|clip| clip.node == node)
    }

    pub fn selected_clip(&self) -> Option<&Clip> {
        let (track, clip_id) = self.selected?;
        self.tracks
            .iter()
            .find(|t| t.id == track)?
            .clips
            .iter()
            .find(|c| c.clip_id == clip_id)
    }

    /// Move one clip by `delta` frames, in body or trim mode. Clamps so a clip
    /// never leaves the sequence or shrinks below one frame.
    pub fn drag_clip(&mut self, track_id: TrackId, clip_id: u64, mode: ClipDragMode, delta: i64) {
        if delta == 0 {
            return;
        }
        let snapping = self.snapping;
        let threshold = (8.0 / self.px_per_frame()).ceil() as u64;
        let fps = self.fps() as u64;
        let mut anchors = vec![self.playhead.0];
        for track in &self.tracks {
            for clip in &track.clips {
                if clip.clip_id != clip_id {
                    anchors.push(clip.start_frame);
                    anchors.push(clip.end_frame());
                }
            }
        }
        let snap = |frame: u64| -> u64 {
            if !snapping {
                return frame;
            }
            let mut candidates = anchors.clone();
            candidates.push((frame / fps) * fps);
            candidates.push((frame / fps + 1) * fps);
            candidates
                .into_iter()
                .min_by_key(|candidate| candidate.abs_diff(frame))
                .filter(|candidate| candidate.abs_diff(frame) <= threshold)
                .unwrap_or(frame)
        };

        let Some(track) = self.tracks.iter_mut().find(|t| t.id == track_id) else {
            return;
        };
        let Some(clip) = track.clips.iter_mut().find(|c| c.clip_id == clip_id) else {
            return;
        };

        match mode {
            ClipDragMode::Body => {
                clip.start_frame = snap((clip.start_frame as i64 + delta).max(0) as u64);
            }
            ClipDragMode::TrimIn => {
                // Keep at least one frame and never trim before the source start.
                let clamp = delta.clamp(
                    -(clip.source_trim_in as i64),
                    clip.duration_frames as i64 - 1,
                );
                let start = snap((clip.start_frame as i64 + clamp).max(0) as u64);
                let applied = start as i64 - clip.start_frame as i64;
                clip.start_frame = start;
                clip.source_trim_in = (clip.source_trim_in as i64 + applied).max(0) as u64;
                clip.duration_frames = (clip.duration_frames as i64 - applied).max(1) as u64;
            }
            ClipDragMode::TrimOut => {
                let end =
                    snap((clip.end_frame() as i64 + delta).max(clip.start_frame as i64 + 1) as u64);
                clip.duration_frames = end.saturating_sub(clip.start_frame).max(1);
            }
        }

        self.duration = self
            .duration
            .max(
                self.tracks
                    .iter()
                    .flat_map(|track| track.clips.iter())
                    .map(|clip| clip.end_frame())
                    .max()
                    .unwrap_or(1),
            )
            .max(1);
        self.set_playhead(self.playhead.0);
    }

    /// Add a clip for every source without one and drop clips whose node is
    /// gone. New clips start at zero and last their natural source duration.
    pub fn sync_sources(&mut self, sources: &[(NodeId, u64)]) {
        let mut next_clip_id = self.next_clip_id;
        for track in &mut self.tracks {
            if track.kind != TrackKind::Video {
                // No audio-producing node exists yet.
                track.clips.clear();
                continue;
            }
            track
                .clips
                .retain(|clip| sources.iter().any(|(node, _)| *node == clip.node));
            for &(node, natural_frames) in sources {
                if track.clips.iter().any(|clip| clip.node == node) {
                    continue;
                }
                track.clips.push(Clip {
                    clip_id: next_clip_id,
                    node,
                    start_frame: 0,
                    duration_frames: natural_frames.max(1),
                    source_trim_in: 0,
                    speed: 1.0,
                });
                next_clip_id += 1;
            }
        }
        self.next_clip_id = next_clip_id;
        self.repair_selection();
    }

    pub fn on_node_removed(&mut self, removed: NodeId) {
        for track in &mut self.tracks {
            track.clips.retain(|clip| clip.node != removed);
            for clip in &mut track.clips {
                if clip.node > removed {
                    clip.node -= 1;
                }
            }
        }
        self.repair_selection();
    }

    /// Drop references that a hand-edited graph.json could leave dangling.
    pub fn repair(&mut self, node_count: usize) {
        for track in &mut self.tracks {
            track
                .clips
                .retain(|clip| clip.node < node_count && clip.duration_frames > 0);
        }
        self.duration = self.duration.max(1);
        self.playhead = FrameIndex(self.playhead.0.min(self.duration - 1));
        if let Some((in_frame, out_frame)) = self.loop_range {
            if in_frame.0 >= out_frame.0 || out_frame.0 > self.duration {
                self.loop_range = None;
            }
        }
        self.repair_selection();
    }

    fn repair_selection(&mut self) {
        if self.selected_clip().is_none() {
            self.selected = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecode_splits_frames_without_floating_point() {
        let tc = Timecode::from_frames(FrameIndex(30 * 3661 + 12), 30);
        assert_eq!(
            tc,
            Timecode {
                hours: 1,
                minutes: 1,
                seconds: 1,
                frames: 12,
                fps: 30,
            }
        );
        assert_eq!(tc.to_string(), "01:01:01:12");
    }

    #[test]
    fn drag_clip_trims_and_moves_without_going_negative() {
        let mut timeline = TimelineState::default();
        timeline.tracks[0].clips.push(Clip {
            clip_id: 7,
            node: 0,
            start_frame: 30,
            duration_frames: 60,
            source_trim_in: 10,
            speed: 1.0,
        });

        timeline.drag_clip(0, 7, ClipDragMode::Body, -100);
        assert_eq!(timeline.tracks[0].clips[0].start_frame, 0);

        timeline.drag_clip(0, 7, ClipDragMode::TrimIn, 5);
        let clip = &timeline.tracks[0].clips[0];
        assert_eq!(clip.start_frame, 5);
        assert_eq!(clip.source_trim_in, 15);
        assert_eq!(clip.duration_frames, 55);

        timeline.drag_clip(0, 7, ClipDragMode::TrimOut, 5);
        assert_eq!(timeline.tracks[0].clips[0].duration_frames, 60);
        assert!(timeline.duration >= 60);
    }

    #[test]
    fn sync_sources_adds_once_and_repair_drops_orphans() {
        let mut timeline = TimelineState::default();
        timeline.sync_sources(&[(0, 90), (2, 30)]);
        timeline.sync_sources(&[(0, 90), (2, 30)]);
        assert_eq!(timeline.tracks[0].clips.len(), 2);

        timeline.repair(2);
        assert_eq!(timeline.tracks[0].clips.len(), 1);
        assert_eq!(timeline.tracks[0].clips[0].node, 0);
    }

    #[test]
    fn in_out_keeps_valid_ordering() {
        let mut timeline = TimelineState::default();
        timeline.set_playhead(40);
        timeline.set_out();
        timeline.set_playhead(10);
        timeline.set_in();
        assert_eq!(timeline.loop_range, Some((FrameIndex(10), FrameIndex(40))));
        // An out point at frame zero would make an empty loop, so it is ignored.
        timeline.set_playhead(0);
        timeline.set_out();
        assert_eq!(timeline.loop_range, Some((FrameIndex(10), FrameIndex(40))));
    }

    #[test]
    fn snapping_sticks_to_clip_edges_and_playhead() {
        let mut timeline = TimelineState {
            playhead: FrameIndex(50),
            snapping: true,
            ..Default::default()
        };
        timeline.tracks[0].clips.push(Clip {
            clip_id: 1,
            node: 0,
            start_frame: 100,
            duration_frames: 30,
            source_trim_in: 0,
            speed: 1.0,
        });
        timeline.tracks[0].clips.push(Clip {
            clip_id: 2,
            node: 1,
            start_frame: 200,
            duration_frames: 10,
            source_trim_in: 0,
            speed: 1.0,
        });
        // Default zoom is 60 px/s at 30 fps, so one frame is 2 px and the
        // magnet radius is 4 frames.
        timeline.drag_clip(0, 1, ClipDragMode::Body, -48);
        assert_eq!(timeline.tracks[0].clips[0].start_frame, 50);
        timeline.drag_clip(0, 1, ClipDragMode::Body, 148);
        assert_eq!(timeline.tracks[0].clips[0].start_frame, 200);
    }
}
