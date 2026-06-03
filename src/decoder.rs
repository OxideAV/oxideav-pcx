//! PCX decode. Always normalises to packed `Rgba` (top-left origin).
//!
//! Supports the (depth, planes) combinations called out by spec §4.1:
//!
//! * 1 bpp × 1 plane — monochrome (each bit = one pixel).
//! * 1 bpp × 3 planes — 8-colour EGA RGB. One plane per primary; plane
//!   order is R, G, B per spec §4 (the bit-plane example at lines
//!   46-58 of the rev-5 technical reference). Each plane bit toggles
//!   its channel between 0x00 and 0xFF, giving the eight on/off
//!   primary combinations.
//! * 1 bpp × 4 planes — 16-colour EGA. Each plane carries the matching
//!   bit-position of an EGA colour index; planes are read in BGR-IRGB
//!   order per the spec table.
//! * 2 bpp × 1 plane — 4-colour CGA, packed (4 pixels/byte). Palette is
//!   the legacy CGA palette selected from `ega_palette[16]` (palette
//!   number bit) + `ega_palette[19]` (foreground intensity / palette
//!   selector).
//! * 4 bpp × 1 plane — 16-colour packed-bits (2 pixels/byte). Palette
//!   is the in-header `ega_palette` (or default EGA fallback).
//! * 8 bpp × 1 plane — 256-colour palette. Palette is the 768-byte
//!   block at end-of-file when the byte 769 from EOF is `0x0C`; if
//!   absent, the decoder produces a grayscale ramp as a fallback.
//! * 8 bpp × 3 planes — 24-bit truecolour. Plane order is R, G, B.
//!
//! With the default `registry` feature on, the gated `PcxDecoder`
//! trait impl wraps [`parse_pcx`] for the `oxideav_core::Decoder`
//! surface.

use crate::error::{PcxError as Error, Result};
use crate::image::{PcxImage, PcxPixelFormat};
use crate::rle;
use crate::types::*;

#[cfg(feature = "registry")]
use oxideav_core::Decoder;
#[cfg(feature = "registry")]
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, VideoFrame, VideoPlane};

/// Factory registered with the codec registry. Consumes one packet
/// per whole PCX file and produces one frame.
#[cfg(feature = "registry")]
pub fn make_decoder(_params: &CodecParameters) -> oxideav_core::Result<Box<dyn Decoder>> {
    Ok(Box::new(PcxDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
    }))
}

#[cfg(feature = "registry")]
struct PcxDecoder {
    codec_id: CodecId,
    pending: Option<VideoFrame>,
    eof: bool,
}

#[cfg(feature = "registry")]
impl Decoder for PcxDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> oxideav_core::Result<()> {
        let image = parse_pcx(&packet.data)?;
        self.pending = Some(image_to_video_frame(image));
        Ok(())
    }
    fn receive_frame(&mut self) -> oxideav_core::Result<Frame> {
        match self.pending.take() {
            Some(f) => Ok(Frame::Video(f)),
            None => {
                if self.eof {
                    Err(oxideav_core::Error::Eof)
                } else {
                    Err(oxideav_core::Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> oxideav_core::Result<()> {
        self.eof = true;
        Ok(())
    }
}

#[cfg(feature = "registry")]
fn image_to_video_frame(image: PcxImage) -> VideoFrame {
    let stride = image.stride();
    VideoFrame {
        pts: image.pts,
        planes: vec![VideoPlane {
            stride,
            data: image.data,
        }],
    }
}

// ---------------------------------------------------------------------------
// Public standalone API
// ---------------------------------------------------------------------------

/// Decode a complete PCX file into a [`PcxImage`].
///
/// The returned image is always packed [`PcxPixelFormat::Rgba`]
/// (palette lookup, planar→packed merging, and 1-bit expansion all
/// happen at decode time so consumers don't have to know the on-disk
/// quirks). Top-left origin.
pub fn parse_pcx(input: &[u8]) -> Result<PcxImage> {
    let header = parse_header(input).ok_or_else(|| Error::invalid("PCX: header truncated"))?;
    if header.manufacturer != PCX_MANUFACTURER {
        return Err(Error::invalid(format!(
            "PCX: bad manufacturer byte 0x{:02X} (expected 0x0A)",
            header.manufacturer
        )));
    }
    if !matches!(header.version, 0 | 2 | 3 | 4 | 5) {
        return Err(Error::invalid(format!(
            "PCX: unknown version byte {} (expected 0/2/3/4/5)",
            header.version
        )));
    }
    if header.encoding != PCX_ENCODING_RLE {
        return Err(Error::unsupported(format!(
            "PCX: encoding byte {} not supported (only 1 = RLE is defined)",
            header.encoding
        )));
    }
    let width = header.width();
    let height = header.height();
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX: zero dimension"));
    }
    if header.x_max < header.x_min || header.y_max < header.y_min {
        return Err(Error::invalid("PCX: x_max < x_min or y_max < y_min"));
    }
    if header.bytes_per_line == 0 {
        return Err(Error::invalid("PCX: bytes_per_line == 0"));
    }
    if header.n_planes == 0 {
        return Err(Error::invalid("PCX: n_planes == 0"));
    }
    // `bytes_per_line` is the per-plane on-disk row width. Per spec §1
    // it MUST be wide enough to carry every pixel of the visible row;
    // some malformed writers under-set this field and the result would
    // silently mis-frame planar→packed reconstruction. Reject up front.
    let min_bpl: u32 = match header.bits_per_pixel {
        1 => width.div_ceil(8),
        2 => width.div_ceil(4),
        4 => width.div_ceil(2),
        8 => width,
        bpp => {
            return Err(Error::unsupported(format!(
                "PCX: bits_per_pixel={bpp} not in the {{1,2,4,8}} set the spec defines"
            )))
        }
    };
    if (header.bytes_per_line as u32) < min_bpl {
        return Err(Error::invalid(format!(
            "PCX: bytes_per_line={} too small for width={} at {} bpp (need ≥ {})",
            header.bytes_per_line, width, header.bits_per_pixel, min_bpl
        )));
    }

    // Decode RLE: scanline-by-scanline so we can spot truncation.
    let scanline = header.scanline_bytes();
    // Total decoded planar size = scanline × height. Both factors are
    // attacker-controlled (up to 255 × 65535 per scanline, 65536 rows),
    // so compute the product with `checked_mul` to avoid a debug-build
    // multiply overflow before it is used as an allocation hint / bound.
    let total_planar = scanline
        .checked_mul(height as usize)
        .ok_or_else(|| Error::invalid("PCX: scanline × height overflows usize"))?;
    let mut cursor = PCX_HEADER_SIZE;
    // The RLE pixel data ends either at end-of-file (no VGA palette)
    // or 769 bytes before EOF (VGA palette block present). Limit
    // `rle_input` accordingly so a stray `0x0C` inside the palette
    // can't be misinterpreted as a literal pixel byte.
    let vga_palette = find_vga_palette(input);
    let rle_end = if vga_palette.is_some() {
        input.len() - PCX_VGA_PALETTE_BLOCK_BYTES
    } else {
        input.len()
    };
    if rle_end < cursor {
        return Err(Error::invalid("PCX: pixel data section is empty"));
    }
    // Decompression-bomb guard. A header can claim arbitrarily large
    // dimensions while the file carries only a few RLE bytes; eagerly
    // reserving `scanline × height` would then try to allocate hundreds
    // of gigabytes for a tiny input. PCX RLE expands at most ~31.5:1
    // (a 2-byte packet yields up to 63 output bytes), so the largest
    // output the available pixel bytes could legitimately produce is
    // bounded by `available × 63`. Reject any claim that exceeds that
    // bound up front, and cap the initial reservation so a borderline
    // (but in-range) claim still grows the buffer lazily as the RLE
    // decoder actually produces bytes.
    let available = rle_end - cursor;
    let max_plausible_output = available.saturating_mul(63);
    if total_planar > max_plausible_output {
        return Err(Error::invalid(format!(
            "PCX: claimed pixel data ({total_planar} bytes) exceeds what {available} RLE bytes can decode"
        )));
    }
    // `total_planar ≤ max_plausible_output ≤ available × 63`, so this
    // reservation is now bounded by the actual input size.
    let mut pixels_planar = Vec::with_capacity(total_planar);
    for _ in 0..height as usize {
        let consumed = rle::decode(&input[cursor..rle_end], &mut pixels_planar, scanline)?;
        cursor += consumed;
    }

    // Re-pack planar scanlines into packed RGBA pixels per
    // (depth, n_planes) combination.
    let data = match (header.bits_per_pixel, header.n_planes) {
        (1, 1) => unpack_1bpp_1plane(&header, &pixels_planar),
        (1, 3) => unpack_1bpp_3planes(&header, &pixels_planar),
        (1, 4) => unpack_1bpp_4planes(&header, &pixels_planar),
        (2, 1) => unpack_2bpp_1plane_cga(&header, &pixels_planar),
        (4, 1) => unpack_4bpp_1plane(&header, &pixels_planar),
        (8, 1) => {
            // `palette_info == 2` (spec §3) forces the grayscale
            // interpretation regardless of whether a tail palette is
            // present. Some scanner / FAX-era tools emit a grayscale
            // PCX with `palette_info=2` and no VGA tail; some emit
            // the flag AND a redundant tail palette. We honour the
            // flag in both cases.
            let palette = if header.palette_info == 2 {
                None
            } else {
                vga_palette
            };
            unpack_8bpp_1plane(&header, &pixels_planar, palette)?
        }
        (8, 3) => unpack_8bpp_3planes(&header, &pixels_planar),
        (bpp, n) => {
            return Err(Error::unsupported(format!(
                "PCX: (bits_per_pixel={bpp}, n_planes={n}) combination not supported"
            )))
        }
    };

    // Surface the authoring DPI only when BOTH fields carry a non-zero
    // value: per spec §3 the 0 sentinel means "unset" (many drawing
    // programs leave the field at zero rather than 72×72), so an
    // asymmetric (0, 300) header would not be a sensible printer/scanner
    // reading.
    let dpi = if header.h_dpi != 0 && header.v_dpi != 0 {
        Some((header.h_dpi, header.v_dpi))
    } else {
        None
    };

    Ok(PcxImage {
        width,
        height,
        pixel_format: PcxPixelFormat::Rgba,
        data,
        pts: None,
        dpi,
    })
}

// ---------------------------------------------------------------------------
// Plane-unpack paths
// ---------------------------------------------------------------------------

// Plane-unpack hot paths share a row-walking idiom: split the
// destination buffer into `w*4`-byte row slices via
// `chunks_exact_mut`, then walk each row's pixels as 4-byte
// destination chunks. Splitting the destination this way gives the
// optimiser enough provenance information to drop the per-pixel
// bounds checks against `out` and to lay the four-byte RGBA stores
// out as a single aligned 32-bit move — both visible in the r209
// bench numbers (24-bit 1920×1080 1.50 → 6.55 GiB/s, 8-bit grayscale
// 512×512 1.82 → 7.18 GiB/s). Output bytes are bit-identical to the
// pre-r209 per-index implementation.

fn unpack_1bpp_1plane(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let mut out = vec![0u8; w * h * 4];
    let src_rows = planar.chunks_exact(bpl);
    let dst_rows = out.chunks_exact_mut(w * 4);
    for (row, dst_row) in src_rows.zip(dst_rows) {
        for (x, dst) in dst_row.chunks_exact_mut(4).enumerate() {
            // 1 = white, 0 = black. (PCX 5.0 spec §4.1.)
            let bit = (row[x >> 3] >> (7 - (x & 7))) & 1;
            let v = if bit != 0 { 0xFF } else { 0x00 };
            dst[0] = v;
            dst[1] = v;
            dst[2] = v;
            dst[3] = 0xFF;
        }
    }
    out
}

fn unpack_1bpp_3planes(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    // 8-colour EGA RGB: one bit-plane per primary, plane order R, G, B
    // (spec §4 bit-plane example, lines 46-58 of the rev-5 technical
    // reference). Each plane bit toggles the channel between 0x00 and
    // 0xFF. No external palette is consulted; the eight colours are
    // the on/off primaries enumerated by the plane bits themselves.
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let mut out = vec![0u8; w * h * 4];
    let src_rows = planar.chunks_exact(bpl * 3);
    let dst_rows = out.chunks_exact_mut(w * 4);
    for (row, dst_row) in src_rows.zip(dst_rows) {
        let (rp, rest) = row.split_at(bpl);
        let (gp, bp) = rest.split_at(bpl);
        for (x, dst) in dst_row.chunks_exact_mut(4).enumerate() {
            let byte = x >> 3;
            let shift = 7 - (x & 7);
            let r_bit = (rp[byte] >> shift) & 1;
            let g_bit = (gp[byte] >> shift) & 1;
            let b_bit = (bp[byte] >> shift) & 1;
            dst[0] = if r_bit != 0 { 0xFF } else { 0x00 };
            dst[1] = if g_bit != 0 { 0xFF } else { 0x00 };
            dst[2] = if b_bit != 0 { 0xFF } else { 0x00 };
            dst[3] = 0xFF;
        }
    }
    out
}

fn unpack_1bpp_4planes(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let palette = ega_palette_or_default(&header.ega_palette);
    let mut out = vec![0u8; w * h * 4];
    let src_rows = planar.chunks_exact(bpl * 4);
    let dst_rows = out.chunks_exact_mut(w * 4);
    for (row, dst_row) in src_rows.zip(dst_rows) {
        // Split each row into its four bit-plane sub-slices once per
        // row so the per-pixel loop's bit extraction works against
        // local slice references rather than recomputing
        // `plane * bpl` for every pixel.
        let (p0, rest) = row.split_at(bpl);
        let (p1, rest) = rest.split_at(bpl);
        let (p2, p3) = rest.split_at(bpl);
        for (x, dst) in dst_row.chunks_exact_mut(4).enumerate() {
            let byte = x >> 3;
            let shift = 7 - (x & 7);
            // Plane order is bit 0 → bit 3 (B, G, R, I in classical
            // EGA hardware terms). Each plane contributes one bit of
            // the 4-bit palette index.
            let idx = (((p0[byte] >> shift) & 1)
                | (((p1[byte] >> shift) & 1) << 1)
                | (((p2[byte] >> shift) & 1) << 2)
                | (((p3[byte] >> shift) & 1) << 3)) as usize;
            let p = palette[idx];
            dst[0] = p[0];
            dst[1] = p[1];
            dst[2] = p[2];
            dst[3] = 0xFF;
        }
    }
    out
}

fn unpack_8bpp_1plane(
    header: &PcxHeader,
    planar: &[u8],
    vga_palette: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    // Build a 256-entry RGBA palette: VGA tail block if present,
    // grayscale ramp otherwise. Storing it as `[u8; 4]` with a
    // baked-in `0xFF` alpha lets the per-pixel loop emit one 4-byte
    // store via `copy_from_slice` instead of three scalar bytes plus
    // a separate alpha byte.
    let palette: [[u8; 4]; 256] = if let Some(p) = vga_palette {
        let mut out = [[0u8; 4]; 256];
        for (i, e) in out.iter_mut().enumerate() {
            *e = [p[i * 3], p[i * 3 + 1], p[i * 3 + 2], 0xFF];
        }
        out
    } else {
        let mut out = [[0u8; 4]; 256];
        for (i, e) in out.iter_mut().enumerate() {
            let v = i as u8;
            *e = [v, v, v, 0xFF];
        }
        out
    };
    let mut out = vec![0u8; w * h * 4];
    let src_rows = planar.chunks_exact(bpl);
    let dst_rows = out.chunks_exact_mut(w * 4);
    for (row, dst_row) in src_rows.zip(dst_rows) {
        for (dst, &b) in dst_row.chunks_exact_mut(4).zip(row.iter().take(w)) {
            dst.copy_from_slice(&palette[b as usize]);
        }
    }
    Ok(out)
}

fn unpack_8bpp_3planes(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let mut out = vec![0u8; w * h * 4];
    let src_rows = planar.chunks_exact(bpl * 3);
    let dst_rows = out.chunks_exact_mut(w * 4);
    for (row, dst_row) in src_rows.zip(dst_rows) {
        // Pre-slice the R/G/B plane sub-rows once and bound each
        // plane slice to exactly `w` bytes so the zip-of-three
        // iterators below can advance with no bounds checks against
        // anything but the destination chunks. Triple-zip keeps the
        // four output stores adjacent in the generated assembly and
        // avoids the per-pixel `[x]` index that the prior pattern
        // forced.
        let (rp, rest) = row.split_at(bpl);
        let (gp, bp) = rest.split_at(bpl);
        let r_iter = rp[..w].iter();
        let g_iter = gp[..w].iter();
        let b_iter = bp[..w].iter();
        let dst_iter = dst_row.chunks_exact_mut(4);
        for (((&r, &g), &b), dst) in r_iter.zip(g_iter).zip(b_iter).zip(dst_iter) {
            dst[0] = r;
            dst[1] = g;
            dst[2] = b;
            dst[3] = 0xFF;
        }
    }
    out
}

fn unpack_2bpp_1plane_cga(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    // 2 bpp packed: 4 pixels per byte, MSB first. CGA 4-colour palette
    // is selected from `ega_palette[16]` and `ega_palette[19]` per
    // CGA hardware: bit 5 of byte 16 is "palette" (0/1, magenta/cyan
    // family), and the high nibble of byte 16 is the background colour
    // (used as palette index 0). Background defaults to black (0).
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    // Pre-bake alpha into a 4-entry RGBA palette for one-store
    // per-pixel writes, same pattern as the 8 bpp paths.
    let cga = cga_palette_from_header(&header.ega_palette);
    let palette: [[u8; 4]; 4] = [
        [cga[0][0], cga[0][1], cga[0][2], 0xFF],
        [cga[1][0], cga[1][1], cga[1][2], 0xFF],
        [cga[2][0], cga[2][1], cga[2][2], 0xFF],
        [cga[3][0], cga[3][1], cga[3][2], 0xFF],
    ];
    let mut out = vec![0u8; w * h * 4];
    let src_rows = planar.chunks_exact(bpl);
    let dst_rows = out.chunks_exact_mut(w * 4);
    for (row, dst_row) in src_rows.zip(dst_rows) {
        for (x, dst) in dst_row.chunks_exact_mut(4).enumerate() {
            // Top two bits = pixel 0, then 2/3, etc.
            let shift = 6 - 2 * (x & 3);
            let idx = ((row[x >> 2] >> shift) & 0b11) as usize;
            dst.copy_from_slice(&palette[idx]);
        }
    }
    out
}

fn unpack_4bpp_1plane(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    // 4 bpp packed: 2 pixels per byte, high nibble first.
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    // Pre-bake alpha into a 16-entry RGBA palette.
    let ega = ega_palette_or_default(&header.ega_palette);
    let mut palette = [[0u8; 4]; 16];
    for (i, e) in palette.iter_mut().enumerate() {
        *e = [ega[i][0], ega[i][1], ega[i][2], 0xFF];
    }
    let mut out = vec![0u8; w * h * 4];
    let src_rows = planar.chunks_exact(bpl);
    let dst_rows = out.chunks_exact_mut(w * 4);
    for (row, dst_row) in src_rows.zip(dst_rows) {
        for (x, dst) in dst_row.chunks_exact_mut(4).enumerate() {
            let byte = row[x >> 1];
            let nib = if x & 1 == 0 {
                (byte >> 4) & 0x0F
            } else {
                byte & 0x0F
            };
            dst.copy_from_slice(&palette[nib as usize]);
        }
    }
    out
}

/// Standard CGA 4-colour palettes per the IBM CGA hardware reference.
/// Each is `[background, c1, c2, c3]`. Background is overridden by the
/// header byte 16 high nibble (the "border/background" register).
///
/// Palette 0 = green / red / brown.
/// Palette 1 = cyan / magenta / white.
/// Both come in low- and high-intensity flavours.
const CGA_PALETTE_0_LOW: [[u8; 3]; 4] = [
    [0x00, 0x00, 0x00], // background (overridden)
    [0x00, 0xAA, 0x00], // green
    [0xAA, 0x00, 0x00], // red
    [0xAA, 0x55, 0x00], // brown
];
const CGA_PALETTE_0_HIGH: [[u8; 3]; 4] = [
    [0x00, 0x00, 0x00],
    [0x55, 0xFF, 0x55], // light green
    [0xFF, 0x55, 0x55], // light red
    [0xFF, 0xFF, 0x55], // yellow
];
const CGA_PALETTE_1_LOW: [[u8; 3]; 4] = [
    [0x00, 0x00, 0x00],
    [0x00, 0xAA, 0xAA], // cyan
    [0xAA, 0x00, 0xAA], // magenta
    [0xAA, 0xAA, 0xAA], // light gray
];
const CGA_PALETTE_1_HIGH: [[u8; 3]; 4] = [
    [0x00, 0x00, 0x00],
    [0x55, 0xFF, 0xFF], // light cyan
    [0xFF, 0x55, 0xFF], // light magenta
    [0xFF, 0xFF, 0xFF], // white
];

/// Standard 16-entry EGA hardware palette (the one returned by
/// `ega_palette_or_default` when the header field is all zeros).
const EGA_DEFAULT_PALETTE: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00],
    [0x00, 0x00, 0xAA],
    [0x00, 0xAA, 0x00],
    [0x00, 0xAA, 0xAA],
    [0xAA, 0x00, 0x00],
    [0xAA, 0x00, 0xAA],
    [0xAA, 0x55, 0x00],
    [0xAA, 0xAA, 0xAA],
    [0x55, 0x55, 0x55],
    [0x55, 0x55, 0xFF],
    [0x55, 0xFF, 0x55],
    [0x55, 0xFF, 0xFF],
    [0xFF, 0x55, 0x55],
    [0xFF, 0x55, 0xFF],
    [0xFF, 0xFF, 0x55],
    [0xFF, 0xFF, 0xFF],
];

/// Resolve a CGA 4-colour palette from the in-header bytes.
///
/// PCX repurposes the EGA palette region for CGA mode (see
/// [`crate::encode_pcx_2bpp_cga`] for the matching writer):
/// * `ega_palette[16]` — high nibble = background colour (EGA index
///   0..15 used as palette entry 0).
/// * `ega_palette[19]` — bit 7 = palette select (0 = palette 1
///   cyan/magenta/white, 1 = palette 0 green/red/brown); bit 6 =
///   intensity (0 = high / bright, 1 = low / dim).
///
/// When `ega_palette[19]` is zero (PCX 3.0+ files commonly leave it
/// blank), the decoder lands on palette 1 high-intensity
/// (cyan/magenta/white) — the most common CGA palette for game
/// screenshots of the era — because both bits being clear maps to
/// `palette_select = 0` (palette 1) and `intensity = high` per the
/// encoding above.
pub(crate) fn cga_palette_from_header(raw: &[u8; 48]) -> [[u8; 3]; 4] {
    let bg_idx = (raw[16] >> 4) as usize;
    let selector = raw[19];
    let palette_zero = selector & 0x80 != 0;
    let low_intensity = selector & 0x40 != 0;
    let mut p = match (palette_zero, low_intensity) {
        (false, false) => CGA_PALETTE_1_HIGH,
        (false, true) => CGA_PALETTE_1_LOW,
        (true, false) => CGA_PALETTE_0_HIGH,
        (true, true) => CGA_PALETTE_0_LOW,
    };
    p[0] = EGA_DEFAULT_PALETTE[bg_idx];
    p
}

/// Extract a 16-entry RGB palette from the 48-byte `ega_palette`
/// header field. If the field is all zeros (which PCX 3.0+ files may
/// emit even for EGA data), fall back to the standard EGA hardware
/// palette listed in spec table §3.1.
fn ega_palette_or_default(raw: &[u8; 48]) -> [[u8; 3]; 16] {
    if raw.iter().all(|&b| b == 0) {
        // Standard EGA 16-colour palette per spec table §3.1
        // (in the same BGR-IRGB index order used above for plane bits).
        // Black, blue, green, cyan, red, magenta, brown, light gray,
        // dark gray, light blue, light green, light cyan, light red,
        // light magenta, yellow, white.
        return EGA_DEFAULT_PALETTE;
    }
    let mut out = [[0u8; 3]; 16];
    for (i, e) in out.iter_mut().enumerate() {
        *e = [raw[i * 3], raw[i * 3 + 1], raw[i * 3 + 2]];
    }
    out
}
