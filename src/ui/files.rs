//! File picking abstraction. Native uses the blocking dialog; wasm uses the
//! async browser dialog and copies the bytes into the in-memory asset pool
//! under a virtual name that FFmpeg/WASM can open.

use std::sync::mpsc::Sender;

use crate::graph::node::NodeId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    Video,
    Image,
    Font,
    Lut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickTarget {
    Node(NodeId, PickKind),
    Export,
}

#[derive(Clone, Debug)]
pub struct Picked {
    /// Native: absolute path. Wasm: virtual MEMFS name stored in the node.
    pub path: String,
}

pub enum PickMsg {
    Done(PickTarget, Option<Picked>),
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // wasm-only
    Error(String),
}

fn filter(kind: PickKind) -> (&'static str, &'static [&'static str]) {
    match kind {
        PickKind::Video => ("Video", &["mp4", "mkv", "mov", "avi", "webm", "flv", "m4v"]),
        PickKind::Image => ("Images", &["png", "jpg", "jpeg", "webp", "bmp", "gif"]),
        PickKind::Font => ("Fonts", &["ttf", "otf", "woff", "woff2"]),
        PickKind::Lut => ("3D LUT", &["cube", "3dl"]),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn pick(target: PickTarget, kind: PickKind, current: &str, tx: Sender<PickMsg>) {
    let mut dialog = rfd::FileDialog::new();
    if let PickTarget::Export = target {
        dialog = dialog
            .set_file_name(current)
            .add_filter("MP4 Video", &["mp4"]);
        if let Some(parent) = std::path::Path::new(current).parent() {
            if parent.exists() && parent != std::path::Path::new("") {
                dialog = dialog.set_directory(parent);
            }
        }
        let picked = dialog.save_file().map(|path| Picked {
            path: path.to_string_lossy().into_owned(),
        });
        let _ = tx.send(PickMsg::Done(target, picked));
        return;
    }

    let (name, extensions) = filter(kind);
    dialog = dialog.add_filter(name, extensions);
    if let Some(parent) = std::path::Path::new(current).parent() {
        if parent.exists() && parent != std::path::Path::new("") {
            dialog = dialog.set_directory(parent);
        }
    }
    let picked = dialog.pick_file().map(|path| Picked {
        path: path.to_string_lossy().into_owned(),
    });
    let _ = tx.send(PickMsg::Done(target, picked));
}

#[cfg(target_arch = "wasm32")]
pub fn pick(target: PickTarget, kind: PickKind, current: &str, tx: Sender<PickMsg>) {
    let current = current.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        if let PickTarget::Export = target {
            let picked = rfd::AsyncFileDialog::new()
                .set_file_name(current)
                .add_filter("MP4 Video", &["mp4"])
                .save_file()
                .await
                .map(|file| Picked {
                    path: file.file_name(),
                });
            let _ = tx.send(PickMsg::Done(target, picked));
            return;
        }

        let (name, extensions) = filter(kind);
        let Some(file) = rfd::AsyncFileDialog::new()
            .add_filter(name, extensions)
            .pick_file()
            .await
        else {
            let _ = tx.send(PickMsg::Done(target, None));
            return;
        };
        let bytes = file.read().await;
        if bytes.is_empty() {
            let _ = tx.send(PickMsg::Error(format!("{} is empty", file.file_name())));
            return;
        }
        let virtual_name = virtual_name(target, &file.file_name());
        crate::assets::put(&virtual_name, bytes);
        let _ = tx.send(PickMsg::Done(target, Some(Picked { path: virtual_name })));
    });
}

#[cfg(target_arch = "wasm32")]
fn virtual_name(target: PickTarget, original: &str) -> String {
    let ext = std::path::Path::new(original)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("bin");
    match target {
        PickTarget::Node(id, PickKind::Video) => format!("input_{id}.{ext}"),
        PickTarget::Node(id, PickKind::Image) => format!("image_{id}.{ext}"),
        PickTarget::Node(id, PickKind::Font) => format!("font_{id}.{ext}"),
        PickTarget::Node(id, PickKind::Lut) => format!("lut_{id}.{ext}"),
        PickTarget::Export => "export.mp4".into(),
    }
}
