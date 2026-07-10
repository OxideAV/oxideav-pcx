# oxideav-pcx

[![CI](https://github.com/OxideAV/oxideav-pcx/actions/workflows/ci.yml/badge.svg)](https://github.com/OxideAV/oxideav-pcx/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/oxideav-pcx.svg)](https://crates.io/crates/oxideav-pcx) [![docs.rs](https://docs.rs/oxideav-pcx/badge.svg)](https://docs.rs/oxideav-pcx) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pure-Rust ZSoft PCX (PC Paintbrush) reader/writer for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework.

Clean-room implementation of the public **ZSoft PCX File Format
Technical Reference Manual**, Revision 5 (1991), the sole source of
truth for bitstream behaviour in this crate.

## Decode

| bits/pixel | n_planes | Source meaning                | Output |
| ---------- | -------- | ----------------------------- | ------ |
| 1          | 1        | Monochrome (1-bit)            | `Rgba` |
| 1          | 2        | 4-colour CGA (planar planes)  | `Rgba` |
| 1          | 3        | 8-colour EGA RGB              | `Rgba` |
| 1          | 4        | 16-colour EGA                 | `Rgba` |
| 2          | 1        | 4-colour CGA (packed bits)    | `Rgba` |
| 4          | 1        | 16-colour packed bits         | `Rgba` |
| 8          | 1        | 256-colour palette (VGA tail) | `Rgba` |
| 8          | 3        | 24-bit RGB (planar)           | `Rgba` |

* Per-row layout is planar: each on-disk scanline is `n_planes ×
  bytes_per_line` bytes, with planes laid one after the other within
  the row (NOT interleaved per pixel). The decoder re-packs planes
  into packed RGBA at decode time.
* The 256-colour VGA tail palette (PCX 3.0+, leading `0x0C` marker
  769 bytes from EOF) is honoured for 8 bpp × 1 plane images. When
  absent, an 8-bit grayscale ramp is used as a fallback. The tail
  probe is confined to that mode: spec §"VGA 256-color palette"
  introduces the appended block only for "more than 16 colors" and
  spec §"24-bit .PCX files" states 24-bit (8 bpp × 3 plane) images
  "do **not** contain a palette". For every other `(bpp, planes)`
  mode the whole post-header region is treated as RLE pixel data, so
  a 24-bit / EGA / CGA stream whose RLE payload happens to carry a
  `0x0C` byte 769 bytes from EOF is no longer mis-framed into
  stripping 769 bytes of real pixels (the coincidental-marker hazard
  the EGFF cross-reference flags for v3.0 files).
* Header field `palette_info` (spec §3): value `2` is the grayscale
  flag — the decoder honours it for 8 bpp × 1 plane images and
  produces a grayscale ramp regardless of any tail palette. The
  default is `1` (colour / BW).
* Header field `bytes_per_line` is range-checked against the visible
  width × `bits_per_pixel`: a value smaller than the per-plane row
  width required by the spec is rejected up front rather than
  silently mis-framing the planar→packed reconstruction. A value
  *larger* than the minimum is honoured as **trailing scanline
  padding** — the spec lets the on-disk per-plane stride exceed the
  picture-window requirement ("Do NOT calculate from Xmax-Xmin", and the
  cross-reference summary's `LinePaddingSize = ((BytesPerLine ×
  NumBitPlanes) × (8 / BitsPerPixel)) - ((XEnd - XStart) + 1)`). Every
  decode path (`parse_pcx` packed RGBA and all typed accessors) strips
  that surplus and recovers exactly the `width × height` window, so a
  foreign-encoder file with arbitrary even over-padding decodes
  identically to its minimal-stride twin. The accepted over-pad band is
  bounded above by the decompression-bomb guard (a stride claiming more
  planar bytes than the RLE payload can possibly back is rejected, not
  allocated).
* RLE decode is **continuous across the whole image**, matching the
  manual's own sample reader (the `for (l = 0; l < lsize; )` loop
  consumes `BytesPerLine × NPlanes × YSIZE` bytes straight through with
  no per-scanline reset). The spec's "decoding break at the end of each
  scan line" is an *encoder* convention, not a decode-time requirement,
  so a file whose run packet straddles a scanline (or plane, or trailing
  padding) boundary now decodes byte-identically to its row-broken
  equivalent instead of being rejected mid-row. A run that would overrun
  the whole-image byte total is still rejected (the total-bytes cap is
  preserved; only the per-row cap was relaxed).
* The 16-entry header EGA palette is used for `1 bpp × 4 planes`
  and `4 bpp × 1 plane`; if the header field is all zeros (which
  PCX 3.0+ files often emit) the standard hardware EGA palette per
  spec table §3.1 is substituted.
* `1 bpp × 3 planes` is the 8-colour EGA RGB mode (each bit-plane
  toggles one primary; plane order R, G, B per spec §4 bit-plane
  example). No external palette is consulted — the eight on/off
  primary combinations come straight out of the three plane bits at
  0x00 / 0xFF per channel.
* The 4-colour CGA palette for `2 bpp × 1 plane` is selected from
  header byte 19 — the colormap's fourth byte — per the manual's "CGA
  Color Map" C / P / I decomposition (bit 7 = color burst 0 color /
  1 monochrome, bit 6 = palette 0 yellow / 1 white family, bit 5 =
  intensity 0 dim / 1 bright), with the background colour pulled from
  the high nibble of header byte 16 (the colormap's first byte). Fixed
  in r401: both bytes were previously read 16 positions too deep into
  the colormap (header bytes 32 / 35) with an inverted two-bit palette
  convention, so foreign spec-conforming CGA files always decoded with
  the default palette.

## Encode

The framework-side [`Encoder`] (`make_encoder`) accepts seven
`oxideav_core::PixelFormat` variants — `Rgba`, `Rgb24`, `Bgr24`,
`Bgra`, `Gray8`, `MonoBlack`, and `MonoWhite`:

* `Rgba` / `Rgb24` / `Bgr24` / `Bgra` route to the 24-bit RGB writer
  (`encode_pcx_24bpp`). The `Bgr*` variants are per-pixel byte-swapped
  to RGB before encode; alpha is dropped from `Rgba` / `Bgra` (PCX
  has no alpha channel).
* `Gray8` routes through `encode_pcx_8bpp_grayscale` so the
  resulting PCX carries `palette_info = 2` (spec §3) and no VGA tail
  palette.
* `MonoBlack` and `MonoWhite` route through `encode_pcx_1bpp_mono`
  after unpacking the MSB-first packed-bit stride into one byte per
  pixel. The `MonoBlack` convention (0 = black, 1 = white) is a
  direct map onto the spec §4.1 monochrome writer; `MonoWhite`
  (0 = white, 1 = black) inverts the bit before emission so the
  decoder still sees bit 1 = white.

The codec capabilities advertise the same seven pixel formats so a
pipeline that picks formats from `accepted_pixel_formats` can hand
PCX whichever variant matches its source frame directly.

Standalone helpers:

* `encode_pcx_8bpp_indexed(w, h, &indices, &palette)` — 8 bpp × 1
  plane plus a 768-byte VGA tail palette.
* `encode_pcx_24bpp(w, h, &rgb)` — 8 bpp × 3 planes, planar RGB.
* `encode_pcx_1bpp_mono(w, h, &pixels)` — 1 bpp × 1 plane mono
  (bit 1 = white, bit 0 = black). Since r401 the writer also stores
  black / white in colormap entries 0 / 1 (the EGFF canonical mode
  matrix treats mono as the 2-colour paletted case), and the decoder
  resolves bits through a *non-zero* colormap's first two triples —
  so a foreign white-on-blue mono file decodes faithfully while
  zero-filled colormaps keep the classic convention. The reference
  doc's errata (Issue #227) has since pinned exactly this reading:
  the bit value is the colormap index, with `colormap[0]` = black /
  `colormap[1]` = white as the standard monochrome convention —
  bit 1 = white. r405 pins both directions at the raw-byte level
  (`tests/round405_mono_polarity.rs`).
* `encode_pcx_1bpp_3planes_ega_rgb(w, h, &rgb)` — 8-colour EGA RGB
  at 1 bpp × 3 planes. Each input channel byte is thresholded at
  0x80 to set its plane bit; the round-trip is exact when the
  source is already 0x00 / 0xFF per channel.
* `encode_pcx_1bpp_4planes_ega(w, h, &indices, &palette)` —
  16-colour EGA at 1 bpp × 4 planes; palette goes into the
  `ega_palette` header field.
* `encode_pcx_4bpp_packed(w, h, &indices, &palette)` — 16-colour
  packed-bits at 4 bpp × 1 plane (2 pixels/byte).
* `encode_pcx_2bpp_cga(w, h, &indices, palette_selector,
  background_index)` — 4-colour CGA packed-bits.
* `encode_pcx_1bpp_2planes_cga(w, h, &indices, palette_selector,
  background_index)` — 4-colour CGA in the plane-oriented 1 bpp × 2
  planes layout (the EGFF canonical CGA mode `BitsPerPixel = 1,
  NumBitPlanes = 2`). Bit `k` of each 2-bit index goes to plane `k`;
  the CGA palette is carried in the header bytes 16 / 19 exactly as the
  packed `encode_pcx_2bpp_cga` writer does, so identical indices flatten
  to identical pixels through either path.
* `encode_pcx_8bpp_grayscale(w, h, &pixels)` — 8 bpp × 1 plane
  grayscale with spec §3 `palette_info = 2` flag set and no tail
  palette appended. The decoder honours the flag and emits
  `(g, g, g, 0xFF)` per pixel regardless of any tail palette.
* `encode_pcx_24bpp_window(x_min, y_min, w, h, &rgb)` — like
  `encode_pcx_24bpp` but sets a non-zero `(x_min, y_min)` window
  origin for the PCX 3.0+ pixel-region edge case.
* `encode_pcx_rgb_auto(w, h, &rgb) -> (Vec<u8>, PcxAutoMode)` — emits
  the **smallest lossless** PCX the way PC Paintbrush did, via a full
  candidate ladder over the crate's spec modes (r401). A raster scan
  assigns each colour a first-seen index and bails on the 257th colour
  (`> 256` → planar 24-bit, byte-identical to `encode_pcx_24bpp`);
  otherwise every candidate whose losslessness precondition holds is
  encoded and the fewest-byte file wins:

  * **Mono1** — colours ⊆ {black, white} → 1 bpp × 1 plane;
  * **Cga2x1 / Cga1x2** — ≤ 4 colours exactly covered by one of the 96
    fixed CGA palettes (6 C/P/I selector families × 16 backgrounds,
    matched through the decoder's own resolver) → 2 bits/pixel in
    either the packed or plane-oriented layout;
  * **EgaRgb1x3** — every channel 0x00/0xFF → 1 bpp × 3 planes;
  * **Indexed4 / Indexed1x4** — ≤ 16 colours → both four-bit
    header-palette geometries (packed nibbles win on noise; periodic
    content RLE-collapses whole bit-planes and flips the win);
  * **Gray8** — all pure greys → `palette_info = 2`, no 769-byte tail
    (escape-heavy grey content can still hand the byte count back to
    Indexed8 — the ladder compares, never assumes);
  * **Indexed8** and **Rgb24** — the always-applicable baselines.

  Every candidate is exact (no quantisation anywhere), so the output
  decodes back to the source RGB bit-for-bit whatever rung wins; the
  returned `PcxAutoMode` records the geometry (plus colour count /
  CGA header encoding where relevant). First-seen palette order and a
  fixed preference order on exact ties keep the bytes deterministic.
  A cross-dimensional minimality suite asserts the chosen file never
  loses to ANY applicable direct-writer candidate. No new on-disk
  geometry — only the size-minimising choice among existing spec modes.
* `encode_pcx_image_auto(&image) -> (Vec<u8>, PcxAutoMode)` — the
  `PcxImage`-level companion (mirrors `encode_pcx_24bpp_image`). Flattens
  an `Rgba` / `Rgb24` image and runs the same ladder while preserving
  header metadata losslessly per branch: the planar branch threads the
  full `(window_origin, dpi, screen_size)` triple (via
  `encode_pcx_24bpp_image`); every compact rung threads authoring DPI
  through its `_dpi` writer variant. Since only the planar geometry has
  window-origin / screen-size header variants, an image carrying either
  field falls back to planar rather than dropping the metadata —
  lossless on both pixels and the requested annotations.

All writers emit **PCX 5.0** with `bytes_per_line` rounded up to
even per spec §1. The RLE encoder coalesces runs of ≤ 63 identical
bytes and escapes any singleton byte ≥ `0xC0` into a length-1
packet so the decoder won't mistake it for a run header.

## DCX multi-page bundles

`parse_dcx` / `encode_dcx` handle the Microsoft FAX `.dcx` wrapper
that concatenates multiple PCX pages into one file:

* 4-byte LE magic `0x3ADE_68B1`.
* Up to 1023 u32 LE offsets terminated by a zero sentinel.
* Each offset points at a stand-alone PCX 5.0 stream.

```rust
use oxideav_pcx::{encode_dcx, encode_pcx_24bpp, parse_dcx};

let p1 = encode_pcx_24bpp(8, 8, &vec![0u8; 8 * 8 * 3])?;
let p2 = encode_pcx_24bpp(8, 8, &vec![255u8; 8 * 8 * 3])?;
let dcx = encode_dcx(&[p1, p2])?;
let parsed = parse_dcx(&dcx)?;
assert_eq!(parsed.pages.len(), 2);
# Ok::<(), oxideav_pcx::PcxError>(())
```

```rust
use oxideav_pcx::{encode_pcx_24bpp, parse_pcx};

let rgb: Vec<u8> = /* 3 × W × H bytes, top-down RGB */ vec![/* … */];
let bytes = encode_pcx_24bpp(width, height, &rgb)?;
let img = parse_pcx(&bytes)?;
assert_eq!(img.width as usize * img.height as usize * 4, img.data.len());
# Ok::<(), oxideav_pcx::PcxError>(())
```

## Benchmarks

Three Criterion harnesses live under `benches/` for A/B-testing decoder
+ encoder hot-path changes against a stable baseline:

* `decode` — drives [`parse_pcx`] / [`parse_dcx`] across every spec §4.1
  (depth, planes) tuple at 320×240 / 640×480 / 512×512 / 1920×1080
  scales plus a 4-page DCX bundle, with a phase-split probe timing the
  spec §3.2 RLE decode in isolation.
* `encode` — drives the eight standalone write paths.
* `roundtrip` — pairs each encode path with its matching decode so a
  perf regression that quietly mis-encodes surfaces as a panic rather
  than a silently-cheaper benchmark number.

Bench inputs are synthesised on the fly via a deterministic fill; no
fixture files are committed. Output bytes stay bit-identical across
optimisation passes (roundtrip + cross-validate). A full ranked
baseline is in [`BENCHMARKS.md`](BENCHMARKS.md).

```sh
cargo bench -p oxideav-pcx --bench decode
cargo bench -p oxideav-pcx --bench encode
cargo bench -p oxideav-pcx --bench roundtrip
```

## Fuzzing

A [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) harness lives
under `fuzz/`. The `decode_pcx` target feeds arbitrary bytes to **every**
public decode entry point — `parse_pcx`, `parse_dcx`, and all ten typed
accessors (`parse_pcx_indexed_8bpp`, `parse_pcx_indexed_4bpp`,
`parse_pcx_indexed_4bpp_ega_hw`, `parse_pcx_indexed_1bpp_4planes`,
`parse_pcx_indexed_2bpp_cga`, `parse_pcx_indexed_1bpp_2planes_cga`,
`parse_pcx_indexed_2bpp_cga_cpi`, `parse_pcx_cga_cpi`,
`parse_pcx_indexed_1bpp_3planes`, `parse_pcx_indexed_4bpp_4planes`) — and
asserts each always returns a `Result` rather than panicking,
integer-overflowing, indexing out of bounds, or allocating an
attacker-claimed pixel buffer. Sharing one byte stream across all twelve
surfaces lets a single mutated input simultaneously probe the canonical
flattener AND each typed accessor's distinct rejection geometry (the CGA
selector dispatch, the EGA-hardware quantiser, the 3-plane / 4×4 index
stacking, the `u16`-per-pixel `(4, 4)` allocation). It is built with
`default-features = false` so it exercises the framework-free decode
path. A 13-entry seed corpus covers all six (depth, planes)
combinations, grayscale, a windowed-origin file, a DCX bundle,
degenerate inputs, plus a paired 4 bpp × 1 plane fixture for each
`Pcx4bppPaletteSource` arm (in-header / spec table §3.1 default).

```sh
cd fuzz && cargo +nightly fuzz run decode_pcx -- -max_total_time=60
```

The current baseline runs 40M+ executions with zero crashes (the
twelve-surface harness adds 4.5M+ executions over a 45s run with zero new
crashes after the entry-point expansion). Two hardening fixes came out of
the initial run: the public
`PcxHeader::width()` / `height()` accessors now saturate instead of
underflow-panicking on an `x_max < x_min` header, and `parse_pcx` has a
decompression-bomb guard that rejects a tiny file claiming enormous
dimensions before it can reserve hundreds of gigabytes.

A second `encode_pcx` target covers the symmetric **encoder** surface: it
carves two `u16` dimensions off the front of the fuzz input (masked to a
`0x1FF` ceiling so the packing-geometry edge cases are hit without the
fuzzer driving a multi-gigabyte allocation off a `0xFFFF × 0xFFFF` claim)
and feeds the remaining bytes as the pixel/index payload to every public
encoder. Each must return `Ok`/`Err` without panicking, going out of
bounds, or overflowing; where it succeeds, the bytes it emits are fed
straight back through `parse_pcx` and the matching typed accessor so the
**encode→decode seam** is under the same no-panic contract. A 40-second
run executes 958k+ iterations with zero crashes.

```sh
cd fuzz && cargo +nightly fuzz run encode_pcx -- -max_total_time=60
```

The same encoder contract is additionally pinned in the CI-run `tests/`
harness (`tests/round337.rs`): a 12-entry dimension matrix spanning the
packing edge cases (1×1, 1-tall, 1-wide, odd / even widths, sub-byte
chunk boundaries) sweeps the index/sample-based encoders and asserts both
the clean-reject behaviour (undersized input, zero dimension,
wrong-length palette, out-of-range CGA background) and **lossless
round-trip through the typed accessor** (`parse_*indexed*(encode(x))
== x` after masking to each mode's bit width), so the property is proven
green on every push rather than only under a manual fuzz session.

`tests/round345.rs` widens that into an **exhaustive cross-dimensional
sweep**: every one of the ten encode→decode paths is driven across a
Cartesian product of 13 widths × 6 heights (~780 round-trips total),
each asserting a bit-exact index / sample recovery. The width set is
chosen to land on and one-past every packing seam — the even-
`bytes_per_line` padding, the 8 / 4 / 2 pixels-per-byte sub-byte chunk
boundaries, and the degenerate 1- / 2-column cases — so a mis-handled
partial-byte or padding edge surfaces as a mismatch rather than slipping
through the single representative size each older fixture pins.

`tests/round345_rle.rs` pins the run-length core (`rle::{encode,
decode}`, spec §3.2) at the unit level: the encode/decode inverse on
random streams including every `0xC0..=0xFF` high byte, the 2-byte
high-byte escape, the 63-byte run cap with multi-packet splitting,
mid-packet / short-stream truncation rejection, the `out_len` overrun
cap, the count-zero header tolerance (matching the manual's `encget`),
and a packet-grammar invariant that the encoded stream never emits a
bare high-byte literal.

## Standalone vs registry-integrated

The crate's default `registry` Cargo feature pulls in `oxideav-core`
and exposes the framework `Decoder` / `Encoder` trait surface plus a
`registry::register` entry point for the `oxideav` umbrella crate.
Disable the feature for an `oxideav-core`-free build:

```toml
oxideav-pcx = { version = "0.0", default-features = false }
```

The standalone build still exposes `parse_pcx`, `encode_pcx_24bpp`,
`encode_pcx_8bpp_indexed`, plus crate-local `PcxImage` /
`PcxPixelFormat` / `PcxError` types.

## Registration

```rust
let mut codecs = oxideav_core::CodecRegistry::new();
let mut containers = oxideav_core::ContainerRegistry::new();
oxideav_pcx::register(&mut codecs, &mut containers);
```

## Authoring DPI (`h_dpi` / `v_dpi`)

The header's `h_dpi` / `v_dpi` fields (offsets 12 / 14) are surfaced
on the decoded [`PcxImage`] as `Option<(u16, u16)>` — per spec §3 the
fields record "the resolutions at which the image was created
(printer or scanner); e.g. a scan might store 300, 300", and a 0 in
either field is the documented "unset" sentinel. The decoder reports
`Some((h, v))` whenever both fields are non-zero, `None` otherwise.

Four `*_dpi` writer variants stamp a custom authoring resolution into
the header in place of the historical 72×72 PC Paintbrush convention
the plain writers default to:

* `encode_pcx_24bpp_dpi(w, h, &rgb, (h_dpi, v_dpi))`
* `encode_pcx_8bpp_indexed_dpi(w, h, &indices, &palette, dpi)`
* `encode_pcx_8bpp_grayscale_dpi(w, h, &pixels, dpi)`
* `encode_pcx_1bpp_mono_dpi(w, h, &pixels, dpi)`
* `encode_pcx_4bpp_packed_dpi(w, h, &indices, &palette, dpi)`
* `encode_pcx_2bpp_cga_dpi(w, h, &indices, palette_selector, background_index, dpi)`
* `encode_pcx_1bpp_3planes_ega_rgb_dpi(w, h, &rgb, dpi)`
* `encode_pcx_1bpp_4planes_ega_dpi(w, h, &indices, &palette, dpi)`

The four EGA / CGA palette-mode `_dpi` writers mirror their
non-DPI siblings byte-for-byte except for the header `h_dpi` / `v_dpi`
words (spec §3 offsets 12 / 14), so the `_dpi` suite now spans every
encode path. Per spec §3 the DPI field is format-independent — "the
resolutions at which the image was created (printer or scanner)" — so a
scanned 16-colour EGA or 4-colour CGA image preserves its authoring
resolution through decode → re-encode instead of being flattened to the
historical 72×72 default. As with the existing `_dpi` writers a 0 in
either component is rejected at the writer boundary per the spec §3
"0 = unset" sentinel.

The wrapper `encode_pcx_24bpp_image` automatically threads
`PcxImage::dpi` through into the new header when the decoded image
carries a `Some(...)`, so a decode → re-encode pass preserves the
scanner DPI end-to-end. Both DPI components must be non-zero; a 0 is
rejected up front to keep the spec §3 "unset" semantic intact at the
writer boundary too. The remaining writers (`encode_pcx_24bpp`,
`encode_pcx_4bpp_packed`, `encode_pcx_2bpp_cga`,
`encode_pcx_1bpp_3planes_ega_rgb`, `encode_pcx_1bpp_4planes_ega`,
`encode_pcx_24bpp_window`) keep the historical 72×72 default.

## Window origin (`x_min` / `y_min`)

The header's `x_min` / `y_min` words (offsets 4 / 6) record the
source crop region the pixel buffer came from. Spec §3 derives the
visible width / height as `x_max - x_min + 1` and `y_max - y_min +
1`; PCX 3.0+ supports a non-zero origin so an editor can preserve the
position of a cropped sub-image inside its parent canvas. The decoder
surfaces this on [`PcxImage`] as
`Option<(u16, u16)> window_origin` — `Some((x, y))` whenever either
header word is non-zero, `None` for the conventional zero-origin
screen-author case so the re-encode wrapper doesn't restate `(0, 0)`
as data.

The combined writer `encode_pcx_24bpp_window_dpi(x_min, y_min, w, h,
&rgb, (h_dpi, v_dpi))` stamps both the origin AND the authoring DPI
into a single PCX 5.0 file, mirroring the existing
[`encode_pcx_24bpp_window`] (origin only) and
[`encode_pcx_24bpp_dpi`] (DPI only) writers. The wrapper
`encode_pcx_24bpp_image` now dispatches across the four
`(window_origin, dpi)` combinations so a decoded windowed-and-tagged
PCX round-trips both fields through one call instead of forcing the
caller to flatten the metadata by hand.

## Authoring screen size (`h_screen_size` / `v_screen_size`)

The header's `h_screen_size` / `v_screen_size` words (offsets 70 / 72)
record what spec §3 describes as "the horizontal / vertical screen
size in pixels (new field found only in PB IV / IV Plus)" — an
authoring-time annotation distinct from the printer / scanner DPI in
`h_dpi` / `v_dpi`. The decoder surfaces this on [`PcxImage`] as
`Option<(u16, u16)> screen_size` — `Some((h, v))` whenever both header
words are non-zero, `None` otherwise (the spec §3 sentinel rule
collapses an asymmetric pair to `None` because a 0 in either component
means "unset", which the pre-PB-IV writers leave at the default).

Two new writers stamp the field in place of the historical zero-fill:

* `encode_pcx_24bpp_screen(w, h, &rgb, (h_screen, v_screen))` — like
  the plain `encode_pcx_24bpp` writer but with a custom screen-size
  pair (DPI stays at the 72×72 default, origin at `(0, 0)`).
* `encode_pcx_24bpp_window_dpi_screen(x_min, y_min, w, h, &rgb,
  (h_dpi, v_dpi), (h_screen, v_screen))` — the maximally-tagged
  writer that combines window origin + authoring DPI + screen size in
  one call.

Both components must be non-zero; a 0 is rejected at the writer
boundary because the decoder would surface such a file with
`screen_size = None`, so emitting the redundant zero pair would be
indistinguishable from the plain writer's output. The wrapper
`encode_pcx_24bpp_image` now dispatches across the eight
`(window_origin, dpi, screen_size)` `Option` combinations so a
decoded fully-tagged PCX round-trips every metadata field through one
call.

## Typed 8 bpp paletted view

[`parse_pcx`] always flattens to packed `Rgba` — convenient for display
pipelines but discards the on-disk palette indices. The typed accessor
[`parse_pcx_indexed_8bpp`] preserves them for the 8 bpp × 1 plane case:
it returns a [`PcxIndexed8`] carrying the raw `width × height` index
buffer (one byte per pixel, top-down, per-row padding stripped) plus
the resolved 256-entry RGB palette and a [`PcxPaletteSource`] tag
recording which spec §3 branch produced it.

* `PcxPaletteSource::VgaTail` — optional 768-byte palette block at
  end-of-file, marker `0x0C` 769 bytes before EOF (spec §3).
* `PcxPaletteSource::GrayscaleFlag` — header `palette_info = 2` forces
  the synthetic `0..=255` grayscale ramp regardless of whether a tail
  block is also present (spec §3).
* `PcxPaletteSource::GrayscaleFallback` — neither flag nor tail block;
  the decoder fills the palette with the synthetic ramp deterministically.

Useful for round-tripping a paletted PCX through `encode_pcx_8bpp_indexed`
without re-quantising the pixels, or for applying palette-swap operations
on the indices directly. Any (depth, planes) combination other than
`(8, 1)` is rejected with `PcxError::Unsupported` — the 24-bit and
EGA/CGA paths have different palette geometries and are not covered by
this accessor.

```rust
use oxideav_pcx::{parse_pcx_indexed_8bpp, PcxPaletteSource};

let view = parse_pcx_indexed_8bpp(bytes)?;
let i = view.indices[0] as usize;
let [r, g, b] = view.palette[i];
assert!(matches!(
    view.palette_source,
    PcxPaletteSource::VgaTail
        | PcxPaletteSource::GrayscaleFlag
        | PcxPaletteSource::GrayscaleFallback
));
# Ok::<(), oxideav_pcx::PcxError>(())
```

## Typed 4 bpp paletted view

[`parse_pcx_indexed_4bpp`] is the symmetric typed accessor for the EGFF
"4 bpp / 1 plane / 16 colours / EGA and VGA" mode: it returns a
[`PcxIndexed4`] carrying the unpacked `width × height` nibble indices
(one byte per pixel, low nibble = palette index `0..=15`, top-down,
per-row padding stripped) alongside the resolved 16-entry RGB palette
and a [`Pcx4bppPaletteSource`] tag recording which spec §3 branch
produced the palette.

* `Pcx4bppPaletteSource::Ega16InHeader` — the 48-byte header
  `ega_palette` field carried at least one non-zero byte; the surfaced
  palette is read straight from those 16 RGB triplets.
* `Pcx4bppPaletteSource::Ega16Default` — the header `ega_palette`
  field was all-zeros (common in PCX 3.0+ files even for EGA data);
  the surfaced palette is the standard 16-entry EGA hardware palette
  per spec table §3.1.

Useful for round-tripping a 16-colour PCX through `encode_pcx_4bpp_packed`
without re-quantising the nibbles, or for applying palette-swap
operations on the indices directly. Any (depth, planes) combination
other than `(4, 1)` is rejected with `PcxError::Unsupported` — the 8 bpp
paletted path has its own typed accessor (`parse_pcx_indexed_8bpp`); the
1 bpp × 4 planes EGA path shares the 16-entry palette geometry but the
on-disk plane shape is different and is not covered by this accessor.

```rust
use oxideav_pcx::{parse_pcx_indexed_4bpp, Pcx4bppPaletteSource};

let view = parse_pcx_indexed_4bpp(bytes)?;
let i = view.indices[0] as usize;
let [r, g, b] = view.palette[i];
assert!(matches!(
    view.palette_source,
    Pcx4bppPaletteSource::Ega16InHeader | Pcx4bppPaletteSource::Ega16Default
));
# Ok::<(), oxideav_pcx::PcxError>(())
```

### EGA hardware 4-level palette quantisation (`*_ega_hw`)

Spec §"EGA/VGA 16-color palette" notes that "on an IBM EGA there are only
4 levels of RGB for each color. Since 256/4 = 64" — and tabulates the
input buckets:

| Setting   | Level |
| --------- | ----: |
| 0–63      | 0     |
| 64–127    | 1     |
| 128–192   | 2     |
| 193–254   | 3     |

A PCX header `Colormap` stores 0–255 per component, but an EGA display
resolves only those four levels, so a file authored on (or for) EGA
hardware whose header palette carries arbitrary 0–255 values is shown
with each component snapped to one of the four EGA DAC output intensities
(`0x00 / 0x55 / 0xAA / 0xFF` — exactly the values the rev-5 manual's
default 16-colour EGA palette, surfaced as
[`Pcx4bppPaletteSource::Ega16Default`], is built from). The manual's
table ends at 254; value 255 falls in the same top bucket.

* [`ega_quantize_level`] — stored 0–255 component → EGA level `0..=3`.
* [`ega_quantize_component`] — level composed with the EGA DAC output
  ramp; idempotent for values already on the ramp.
* [`ega_quantize_palette`] — routes every component of a 16-entry palette.
* [`parse_pcx_indexed_4bpp_ega_hw`] — the EGA-hardware sibling of
  [`parse_pcx_indexed_4bpp`]: identical indices and
  [`Pcx4bppPaletteSource`] tag, palette snapped to the EGA-displayable
  colours. When the header field is all-zeros the spec table §3.1
  default is substituted first (as the raw accessor does); that default
  is already on the ramp, so the quantised view differs from the raw one
  only for the `Ega16InHeader` branch carrying off-ramp scanner / editor
  values. Strictly additive — the raw [`parse_pcx_indexed_4bpp`] view and
  the canonical [`parse_pcx`] flatten path are unchanged.

```rust
use oxideav_pcx::{ega_quantize_component, ega_quantize_level};

assert_eq!(ega_quantize_level(100), 1); // 64–127 bucket
assert_eq!(ega_quantize_component(100), 0x55); // level 1 → DAC ramp
assert_eq!(ega_quantize_component(0xAA), 0xAA); // on-ramp → fixed
```

## Typed 1 bpp × 4 planes paletted view

[`parse_pcx_indexed_1bpp_4planes`] is the third 16-colour typed accessor —
covering the spec §4.1 EGA bit-plane on-disk layout where each scanline
carries four 1-bit planes laid out one after another within the row (plane
0, plane 1, plane 2, plane 3 — the same plane order
`encode_pcx_1bpp_4planes_ega` writes). The bit at each x-position across
the four planes stacks into a 4-bit palette index (`p0 | p1<<1 | p2<<2 |
p3<<3`).

The returned [`PcxIndexed1x4`] carries the resolved `width × height`
nibble indices (one byte per pixel, low nibble = palette index `0..=15`,
top-down, padding stripped) alongside the resolved 16-entry RGB palette
and a [`Pcx1bpp4PlanesPaletteSource`] tag recording which spec §3 branch
produced the palette.

* `Pcx1bpp4PlanesPaletteSource::Ega16InHeader` — the 48-byte header
  `ega_palette` field carried at least one non-zero byte; the surfaced
  palette is read straight from those 16 RGB triplets.
* `Pcx1bpp4PlanesPaletteSource::Ega16Default` — the header `ega_palette`
  field was all-zeros (common in PCX 3.0+ files even for EGA data); the
  surfaced palette is the standard 16-entry EGA hardware palette per
  spec table §3.1.

Useful for round-tripping a 16-colour EGA PCX through
`encode_pcx_1bpp_4planes_ega` without re-quantising the pixels, or for
applying palette-swap operations on the indices directly. Any (depth,
planes) combination other than `(1, 4)` is rejected with
`PcxError::Unsupported` — the 4 bpp × 1 plane path has its own typed
accessor (`parse_pcx_indexed_4bpp`); the 8 bpp paletted path has
`parse_pcx_indexed_8bpp`. The three 16-colour-or-greater paletted views
share the same nibble / byte index convention so a downstream pipeline
can hand any of them to a 16-colour code path without branching on the
on-disk depth.

```rust
use oxideav_pcx::{parse_pcx_indexed_1bpp_4planes, Pcx1bpp4PlanesPaletteSource};

let view = parse_pcx_indexed_1bpp_4planes(bytes)?;
let i = view.indices[0] as usize;
let [r, g, b] = view.palette[i];
assert!(matches!(
    view.palette_source,
    Pcx1bpp4PlanesPaletteSource::Ega16InHeader
        | Pcx1bpp4PlanesPaletteSource::Ega16Default
));
# Ok::<(), oxideav_pcx::PcxError>(())
```

## Typed 2 bpp × 1 plane CGA paletted view

[`parse_pcx_indexed_2bpp_cga`] is the typed accessor for the 4-colour CGA
mode (2 bpp × 1 plane, 4 pixels/byte). PCX repurposes the start of the
48-byte colormap header region for CGA mode (manual §"CGA Color Map"):
header byte 16 — the colormap's first byte — carries the EGA index used
for palette entry 0 (the CGA "background / border" colour) in its high
nibble, and header byte 19 — the colormap's fourth byte — carries the
C / P / I selector bits (C bit 7 = color burst, P bit 6 = palette
family, I bit 5 = intensity).

The returned [`PcxIndexed2x1Cga`] surfaces the unpacked `width × height`
2-bit indices (one byte per pixel, low two bits = palette index
`0..=3`, top-down, padding stripped) alongside the resolved 4-entry RGB
palette, the resolved `background_index` (`0..=15`) read from header
byte 16's high nibble, and a [`Pcx2bppCgaPaletteSource`] tag
recording which resolved palette family the decoder landed on.

* `Pcx2bppCgaPaletteSource::Palette1HighIntensity` — selector byte 0x60
  (C=0, P=1 white family, I=1 bright); the most common CGA palette for
  game screenshots of the era — cyan / magenta / white.
* `Pcx2bppCgaPaletteSource::Palette1LowIntensity` — selector byte 0x40
  — dim cyan / dim magenta / light gray.
* `Pcx2bppCgaPaletteSource::Palette0HighIntensity` — selector byte 0x20
  — light green / light red / yellow.
* `Pcx2bppCgaPaletteSource::Palette0LowIntensity` — selector byte 0x00
  — green / red / brown (also where zero-filled legacy headers land).
* `Pcx2bppCgaPaletteSource::MonochromeDim` / `::MonochromeBright` —
  selector bytes 0x80 / 0xA0 (C=1) — the composite-monochrome
  four-level grey ramps.

The [`Pcx2bppCgaPaletteSource::palette_selector`] helper reconstructs
the byte 19 selector pattern so a round-trip caller can hand the
surfaced view straight back to `encode_pcx_2bpp_cga` without
re-deriving the bit positions — a decode → re-encode pass produces a
byte-identical PCX file.

Useful for round-tripping a 4-colour CGA PCX through
`encode_pcx_2bpp_cga` without re-quantising the indices, or for
applying palette-swap operations on the indices directly. Any (depth,
planes) combination other than `(2, 1)` is rejected with
`PcxError::Unsupported` — the 16-colour packed-bits path has its own
typed accessor (`parse_pcx_indexed_4bpp`); the 8 bpp paletted path has
`parse_pcx_indexed_8bpp`; the EGA bit-plane path has
`parse_pcx_indexed_1bpp_4planes`.

```rust
use oxideav_pcx::{parse_pcx_indexed_2bpp_cga, Pcx2bppCgaPaletteSource};

let view = parse_pcx_indexed_2bpp_cga(bytes)?;
let i = view.indices[0] as usize;
let [r, g, b] = view.palette[i];
assert!(matches!(
    view.palette_source,
    Pcx2bppCgaPaletteSource::Palette1HighIntensity
        | Pcx2bppCgaPaletteSource::Palette1LowIntensity
        | Pcx2bppCgaPaletteSource::Palette0HighIntensity
        | Pcx2bppCgaPaletteSource::Palette0LowIntensity
));
# Ok::<(), oxideav_pcx::PcxError>(())
```

### Spec-faithful CGA C / P / I selector (`parse_pcx_indexed_2bpp_cga_cpi`)

The [`Pcx2bppCgaPaletteSource`] accessor above reads only the top two bits
of header byte 19. The verbatim ZSoft manual ("CGA Color Map", Header Byte
#19) actually defines **three** significant bits ordered **C, P, I**:

* **C** — bit 7 — color burst enable: `0` = color, `1` = monochrome.
* **P** — bit 6 — palette: `0` = yellow family (green / red / brown),
  `1` = white family (cyan / magenta / white).
* **I** — bit 5 — intensity: `0` = dim, `1` = bright.

[`parse_pcx_indexed_2bpp_cga_cpi`] decodes all three into a
[`Pcx2bppCgaCpi`] (`from_byte19` / `to_byte19` mask off the lower five
"ignored" bits per the manual) and resolves the matching palette. The
**color-burst monochrome** mode (`C = 1`) — which the two-bit accessor
cannot represent — resolves a four-level composite-grey ramp derived from
the spec's own EGA quantisation table (the four signal levels in
"EGA/VGA 16-color palette"), in dim and bright flavours; palette entry 0
is still overridden by the header byte 16 background nibble. The matching
[`encode_pcx_2bpp_cga_cpi`] writer round-trips every C / P / I combination
byte-for-byte. The legacy [`parse_pcx_indexed_2bpp_cga`] /
[`encode_pcx_2bpp_cga`] pair and `parse_pcx`'s `(2, 1)` flatten path are
unchanged — the C / P / I pair is strictly additive.

```rust
use oxideav_pcx::{encode_pcx_2bpp_cga_cpi, parse_pcx_indexed_2bpp_cga_cpi, Pcx2bppCgaCpi};

let cpi = Pcx2bppCgaCpi { monochrome: true, palette_white: false, intensity_bright: true };
let pcx = encode_pcx_2bpp_cga_cpi(8, 4, &[0u8; 8 * 4], cpi, 0)?;
let view = parse_pcx_indexed_2bpp_cga_cpi(&pcx)?;
assert_eq!(view.cpi, cpi);
# Ok::<(), oxideav_pcx::PcxError>(())
```

### Spec-faithful CGA flatten entry point (`parse_pcx_cga_cpi`)

[`parse_pcx_indexed_2bpp_cga_cpi`] above preserves the on-disk *indices*;
[`parse_pcx_cga_cpi`] is its **flatten**-to-`Rgba` sibling for callers that
want packed pixels directly. It resolves the 4-colour palette through the
same full C / P / I decomposition of header byte 19, so the
**color-burst monochrome** mode (`C = 1`) flattens to the four-level
composite-grey ramp instead of being mis-coloured as a chroma palette —
the gap the legacy `parse_pcx` flatten path (a `(palette-select,
intensity)` two-bit model of bits 7 / 6) cannot close. It covers **both**
CGA on-disk layouts: `2 bpp × 1 plane` packed (4 px/byte) and
`1 bpp × 2 planes` plane-oriented; identical indices + header palette
bytes flatten to identical pixels through either layout. The surfaced
`dpi` / `window_origin` / `screen_size` fields follow the same spec §3
"0 = unset" sentinel rules as [`parse_pcx`]. Any `(bpp, planes)` other
than `(2, 1)` or `(1, 2)` is rejected with `PcxError::Unsupported` — every
non-CGA mode already flattens spec-faithfully through [`parse_pcx`], which
honours its header palette directly. The legacy [`parse_pcx`] flatten path
and the legacy CGA typed accessors are unchanged — the C / P / I flatten
entry point is strictly additive.

```rust
use oxideav_pcx::{encode_pcx_2bpp_cga_cpi, parse_pcx_cga_cpi, Pcx2bppCgaCpi};

let cpi = Pcx2bppCgaCpi { monochrome: true, palette_white: false, intensity_bright: false };
let pcx = encode_pcx_2bpp_cga_cpi(8, 4, &[0u8; 8 * 4], cpi, 0)?;
let img = parse_pcx_cga_cpi(&pcx)?;
// Monochrome mode → every pixel is grey (R == G == B).
assert!(img.data.chunks_exact(4).all(|px| px[0] == px[1] && px[1] == px[2]));
# Ok::<(), oxideav_pcx::PcxError>(())
```

## Typed 1 bpp × 3 planes 8-colour EGA RGB view

[`parse_pcx_indexed_1bpp_3planes`] is the fifth (and final) paletted typed
accessor — covering the 8-colour EGA RGB mode described in spec §4 where
each scanline carries three 1-bit planes laid out one after another within
the row (plane 0 = R, plane 1 = G, plane 2 = B — the same plane order
`encode_pcx_1bpp_3planes_ega_rgb` writes). The three bits at the same
x-position stack into a 3-bit colour index (`r | g << 1 | b << 2`), and
each plane bit toggles its channel between `0x00` and `0xFF`.

Unlike the other paletted modes, this carries **no on-disk palette** — the
eight colours are the on/off primary combinations enumerated by the plane
bits themselves, so the [`Pcx1bpp3PlanesPaletteSource`] tag has a single
`FixedPrimaries` arm (present for API symmetry + to document the
no-header-palette property explicitly). The returned [`PcxIndexed1x3`]
carries the unpacked `width × height` colour indices (one byte per pixel,
low three bits = colour index `0..=7`, top-down, padding stripped)
alongside the fixed 8-entry RGB palette.

Useful for round-tripping an 8-colour EGA RGB PCX through
`encode_pcx_1bpp_3planes_ega_rgb` without re-thresholding, or for applying
colour-swap operations on the indices directly. Any (depth, planes)
combination other than `(1, 3)` is rejected with `PcxError::Unsupported`.

```rust
use oxideav_pcx::{parse_pcx_indexed_1bpp_3planes, Pcx1bpp3PlanesPaletteSource};

let view = parse_pcx_indexed_1bpp_3planes(bytes)?;
let i = view.indices[0] as usize;
let [r, g, b] = view.palette[i];
assert!(matches!(
    view.palette_source,
    Pcx1bpp3PlanesPaletteSource::FixedPrimaries
));
# Ok::<(), oxideav_pcx::PcxError>(())
```

## Typed 1 bpp × 2 planes CGA paletted view

[`parse_pcx_indexed_1bpp_2planes_cga`] is the typed accessor for the
plane-oriented 4-colour CGA mode the EGFF canonical PCX mode matrix
lists as `BitsPerPixel = 1, NumBitPlanes = 2` — the bit-plane sibling
of the packed `2 bpp × 1 plane` CGA layout. Each on-disk scanline
carries plane 0 then plane 1 one after another within the row; the bit
at the same x-position in each plane stacks into the 2-bit palette index
(`p0 | p1 << 1`), the same bit ordering the 1 bpp × 4 planes EGA path
uses.

The returned [`PcxIndexed1x2Cga`] surfaces one byte per pixel (low two
bits = palette index `0..=3`, top-down, padding stripped) alongside the
resolved 4-entry RGB palette, the `background_index` (`0..=15`) read
from `ega_palette[16]`'s high nibble, and the same
[`Pcx2bppCgaPaletteSource`] tag the packed accessor uses (the palette
resolution from header bytes 16 / 19 is shared verbatim, so the two CGA
layouts resolve identical colours from identical header bytes). The
[`Pcx2bppCgaPaletteSource::palette_selector`] helper reconstructs the
byte 19 selector so a decode → re-encode pass through
[`encode_pcx_1bpp_2planes_cga`] is byte-identical. Any (depth, planes)
combination other than `(1, 2)` is rejected with `PcxError::Unsupported`
— the packed CGA layout has its own typed accessor
([`parse_pcx_indexed_2bpp_cga`]).

```rust
use oxideav_pcx::{parse_pcx_indexed_1bpp_2planes_cga, Pcx2bppCgaPaletteSource};

let view = parse_pcx_indexed_1bpp_2planes_cga(bytes)?;
let i = view.indices[0] as usize;
let [r, g, b] = view.palette[i];
assert!(matches!(
    view.palette_source,
    Pcx2bppCgaPaletteSource::Palette1HighIntensity
        | Pcx2bppCgaPaletteSource::Palette1LowIntensity
        | Pcx2bppCgaPaletteSource::Palette0HighIntensity
        | Pcx2bppCgaPaletteSource::Palette0LowIntensity
));
# Ok::<(), oxideav_pcx::PcxError>(())
```

## 4 bpp × 4 planes composite-index mode

[`parse_pcx_indexed_4bpp_4planes`] / [`encode_pcx_4bpp_4planes`] cover the
one `(bpp, planes)` slot the EGFF canonical PCX video-mode matrix does
**not** list as a hardware video mode but which the format is structurally
able to describe. The cross-reference summary's colour-count formula
`MaxNumberOfColors = (1 << (BitsPerPixel * NumBitPlanes))` evaluates to
`1 << (4 × 4) = 65536` for this mode, and the on-disk layout is the same
plane-oriented form every multi-plane PCX uses (spec §"Image File (.PCX)
Format": "each line of the image is stored by color plane"): each scanline
carries plane 0, plane 1, plane 2, plane 3 one after another, each holding
4 bits/pixel (2 pixels/byte, high nibble first — the same packing the
`4 bpp × 1 plane` path uses). The nibble at the same x-position across the
four planes stacks into a 16-bit composite index (`p0 | p1 << 4 |
p2 << 8 | p3 << 12`), the natural generalisation of the
`parse_pcx_indexed_1bpp_4planes` plane-`k`-supplies-chunk-`k` ordering from
1-bit to 4-bit chunks.

Unlike the ≤ 256-colour paletted modes, **no palette is surfaced or
written**: the ZSoft rev-5 manual and the EGFF cross-reference define
palette geometries only for the ≤ 256-colour modes (16-entry header
`Colormap` for EGA/CGA, 768-byte VGA tail for 256-colour) and state the
24-bit mode carries no palette at all. There is no documented 65536-entry
palette for this mode, so [`PcxIndexed4x4`] carries the raw `width ×
height` composite indices only (one `u16` per pixel, top-down, per-row
padding stripped) and leaves interpretation to the caller. For the same
reason [`parse_pcx`] (which must produce packed `Rgba`) rejects `(4, 4)`
with `PcxError::Unsupported` rather than inventing a colour mapping the
spec does not define. Any `(depth, planes)` other than `(4, 4)` is
rejected by the typed accessor.

```rust
use oxideav_pcx::{encode_pcx_4bpp_4planes, parse_pcx_indexed_4bpp_4planes};

let indices: Vec<u16> = vec![0x0000, 0xFFFF, 0x1234, 0xABCD]; // 4×1
let pcx = encode_pcx_4bpp_4planes(4, 1, &indices)?;
let view = parse_pcx_indexed_4bpp_4planes(&pcx)?;
assert_eq!(view.indices, indices);
# Ok::<(), oxideav_pcx::PcxError>(())
```

With this slot covered, every row of the EGFF canonical mode matrix
(monochrome / CGA / EGA / EGA-VGA / Extended-VGA / Extended-VGA-XGA) plus
the structurally-reachable 4 bpp × 4 planes composite-index mode is handled
on both decode and encode.

## Lacks

* `PixelFormat::Pal8` input to the framework `Encoder`. The
  out-of-band palette companion needed by `Pal8` isn't currently
  carried by `VideoFrame`; standalone callers can still reach
  `encode_pcx_8bpp_indexed` / `encode_pcx_4bpp_packed` /
  `encode_pcx_2bpp_cga` / `encode_pcx_1bpp_2planes_cga` /
  `encode_pcx_1bpp_4planes_ega` directly with an explicit palette
  argument.
* The framework `Encoder` always emits the 24-bit planar form for RGB
  input (predictable bytes for pipeline consumers). A standalone caller
  that wants the smallest lossless file without pre-choosing a mode can
  use `encode_pcx_rgb_auto` / `encode_pcx_image_auto`, whose r401
  candidate ladder covers every compact spec mode (mono, both CGA
  layouts, EGA-RGB, both 16-colour header-palette layouts, grayscale,
  indexed, planar) and provably returns the fewest-byte exact file.
* Interop caveat, not a capability gap: the `Gray8` rung's tail-less
  `palette_info = 2` form is spec-valid but some mainstream readers
  unconditionally demand the appended VGA tail on 8 bpp files and
  refuse it (the EGFF cross-reference notes most programs ignore the
  grayscale flag). Callers targeting such readers can emit the ramp
  through `encode_pcx_8bpp_indexed` — the Indexed8 rung is exactly
  that file, 769 bytes larger.
* Interop caveat, not a capability gap: on 1 bpp × 1 plane files at
  least one mainstream reader hard-codes the opposite bit polarity
  (bit 1 = black) and ignores the two-entry colormap entirely, in
  both its reader and its writer. The reference doc's errata
  (Issue #227) pins the conformant reading this crate implements —
  bit = colormap index, bit 1 = white — so such a reader shows our
  mono files inverted and vice versa (measured black-box; the bit
  *geometry* round-trips exactly, only the global polarity differs —
  see `magick_confirms_mono_bit_geometry_modulo_polarity` in
  `tests/cross_validate.rs`). Callers targeting such readers can
  route bilevel content through the Indexed4 / Indexed8 palette
  forms, which those readers resolve through the palette as written.
