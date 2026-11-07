# Troubleshooting & FAQ

Start with the red text in the app. Compile errors (`Graph: ...`) and FFmpeg
errors both appear below the preview frame, while export errors appear in the
top-bar status line. The **Generated FFmpeg command** panel shows the exact
preview argv and is the best clue for FFmpeg-level failures.

Useful shell checks:

```sh
ffmpeg -version
ffprobe -version
ffmpeg -hide_banner -filters | grep -E ' (lut3d|colorbalance|colorlevels|gblur|unsharp|drawtext|negate) '
ffmpeg -hide_banner -encoders | grep -E 'libx264|nvenc|videotoolbox|vaapi'
```

## Nothing renders

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| `could not start ffmpeg: No such file or directory` | FFmpeg is not on the app's `PATH`. | Install FFmpeg, or launch the app from a shell where `ffmpeg -version` works. GUI launchers often have a shorter `PATH` than a terminal. |
| `Graph: graph needs an Output node` | You deleted the Output. | Add **Output** from the palette. |
| `Graph: Input node #N has no file` | Source path is empty or whitespace. | Select the node and **Browse...**, or paste an absolute path. |
| `Graph: <Title> node #N is missing its '<port>' input` | A required port is unconnected (or the wire was lost after a node delete). | Reconnect it. Overlay needs both background and foreground; Output needs video. |
| `Graph: <A> output is Video but <B> input N is Mask` | A mask input was fed by something the UI considers incompatible. Video→Mask is normally allowed; this usually indicates a hand-edited graph. | Reload through **Load graph**, which repairs links, or connect a Video output. |
| `Graph: graph contains a cycle` | Hand-edited `graph.json`. | Remove the offending link. The UI itself rejects cycles. |
| `Graph: <Title> node #N does not feed the output` | A reachable node has no surviving outgoing link, typically duplicate links into the same input in a hand-edited file. | Remove duplicates, then load again. |
| `Graph: Node ... does not feed the output` with a graph that looks fine | Only nodes reachable **backwards** from the first Output are compiled. A second Output or a disconnected branch is ignored. | Ensure the branch feeds the first Output. |
| `ffmpeg failed: <tail>` | FFmpeg rejected something downstream. Read the tail. | See the sections below; reproduce the shown command in a terminal. |
| `ffmpeg returned N bytes, expected 921600` | The selected timestamp produces no frame, or the process wrote a partial frame. | Move the timestamp back; see [Timestamp past the end](#timestamp-past-the-end-and-empty-preview). |
| Red `Graph:` error but the last frame remains | That is intentional: the previous good texture is kept only for some error paths; most compile errors clear it. | Fix the graph. |
| Preview and export look different in blur/sharpness | Preview filters at source resolution then downscales; export is full resolution. Softness of small details differs. | Judge on the export. |

### The preview runtime failed to start

The status line shows `could not start preview runtime: ...` at launch. The app
still edits and exports (the export thread is independent), but no previews
arrive. Restarting is the practical fix; a constrained environment that forbids
threads or I/O will keep failing.

### Timestamp past the end and empty preview

`ffmpeg returned N bytes, expected 921600. Is the timestamp past the end of the
clip?` means FFmpeg succeeded but produced no full raw frame. Possibilities:

- The seek is beyond the source's real duration. The timeline `Input` clip is a
  5 s guess, and the viewer slider runs to the **sequence** duration, which may
  be longer than the file.
- A timeline clip starts later than the playhead and it is the only source (a
  gap). A timeline-trimmed source does not exist before its `start_frame`.
- A clip was trimmed past the source's end.
- The source is a still with a shorter `duration` than the seek.
- The Output has a stream but nothing decoded at that time (corrupt/empty file).

Fixes: scrub earlier, shorten `start_frame`/increase the clip duration, increase
an Image's duration, or move the timestamp slider left. For a source shorter than
the timeline, trim the timeline duration down.

## LUT issues

| Symptom | Cause / fix |
| --- | --- |
| LUT does nothing | `path` is empty or whitespace — that is an intentional identity pass. Pick a file. |
| `No such file or directory` | Path is wrong, or `.cube`/`.3dl` was not actually selected. Use an absolute path. |
| `No option name near '...'` | A path separator reached FFmpeg unescaped. This should be handled by the compiler; if it happens, save the graph and report the exact path. Colons/apostrophes are the usual suspects. |
| `Unable to parse ...` | The LUT file is not a valid `.cube`/`.3dl`, or uses a size/orientation FFmpeg's `lut3d` does not accept. Try it with `ffmpeg -vf lut3d=file=...` directly. |
| Colours look washed/oversaturated compared with a grading app | FFmpeg applies the LUT to RGB values after `format=rgba`, without colour management, log decoding or a transfer-function conversion. Feed it the expected colour space or bake the conversion. |

The compiler does not validate the LUT at compile time; the error appears when
the preview/export process runs. A path containing spaces, `:`, `'`, `,`, `[`,
`]`, `;` or `\` is escaped for the filtergraph, but `\` handling is inherently
fragile across platforms — prefer simple directory names if a path misbehaves.

## Text / font issues

| Symptom | Cause / fix |
| --- | --- |
| `No such filter: 'drawtext'` | Your FFmpeg build lacks libfreetype. Install a full build. |
| `Cannot find a valid font for the family ...` | No font file and no usable default fontconfig. Set **Font file** to an explicit `.ttf`/`.otf` path. |
| Text shows `%{...}` literally or `%` is missing | By design: `expansion=none` and `%` is escaped. Everything is literal. |
| Custom X/Y ignored | The expression contains a character outside the allow-list (letters, digits, `+ - * / % ^ ( ) . _`, space). It falls back to centring. Use a `drawtext` expression without commas/colons. |
| Text is invisible over a video | The colour matches the footage, the text is off-canvas, or a previous filter produced a fully transparent frame. |
| Text node makes the output short | A Text node **with no video input** is a generated source bounded by its `duration`; it is not timeline-gated. Put a video source in the chain if you need the composition length. |
| `.woff` font fails | The picker offers it, but FFmpeg/fontconfig may not load web fonts. Use `.ttf` or `.otf`. |

## Colour filter issues

- **Levels gamma has a stronger effect than expected.** Gamma is an `eq` filter
  applied after `colorlevels`; it is not a `colorlevels` option. It affects the
  whole channel, including highlights/shadows, not just midtones.
- **Color Balance shifts everything.** Lift/gamma/gain are FFmpeg's
  `colorbalance` parameter names; they are not a full ASC-CDL or log-grade
  implementation. Values are plain `-1..1` RGB adjustments.
- **Invert at 0.5 looks odd.** Partial invert is a linear blend:
  `val + a*(255 - 2*val)`, so a=0.5 maps every value to 127.5 — a flat grey.
  Use it as an effect or set a=1 for a true negative.
- **Solarize does nothing at 1.0.** Threshold 1.0 means 255; only pixels already
  at 255 are affected.
- **Alpha is not inverted.** `negate` runs with its default `negate_alpha=0`, and
  the `lutrgb` expressions set only `r`, `g`, `b`. Use an Alpha Key node to
  invert alpha.
- **Missing `colorbalance`/`colorlevels`/`lut3d`** – your FFmpeg build is older
  or minimal; install a full/newer build.

## Timeline gotchas

- **Input clips are 5 seconds** regardless of the file. Drag the right handle to
  the real length. The app does not probe for this.
- **Sync clips does not resize.** It only creates missing clips and removes
  orphans. Change an Image duration, then adjust the clip's right edge yourself.
- **New clips start at frame 0.** If you add several sources and press Sync, they
  overlap logically (each source feeds its own node; that is fine) but all begin
  at the same sequence position. Move them.
- **Clip duration only grows.** Shortening the furthest clip does not shorten the
  sequence ruler.
- **Changing fps reinterprets frames; it does not rescale them.** Frame 150 stays
  frame 150. Also, the change is not by itself a graph edit; the compiled filter
  updates on the next edit/seek.
- **Loop In/Out is preview-playback only.** It is not an export range. Export
  renders the entire graph. To export a section, trim the source clips.
- **Out is exclusive.** `Out` at frame 40 plays the last frame at 39.
- **Out at frame 0 is rejected**, because that would be an empty loop.
- **Audio track is a placeholder.** Sync clears it, nothing renders it, and
  export ignores it. See [the audio FAQ](#exported-file-has-no-audio).
- **Speed is not in the UI.** The data model and compiler support `speed`; edit
  `graph.json` to try it.
- **Overlapping clips are not composited.** The graph decides compositing; two
  clips on one track simply both reference nodes.
- **Clean, then preview.** After deleting nodes or adding clips, a preview of an
  empty timeline region can fail. Scrub to a covered frame.

## Export problems

### The Export button is disabled

It requires all of:

- `compiled` is `Some` (no compile error);
- no export is already running;
- the export path is non-empty after trim.

Fix the graph, wait for the running export, or enter a path.

### Export fails immediately

The path's parent directory must exist and be writable; FFmpeg does **not** create
directories. `export.mp4` is relative to the working directory, which may be
read-only (e.g. launching from a `.app` or a system folder). Use **Browse...** to
pick a writable location.

Codec availability is another common cause: a “CPU” export still needs libx264 in
your FFmpeg build. Check `ffmpeg -encoders | grep libx264`; use a full FFmpeg
build otherwise.

### GPU export failed but CPU succeeded

Expected. The UI runs the whole export again with CPU args and reports
`(CPU fallback)`. The GPU and CPU messages are both in the error string if both
fail. Common causes:

- the selected backend's encoder is not in this FFmpeg build
  (`h264_nvenc`, `h264_videotoolbox`, `h264_vaapi`);
- no compatible hardware/driver;
- VAAPI device is not `/dev/dri/renderD128` (the path is hard-coded);
- CUDA/VAAPI requested on a machine without the vendor runtime.

Acceleration is a preference, not a guarantee. The preview uses decode-only
`-hwaccel` and falls back once, then stays on CPU until you change the
acceleration combo.

### Progress looks wrong or stuck

`estimated_duration` is the maximum of configured durations and probed Input
durations — **not** the timeline sequence or the current loop. Re-timed or
trimmed clips can make the real export much shorter/longer. The bar is capped at
99% until completion and jumps to 100% at the end; very fast exports may appear
to jump directly. `out_time_us` is read from FFmpeg's progress pipe.

If it sits at 0% for a long time, FFmpeg is likely still analysing/opening inputs
or a filter (blur, LUT at high resolution) is expensive. Check the process is
alive.

### Exported file has no audio

Audio is not mixed or timeline-aware. Export adds exactly:

```text
-map 0:a? -c:a aac
```

only when the graph has at least one file input, and only for **FFmpeg input 0**
(the first reachable `Input`/`Image` in node order):

- If the first input is an **image**, it has no audio; the `?` makes that
  non-fatal, and audio from later video inputs is not mapped.
- If the source video has no audio stream, there is no audio.
- If the source codec cannot be decoded/transcoded by your FFmpeg, the map may
  fail or yield silence.
- Timeline audio clips do not exist; adding an audio track in JSON does nothing.
- The Audio *port kind* exists only in the type system; no node produces it.

Workaround: place the video source first in node order, or mux audio afterwards
with ffmpeg.

### Exported file has no video / is corrupt

- Check the Output canvas dimensions are even. The UI/compiler forces even sizes;
  hand-edited dimensions are rounded down.
- Check the first input and the graph produce frames. A timeline gap or a clip
  trimmed past the source's end can yield an empty stream.
- Try `Acceleration: CPU / software` and export again.
- If an output file already exists and is locked (player open), `-y` may still
  fail at the OS level. Close the player.

## Canvas and editing

- **Can't connect two ports.** Types must match; only `Video → Mask` is allowed.
  A cycle is rejected. Look at the port tint while dragging: green is valid, red
  is not.
- **One input keeps replacing another.** Input ports are single-source by
  design. Branch with an output fan-out (the compiler inserts `split`).
- **Delete does nothing.** A text field has focus, so keyboard shortcuts are
  suppressed. Click the canvas background first.
- **Wire clicked is the wrong one.** Wires are hit-tested within 10 px and only
  when the pointer is not over a node body. Zoom/scroll is not implemented because
  the canvas has no pan; move nodes instead.
- **Node positions reset / overlap after adding.** New nodes are staggered by
  `index % 4` / `index % 5`, which repeats every few nodes.
- **No undo.** Save often. **Load graph** discards unsaved changes without a
  prompt.
- **Deleting a node shifts other nodes' numbers.** Ids are array indexes; the
  inspector title and timeline references are updated in memory, but any notes
  you keep about “node #7” may become stale.

## Saving / loading

| Symptom | Explanation |
| --- | --- |
| “My graph is gone” on startup | Startup silently falls back to the demo if `graph.json` is missing, unreadable, or fails to parse. The file may be in a different working directory than you think (e.g. when launched from a desktop icon). |
| Load reports a JSON parse error | An unknown node variant, a missing required field, a wrong type, a partial `timeline` object, or `"timeline": null`. Serde errors are usually explicit about the line/field. |
| Hand-edited graph misbehaves | Node `id`s must match their array index; startup does not renumber them. Load through the app once (the **Load graph** button renumbers ids and drops out-of-range links) and save. |
| Paths with backslashes broke the file | JSON escaping: use `\\` for a literal backslash, `/` where possible. |
| Two app windows overwrite each other | Both use the same `graph.json`; the last **Save graph** wins. No locking. |
| Closing without saving lost changes | There is no auto-save and no close prompt. |

To recover from a suspected corruption, copy `graph.json` aside and use the
app's **Load graph**; it reports the parse error and leaves the current graph
untouched. Fixing the file by hand is safe as long as the enum strings and field
types are exact (see [Graph format](graph-format.md)).

## File dialogs

- **Browse... does nothing / errors.** `rfd` needs a desktop portal (Linux) or
  Zenity as its fallback. Under a bare window manager, SSH, container, or a
  sandbox without portal access, native dialogs may fail. Type the path directly.
- **Dialog starts in the wrong folder.** It starts in the current value's parent
  only if that directory exists; otherwise it uses the system default.
- **Wrong file is hidden by the filter.** Use the dialog's “All files” option
  where available, or type the path.

## Performance

- Preview renders one frame per debounce window. Every slider release spawns an
  FFmpeg process that decodes from the nearest keyframe to the timestamp; deep
  seeks in long GOP files are slow.
- The **filters run at full source resolution** before the result is scaled to
  640×360. Blur, unsharp and LUTs on 4K sources are expensive even for a small
  preview. Add/keep a Transform early or use proxy media to speed up iteration.
- Raise the preview debounce? Not exposed. Drag more slowly; each edit restarts
  an 80 ms timer and cancels the previous process.
- Export is a normal FFmpeg encode; hardware encoders are faster but can differ in
  quality/bitrate from libx264.
- A very large `graph.json` (thousands of nodes) increases compile and paint cost;
  the compiler is O(nodes + links) each debounce.

## FAQ

**Is the preview real-time?**
No. It is a still frame. Playback moves the playhead on a wall clock and requests
frames as fast as the previous extraction allows; it drops rather than queues.

**Does GPU acceleration speed up filters?**
No. It is hardware decode and encode only. `-hwaccel_output_format` is
deliberately omitted so filters stay on CPU and in system memory.

**Which pixel format does the app use internally?**
RGBA, `setsar=1`, with alpha preserved. Preview is raw RGBA 640×360 (921600
bytes). Export converts to `yuv420p` (or VAAPI's `nv12` surface) and H.264.

**Can I use relative paths?**
Yes for FFmpeg inputs and export outputs, but they resolve against the app's
working directory, not the graph file. `graph.json` itself is always in the
working directory. Absolute paths are more predictable.

**Does export cover the loop In/Out?**
No. Loop In/Out affects preview playback only. Export renders the whole graph.

**Does export start at the playhead or the timeline In point?**
No. Export always starts at the beginning of the graph and ignores the preview
timestamp and playhead. The timeline only contributes `trim`/`setpts` on
sources.

**Can I have more than one Output?**
The UI prevents it. The compiler picks the first Output in node order and ignores
the rest. Don't rely on multiple Outputs.

**Can I add audio?**
Not yet. The type exists; no node produces audio; the track is a placeholder.
Export passes through the first input's audio if present.

**Can I undo?**
No. Save before risky edits. Loading a saved graph is the undo.

**Why does the canvas not use `egui_node_graph2`?**
It is pinned to egui 0.29 while this app uses eframe 0.37. The custom canvas is
smaller than maintaining a fork.

**Why is the Input clip 5 seconds?**
Until the timeline probes files with `ffprobe`, a guessed length is preferable to
a zero-length clip. Trim the right edge to the real duration.

**Where is the config/cache?**
There is none. The only persistent state is `graph.json` (and the exported files
you create), both in the working directory. No recent-files list, no presets, no
settings file.

**Is there a CLI or headless mode?**
No. The compiler is a library module inside the binary and is only exposed
through the UI and tests.

**Which platforms are supported?**
The release workflow builds Linux x86_64, Windows x86_64, macOS Intel and Apple
Silicon. Anywhere eframe, rfd and FFmpeg work should build from source, but only
those four are packaged.

## Reporting a reproducible bug

Include:

1. OS/architecture, FFmpeg `ffmpeg -version`, and what the acceleration combo was
   set to.
2. The graph: **Save graph** and attach `graph.json` (redact private paths).
3. The red error text, including the full FFmpeg tail if shown.
4. The content of **Generated FFmpeg command** and whether it reproduces in a
   terminal.
5. Whether the preview works at an earlier timestamp and fails at this one, or
   fails everywhere.
6. Export destination filesystem/writability, if the problem is export-only.

Because the preview truncates FFmpeg's stderr to the last 600 characters, running
the shown command manually (`ffmpeg <args>` with the raw pipe redirected to a file)
often reveals the first useful error.
