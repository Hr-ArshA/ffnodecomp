# FFNodeComp

Minimal node-based video compositor: an egui graph editor that compiles to an
FFmpeg `-filter_complex` command, renders a single-frame preview asynchronously,
and exports an H.264 MP4.

## Run

```sh
cargo run --release
```

Requires `ffmpeg` on `PATH`. On first launch a small demo graph is created.
Paste an absolute video path into the Input node, then use the timestamp slider.

### Nodes

- **Input**: external video stream (`-i <path>`).
- **Transform**: scale to `w`x`h` and pad onto the Output canvas at `x,y` with transparent black.
- **Color Correction**: brightness, contrast, and saturation via `eq`.
- **Overlay**: composite foreground over background at an offset with `overlay=format=auto`.
- **Chroma Key** (`colorkey`): pick a key color (green/blue presets), then tune
  `similarity` and `blend` right on the card or in the inspector.
- **Alpha Key**: choose `LumaKey` (`lumakey`), `ExtractAlpha`, or `InvertAlpha`
  (`lutrgb=a='255-val'`). Connecting its optional **mask** input replaces the
  luma key with `alphamerge`, so an external matte wins.
- **Text** (`drawtext`): title/subtitle overlay with font size, color, font file, and position
  presets (`Center`, `Top center`, `Bottom center`, `Custom X/Y` expressions). If `video` is
  unconnected, it generates a transparent canvas (`color=c=black@0.0`) bounded by `duration`.
- **Image**: static image/still background treated as an external input stream
  (`-loop 1 -framerate <FPS> -t <DURATION> -i <PATH>`).
- **Solid Color**: generated background canvas via the FFmpeg `color` lavfi source filter
  (`color=c=<HEX>:s=<W>x<H>:d=<D>:r=<FPS>`).
- **Output**: sets canvas dimensions, letterboxes the incoming stream, and drives export.

### Graph & Connection Lifecycle

- **Connect**: click an **output** port, then an **input** port, or drag from an output to an input.
- **Single-input rule**: input ports accept only one incoming connection; connecting a new wire
  automatically replaces the existing connection.
- **Rewire / Detach**: drag a wire away from a connected input port. Drop on another valid input
  to reconnect, or drop into empty space to leave it disconnected.
- **Delete edge**: right-click any wire $\rightarrow$ **"Delete Connection"**, or click to select and press
  `Delete` / `Backspace`.
- **Delete node**: select a node header or body and press `Delete` / `Backspace`, or use the inspector button.
- **Port feedback**: wire colors indicate data type (Orange = Video, Green = Audio, Magenta = Mask).
  While connecting or dragging a wire, compatible target ports highlight green; incompatible or
  cycle-inducing ports tint red.
- **Cycle prevention**: connections that would create a directed loop are rejected by cycle detection.
- **Dynamic re-plan & cancellation**: every graph mutation increments `graph_version`, instantly
  aborts running FFmpeg preview processes, and debounces recompilation of the reachable DAG.

### Controls & Export

- **Save graph** / **Load graph**: saves to / loads from `graph.json` in the working directory.
- **File Chooser**: native file selection dialogs (`rfd`) for video (`Input`), image (`Image`), and font (`Text`) paths on both the canvas cards and inspector panel.
- **Configurable Export Destination**: choose any output path or use the file save dialog in the top bar.
- **Live Progress Bar**: tracks and displays FFmpeg encoding progress in real time via `-progress pipe:1`.
- **GPU Acceleration**: CUDA, VAAPI, VideoToolbox, or CPU software decode/encode.
  Failed GPU calls fall back to CPU automatically and display diagnostics in the viewer panel.

## Layout

```
src/graph/node.rs        data model, typed ports (Video, Audio, Mask), compatibility, DAG cycle guard
src/graph/mutations.rs   edge/port disconnection, replacement, cycle guards, EdgeId/PortId
src/graph/compiler.rs    reachability, topological sort, fan-out split, filter_complex generation
src/graph/nodes/         per-node filter generators (text.rs, image.rs, solid_color.rs)
src/preview/engine.rs    tokio FFmpeg worker, instant cancellation, GPU fallback, raw RGBA extraction
src/ui/canvas.rs         hand-rolled node canvas (wire hit tests, drag-away disconnect, port tinting)
src/ui/viewer.rs         preview texture, timestamp, command inspector
src/ui/mod.rs            app state, graph_version monotonic counter, inspector, export thread
```

## Composition model

- Every intermediate stream is RGBA. File inputs are decoded as-is.
- Hardware decode flags (`-hwaccel`) apply only to video files; still image inputs are decoded in software.
- Text escaping: emitted unquoted with individual backslash escapes for special characters and `expansion=none`.
- Preview appends a 640x360 letterboxed scale, reads raw RGBA from stdout, and drops frames whose version is superseded.

## Deliberate MVP limitations

- GPU acceleration is decode/encode only. Filter graph execution runs on the CPU
  because `-hwaccel_output_format` would break `eq`, `pad`, and `overlay`.
- Audio ports exist in the type system but no MVP node produces audio.
- One Output node drives the render. Extra Output nodes are ignored.
