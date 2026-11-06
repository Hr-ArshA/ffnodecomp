//! FFmpeg preview extraction. All process work happens on a Tokio worker so
//! the egui thread never waits for a decode.

use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::process::Command;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

/// Preview output is fixed-size so the UI knows the raw buffer layout without
/// probing the source. Aspect ratio is preserved and letterboxed.
pub const PREVIEW_W: u32 = 640;
pub const PREVIEW_H: u32 = 360;

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
    pub fn decode_flags(self) -> &'static [&'static str] {
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

pub struct PreviewEngine {
    rt: Runtime,
    tx: UnboundedSender<PreviewMsg>,
    rx: UnboundedReceiver<PreviewMsg>,
    task: Option<JoinHandle<()>>,
    /// Once a GPU request fails, stop paying the failed attempt on every
    /// slider tick. The UI shows the fallback note from the first frame.
    force_cpu: Arc<AtomicBool>,
}

impl PreviewEngine {
    pub fn new() -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("could not start preview runtime: {e}"))?;
        let (tx, rx) = unbounded_channel();
        Ok(Self {
            rt,
            tx,
            rx,
            task: None,
            force_cpu: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn poll(&mut self) -> Option<PreviewMsg> {
        self.rx.try_recv().ok()
    }

    /// A GPU backend change should get a fresh chance even if the previous
    /// backend already fell back to CPU.
    pub fn reset_fallback(&mut self) {
        self.force_cpu.store(false, Ordering::Relaxed);
    }

    /// Abort the in-flight extraction, e.g. when the graph stops compiling.
    pub fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    /// Replace the in-flight extraction. Aborting the task drops the child
    /// process; `kill_on_drop` makes sure FFmpeg actually exits.
    pub fn request(&mut self, generation: u64, gpu: Option<Vec<String>>, cpu: Vec<String>) {
        if let Some(task) = self.task.take() {
            task.abort();
        }

        let tx = self.tx.clone();
        let force_cpu = Arc::clone(&self.force_cpu);

        self.task = Some(self.rt.spawn(async move {
            let prefer_cpu = gpu.is_none() || force_cpu.load(Ordering::Relaxed);

            if prefer_cpu {
                match run_frame(&cpu).await {
                    Ok(pixels) => {
                        let _ = tx.send(PreviewMsg::Frame {
                            generation,
                            pixels,
                            note: None,
                        });
                    }
                    Err(message) => {
                        let _ = tx.send(PreviewMsg::Error {
                            generation,
                            message,
                        });
                    }
                }
                return;
            }

            let gpu_args = gpu.unwrap_or_default();
            match run_frame(&gpu_args).await {
                Ok(pixels) => {
                    let _ = tx.send(PreviewMsg::Frame {
                        generation,
                        pixels,
                        note: None,
                    });
                }
                Err(gpu_error) => {
                    // Graceful fallback: report the GPU failure once, then
                    // keep using the CPU path.
                    force_cpu.store(true, Ordering::Relaxed);
                    match run_frame(&cpu).await {
                        Ok(pixels) => {
                            let _ = tx.send(PreviewMsg::Frame {
                                generation,
                                pixels,
                                note: Some(format!(
                                    "GPU unavailable, using CPU. ({})",
                                    short(&gpu_error)
                                )),
                            });
                        }
                        Err(cpu_error) => {
                            let _ = tx.send(PreviewMsg::Error {
                                generation,
                                message: cpu_error,
                            });
                        }
                    }
                }
            }
        }));
    }
}

async fn run_frame(args: &[String]) -> Result<Vec<u8>, String> {
    let child = Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start ffmpeg: {e}"))?;

    // tokio drains stdout and stderr concurrently, so a chatty FFmpeg cannot
    // deadlock the pipe before we read the frame.
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("ffmpeg wait failed: {e}"))?;

    if !output.status.success() {
        return Err(format!("ffmpeg failed: {}", error_tail(&output.stderr)));
    }

    let expected = PREVIEW_W as usize * PREVIEW_H as usize * 4;
    if output.stdout.len() < expected {
        return Err(format!(
            "ffmpeg returned {} bytes, expected {expected}. Is the timestamp past the end of the clip?",
            output.stdout.len()
        ));
    }

    let mut pixels = output.stdout;
    pixels.truncate(expected);
    Ok(pixels)
}

/// Runs export with live progress reports via `-progress pipe:1`.
pub fn run_export(
    args: &[String],
    total_duration: f32,
    mut on_progress: impl FnMut(f32),
) -> Result<(), String> {
    let mut cmd_args = Vec::with_capacity(args.len() + 2);
    let mut added = false;
    for arg in args {
        if arg == "-y" && !added {
            cmd_args.push("-progress".to_string());
            cmd_args.push("pipe:1".to_string());
            added = true;
        }
        cmd_args.push(arg.clone());
    }
    if !added {
        cmd_args.push("-progress".to_string());
        cmd_args.push("pipe:1".to_string());
    }

    let mut child = std::process::Command::new("ffmpeg")
        .args(&cmd_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start ffmpeg: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or("could not capture ffmpeg stdout")?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or("could not capture ffmpeg stderr")?;

    let err_handle = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    use std::io::{BufRead, BufReader};
    let reader = BufReader::new(stdout);
    for line in reader.lines().map_while(Result::ok) {
        if let Some(rest) = line.strip_prefix("out_time_us=") {
            if let Ok(us) = rest.trim().parse::<f32>() {
                if total_duration > 0.0 {
                    let frac = (us / 1_000_000.0 / total_duration).clamp(0.0, 0.99);
                    on_progress(frac);
                }
            }
        } else if line.starts_with("progress=end") {
            on_progress(1.0);
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("ffmpeg wait failed: {e}"))?;
    let stderr_bytes = err_handle.join().unwrap_or_default();

    if status.success() {
        on_progress(1.0);
        Ok(())
    } else {
        Err(format!("ffmpeg failed: {}", error_tail(&stderr_bytes)))
    }
}

pub fn probe_duration(path: &str) -> Option<f32> {
    if path.trim().is_empty() {
        return None;
    }
    if let Ok(out) = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
    {
        if out.status.success() {
            if let Ok(text) = std::str::from_utf8(&out.stdout) {
                if let Ok(dur) = text.trim().parse::<f32>() {
                    if dur > 0.0 {
                        return Some(dur);
                    }
                }
            }
        }
    }

    if let Ok(out) = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-i", path])
        .output()
    {
        let text = String::from_utf8_lossy(&out.stderr);
        if let Some(pos) = text.find("Duration: ") {
            let rem = &text[pos + 10..];
            let time_str = rem.split([',', ' ']).next().unwrap_or("");
            let parts: Vec<&str> = time_str.split(':').collect();
            if parts.len() == 3 {
                if let (Ok(h), Ok(m), Ok(s)) = (
                    parts[0].parse::<f32>(),
                    parts[1].parse::<f32>(),
                    parts[2].parse::<f32>(),
                ) {
                    let total = h * 3600.0 + m * 60.0 + s;
                    if total > 0.0 {
                        return Some(total);
                    }
                }
            }
        }
    }

    None
}

pub fn estimated_duration(graph: &crate::graph::node::Graph) -> f32 {
    use crate::graph::node::NodeKind;
    let mut max_dur: f32 = 0.0;
    for node in &graph.nodes {
        match &node.kind {
            NodeKind::SolidColor { duration, .. }
            | NodeKind::Image { duration, .. }
            | NodeKind::Text { duration, .. } => {
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

fn error_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    let tail: String = text
        .chars()
        .rev()
        .take(600)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if tail.is_empty() {
        "no error output".into()
    } else {
        tail
    }
}

fn short(message: &str) -> String {
    let first = message.lines().next().unwrap_or(message);
    first.chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_duration_nonexistent_returns_none() {
        assert_eq!(probe_duration(""), None);
        assert_eq!(probe_duration("/tmp/nonexistent_file_12345.mp4"), None);
    }

    #[test]
    fn run_export_reports_progress_and_completes() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }

        let out = "/tmp/opencode/export_test.mp4";
        let args = vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            "testsrc=duration=1:size=320x240:rate=30".into(),
            "-c:v".into(),
            "libx264".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-y".into(),
            out.into(),
        ];

        let mut progress_called = false;
        let mut final_reached = false;
        let res = run_export(&args, 1.0, |frac| {
            progress_called = true;
            if frac >= 0.99 {
                final_reached = true;
            }
        });

        assert!(res.is_ok(), "run_export succeeded");
        assert!(progress_called, "progress callback was invoked");
        assert!(final_reached, "progress reached 1.0");
        assert!(std::path::Path::new(out).exists(), "output file exists");
        let _ = std::fs::remove_file(out);
    }
}
