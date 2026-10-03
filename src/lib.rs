//! Pure-Rust ZSoft PCX (PC Paintbrush) reader/writer.
//!
//! Clean-room implementation of the public **ZSoft PCX File Format
//! Technical Reference Manual**, Revision 5 (1991), the sole source
//! of truth for bitstream behaviour in this crate.
//!
//! The crate follows the OxideAV image-crate contract
//! (`IMAGE_CRATE_API`): the same small root vocabulary every
//! `oxideav-<format>` picture crate exposes, usable with
//! `default-features = false` and no `oxideav-core`.
//!
//! ```no_run
//! let bytes = std::fs::read("in.pcx")?;
//! if oxideav_pcx::probe(&bytes) {
//!     let info = oxideav_pcx::info(&bytes)?;        // header only
//!     let img = oxideav_pcx::decode(&bytes)?;       // PcxImage, native layout
//!     let rgba: Vec<u8> = img.to_rgba8();           // packed RGBA, 4 * width bytes per row
//!     let (w, h) = (img.width(), img.height());
//!     let _ = info;
//!
//!     let opts = oxideav_pcx::EncodeOptions::default().with_dpi((300, 300));
//!     let out = oxideav_pcx::encode_rgba8(w, h, &rgba, &opts)?;
//!     std::fs::write("out.pcx", out)?;
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Read coverage
//!
//! | bits/pixel | n_planes | Source meaning                | Native layout |
//! | ---------- | -------- | ----------------------------- | ------------- |
//! | 1          | 1        | Monochrome (1-bit)            | `Pal8` + 2-entry palette |
//! | 1          | 2        | 4-colour CGA (planar)         | `Pal8` + 4-entry palette |
//! | 1          | 3        | 8-colour EGA RGB              | `Pal8` + 8 primaries |
//! | 1          | 4        | 16-colour EGA                 | `Pal8` + 16-entry palette |
//! | 2          | 1        | 4-colour CGA (packed)         | `Pal8` + 4-entry palette |
//! | 4          | 1        | 16-colour packed-bits         | `Pal8` + 16-entry palette |
//! | 8          | 1        | 256-colour palette (VGA tail) | `Pal8` + 256-entry palette |
//! | 8          | 1        | grayscale (`palette_info = 2`, or no tail) | `Gray8` |
//! | 8          | 3        | 24-bit RGB (planar)           | `Rgb24` |
//!
//! Per-row layout on disk is planar (each scanline is `n_planes ×
//! bytes_per_line` bytes; planes are laid out one after the other
//! within the row, NOT interleaved per pixel). The decoder re-packs
//! planes into the native layout at decode time; [`PcxImage::to_rgba8`]
//! expands the palette.
//!
//! ## Write coverage
//!
//! [`encode`] writes a [`PcxImage`] in one on-disk geometry
//! ([`PcxLayout`]) — the one it was decoded from, or the natural one
//! for its layout and palette — with [`EncodeOptions`] fields for the
//! geometry, the compact ladder, authoring DPI, window origin, PB IV
//! screen size and the version byte. All writers emit `bytes_per_line`
//! rounded up to even per spec §1; RLE escapes any literal byte ≥
//! `0xC0` even when its run length is 1.
//!
//! ## DCX multi-page bundles
//!
//! [`decode_all`] / [`encode_dcx`] handle the Microsoft FAX multi-page
//! wrapper: 4-byte LE magic [`DCX_MAGIC`] (`0x3ADE_68B1`) + up to 1023
//! u32 LE page offsets terminated by a zero sentinel + concatenated
//! stand-alone PCX 5.0 streams. [`probe`] / [`info`] / [`decode`]
//! accept a bundle too (`decode` = the first page).
//!
//! ## Typed paletted views
//!
//! The `parse_pcx_indexed_*` accessors are the depth layer below the
//! contract: each returns the raw indices of one geometry plus a
//! palette-source tag recording which spec §3 branch produced the
//! palette ([`PcxIndexed8`], [`PcxIndexed4`], [`PcxIndexed1x4`],
//! [`PcxIndexed2x1Cga`], [`PcxIndexed2x1CgaCpi`], [`PcxIndexed1x2Cga`],
//! [`PcxIndexed1x3`], and the palette-less `4 bpp × 4 planes`
//! composite [`PcxIndexed4x4`] that [`decode`] rejects).
//!
//! ## Standalone vs registry-integrated
//!
//! The crate's default `registry` Cargo feature pulls in `oxideav-core`
//! and exposes the framework `Decoder` / `Encoder` trait surface plus
//! the [`register`] entry point and the [`make_decoder`] /
//! [`make_encoder`] factories. Disable the feature (`default-features =
//! false`) for an `oxideav-core`-free build that still exposes the
//! whole standalone API.

pub mod api;
#[cfg(feature = "registry")]
pub mod container;
pub mod dcx;
#[cfg(feature = "registry")]
pub mod dcx_container;
pub mod decoder;
pub mod encoder;
pub mod error;
pub mod image;
pub mod options;
#[cfg(feature = "registry")]
pub mod registry;
pub mod rle;
pub mod types;

/// Codec id for PCX image frames.
pub const CODEC_ID_STR: &str = "pcx";

// ---- The image-crate contract (IMAGE_CRATE_API) ----
pub use api::{
    decode, decode_all, decode_all_with, decode_from, decode_rgb8, decode_rgba8, decode_with,
    encode, encode_rgb8, encode_rgba8, encode_to, header, info, probe,
};
pub use error::{Error, PcxError, Result};
pub use image::{
    ColorInfo, ColorRange, Frame, ImageInfo, Metadata, Palette, PcxImage, PcxLayout,
    PcxPixelFormat, PixelFormat, Plane, RgbImage, RgbaImage,
};
pub use options::{DecodeOptions, EncodeOptions};

// ---- PCX depth: typed paletted views, header, DCX, EGA quantisation ----
pub use dcx::{encode_dcx, parse_offset_table, DCX_MAGIC, DCX_MAX_PAGES};
#[doc(hidden)]
pub use decoder::__bench_decode_planar_len;
pub use decoder::{
    ega_quantize_component, ega_quantize_level, ega_quantize_palette,
    parse_pcx_indexed_1bpp_2planes_cga, parse_pcx_indexed_1bpp_3planes,
    parse_pcx_indexed_1bpp_4planes, parse_pcx_indexed_2bpp_cga, parse_pcx_indexed_2bpp_cga_cpi,
    parse_pcx_indexed_4bpp, parse_pcx_indexed_4bpp_4planes, parse_pcx_indexed_4bpp_ega_hw,
    parse_pcx_indexed_8bpp,
};
pub use encoder::encode_pcx_4bpp_4planes;
pub use image::{
    Pcx1bpp3PlanesPaletteSource, Pcx1bpp4PlanesPaletteSource, Pcx2bppCgaCpi,
    Pcx2bppCgaPaletteSource, Pcx4bppPaletteSource, PcxIndexed1x2Cga, PcxIndexed1x3, PcxIndexed1x4,
    PcxIndexed2x1Cga, PcxIndexed2x1CgaCpi, PcxIndexed4, PcxIndexed4x4, PcxIndexed8,
    PcxPaletteSource,
};
pub use types::{
    find_vga_palette, PcxHeader, PCX_ENCODING_RLE, PCX_HEADER_SIZE, PCX_MANUFACTURER,
    PCX_VGA_PALETTE_BLOCK_BYTES, PCX_VGA_PALETTE_BYTES, PCX_VGA_PALETTE_MARKER,
};

// ---- Pre-contract names, kept for one release ----
#[allow(deprecated)]
pub use dcx::{parse_dcx, DcxImage};
#[allow(deprecated)]
pub use decoder::{parse_pcx, parse_pcx_cga_cpi};
#[allow(deprecated)]
pub use encoder::{
    encode_pcx_1bpp_2planes_cga, encode_pcx_1bpp_2planes_cga_dpi, encode_pcx_1bpp_3planes_ega_rgb,
    encode_pcx_1bpp_3planes_ega_rgb_dpi, encode_pcx_1bpp_4planes_ega,
    encode_pcx_1bpp_4planes_ega_dpi, encode_pcx_1bpp_mono, encode_pcx_1bpp_mono_dpi,
    encode_pcx_24bpp, encode_pcx_24bpp_dpi, encode_pcx_24bpp_image, encode_pcx_24bpp_screen,
    encode_pcx_24bpp_window, encode_pcx_24bpp_window_dpi, encode_pcx_24bpp_window_dpi_screen,
    encode_pcx_2bpp_cga, encode_pcx_2bpp_cga_cpi, encode_pcx_2bpp_cga_dpi, encode_pcx_4bpp_packed,
    encode_pcx_4bpp_packed_dpi, encode_pcx_8bpp_grayscale, encode_pcx_8bpp_grayscale_dpi,
    encode_pcx_8bpp_indexed, encode_pcx_8bpp_indexed_dpi, encode_pcx_image_auto,
    encode_pcx_indexed_auto, encode_pcx_rgb_auto, PcxAutoMode,
};
#[allow(deprecated)]
pub use types::parse_header;

#[cfg(feature = "registry")]
#[doc(hidden)]
pub use registry::__oxideav_entry;
#[cfg(feature = "registry")]
#[allow(deprecated)]
pub use registry::register_runtime;
#[cfg(feature = "registry")]
pub use registry::{
    make_decoder, make_encoder, register, register_codecs, register_containers, register_registries,
};
