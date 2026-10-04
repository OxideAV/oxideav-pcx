# oxideav-pcx

[![CI](https://github.com/OxideAV/oxideav-pcx/actions/workflows/ci.yml/badge.svg)](https://github.com/OxideAV/oxideav-pcx/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/oxideav-pcx.svg)](https://crates.io/crates/oxideav-pcx) [![docs.rs](https://docs.rs/oxideav-pcx/badge.svg)](https://docs.rs/oxideav-pcx) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pure-Rust ZSoft PCX (PC Paintbrush) reader/writer for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework — also usable
as a plain image library with zero framework dependencies.

Clean-room implementation of the public **ZSoft PCX File Format
Technical Reference Manual**, Revision 5 (1991), the sole source of
truth for bitstream behaviour in this crate. Every `(bits/pixel,
planes)` mode of the spec decodes and encodes, plus the DCX multi-page
bundle.

## Standalone use

`oxideav-pcx` follows the OxideAV image-crate contract
(`IMAGE_CRATE_API`): the same small root vocabulary every
`oxideav-<format>` image crate exposes, usable with
`default-features = false` and no `oxideav-core`, returning pixels as
plain `Vec<u8>`.

```toml
[dependencies]
oxideav-pcx = { version = "0.1", default-features = false }
```

```rust
let bytes = std::fs::read("in.pcx")?;
if oxideav_pcx::probe(&bytes) {
    let info  = oxideav_pcx::info(&bytes)?;         // header only: width, height, format, layout
    let img   = oxideav_pcx::decode(&bytes)?;       // PcxImage, native layout (Pal8 / Gray8 / Rgb24)
    let rgba: Vec<u8> = img.to_rgba8();             // tightly packed RGBA, 4 * width bytes per row
    let (w, h) = (img.width(), img.height());

    let opts = oxideav_pcx::EncodeOptions::default().with_dpi((300, 300));
    let out: Vec<u8> = oxideav_pcx::encode_rgba8(w, h, &rgba, &opts)?;   // alpha dropped (PCX has none)
    std::fs::write("out.pcx", out)?;
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

| Item | Signature |
|---|---|
| `probe` | `fn(&[u8]) -> bool` — header plausibility sniff (manufacturer `0x0A`, version, encoding, geometry) or the DCX magic; allocation-free |
| `info` | `fn(&[u8]) -> Result<ImageInfo, Error>` — `width`, `height`, `format`, `frames`, `has_alpha`, `color`, `has_icc` / `has_exif` / `has_xmp`, plus `layout`, `version`, `bits_per_pixel`, `n_planes`, `bytes_per_line`, `palette_info`, `has_vga_palette`, `dpi`, `window_origin`, `screen_size` |
| `header` | `fn(&[u8]) -> Result<PcxHeader, Error>` — the validated 128-byte header, field by field |
| `decode` / `decode_with` | `fn(&[u8][, &DecodeOptions]) -> Result<PcxImage, Error>` — native layout, palette attached; a DCX bundle yields its first page |
| `decode_rgb8` / `decode_rgba8` | `-> Result<RgbImage / RgbaImage, Error>` — `{ width, height, data }`, tightly packed, 3 / 4 bytes per pixel |
| `decode_all` / `decode_all_with` | `-> Result<Vec<Frame>, Error>` — `Frame { image, delay: None, index }`; one frame for a PCX, one per page for a DCX bundle |
| `decode_from` | `fn<R: Read>(R) -> Result<PcxImage, Error>` |
| `encode` | `fn(&PcxImage, &EncodeOptions) -> Result<Vec<u8>, Error>` — the image's own geometry, never a silent conversion |
| `encode_rgb8` / `encode_rgba8` | `fn(w, h, &[u8], &EncodeOptions)` — 24-bit (8 bpp × 3 planes); `encode_rgba8` drops alpha |
| `encode_all` | `fn(&[Frame], &EncodeOptions) -> Result<Vec<u8>, Error>` — a DCX bundle, one page per frame (each written as `encode` would); the mirror of `decode_all` |
| `encode_to` | `fn<W: Write>(&PcxImage, &EncodeOptions, W) -> Result<(), Error>` |
| `PcxImage` | `{ width, height, format: PixelFormat, planes: Vec<Plane>, color: ColorInfo, metadata: Metadata, palette: Option<Palette>, dpi, window_origin, screen_size: Option<(u16, u16)>, layout: Option<PcxLayout> }` with `new` / `new_indexed` / `packed` / `from_rgb8` / `from_rgba8` / `from_gray8` (all `-> Result`), `width()` / `height()` / `format()` / `stride()`, `as_bytes()` / `into_raw()`, `to_rgb8()` / `to_rgba8()` (+ `try_` variants), `to_indexed()`, `into_legacy_layout()` |
| `PixelFormat` | `= PcxPixelFormat`: `Pal8`, `Gray8`, `Rgb24`, `Rgba` (input only — names mirror `oxideav_core::PixelFormat`) |
| `PcxLayout` | the on-disk geometry: `Mono1`, `Cga2x1`, `Cga1x2`, `EgaRgb1x3`, `Indexed4`, `Indexed1x4`, `Indexed8`, `Gray8`, `Rgb24` |
| `Error` | `= PcxError`: `InvalidData`, `Unsupported`, `LimitExceeded`, `Io(std::io::Error)` |

`to_rgba8` is an exact integer kernel per layout: `Pal8` looked up in
the palette (every PCX palette entry is opaque), `Gray8` replicated to
R = G = B, `Rgb24` widened with alpha `255`. No colour management is
applied. It reproduces the pre-contract `parse_pcx` flatten byte for
byte.

The pre-contract names — `parse_pcx`, `parse_pcx_cga_cpi`,
`parse_header`, `parse_dcx` / `DcxImage`, every `encode_pcx_*` writer
(`_dpi` / `_window` / `_screen` / `_image` / `_auto` variants included),
`PcxAutoMode` and `register_runtime` — remain for one release as
`#[deprecated]` thin wrappers with byte-identical output (pinned by
`tests/golden/`). The typed paletted accessors (`parse_pcx_indexed_*`),
`encode_pcx_4bpp_4planes`, `encode_dcx` / `parse_offset_table` and the
EGA quantisation helpers are depth APIs below the contract and keep
their names.

## Framework use

With the default-on `registry` feature the crate plugs into the
`oxideav-core` registry; normally `oxideav_meta::register_all(&mut ctx)`
does this for every enabled sibling, and `.pcx` / `.pcc` / `.dcx` files
are probed and routed by extension *and* magic through the generic
demux → decode flow:

```rust
# let img = oxideav_pcx::decode(&std::fs::read("in.pcx")?)?;
# let mut params = oxideav_core::CodecParameters::video(oxideav_core::CodecId::new("pcx"));
# params.width = Some(img.width());
# params.height = Some(img.height());
# params.pixel_format = Some(img.format().into());
let mut ctx = oxideav_core::RuntimeContext::new();
oxideav_pcx::register(&mut ctx);                       // codec "pcx" + the PCX and DCX containers
let dec = oxideav_pcx::make_decoder(&params)?;         // / make_encoder
let frame: oxideav_core::VideoFrame = img.into();      // From<PcxImage>: plane + palette side-channel
let back = oxideav_pcx::PcxImage::from_video_frame(&frame, &params)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

The trait-side `Decoder` / `Encoder` are thin adapters over the
standalone functions (one implementation). The decoder emits the
**native layout** — `Pal8` indices with the file's palette on the
`VideoFrame` palette side-channel for every sub-24-bit geometry,
`Gray8` for grayscale files, `Rgb24` for 24-bit files — and the
demuxers declare that layout in the stream parameters. The encoder
accepts `Rgb24`, `Gray8`, `Pal8` (palette on the side-channel) 1:1,
plus `Rgba` / `Bgra` (alpha dropped), `Bgr24` (swapped) and
`MonoBlack` / `MonoWhite` (unpacked to a black / white `Pal8` that
writes as 1 bpp). `register_codecs` / `register_containers` /
`register_registries` serve callers holding the sub-registries.

## Supported layouts

### Decode

| bits/pixel | n_planes | Source meaning | `PcxLayout` | Native layout |
| --- | --- | --- | --- | --- |
| 1 | 1 | Monochrome | `Mono1` | `Pal8` + 2 entries (header colormap 0 / 1, or black / white when zero-filled) |
| 1 | 2 | 4-colour CGA, planar | `Cga1x2` | `Pal8` + 4 entries (CGA colour map: background nibble + C / P / I selector) |
| 1 | 3 | 8-colour EGA RGB | `EgaRgb1x3` | `Pal8` + the 8 on/off primaries |
| 1 | 4 | 16-colour EGA bit-planes | `Indexed1x4` | `Pal8` + 16 entries (header colormap, or the EGA hardware default when zero-filled) |
| 2 | 1 | 4-colour CGA, packed | `Cga2x1` | `Pal8` + 4 entries |
| 4 | 1 | 16-colour packed nibbles | `Indexed4` | `Pal8` + 16 entries |
| 8 | 1 | 256-colour, VGA tail present | `Indexed8` | `Pal8` + 256 entries |
| 8 | 1 | `palette_info = 2`, or no VGA tail | `Gray8` | `Gray8` (the pixel byte is the grey level) |
| 8 | 3 | 24-bit RGB | `Rgb24` | `Rgb24` |

The `4 bpp × 4 planes` composite slot (no spec palette geometry) is
`Error::Unsupported` for `decode`; `parse_pcx_indexed_4bpp_4planes`
hands out its raw `u16` indices. Over-padded `bytes_per_line` is
honoured as trailing scanline padding and stripped; the plane stride
is always `width × bytes_per_pixel`.

### Encode

| Image | Geometry written |
| --- | --- |
| `Rgb24` | `Rgb24` |
| `Gray8` | `Gray8` (`palette_info = 2`, grey-ramp VGA tail unless `gray_tail = false`) |
| `Pal8`, palette `[black, white]` | `Mono1` |
| `Pal8`, ≤ 4 entries all in one CGA hardware palette | `Cga2x1` |
| `Pal8`, exactly the 8 primaries in index order | `EgaRgb1x3` |
| `Pal8`, ≤ 16 entries (not all zero) | `Indexed4` |
| `Pal8`, ≤ 256 entries | `Indexed8` |
| `Rgba` | `Error::Unsupported` unless `drop_alpha` → `Rgb24` |

An image decoded by this crate carries its source geometry on
`PcxImage::layout` and is written back in it, so `decode(encode(img))
== img` holds for everything `decode` produces (planes, palette, colour,
metadata and header extras; pinned in `tests/contract.rs`). A caller
palette shorter than the geometry's table (2 / 4 / 8 / 16 / 256
entries) is zero-padded on disk and reads back at the geometry's
length. `EncodeOptions::layout` forces any geometry; an image that does
not fit it losslessly is `Error::Unsupported`, never quantised.

## Options

`DecodeOptions` (`Default` + `with_*`): `max_width`, `max_height`,
`max_pixels`, `max_bytes` (the RLE-expanded planar buffer and the native
output plane must each fit; default 1 GiB, `None` lifts it) — all
checked against the header before any allocation
(`Error::LimitExceeded`) — and `strict` (default `false`): in lenient
mode the RLE stream is consumed as one run over the whole image (the
manual's own reader), an odd `bytes_per_line` and a non-zero reserved
byte are ignored; in strict mode a run packet crossing a scanline
boundary, an odd `bytes_per_line` ("MUST be even") and a non-zero
reserved byte are `Error::InvalidData`. Manufacturer, version,
encoding, geometry and the decompression-bomb cap are enforced in both
modes.

`EncodeOptions` (`Default` + `with_*`): `layout: Option<PcxLayout>`
(force a geometry), `compact` (try every lossless geometry — monochrome,
both CGA layouts, EGA RGB, both 16-colour layouts, grayscale, indexed,
24-bit — and keep the fewest bytes; ties keep the earlier), `dpi`,
`window_origin`, `screen_size` (header overrides; `None` writes the
image's own values, else `0` = unset), `version` (default 5),
`drop_alpha` (default `false`), `gray_tail` (default `true`: `Gray8`
files carry the 256-entry grey-ramp VGA block as well as the
`palette_info = 2` flag, because the black-box reader rejects any
8 bpp × 1 plane file without a tail; the flag wins on decode either
way). PCX defines a single encoding byte value (`1`, run-length), so
there is no RLE toggle.

## Metadata and colour

PCX has no ICC / Exif / XMP / gamma carrier: `PcxImage::metadata` is
always empty on decode and ignored on encode. The header's own
annotations are typed extras — `dpi` (`h_dpi` / `v_dpi`, `Some` iff
both non-zero), `window_origin` (`x_min` / `y_min`, `Some` iff either
non-zero; header metadata only, the pixel buffer is never shifted) and
`screen_size` (PB IV `h_screen_size` / `v_screen_size`, `Some` iff both
non-zero) — and round-trip through `encode`.

PCX signals no colour space, so `PcxImage::color` is always
`ColorInfo::pcx_default()` = full-range RGB (`matrix` 0) with
unspecified primaries and transfer (H.273 code point 2). This is the
crate's documented convention, not a value read from the file. The
registry frame bridge stamps a colour-signal side-channel only when a
caller-built image carries more than that default.

## Limits

Every function returns `Error` on hostile input, never panics (fuzzed:
`probe` / `info` / `header` / `decode` / `decode_with` / `decode_rgb8`
/ `decode_rgba8` / `decode_all` plus the typed accessors, and the
encode → decode seam). `DecodeOptions` limits fire before allocation;
the RLE expansion is bounded by what the input bytes can possibly back
(63 output bytes per input byte); the VGA tail probe is confined to
`8 bpp × 1 plane`, the only geometry the spec defines it for, so a
coincidental `0x0C` byte 769 bytes from the end of another mode is
never mis-framed. Dimensions are `u16` on disk; `encode` refuses larger
images with `Error::Unsupported`.

## PCX specifics

### Typed paletted views

When you want the *indices* and palette-source provenance of one
geometry, every paletted mode has a typed accessor:

| Mode | Accessor | Returns |
| --- | --- | --- |
| 8 bpp × 1 | `parse_pcx_indexed_8bpp` | `PcxIndexed8` (+ `PcxPaletteSource`: grayscale flag / VGA tail / ramp fallback) |
| 4 bpp × 1 | `parse_pcx_indexed_4bpp` / `_ega_hw` | `PcxIndexed4` (+ `Pcx4bppPaletteSource`) |
| 4 bpp × 4 | `parse_pcx_indexed_4bpp_4planes` | `PcxIndexed4x4` (`u16` composite indices, no palette) |
| 1 bpp × 4 | `parse_pcx_indexed_1bpp_4planes` | `PcxIndexed1x4` |
| 1 bpp × 3 | `parse_pcx_indexed_1bpp_3planes` | `PcxIndexed1x3` |
| 1 bpp × 2 | `parse_pcx_indexed_1bpp_2planes_cga` | `PcxIndexed1x2Cga` |
| 2 bpp × 1 | `parse_pcx_indexed_2bpp_cga[_cpi]` | `PcxIndexed2x1Cga` / `PcxIndexed2x1CgaCpi` |

### DCX multi-page bundles

`decode_all` / `encode_all` handle the Microsoft FAX multi-page wrapper
(4-byte magic `0x3ADE68B1`, up to `DCX_MAX_PAGES` = 1023 single-page
PCX members); `probe` / `info` / `decode` accept a bundle too, and
`decode_all(encode_all(frames)) == frames` (page `layout` aside, which
is `None` on a caller-built image). `encode_dcx` is the byte-level
depth alias that wraps already-encoded PCX streams. The
framework side registers it as its own container, so a DCX demuxes as
one video stream with one packet per page.

### Format notes (interop behaviour worth knowing)

* **Monochrome polarity is bit 1 = white**, resolved through a
  non-zero colormap's first two triples (so a foreign white-on-blue
  mono file decodes faithfully); the reference doc's errata (Issue
  #227) pins exactly this reading. `encode` writes an RGB / grey
  black-and-white image with the canonical black / white colormap and
  stores a two-entry `Pal8` palette verbatim.
* **CGA palettes** follow the manual's C / P / I decomposition of
  header byte 19 with the background colour from byte 16's high nibble.
* **All-zero header EGA palettes** (common in PCX 3.0+ files) fall back
  to the standard hardware EGA palette per spec table §3.1; the encoder
  therefore never writes an all-zero 16-entry colormap (all-black tables
  take the VGA tail).
* **`palette_info = 2`** (spec §3 grayscale flag) is honoured on
  8 bpp × 1 decode even when a tail palette is also present. The
  black-box reader (ImageMagick) rejects a tail-less 8 bpp file
  outright, hence the `gray_tail` default; it also reads monochrome
  files with the opposite bit polarity (docs erratum #246), which
  `tests/cross_validate.rs` canaries.
* **A tail-less foreign 8 bpp file** whose last 769 RLE bytes happen to
  start with `0x0C` is indistinguishable from a tail-palette file by
  framing alone; the decoder follows the spec's tail rule.

## Validation

Round trips are pinned per geometry; the pre-contract writers and
reader are pinned byte-for-byte against fixtures generated from the
previous release (`tests/golden/`); encoder output is cross-validated
pixel-exactly against an independent black-box reader
(`tests/cross_validate.rs`); two fuzz targets (decode with the contract
round trip, encode with semantic oracles) run continuously in CI with
seed corpora, and Criterion benches track the RLE and planar-repack hot
paths. Details live in `fuzz/` and `benches/`.

## License

MIT — see [LICENSE](LICENSE).
