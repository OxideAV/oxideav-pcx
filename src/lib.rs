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
pub use decoder::parse_pcx;
pub use encoder::{
    encode_pcx_1bpp_3planes_ega_rgb, encode_pcx_1bpp_4planes_ega, encode_pcx_1bpp_mono,
    encode_pcx_24bpp, encode_pcx_24bpp_image, encode_pcx_24bpp_window, encode_pcx_2bpp_cga,
    encode_pcx_4bpp_packed, encode_pcx_8bpp_grayscale, encode_pcx_8bpp_indexed,
};
pub use error::{PcxError, Result};
pub use image::{PcxImage, PcxPixelFormat};
pub use types::{
    find_vga_palette, parse_header, PcxHeader, PCX_ENCODING_RLE, PCX_HEADER_SIZE, PCX_MANUFACTURER,
    PCX_VGA_PALETTE_BLOCK_BYTES, PCX_VGA_PALETTE_BYTES, PCX_VGA_PALETTE_MARKER,
};

#[cfg(feature = "registry")]
pub use registry::{register, register_codecs, register_containers};
