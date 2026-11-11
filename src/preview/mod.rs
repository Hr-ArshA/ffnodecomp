//! Platform-neutral preview/export types plus the backend selected at compile
//! time. Native uses Tokio + child processes; wasm uses ffmpeg.wasm.

#[cfg(not(target_arch = "wasm32"))]
pub mod engine;
#[cfg(target_arch = "wasm32")]
pub mod web_engine;

/// Preview output is fixed-size so the UI knows the raw buffer layout without
/// probing the source. Aspect ratio is preserved and letterboxed.
pub const PREVIEW_W: u32 = 640;
pub const PREVIEW_H: u32 = 360;
/// ffmpeg.wasm cannot pipe raw frames back to Rust, so the wasm preview writes
/// this MEMFS file and reads it afterwards.
pub const PREVIEW_FILE: &str = "preview.rgba";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accel {
    Cpu,
    Cuda,
    VideoToolbox,
    Vaapi,
}

impl Accel {
    pub const ALL: [Accel; 4] = [Accel::Cpu, Accel::Cuda, Accel::VideoToolbox, Accel::Vaapi];

    pub fn label(self) -> &'static str {
        match self {
            Accel::Cpu => "CPU / software",
            Accel::Cuda => "NVIDIA (CUDA / NVENC)",
            Accel::VideoToolbox => "Apple (VideoToolbox)",
            Accel::Vaapi => "Linux (VAAPI)",
        }
    }

    /// Decode-side flags. `-hwaccel_output_format` is deliberately omitted:
    /// the filter graph runs on the CPU and FFmpeg downloads frames for us.
    /// ffmpeg.wasm has no hardware backends, so all flags are empty there.
    pub fn decode_flags(self) -> &'static [&'static str] {
        if cfg!(target_arch = "wasm32") {
            return &[];
        }
        match self {
            Accel::Cpu => &[],
            Accel::Cuda => &["-hwaccel", "cuda"],
            Accel::VideoToolbox => &["-hwaccel", "videotoolbox"],
            Accel::Vaapi => &["-hwaccel", "vaapi"],
        }
    }
}

pub enum PreviewMsg {
    Frame {
        generation: u64,
        pixels: Vec<u8>,
        note: Option<String>,
    },
    Error {
        generation: u64,
        message: String,
    },
}

pub enum ExportMsg {
    Progress(f32),
    Finished(Result<String, String>),
}

/// Everything an export needs, independent of the process model.
pub struct ExportJob {
    pub args: Vec<String>,
    /// Native GPU export retries with this on failure. Wasm has no GPU path.
    pub fallback: Option<Vec<String>>,
    pub total: f32,
    /// MEMFS path (wasm) or filesystem path (native) written by FFmpeg.
    pub out: String,
    /// Browser download name. Ignored natively.
    pub download_name: String,
}

/// The UI only cares about these four operations, so both engines can hide
/// their scheduler (Tokio task vs `spawn_local`).
pub trait VideoEngineBackend {
    fn poll(&mut self) -> Option<PreviewMsg>;
    fn request(&mut self, generation: u64, gpu: Option<Vec<String>>, cpu: Vec<String>);
    fn cancel(&mut self);
    fn reset_fallback(&mut self);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn create_engine() -> Result<Box<dyn VideoEngineBackend>, String> {
    Ok(Box::new(engine::PreviewEngine::new()?))
}

#[cfg(target_arch = "wasm32")]
pub fn create_engine() -> Result<Box<dyn VideoEngineBackend>, String> {
    Ok(Box::new(web_engine::WebEngine::new()))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_export(job: ExportJob, tx: std::sync::mpsc::Sender<ExportMsg>) {
    engine::spawn_export(job, tx);
}

#[cfg(target_arch = "wasm32")]
pub fn spawn_export(job: ExportJob, tx: std::sync::mpsc::Sender<ExportMsg>) {
    web_engine::spawn_export(job, tx);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn probe_duration(path: &str) -> Option<f32> {
    engine::probe_duration(path)
}

#[cfg(target_arch = "wasm32")]
pub fn probe_duration(_path: &str) -> Option<f32> {
    None
}

/// Progress-bar denominator. On wasm, file inputs are not probed (no
/// ffprobe), so configured durations are used and 5 s is the floor.
pub fn estimated_duration(graph: &crate::graph::node::Graph) -> f32 {
    use crate::graph::node::NodeKind;
    let mut max_dur: f32 = 0.0;
    for node in &graph.nodes {
        match &node.kind {
            NodeKind::SolidColor { duration, .. }
            | NodeKind::Image { duration, .. }
            | NodeKind::Text { duration, .. }
            | NodeKind::ShapeMask { duration, .. } => {
                max_dur = max_dur.max(*duration);
            }
            NodeKind::Input { path } => {
                if let Some(dur) = probe_duration(path) {
                    max_dur = max_dur.max(dur);
                }
            }
            _ => {}
        }
    }
    if max_dur > 0.0 {
        max_dur
    } else {
        5.0
    }
}
