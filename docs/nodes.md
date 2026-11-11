# Nodes Reference

Every effect is a node in the graph. A node holds plain Rust data (no egui types)
and serialises to `graph.json`; the compiler turns it into one or more FFmpeg
filters. This page lists ports, fields, ranges, defaults, and the exact filters
emitted.

For the pipeline that assembles these filters, see
[Compilation](compilation.md). For the UI that edits them, see the
[User guide](user-guide.md).

## Ports

- Every node has **one video output** except `Output` (none) and the three mask
  nodes (`MatteBlur`, `MaskCombine`, `ShapeMask`), which output a single `Mask`.
- A port is `Video`, `Mask`, or `Audio`.
- `Video → Mask` is allowed. Every other connection requires equal kinds.
- All inputs are **single-source**. Connecting a second wire replaces the first.
- `Audio` is defined in the type system but no node currently produces or accepts
  audio on a port. The timeline's audio track is a placeholder.
- Outputs fan out freely. The compiler inserts an FFmpeg `split` when one output
  feeds more than one consumer (see [Compilation](compilation.md#fan-out-and-split)).

| Node | Inputs | Output |
| --- | --- | --- |
| Input | none | video |
| Image | none | video |
| Solid Color | none | video |
| Text | `video (opt)` | video |
| Transform | `video` | video |
| Color Correction | `video` | video |
| Overlay | `background`, `foreground` | video |
| Chroma Key | `video` | video |
| Alpha Key | `video`, `mask (opt)` | video |
| Blur | `video` | video |
| Sharpen | `video` | video |
| 3D LUT | `video` | video |
| Color Balance | `video` | video |
| Levels | `video` | video |
| Invert / Solarize | `video` | video |
| Matte Blur / Erode / Dilate | `mask` | mask |
| Mask Combine | `mask a`, `mask b` | mask |
| Shape Mask | none | mask |
| Output | `video` | none |

## Palette defaults

These are the values created by the **Add node** buttons.

| Node | Defaults |
| --- | --- |
| Input | empty path |
| Transform | 1280×720, position 0,0 |
| Color Correction | brightness `0.0`, contrast `1.0`, saturation `1.0` |
| Overlay | offset 0,0 |
| Output | 1280×720 |
| Chroma Key | colour green `#00FF00`, similarity `0.3`, blend `0.1` |
| Alpha Key | Luma key, threshold `0.5`, tolerance `0.1`, softness `0.1` |
| Text | `"Title"`, size 36, white, system font, centre, duration 5 s |
| Image | empty path, duration 5 s, framerate 30 |
| Solid Color | `#202020`, 1280×720, 5 s, 30 fps |
| Blur | horizontal `3.0`, vertical `3.0`, steps `1` |
| Sharpen | amount `1.0`, clarity `0.0`, matrix 5×5 |
| 3D LUT | empty path (identity) |
| Color Balance | all nine values `0.0` |
| Levels | black `0.0`, white `1.0`, gamma `1.0` per channel |
| Invert / Solarize | invert `0.0`, solarize `0.0`, threshold `0.5` |
| Matte Blur | blur `3.0`, erode `0`, dilate `0` |
| Mask Combine | Multiply (AND) |
| Shape Mask | rectangle 640×360 at 0,0, feather `20`, not inverted, 5 s |

---

## Sources

### Input

External video file, decoded as its first video stream.

- **Inputs:** none. **Output:** `video`.
- **Field:** `path: String`.

Compiler mapping: the file becomes an FFmpeg `-i <path>` input (in node order
among reachable sources) with decode flags for the selected acceleration, then:

```text
[<n>:v]format=rgba,setsar=1[<label>]
```

If the node has a timeline clip, a trim/re-time suffix is appended (see
[Timeline](timeline.md#compilation)).

Limits / notes:

- An empty or whitespace-only path is a compile error:
  `Input node #N has no file`.
- The path is passed as a single argv element. No shell quoting is needed for
  spaces; `graph.json` is JSON-escaped.
- Only the first video stream is consumed by the filtergraph. Container support
  is whatever your FFmpeg build has.
- `ffprobe` is not used to discover the duration for the timeline; a clip is
  seeded at 5 s. Export duration is probed only for progress display.

### Image

A still image looped into a video stream.

- **Inputs:** none. **Output:** `video`.
- **Fields:** `path: String`, `duration: f32`, `framerate: u32`.

Compiler input options (before `-i`):

```text
-loop 1 -framerate <fps> -t <duration>
```

with `framerate` clamped to `1..=120` and `duration` to `0.1..=600.0`, written
with three decimals. Hardware decode flags are **not** applied to image inputs.
The filter is:

```text
format=rgba,setsar=1
```

plus the timeline gate when a clip exists.

Notes:

- The file is probed by FFmpeg's image demuxer; one path is one image.
- Empty path is a compile error: `Image node #N has no file`.
- Images can be looped as long as the graph needs them, but they never supply
  audio.

### Solid Color

Generated background; no external file and no FFmpeg input index.

- **Inputs:** none. **Output:** `video`.
- **Fields:** `color: [u8; 3]` (sRGB), `width: u32`, `height: u32`,
  `duration: f32`, `framerate: u32`.

Mapping:

```text
color=c=0xRRGGBB:s=<evenW>x<evenH>:d=<duration>:r=<fps>,format=rgba,setsar=1
```

- width/height are max(`2`, forced even), capped at 7680.
- duration is clamped to `0.1..=600.0`, three decimals.
- framerate is clamped to `1..=120`.
- The colour is opaque. Transparency comes from other nodes or Transform
  padding.

The node is not an ffmpeg input; it starts a `lavfi` filter chain. It is not
auto-placed on the timeline and is never timeline-gated.

### Text in source mode

`Text` with its optional `video` input **unconnected** generates its own canvas:

```text
color=c=black@0.0:s=<W>x<H>:d=<duration>:r=30,drawtext=...,format=rgba,setsar=1
```

`W`/`H` are the Output canvas dimensions, forced even and clamped to `7680`,
`duration` to `0.1..=600.0`, and the framerate is fixed at **30 fps** (the
timeline timebase is not used). The canvas is transparent, so compositing it over
something else shows only the glyphs.

See [Text](#text) below for the `drawtext` parameters and escaping.

### Text

- **Input:** `video (opt)`.
- **Output:** `video`.
- **Fields:**

| Field | Type | UI range | Behaviour |
| --- | --- | --- | --- |
| `text` | string | multiline | Literal text; `expansion=none` disables `%{...}` expansion. |
| `font_size` | f32 | 6..400 | Rounded, clamped to 6..400 in the filter. |
| `font_color` | [u8;3] | colour picker | Emitted as `0xRRGGBB`. |
| `font_file` | string | text + Browse (`ttf, otf, woff, woff2`) | Empty = FFmpeg default font. Inserted as `fontfile='...'` first. |
| `position` | enum | `Center`, `Top center`, `Bottom center`, `Custom X/Y` | Position expression. |
| `custom_x` | string | text | Only used by Custom X/Y. |
| `custom_y` | string | text | Only used by Custom X/Y. |
| `duration` | f32 | 0.1..600 s | Used **only** when there is no video input (source mode). |

Position expressions:

| Position | x | y |
| --- | --- | --- |
| Center | `(w-text_w)/2` | `(h-text_h)/2` |
| Top center | `(w-text_w)/2` | `24` |
| Bottom center | `(w-text_w)/2` | `h-text_h-24` |
| Custom X/Y | sanitised input, else centre | sanitised input, else centre |

Custom expressions may contain ASCII letters, digits, `+ - * / % ^ ( ) . _`
and spaces. Anything else – including `:`, `,`, `[`, `]`, `;` – makes the whole
expression fall back to the centre expression. This prevents filtergraph
injection but also means a custom expression with a colon is silently ignored.

Escaping:

| Character | Emitted as |
| --- | --- |
| `'` | `''` |
| `\` | `\\` |
| `:` | `\:` |
| `%` | `\%` |
| others | unchanged |

The text is wrapped as `text='...'` and the filter ends with `expansion=none`, so
`%{...}` and `\n` are **not** expanded; newlines are literal only if the backend
accepts them in the string.

With video connected:

```text
drawtext=...,format=rgba
```

Without:

```text
color=c=black@0.0:s=<W>x<H>:d=<duration>:r=30,drawtext=...,format=rgba,setsar=1
```

`drawtext` requires an FFmpeg build with libfreetype. A missing font file or
fontconfig failure surfaces as an FFmpeg error in the preview panel, not as a
compiler error. `woff`/`woff2` paths can be selected but may not be loadable by
FFmpeg/fontconfig; `ttf`/`otf` are the safe choices.

---

## Compositing

### Transform

Scale then pad onto a transparent canvas.

- **Inputs:** `video`. **Output:** `video`.
- **Fields:** `width`, `height`, `x`, `y` (all `u32`; UI ranges: size 2..7680,
  position 0..7680).

Mapping:

```text
scale=<W>:<H>:flags=bicubic,
pad=max(iw\,<canvasW>):max(ih\,<canvasH>):<x>:<y>:color=black@0,
setsar=1
```

- `W`/`H` are forced even and clamped to `<= 7680`.
- The pad target is at least the Output canvas, so a scaled layer can extend
  beyond it; `Output` then letterboxes/scales the result back in.
- Padding is transparent black, letting Overlay show the background through it.
- The comma between `iw` and `canvasW` inside `max()` is escaped for the
  filtergraph.

### Overlay

Composite `foreground` over `background`.

- **Inputs:** `background` (video), `foreground` (video). Both required.
  **Output:** `video`.
- **Fields:** `x: i32`, `y: i32` (UI has no explicit range).

Mapping:

```text
overlay=x=<x>:y=<y>:eof_action=pass:format=auto,format=rgba,setsar=1
```

- Negative offsets are allowed and place the foreground partially off-canvas.
- `eof_action=pass` keeps the background visible after the foreground ends.
- `format=auto` lets FFmpeg pick the best overlay input format, usually preserving
  alpha for RGBA. The chain is re-normalised to RGBA afterwards.
- Both inputs must be connected; otherwise compilation reports a missing
  `background`/`foreground` input.

### Chroma Key

Removes a key colour and makes it transparent.

- **Input:** `video`. **Output:** `video`.
- **Fields:** `color: [u8;3]`, `similarity: f32` (0.01..1 UI), `blend: f32` (0..1 UI).

Mapping:

```text
format=rgba,
colorkey=color=0xRRGGBB:similarity=<s>:blend=<b>,
format=rgba
```

Values are clamped to `similarity 0.01..=1`, `blend 0..=1`. Green/blue presets
only fill the colour field. The output carries an alpha channel; use it above
another layer in `Overlay`.

### Alpha Key

Derives or applies transparency. Its optional **mask** input wins over the
selected mode.

- **Inputs:** `video` (required), `mask (opt)` (`Mask`, fed by a Video output).
- **Output:** `video`.
- **Fields:** `mode` (`LumaKey`, `ExtractAlpha`, `InvertAlpha`),
  `threshold`, `tolerance`, `softness` (UI 0..1).

**With mask connected:**

1. `scale2ref` normalises the mask to the video size.
2. `alphamerge` applies the mask.
3. For `InvertAlpha`, `lutrgb=a='255-val'` inverts the resulting alpha.

```text
[<mask>][<video>]scale2ref[<n>m][<n>v];
[<video>][<mask>]alphamerge[,lutrgb=a='255-val'],format=rgba
```

Exactly one `alphamerge` is emitted; the luma key settings are ignored.

**Without mask:**

| Mode | Mapping |
| --- | --- |
| Luma key | `format=rgba,lumakey=threshold=<t>:tolerance=<tol>:softness=<s>,format=rgba` |
| Extract alpha | `format=rgba,alphaextract,format=rgba` |
| Invert alpha | `format=rgba,lutrgb=a='255-val'` |

`threshold`, `tolerance`, `softness` are clamped to `0..=1` in all modes. For
Extract/Invert those three fields are stored but have no effect.

### Output

The render root and canvas definition.

- **Input:** `video` (required). **Outputs:** none.
- **Fields:** `width`, `height` (UI 2..7680).

Mapping:

```text
format=rgba,
scale=<W>:<H>:force_original_aspect_ratio=decrease,
pad=<W>:<H>:(ow-iw)/2:(oh-ih)/2:color=black,
setsar=1
[vout]
```

- Dimensions are forced even (`value.max(2) & !1`, capped at 7680).
- The incoming stream is scaled to fit, preserving aspect, and letterboxed with
  black bars. Transparent regions become black.
- Only the **first** `Output` node in node order is compiled. The palette prevents
  adding a second one; a hand-edited graph with two will silently ignore the
  extra.

---

## Masks

A mask is a single-channel grayscale matte: white = opaque, black = transparent.
Mask outputs cannot drive the `video` input of `Output` or most effects; connect
them to the `mask (opt)` input of **Alpha Key**, or to another mask node. The
compiler normalises every mask to `format=gray`.

### Matte Blur / Erode / Dilate

Softens or repairs a matte, for example after a chroma key or on a cropped edge.

- **Input:** `mask` (required). **Output:** `mask`.
- **Fields:** `blur`, `erode`, `dilate` (`f32`, `u32`, `u32`).
- **UI ranges:** blur 0..50, erode 0..4 passes, dilate 0..4 passes.
- **Default:** blur 3, erode 0, dilate 0.

Mapping:

```text
format=gray,
[boxblur=luma_radius=<blur>:luma_power=2,]   # only when blur > 0.05
[erosion × erode]
[dilation × dilate]
format=gray
```

`blur` is clamped to `0..=100` and written with two decimals; the pass counts are
clamped to `0..=4`. Blur runs before morphology. A `Video` source may be wired
straight in; it is converted to luma first.

### Mask Combine

Boolean-style combination of two mattes.

- **Inputs:** `mask a` and `mask b` (both required). **Output:** `mask`.
- **Field:** `mode`.
- **Values:** `Multiply` (intersection / AND), `Addition` (union / OR),
  `Difference` (XOR). Default `Multiply`.

Mapping:

```text
[mask b][mask a] scale2ref[b][a];
[a][b] format=gray,blend=all_mode=<mode>:all_opacity=1,format=gray
```

`mask b` is scaled to `mask a`'s dimensions first because FFmpeg's `blend`
requires matching sizes. `all_mode` values are `multiply`, `addition`, and
`difference`.

### Shape Mask

A procedural rectangle or oval matte, optionally feathered.

- **Inputs:** none. **Output:** `mask` (generated at the Output canvas size).
- **Fields:** `shape`, `width`, `height`, `x`, `y`, `feather`, `invert`,
  `duration`.
- **UI ranges:** width/height 2..7680 (forced even), x/y −7680..7680, feather
  0..200 px, duration 0.1..600 s.
- **Default:** rectangle, 640×360 at (0,0), feather 20, not inverted, 5 s.

Mapping (canvas dimensions `<W>`×`<H>`):

```text
color=c=black:s=<W>x<H>:d=<duration>:r=30,format=gray,
rectangle: drawbox=x=<x>:y=<y>:w=<w>:h=<h>:color=white:t=fill
oval:      geq=lum='<ellipse expression>'
[boxblur=luma_radius=<feather>:luma_power=2]  # rectangle feather
[negate]                                      # when invert
format=gray
```

- The rectangle feather uses `boxblur`; the oval builds its falloff directly in
  `geq`, using `hypot` around the ellipse centre and `clip()` for the edge ramp.
- A feather of 0 gives hard edges.
- Rectangle `drawbox` is clipped by the canvas automatically. The oval's `geq`
  expression is evaluated per pixel, so it is slower than `drawbox`.
- The node is a source: its own `duration` is independent of the timeline and it
  is not gated by timeline clips.

---

## Colour and image filters

### Color Correction

- **Fields:** `brightness`, `contrast`, `saturation` (f32).
- **UI ranges:** brightness −1..1, contrast 0..2, saturation 0..3.
- **Default:** `0.0`, `1.0`, `1.0`.

Mapping:

```text
eq=brightness=<b>:contrast=<c>:saturation=<s>,format=rgba
```

All three are clamped to the UI ranges at compile time and written with three
decimals. `contrast=1`, `saturation=1` are neutral.

### Blur

Gaussian blur using FFmpeg's `gblur`.

- **Fields:** `sigma` (horizontal), `sigma_v` (vertical), `steps: u32`.
- **UI ranges:** sigma/sigma_v 0..100, steps 1..6.

Mapping:

```text
format=rgba,
gblur=sigma=<h>:sigmaV=<v>:steps=<n>,
format=rgba
```

Compiler clamps sigma/sigma_v to `0..=1024` and steps to `1..=6`, each with two
decimals (steps as an integer). `sigma=0` with `sigmaV=0` is effectively an
identity pass. Blur runs at the source/stream resolution; the preview's 640×360
downscale happens afterwards, so the preview approximates—but does not exactly
match—the exported result.

### Sharpen

Edge sharpening via `unsharp`.

- **Fields:** `amount`, `clarity` (edge sharpness + clarity), `msize_x`,
  `msize_y` (matrix sizes).
- **UI ranges:** amount and clarity −2..5; matrix 3..23.

Mapping:

```text
format=rgba,
unsharp=lx=<oddX>:ly=<oddY>:la=<la>,
format=rgba
```

- `lx`/`ly` are passed through `odd_matrix`: clamp to `3..=23`, then force odd
  (`5 → 5`, `6 → 7`, `24 → 23`). Even values are not accepted by FFmpeg.
- `la = (amount + clarity).clamp(-2.0, 5.0)`, two decimals. A negative `la`
  softens/blurs; positive sharpens.
- The UI default matrix is 5×5 and the amount 1.0.
- Changing numeric fields with the DragValue returns an odd value immediately, so
  the stored field and the emitted filter agree.

### 3D LUT

Applies a cinematic lookup table.

- **Field:** `path: String`.
- **Picker:** native file dialog filtered to `.cube` and `.3dl`.
- **Default:** empty (identity).

Mapping:

- Empty or whitespace-only path:

  ```text
  format=rgba
  ```

  This is deliberately an identity pass, not a compile error, so a LUT node can
  be added before choosing a file.

- Otherwise:

  ```text
  format=rgba,lut3d=file=<escaped-path>,format=rgba
  ```

The path is escaped for FFmpeg's two-level filtergraph/option parsing, **not** for
a shell. The exact escaping is documented in
[Compilation § LUT path escaping](compilation.md#lut-path-escaping).

Notes:

- `.cube` and `.3dl` are supported by FFmpeg; the node does not parse or validate
  the file. A missing/invalid LUT is reported by FFmpeg as a preview or export
  error.
- The LUT is loaded by FFmpeg each run; there is no caching in the app.
- Paths with spaces work. Avoid relying on shell quoting: the path is an argv
  value.

### Color Balance

Lift / gamma / gain (shadows / midtones / highlights) per RGB channel.

- **Fields:** `shadows: [f32;3]`, `midtones: [f32;3]`, `highlights: [f32;3]`
  (RGB order).
- **UI range:** −1..1 for each channel, 2 decimals, grouped as
  **Shadows (Lift)**, **Midtones (Gamma)**, **Highlights (Gain)**.
- **Default:** all zeros (identity).

Mapping:

```text
format=rgba,
colorbalance=rs=<r>:gs=<g>:bs=<b>:
             rm=<r>:gm=<g>:bm=<b>:
             rh=<r>:gh=<g>:bh=<b>,
format=rgba
```

All nine values are clamped to `−1..=1` and written with three decimals. The node
has no preserve-lightness option.

### Levels

Input black/white points and per-channel gamma.

- **Fields:** `black: [f32;3]`, `white: [f32;3]`, `gamma: [f32;3]` (RGB order).
- **UI ranges:** black 0..1, white 0..1, gamma 0.1..10.
- **Defaults:** black 0, white 1, gamma 1 (identity).
- The inspector shows a **histogram placeholder**; it does not sample pixels or
  draw a histogram.

Mapping:

```text
format=rgba,
colorlevels=rimin=<r>:gimin=<g>:bimin=<b>:
             rimax=<r>:gimax=<g>:bimax=<b>,
eq=gamma_r=<r>:gamma_g=<g>:gamma_b=<b>,
format=rgba
```

Clamping before emission:

- black `0.0..=0.999`
- white `max(black + 0.001)..=1.0` per channel, so the range never inverts
- gamma `0.1..=10.0`
- all written with four decimals

FFmpeg's `colorlevels` has no gamma option in this build, so gamma is a separate
`eq` instance. That affects the whole frame per channel, not just the
black/white window. Values above 1 brighten midtones; below 1 darken them.

### Invert / Solarize

Combined channel inversion and threshold solarisation.

- **Fields:** `invert`, `solarize`, `threshold` (f32).
- **UI ranges:** all three 0..1. `threshold` is shown as 0..1 and multiplied by 255
  when compiled.
- **Default:** invert 0, solarize 0, threshold 0.5.

Mapping (alpha is never modified):

- `invert >= 0.999` → `negate`.
- `0.0005 < invert < 0.999` → a `lutrgb` blend:
  `clip(val + a*(255 - 2*val), 0, 255)` with `a = invert`. This linearly
  interpolates between the original value and its inverse.
- `invert <= 0.0005` → no inversion filter.
- `solarize > 0.0005` → an additional `lutrgb` with
  `clip(val + s*(255 - 2*val)*gte(val, t), 0, 255)`, where
  `t = threshold * 255`. Values at or above the threshold are inverted by the
  amount `s`; values below it are untouched.
- Both `r`, `g`, `b` get the same expression; alpha is preserved.
- When both amounts are off, only `format=rgba` is emitted (identity).
- Inversion is applied before solarisation.

Full chain shape:

```text
format=rgba[,negate | ,lutrgb=r='...':g='...':b='...'][,lutrgb=...],format=rgba
```

The threshold is written with two decimals, the amounts with four. Because
`gte` is inclusive, a threshold of exactly 1.0 affects only value 255.

---

## See also

- [Timeline](timeline.md) – clip windows and source re-timing.
- [Compilation](compilation.md) – how nodes become a filtergraph and FFmpeg args.
- [Troubleshooting](troubleshooting.md) – FFmpeg errors for LUTs, fonts, hardware.
