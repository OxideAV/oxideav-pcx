//! PCX structural types.
//!
//! PCX on-disk layout (ZSoft PCX File Format Technical Reference Manual,
//! Revision 5, 1991):
//!
//! ```text
//!   [Header (128 B, fixed)]
//!     manufacturer       (u8)         — always 0x0A ("ZSoft .pcx")
//!     version            (u8)         — 0 / 2 / 3 / 4 / 5
//!     encoding           (u8)         — always 1 (RLE)
//!     bits_per_pixel     (u8)         — 1 / 2 / 4 / 8 (per plane)
//!     x_min              (u16 LE)
//!     y_min              (u16 LE)
//!     x_max              (u16 LE)
//!     y_max              (u16 LE)
//!     h_dpi              (u16 LE)
//!     v_dpi              (u16 LE)
//!     ega_palette        ([u8; 48])   — 16 RGB triplets
//!     reserved           (u8)         — 0
//!     n_planes           (u8)         — 1 / 3 / 4
//!     bytes_per_line     (u16 LE)     — bytes for ONE plane of ONE row
//!     palette_info       (u16 LE)     — 1 = colour/BW, 2 = grayscale
//!     h_screen_size      (u16 LE)     — only PCX 4.0 / 5.0
//!     v_screen_size      (u16 LE)     — only PCX 4.0 / 5.0
//!     filler             ([u8; 54])   — pad to 128 bytes
//!   [Pixel data — RLE bytestream, `n_planes * bytes_per_line` bytes
//!    decoded per scanline (planes laid one after the other within
//!    a single row, NOT interleaved per pixel).]
//!   [Optional 256-colour VGA palette — present iff version ≥ 5 AND
//!    bits_per_pixel × n_planes ≥ 8 AND the byte 769 from EOF is
//!    `0x0C` ("palette signature"); the next 768 bytes are RGB×256.]
//! ```
//!
//! Image width / height in pixels are derived from the window:
//! `width = x_max - x_min + 1`, `height = y_max - y_min + 1`.

/// Header size in bytes (always 128).
pub const PCX_HEADER_SIZE: usize = 128;

/// Manufacturer byte at offset 0 — identifies a ZSoft .pcx file.
pub const PCX_MANUFACTURER: u8 = 0x0A;

/// `encoding` byte (offset 2): always 1 (RLE) per spec §1.
pub const PCX_ENCODING_RLE: u8 = 1;

/// VGA palette marker byte that prefixes the 768-byte tail palette
/// (PCX 3.0+).
pub const PCX_VGA_PALETTE_MARKER: u8 = 0x0C;

/// Length in bytes of the optional VGA palette (256 RGB triplets).
pub const PCX_VGA_PALETTE_BYTES: usize = 768;

/// Total length in bytes of the VGA palette block, including its
/// 1-byte `0x0C` marker prefix.
pub const PCX_VGA_PALETTE_BLOCK_BYTES: usize = 1 + PCX_VGA_PALETTE_BYTES;

/// Parsed PCX header fields. Sizes match the on-disk u8/u16 widths;
/// `width` and `height` are computed from the window for caller
/// convenience.
#[derive(Debug, Clone, Copy)]
pub struct PcxHeader {
    pub manufacturer: u8,
    pub version: u8,
    pub encoding: u8,
    pub bits_per_pixel: u8,
    pub x_min: u16,
    pub y_min: u16,
    pub x_max: u16,
    pub y_max: u16,
    pub h_dpi: u16,
    pub v_dpi: u16,
    /// 16 × 3 bytes — the EGA-era hardware palette. Used only when
    /// `bits_per_pixel × n_planes ∈ {1, 2, 4}`. PCX 3.0+ files (with
    /// `version ≥ 3`) traditionally zero this field even when the data
    /// is EGA, in which case the decoder may need to fall back to a
    /// default palette; we honour whatever is actually present.
    pub ega_palette: [u8; 48],
    pub reserved: u8,
    pub n_planes: u8,
    pub bytes_per_line: u16,
    pub palette_info: u16,
    pub h_screen_size: u16,
    pub v_screen_size: u16,
    /// 54 bytes of padding — surfaced for completeness but ignored by
    /// the decoder.
    pub filler: [u8; 54],
}

impl PcxHeader {
    /// Picture width in pixels, computed from `x_max - x_min + 1`.
    ///
    /// Saturating: a malformed header with `x_max < x_min` yields `0`
    /// rather than underflowing. This accessor is public and may be
    /// called on an unvalidated header straight out of [`parse_header`],
    /// so it must never panic; `parse_pcx` separately rejects the
    /// `x_max < x_min` and zero-dimension cases with a typed error.
    pub fn width(&self) -> u32 {
        (self.x_max as u32 + 1).saturating_sub(self.x_min as u32)
    }
    /// Picture height in pixels, computed from `y_max - y_min + 1`.
    ///
    /// Saturating for the same reason as [`PcxHeader::width`]: an
    /// unvalidated `y_max < y_min` header yields `0` instead of an
    /// integer-underflow panic.
    pub fn height(&self) -> u32 {
        (self.y_max as u32 + 1).saturating_sub(self.y_min as u32)
    }
    /// On-disk total bytes for one full scanline:
    /// `n_planes × bytes_per_line`. Independent of pixel-width
    /// (the field is rounded up to the device word size at write time).
    pub fn scanline_bytes(&self) -> usize {
        self.n_planes as usize * self.bytes_per_line as usize
    }
}

/// Parse the 128-byte PCX header. Returns `None` if the input is
/// shorter than 128 bytes — the caller turns that into a useful error
/// message.
pub fn parse_header(input: &[u8]) -> Option<PcxHeader> {
    if input.len() < PCX_HEADER_SIZE {
        return None;
    }
    let mut ega_palette = [0u8; 48];
    ega_palette.copy_from_slice(&input[16..64]);
    let mut filler = [0u8; 54];
    filler.copy_from_slice(&input[74..128]);
    Some(PcxHeader {
        manufacturer: input[0],
        version: input[1],
        encoding: input[2],
        bits_per_pixel: input[3],
        x_min: read_u16_le(input, 4),
        y_min: read_u16_le(input, 6),
        x_max: read_u16_le(input, 8),
        y_max: read_u16_le(input, 10),
        h_dpi: read_u16_le(input, 12),
        v_dpi: read_u16_le(input, 14),
        ega_palette,
        reserved: input[64],
        n_planes: input[65],
        bytes_per_line: read_u16_le(input, 66),
        palette_info: read_u16_le(input, 68),
        h_screen_size: read_u16_le(input, 70),
        v_screen_size: read_u16_le(input, 72),
        filler,
    })
}

#[doc(hidden)]
#[inline]
pub fn read_u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

/// Locate the 768-byte VGA palette appended to a PCX 3.0+ file.
/// The palette is present iff the byte 769 from the end of file is
/// `0x0C`. Returns `Some(palette_bytes)` when found.
pub fn find_vga_palette(input: &[u8]) -> Option<&[u8]> {
    if input.len() < PCX_HEADER_SIZE + PCX_VGA_PALETTE_BLOCK_BYTES {
        return None;
    }
    let off = input.len() - PCX_VGA_PALETTE_BLOCK_BYTES;
    if input[off] != PCX_VGA_PALETTE_MARKER {
        return None;
    }
    Some(&input[off + 1..off + 1 + PCX_VGA_PALETTE_BYTES])
}
