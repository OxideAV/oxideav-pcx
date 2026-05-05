//! Pure-Rust ZSoft PCX (PC Paintbrush) reader/writer.
//!
//! Clean-room implementation of the public **ZSoft PCX File Format
//! Technical Reference Manual**, Revision 5 (1991). No `image` crate's
//! PCX submodule, GIMP PCX plugin, FreeImage, DevIL, or libpcx source
//! was consulted, paraphrased, or cross-checked.
//!
//! ## Read coverage
//!
//! | bits/pixel | n_planes | Source meaning              | Output |
//! | ---------- | -------- | --------------------------- | ------ |
//! | 1          | 1        | Monochrome (1-bit)          | `Rgba` |
//! | 1          | 4        | 16-colour EGA               | `Rgba` |
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
//! * [`encode_pcx_8bpp_indexed`] — 8 bpp × 1 plane, plus an appended
//!   256-entry VGA palette.
//! * [`encode_pcx_24bpp`] — 8 bpp × 3 planes, planar RGB. No tail
//!   palette.
//!
//! Both write PCX 5.0 with `bytes_per_line` rounded up to even per
//! spec §1; RLE escapes any literal byte ≥ `0xC0` even when its run
//! length is 1.
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

pub use decoder::parse_pcx;
pub use encoder::{encode_pcx_24bpp, encode_pcx_24bpp_image, encode_pcx_8bpp_indexed};
pub use error::{PcxError, Result};
pub use image::{PcxImage, PcxPixelFormat};
pub use types::{
    find_vga_palette, parse_header, PcxHeader, PCX_ENCODING_RLE, PCX_HEADER_SIZE, PCX_MANUFACTURER,
    PCX_VGA_PALETTE_BLOCK_BYTES, PCX_VGA_PALETTE_BYTES, PCX_VGA_PALETTE_MARKER,
};

#[cfg(feature = "registry")]
pub use registry::{register, register_codecs, register_containers};
