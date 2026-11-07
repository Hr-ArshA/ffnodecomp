# Graph File Format (`graph.json`)

FFNodeComp saves the entire editor document – nodes, links and timeline – as one
pretty-printed JSON file:

- **Save graph** / **Load graph** use `graph.json` in the process working
  directory (no path picker, no alternate profile).
- Startup reads the same file if it exists; otherwise it uses the built-in demo
  graph.
- The file is in `.gitignore`, so it is not versioned by default.
- There is no auto-save. Use **Save graph** before quitting.
- There is no format version field. The schema is the serde shape of
  `Graph`/`NodeKind`/`TimelineState` at the build you are running.

## Top level

```json
{
  "nodes": [ ... ],
  "links": [ ... ],
  "timeline": { ... }
}
```

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `nodes` | array of Node | yes | Order defines ids. |
| `links` | array of Link | yes | Order is significant to the compiler (see below). |
| `timeline` | TimelineState object | no | `#[serde(default)]`: absent → `TimelineState::default()`. **Not** `null`. |

Unknown fields are ignored by serde unless the structs opt out (they do not).
Unknown enum variant names are hard errors.

## Node

```json
{
  "id": 0,
  "kind": { "Input": { "path": "/home/me/clip.mp4" } },
  "pos": [60.0, 80.0]
}
```

| Field | Type | Notes |
| --- | --- | --- |
| `id` | integer | Should equal the array index. See [Repair rules](#repair-rules). |
| `kind` | externally tagged enum | `{ "Variant": { ...fields... } }`. No unit variants. |
| `pos` | `[f32, f32]` | Canvas-space top-left. Negative values are accepted by the loader but the UI clamps on drag. |

### Node kinds and their exact fields

All field names are used verbatim as JSON keys. All fields are required for the
variant that uses them.

```json
{ "Input":          { "path": "..." } }
{ "Transform":      { "width": 1280, "height": 720, "x": 0, "y": 0 } }
{ "Color":          { "brightness": 0.0, "contrast": 1.0, "saturation": 1.0 } }
{ "Overlay":        { "x": 0, "y": 0 } }
{ "ColorKey":       { "color": [0, 255, 0], "similarity": 0.3, "blend": 0.1 } }
{ "AlphaKey":       { "mode": "LumaKey", "threshold": 0.5,
                      "tolerance": 0.1, "softness": 0.1 } }
{ "Text":           { "text": "Title", "font_size": 36.0,
                      "font_color": [255, 255, 255], "font_file": "",
                      "position": "Center", "custom_x": "", "custom_y": "",
                      "duration": 5.0 } }
{ "Image":          { "path": "...", "duration": 5.0, "framerate": 30 } }
{ "SolidColor":     { "color": [32, 32, 32], "width": 1280, "height": 720,
                      "duration": 5.0, "framerate": 30 } }
{ "Blur":           { "sigma": 3.0, "sigma_v": 3.0, "steps": 1 } }
{ "Sharpen":        { "amount": 1.0, "clarity": 0.0,
                      "msize_x": 5, "msize_y": 5 } }
{ "Lut3d":          { "path": "/path/to/look.cube" } }
{ "ColorBalance":   { "shadows":    [0.0, 0.0, 0.0],
                      "midtones":   [0.0, 0.0, 0.0],
                      "highlights": [0.0, 0.0, 0.0] } }
{ "Levels":         { "black": [0.0, 0.0, 0.0],
                      "white": [1.0, 1.0, 1.0],
                      "gamma": [1.0, 1.0, 1.0] } }
{ "InvertSolarize": { "invert": 0.0, "solarize": 0.0, "threshold": 0.5 } }
{ "Output":         { "width": 1280, "height": 720 } }
```

Enum values (always exact strings):

- `AlphaKey.mode`: `"LumaKey"` | `"ExtractAlpha"` | `"InvertAlpha"`
- `Text.position`: `"Center"` | `"TopCenter"` | `"BottomCenter"` | `"CustomXY"`

Numeric types are enforced: `u32` fields (dimensions, framerate, matrix sizes,
`steps`) reject negative or fractional JSON numbers; `f32` fields accept
integers; colour arrays must contain three integers in `0..=255`.

## Link

```json
{ "from": 0, "from_port": 0, "to": 1, "to_port": 0 }
```

All four fields are required `usize` values.

- `from` is the source node id; `from_port` indexes its **outputs** (always 0 for
  current nodes).
- `to` is the target node id; `to_port` indexes its **inputs**.
- Input ports accept one connection. If the file contains duplicates, the
  **last** link in the array wins at compile time.
- Links may form a cycle in JSON; the compiler rejects a cyclic reachable subgraph
  at render time. The interactive `connect` path rejects cycles immediately.

## Timeline

The complete shape with typical values:

```json
"timeline": {
  "playhead": 0,
  "timebase": 30,
  "duration": 300,
  "zoom": 60.0,
  "scroll_offset": 0.0,
  "playback_state": "Paused",
  "loop_range": null,
  "tracks": [
    {
      "id": 0,
      "name": "Video 1",
      "kind": "Video",
      "clips": [
        {
          "clip_id": 1,
          "node": 0,
          "start_frame": 0,
          "duration_frames": 150,
          "source_trim_in": 0,
          "speed": 1.0
        }
      ]
    },
    { "id": 1, "name": "Audio 1", "kind": "Audio", "clips": [] }
  ],
  "next_clip_id": 2,
  "selected": null,
  "snapping": true
}
```

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `playhead` | integer | `0` | Current frame. |
| `timebase` | integer | `30` | Sequence fps. Editor offers 24/25/30/50/60; any ≥ 1 is accepted (`0` behaves as 1). |
| `duration` | integer | `300` | Sequence length in frames; at least 1 after repair. |
| `zoom` | number | `60.0` | Pixels per second. UI range 8..400. |
| `scroll_offset` | number | `0.0` | Leftmost visible frame. |
| `playback_state` | string | `"Paused"` | `"Paused"` \| `"Playing"` \| `"Scrubbing"`. |
| `loop_range` | null or `[in, out]` | `null` | Out is exclusive; both are frame numbers. |
| `tracks` | array | two tracks | See Track. |
| `next_clip_id` | integer | `1` | Allocator for new clips. |
| `selected` | null or `[track_id, clip_id]` | `null` | Timeline selection. |
| `snapping` | boolean | `true` | Magnet behaviour. |

> If you include a `timeline` object, **all** of its fields must be present.
> `#[serde(default)]` applies to the whole field, not to missing members. Omit
> `timeline` entirely to accept the defaults. `"timeline": null` is an error.

`FrameIndex` and `Timecode` are newtype/struct wrappers; `FrameIndex` serialises
as a bare integer. `Timecode` is derived but not stored in the timeline; it is
recomputed from the playhead and timebase.

### Track

```json
{ "id": 0, "name": "Video 1", "kind": "Video", "clips": [] }
```

`kind` is `"Video"` or `"Audio"` (case-sensitive). The UI has no track creation
control; extra tracks in JSON are painted and, for video tracks, receive
auto-created clips. The compiler looks up clips by **node**, not track, so track
ownership only affects display.

### Clip

| Field | Type | Meaning |
| --- | --- | --- |
| `clip_id` | integer | Unique timeline clip id (not a node id). |
| `node` | integer | Graph node id/array index of the source. |
| `start_frame` | integer | Timeline position. |
| `duration_frames` | integer | Length in sequence frames; 0 is removed by repair. |
| `source_trim_in` | integer | Source frame offset. |
| `speed` | number | Source rate multiplier, clamped 0.1..10 when compiled. |

`end_frame` and `source_frames` are derived, not stored.

## Repair rules

There are two repair paths. They are **not identical**.

### Startup (`App::new`)

1. Read `graph.json`; if reading or parsing fails, use `demo_graph()` silently.
2. `graph.timeline.repair(graph.nodes.len())`:
   - drop clips whose `node` is out of range or whose `duration_frames == 0`;
   - `duration = max(duration, 1)`;
   - `playhead = min(playhead, duration - 1)`;
   - clear `loop_range` if `in >= out` or `out > duration`;
   - clear a selection that no longer resolves.

**Startup does not renumber node ids or drop out-of-range links.** A hand-edited
file with mismatched ids can behave unpredictably (the canvas and compiler index
`nodes` by `id`, while some mutations treat `id` as an array position). Save and
reload through the app, or use **Load graph**, after hand-editing.

### Load button (`App::load_graph`)

1. Read and parse.
2. Set every `node.id` to its array index.
3. Retain only links whose `from` and `to` are `< nodes.len()`.
4. `timeline.repair(nodes.len())` as above.
5. Reset selection, pending wire, drag state and preview texture; schedule a
   recompile.

A parse error is reported in the status line and the current graph is left
untouched.

## Example: minimal document

Two seconds of a video (as a timeline clip) letterboxed to 720p:

```json
{
  "nodes": [
    {
      "id": 0,
      "kind": { "Input": { "path": "/media/clip.mp4" } },
      "pos": [60.0, 80.0]
    },
    {
      "id": 1,
      "kind": { "Output": { "width": 1280, "height": 720 } },
      "pos": [320.0, 80.0]
    }
  ],
  "links": [
    { "from": 0, "from_port": 0, "to": 1, "to_port": 0 }
  ],
  "timeline": {
    "playhead": 15,
    "timebase": 30,
    "duration": 60,
    "zoom": 60.0,
    "scroll_offset": 0.0,
    "playback_state": "Paused",
    "loop_range": [0, 60],
    "tracks": [
      {
        "id": 0,
        "name": "Video 1",
        "kind": "Video",
        "clips": [
          {
            "clip_id": 1,
            "node": 0,
            "start_frame": 0,
            "duration_frames": 60,
            "source_trim_in": 0,
            "speed": 1.0
          }
        ]
      },
      { "id": 1, "name": "Audio 1", "kind": "Audio", "clips": [] }
    ],
    "next_clip_id": 2,
    "selected": null,
    "snapping": true
  }
}
```

This compiles the Input with:

```text
trim=start=0.000000:duration=2.000000,setpts=(PTS-STARTPTS)/1.0000+0.000000/TB
```

## Compatibility notes

- **Adding fields to an existing `NodeKind` variant breaks old files** for that
  variant; there is no field defaulting.
- **Adding a new variant** keeps old files loadable. New files are not loadable
  by older binaries (unknown variant error).
- **Renaming a variant or field** breaks every file containing it.
- `timeline` was added after the first graph format and is the only field with
  `#[serde(default)]`. Pre-timeline files therefore still load.
- There is no explicit version. If the format evolves, consider adding one before
  changing existing shapes.
- Because startup falls back to the demo on any parse error, a corrupt file can
  look like “my graph vanished” rather than an error. Use **Load graph** to see
  the actual parse error without restarting.

## Safe hand-editing recipe

1. Save from the app and quit (or copy `graph.json`).
2. Keep every `id` equal to its array index; keep links within range.
3. Keep one incoming link per input port. Do not duplicate links unless you know
   the last-wins rule.
4. Do not introduce cycles into the reachable graph.
5. Leave the Output node with exactly one incoming link.
6. Use only the enum strings listed above.
7. Use forward slashes or JSON-escaped backslashes in paths. JSON treats `\` as
   an escape, so a Windows path needs `\\`.
8. Load through the app's **Load graph** button once, then save, to normalise ids
   and repair the timeline.
9. Do not edit the file while the app is running and then press Save: the app
   writes its in-memory graph and overwrites your file.
