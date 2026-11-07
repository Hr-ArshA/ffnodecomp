# Timeline

FFNodeComp has a frame-accurate, non-linear timeline docked at the bottom of the
window. It is not a real-time playback engine: the playhead drives which single
frame FFmpeg extracts, and playback is best-effort wall-clock stepping.

The source of truth is `src/timeline/mod.rs`, plus the `timeline_view` and
`timeline` methods in the UI. The timeline is stored inside the `Graph` and is
saved to and loaded from `graph.json` with the nodes.

## Why frames

Every timeline quantity is an integer frame count (`FrameIndex(u64)`), never
accumulated floating-point seconds:

- scrubbing, stepping and looping cannot drift;
- clip edges always land on an exact frame;
- the timecode is derived without rounding error.

Seconds only appear at the FFmpeg boundary (`trim`/`setpts`/`-ss`), converted as
`frame / fps`.

## Data model

```rust
struct TimelineState {
    playhead: FrameIndex,          // 0..duration-1
    timebase: u32,                 // sequence fps, default 30
    duration: u64,                 // total frames, default 300
    zoom: f32,                     // pixels per second, default 60
    scroll_offset: f32,            // leftmost visible frame, default 0
    playback_state: PlaybackState, // Paused | Playing | Scrubbing
    loop_range: Option<(FrameIndex, FrameIndex)>, // in, exclusive out
    tracks: Vec<Track>,
    next_clip_id: u64,             // default 1
    selected: Option<(TrackId, u64)>, // (track id, clip id)
    snapping: bool,                // default true
}
```

`TimelineState::default()` creates:

- **30 fps**, **300 frames** (10 seconds), 60 px/s zoom, scroll 0;
- one **video** track `Video 1` and one **audio** track `Audio 1`, both empty;
- snapping on, no selection, no loop, paused at frame 0.

A `Track` has an `id`, a display `name`, a `kind` (`Video` or `Audio`), and
`Vec<Clip>`. Only video tracks receive auto-created clips (audio tracks are cleared on sync). The UI currently has no controls to
add, remove or rename tracks; a hand-edited `graph.json` can add them.

A `Clip`:

| Field | Meaning |
| --- | --- |
| `clip_id` | Unique across the timeline (allocated by `next_clip_id`). |
| `node: NodeId` | Index of the graph source node this clip displays. |
| `start_frame` | Timeline position of the clip's first frame. |
| `duration_frames` | Length on the timeline, in sequence frames. |
| `source_trim_in` | Offset into the source asset, in source frames. |
| `speed: f32` | Playback rate multiplier, clamped to `0.1..=10.0` when used. |

Derived:

- `end_frame() = start_frame + duration_frames` (exclusive);
- `source_frames() = round(duration_frames * clamp(speed, 0.1, 10.0))` – how many
  source frames the clip consumes.

Auto-created clips always use `start_frame = 0`, `source_trim_in = 0`,
`speed = 1.0`, and a natural duration.

Timecode (`HH:MM:SS:FF`):

- fps = `timebase.max(1)`;
- hours are modulo 24; each component is a `u8`;
- 60 fps and above would still fit `frames` in a `u8` at these speed limits.

## How clips are created

The timeline only knows about sources the compiler can trim:

- **Input** – 5 seconds (a fixed guess, not probed) → `5 * fps` frames.
- **Image** – its configured `duration` → `round(duration * fps)` frames.
- Everything else (Solid Color, Text, filters) – no clip.

Both lengths are at least one frame. The Input guess exists because the UI does
not probe a file for its real duration; use the clip's right handle to match the
actual length.

On startup, `graph.json`, **Sync clips**, adding a node, and the demo-graph
builder all call `sync_timeline_clips`:

1. collect `(node id, natural frames)` for every source node;
2. on each **video** track, drop clips whose node no longer exists;
3. add a clip at frame 0 for every source that has none on that track;
4. clear every **audio** track.

Consequences:

- Sync creates *missing* clips and removes *orphans*; it does not resize or
  reposition existing clips. After changing an Image's duration, delete/recreate
  its clip (or edit the clip's right edge) if you want the new length.
- Deleting a source clip is respected until you press **Sync clips** again.
- Deleting a node from the graph also removes its clip immediately
  (`on_node_removed`), and decrements the `node` index of clips that pointed to
  later nodes.
- Pressing **Sync clips** with several sources stacks all new clips at frame 0.
  Move them with the body handle.

## Editing clips

The ruler shows a tick every whole second (the step doubles until labels fit),
plus `HH:MM:SS:FF` labels when zoomed to ≥ 40 px/s, otherwise `Ns`.

Each clip is drawn as a coloured block 52 px high on its lane:

- video clips use the video-orange border and a dark orange fill;
- audio clips use green;
- the selected clip gets a white border;
- 7 px handles at the left and right edges are the trim grips.

All times are converted with `pixels_per_frame = zoom / fps`, clamped to at
least 0.001.

| Drag | Effect |
| --- | --- |
| Clip body | Moves `start_frame`. Clamped to ≥ 0. |
| Left handle | **Trim in**: moves the clip start and its source offset together. `source_trim_in` changes by the same delta, clamped to ≥ 0; duration changes inversely. Cannot go below one frame. |
| Right handle | **Trim out**: moves the end. Cannot go left of `start_frame + 1`. Extending is unbounded. |

The exact model:

- Body: `start_frame = max(0, start + delta)` (then snapped).
- Trim in: `delta = clamp(delta, -source_trim_in, duration_frames - 1)`;
  `start += delta`, `source_trim_in += delta`, `duration -= delta`, all clamped
  to sane values.
- Trim out: `end = max(start + 1, end + delta)`;
  `duration = end - start`.

`duration` (the sequence length) only ever grows to the furthest clip end; it
does not shrink when clips are moved left.

### Snapping

Snapping is on by default (`Snap` checkbox). While dragging a clip:

- **Anchors:** the playhead, and the start and end of every *other* clip (on all
  tracks).
- **Grid:** the nearest whole-second boundary (floor and ceiling).
- The magnet radius is `ceil(8 / pixels_per_frame)` frames, i.e. about 8 px.
- The closest candidate within the radius wins; otherwise the unsnapped frame
  is used.

Body drags snap the new start. Trim-in snaps `start_frame` and then applies the
actual delta to the source offset. Trim-out snaps the new end. Snapping can be
toggled off for frame-exact positioning.

### Selecting

Pressing anywhere in a clip body selects `(track id, clip id)`. Clip dragging
sets the same selection. A selection is dropped by `repair`/`repair_selection`
when its track or clip no longer exists. The selection is part of the saved
document.

## Playhead and zoom

- Click or drag anywhere on the ruler to scrub. This sets
  `playback_state = Scrubbing`, moves the playhead, cancels the in-flight preview
  and schedules a new frame on the 33 ms scrub debounce.
- Releasing the ruler leaves playback `Paused`.
- `set_playhead(frame)` clamps to `0..=duration-1`; duration is at least 1, so the
  playhead never points past the sequence.
- The playhead is drawn on top of clips with a handle in the ruler.

Zoom and pan:

- **–** multiplies zoom by 0.8, **+** by 1.25, clamped to **8..400 px/s**.
- `Ctrl`/`⌘` + wheel zooms in that range (factor
  `(1 + scroll_y * 0.002).clamp(0.75, 1.33)`); plain wheel pans horizontally.
- Pan is clamped to `0..duration`, in frames.
- Wheel handling ignores the track-header column; it requires hovering the
  timeline body.

The toolbar's fps selector offers **24, 25, 30, 50, 60**. The timebase is not
rescaled when changed: frame 120 stays frame 120, so the clip's wall-clock
position and length change. The compiler uses the new fps on the next refresh.
Zoom, scroll, fps, selection, snapping and the loop range are all persisted in
`graph.json`.

## Transport

The transport is at the top of the timeline panel.

| Button / key | Action |
| --- | --- |
| **Play / Pause**, `Space` | Toggle playback. Playback is best-effort real time. |
| **Start**, `Home` | Pause, playhead to frame 0, immediate preview refresh. |
| **<** / `←` | Pause, step −1 frame (clamped to 0). |
| **>** / `→` | Pause, step +1 frame (clamped to duration−1). |
| **Set In**, `I` | Set loop in point at the playhead. |
| **Set Out**, `O` | Set loop out point at the playhead (exclusive). |
| **Clear loop** | Remove the loop range (appears only when one exists). |
| Timecode | Current playhead, `HH:MM:SS:FF`. |
| **Sync clips** | Rebuild missing/orphan clips as above. |

Keys are ignored while any text widget has keyboard focus
(`egui_wants_keyboard_input()`), so typing in the inspector or path fields does
not trigger transport actions. Playback also requests repaints every 16 ms while
running.

### Loop range

- `Set In` at frame `p`: keeps an existing out point if it is later than `p`,
  otherwise sets out to `duration.max(p + 1)`.
- `Set Out` at frame `p`: keeps an existing in point if it is earlier than `p`,
  otherwise uses 0. It is rejected if `in >= p`, so the out point is always
  strictly later than the in point and frame 0 cannot be an out point.
- **Out is exclusive.** Playing a loop `(10, 40)` shows frames 10..39, then jumps
  back to 10.
- On load/repair a loop is dropped if `in >= out` or `out > duration`.
- The loop only affects **playback**. Scrubbing may move the playhead outside it.

### Playback clock

Playback uses a wall-clock anchor, `(Instant::now(), frame_at_start)`, not a frame
counter:

```
frame = start_frame + floor(elapsed_seconds * fps)
```

- If the clock reaches `duration`, the playhead is clamped to `duration - 1` and
  playback pauses.
- With a loop range, reaching the exclusive out point jumps to the in point and
  resets the anchor.
- Preview extraction may be slower than real time. Playback does not wait for
  it: the playhead updates continuously, and frame seeks use the 33 ms debounce
  without issuing an immediate cancel (unlike scrubbing). However, once each
  debounced refresh arrives, requesting a new frame aborts the still-running
  task, so slower extractions can stall or drop frames.

## Timeline ↔ preview coupling

- The viewer's `time (s)` slider, the timeline ruler and playback all seek the
  same preview clock. Moving the viewer slider pauses playback and moves the
  playhead to the nearest frame.
- Scrubbing cancels the current FFmpeg process immediately, increments the graph
  generation, and re-plans after **33 ms** of quiet.
- Structural/parameter edits use the **80 ms** graph-edit debounce.
- The preview worker tags each result with the generation it was requested for;
  the UI discards results whose generation is no longer current.
- `ctx.request_repaint_after(16 ms)` runs while a dirty/scrub timer is pending, so
  a scrub scheduled late in a frame still renders promptly.

## Compilation

A clip only affects the graph if its node is an `Input` or `Image` **and** that
node is reachable from the compiled Output. Clips on Text, Solid Color, filters
or unreachable nodes are ignored.

For a reachable `Input`/`Image` with a clip, the compiler appends:

```text
,trim=start=<source_trim_in/fps>:duration=<source_frames/fps>,
 setpts=(PTS-STARTPTS)/<speed>+<start_frame/fps>/TB
```

with:

- `fps = timeline.timebase.max(1)`;
- `source_frames = round(duration_frames * clamp(speed, 0.1, 10.0))`;
- six decimals for trim/start, four for speed.

Interpretation:

1. `trim` selects `source_frames` frames from the source, starting at
   `source_trim_in`.
2. `setpts` divides the remaining timestamps by `speed`, which is the inverse of
   slow motion (`speed = 2` plays twice as fast). This makes the clip consume
   `source_frames / speed ≈ duration_frames` output frames.
3. Adding `start_frame/fps` shifts the clip to its timeline position in
   sequence time.
4. `(PTS-STARTPTS)` rebases timestamps to zero before the offset.

Examples (fps = 30):

| Clip | Filter suffix |
| --- | --- |
| start 30, duration 60, trim 15, speed 2 | `trim=start=0.500000:duration=4.000000,setpts=(PTS-STARTPTS)/2.0000+1.000000/TB` |
| start 0, duration 90, trim 0, speed 1 | `trim=start=0.000000:duration=3.000000,setpts=(PTS-STARTPTS)/1.0000+0.000000/TB` |

Nodes without a clip are byte-identical to the pre-timeline output: no `trim`,
no `setpts`.

The UI does not expose a speed control; new clips are always `speed = 1.0`. A
hand-edited `graph.json` can set it, and the compiler honours it clamped to
0.1..10.

### Preview seeking

`Compiled::timeline_active` is true when at least one **reachable Input or Image**
has a clip. This is a global switch:

- **No timeline clip:** preview seeks on the **input** (`-ss <t>` before
  `-i <path>`). This is the fast path.
- **Any timeline clip:** preview seeks on the **output**, appending
  `-ss <t>` after `-filter_complex`. Input-side seek is disabled for every input,
  so `t` means sequence time and the `setpts` offsets are respected.

Export never passes `-ss`; the whole re-timed graph is rendered.

## Limits of this timeline

- **Not a compositor across tracks.** Multiple clips all feed their own source
  nodes; the graph itself (Overlay etc.) decides composition. Clips on the same
  track do not replace each other during playback.
- **One clip per source is expected.** The compiler looks up the *first* clip for
  a node across tracks; extra clips for the same node are not applied.
- **No audio clips.** Audio track(s) are cleared by sync and never rendered.
  Export may still pass through the first input's audio (see the
  [User guide](user-guide.md#exporting)).
- **Input duration is a 5 s guess.** No `ffprobe` call feeds the timeline.
- **Timebase changes do not rescale frames.**
- **Gaps have no frame.** If a delayed clip is the only thing covering the
  current time, the preview may return no output (FFmpeg reports fewer bytes than
  expected), and export starts at the clip's own PTS.
- **Duration does not shrink** when clips are moved earlier or shortened below
  the furthest end.
- **No speed control UI**, though the data and compiler support it.
- **Dragging is quantised to whole frames** and recompiles on the scrub debounce;
  very large projects will feel the FFmpeg cost, not the maths.
