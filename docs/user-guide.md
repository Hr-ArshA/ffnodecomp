# User Guide

This page covers the interface and day-to-day workflow. For node parameters see
[Nodes](nodes.md); for timeline editing see [Timeline](timeline.md); for
compilation details see [Compilation](compilation.md).

## Window layout

FFNodeComp opens at 1440×900 (minimum 1000×640) with five resizable regions:

```
┌───────────────────────────────────────────────────────────┐
│ Top bar: acceleration, save/load, export path, Export MP4  │
├──────────────┬──────────────────────────────┬─────────────┤
│ Add node     │                              │ Preview     │
│ + Inspector  │          Node canvas         │ 640×360     │
│ (left)       │                              │ + command   │
│              │                              │   inspector │
├──────────────┴──────────────────────────────┴─────────────┤
│ Timeline: transport, ruler, Video 1 / Audio 1 tracks       │
└───────────────────────────────────────────────────────────┘
```

The left, right and bottom panels are draggable separators; the canvas fills the
remaining central area.

## Top bar

| Control | Behaviour |
| --- | --- |
| **Acceleration** | `CPU / software`, `NVIDIA (CUDA / NVENC)`, `Apple (VideoToolbox)`, `Linux (VAAPI)`. Changes decode flags for preview and the encoder for export. Changing it also re-enables a GPU path after an automatic CPU fallback and schedules a new preview. |
| **Save graph** | Writes pretty-printed JSON to `graph.json` in the process working directory. |
| **Load graph** | Reads `graph.json`, repairs dangling ids and timeline references, and closes the preview texture (re-rendered on the next tick). |
| **Export:** text field | Output path, default `export.mp4`. Relative paths are relative to the working directory. |
| **Browse...** | Native save dialog, filtered to `.mp4`. Starts in the current export file's parent directory when it exists. |
| **Export MP4** | Disabled while the graph does not compile, while another export is running, or when the path is empty. |
| Progress bar | Appears only during export. Shows `out_time_us / estimated source duration`, capped at 99%, then 100% on completion. |
| Status line | Green success or red error message; also used for connection and load/save feedback. |

**Save graph / Load graph have no auto-save and no "save as".** They always use
`graph.json` in the directory the process was started from. `graph.json` is
listed in `.gitignore`, so personal graphs are not committed by accident.
Close the app only after saving, or copy the file first.

## Adding nodes

The **Add node** grid on the left creates each node type at a staggered position
near the top of the canvas and selects it. Adding a node is a structural edit:
the timeline's video sources are synchronised, the preview is cancelled and
recompiled after the 80 ms debounce.

**Output** is disabled when the graph already contains an `Output` node. Only
one Output is supported by the UI; a hand-edited graph with several would compile
the first one in node order (see [Compilation](compilation.md)).

Available buttons, in grid order:

```
Input          Transform
Color          Overlay
Output
Chroma Key     Alpha Key
Text           Image
Solid Color    Blur
Sharpen        3D LUT
Color Balance  Levels
Invert / Solarize
```

New nodes get sensible defaults; see the "Palette defaults" table in
[Nodes](nodes.md).

## Canvas interactions

The canvas is hand-painted: a 40 px grid, rounded node cards, typed ports, and
Bézier wires. Each node is 190 px wide.

| Action | Result |
| --- | --- |
| Click a node body, or drag its header | Select it; the inspector shows its fields. Deselects any selected wire. |
| Drag a node header | Move the node. Positions are clamped to `x ≥ 0`, `y ≥ 0`. |
| Click an **output** port (right side), then an **input** port (left side) | Connect. The second click validates type and cycles, replaces anything already connected to that input, and re-renders the preview. |
| Click a selected output port again | Cancel the pending connection. |
| Click an **input** port with no pending output | Shows "Click an output port first" status. |
| Drag away from a **connected input** port | Detaches that wire and starts dragging it from the original source. Release over a compatible input to reconnect; release over empty space to leave it disconnected. |
| Click a wire | Select it (10 px hit radius). Selected wires are highlighted yellow. |
| Right-click a wire | Context menu with **Delete Connection**. |
| `Delete` / `Backspace` | Deletes the selected wire, otherwise the selected node. Ignored while a text field has keyboard focus. |
| Inspector **Delete node** | Red button at the bottom of the inspector; same effect. |

### Port feedback

While a wire is pending, every input port is tinted:

- **Green, slightly larger** – type compatible and would not create a cycle.
- **Red** – incompatible type (including a self-link or a cycle).

Port colours identify kinds:

| Port kind | Colour | Hex |
| --- | --- | --- |
| Video | Orange | `#F08C32` |
| Audio | Green | `#64D278` (type exists; no MVP node emits audio) |
| Mask | Magenta | `#DC5AC8` |

`Video → Mask` is allowed (a video can drive an alpha-key matte). All other pairs
must match exactly. See [Nodes](nodes.md#ports).

### Inline controls on cards

Some nodes expose their most-used controls directly on the card, in addition to
the inspector:

- **Input** and **Image** – a file-picker button (`Select video...`,
  `Change video...`, `Select image...`, `Change image...`).
- **Chroma Key** – an sRGB colour button, hex readout, `G`/`B` screen presets,
  **Sim** (similarity) and **Blend** sliders.
- **Alpha Key** – mode combo box, **Thr**, **Tol**, **Soft** sliders.

All inline changes are full graph edits and are debounced like inspector changes.

## Inspector

Selecting a node shows its title and index (`Color Correction #2`) followed by
its fields. Fields are live-bound to the data model; every change bumps the graph
generation and schedules a recompile. See [Nodes](nodes.md) for every control and
range.

Common inspector elements:

- **Browse...** – native file picker. Dialog filters and starting directories:
  - Input video: `mp4, mkv, mov, avi, webm, flv, m4v`
  - Image: `png, jpg, jpeg, webp, bmp, gif`
  - Font: `ttf, otf, woff, woff2`
  - 3D LUT: `cube, 3dl`

  A dialog opens in the current path's parent folder if it exists. If the file
  dialog is cancelled, the field is unchanged.
- **Text edit fields** accept absolute paths as well as relative ones.
  Spaces are fine; the compiler passes paths as argv elements, not shell text.
  LUT paths containing filtergraph separators are escaped separately.
- **Delete node** – removes the node, all its wires, and its timeline clip.
  Remaining node ids are renumbered; wires and timeline references are fixed up.

## Node canvas keyboard and mouse summary

| Input | Action |
| --- | --- |
| Left click node | Select |
| Drag header | Move node |
| Click output, then input | Connect |
| Drag from connected input | Detach/rewire |
| Click wire | Select wire |
| Right click wire | Delete-wire menu |
| `Delete` / `Backspace` | Delete selected wire, else selected node |
| Space | Play/pause (timeline; ignored while typing) |
| `←` / `→` | Step one frame back/forward |
| `Home` | Jump playhead to frame 0 |
| `I` | Set loop in point at playhead |
| `O` | Set loop out point at playhead |
| `Ctrl` + mouse wheel over timeline | Zoom timeline |
| Mouse wheel over timeline | Pan timeline horizontally |

## Preview panel

The right-hand panel is a single-frame preview, not a player.

- **time (s) slider** – 0 to the timeline duration. Moving it pauses playback,
  moves the timeline playhead to the nearest frame, cancels the in-flight frame,
  and schedules a new extraction on the 33 ms scrub debounce.
- **Image** – the last successful frame, 640×360, letterboxed and scaled to fit
  the panel width if narrower.
- **No preview frame** – shown until the first frame arrives.
- **Yellow note** – e.g. `GPU unavailable, using CPU. (...)` after a fallback.
- **Red preview error** – FFmpeg failure text (last 600 characters of stderr).
- **Red graph error** – compiler validation error.
- **Generated FFmpeg command** – a collapsible, scrollable, monospace rendering
  of the *preview* argument vector for the current timestamp and acceleration.
  It is shell-quoted for pasting into a terminal. Export uses a different tail
  (`[vout]`, encoder flags, audio mapping) and is not shown here.

The preview is always 640×360 RGBA regardless of the Output node size. The
Output canvas is scaled down (preserving aspect) and letterboxed in black.

## Timeline

The docked bottom panel contains the transport, ruler, tracks, clips and
playhead. Full behaviour is in [Timeline](timeline.md). Quick reference:

- **Play / Pause** (`Space`), **Start** (`Home`), **<** / **>** one frame.
- **Set In** (`I`) / **Set Out** (`O`) / **Clear loop** for a playback loop range.
- Timecode readout in `HH:MM:SS:FF`, plus the current loop range.
- **fps** selector: 24, 25, 30, 50, 60. Changing it does **not** rescale existing
  frame values.
- **Snap** checkbox (on by default); **–** / **+** zoom buttons.
- **Sync clips** creates a clip for every missing video source (Input, Image)
  and removes clips whose node no longer exists.

## Exporting

1. Make sure the graph compiles — the Export button stays disabled while it
   does not, and the preview panel shows the compile error.
2. Set the destination in the top bar (type it or use **Browse...**).
3. Press **Export MP4**.

The export runs on a background thread, so the editor and preview stay
responsive. The progress bar appears in the top bar while it runs. On
completion the status line reports the file path. If a GPU encoder was selected
but failed, export automatically retries once with the CPU path and reports
`(CPU fallback)`.

Output is always H.264:

| Acceleration | Encoder | Extra |
| --- | --- | --- |
| CPU | `libx264 -preset veryfast -crf 23` | `-pix_fmt yuv420p` |
| NVIDIA | `h264_nvenc -preset p4 -cq 23` | `-pix_fmt yuv420p` |
| Apple | `h264_videotoolbox -b:v 6M` | `-pix_fmt yuv420p` |
| VAAPI | `h264_vaapi -qp 23` | `format=nv12,hwupload`; no `-pix_fmt` |

If the graph contains any reachable `Input` or `Image` node, export also maps
`0:a? -c:a aac`, i.e. the audio of the first FFmpeg input, when present. Audio is
otherwise absent. The export automatically overwrites the destination
(`-y`).

## Persistence and state

Saved to `graph.json` on **Save graph**:

- nodes (id, kind and all fields, canvas position)
- links (from/from-port/to/to-port)
- the whole timeline: playhead, timebase, duration, zoom, scroll, playback state,
  loop range, tracks, clips, next clip id, selection, snapping flag

Not persisted:

- acceleration choice, preview timestamp, export path, window layout,
  in-flight preview/export, status messages.

The graph is loaded on startup if the file parses; otherwise the built-in demo
graph is used. There is no automatic save.

## Current limitations (short)

- Preview is a single still frame, not video playback.
- Audio has a type and a track but no producing nodes or mixing.
- One clip per source node; clips are not blended/composited against each other.
- Multiple Output nodes are not supported by the UI; compilation uses the first.
- `Input` clips default to 5 s until you trim them; the file is not probed for the
  timeline.
- GPU acceleration covers hardware decode and encode only; filters run on CPU.
- `.cube` / `.3dl` LUT files are not validated before FFmpeg runs.

See [Troubleshooting](troubleshooting.md) for symptoms and workarounds.
