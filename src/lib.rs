//! Pure-Rust ZSoft PCX (PC Paintbrush) reader/writer.
//!
//! Clean-room implementation of the public **ZSoft PCX File Format
//! Technical Reference Manual**, Revision 5 (1991), the sole source
//! of truth for bitstream behaviour in this crate.
//!
//! ## Read coverage
//!
//! | bits/pixel | n_planes | Source meaning              | Output |
//! | ---------- | -------- | --------------------------- | ------ |
//! | 1          | 1        | Monochrome (1-bit)          | `Rgba` |
//! | 1          | 2        | 4-colour CGA (planar)       | `Rgba` |
//! | 1          | 3        | 8-colour EGA RGB            | `Rgba` |
//! | 1          | 4        | 16-colour EGA               | `Rgba` |
//! | 2          | 1        | 4-colour CGA (packed)       | `Rgba` |
//! | 4          | 1        | 16-colour packed-bits       | `Rgba` |
//! | 8          | 1        | 256-colour palette (VGA tail) | `Rgba` |
//! | 8          | 3        | 24-bit RGB (planar)         | `Rgba` |
//!
//! Per-row layout is planar (each scanline is `n_planes ×
//! bytes_per_line` bytes; planes are laid out one after the other
//! within the row, NOT interleaved per pixel). The decoder re-packs
//! planes into packed RGBA at decode time.
//!
//! The optional 256-colour VGA palette block (PCX 3.0+, marker `0x0C`
//! at byte `len - 769`) is honoured for 8 bpp × 1 plane images.
//!
//! ## Write coverage
//!
//! * [`encode_pcx_8bpp_indexed`] — 8 bpp × 1 plane plus a 256-entry
//!   VGA tail palette.
//! * [`encode_pcx_24bpp`] — 8 bpp × 3 planes, planar RGB. No tail
//!   palette.
//! * [`encode_pcx_1bpp_mono`] — 1 bpp × 1 plane monochrome.
//! * [`encode_pcx_1bpp_3planes_ega_rgb`] — 8-colour EGA RGB at 1 bpp ×
//!   3 planes (each input channel thresholded at 0x80 into one
//!   bit-plane).
//! * [`encode_pcx_1bpp_4planes_ega`] — 16-colour EGA at 1 bpp × 4
//!   planes.
//! * [`encode_pcx_4bpp_packed`] — 16-colour packed-bits at 4 bpp ×
//!   1 plane.
//! * [`encode_pcx_2bpp_cga`] — 4-colour CGA packed-bits at 2 bpp ×
//!   1 plane.
//! * [`encode_pcx_1bpp_2planes_cga`] — 4-colour CGA in the plane-
//!   oriented 1 bpp × 2 planes layout (EGFF canonical CGA mode).
//! * [`encode_pcx_8bpp_grayscale`] — 8 bpp × 1 plane grayscale with
//!   spec §3 `palette_info = 2` flag set, no tail palette appended.
//! * [`encode_pcx_24bpp_window`] — like [`encode_pcx_24bpp`] but lets
//!   the caller set a non-zero `(x_min, y_min)` window origin for the
//!   PCX 3.0+ pixel-region edge case.
//!
//! All writers emit PCX 5.0 with `bytes_per_line` rounded up to even
//! per spec §1; RLE escapes any literal byte ≥ `0xC0` even when its
//! run length is 1.
//!
//! ## Authoring DPI
//!
//! The header's `h_dpi` / `v_dpi` words (offsets 12 / 14) record what
//! spec §3 calls "the resolutions at which the image was created
//! (printer or scanner); e.g. a scan might store 300, 300". The
//! decoder surfaces these on [`PcxImage::dpi`] as `Option<(u16,
//! u16)>` (`Some` iff both fields are non-zero — a 0 means "unset"
//! per the spec §3 sentinel). The plain `encode_pcx_*` writers stamp
//! the historical 72×72 PC Paintbrush convention; the matching
//! [`encode_pcx_24bpp_dpi`] / [`encode_pcx_8bpp_indexed_dpi`] /
//! [`encode_pcx_8bpp_grayscale_dpi`] / [`encode_pcx_1bpp_mono_dpi`]
//! variants take a `(h, v)` tuple instead so a scanner DPI round-
//! trips through decode + re-encode without loss. The convenience
//! wrapper [`encode_pcx_24bpp_image`] automatically threads
//! `PcxImage::dpi` through when set.
//!
//! ## Window origin
//!
//! The header's `x_min` / `y_min` words (offsets 4 / 6) record the
//! source crop region the pixel buffer came from. Spec §3 derives the
//! visible width / height as `x_max - x_min + 1` / `y_max - y_min +
//! 1`; PCX 3.0+ supports a non-zero origin so an editor can preserve
//! the position of a cropped sub-image inside its parent canvas. The
//! decoder surfaces this on [`PcxImage::window_origin`] as
//! `Option<(u16, u16)>` (`Some` whenever either header word is
//! non-zero, `None` for the conventional zero-origin screen-author
//! case). [`encode_pcx_24bpp_window_dpi`] combines the existing
//! window-only and DPI-only writers into one call so the wrapper can
//! round-trip both metadata fields at once; [`encode_pcx_24bpp_image`]
//! dispatches across the eight `(window_origin, dpi, screen_size)`
//! combinations automatically.
//!
//! ## Authoring screen size
//!
//! The header's `h_screen_size` / `v_screen_size` words (offsets 70 /
//! 72) record what spec §3 describes as "Horizontal / Vertical screen
//! size in pixels (new field found only in PB IV / IV Plus)" — an
//! authoring-time annotation distinct from the printer / scanner DPI.
//! The decoder surfaces this on [`PcxImage::screen_size`] as
//! `Option<(u16, u16)>` (`Some` iff both header words are non-zero;
//! `None` otherwise per the spec §3 sentinel — a 0 in either component
//! means "unset", which pre-PB-IV writers leave the field at). The
//! [`encode_pcx_24bpp_screen`] writer (screen-only) and
//! [`encode_pcx_24bpp_window_dpi_screen`] writer (maximally-tagged)
//! stamp a non-zero pair into the header; the convenience wrapper
//! [`encode_pcx_24bpp_image`] threads `PcxImage::screen_size` through
//! when set.
//!
//! ## Typed paletted views (8 bpp × 1 plane, 4 bpp × 1 plane, 1 bpp × 4 planes)
//!
//! [`parse_pcx`] always flattens to packed `Rgba` — convenient for
//! display pipelines but discards the on-disk palette indices.
//! [`parse_pcx_indexed_8bpp`] is the typed accessor for the 8 bpp × 1
//! plane (256-colour) case: it returns a [`PcxIndexed8`] carrying the
//! raw `width × height` index buffer (one byte per pixel, top-down,
//! padding bytes stripped) plus the resolved 256-entry RGB palette and
//! a [`PcxPaletteSource`] tag recording which spec §3 branch produced
//! it (`palette_info = 2` grayscale flag, optional VGA tail block,
//! grayscale-ramp fallback). Useful for round-tripping a paletted PCX
//! through [`encode_pcx_8bpp_indexed`] without re-quantising.
//!
//! [`parse_pcx_indexed_4bpp`] is the symmetric typed accessor for the
//! 4 bpp × 1 plane (16-colour packed-bits) case listed in EGFF table
//! entry "4 bpp / 1 plane / 16 colours / EGA and VGA". It returns a
//! [`PcxIndexed4`] carrying the unpacked `width × height` nibble
//! indices (one byte per pixel, low nibble = palette index `0..=15`)
//! plus the resolved 16-entry palette and a [`Pcx4bppPaletteSource`]
//! tag recording whether the header `ega_palette` field carried
//! non-zero bytes or the spec table §3.1 hardware default was
//! substituted. Useful for round-tripping a 16-colour PCX through
//! [`encode_pcx_4bpp_packed`] without re-quantising.
//!
//! [`parse_pcx_indexed_1bpp_4planes`] is the third 16-colour typed
//! accessor — covering the alternate spec §4.1 on-disk layout where the
//! per-pixel index is split across four 1-bit planes laid out one after
//! another within each scanline (plane 0, plane 1, plane 2, plane 3 —
//! the same plane order [`encode_pcx_1bpp_4planes_ega`] writes). It
//! returns a [`PcxIndexed1x4`] carrying the resolved `width × height`
//! 4-bit indices alongside the same 16-entry RGB palette + a
//! [`Pcx1bpp4PlanesPaletteSource`] tag. Useful for round-tripping a
//! 16-colour EGA PCX through [`encode_pcx_1bpp_4planes_ega`] without
//! re-quantising.
//!
//! [`parse_pcx_indexed_2bpp_cga`] is the typed accessor for the
//! 4-colour CGA mode (2 bpp × 1 plane, 4 pixels/byte). It returns a
//! [`PcxIndexed2x1Cga`] carrying the unpacked `width × height` 2-bit
//! indices (low two bits = palette index `0..=3`) alongside the
//! resolved 4-entry RGB palette, the resolved `background_index`
//! (`0..=15`) read from `ega_palette[16]`'s high nibble, and a
//! [`Pcx2bppCgaPaletteSource`] tag recording which CGA palette family
//! (palette 0 / 1 × low / high intensity) the decoder landed on. The
//! [`Pcx2bppCgaPaletteSource::palette_selector`] helper reconstructs the
//! byte 19 selector pattern so a round-trip caller can hand it
//! straight back to [`encode_pcx_2bpp_cga`] without re-deriving the
//! bit positions.
//!
//! [`parse_pcx_indexed_1bpp_3planes`] is the fifth (and final) paletted
//! typed view — covering the 8-colour EGA RGB mode described in spec §4
//! where each scanline carries three 1-bit planes (plane order R, G, B,
//! the same order [`encode_pcx_1bpp_3planes_ega_rgb`] writes). The three
//! bits at the same x-position stack into a 3-bit colour index
//! (`r | g << 1 | b << 2`). It returns a [`PcxIndexed1x3`] carrying one
//! byte per pixel (low three bits = colour index `0..=7`, top-down,
//! padding stripped) alongside the fixed 8-entry on/off-primary RGB
//! palette + a [`Pcx1bpp3PlanesPaletteSource`] tag. Unlike the other
//! paletted modes this carries no on-disk palette — the eight colours
//! are intrinsic to the plane bits — so the source tag has a single
//! [`Pcx1bpp3PlanesPaletteSource::FixedPrimaries`] arm.
//!
//! [`parse_pcx_cga_cpi`] is the spec-faithful *flatten*-to-`Rgba` sibling
//! of [`parse_pcx_indexed_2bpp_cga_cpi`]: it resolves the 4-colour CGA
//! palette through the full C / P / I decomposition of header byte 19
//! (incl. the color-burst monochrome composite-grey ramp, the mode the
//! legacy [`parse_pcx`] flatten path cannot express), across both the
//! `2 bpp × 1 plane` packed and `1 bpp × 2 planes` planar CGA layouts.
//!
//! ## DCX multi-page bundles
//!
//! [`parse_dcx`] / [`encode_dcx`] handle the Microsoft FAX multi-page
//! wrapper: 4-byte LE magic [`DCX_MAGIC`] (`0x3ADE_68B1`) + up to 1023
//! u32 LE page offsets terminated by a zero sentinel + concatenated
//! stand-alone PCX 5.0 streams.
//!
//! ## Framework `Encoder` accepted pixel formats
//!
//! The default-on `registry` feature's `make_encoder` accepts seven
//! `oxideav_core::PixelFormat` variants. `Rgba` / `Rgb24` / `Bgr24` /
//! `Bgra` route to [`encode_pcx_24bpp`] (with per-pixel byte swap for
//! the `Bgr*` variants, and alpha dropped from `Rgba` / `Bgra`).
//! `Gray8` routes to [`encode_pcx_8bpp_grayscale`]. `MonoBlack` and
//! `MonoWhite` unpack the MSB-first 1-bit stride into one byte per
//! pixel and route to [`encode_pcx_1bpp_mono`]; `MonoWhite` is
//! bit-inverted on the way in so the on-disk PCX always carries the
//! spec §4.1 bit-1 = white polarity.
//!
//! ## Standalone vs registry-integrated
//!
//! The crate's default `registry` Cargo feature pulls in `oxideav-core`
//! and exposes the framework `Decoder` / `Encoder` trait surface plus
//! a [`registry::register`] entry point. Disable the feature
//! (`default-features = false`) for an `oxideav-core`-free build that
//! still exposes the standalone [`parse_pcx`] / [`encode_pcx_8bpp_indexed`]
//! / [`encode_pcx_24bpp`] API plus crate-local [`PcxImage`] /
//! [`PcxPixelFormat`] / [`PcxError`] types.

#[cfg(feature = "registry")]
pub mod container;
pub mod dcx;
#[cfg(feature = "registry")]
pub mod dcx_container;
pub mod decoder;
pub mod encoder;
pub mod error;
pub mod image;
#[cfg(feature = "registry")]
pub mod registry;
pub mod rle;
pub mod types;

/// Codec id for PCX image frames.
pub const CODEC_ID_STR: &str = "pcx";

pub use dcx::{encode_dcx, parse_dcx, DcxImage, DCX_MAGIC, DCX_MAX_PAGES};
#[doc(hidden)]
pub use decoder::__bench_decode_planar_len;
pub use decoder::{
    ega_quantize_component, ega_quantize_level, ega_quantize_palette, parse_pcx, parse_pcx_cga_cpi,
    parse_pcx_indexed_1bpp_2planes_cga, parse_pcx_indexed_1bpp_3planes,
    parse_pcx_indexed_1bpp_4planes, parse_pcx_indexed_2bpp_cga, parse_pcx_indexed_2bpp_cga_cpi,
    parse_pcx_indexed_4bpp, parse_pcx_indexed_4bpp_4planes, parse_pcx_indexed_4bpp_ega_hw,
    parse_pcx_indexed_8bpp,
};
pub use encoder::{
    encode_pcx_1bpp_2planes_cga, encode_pcx_1bpp_2planes_cga_dpi, encode_pcx_1bpp_3planes_ega_rgb,
    encode_pcx_1bpp_3planes_ega_rgb_dpi, encode_pcx_1bpp_4planes_ega,
    encode_pcx_1bpp_4planes_ega_dpi, encode_pcx_1bpp_mono, encode_pcx_1bpp_mono_dpi,
    encode_pcx_24bpp, encode_pcx_24bpp_dpi, encode_pcx_24bpp_image, encode_pcx_24bpp_screen,
    encode_pcx_24bpp_window, encode_pcx_24bpp_window_dpi, encode_pcx_24bpp_window_dpi_screen,
    encode_pcx_2bpp_cga, encode_pcx_2bpp_cga_cpi, encode_pcx_2bpp_cga_dpi, encode_pcx_4bpp_4planes,
    encode_pcx_4bpp_packed, encode_pcx_4bpp_packed_dpi, encode_pcx_8bpp_grayscale,
    encode_pcx_8bpp_grayscale_dpi, encode_pcx_8bpp_indexed, encode_pcx_8bpp_indexed_dpi,
    encode_pcx_image_auto, encode_pcx_rgb_auto, PcxAutoMode,
};
pub use error::{PcxError, Result};
pub use image::{
    Pcx1bpp3PlanesPaletteSource, Pcx1bpp4PlanesPaletteSource, Pcx2bppCgaCpi,
    Pcx2bppCgaPaletteSource, Pcx4bppPaletteSource, PcxImage, PcxIndexed1x2Cga, PcxIndexed1x3,
    PcxIndexed1x4, PcxIndexed2x1Cga, PcxIndexed2x1CgaCpi, PcxIndexed4, PcxIndexed4x4, PcxIndexed8,
    PcxPaletteSource, PcxPixelFormat,
};
pub use types::{
    find_vga_palette, parse_header, PcxHeader, PCX_ENCODING_RLE, PCX_HEADER_SIZE, PCX_MANUFACTURER,
    PCX_VGA_PALETTE_BLOCK_BYTES, PCX_VGA_PALETTE_BYTES, PCX_VGA_PALETTE_MARKER,
};

#[cfg(feature = "registry")]
pub use registry::{register, register_codecs, register_containers};
