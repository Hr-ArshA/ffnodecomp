//! Browser backend: all work goes through the JS `@ffmpeg/ffmpeg` bridge.
//! There is no process to kill, so stale preview requests are filtered by a
//! token before they start (and again before their result is sent).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use super::{
    ExportJob, ExportMsg, PreviewMsg, VideoEngineBackend, PREVIEW_FILE, PREVIEW_H, PREVIEW_W,
};

#[wasm_bindgen(module = "/web/ffmpeg_bridge.js")]
extern "C" {
    #[wasm_bindgen(js_name = ffExec)]
    async fn ff_exec(args: &js_sys::Array, on_progress: &js_sys::Function) -> JsValue;

    #[wasm_bindgen(js_name = ffWrite)]
    async fn ff_write(name: &str, data: &js_sys::Uint8Array) -> JsValue;

    #[wasm_bindgen(js_name = ffRead)]
    async fn ff_read(name: &str) -> JsValue;

    #[wasm_bindgen(js_name = ffDelete)]
    async fn ff_delete(name: &str) -> JsValue;

    #[wasm_bindgen(js_name = ffDownload)]
    async fn ff_download(name: &str, filename: &str) -> JsValue;
}

pub struct WebEngine {
    tx: Sender<PreviewMsg>,
    rx: Receiver<PreviewMsg>,
    token: Rc<Cell<u64>>,
    written: Rc<Cell<u64>>,
}

impl WebEngine {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            token: Rc::new(Cell::new(0)),
            written: Rc::new(Cell::new(u64::MAX)),
        }
    }

    fn next_token(&self) -> u64 {
        let token = self.token.get().wrapping_add(1);
        self.token.set(token);
        token
    }
}

impl Default for WebEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoEngineBackend for WebEngine {
    fn poll(&mut self) -> Option<PreviewMsg> {
        self.rx.try_recv().ok()
    }

    fn request(&mut self, generation: u64, _gpu: Option<Vec<String>>, cpu: Vec<String>) {
        let token = self.next_token();
        let current = Rc::clone(&self.token);
        let written = Rc::clone(&self.written);
        let tx = self.tx.clone();
        spawn_local(async move {
            sync_assets(&written).await;
            // A newer request supersedes this one before FFmpeg starts.
            if current.get() != token {
                return;
            }
            let result = run_frame(&cpu).await;
            if current.get() != token {
                return;
            }
            let _ = match result {
                Ok(pixels) => tx.send(PreviewMsg::Frame {
                    generation,
                    pixels,
                    note: None,
                }),
                Err(message) => tx.send(PreviewMsg::Error {
                    generation,
                    message,
                }),
            };
        });
    }

    fn cancel(&mut self) {
        self.next_token();
    }

    fn reset_fallback(&mut self) {}
}

pub fn spawn_export(job: ExportJob, tx: Sender<ExportMsg>) {
    spawn_local(async move {
        let _ = sync_all_assets().await;
        let result = run_export(&job, tx.clone()).await;
        let _ = tx.send(ExportMsg::Finished(result));
    });
}

async fn run_frame(args: &[String]) -> Result<Vec<u8>, String> {
    let noop = Closure::<dyn FnMut(f64)>::new(|_| {});
    let result = ff_exec(&string_array(args), noop.as_ref().unchecked_ref()).await;
    let (code, log) = exec_result(&result);
    if code != 0.0 {
        return Err(format!("ffmpeg failed: {}", tail(&log)));
    }
    let data = check(
        ff_read(PREVIEW_FILE).await,
        &format!("could not read {PREVIEW_FILE}"),
    )?;
    let data = data
        .dyn_into::<js_sys::Uint8Array>()
        .map_err(|_| format!("{PREVIEW_FILE} is not a byte buffer"))?;
    let expected = PREVIEW_W as usize * PREVIEW_H as usize * 4;
    let mut pixels = data.to_vec();
    if pixels.len() < expected {
        return Err(format!(
            "ffmpeg returned {} bytes, expected {expected}. Is the timestamp past the end of the clip?",
            pixels.len()
        ));
    }
    pixels.truncate(expected);
    Ok(pixels)
}

async fn run_export(job: &ExportJob, tx: Sender<ExportMsg>) -> Result<String, String> {
    let progress = {
        let tx = tx.clone();
        Closure::<dyn FnMut(f64)>::new(move |value: f64| {
            let _ = tx.send(ExportMsg::Progress(value as f32));
        })
    };
    let result = ff_exec(&string_array(&job.args), progress.as_ref().unchecked_ref()).await;
    let (code, log) = exec_result(&result);
    if code != 0.0 {
        return Err(format!("Export failed: {}", tail(&log)));
    }
    let _ = tx.send(ExportMsg::Progress(1.0));
    check(
        ff_download(&job.out, &job.download_name).await,
        "could not start download",
    )?;
    let _ = ff_delete(&job.out).await;
    Ok(format!("Export finished: {}", job.download_name))
}

async fn sync_assets(written: &Rc<Cell<u64>>) {
    let version = crate::assets::version();
    if written.get() == version {
        return;
    }
    write_all_assets().await;
    written.set(version);
}

async fn sync_all_assets() {
    write_all_assets().await;
}

async fn write_all_assets() {
    for (name, bytes) in crate::assets::snapshot() {
        let data = js_sys::Uint8Array::from(bytes.as_slice());
        let _ = ff_write(&name, &data).await;
    }
}

fn string_array(values: &[String]) -> js_sys::Array {
    let array = js_sys::Array::new();
    for value in values {
        array.push(&JsValue::from_str(value));
    }
    array
}

fn exec_result(result: &JsValue) -> (f64, String) {
    let code = js_sys::Reflect::get(result, &JsValue::from_str("code"))
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or(-1.0);
    let log = js_sys::Reflect::get(result, &JsValue::from_str("log"))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| "no output".into());
    (code, log)
}

fn check(result: JsValue, what: &str) -> Result<JsValue, String> {
    let ok = js_sys::Reflect::get(&result, &JsValue::from_str("ok"))
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if ok {
        return js_sys::Reflect::get(&result, &JsValue::from_str("value"))
            .map_err(|_| format!("{what}: malformed bridge reply"));
    }
    let error = js_sys::Reflect::get(&result, &JsValue::from_str("error"))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| format!("{result:?}"));
    Err(format!("{what}: {error}"))
}

fn tail(text: &str) -> String {
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
