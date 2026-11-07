# Compilation

`src/graph/compiler.rs` turns a `Graph` into FFmpeg arguments. It is a pure,
deterministic function: the same nodes, links, Output and timeline always produce
the same filtergraph and argv. It never runs FFmpeg and never touches egui types.

```rust
pub fn compile(graph: &Graph) -> Result<Compiled, String>;

pub struct Compiled {
    pub inputs: Vec<InputSpec>,     // one per reachable file/image source
    pub filter_complex: String,    // no trailing semicolon
    pub timeline_active: bool,     // any reachable Input/Image has a clip
}

impl Compiled {
    pub fn preview_args(&self, t: f32, accel: Accel) -> Vec<String>;
    pub fn export_args(&self, out: &str, accel: Accel) -> Vec<String>;
}
```

## Stages

1. **Pick the Output.** The first node whose kind is `Output`, in node order.
   No Output → `graph needs an Output node`.
2. **Canvas.** Take the Output's width/height, force each even and clamp to
   `[2, 7680]` (`value.max(2) & !1`, min 7680).
3. **Reachability.** Walk incoming links backwards from the Output and collect
   every node id. Nodes that cannot reach the Output are ignored completely,
   including compile errors about their missing files. The traversal also
   tolerates links that point at a nonexistent source id, which are rejected in
   the next stage.
4. **Validate visible links.** For every link whose endpoints are both
   reachable:
   - both nodes must exist (`link references missing node N`);
   - the source node must have that output port;
   - the target node must have that input port;
   - the port kinds must be compatible (`Video → Mask` is allowed, everything
     else needs equality).
5. **Topological order.** Kahn's algorithm over the reachable subgraph, with
   ready nodes kept in a `BTreeSet`, so ties are broken by ascending node id.
   The result is deterministic. If not all reachable nodes are emitted:
   `graph contains a cycle` (normally prevented by the UI).
6. **FFmpeg inputs.** Walk the graph in node order. Every reachable `Input` and
   `Image` becomes an `InputSpec` and gets an FFmpeg input index (`0`, `1`, …).
   - `Input`: path, no extra options, decode allowed.
   - `Image`: path, `-loop 1 -framerate <fps> -t <duration>` before `-i`, and
     **no** hardware decode flags.
   - Other sources (Solid Color, Text) are lavfi filters and add no input.
   An empty `Input`/`Image` path is a hard error.
   The FFmpeg input index order is node order, **not** topological order.
7. **Fan-out bookkeeping.** For every input port, the **last** link in
   `graph.links` wins (the UI keeps only one). Only winning links are considered
   active outputs. Each active link gets a label:

   ```text
   [n<source-node>p<source-port>_<fanout-index>]
   ```

   `fanout-index` counts active links from that node, in link order.
8. **Emit one filter statement per topological node.** Gather the labels for
   its connected inputs (or `[i:v]` for a file source), select the filter by kind,
   and emit into `filter_complex` with `emit()`:

   ```text
   <in-label>...<filter><out-label>...;
   ```

   Optional unconnected inputs are skipped. Required unconnected inputs abort
   with `<Title> node #N is missing its '<port>' input`. The Output node emits the
   letterbox chain and the final `[vout]` label and terminates.
9. **No dead ends.** Every non-Output node must have at least one active outgoing
   link; otherwise compilation fails with `"<Title> node #N does not feed the
   output"`. This is reachable in hand-edited graphs where a duplicate link
   loses the "last wins" race.
10. **Trailing separator.** The accumulated `;` separators are trimmed from the
    end. The filtergraph is a single argv element, never shell text.

If no reachable Output was emitted, compilation fails with
`could not reach an Output node` (defensive; stage 1 normally catches this).

### Multiple Outputs and duplicate links

- Only the first `Output` node is compiled. Others are neither a root nor
  emitted, and the palette disables adding a second one.
- Only one link per input port is active. If a hand-edited file has two, the
  later link in `links` is used. The earlier source may then be reported as
  "does not feed the output" instead of the duplicate being silently ignored.

## The RGBA pipeline

Every stream is normalised to `rgba` while it travels through the graph:

- `Input`: `format=rgba,setsar=1`
- `Image`: `format=rgba,setsar=1`
- `Solid Color` / Text source: end with `format=rgba,setsar=1`
- filters: wrap themselves in `format=rgba,…,format=rgba` where the filter
  could change the format
- `Output`: starts with `format=rgba` and pads with opaque black

This lets the preview and export tails assume one pixel format and keep alpha
available for Overlay/Alpha Key. It costs conversions if a filter chain is all
YUV, but keeps correctness and debuggability for an MVP.

## Per-node emission

Each node contributes exactly one filter string. The full mappings live in
[Nodes](nodes.md). Shape notes:

- `Overlay` has two incoming labels, in port order: background then foreground.
- `Alpha Key` with a mask first emits a separate `scale2ref` statement, then the
  `alphamerge` statement. The `scale2ref` input order is `mask, video`; the
  `alphamerge` order is `video, mask`.
- `Output` emits its letterbox filter and `[vout]` and stops.
- Everything else consumes one video label and produces one.

## Fan-out and `split`

An FFmpeg label may only be consumed once. If a node's output feeds more than one
active input:

```text
[in]<node filter>[n5tmp];
[n5tmp]split=2[out1][out2];
```

Single consumer:

```text
[in]<node filter>[n5p0_0];
```

Zero consumers on a non-Output node is an error. `split` copies are independent
streams; downstream filters do not share state.

## Worked example

Graph (from `demo_graph()`):

```text
Input #0 → Transform #1 → Color #2 → Output #3 (1280×720)
```

`Input` has no file in the demo, so compilation reports
`Input node #0 has no file`. Set `/tmp/clip.mp4`, then the emitted graph is:

```text
[0:v]format=rgba,setsar=1[n0p0_0];
[n0p0_0]scale=1280:720:flags=bicubic,pad=max(iw\,1280):max(ih\,720):0:0:color=black@0,setsar=1[n1p0_0];
[n1p0_0]eq=brightness=0.000:contrast=1.000:saturation=1.000,format=rgba[n2p0_0];
[n2p0_0]format=rgba,scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1[vout]
```

The preview tail then appends:

```text
[vout]scale=640:360:force_original_aspect_ratio=decrease,pad=640:360:(ow-iw)/2:(oh-ih)/2:color=black,format=rgba[preview]
```

Note: labels use each link's *source* node and *source port*; the `Transform`
statement's output label includes `n1` because that statement is the node whose
result is being labelled.

## Argument assembly

### Common input args

```text
-hide_banner -loglevel error -nostdin
[-vaapi_device /dev/dri/renderD128]   # Vaapi only
per input (in order):
  [-ss <seconds>]                     # preview only, no timeline
  <extra options>                     # Image: -loop 1 -framerate N -t D
  [-hwaccel cuda|videotoolbox|vaapi]  # decode: true only
  -i <path>
```

Decode flags (`Accel::decode_flags`):

| Acceleration | Flags |
| --- | --- |
| CPU | none |
| CUDA | `-hwaccel cuda` |
| VideoToolbox | `-hwaccel videotoolbox` |
| VAAPI | `-hwaccel vaapi` |

Hardware decode is a **decode-only** optimisation. The app deliberately never
adds `-hwaccel_output_format`, so frames are downloaded to system memory and all
filters run on the CPU. This keeps `eq`, `pad`, `overlay`, `drawtext` and the new
colour filters working without CUDA/VAAPI filter variants. The filtergraph is
not GPU-accelerated.

Still images skip decode flags because `-loop 1` belongs to the image demuxer
and hardware decoding of a still adds failure modes for no benefit.

### Preview

```text
<common input args>                          # -ss before inputs if no timeline clip
-filter_complex "<compiled graph>;
  [vout]scale=640:360:force_original_aspect_ratio=decrease,
         pad=640:360:(ow-iw)/2:(oh-ih)/2:color=black,
         format=rgba[preview]"
-map [preview]
[-ss <t>]                                   # only if timeline_active, after the graph
-frames:v 1
-f rawvideo
-pix_fmt rgba
pipe:1
```

- Output is exactly `640 * 360 * 4 = 921600` bytes when successful.
- Source aspect ratio is preserved; unused space is black. The Output node has
  already letterboxed into its own aspect; the preview adds a second fit to the
  fixed viewport.
- If more than 921600 bytes arrive, the worker truncates; if fewer arrive, it
  reports an error including the byte count and hints at the timestamp being
  past the clip end.
- Preview never writes a file and never carries audio.

### Export

```text
<common input args>                          # never -ss
-filter_complex "<graph>[;hwupload tail]"
-map <[vout] | [vout_hw]>
<encoder args>
[-pix_fmt yuv420p]                           # omitted for VAAPI
[-map 0:a? -c:a aac]                         # only if inputs is non-empty
-y <out>
```

Encoder table:

| Accel | Map label | Encoder args |
| --- | --- | --- |
| CPU | `[vout]` | `-c:v libx264 -preset veryfast -crf 23` |
| CUDA | `[vout]` | `-c:v h264_nvenc -preset p4 -cq 23` |
| VideoToolbox | `[vout]` | `-c:v h264_videotoolbox -b:v 6M` |
| VAAPI | `[vout_hw]` | `-c:v h264_vaapi -qp 23` |

VAAPI additionally appends `;[vout]format=nv12,hwupload[vout_hw]` to the
filtergraph and omits `-pix_fmt yuv420p` (the encoder wants a hardware surface,
not system-memory yuv420p). The other encoders accept downloaded frames.

Audio: `-map 0:a?` maps all audio streams of **FFmpeg input 0** only, transcoded
to AAC. The trailing `?` makes an absent audio stream non-fatal. `0` is the first
reachable `Input`/`Image` in node order; if the first input is an image and a
second input is a video with sound, that sound is **not** included. There is no
audio mixing or timeline audio. This is called out in
[Troubleshooting](troubleshooting.md#exported-file-has-no-audio).

## Escaping

Two independent escaping problems exist: filtergraph option parsing (FFmpeg)
and shell display (the command inspector).

### LUT path escaping

`escape_lut_path` exists because `lut3d=file=...` is parsed twice: once by the
filtergraph tokenizer and once by the filter's option parser. Both consume
backslashes. The output is inserted **unquoted**:

```text
lut3d=file=<escaped>
```

| Input char | Emitted |
| --- | --- |
| `\` | four backslashes |
| `'` | three backslashes + `'` |
| `:` | two backslashes + `:` |
| `,` | `\` + `,` |
| `[` | `\[` |
| `]` | `\]` |
| `;` | `\;` |
| space | space, literal |
| `=` | literal |
| anything else | literal |

The emitted quote/apostrophe form is deliberately *raw*, not a shell quote: the
graph is one argv element, so there is no shell to satisfy. Spaces work because
the option value is not tokenised on whitespace. `,`, `[`, `]` and `;` are
filtergraph separators; `:` separates options; `\` is the escape itself.

Unit coverage: `lut_path_escapes_filtergraph_separators`. A real-FFmpeg test
(`color_nodes_render_with_real_ffmpeg`) writes a LUT into a temp path whose name
contains a space, an apostrophe and a colon, compiles and renders, and asserts
921600 bytes.

### Text escaping

`drawtext` uses a different scheme; see
[Nodes § Text](nodes.md#text). Summary: wrap in single quotes, double the quote,
double backslashes, and backslash-escape `:` and `%`; add `expansion=none`.
Custom X/Y expressions use an allow-list instead of escaping and fall back to
centering if anything suspicious is present.

### Command-line display quoting

`command_line()` is used only by the preview panel. Each argument that contains
anything outside

```text
A-Z a-z 0-9 - _ . / : = , [ ] @
```

is wrapped in single quotes, with every `'` replaced by `'\''`. Empty arguments
become `''`. The result is POSIX-shell-pasteable; it is not what the process
executes (the process gets the raw argv).

## Determinism and byte stability

- Ready nodes are taken from a `BTreeSet`, so topological order is stable.
- FFmpeg input indices follow node order, not hash order.
- Link labels include the source node/port and a fan-out counter derived from
  link order.
- Every floating value is formatted with explicit precision (`.3`, `.4`, `.6`,
  or two decimals for blur).
- A graph with no timeline clips gets no `trim`/`setpts`; the compiler output is
  unchanged from before the timeline feature.

This matters for tests, for comparing the command inspector across sessions, and
for re-rendering identical exports.

## Errors

`compile` returns `Result<Compiled, String>`. Messages are short and shown in the
preview panel prefixed with `Graph:`.

| Message | Cause |
| --- | --- |
| `graph needs an Output node` | No `Output` kind exists. |
| `Output node is malformed` | Internal inconsistency; Output disappeared. |
| `link references missing node N` | A reachable link's endpoint id is out of range. |
| `<Title> has no output P` | `from_port` out of range. |
| `<Title> has no input P` | `to_port` out of range. |
| `<A> output is Video but <B> input N is Mask` | Incompatible kinds. |
| `graph contains a cycle` | Hand-edited cycle. |
| `Input node #N has no file` | Empty/whitespace path, reachable. |
| `Image node #N has no file` | Empty/whitespace path, reachable. |
| `<Title> node #N is missing its '<port>' input` | Required port unconnected. |
| `<Title> node #N does not feed the output` | Reachable node with no active outgoing link. |
| `could not reach an Output node` | Defensive fallback. |
| `internal: Output reached the filter arm` | Impossible unless dispatch is broken. |

Note what is *not* an error:

- a LUT node with an empty path (identity pass);
- unreachable nodes, no matter how broken;
- a Text node with no font file (FFmpeg default);
- a `.cube` file that does not exist (FFmpeg error at render time, not compile);
- more than one Output (later ones ignored);
- a missing `ffmpeg` binary (discovered when spawning).

## Preview vs export: same graph, different tails

| | Preview | Export |
| --- | --- | --- |
| Seek | input `-ss t`, or output `-ss t` if timeline active | none |
| Filter tail | fit/pad to 640×360, RGBA | none; graph already ends at canvas size |
| Map | `[preview]` | `[vout]` / `[vout_hw]` |
| Encoder | none (`-f rawvideo`) | H.264 hardware or software |
| Pixel format | `rgba` | `yuv420p`, except VAAPI surfaces |
| Audio | none | optional first input audio → AAC |
| Duration | `-frames:v 1` | until inputs end |
| Process | Tokio task, cancellable | dedicated thread, progress channel |

Because the preview reuses the same `filter_complex` and input specs, a graph
that previews successfully is very likely to export successfully. The
differences are the encoder, audio mapping and (for VAAPI) the upload tail.
