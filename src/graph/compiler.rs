//! Deterministic graph -> FFmpeg `-filter_complex` compiler.
//!
//! Every stream is normalised to RGBA while it travels through the graph:
//! the preview and export tails then only have to deal with one pixel format.
//! Fan-out is handled with `split` because an FFmpeg label can only be
//! consumed once.

use std::collections::{BTreeSet, HashMap, HashSet};

use super::node::{compatible, Graph, KeyMode, NodeId, NodeKind};
use super::nodes::{image, solid_color, text};
use crate::preview::engine::{Accel, PREVIEW_H, PREVIEW_W};

/// One FFmpeg input. Image sources need demuxer options and must not receive
/// hardware decode flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputSpec {
    pub path: String,
    /// Options placed before `-i`.
    pub extra: Vec<String>,
    pub decode: bool,
}

impl InputSpec {
    fn file(path: String) -> Self {
        Self {
            path,
            extra: Vec::new(),
            decode: true,
        }
    }
}

/// GPU-accelerated filters are intentionally not used: forcing
/// `-hwaccel_output_format cuda/vaapi` would keep frames on the GPU and break
/// `eq`, `pad` and `overlay` below. With plain `-hwaccel`, FFmpeg decodes on
/// the device and downloads frames to system memory, which is a safe win for
/// the MVP. `shortcut: decode-only GPU acceleration; add scale_cuda et al.
/// when filter-side acceleration matters.`
#[derive(Debug)]
pub struct Compiled {
    /// One FFmpeg input per reachable file/image source, in node order.
    pub inputs: Vec<InputSpec>,
    pub filter_complex: String,
    /// At least one reachable source is placed on the timeline. Preview then
    /// seeks on the *output* (after trim/setpts) instead of the input, so the
    /// extracted frame matches sequence time rather than source time.
    pub timeline_active: bool,
}

impl Compiled {
    /// Arguments for a single-frame preview. `t` is the seek timestamp.
    pub fn preview_args(&self, t: f32, accel: Accel) -> Vec<String> {
        // Timeline-trimmed sources are re-timed inside the filter graph, so an
        // input-side `-ss` would seek the asset instead of the sequence. Seek
        // the filtered output in that case.
        let input_seek = (!self.timeline_active).then_some(t);
        let mut args = input_args(&self.inputs, input_seek, accel);
        let filter_complex = format!(
            "{};[vout]scale={PREVIEW_W}:{PREVIEW_H}:force_original_aspect_ratio=decrease,\
             pad={PREVIEW_W}:{PREVIEW_H}:(ow-iw)/2:(oh-ih)/2:color=black,format=rgba[preview]",
            self.filter_complex
        );
        args.extend(
            [
                "-filter_complex",
                filter_complex.as_str(),
                "-map",
                "[preview]",
            ]
            .map(String::from),
        );
        if self.timeline_active {
            args.push("-ss".into());
            args.push(format!("{t:.3}"));
        }
        args.extend(
            [
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ]
            .map(String::from),
        );
        args
    }

    /// Arguments for a full render to `out`.
    pub fn export_args(&self, out: &str, accel: Accel) -> Vec<String> {
        let mut args = input_args(&self.inputs, None, accel);

        // VAAPI encoding needs an explicit upload; NVENC/VideoToolbox accept
        // system-memory frames directly.
        let (filter_complex, map_label, encoder) = match accel {
            Accel::Cpu => (
                self.filter_complex.clone(),
                "[vout]",
                vec!["-c:v", "libx264", "-preset", "veryfast", "-crf", "23"],
            ),
            Accel::Cuda => (
                self.filter_complex.clone(),
                "[vout]",
                vec!["-c:v", "h264_nvenc", "-preset", "p4", "-cq", "23"],
            ),
            Accel::VideoToolbox => (
                self.filter_complex.clone(),
                "[vout]",
                vec!["-c:v", "h264_videotoolbox", "-b:v", "6M"],
            ),
            Accel::Vaapi => (
                format!(
                    "{};[vout]format=nv12,hwupload[vout_hw]",
                    self.filter_complex
                ),
                "[vout_hw]",
                vec!["-c:v", "h264_vaapi", "-qp", "23"],
            ),
        };

        args.extend(
            [
                "-filter_complex",
                filter_complex.as_str(),
                "-map",
                map_label,
            ]
            .map(String::from),
        );
        args.extend(encoder.into_iter().map(String::from));
        if accel != Accel::Vaapi {
            // h264_vaapi expects the hwupload/nv12 surface, not yuv420p.
            args.extend(["-pix_fmt", "yuv420p"].map(String::from));
        }
        // ponytail: audio mixing for multiple sources; pass primary input audio for now.
        if !self.inputs.is_empty() {
            args.extend(["-map", "0:a?", "-c:a", "aac"].map(String::from));
        }
        args.extend(["-y", out].map(String::from));
        args
    }
}

fn input_args(inputs: &[InputSpec], seek: Option<f32>, accel: Accel) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin"]
        .map(String::from)
        .into();

    if accel == Accel::Vaapi {
        args.extend(["-vaapi_device", "/dev/dri/renderD128"].map(String::from));
    }

    for input in inputs {
        if let Some(t) = seek {
            args.push("-ss".into());
            args.push(format!("{t:.3}"));
        }
        args.extend(input.extra.iter().cloned());
        if input.decode {
            args.extend(accel.decode_flags().iter().map(|s| s.to_string()));
        }
        args.push("-i".to_string());
        args.push(input.path.clone());
    }
    args
}

/// Turn an argument vector into something that can be pasted into a shell.
pub fn command_line(args: &[String]) -> String {
    let mut line = String::from("ffmpeg");
    for arg in args {
        line.push(' ');
        line.push_str(&shell_quote(arg));
    }
    line
}

fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=,[]@".contains(c))
    {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// Compile the graph rooted at the first Output node. Unreachable nodes are
/// ignored, which keeps half-built experiments out of the FFmpeg command.
pub fn compile(graph: &Graph) -> Result<Compiled, String> {
    let output_id = graph
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Output { .. }))
        .map(|n| n.id)
        .ok_or("graph needs an Output node")?;

    let (canvas_w, canvas_h) = match graph.nodes.get(output_id).map(|node| &node.kind) {
        Some(NodeKind::Output { width, height }) => (even(*width), even(*height)),
        _ => return Err("Output node is malformed".into()),
    };

    // 1. Only compile what can actually reach the Output node.
    let mut reachable = HashSet::new();
    let mut stack = vec![output_id];
    while let Some(id) = stack.pop() {
        if reachable.insert(id) {
            for link in &graph.links {
                if link.to == id {
                    stack.push(link.from);
                }
            }
        }
    }

    // 2. Validate links before doing any label bookkeeping.
    for link in &graph.links {
        if !reachable.contains(&link.from) || !reachable.contains(&link.to) {
            continue;
        }
        let source = graph
            .node(link.from)
            .ok_or_else(|| format!("link references missing node {}", link.from))?;
        let target = graph
            .node(link.to)
            .ok_or_else(|| format!("link references missing node {}", link.to))?;
        let out =
            source.kind.outputs().get(link.from_port).ok_or_else(|| {
                format!("{} has no output {}", source.kind.title(), link.from_port)
            })?;
        let input = target
            .kind
            .inputs()
            .get(link.to_port)
            .ok_or_else(|| format!("{} has no input {}", target.kind.title(), link.to_port))?;
        if !compatible(out.kind, input.kind) {
            return Err(format!(
                "{} output is {:?} but {} input {} is {:?}",
                source.kind.title(),
                out.kind,
                target.kind.title(),
                link.to_port,
                input.kind
            ));
        }
    }

    // 3. Topological order. Kahn's algorithm with a sorted ready set makes the
    //    output deterministic regardless of HashMap iteration order.
    let mut indegree: HashMap<NodeId, usize> = reachable.iter().map(|&id| (id, 0)).collect();
    for link in &graph.links {
        if reachable.contains(&link.from) && reachable.contains(&link.to) {
            *indegree.entry(link.to).or_insert(0) += 1;
        }
    }
    let mut ready: BTreeSet<NodeId> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(&id, _)| id)
        .collect();
    let mut order = Vec::with_capacity(reachable.len());
    while let Some(id) = ready.iter().next().copied() {
        ready.remove(&id);
        order.push(id);
        for link in &graph.links {
            if link.from != id || !reachable.contains(&link.to) {
                continue;
            }
            if let Some(degree) = indegree.get_mut(&link.to) {
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(link.to);
                }
            }
        }
    }
    if order.len() != reachable.len() {
        return Err("graph contains a cycle".into());
    }

    // 4. FFmpeg inputs and the link -> label map. Solid Color and text-only
    //    sources have no `-i`; Image does, with a loop/duration.
    let mut inputs = Vec::new();
    let mut node_inputs: HashMap<NodeId, usize> = HashMap::new();
    for node in &graph.nodes {
        if !reachable.contains(&node.id) {
            continue;
        }
        match &node.kind {
            NodeKind::Input { path } => {
                if path.trim().is_empty() {
                    return Err(format!("Input node #{} has no file", node.id));
                }
                let idx = inputs.len();
                inputs.push(InputSpec::file(path.clone()));
                node_inputs.insert(node.id, idx);
            }
            NodeKind::Image {
                path,
                duration,
                framerate,
            } => {
                if path.trim().is_empty() {
                    return Err(format!("Image node #{} has no file", node.id));
                }
                let idx = inputs.len();
                inputs.push(InputSpec {
                    path: path.clone(),
                    extra: image::input_options(*duration, *framerate),
                    decode: false,
                });
                node_inputs.insert(node.id, idx);
            }
            _ => {}
        }
    }

    let mut incoming: HashMap<(NodeId, usize), usize> = HashMap::new();
    for (index, link) in graph.links.iter().enumerate() {
        if reachable.contains(&link.from) && reachable.contains(&link.to) {
            // Last link wins, matching the UI's single-source input ports.
            incoming.insert((link.to, link.to_port), index);
        }
    }

    let mut out_links: HashMap<NodeId, Vec<usize>> = HashMap::new();
    for (index, link) in graph.links.iter().enumerate() {
        if !reachable.contains(&link.from) || !reachable.contains(&link.to) {
            continue;
        }
        if incoming.get(&(link.to, link.to_port)) == Some(&index) {
            out_links.entry(link.from).or_default().push(index);
        }
    }

    let mut link_label: Vec<Option<String>> = vec![None; graph.links.len()];
    for (id, links) in &out_links {
        for (fanout_index, &link_index) in links.iter().enumerate() {
            let port = graph.links[link_index].from_port;
            link_label[link_index] = Some(format!("[n{id}p{port}_{fanout_index}]"));
        }
    }

    // 5. Emit one filter statement per node.
    let mut filter_complex = String::new();
    let mut saw_output = false;

    for &id in &order {
        let node = &graph.nodes[id];

        let mut labels_in = Vec::with_capacity(node.kind.inputs().len().max(1));
        if let Some(&input_idx) = node_inputs.get(&id) {
            labels_in.push(format!("[{input_idx}:v]"));
        } else {
            for port in 0..node.kind.inputs().len() {
                match incoming.get(&(id, port)) {
                    Some(link_index) => {
                        let label = link_label[*link_index]
                            .clone()
                            .ok_or_else(|| format!("internal: unlabelled link into node #{id}"))?;
                        labels_in.push(label);
                    }
                    // Optional ports (the Alpha Key mask) simply do not
                    // participate in the filter when unconnected.
                    None if node.kind.inputs()[port].optional => {}
                    None => {
                        return Err(format!(
                            "{} node #{} is missing its '{}' input",
                            node.kind.title(),
                            id,
                            node.kind.inputs()[port].name
                        ))
                    }
                }
            }
        }

        // The external matte wins over the luma key: one alphamerge, no
        // multiplication of two alpha masks.
        let has_mask =
            matches!(node.kind, NodeKind::AlphaKey { .. }) && incoming.contains_key(&(id, 1));
        // A Text node without a video input draws over a generated canvas.
        let has_video = !labels_in.is_empty();

        if matches!(node.kind, NodeKind::Output { .. }) {
            let filter = format!(
                "format=rgba,scale={canvas_w}:{canvas_h}:force_original_aspect_ratio=decrease,\
                 pad={canvas_w}:{canvas_h}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1"
            );
            emit(&mut filter_complex, &labels_in, &filter, &["[vout]".into()]);
            saw_output = true;
            continue;
        }

        // Every node currently has a single video output, so the outgoing
        // link labels are the node's fan-out labels.
        let output_labels: Vec<String> = out_links
            .get(&id)
            .map(|links| {
                links
                    .iter()
                    .filter_map(|&index| link_label[index].clone())
                    .collect()
            })
            .unwrap_or_default();

        let filter = match &node.kind {
            NodeKind::Input { .. } => {
                format!("format=rgba,setsar=1{}", timeline_gate(graph, id))
            }
            NodeKind::Transform {
                width,
                height,
                x,
                y,
            } => {
                let width = even(*width);
                let height = even(*height);
                format!(
                    "scale={width}:{height}:flags=bicubic,\
                     pad=max(iw\\,{canvas_w}):max(ih\\,{canvas_h}):{x}:{y}:color=black@0,setsar=1"
                )
            }
            NodeKind::Color {
                brightness,
                contrast,
                saturation,
            } => format!(
                "eq=brightness={:.3}:contrast={:.3}:saturation={:.3},format=rgba",
                brightness.clamp(-1.0, 1.0),
                contrast.clamp(0.0, 2.0),
                saturation.clamp(0.0, 3.0)
            ),
            NodeKind::Overlay { x, y } => {
                format!("overlay=x={x}:y={y}:eof_action=pass:format=auto,format=rgba,setsar=1")
            }
            NodeKind::ColorKey {
                color,
                similarity,
                blend,
            } => format!(
                "format=rgba,colorkey=color=0x{:02X}{:02X}{:02X}:similarity={:.3}:blend={:.3},format=rgba",
                color[0],
                color[1],
                color[2],
                similarity.clamp(0.01, 1.0),
                blend.clamp(0.0, 1.0)
            ),
            NodeKind::AlphaKey {
                mode,
                threshold,
                tolerance,
                softness,
            } => {
                if has_mask {
                    let vid = &labels_in[0];
                    let mask = &labels_in[1];
                    let s_mask = format!("[n{id}m]");
                    let s_vid = format!("[n{id}v]");
                    let invert = if *mode == KeyMode::InvertAlpha {
                        ",lutrgb=a='255-val'"
                    } else {
                        ""
                    };
                    emit(
                        &mut filter_complex,
                        &[mask.clone(), vid.clone()],
                        "scale2ref",
                        &[s_mask.clone(), s_vid.clone()],
                    );
                    labels_in = vec![s_vid, s_mask];
                    format!("alphamerge{invert},format=rgba")
                } else {
                    match mode {
                        KeyMode::LumaKey => format!(
                            "format=rgba,lumakey=threshold={:.3}:tolerance={:.3}:softness={:.3},format=rgba",
                            threshold.clamp(0.0, 1.0),
                            tolerance.clamp(0.0, 1.0),
                            softness.clamp(0.0, 1.0)
                        ),
                        KeyMode::ExtractAlpha => "format=rgba,alphaextract,format=rgba".into(),
                        KeyMode::InvertAlpha => "format=rgba,lutrgb=a='255-val'".into(),
                    }
                }
            }
            NodeKind::Image { .. } => {
                format!("{}{}", image::filter(), timeline_gate(graph, id))
            }
            NodeKind::SolidColor {
                color,
                width,
                height,
                duration,
                framerate,
            } => solid_color::filter(*color, *width, *height, *duration, *framerate),
            NodeKind::Text {
                text,
                font_size,
                font_color,
                font_file,
                position,
                custom_x,
                custom_y,
                duration,
            } => text::filter(&text::TextSpec {
                text,
                font_size: *font_size,
                font_color: *font_color,
                font_file,
                position: *position,
                custom_x,
                custom_y,
                width: canvas_w,
                height: canvas_h,
                duration: *duration,
                framerate: 30,
                has_video,
            }),
            NodeKind::Blur {
                sigma,
                sigma_v,
                steps,
            } => {
                let s = sigma.clamp(0.0, 1024.0);
                let sv = sigma_v.clamp(0.0, 1024.0);
                let st = (*steps).clamp(1, 6);
                format!("format=rgba,gblur=sigma={s:.2}:sigmaV={sv:.2}:steps={st},format=rgba")
            }
            NodeKind::Sharpen {
                amount,
                clarity,
                msize_x,
                msize_y,
            } => {
                let lx = crate::graph::node::odd_matrix(*msize_x);
                let ly = crate::graph::node::odd_matrix(*msize_y);
                let la = (amount + clarity).clamp(-2.0, 5.0);
                format!("format=rgba,unsharp=lx={lx}:ly={ly}:la={la:.2},format=rgba")
            }
            NodeKind::Lut3d { path } => {
                if path.trim().is_empty() {
                    // No LUT chosen yet: an identity pass, not a compile error.
                    "format=rgba".to_string()
                } else {
                    format!("format=rgba,lut3d=file={},format=rgba", escape_lut_path(path))
                }
            }
            NodeKind::ColorBalance {
                shadows,
                midtones,
                highlights,
            } => {
                let s = shadows.map(|value| value.clamp(-1.0, 1.0));
                let m = midtones.map(|value| value.clamp(-1.0, 1.0));
                let h = highlights.map(|value| value.clamp(-1.0, 1.0));
                format!(
                    "format=rgba,colorbalance=rs={:.3}:gs={:.3}:bs={:.3}:\
                     rm={:.3}:gm={:.3}:bm={:.3}:rh={:.3}:gh={:.3}:bh={:.3},format=rgba",
                    s[0], s[1], s[2], m[0], m[1], m[2], h[0], h[1], h[2]
                )
            }
            NodeKind::Levels {
                black,
                white,
                gamma,
            } => {
                // colorlevels rejects an inverted range, so clamp each pair
                // before handing it to FFmpeg.
                let lo = black.map(|value| value.clamp(0.0, 0.999));
                let hi = [
                    white[0].clamp(lo[0] + 0.001, 1.0),
                    white[1].clamp(lo[1] + 0.001, 1.0),
                    white[2].clamp(lo[2] + 0.001, 1.0),
                ];
                let g = gamma.map(|value| value.clamp(0.1, 10.0));
                format!(
                    "format=rgba,colorlevels=rimin={:.4}:gimin={:.4}:bimin={:.4}:\
                     rimax={:.4}:gimax={:.4}:bimax={:.4},\
                     eq=gamma_r={:.4}:gamma_g={:.4}:gamma_b={:.4},format=rgba",
                    lo[0], lo[1], lo[2], hi[0], hi[1], hi[2], g[0], g[1], g[2]
                )
            }
            NodeKind::InvertSolarize {
                invert,
                solarize,
                threshold,
            } => {
                let invert = invert.clamp(0.0, 1.0);
                let solarize = solarize.clamp(0.0, 1.0);
                let threshold = threshold.clamp(0.0, 1.0) * 255.0;
                let mut filters = String::from("format=rgba");
                if invert >= 0.999 {
                    filters.push_str(",negate");
                } else if invert > 0.0005 {
                    // Blend toward the negated value instead of a hard flip.
                    let expr = format!("clip(val+{invert:.4}*(255-2*val),0,255)");
                    filters.push_str(&format!(
                        ",lutrgb=r='{expr}':g='{expr}':b='{expr}'"
                    ));
                }
                if solarize > 0.0005 {
                    let expr =
                        format!("clip(val+{solarize:.4}*(255-2*val)*gte(val,{threshold:.2}),0,255)");
                    filters.push_str(&format!(
                        ",lutrgb=r='{expr}':g='{expr}':b='{expr}'"
                    ));
                }
                filters.push_str(",format=rgba");
                filters
            }
            NodeKind::Output { .. } => return Err("internal: Output reached the filter arm".into()),
        };

        // A filter only has one output. Fan-out therefore needs a `split` on
        // a private label, and each consumer gets its own copy.
        let links = out_links.get(&id).cloned().unwrap_or_default();
        match links.len() {
            0 => {
                return Err(format!(
                    "{} node #{} does not feed the output",
                    node.kind.title(),
                    id
                ))
            }
            1 => emit(&mut filter_complex, &labels_in, &filter, &output_labels),
            count => {
                let temp = format!("[n{id}tmp]");
                emit(
                    &mut filter_complex,
                    &labels_in,
                    &filter,
                    std::slice::from_ref(&temp),
                );
                emit(
                    &mut filter_complex,
                    &[temp],
                    &format!("split={count}"),
                    &output_labels,
                );
            }
        }
    }

    if !saw_output {
        return Err("could not reach an Output node".into());
    }

    let timeline_active = graph.nodes.iter().any(|node| {
        reachable.contains(&node.id)
            && matches!(node.kind, NodeKind::Input { .. } | NodeKind::Image { .. })
            && graph.timeline.clip_for_node(node.id).is_some()
    });

    Ok(Compiled {
        inputs,
        filter_complex: filter_complex.trim_end_matches(';').to_string(),
        timeline_active,
    })
}

/// `trim` + `setpts` suffix for a source node placed on the timeline. The
/// returned string is empty when the node has no clip, so the compiler output
/// stays byte-identical for graphs that never touched the timeline.
fn timeline_gate(graph: &Graph, node_id: NodeId) -> String {
    let timeline = &graph.timeline;
    let Some(clip) = timeline.clip_for_node(node_id) else {
        return String::new();
    };
    let fps = timeline.fps() as f64;
    let trim_in = clip.source_trim_in as f64 / fps;
    let source_duration = clip.source_frames() as f64 / fps;
    let start = clip.start_frame as f64 / fps;
    let speed = clip.speed.clamp(0.1, 10.0) as f64;
    format!(
        ",trim=start={trim_in:.6}:duration={source_duration:.6},\
         setpts=(PTS-STARTPTS)/{speed:.4}+{start:.6}/TB"
    )
}

/// Escape a LUT path for a filter option value. The filtergraph and the
/// option parser each consume one level of backslashes, so characters that
/// would split the option or graph need two (quotes three, backslashes four).
/// Spaces survive unquoted because the whole graph is one argv entry.
fn escape_lut_path(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len() + 8);
    for ch in path.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\\\\\"),
            '\'' => escaped.push_str("\\\\\\'"),
            ':' => escaped.push_str("\\\\:"),
            ',' | '[' | ']' | ';' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn emit(filter_complex: &mut String, inputs: &[String], filter: &str, outputs: &[String]) {
    for label in inputs {
        filter_complex.push_str(label);
    }
    filter_complex.push_str(filter);
    for label in outputs {
        filter_complex.push_str(label);
    }
    filter_complex.push(';');
}

fn even(value: u32) -> u32 {
    (value.max(2) & !1).min(7680)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::node::TextPos;

    fn link(graph: &mut Graph, from: NodeId, from_port: usize, to: NodeId, to_port: usize) {
        graph.links.push(super::super::node::Link {
            from,
            from_port,
            to,
            to_port,
        });
    }

    fn output(width: u32, height: u32) -> NodeKind {
        NodeKind::Output { width, height }
    }

    fn input() -> NodeKind {
        NodeKind::Input {
            path: "/tmp/a.mp4".into(),
        }
    }

    #[test]
    fn linear_graph_has_no_split() {
        let mut graph = Graph::default();
        let a = graph.add_node(input(), [0.0, 0.0]);
        let b = graph.add_node(
            NodeKind::Color {
                brightness: 0.0,
                contrast: 1.0,
                saturation: 1.0,
            },
            [0.0, 0.0],
        );
        let c = graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut graph, a, 0, b, 0);
        link(&mut graph, b, 0, c, 0);

        let compiled = compile(&graph).expect("compiles");
        assert!(compiled.filter_complex.contains("[vout]"));
        assert!(!compiled.filter_complex.contains("split"));
        assert_eq!(compiled.inputs.len(), 1);
        assert_eq!(compiled.inputs[0].path, "/tmp/a.mp4");
    }

    #[test]
    fn timeline_clip_trims_and_seeks_on_output() {
        use crate::timeline::Clip;

        let mut graph = Graph::default();
        let source = graph.add_node(input(), [0.0, 0.0]);
        let out = graph.add_node(output(640, 360), [0.0, 0.0]);
        link(&mut graph, source, 0, out, 0);
        graph.timeline.tracks[0].clips.push(Clip {
            clip_id: 1,
            node: source,
            start_frame: 30,
            duration_frames: 60,
            source_trim_in: 15,
            speed: 2.0,
        });

        let compiled = compile(&graph).expect("compiles");
        assert!(compiled.timeline_active);
        assert!(compiled
            .filter_complex
            .contains("trim=start=0.500000:duration=4.000000"));
        assert!(compiled
            .filter_complex
            .contains("setpts=(PTS-STARTPTS)/2.0000+1.000000/TB"));

        // A timeline-trimmed source must seek after the filter graph so `-ss`
        // addresses sequence time, not source time.
        let args = compiled.preview_args(1.5, Accel::Cpu);
        let filter_at = args.iter().position(|a| a == "-filter_complex").unwrap();
        let seek_at = args.iter().position(|a| a == "-ss").expect("output seek");
        assert!(seek_at > filter_at);
        assert_eq!(args[seek_at + 1], "1.500");
    }

    #[test]
    fn fanout_inserts_split() {
        let mut graph = Graph::default();
        let a = graph.add_node(input(), [0.0, 0.0]);
        let color = graph.add_node(
            NodeKind::Color {
                brightness: 0.0,
                contrast: 1.0,
                saturation: 1.0,
            },
            [0.0, 0.0],
        );
        let transform = graph.add_node(
            NodeKind::Transform {
                width: 640,
                height: 360,
                x: 0,
                y: 0,
            },
            [0.0, 0.0],
        );
        let overlay = graph.add_node(NodeKind::Overlay { x: 0, y: 0 }, [0.0, 0.0]);
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);

        link(&mut graph, a, 0, color, 0);
        link(&mut graph, a, 0, transform, 0);
        link(&mut graph, color, 0, overlay, 0);
        link(&mut graph, transform, 0, overlay, 1);
        link(&mut graph, overlay, 0, out, 0);

        let compiled = compile(&graph).expect("compiles");
        assert!(compiled.filter_complex.contains("split=2"));
        // Both split copies must be consumed exactly once.
        assert!(compiled.filter_complex.contains("[n0p0_0]"));
        assert!(compiled.filter_complex.contains("[n0p0_1]"));
    }

    #[test]
    fn cycle_is_rejected() {
        let mut graph = Graph::default();
        // Both nodes have inputs, otherwise port validation fires before the
        // cycle check. This is exactly the graph a hand-edited JSON can hold.
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
        let out = graph.add_node(output(640, 360), [0.0, 0.0]);
        link(&mut graph, a, 0, b, 0);
        link(&mut graph, b, 0, a, 0);
        link(&mut graph, b, 0, out, 0);

        let error = compile(&graph).expect_err("must reject cycle");
        assert!(error.contains("cycle"));
    }

    #[test]
    fn color_key_emits_colorkey_with_rgba() {
        let mut graph = Graph::default();
        let a = graph.add_node(input(), [0.0, 0.0]);
        let key = graph.add_node(
            NodeKind::ColorKey {
                color: [0, 255, 0],
                similarity: 0.3,
                blend: 0.1,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut graph, a, 0, key, 0);
        link(&mut graph, key, 0, out, 0);

        let compiled = compile(&graph).expect("compiles");
        assert!(compiled.filter_complex.contains(
            "format=rgba,colorkey=color=0x00FF00:similarity=0.300:blend=0.100,format=rgba"
        ));
    }

    #[test]
    fn alpha_key_without_mask_uses_lumakey() {
        let mut graph = Graph::default();
        let a = graph.add_node(input(), [0.0, 0.0]);
        let key = graph.add_node(
            NodeKind::AlphaKey {
                mode: KeyMode::LumaKey,
                threshold: 0.5,
                tolerance: 0.1,
                softness: 0.2,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut graph, a, 0, key, 0);
        link(&mut graph, key, 0, out, 0);

        // The unconnected optional mask port must not fail compilation.
        let compiled = compile(&graph).expect("compiles");
        assert!(compiled
            .filter_complex
            .contains("lumakey=threshold=0.500:tolerance=0.100:softness=0.200"));
        assert!(!compiled.filter_complex.contains("alphamerge"));
    }

    #[test]
    fn alpha_key_with_mask_uses_alphamerge() {
        let mut graph = Graph::default();
        let a = graph.add_node(input(), [0.0, 0.0]);
        let mask = graph.add_node(input(), [0.0, 0.0]);
        let key = graph.add_node(
            NodeKind::AlphaKey {
                mode: KeyMode::LumaKey,
                threshold: 0.5,
                tolerance: 0.1,
                softness: 0.2,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut graph, a, 0, key, 0);
        link(&mut graph, mask, 0, key, 1);
        link(&mut graph, key, 0, out, 0);

        let compiled = compile(&graph).expect("compiles");
        assert!(compiled.filter_complex.contains("alphamerge"));
        assert!(!compiled.filter_complex.contains("lumakey"));
        assert_eq!(compiled.inputs.len(), 2);
    }

    #[test]
    fn invert_alpha_uses_lutrgb() {
        let mut graph = Graph::default();
        let a = graph.add_node(input(), [0.0, 0.0]);
        let key = graph.add_node(
            NodeKind::AlphaKey {
                mode: KeyMode::InvertAlpha,
                threshold: 0.5,
                tolerance: 0.1,
                softness: 0.0,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut graph, a, 0, key, 0);
        link(&mut graph, key, 0, out, 0);

        let compiled = compile(&graph).expect("compiles");
        assert!(compiled.filter_complex.contains("lutrgb=a='255-val'"));
    }

    #[test]
    fn text_node_escapes_text_and_can_be_a_source() {
        let mut graph = Graph::default();
        let title = graph.add_node(
            NodeKind::Text {
                text: "it's: 50%".into(),
                font_size: 48.0,
                font_color: [255, 255, 255],
                font_file: "/tmp/My Font.ttf".into(),
                position: TextPos::CustomXY,
                custom_x: "40".into(),
                custom_y: "60".into(),
                duration: 4.0,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut graph, title, 0, out, 0);

        let compiled = compile(&graph).expect("text-only graph compiles");
        assert!(compiled.inputs.is_empty());
        assert!(compiled
            .filter_complex
            .contains("color=c=black@0.0:s=1280x720:d=4.000:r=30,drawtext="));
        assert!(compiled.filter_complex.contains("text='it''s\\: 50\\%'"));
        assert!(compiled
            .filter_complex
            .contains("fontfile='/tmp/My Font.ttf'"));
        assert!(compiled.filter_complex.contains(":x=40:y=60"));
    }

    #[test]
    fn text_node_over_video_keeps_the_input_label() {
        let mut graph = Graph::default();
        let a = graph.add_node(input(), [0.0, 0.0]);
        let title = graph.add_node(
            NodeKind::Text {
                text: "hi".into(),
                font_size: 36.0,
                font_color: [255, 255, 255],
                font_file: String::new(),
                position: TextPos::Center,
                custom_x: String::new(),
                custom_y: String::new(),
                duration: 5.0,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut graph, a, 0, title, 0);
        link(&mut graph, title, 0, out, 0);

        let compiled = compile(&graph).expect("compiles");
        assert!(compiled
            .filter_complex
            .contains("[n0p0_0]drawtext=text='hi':"));
        assert!(!compiled.filter_complex.contains("color=c=black@0.0"));
        assert!(compiled
            .filter_complex
            .contains("x=(w-text_w)/2:y=(h-text_h)/2"));
    }

    #[test]
    fn image_node_becomes_a_looped_input() {
        let mut graph = Graph::default();
        let image = graph.add_node(
            NodeKind::Image {
                path: "/tmp/bg.png".into(),
                duration: 8.0,
                framerate: 25,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut graph, image, 0, out, 0);

        let compiled = compile(&graph).expect("compiles");
        assert_eq!(compiled.inputs.len(), 1);
        assert_eq!(
            compiled.inputs[0].extra,
            ["-loop", "1", "-framerate", "25", "-t", "8.000"]
                .map(String::from)
                .to_vec()
        );
        assert!(compiled.filter_complex.contains("format=rgba,setsar=1"));

        let args = compiled.preview_args(2.5, Accel::Cuda);
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-ss" && pair[1] == "2.500"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-t" && pair[1] == "8.000"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-i" && pair[1] == "/tmp/bg.png"));
        // Hardware decode flags belong to real videos, not still images.
        assert!(!args.iter().any(|arg| arg == "cuda"));
    }

    #[test]
    fn solid_color_node_needs_no_input_file() {
        let mut graph = Graph::default();
        let solid = graph.add_node(
            NodeKind::SolidColor {
                color: [0x33, 0x66, 0xCC],
                width: 1280,
                height: 720,
                duration: 3.0,
                framerate: 30,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut graph, solid, 0, out, 0);

        let compiled = compile(&graph).expect("compiles");
        assert!(compiled.inputs.is_empty());
        assert!(compiled
            .filter_complex
            .starts_with("color=c=0x3366CC:s=1280x720:d=3.000:r=30,format=rgba,setsar=1"));
    }

    #[test]
    fn missing_input_file_is_an_error() {
        let mut graph = Graph::default();
        let a = graph.add_node(NodeKind::Input { path: "  ".into() }, [0.0, 0.0]);
        let out = graph.add_node(output(640, 360), [0.0, 0.0]);
        link(&mut graph, a, 0, out, 0);

        let error = compile(&graph).expect_err("must reject empty path");
        assert!(error.contains("no file"));
    }

    #[test]
    fn full_layer_stack_compiles_and_renders_with_real_ffmpeg() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        if !std::path::Path::new("/tmp/opencode/img.png").exists() {
            return;
        }

        let mut graph = Graph::default();
        let bg = graph.add_node(
            NodeKind::SolidColor {
                color: [20, 30, 40],
                width: 1280,
                height: 720,
                duration: 3.0,
                framerate: 30,
            },
            [0.0, 0.0],
        );
        let img = graph.add_node(
            NodeKind::Image {
                path: "/tmp/opencode/img.png".into(),
                duration: 3.0,
                framerate: 30,
            },
            [0.0, 0.0],
        );
        let overlay = graph.add_node(NodeKind::Overlay { x: 50, y: 50 }, [0.0, 0.0]);
        let blur = graph.add_node(
            NodeKind::Blur {
                sigma: 2.0,
                sigma_v: 2.0,
                steps: 1,
            },
            [0.0, 0.0],
        );
        let sharpen = graph.add_node(
            NodeKind::Sharpen {
                amount: 1.0,
                clarity: 0.2,
                msize_x: 5,
                msize_y: 5,
            },
            [0.0, 0.0],
        );
        let text = graph.add_node(
            NodeKind::Text {
                text: "Layer Test: 100%".into(),
                font_size: 40.0,
                font_color: [255, 255, 0],
                font_file: String::new(),
                position: TextPos::Center,
                custom_x: String::new(),
                custom_y: String::new(),
                duration: 3.0,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);

        link(&mut graph, bg, 0, overlay, 0);
        link(&mut graph, img, 0, overlay, 1);
        link(&mut graph, overlay, 0, blur, 0);
        link(&mut graph, blur, 0, sharpen, 0);
        link(&mut graph, sharpen, 0, text, 0);
        link(&mut graph, text, 0, out, 0);

        let compiled = compile(&graph).expect("multi-layer graph compiles");
        let args = compiled.preview_args(0.5, Accel::Cpu);

        let proc = std::process::Command::new("ffmpeg")
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .expect("ffmpeg executes");

        assert!(
            proc.status.success(),
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&proc.stderr)
        );
        assert_eq!(proc.stdout.len(), 640 * 360 * 4);
    }

    #[test]
    fn explicit_stream_labels_and_pipeline_fixes() {
        let mut graph = Graph::default();
        let a = graph.add_node(input(), [0.0, 0.0]);
        let b = graph.add_node(
            NodeKind::Image {
                path: "/tmp/b.png".into(),
                duration: 5.0,
                framerate: 30,
            },
            [0.0, 0.0],
        );
        let overlay = graph.add_node(NodeKind::Overlay { x: 10, y: 20 }, [0.0, 0.0]);
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut graph, a, 0, overlay, 0);
        link(&mut graph, b, 0, overlay, 1);
        link(&mut graph, overlay, 0, out, 0);

        let compiled = compile(&graph).expect("compiles");
        assert!(compiled
            .filter_complex
            .contains("[0:v]format=rgba,setsar=1"));
        assert!(compiled
            .filter_complex
            .contains("[1:v]format=rgba,setsar=1"));
        assert!(compiled
            .filter_complex
            .contains("overlay=x=10:y=20:eof_action=pass"));

        let export_args = compiled.export_args("/tmp/out.mp4", Accel::Cpu);
        assert!(export_args
            .windows(2)
            .any(|w| w[0] == "-map" && w[1] == "0:a?"));
        assert!(export_args
            .windows(2)
            .any(|w| w[0] == "-c:a" && w[1] == "aac"));
    }

    #[test]
    fn extract_alpha_and_scaled_mask_alphamerge() {
        let mut graph = Graph::default();
        let a = graph.add_node(input(), [0.0, 0.0]);
        let key = graph.add_node(
            NodeKind::AlphaKey {
                mode: KeyMode::ExtractAlpha,
                threshold: 0.5,
                tolerance: 0.1,
                softness: 0.2,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut graph, a, 0, key, 0);
        link(&mut graph, key, 0, out, 0);

        let compiled = compile(&graph).expect("compiles");
        assert!(compiled.filter_complex.contains("alphaextract"));

        let mut mask_graph = Graph::default();
        let vid = mask_graph.add_node(input(), [0.0, 0.0]);
        let mask = mask_graph.add_node(input(), [0.0, 0.0]);
        let akey = mask_graph.add_node(
            NodeKind::AlphaKey {
                mode: KeyMode::LumaKey,
                threshold: 0.5,
                tolerance: 0.1,
                softness: 0.2,
            },
            [0.0, 0.0],
        );
        let out2 = mask_graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut mask_graph, vid, 0, akey, 0);
        link(&mut mask_graph, mask, 0, akey, 1);
        link(&mut mask_graph, akey, 0, out2, 0);

        let compiled_mask = compile(&mask_graph).expect("compiles");
        assert!(compiled_mask.filter_complex.contains("scale2ref"));
        assert!(compiled_mask.filter_complex.contains("alphamerge"));
    }

    #[test]
    fn blur_and_sharpen_nodes_compile_with_filters() {
        let mut graph = Graph::default();
        let a = graph.add_node(input(), [0.0, 0.0]);
        let blur = graph.add_node(
            NodeKind::Blur {
                sigma: 4.5,
                sigma_v: 6.0,
                steps: 2,
            },
            [0.0, 0.0],
        );
        let sharpen = graph.add_node(
            NodeKind::Sharpen {
                amount: 1.2,
                clarity: 0.3,
                msize_x: 6, // Even number: must be forced to odd (7)
                msize_y: 4, // Even number: must be forced to odd (5)
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);

        link(&mut graph, a, 0, blur, 0);
        link(&mut graph, blur, 0, sharpen, 0);
        link(&mut graph, sharpen, 0, out, 0);

        let compiled = compile(&graph).expect("graph with blur and sharpen compiles");
        assert!(compiled
            .filter_complex
            .contains("gblur=sigma=4.50:sigmaV=6.00:steps=2"));
        assert!(compiled
            .filter_complex
            .contains("unsharp=lx=7:ly=5:la=1.50"));
    }

    #[test]
    fn color_effect_nodes_compile_with_filters() {
        let mut graph = Graph::default();
        let a = graph.add_node(input(), [0.0, 0.0]);
        let balance = graph.add_node(
            NodeKind::ColorBalance {
                shadows: [-0.2, 0.1, 0.25],
                midtones: [0.0, 0.0, 0.0],
                highlights: [0.3, -0.1, 0.0],
            },
            [0.0, 0.0],
        );
        let levels = graph.add_node(
            NodeKind::Levels {
                black: [0.05, 0.0, 0.1],
                white: [0.95, 1.0, 0.8],
                gamma: [1.2, 1.0, 0.8],
            },
            [0.0, 0.0],
        );
        let fx = graph.add_node(
            NodeKind::InvertSolarize {
                invert: 0.5,
                solarize: 0.5,
                threshold: 0.5,
            },
            [0.0, 0.0],
        );
        let lut = graph.add_node(
            NodeKind::Lut3d {
                path: "/tmp/luts/teal's:look.cube".into(),
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(1280, 720), [0.0, 0.0]);
        link(&mut graph, a, 0, balance, 0);
        link(&mut graph, balance, 0, levels, 0);
        link(&mut graph, levels, 0, fx, 0);
        link(&mut graph, fx, 0, lut, 0);
        link(&mut graph, lut, 0, out, 0);

        let compiled = compile(&graph).expect("colour nodes compile");
        let chain = &compiled.filter_complex;
        assert!(chain.contains(
            "colorbalance=rs=-0.200:gs=0.100:bs=0.250:rm=0.000:gm=0.000:bm=0.000:\
             rh=0.300:gh=-0.100:bh=0.000"
        ));
        assert!(chain.contains(
            "colorlevels=rimin=0.0500:gimin=0.0000:bimin=0.1000:\
             rimax=0.9500:gimax=1.0000:bimax=0.8000"
        ));
        assert!(chain.contains("eq=gamma_r=1.2000:gamma_g=1.0000:gamma_b=0.8000"));
        assert!(chain.contains("lutrgb=r='clip(val+0.5000*(255-2*val),0,255)'"));
        assert!(chain.contains("gte(val,127.50)"));
        assert!(chain.contains("lut3d=file=/tmp/luts/teal"));
    }

    #[test]
    fn lut_path_escapes_filtergraph_separators() {
        assert_eq!(escape_lut_path("/plain path/x.cube"), "/plain path/x.cube");
        assert_eq!(escape_lut_path("a:b"), "a\\\\:b");
        assert_eq!(escape_lut_path("it's"), "it\\\\\\'s");
        assert_eq!(escape_lut_path("a\\b"), "a\\\\\\\\b");
        assert_eq!(escape_lut_path("a,b[1];c"), "a\\,b\\[1\\]\\;c");
    }

    #[test]
    fn neutral_color_nodes_are_identity() {
        let mut graph = Graph::default();
        let a = graph.add_node(input(), [0.0, 0.0]);
        let lut = graph.add_node(NodeKind::Lut3d { path: "  ".into() }, [0.0, 0.0]);
        let fx = graph.add_node(
            NodeKind::InvertSolarize {
                invert: 0.0,
                solarize: 0.0,
                threshold: 0.5,
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(640, 360), [0.0, 0.0]);
        link(&mut graph, a, 0, lut, 0);
        link(&mut graph, lut, 0, fx, 0);
        link(&mut graph, fx, 0, out, 0);

        let compiled = compile(&graph).expect("identity colour nodes compile");
        assert!(!compiled.filter_complex.contains("lut3d"));
        assert!(!compiled.filter_complex.contains("lutrgb"));
        assert!(!compiled.filter_complex.contains("negate"));
    }

    #[test]
    fn color_nodes_render_with_real_ffmpeg() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }

        let lut_path = std::env::temp_dir().join("ffnodecomp it's:lut.cube");
        let cube = "TITLE \"identity\"\nLUT_3D_SIZE 2\n\
                    0.0 0.0 0.0\n1.0 0.0 0.0\n0.0 1.0 0.0\n1.0 1.0 0.0\n\
                    0.0 0.0 1.0\n1.0 0.0 1.0\n0.0 1.0 1.0\n1.0 1.0 1.0\n";
        if std::fs::write(&lut_path, cube).is_err() {
            return;
        }

        let mut graph = Graph::default();
        let bg = graph.add_node(
            NodeKind::SolidColor {
                color: [40, 80, 120],
                width: 640,
                height: 360,
                duration: 1.0,
                framerate: 30,
            },
            [0.0, 0.0],
        );
        let balance = graph.add_node(
            NodeKind::ColorBalance {
                shadows: [0.1, -0.1, 0.0],
                midtones: [0.0, 0.1, -0.1],
                highlights: [0.05, 0.0, 0.1],
            },
            [0.0, 0.0],
        );
        let levels = graph.add_node(
            NodeKind::Levels {
                black: [0.02, 0.0, 0.05],
                white: [0.95, 1.0, 0.9],
                gamma: [1.1, 0.9, 1.0],
            },
            [0.0, 0.0],
        );
        let fx = graph.add_node(
            NodeKind::InvertSolarize {
                invert: 0.25,
                solarize: 0.25,
                threshold: 0.6,
            },
            [0.0, 0.0],
        );
        let lut = graph.add_node(
            NodeKind::Lut3d {
                path: lut_path.to_string_lossy().into_owned(),
            },
            [0.0, 0.0],
        );
        let out = graph.add_node(output(640, 360), [0.0, 0.0]);
        link(&mut graph, bg, 0, balance, 0);
        link(&mut graph, balance, 0, levels, 0);
        link(&mut graph, levels, 0, fx, 0);
        link(&mut graph, fx, 0, lut, 0);
        link(&mut graph, lut, 0, out, 0);

        let compiled = compile(&graph).expect("colour chain compiles");
        let args = compiled.preview_args(0.2, Accel::Cpu);
        let proc = std::process::Command::new("ffmpeg")
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .expect("ffmpeg executes");
        let _ = std::fs::remove_file(&lut_path);

        assert!(
            proc.status.success(),
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&proc.stderr)
        );
        assert_eq!(proc.stdout.len(), 640 * 360 * 4);
    }
}
