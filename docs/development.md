# Development

## Prerequisites

- **Rust:** a recent stable toolchain, edition 2021. No nightly features are used.
- **FFmpeg:** `ffmpeg` on `PATH` at runtime (decode, filter, encode).
- **ffprobe:** optional but recommended; used for export duration and progress.
  If it is missing or fails, the app falls back to parsing `ffmpeg -i` stderr for
  `Duration:`, and ultimately to a 5 s estimate.
- **Desktop stack:** eframe/winit needs a windowing system and GPU/GL driver.
  On Linux the release workflow installs:

  ```sh
  sudo apt-get install -y --no-install-recommends \
    libx11-dev libx11-xcb-dev libxcb-render0-dev libxcb-shape0-dev \
    libxcb-xfixes0-dev libxkbcommon-dev libasound2-dev libgl1-mesa-dev
  ```

- **Native file dialogs:** `rfd` 0.17 uses the XDG desktop portal on Linux, with
  a Zenity fallback. Run under a desktop session that provides one of them if
  **Browse...** does not open. Dialog failures are not fatal; paths can be typed.
- FFmpeg must include the filters and encoders you use: `drawtext` requires
  libfreetype; `lut3d` is built-in; H.264 encoders depend on the build
  (`libx264`, `h264_nvenc`, `h264_videotoolbox`, `h264_vaapi`).

## Build and run

```sh
cargo run --release
```

Debug builds are usable but slower. `Cargo.toml` already raises optimisation:

```toml
[profile.dev]
opt-level = 1

[profile.dev.package."*"]
opt-level = 3
```

Dependencies are optimised harder than the app crate so the UI stays responsive
in `cargo run`.

Other commands:

```sh
cargo build --release            # binary at target/release/ffnodecomp
cargo check                      # fast type check
cargo test                       # all unit tests
cargo test node::tests           # filter by module/test-name substring
cargo test -- --nocapture        # show test output
cargo fmt                        # format
cargo fmt --check                # verify formatting
cargo clippy --all-targets       # lint all code, including tests
```

Recommended pre-commit gate:

```sh
cargo fmt --check && cargo clippy --all-targets && cargo test
```

## WebAssembly

The browser build uses the same `ui::App` through `src/lib.rs`; `main.rs` is
native-only. `Cargo.toml` splits dependencies by target: Tokio is native-only,
while `wasm-bindgen`, `wasm-bindgen-futures`, `js-sys` and `web-sys` are
wasm-only. `web-time` replaces `std::time::Instant` on wasm.

Prerequisites:

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk wasm-bindgen-cli
```

`trunk` and `wasm-bindgen-cli` must be compatible with the `wasm-bindgen`
crate version Cargo resolves. A version mismatch produces an error telling you
which CLI version to install.

Build and serve:

```sh
trunk build                 # debug; dist/ is ready for any static server
trunk build --release
python3 web/serve.py        # http://127.0.0.1:8080, with COOP/COEP headers
```

`index.html` selects the library target with
`data-target-name="ffnodecomp_web"` because the crate also has a native binary.
Trunk injects the `init()` call and exposes the exports as
`window.wasmBindings`; the page then constructs `WebHandle` and starts it on
`#the_canvas_id`.

The FFmpeg runtime is not bundled. `web/ffmpeg_bridge.js` loads pinned
`@ffmpeg/ffmpeg`, `@ffmpeg/util` and `@ffmpeg/core` builds from jsDelivr on
first use, so the first preview or export needs network access. Browsers refuse
cross-origin Worker URLs, so the matching `worker.js`, `const.js` and
`errors.js` are vendored under `web/ffmpeg_worker/` (copied into `dist/` by
Trunk) and passed via `classWorkerURL`. Keep that copy in sync when bumping
`@ffmpeg/ffmpeg`. The core is single-threaded; `web/serve.py` still sends
`Cross-Origin-Opener-Policy` and `Cross-Origin-Embedder-Policy` so a
multi-threaded core can be swapped in later.

Check the wasm target explicitly:

```sh
cargo check --target wasm32-unknown-unknown
trunk build
```

There is no wasm test runner. Rust-side graph/compiler tests run natively;
browser behaviour (dialogs, MEMFS, downloads) is manual.

## Test suite

Tests are inline `#[cfg(test)]` modules next to the code they exercise. There is
no separate integration-test directory or fixture tree.

What is covered:

- **graph::mutations** – replace-on-input, disconnect edge/port, type mismatch,
  cycle rejection, direction checks, out-of-range ids.
- **graph::node** – cycle/self-link rejection, `odd_matrix` clamping.
- **graph::nodes::*** – image input options and clamps; solid-colour filter
  string; drawtext escaping, position expressions, source-mode canvas.
- **graph::compiler** – linear graphs have no `split`; fan-out inserts one;
  alpha-key mask vs luma paths; image as looped input; missing input file;
  timeline trim/setpts and output-side seek; blur/sharpen filters and odd matrix
  coercion; all four colour nodes; LUT path escaping; neutral nodes identity;
  text layer; solid-color source.
- **preview::engine** – `probe_duration` returns `None` for an empty/nonexistent
  path; `run_export` reports progress and writes a file (guarded by FFmpeg
  availability).
- **timeline** – timecode without floats, trim/move clamping, sync/repair, in/out
  ordering, snapping to clip edges and playhead.
- **ui** – shell quoting of paths with spaces.

Guides:

- Most tests are pure and always run.
- Real-FFmpeg tests check `ffmpeg -version` first and `return` early if FFmpeg is
  unavailable, so `cargo test` works on a bare Rust machine.
- `full_layer_stack_compiles_and_renders_with_real_ffmpeg` additionally requires
  the sample image `/tmp/opencode/img.png`; it skips if missing. To run it, put
  any PNG there.
- `color_nodes_render_with_real_ffmpeg` writes its own identity `.cube` into the
  system temp directory, renders 640×360, and requires exactly 921600 stdout
  bytes.
- `run_export_reports_progress_and_completes` writes
  `/tmp/opencode/export_test.mp4`; create that directory first if it does not
  exist.
- The suite contains 41 tests at the time of writing. New node work should add at
  least a filter-string test, plus a real render test when the FFmpeg mapping is
  non-trivial.

The test command in CI is not configured; release builds do not run tests.
Run them locally before tagging.

## Dependencies

```toml
eframe = "0.37"                # egui + winit + glow; native app shell
rfd = "0.17.2"                 # open/save dialogs (native + browser async)
serde = { version="1", features=["derive"] }
serde_json = "1"               # graph.json
web-time = "1"                 # wasm-safe Instant
tokio = { version="1", features=["full"] }  # native-only: preview process
wasm-bindgen / js-sys / web-sys             # wasm-only bridge
```

There is no `egui_node_graph2`: it is pinned to egui 0.29 while this app needs
0.37, so `src/ui/canvas.rs` is a hand-rolled canvas (~650 lines). There are no
FFmpeg bindings; the CLI is the API natively and `@ffmpeg/ffmpeg` is the API in
the browser. No image, audio, or logging crates. Avoid adding a dependency
unless the task genuinely needs it.

## Project conventions

- **Layering:** `graph` and `timeline` never import egui/eframe. UI code calls
  them. This keeps the compiler and timeline unit-testable and serialisable.
- **Node modules:** one file per node kind that emits its own source/filter
  string. Simple filters stay in `compiler.rs` to keep their RGBA wrapping
  visible in one match.
- **Clamp at the boundary:** UI widgets offer a range, but every outgoing numeric
  value is also clamped in the compiler. Tests assert both. Never trust
  hand-edited JSON.
- **One home per mapping:** if a node needs a filter string, it lives in exactly
  one function. Do not grow a second “preview-only” filter path.
- **Deletion over abstraction:** no node trait, no factory, no registry, no
  plugin system. A new node is one enum variant and some match arms.
- **Determinism:** stable ordering and fixed formatting precision, so compiled
  output and tests do not churn.
- **Comments:** explain the *why* the code cannot show. Deferred work is marked
  with `shortcut:` and `ponytail:` comments; keep them accurate.
- **Dependency-lean:** standard library/platform/first-party patterns before
  crates.

## Adding a node – checklist

The steps below follow the shape of an existing filter such as `Blur`.

### 1. Data model (`src/graph/node.rs`)

- Add a variant to `NodeKind`. Use owned `String` for paths, plain numeric types
  for values, arrays for RGB.
- Add its display name to `NodeKind::title`.
- Add a one-line `summary()` arm (what appears on the card).
- `inputs()`/`outputs()`: a normal one-video-in/one-video-out node needs **no
  change** – `_ => &VIDEO_IN` and `_ => &VIDEO_OUT` cover it. Special cases
  (zero inputs, two inputs, optional mask) get an explicit arm.
- Serde: the variant is automatically serialisable; its JSON name is the Rust
  name. Never rename a shipped variant without accepting the break.

### 2. Compiler (`src/graph/compiler.rs`)

- Add an arm to the `filter` match in `compile`'s node loop.
- Clamp every parameter to a range FFmpeg accepts; format numbers explicitly
  (`{:.3}` etc.) so output is stable.
- If the filter can change pixel format/chroma, wrap it:

  ```rust
  format!("format=rgba,<filter>...,format=rgba")
  ```

  Preserve alpha unless the effect deliberately does not.
- Escape option values. If the value can contain `: , [ ] ; ' \`, use or extend
  the same double-level escaping as `escape_lut_path`, and add a real-FFmpeg
  path test. Do not reuse `drawtext`'s quoting for filter options blindly.
- If the node is a new FFmpeg **input**, add it to the input-collection match in
  stage 6 and put demuxer options in `InputSpec::extra`; set `decode: false` for
  stills. If it is a **lavfi source**, no input entry is needed.
- If the node needs a non-video port, `PortSpec { optional: true }` lets it be
  unconnected. Multi-ported nodes with special label order or extra statements
  (like Alpha Key) need explicit code before the generic `emit`.
- If it is a trimmable source (`Input`- or `Image`-like), call `timeline_gate(graph, id)`
  in its filter arm, add it to `natural_frames`, and include it in the
  `timeline_active` check. Text and Solid Color deliberately are not.

### 3. UI

- `src/ui/mod.rs`, **left_panel**: add a palette button that constructs a default
  `NodeKind` and calls `self.add_node(...)`. Mind the 2-column grid (`end_row`).
- `src/ui/mod.rs`, **inspector**: add an arm with sliders/drag values/text fields.
  - Use `.range(...)` for hard bounds, and clamp again in the compiler.
  - Path fields: text edit + **Browse...** using `rfd::FileDialog` with a filter,
    starting in the current file's existing parent. Follow the Input/Image/LUT
    patterns.
  - Set `changed` on every edit; it calls `self.touch()` at the end.
- `src/ui/canvas.rs`, `accent`: add a colour for the card.
- Optional: `inline_height` + `inline_controls` for card-level controls; update
  the `24.0`/`64.0`/`84.0` height accordingly.
- `src/ui/canvas.rs`, `inline_height`: must return the exact height the controls
  consume, or ports and content overlap.
- If the node is a source, consider timeline integration (see step 2) and the
  Output button.

### 4. Tests

Add, in `src/graph/compiler.rs` tests:

- a **filter-string** test that links the node between `input()`/a source and an
  Output, compiles, and asserts the exact important substring;
- a **clamping/neutrality** test for out-of-range values if relevant;
- a **real-FFmpeg render** test only if output format, escaping or multi-statement
  behaviour is new. Guard it with the `ffmpeg -version` check, render a small
  canvas, assert the status and the exact byte count.
- For an escaping helper, add a direct unit test of input → output.

For a new source, test the “needs no input file” path (no `-i` in
`compiled.inputs`).

### 5. Documentation

- Add a bullet to `README.md` → **Nodes**.
- Add a section to `docs/nodes.md` (ports, fields, UI ranges, mapping, limits).
- If it adds a module, update `docs/architecture.md`'s module map; if it adds a
  target, update `docs/compilation.md`.
- If it changes the JSON shape, update `docs/graph-format.md`.

### 6. Verify

```sh
cargo fmt
cargo clippy --all-targets
cargo test
cargo run --release
```

Then exercise the node with real media, including a path with spaces and, for
file options, `'`, `:`, `,`, `[`, `]`, `;`.

## Release workflow

`.github/workflows/release.yml` triggers on tags matching:

```text
v[0-9]+.[0-9]+.[0-9]+*
```

e.g. `v0.1.0`, `v0.2.0-rc1`.

For each of four matrix targets:

| Platform | Target | Artifact |
| --- | --- | --- |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `.tar.gz` |
| Windows x86_64 | `x86_64-pc-windows-msvc` | `.zip` |
| macOS Apple Silicon | `aarch64-apple-darwin` | `.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | `.tar.gz` |

It:

1. checks out the repo;
2. on Linux, installs the X11/xcb/ALSA/GL headers;
3. installs the stable Rust toolchain for the target;
4. caches Cargo;
5. runs `cargo build --release --target <target>`;
6. copies the binary, `README.md` and `LICENSE*` into a tag-and-target-named
   folder (`ffnodecomp-${tag}-${target}`) and archives it (`tar.gz` on Unix,
   `7z` → `.zip` on Windows);
7. uploads the archive to a GitHub Release with generated notes.

The release job does **not** run `cargo fmt`, `clippy` or tests. The app is not
code-signed, notarised or bundled with FFmpeg; users supply FFmpeg on `PATH`.
There is no automatic version bump, so keep `Cargo.toml`'s `version` in sync with
the tag (the binary currently does not display it).

To reproduce a Linux release locally:

```sh
cargo build --release
./target/release/ffnodecomp
```

## License

MIT, copyright © 2026 Alireza Shabanpour. See `LICENSE`. Contributions are
assumed to be under the same licence.
