# oxideav-pcx

Pure-Rust ZSoft PCX (PC Paintbrush) reader/writer for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework.

Clean-room implementation of the public **ZSoft PCX File Format
Technical Reference Manual**, Revision 5 (1991), the sole source of
truth for bitstream behaviour in this crate.

## Decode

| bits/pixel | n_planes | Source meaning                | Output |
| ---------- | -------- | ----------------------------- | ------ |
| 1          | 1        | Monochrome (1-bit)            | `Rgba` |
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
  absent, an 8-bit grayscale ramp is used as a fallback.
* Header field `palette_info` (spec §3): value `2` is the grayscale
  flag — the decoder honours it for 8 bpp × 1 plane images and
  produces a grayscale ramp regardless of any tail palette. The
  default is `1` (colour / BW).
* Header field `bytes_per_line` is range-checked against the visible
  width × `bits_per_pixel`: a value smaller than the per-plane row
  width required by the spec is rejected up front rather than
  silently mis-framing the planar→packed reconstruction.
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
  header byte 19 (bit 7 = palette select 0/1, bit 6 = intensity
  high/low) with the background colour pulled from the high nibble
  of header byte 16.

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
  (bit 1 = white, bit 0 = black).
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
* `encode_pcx_8bpp_grayscale(w, h, &pixels)` — 8 bpp × 1 plane
  grayscale with spec §3 `palette_info = 2` flag set and no tail
  palette appended. The decoder honours the flag and emits
  `(g, g, g, 0xFF)` per pixel regardless of any tail palette.
* `encode_pcx_24bpp_window(x_min, y_min, w, h, &rgb)` — like
  `encode_pcx_24bpp` but sets a non-zero `(x_min, y_min)` window
  origin for the PCX 3.0+ pixel-region edge case.

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

Round 197 (depth-mode benchmarks) adds three Criterion harnesses under
`benches/` so future optimisation rounds can A/B-test changes to the
decoder + encoder hot paths against a stable baseline:

* `decode` — drives [`parse_pcx`] / [`parse_dcx`] across every spec §4.1
  (depth, planes) tuple at 320×240 / 640×480 / 512×512 / 1920×1080
  scales plus a 4-page DCX bundle.
* `encode` — drives the eight standalone write paths
  (`encode_pcx_24bpp` / `encode_pcx_8bpp_indexed` /
  `encode_pcx_8bpp_grayscale` / `encode_pcx_1bpp_mono` /
  `encode_pcx_4bpp_packed` / `encode_pcx_2bpp_cga` /
  `encode_pcx_1bpp_4planes_ega` / `encode_dcx`).
* `roundtrip` — pairs each encode path with its matching decode so a
  perf regression that quietly mis-encodes surfaces as a panic rather
  than a silently-cheaper benchmark number.

Bench inputs are synthesised on the fly via a deterministic xorshift32
fill; no fixture files are committed. Run with:

```sh
cargo bench -p oxideav-pcx --bench decode
cargo bench -p oxideav-pcx --bench encode
cargo bench -p oxideav-pcx --bench roundtrip
```

Round 209 (depth-mode profile / optimisation) reworked the six planar
unpack hot paths in `src/decoder.rs` against the r197 baseline. Single
threaded apple-silicon medians (3 s measurement / 30 samples / fresh
target dir per side): 24-bit 1920×1080 decode 6.63 ms → 5.04 ms
(−24.0 %, 1.16 → 1.53 GiB/s); 24-bit 640×480 879 µs → 731 µs
(−16.8 %); 24-bit 320×240 206 µs → 185 µs (−10.2 %); 8-bit indexed
320×240 128 µs → 92 µs (−28.1 %, 2.24 → 3.12 GiB/s); 8-bit grayscale
512×512 491 µs → 366 µs (−25.4 %, 1.99 → 2.67 GiB/s); 1-bit
monochrome 512×512 226 µs → 182 µs (−19.5 %, 4.32 → 5.38 GiB/s); 4-bit
packed 320×240 105 µs → 74 µs (−29.4 %); 2-bit CGA 320×240 84 µs →
51 µs (−39.0 %, 3.42 → 5.59 GiB/s); 1-bit × 4-plane EGA 320×240 148 µs
→ 114 µs (−22.8 %). Geometric-mean speedup ≈ 22.6 % across the nine
single-frame paths. Output bytes stay bit-identical (cross-validate +
roundtrip tests all pass). The transformation is mechanical:
`chunks_exact_mut(w * 4)` over the destination row + per-pixel
`chunks_exact_mut(4)` for the four RGBA stores, pre-sliced per-plane
row references for the multi-plane variants, and pre-baked
`[r, g, b, 0xFF]` palettes so per-pixel palette lookups become one
`copy_from_slice` instead of three scalar byte writes plus an alpha
byte.

## Fuzzing

A [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) harness lives
under `fuzz/`. The `decode_pcx` target feeds arbitrary bytes to both
`parse_pcx` and `parse_dcx` and asserts they always return a `Result`
rather than panicking, integer-overflowing, indexing out of bounds, or
allocating an attacker-claimed pixel buffer. It is built with
`default-features = false` so it exercises the framework-free decode
path. A 12-entry seed corpus covers all six (depth, planes)
combinations, grayscale, a windowed-origin file, a DCX bundle, and
degenerate inputs.

```sh
cd fuzz && cargo +nightly fuzz run decode_pcx -- -max_total_time=60
```

The current baseline runs 40M+ executions with zero crashes. Two
hardening fixes came out of the initial run: the public
`PcxHeader::width()` / `height()` accessors now saturate instead of
underflow-panicking on an `x_max < x_min` header, and `parse_pcx` has a
decompression-bomb guard that rejects a tiny file claiming enormous
dimensions before it can reserve hundreds of gigabytes.

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

## Lacks

* 4 bpp × 4 planes (16-colour planar with finer per-plane depth)
  is the one remaining `(bpp, planes)` slot the EGFF PCX summary
  doesn't list as a formal video mode; real-world files at this
  depth are vanishingly rare and the existing 1 bpp × 4 / 4 bpp ×
  1 / 1 bpp × 3 paths cover every EGA/VGA fixture we've seen.
* `PixelFormat::Pal8` input to the framework `Encoder`. The
  out-of-band palette companion needed by `Pal8` isn't currently
  carried by `VideoFrame`; standalone callers can still reach
  `encode_pcx_8bpp_indexed` / `encode_pcx_4bpp_packed` /
  `encode_pcx_2bpp_cga` / `encode_pcx_1bpp_4planes_ega` directly
  with an explicit palette argument.
* Per-pixel-region authoring DPI override for the EGA/CGA / 4 bpp /
  1 bpp × 3 / 1 bpp × 4 writers (these still emit 72×72). The
  `_dpi` writer suite covers the four formats consumers actually
  scan into PCX (24-bit, 8 bpp indexed, 8 bpp grayscale, 1 bpp mono);
  the EGA/CGA palette-mode writers stay at the historical default
  because EGA / CGA hardware never had a non-screen authoring DPI to
  carry.
