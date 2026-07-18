# oxideav-pcx

[![CI](https://github.com/OxideAV/oxideav-pcx/actions/workflows/ci.yml/badge.svg)](https://github.com/OxideAV/oxideav-pcx/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/oxideav-pcx.svg)](https://crates.io/crates/oxideav-pcx) [![docs.rs](https://docs.rs/oxideav-pcx/badge.svg)](https://docs.rs/oxideav-pcx) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pure-Rust ZSoft PCX (PC Paintbrush) reader/writer for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework — also usable
as a plain image library with zero framework dependencies.

Clean-room implementation of the public **ZSoft PCX File Format
Technical Reference Manual**, Revision 5 (1991), the sole source of
truth for bitstream behaviour in this crate. Every `(bits/pixel,
planes)` mode of the spec decodes and encodes; the encoder can pick the
provably smallest conformant file for you.

## Using it as a plain image library

Turn off the default `registry` feature and the crate has no
dependencies at all:

```toml
[dependencies]
oxideav-pcx = { version = "0.0", default-features = false }
```

Decode any PCX to packed RGBA:

```rust
let img = oxideav_pcx::parse_pcx(&std::fs::read("in.pcx")?)?;
// img.width / img.height / img.data (RGBA, row-major, top-left)
// img.dpi: Some((h, v)) when the file records a scan resolution
```

Encode RGB pixels to the smallest lossless PCX — the writer races every
spec mode (mono, CGA, EGA-RGB, 16-colour, grayscale, 256-indexed,
24-bit) and returns the fewest-byte exact file plus which mode won:

```rust
let (bytes, mode) = oxideav_pcx::encode_pcx_rgb_auto(w, h, &rgb)?;
// mode: PcxAutoMode::{Indexed8{colors}, Rgb24, Gray8, Mono1, ...}
std::fs::write("out.pcx", bytes)?;
```

`encode_pcx_image_auto(&PcxImage)` does the same from a decoded image
(round-trip), and `encode_pcx_indexed_auto(w, h, &indices, &palette)`
writes your palette verbatim when you already have indexed data. When
you want a *specific* on-disk geometry instead of the smallest one,
every spec mode has an explicit writer:
`encode_pcx_1bpp_mono` · `encode_pcx_2bpp_cga[_cpi]` ·
`encode_pcx_1bpp_2planes_cga` · `encode_pcx_1bpp_3planes_ega_rgb` ·
`encode_pcx_1bpp_4planes_ega` · `encode_pcx_4bpp_packed` ·
`encode_pcx_4bpp_4planes` · `encode_pcx_8bpp_grayscale` ·
`encode_pcx_8bpp_indexed` · `encode_pcx_24bpp`. Each has variants for
authoring metadata (`*_dpi`, and for 24-bit also `*_window` /
`*_screen`) — see [Authoring metadata](#authoring-metadata).

## Using it through the oxideav framework

With the default `registry` feature the crate plugs into
`oxideav-core`'s registries — this is the generic image path the
framework CLI and pipelines use (`oxideav-meta`'s `register_all` wires
it up automatically; `.pcx` / `.pcc` files are probed and routed by
extension *and* magic):

```rust
use oxideav_core::{CodecId, CodecParameters, CodecRegistry, Frame, Packet, TimeBase};

let mut reg = CodecRegistry::new();
oxideav_pcx::register_codecs(&mut reg);

// Decode: the whole file is one packet.
let params = CodecParameters::video(CodecId::new("pcx"));
let mut dec = reg.make_decoder(&params)?;
dec.send_packet(&Packet::new(0, TimeBase::new(1, 1), std::fs::read("in.pcx")?))?;
let Frame::Video(frame) = dec.receive_frame()? else { unreachable!() };
// frame.planes[0] is packed RGBA (or the format you requested — see below)
```

The registered decoder honours `CodecParameters::pixel_format` for the
output surface; the registered encoder accepts eight input formats:
`Rgba`, `Rgb24`, `Bgr24`, `Bgra`, `Gray8`, `MonoBlack`, `MonoWhite`,
and `Pal8`. Encoding from `Rgba`/`Rgb24` runs the same smallest-file
ladder as `encode_pcx_rgb_auto`; `Gray8` sets the spec grayscale flag;
the mono formats emit 1-bpp files.

**`Pal8` and the palette side-channel** (oxideav-core ≥ 0.1.30): a
`Pal8` frame carries its colour table on the `VideoFrame` palette
side-channel. The encoder reads the caller's palette off the frame and
stores it verbatim in the fewest-byte indexed mode; a `Pal8`-requested
decode returns index frames with the file's palette attached — frames
are self-describing in both directions.

`register_containers` (or the combined `register`) additionally
registers the PCX single-image container (probe + `.pcx`/`.pcc`
extensions, demuxer + muxer) and the DCX multi-page bundle, so generic
demux→decode pipelines open PCX/DCX files without format-specific code.

## Decode coverage

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

`parse_pcx` always produces packed RGBA. When you want the *indices*
and palette instead of expanded pixels, every paletted mode has a typed
accessor returning the raw index plane plus a typed palette source:

| Mode          | Accessor                             | Returns            |
| ------------- | ------------------------------------ | ------------------ |
| 8 bpp × 1     | `parse_pcx_indexed_8bpp`             | `PcxIndexed8`      |
| 4 bpp × 1     | `parse_pcx_indexed_4bpp`             | `PcxIndexed4`      |
| 4 bpp × 4     | `parse_pcx_indexed_4bpp_4planes`     | `PcxIndexed4x4`    |
| 1 bpp × 4     | `parse_pcx_indexed_1bpp_4planes`     | `PcxIndexed1x4`    |
| 1 bpp × 3     | `parse_pcx_indexed_1bpp_3planes`     | `PcxIndexed1x3`    |
| 1 bpp × 2     | `parse_pcx_indexed_1bpp_2planes_cga` | `PcxIndexed1x2Cga` |
| 2 bpp × 1     | `parse_pcx_indexed_2bpp_cga[_cpi]`   | `PcxIndexed2x1Cga` |
| CGA flattened | `parse_pcx_cga_cpi`                  | `Pcx2bppCgaCpi`    |

All views share the same shape — index plane, dimensions, stride, and a
palette-source enum telling you where the colours came from (header
palette, VGA tail, hardware default, grayscale ramp…):

```rust
let idx = oxideav_pcx::parse_pcx_indexed_8bpp(&bytes)?;
// idx.indices: one byte per pixel; idx.palette_source: PcxPaletteSource
```

See [docs.rs](https://docs.rs/oxideav-pcx) for each view's exact
palette-source semantics (including the EGA hardware-quantised
`parse_pcx_indexed_4bpp_ega_hw` variant).

## Authoring metadata

The header's authoring fields are surfaced on decode and settable on
encode without ceremony:

* **DPI** — `PcxImage::dpi` reports `(h, v)` when both header fields
  are non-zero; every explicit writer has a `*_dpi` sibling to stamp it.
* **Window origin** (`x_min`/`y_min`) and **authoring screen size** —
  readable from `parse_header`, settable through the 24-bit
  `*_window` / `*_screen` writer variants. Decode always returns the
  visible window regardless of origin.

## DCX multi-page bundles

`parse_dcx` / `encode_dcx` handle the Microsoft FAX multi-page wrapper
(up to `DCX_MAX_PAGES` = 1023 single-page PCX members); the framework
side registers it as its own container, so a DCX demuxes as one video
stream with one packet per page.

## Format notes (interop behaviour worth knowing)

* **VGA tail palette probing is confined to 8 bpp × 1 plane** — the
  only mode the spec defines it for. RLE payloads of other modes that
  coincidentally carry a `0x0C` byte 769 bytes from EOF are not
  mis-framed (the coincidental-marker hazard the EGFF cross-reference
  flags for v3.0 files).
* **Over-padded strides decode exactly.** `bytes_per_line` larger than
  the minimum is honoured as trailing scanline padding per the manual
  ("Do NOT calculate from Xmax-Xmin") and stripped on every decode
  path; a stride the RLE payload cannot possibly back is rejected, not
  allocated (decompression-bomb guard).
* **RLE decodes continuously across the whole image**, matching the
  manual's own sample reader — run packets straddling scanline, plane,
  or padding boundaries decode byte-identically to their row-broken
  equivalents. Whole-image overrun is still rejected.
* **Monochrome polarity is bit 1 = white**, resolved through a
  non-zero colormap's first two triples (so a foreign white-on-blue
  mono file decodes faithfully); the reference doc's errata (Issue
  #227) pins exactly this reading, and both directions are pinned at
  the raw-byte level (`tests/round405_mono_polarity.rs`).
* **CGA palettes** follow the manual's C / P / I decomposition of
  header byte 19 with the background colour from byte 16's high
  nibble; the spec-faithful selector is also exposed directly
  (`parse_pcx_indexed_2bpp_cga_cpi` / `parse_pcx_cga_cpi`).
* **All-zero header EGA palettes** (common in PCX 3.0+ files) fall
  back to the standard hardware EGA palette per spec table §3.1.
* **`palette_info = 2`** (spec §3 grayscale flag) is honoured on
  8 bpp × 1 decode and set by the grayscale writers.

## Validation

Round-trips are pinned index- and palette-exact; encoder output is
cross-validated pixel-exactly against an independent black-box reader;
five fuzz targets (decode, encode with semantic round-trip oracles,
header, DCX, RLE core) run continuously in CI with seed corpora, and
Criterion benches track the RLE and planar-repack hot paths. Details
live in `fuzz/` and `benches/`.

The one known interop caveat is documented in the `Gray8` rung: a
tail-less foreign 8 bpp file whose last 769 RLE bytes happen to start
with `0x0C` is indistinguishable from a tail-palette file by framing
alone; the decoder follows the spec's tail rule.

## License

MIT — see [LICENSE](LICENSE).
