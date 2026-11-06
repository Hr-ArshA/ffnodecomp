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

/// Blocking helper for exports. Called from a plain thread, not the UI thread.
pub fn run_blocking(args: &[String]) -> Result<(), String> {
    let output = std::process::Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not start ffmpeg: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("ffmpeg failed: {}", error_tail(&output.stderr)))
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
