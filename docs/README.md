# FFNodeComp Documentation

FFNodeComp is a minimal, node-based video compositor. You wire video sources and
filters together on a canvas, the graph is compiled into a single FFmpeg
`-filter_complex` filtergraph, a still frame is rendered asynchronously for
preview, and the result can be exported to an H.264 MP4.

- **UI:** Rust + [egui/eframe](https://github.com/emilk/egui) 0.37, custom-drawn node canvas and timeline.
- **Engine:** the system `ffmpeg` / `ffprobe` binaries, driven through `std::process` and Tokio.
- **Dependency-lean:** no FFmpeg bindings, no graph-widget crate, no database.
- **Storage:** plain JSON (`graph.json`) in the current working directory.
- **License:** MIT.

## Contents

| Document | What it covers |
| --- | --- |
| [User guide](user-guide.md) | Window layout, canvas editing, inspector, file pickers, preview, save/load, export, shortcuts. |
| [Nodes](nodes.md) | Every node kind: ports, fields, ranges, defaults, and exact FFmpeg mapping. |
| [Timeline](timeline.md) | Frame timebase, clips, trim/move/snap, transport, loop, and how clips become FFmpeg filters. |
| [Compilation](compilation.md) | Graph validation, reachability, topological order, labels, fan-out, RGBA pipeline, preview/export argument assembly, escaping. |
| [Architecture](architecture.md) | Module layout, state, threading, preview/export data flow, debounce and generation model, persistence. |
| [Graph format](graph-format.md) | `graph.json` schema, defaults, compatibility rules, and safe hand-editing. |
| [Development](development.md) | Build/test/lint commands, dependencies, how to add a node, release workflow, license. |
| [Troubleshooting](troubleshooting.md) | Common errors, GPU fallback, timeline gotchas, performance advice, known limitations. |

## What it does today

- Sources: video file, still image, solid colour, text.
- Compositing: transform/scale/pad, overlay, chroma key, alpha/luma key with an
  optional external matte.
- Filters: brightness/contrast/saturation, Gaussian blur, unsharp sharpening,
  cinematic 3D LUT, lift/gamma/gain colour balance, levels + per-channel gamma,
  invert and solarize.
- Single-frame, debounced preview at a fixed 640×360 RGBA, with automatic CPU
  fallback when a GPU path fails.
- H.264 MP4 export with a live progress bar.
- A frame-accurate, non-linear timeline that trims and re-times source nodes.

## Quick start

### Requirements

- A recent stable Rust toolchain (edition 2021).
- `ffmpeg` on `PATH` (decoding, filtering, encoding).
- `ffprobe` on `PATH` for accurate export progress and duration probing.
  FFmpeg is used as a fallback if `ffprobe` is missing.
- A desktop session. Linux builds need the usual winit/X11 or Wayland
  development libraries and a file-dialog portal for native pickers.

### Run

```sh
cargo run --release
```

On first launch the app loads `graph.json` from the current working directory if
it exists, otherwise it creates a small demo graph: `Input → Transform → Color →
Output`.

1. Select the **Input** node and use **Browse...** (or type an absolute path).
2. Click the **Output** node and set the canvas size.
3. Drag the **time (s)** slider in the Preview panel, or scrub the timeline ruler.
4. Set the export path in the top bar and press **Export MP4**.

The preview is one frame, not real-time playback. See
[Troubleshooting](troubleshooting.md) for errors and known limitations.

## Design in one paragraph

Nodes are a serialisable data model with typed ports; links are validated for
type compatibility and cycles when made. Only nodes that can reach the first
`Output` node are compiled. The compiler emits one filter statement per node in
a deterministic topological order, normalises every intermediate stream to
RGBA, inserts `split` where an output fans out, and produces two argument
vectors: one for a single-frame preview and one for a full export. A Tokio worker
runs FFmpeg for previews, tagged with a monotonically increasing graph generation
so stale frames are discarded; export runs on a plain thread and reports
progress through `-progress pipe:1`. The timeline is part of the graph data and
turns clip windows into `trim` + `setpts` suffixes on source nodes.
