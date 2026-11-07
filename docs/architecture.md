# Architecture

This page describes how the code is organised, what owns what, and how a frame
travels from a widget edit to an FFmpeg process and back.

The project is deliberately small (~6.2k lines of Rust) and dependency-lean:
eframe/egui for the UI, `rfd` for native file dialogs, serde for persistence,
Tokio for cancellable preview work. FFmpeg is invoked as an external process.

## Module map

```text
src/
├── main.rs                 eframe entry point, window options, module declarations
├── graph/
│   ├── mod.rs              compiler, mutations, node, nodes
│   ├── node.rs             NodeKind, Node, Link, Graph, ports, defaults, demo graph
│   ├── mutations.rs        typed PortId/EdgeId, connect/disconnect, cycle checks, GraphError
│   ├── compiler.rs         Graph → filter_complex + preview/export argv, escaping, tests
│   └── nodes/
│       ├── mod.rs
│       ├── image.rs        still-image demuxer options and RGBA normalisation
│       ├── solid_color.rs  lavfi `color` source
│       └── text.rs         drawtext source/filter, position sanitising, text escaping
├── preview/
│   ├── mod.rs
│   └── engine.rs           Accel, Tokio preview worker, run_export, probe_duration
├── timeline/
│   └── mod.rs              FrameIndex, Timecode, Clip, Track, TimelineState, drag/snap/loop
└── ui/
    ├── mod.rs              App state, panels, inspector, save/load, export, debounce
    ├── canvas.rs           hand-rolled node canvas, ports, wires, inline controls
    ├── timeline_view.rs    docked timeline painting, clip/ruler interaction, transport
    └── viewer.rs           preview image, timestamp slider, command inspector
```

Rules of thumb visible in the code:

- `graph` contains **no egui types**. It compiles, serialises and unit-tests
  headlessly.
- `timeline` depends only on `graph::node::NodeId` and serde. It can be tested
  without a window.
- Per-node FFmpeg mapping for *source/layer* nodes lives in `graph/nodes/*` so the
  compiler does not become a 2000-line match. Simple filter nodes stay in
  `compiler.rs` because that is where their ordering relative to `format=rgba`
  matters.
- `ui` is the only place that knows about `egui`, `eframe`, `rfd` and panel
  layout. It calls `compiler::compile` and `PreviewEngine`, never FFmpeg
  directly (except through `preview::engine` functions).
- `preview` owns every process concern: spawn, pipes, cancellation, encoders,
  progress parsing, duration probing.

## Core data

```rust
Graph {
    nodes: Vec<Node>,       // id mirrors the vector index
    links: Vec<Link>,
    timeline: TimelineState, // #[serde(default)]
}

Node {
    id: usize,              // == position in nodes
    kind: NodeKind,         // tagged enum, one variant per node type
    pos: [f32; 2],          // canvas-space top-left
}

Link {
    from: usize, from_port: usize,
    to: usize,   to_port: usize,
}
```

- **Node ids are indexes.** `add_node` pushes and returns `nodes.len()-1`.
- `remove_node(id)` removes the vector entry, drops attached links, decrements
  every link endpoint greater than `id`, renumbers all node ids, and calls
  `timeline.on_node_removed(id)` (which drops/decrements clip references).
- Because of renumbering, a `NodeId` is valid only until the next structural
  mutation. UI code re-reads ids each frame; it never caches one across a
  deletion.
- `Link` order in `Graph::links` is significant to the compiler's "last link
  wins" rule for duplicate input connections (the UI prevents duplicates).
- Ports are described by static `PortSpec` slices per `NodeKind`
  (`inputs()`/`outputs()`), so validity and wire colouring are data, not
  scattered `match` arms. `compatible(from, to)` allows equality plus
  `Video → Mask`.
- The graph is self-contained and serde-serialisable; see
  [Graph format](graph-format.md).

## App state

```rust
pub struct App {
    graph: Graph,                  // nodes, links, timeline
    accel: Accel,                  // CPU | Cuda | VideoToolbox | Vaapi
    timestamp: f32,                // preview seek, seconds

    engine: Option<PreviewEngine>, // Tokio worker; None if runtime failed
    texture: Option<TextureHandle>,// last successfully loaded preview frame
    preview_error: Option<String>, // FFmpeg stderr tail
    preview_note: Option<String>,  // GPU→CPU fallback note
    graph_version: u64,            // monotonic; also preview request generation
    preview_pending: bool,         // a request is outstanding

    dirty_at: Option<Instant>,     // debounce anchor; None = compiled & idle
    compiled: Option<Compiled>,    // last successful compile
    compile_error: Option<String>, // last compile failure

    selected: Option<NodeId>,
    selected_edge: Option<usize>,
    pending_link: Option<(NodeId, usize)>,
    wire_drag: bool,
    dragging: Option<(NodeId, Vec2)>,

    status: Option<(String, bool)>,
    export_path: String,
    export_progress: f32,
    export_rx: Option<Receiver<ExportMsg>>,
    debounce: Duration,            // 80 ms edits / 33 ms scrub
    timeline_drag: Option<TimelineDrag>,
    playback_anchor: Option<(Instant, u64)>,
}
```

Important invariants:

- `graph_version` is the single source of truth for "which graph/timestamp is
  this result for". It increments on every edit and scrub seek. Preview messages
  are compared against it; mismatched frames are dropped.
- `dirty_at: Option<Instant>` means "a compile/preview refresh is scheduled".
  `None` means the last compile is current. `preview_pending` separately means a
  worker is expected to answer.
- `compiled` and `compile_error` are mutually exclusive in practice.
- `texture` survives compile failures of the *current* graph only if the compiler
  fails before a new frame is requested; `refresh_preview` clears it on a
  compile error.
- `export_rx` is `Some` exactly while an export thread is running.

## Frame order

Every eframe frame calls `App::ui` in this order:

1. `poll_preview(ctx)` — drain all `PreviewMsg`, discard stale generations, load
   a texture for the newest matching frame.
2. `poll_export()` — drain `ExportMsg::Progress` / `Finished` without blocking.
3. `handle_timeline_keys(ctx)` — transport shortcuts if no text widget owns the
   keyboard.
4. `advance_playback(ctx)` — wall-clock playhead advance and seek scheduling.
5. Debounce check: if `dirty_at` is elapsed, clear it and `refresh_preview()`
   (compile, build argv, request a frame). Otherwise request a repaint at the
   remaining time.
6. Draw `top_bar`, `left_panel`, `viewer`, `timeline`, then `CentralPanel::canvas`.
7. Schedule repaints:
   - 30 ms while `preview_pending`;
   - 16 ms while `dirty_at` is set (a scrub may have been scheduled during draw,
     after step 5's check);
   - 50 ms while `export_rx` is set.

Step 7 is why scrubbing responds on the next frame instead of waiting for a
mouse move: the seek is issued while painting the timeline, after the debounce
test for that frame.

## Preview pipeline

```text
UI edit / scrub / play tick
        │
        ├── touch() or touch_throttled()/seek_to_secs()
        │     ├── graph_version += 1        (cancels stale results)
        │     ├── engine.cancel()           (aborts child; kill_on_drop)
        │     └── dirty_at = now, debounce = 80 or 33 ms
        │
        ▼  (after debounce, on UI thread)
compiler::compile(&graph)
        │
        ├── Err → compile_error, texture = None, cancel, bump generation
        │
        └── Ok(Compiled)
              ├── gpu_args = if accel != Cpu { preview_args(t, accel) }
              ├── cpu_args = preview_args(t, Cpu)
              └── engine.request(graph_version, gpu_args, cpu_args)
                    │
                    ▼  (Tokio worker thread)
             spawn ffmpeg -frames:v 1 ... pipe:1
                    │
             ┌──────┴───────┐
             │ success      │ GPU failure
             ▼              ▼
        Frame {gen,     force_cpu = true
          pixels}       rerun cpu_args
             │              └── Frame {gen, pixels, note}
             ▼
        unbounded mpsc channel
             │
             ▼  (next UI frame)
        poll_preview → gen == graph_version ? load RGBA texture : drop
```

### Cancellation and generations

`PreviewEngine::request` aborts the previous Tokio task before spawning the new
one. The child was created with `kill_on_drop(true)`, so aborting the task drops
the future and kills FFmpeg instead of leaking it. This is why a fast slider drag
does not accumulate hundreds of decoders.

Results are addressed by generation rather than by "latest wins":

- `touch()` / `touch_throttled()` / a cancelling seek bump `graph_version`;
- the same value is passed to `request`;
- `poll_preview` compares `generation != graph_version` and drops mismatches;
- the value is also used to invalidate the in-flight texture when compilation
  breaks.

`preview_pending` is set on request and cleared on Frame/Error or cancel.

### GPU fallback

- `Accel::Cpu` passes `gpu = None`. The worker takes the CPU path directly; no
  GPU process is ever spawned.
- Otherwise `gpu = Some(args)` and the CPU args are kept as a fallback.
- If the GPU run fails (spawn error, non-zero status, short stdout), the worker
  sets the sticky `force_cpu` flag, runs the CPU command now, and attaches a
  one-line note with the first line of the GPU error (up to 160 chars).
- `force_cpu` is an `AtomicBool` owned by the engine, so subsequent requests skip
  the failed GPU attempt without UI round-trips.
- Changing the acceleration combo calls `engine.reset_fallback()`, giving the
  new backend a fresh chance.

This is *preview* fallback. Export has its own fallback: if a non-CPU export
fails, the UI thread runs the whole export again with CPU arguments and reports
`(CPU fallback)` on success. The GPU export is not retried after a CPU success.

### Why a Tokio runtime

The worker needs to spawn a child, await it, read pipes and be abortable from the
UI. `tokio::process::Command` gives `kill_on_drop` and `JoinHandle::abort()`
without hand-rolling threads and shared kill flags. The runtime is
multi-threaded with two worker threads, created once at app start. If it cannot
be built, the app still opens (the status line shows the error) but previews
never arrive.

Reading stdout/stderr uses `wait_with_output`, which drains both concurrently.
That prevents the classic deadlock where FFmpeg fills a pipe while the UI waits
for the child.

## Export pipeline

```text
start_export()
  ├── requires self.compiled (otherwise status error)
  ├── total = preview::engine::estimated_duration(&graph)
  ├── gpu_args = compiled.export_args(path, accel)
  ├── cpu_args = compiled.export_args(path, Cpu)
  ├── channel::<ExportMsg>()
  └── std::thread::spawn:
        if accel == Cpu: run cpu
        else: run gpu; on Err run cpu
        send Finished(Result<message, message>)

run_export(args, total, on_progress):
  ├── inject -progress pipe:1 before -y (or append)
  ├── spawn ffmpeg (std::process)
  ├── drain stderr on a helper thread into a Vec
  ├── read stdout lines:
  │     out_time_us=<µs> → frac = µs/1e6/total clamp 0..0.99
  │     progress=end     → 1.0
  ├── wait, join stderr, success → 1.0
  └── failure → Err(last 600 chars of stderr)
```

The UI polls `export_rx` with `try_recv()` in `poll_export`; a disconnected
channel means the worker died and is surfaced as an error. The Export button is
disabled while `export_rx.is_some()`, so at most one export runs.

Progress is deliberately capped at 0.99 until completion, preventing a
flickering “100%” while muxing. The denominator is
`estimated_duration(&graph)`:

- the maximum configured duration among Solid Color, Image and Text nodes;
- the maximum probed duration among Input nodes (`ffprobe`, then an
  `ffmpeg -i` stderr `Duration:` fallback);
- at least 5.0 s when nothing is known.

It is **not** the timeline sequence length and does not account for `speed`,
trim windows or composition. Treat the bar as an estimate; it can under- or
over-shoot. It still reaches 1.0 on success.

## Timeline integration

The timeline is a field on `Graph` rather than a parallel document, so:

- one `save`/`load` handles both;
- `compile(&graph)` can append clip timing without changing its signature;
- deleting a node can atomically fix up clips;
- a pre-timeline `graph.json` works via `#[serde(default)]`.

`Graph::sync_timeline_clips` is called from the UI after adding a node, from
`demo_graph`, and from the **Sync clips** button (file loading calls
`timeline.repair` instead). Node removal calls `timeline.on_node_removed`. The
compiler itself never mutates the timeline; `timeline_gate` is a read-only
lookup.

The UI owns the interactions, not the model: `timeline_view` computes
frame↔pixel transforms from zoom/fps/scroll, then calls
`TimelineState::drag_clip`, `set_playhead`, `set_in`, `set_out`, and uses
`App::seek_to_secs` to couple sequence time to the preview clock. Snapping and
clamping live in `timeline/mod.rs` so they are unit-testable without egui.

## Persistence and compatibility

- `GRAPH_FILE = "graph.json"`, `EXPORT_FILE = "export.mp4"`, both relative to the
  process working directory.
- Startup: `read_to_string(graph.json)` → `serde_json::from_str::<Graph>` →
  fallback `demo_graph()`. A corrupt file silently starts the demo.
- Load: renumber `node.id` to vector index, drop out-of-range links, `repair` the
  timeline, reset transient UI state, `touch`.
- Save: `serde_json::to_string_pretty` then write; errors become red status
  messages.
- There is no schema version and no autosave. `#[serde(default)]` on `timeline`
  is the only explicit compatibility affordance. Unknown object fields are
  ignored by serde by default; unknown enum variants fail the whole load (and the
  startup path then falls back to the demo).
- Floats are written in JSON's shortest round-trippable form. A save/load round
  trip preserves the graph structurally, though it can change formatting.
- The size of `graph.json` is dominated by node fields and is tiny (a few KB for
  normal graphs).

## Threading summary

| Work | Thread | Cancellation |
| --- | --- | --- |
| egui paint, graph edits, compile, save/load | UI (main) | n/a |
| Preview FFmpeg spawn/wait/read | Tokio worker (2-thread runtime) | task abort + `kill_on_drop` |
| Export FFmpeg | dedicated `std::thread` | none from UI; runs to completion |
| Export stderr drain | another `std::thread` | joined after child exits |
| Duration probing | UI thread, synchronous | short external process |

Duration probing during export start can block the UI briefly if `ffprobe` is
slow. Preview and export never do.

Shared state across threads is limited to owned `Vec<String>` argv, a cloned
`std::sync::mpsc::Sender`, and an `Arc<AtomicBool>` (`force_cpu`). The graph is
not shared with FFmpeg workers; argv is a snapshot. Editing during export
therefore does not change the running export.

## Testing and design constraints

- Unit tests live next to the modules. There are 39 at the time of writing,
  covering cycle/port validation, `odd_matrix`, image/solid/text mappings, text
  position sanitising, compiler filters, fan-out, timeline drag/loop/timecode,
  `probe_duration` for a missing file, export progress, and shell quoting.
- Real-FFmpeg tests are guarded: if `ffmpeg` cannot be spawned they return early,
  so the suite still passes on a machine without FFmpeg.
- The `full_layer_stack_compiles_and_renders_with_real_ffmpeg` and
  `color_nodes_render_with_real_ffmpeg` tests render a frame and assert exactly
  921600 bytes.
- Timeline and node logic have no UI dependency, so they are tested as plain
  Rust.
- The canvas is hand-rolled rather than using `egui_node_graph2`, which is pinned
  to egui 0.29 while this app uses eframe 0.37. It is ~650 lines, handles typed
  ports, drag-away rewiring, Bézier hit testing and wire selection.

Shortcuts deferred on purpose are marked in source with `shortcut:` or
`ponytail:` comments. Notable ones:

- GPU acceleration is decode/encode only, no filter-side acceleration.
- Export audio is the first input's audio only, no mixing.
- One Output root, first match.
- Input clip lengths are guessed at 5 s; no ffprobe-backed timeline.
- Preview is one frame at a fixed 640×360, not a playing proxy.
- No undo/redo, no multi-selection, no copy/paste, no node search.
- No tests for egui painting or native dialogs; those are exercised manually.
