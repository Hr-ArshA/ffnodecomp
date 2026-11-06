//! Graph data model. Deliberately free of egui types so it can be compiled,
//! serialized and unit-tested without a window.

use serde::{Deserialize, Serialize};

/// Nodes are addressed by index. Removal renumbers links, so ids stay durable
/// only while the graph is alive (and across save/load, since order is stable).
pub type NodeId = usize;

/// Ports are typed even though the MVP only produces video. Adding an audio
/// node later gets connection validation for free.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortKind {
    Video,
    #[allow(dead_code)] // no audio-producing node in the MVP yet
    Audio,
    /// A single-channel matte. Accepts Video sources.
    Mask,
}

/// Video may feed a Mask input; every other pair must match exactly.
pub fn compatible(from: PortKind, to: PortKind) -> bool {
    from == to || (from == PortKind::Video && to == PortKind::Mask)
}

#[derive(Clone, Copy, Debug)]
pub struct PortSpec {
    pub name: &'static str,
    pub kind: PortKind,
    /// Optional inputs may be left unconnected; the compiler skips them.
    pub optional: bool,
}

const NO_PORTS: [PortSpec; 0] = [];
const VIDEO_IN: [PortSpec; 1] = [PortSpec {
    name: "video",
    kind: PortKind::Video,
    optional: false,
}];
const VIDEO_OUT: [PortSpec; 1] = [PortSpec {
    name: "video",
    kind: PortKind::Video,
    optional: false,
}];
const OVERLAY_IN: [PortSpec; 2] = [
    PortSpec {
        name: "background",
        kind: PortKind::Video,
        optional: false,
    },
    PortSpec {
        name: "foreground",
        kind: PortKind::Video,
        optional: false,
    },
];
const ALPHA_KEY_IN: [PortSpec; 2] = [
    PortSpec {
        name: "video",
        kind: PortKind::Video,
        optional: false,
    },
    PortSpec {
        name: "mask (opt)",
        kind: PortKind::Mask,
        optional: true,
    },
];
const TEXT_IN: [PortSpec; 1] = [PortSpec {
    name: "video (opt)",
    kind: PortKind::Video,
    optional: true,
}];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyMode {
    LumaKey,
    ExtractAlpha,
    InvertAlpha,
}

impl KeyMode {
    pub const ALL: [KeyMode; 3] = [
        KeyMode::LumaKey,
        KeyMode::ExtractAlpha,
        KeyMode::InvertAlpha,
    ];

    pub fn label(self) -> &'static str {
        match self {
            KeyMode::LumaKey => "Luma key",
            KeyMode::ExtractAlpha => "Extract alpha",
            KeyMode::InvertAlpha => "Invert alpha",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextPos {
    Center,
    TopCenter,
    BottomCenter,
    CustomXY,
}

impl TextPos {
    pub const ALL: [TextPos; 4] = [
        TextPos::Center,
        TextPos::TopCenter,
        TextPos::BottomCenter,
        TextPos::CustomXY,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TextPos::Center => "Center",
            TextPos::TopCenter => "Top center",
            TextPos::BottomCenter => "Bottom center",
            TextPos::CustomXY => "Custom X/Y",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NodeKind {
    Input {
        path: String,
    },
    Transform {
        width: u32,
        height: u32,
        x: u32,
        y: u32,
    },
    Color {
        brightness: f32,
        contrast: f32,
        saturation: f32,
    },
    Overlay {
        x: i32,
        y: i32,
    },
    ColorKey {
        color: [u8; 3],
        similarity: f32,
        blend: f32,
    },
    AlphaKey {
        mode: KeyMode,
        threshold: f32,
        tolerance: f32,
        softness: f32,
    },
    Text {
        text: String,
        font_size: f32,
        font_color: [u8; 3],
        font_file: String,
        position: TextPos,
        custom_x: String,
        custom_y: String,
        /// Used only when no video is plugged in and Text acts as a source.
        duration: f32,
    },
    Image {
        path: String,
        duration: f32,
        framerate: u32,
    },
    SolidColor {
        color: [u8; 3],
        width: u32,
        height: u32,
        duration: f32,
        framerate: u32,
    },
    Output {
        width: u32,
        height: u32,
    },
}

/// `#RRGGBB` for display; FFmpeg wants `0xRRGGBB`.
pub fn hex_color(color: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2])
}

fn file_label(path: &str) -> String {
    if path.trim().is_empty() {
        return "no file".into();
    }
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

impl NodeKind {
    pub fn title(&self) -> &'static str {
        match self {
            NodeKind::Input { .. } => "Input",
            NodeKind::Transform { .. } => "Transform",
            NodeKind::Color { .. } => "Color Correction",
            NodeKind::Overlay { .. } => "Overlay",
            NodeKind::ColorKey { .. } => "Chroma Key",
            NodeKind::AlphaKey { .. } => "Alpha Key",
            NodeKind::Text { .. } => "Text",
            NodeKind::Image { .. } => "Image",
            NodeKind::SolidColor { .. } => "Solid Color",
            NodeKind::Output { .. } => "Output",
        }
    }

    pub fn inputs(&self) -> &'static [PortSpec] {
        match self {
            NodeKind::Input { .. } | NodeKind::Image { .. } | NodeKind::SolidColor { .. } => {
                &NO_PORTS
            }
            NodeKind::Output { .. } => &VIDEO_IN,
            NodeKind::Overlay { .. } => &OVERLAY_IN,
            NodeKind::AlphaKey { .. } => &ALPHA_KEY_IN,
            NodeKind::Text { .. } => &TEXT_IN,
            _ => &VIDEO_IN,
        }
    }

    pub fn outputs(&self) -> &'static [PortSpec] {
        match self {
            NodeKind::Output { .. } => &NO_PORTS,
            _ => &VIDEO_OUT,
        }
    }

    /// One-line status shown on the node card.
    pub fn summary(&self) -> String {
        match self {
            NodeKind::Input { path } => file_label(path),
            NodeKind::Transform {
                width,
                height,
                x,
                y,
            } => {
                format!("{width}x{height} @ {x},{y}")
            }
            NodeKind::Color {
                brightness,
                contrast,
                saturation,
            } => {
                format!("b {brightness:+.2}  c {contrast:.2}  s {saturation:.2}")
            }
            NodeKind::Overlay { x, y } => format!("at {x},{y}"),
            NodeKind::ColorKey {
                color,
                similarity,
                blend,
            } => format!("{} s{similarity:.2} b{blend:.2}", hex_color(*color)),
            NodeKind::AlphaKey {
                mode,
                threshold,
                tolerance,
                softness,
            } => format!(
                "{} t{threshold:.2} tol{tolerance:.2} s{softness:.2}",
                mode.label()
            ),
            NodeKind::Text {
                text, font_size, ..
            } => {
                let text = text.trim();
                if text.is_empty() {
                    "no text".into()
                } else {
                    format!("\"{}\" {font_size:.0}px", truncate(text, 18))
                }
            }
            NodeKind::Image { path, .. } => file_label(path),
            NodeKind::SolidColor {
                color,
                width,
                height,
                ..
            } => format!("{} {width}x{height}", hex_color(*color)),
            NodeKind::Output { width, height } => format!("canvas {width}x{height}"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    /// Canvas-space top-left corner, independent of egui.
    pub pos: [f32; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Link {
    pub from: NodeId,
    pub from_port: usize,
    pub to: NodeId,
    pub to_port: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
}

impl Graph {
    pub fn add_node(&mut self, kind: NodeKind, pos: [f32; 2]) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(Node { id, kind, pos });
        id
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// Connect `from:from_port` to `to:to_port`, replacing anything already
    /// plugged into that input. Inputs are single-source; outputs fan out.
    pub fn connect(
        &mut self,
        from: NodeId,
        from_port: usize,
        to: NodeId,
        to_port: usize,
    ) -> Result<(), String> {
        self.connect_ports(
            super::mutations::PortId {
                node: from,
                port: from_port,
                input: false,
            },
            super::mutations::PortId {
                node: to,
                port: to_port,
                input: true,
            },
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    /// True when adding `from -> to` would create a cycle. Also rejects
    /// self-connections because a node can reach itself trivially.
    pub fn would_cycle(&self, from: NodeId, to: NodeId) -> bool {
        let mut stack = vec![to];
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = stack.pop() {
            if id == from {
                return true;
            }
            if !seen.insert(id) {
                continue;
            }
            for link in &self.links {
                if link.from == id {
                    stack.push(link.to);
                }
            }
        }
        false
    }

    pub fn remove_node(&mut self, id: NodeId) {
        if id >= self.nodes.len() {
            return;
        }
        self.nodes.remove(id);
        self.links.retain(|l| l.from != id && l.to != id);
        for link in &mut self.links {
            if link.from > id {
                link.from -= 1;
            }
            if link.to > id {
                link.to -= 1;
            }
        }
        for (index, node) in self.nodes.iter_mut().enumerate() {
            node.id = index;
        }
    }
}

/// Small starter graph so the editor is not a blank page. The Input path is
/// empty until the user fills it in, which the compiler reports as an error.
pub fn demo_graph() -> Graph {
    let mut graph = Graph::default();
    let input = graph.add_node(
        NodeKind::Input {
            path: String::new(),
        },
        [60.0, 80.0],
    );
    let transform = graph.add_node(
        NodeKind::Transform {
            width: 1280,
            height: 720,
            x: 0,
            y: 0,
        },
        [280.0, 80.0],
    );
    let color = graph.add_node(
        NodeKind::Color {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
        },
        [500.0, 80.0],
    );
    let output = graph.add_node(
        NodeKind::Output {
            width: 1280,
            height: 720,
        },
        [720.0, 80.0],
    );

    let _ = graph.connect(input, 0, transform, 0);
    let _ = graph.connect(transform, 0, color, 0);
    let _ = graph.connect(color, 0, output, 0);
    graph
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_rejects_cycles_and_bad_ports() {
        let mut graph = Graph::default();
        let a = graph.add_node(
            NodeKind::Color {
                brightness: 0.0,
                contrast: 1.0,
                saturation: 1.0,
            },
            [0.0, 0.0],
        );
        let b = graph.add_node(
            NodeKind::Transform {
                width: 640,
                height: 360,
                x: 0,
                y: 0,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(
            NodeKind::Output {
                width: 640,
                height: 360,
            },
            [0.0, 0.0],
        );

        assert!(graph.connect(a, 0, b, 0).is_ok());
        assert!(graph.connect(b, 0, a, 0).is_err(), "cycle must be rejected");
        assert!(
            graph.connect(a, 0, a, 0).is_err(),
            "self link must be rejected"
        );
        assert!(graph.connect(a, 0, out, 0).is_ok());
        assert!(
            graph.connect(out, 0, a, 0).is_err(),
            "Output has no output port"
        );
    }
}
