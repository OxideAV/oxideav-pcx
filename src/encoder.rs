//! PCX encode. Every file is RLE-compressed (the only encoding byte the
//! spec defines); the header version defaults to 5.
//!
//! [`encode_image`] (the contract's `encode`) writes a [`PcxImage`] in
//! one on-disk geometry ([`PcxLayout`]): the geometry the options
//! force, else the one the image was decoded from, else the natural
//! geometry for its layout and palette ([`PcxLayout::natural_for`]).
//! Compact mode encodes every geometry whose losslessness precondition
//! holds and keeps the fewest bytes. The per-geometry writers below
//! (`write_*`) take the payload in the geometry's own terms (bits,
//! 2-/4-/8-bit indices, grey bytes, packed RGB) plus the header
//! annotations ([`HeaderMeta`]); the pre-contract `encode_pcx_*`
//! writers are thin wrappers over them with the historical 72 × 72 DPI
//! default, so their output is byte-identical to earlier releases.
//!
//! The RLE encoder coalesces runs of identical bytes (≤ 63 each) and
//! escapes any singleton byte ≥ `0xC0` into a length-1 packet so the
//! decoder won't mistake it for a run header.

use std::borrow::Cow;

use crate::error::{PcxError as Error, Result};
use crate::image::{Pcx2bppCgaCpi, PcxImage, PcxLayout, PcxPixelFormat};
use crate::options::EncodeOptions;
use crate::rle;
use crate::types::*;

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

/// Two-entry colormap written by the monochrome writers: entry 0 = pure
/// black, entry 1 = pure white, remaining 14 triples zero. The EGFF
/// cross-reference's canonical mode matrix treats `1 bpp × 1 plane` as
/// the 2-colour paletted case of the header colormap, so a
/// colormap-driven reader resolves bit 0 / bit 1 through entries 0 / 1;
/// writing the palette explicitly makes our mono files self-describing
/// for such readers while the spec §4.1 bit convention (1 = white)
/// stays byte-identical for bit-driven ones.
pub(crate) const MONO_COLORMAP: [u8; 48] = {
    let mut p = [0u8; 48];
    p[3] = 0xFF;
    p[4] = 0xFF;
    p[5] = 0xFF;
    p
};

/// Authoring DPI the pre-contract writers stamp when the caller supplies
/// none. 72×72 matches the "screen DPI" convention PC Paintbrush and the
/// rev-5 manual's example header carry; scanner software that emits PCX
/// typically overrides this to 300/300. The contract `encode` writes
/// `0 / 0` ("unset") instead when the image carries no DPI.
pub(crate) const DEFAULT_DPI: (u16, u16) = (72, 72);

/// The header words that are annotations rather than geometry: window
/// origin, authoring DPI, PB IV screen size and the version byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeaderMeta {
    pub x_min: u16,
    pub y_min: u16,
    pub dpi: (u16, u16),
    pub screen_size: (u16, u16),
    pub version: u8,
}

impl HeaderMeta {
    /// The pre-contract writers' header: zero origin, 72 × 72 DPI, zero
    /// screen size, version 5.
    pub(crate) const LEGACY: Self = Self {
        x_min: 0,
        y_min: 0,
        dpi: DEFAULT_DPI,
        screen_size: (0, 0),
        version: 5,
    };

    fn with_dpi(mut self, dpi: (u16, u16)) -> Self {
        self.dpi = dpi;
        self
    }

    fn with_origin(mut self, x_min: u16, y_min: u16) -> Self {
        self.x_min = x_min;
        self.y_min = y_min;
        self
    }

    fn with_screen(mut self, screen_size: (u16, u16)) -> Self {
        self.screen_size = screen_size;
        self
    }
}

#[allow(clippy::too_many_arguments)]
fn write_header(
    out: &mut Vec<u8>,
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    bits_per_pixel: u8,
    n_planes: u8,
    bytes_per_line: u16,
    ega_palette: &[u8; 48],
    palette_info: u16,
) {
    let start = out.len();
    // `check_origin` admits `x_min + width == 65536` (x_max = 65535), so
    // the sum must be formed in u32 before the `- 1`.
    let x_max = (u32::from(meta.x_min) + u32::from(width) - 1) as u16;
    let y_max = (u32::from(meta.y_min) + u32::from(height) - 1) as u16;
    out.push(PCX_MANUFACTURER); // 0
    out.push(meta.version); // 1
    out.push(PCX_ENCODING_RLE); // 2
    out.push(bits_per_pixel); // 3
    out.extend_from_slice(&meta.x_min.to_le_bytes()); // x_min  4
    out.extend_from_slice(&meta.y_min.to_le_bytes()); // y_min  6
    out.extend_from_slice(&x_max.to_le_bytes()); // x_max  8
    out.extend_from_slice(&y_max.to_le_bytes()); // y_max 10
    out.extend_from_slice(&meta.dpi.0.to_le_bytes()); // h_dpi  12
    out.extend_from_slice(&meta.dpi.1.to_le_bytes()); // v_dpi  14
    out.extend_from_slice(ega_palette); // ega_palette  16..64
    out.push(0); // reserved 64
    out.push(n_planes); // n_planes 65
    out.extend_from_slice(&bytes_per_line.to_le_bytes()); // bytes_per_line 66
    out.extend_from_slice(&palette_info.to_le_bytes()); // palette_info 68
    out.extend_from_slice(&meta.screen_size.0.to_le_bytes()); // h_screen_size 70
    out.extend_from_slice(&meta.screen_size.1.to_le_bytes()); // v_screen_size 72
    out.extend_from_slice(&[0u8; 54]); // filler 74..128
    debug_assert_eq!(out.len() - start, PCX_HEADER_SIZE);
}

#[inline]
fn round_up_to_even(v: u16) -> u16 {
    if v % 2 == 0 {
        v
    } else {
        v + 1
    }
}

fn check_dims(width: u16, height: u16) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    Ok(())
}

fn check_len(len: usize, width: u16, height: u16, per_pixel: usize, what: &str) -> Result<()> {
    if len < width as usize * height as usize * per_pixel {
        return Err(Error::invalid(format!(
            "PCX encoder: {what} input shorter than width × height{}",
            if per_pixel == 1 {
                String::new()
            } else {
                format!(" × {per_pixel}")
            }
        )));
    }
    Ok(())
}

fn check_origin(meta: &HeaderMeta, width: u16, height: u16) -> Result<()> {
    if (meta.x_min as u32 + width as u32) > u16::MAX as u32 + 1 {
        return Err(Error::invalid(
            "PCX encoder: x_min + width exceeds u16::MAX + 1",
        ));
    }
    if (meta.y_min as u32 + height as u32) > u16::MAX as u32 + 1 {
        return Err(Error::invalid(
            "PCX encoder: y_min + height exceeds u16::MAX + 1",
        ));
    }
    Ok(())
}

#[inline]
fn check_screen_size(screen_size: (u16, u16)) -> Result<()> {
    if screen_size.0 == 0 || screen_size.1 == 0 {
        return Err(Error::invalid(format!(
            "PCX encoder: screen_size components must both be non-zero (got {:?}); spec §3 treats 0 as 'unset'",
            screen_size
        )));
    }
    Ok(())
}

#[inline]
fn check_dpi(dpi: (u16, u16)) -> Result<()> {
    if dpi.0 == 0 || dpi.1 == 0 {
        return Err(Error::invalid(format!(
            "PCX encoder: dpi components must both be non-zero (got {:?}); spec §3 treats 0 as 'unset'",
            dpi
        )));
    }
    Ok(())
}

/// Pack `width` 1-bit samples MSB-first into one plane's scanline slice
/// `dst`, eight pixels per output byte.
///
/// The PCX 1-bit-per-plane on-disk layout (spec §"Image File (.PCX)
/// Format") places pixel `x` at bit `7 - (x % 8)` of byte `x / 8`
/// within the plane. Each group of up to eight pixels is folded into one
/// accumulator with a shift-OR and written once, so the inner loop has
/// no per-pixel array index, no per-pixel branch into the destination,
/// and no read-modify-write on `dst`. Absent tail pixels (when `width`
/// is not a multiple of 8) contribute a 0 bit, and `dst` bytes beyond
/// `width.div_ceil(8)` (the even-stride padding) are left untouched at
/// their caller-zeroed value.
#[inline]
fn pack_1bpp_plane_row(dst: &mut [u8], width: usize, get_bit: impl Fn(usize) -> bool) {
    let full = width / 8;
    for (b, cell) in dst.iter_mut().take(full).enumerate() {
        let base = b * 8;
        let mut acc = 0u8;
        for k in 0..8 {
            acc |= (get_bit(base + k) as u8) << (7 - k);
        }
        *cell = acc;
    }
    let rem = width % 8;
    if rem != 0 {
        let base = full * 8;
        let mut acc = 0u8;
        for k in 0..rem {
            acc |= (get_bit(base + k) as u8) << (7 - k);
        }
        dst[full] = acc;
    }
}

// ---------------------------------------------------------------------------
// Per-geometry writers (payload in the geometry's own terms)
// ---------------------------------------------------------------------------

/// 8 bpp × 1 plane indices + the 768-byte VGA tail palette.
pub(crate) fn write_indexed8(
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(indices.len(), width, height, 1, "indexed")?;
    if palette.len() != PCX_VGA_PALETTE_BYTES {
        return Err(Error::invalid(format!(
            "PCX encoder: 256-colour palette must be exactly {PCX_VGA_PALETTE_BYTES} bytes (got {})",
            palette.len()
        )));
    }
    // Bytes-per-line is the on-disk per-plane row width, rounded up to
    // an even number per spec §1 ("the value must be even").
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + indices.len() / 2 + PCX_VGA_PALETTE_BYTES);
    write_header(
        &mut out,
        meta,
        width,
        height,
        8,
        1,
        bytes_per_line,
        &[0u8; 48],
        1,
    );
    let mut row = Vec::with_capacity(bytes_per_line as usize);
    for y in 0..height as usize {
        row.clear();
        row.extend_from_slice(&indices[y * width as usize..y * width as usize + width as usize]);
        row.resize(bytes_per_line as usize, 0);
        rle::encode(&row, &mut out);
    }
    out.push(PCX_VGA_PALETTE_MARKER);
    out.extend_from_slice(palette);
    Ok(out)
}

/// 8 bpp × 1 plane grey bytes with `palette_info = 2`; `tail` appends
/// the 256-entry grey-ramp VGA block (the flag wins over the tail on
/// decode, so the pixels read back identically either way).
pub(crate) fn write_gray8(
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    pixels: &[u8],
    tail: bool,
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(pixels.len(), width, height, 1, "grayscale")?;
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + bytes_per_line as usize * height as usize);
    write_header(
        &mut out,
        meta,
        width,
        height,
        8,
        1,
        bytes_per_line,
        &[0u8; 48],
        2, // palette_info = 2 → grayscale per spec §3
    );
    let mut row = Vec::with_capacity(bytes_per_line as usize);
    for y in 0..height as usize {
        row.clear();
        row.extend_from_slice(&pixels[y * width as usize..y * width as usize + width as usize]);
        row.resize(bytes_per_line as usize, 0);
        rle::encode(&row, &mut out);
    }
    if tail {
        out.push(PCX_VGA_PALETTE_MARKER);
        for i in 0..=255u8 {
            out.extend_from_slice(&[i, i, i]);
        }
    }
    Ok(out)
}

/// 8 bpp × 3 planes from packed RGB.
pub(crate) fn write_rgb24(
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    rgb: &[u8],
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(rgb.len(), width, height, 3, "rgb")?;
    check_origin(meta, width, height)?;
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + rgb.len() / 2);
    write_header(
        &mut out,
        meta,
        width,
        height,
        8,
        3,
        bytes_per_line,
        &[0u8; 48],
        1,
    );
    let mut row = Vec::with_capacity(bytes_per_line as usize * 3);
    for y in 0..height as usize {
        row.clear();
        // Plane R, then plane G, then plane B (each `bytes_per_line`
        // bytes long).
        for plane in 0..3 {
            for x in 0..width as usize {
                let off = (y * width as usize + x) * 3 + plane;
                row.push(rgb[off]);
            }
            row.resize((plane + 1) * bytes_per_line as usize, 0);
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// 1 bpp × 1 plane from one byte per pixel (non-zero = bit 1), with the
/// given header colormap.
pub(crate) fn write_mono(
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    pixels: &[u8],
    colormap: &[u8; 48],
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(pixels.len(), width, height, 1, "1bpp")?;
    let bytes_per_line = round_up_to_even(width.div_ceil(8));
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * height as usize);
    write_header(
        &mut out,
        meta,
        width,
        height,
        1,
        1,
        bytes_per_line,
        colormap,
        1,
    );
    let mut row = vec![0u8; bytes_per_line as usize];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        let line = &pixels[y * width as usize..];
        pack_1bpp_plane_row(&mut row, width as usize, |x| line[x] != 0);
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// 1 bpp × N planes (N = 2 / 3 / 4) from one index byte per pixel: bit
/// `k` of each index goes to plane `k`.
fn write_bitplanes(
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    indices: &[u8],
    n_planes: u8,
    colormap: &[u8; 48],
    what: &str,
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(indices.len(), width, height, 1, what)?;
    let planes = n_planes as usize;
    let bytes_per_line = round_up_to_even(width.div_ceil(8));
    let mut out =
        Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * planes * height as usize);
    write_header(
        &mut out,
        meta,
        width,
        height,
        1,
        n_planes,
        bytes_per_line,
        colormap,
        1,
    );
    let mut row = vec![0u8; bytes_per_line as usize * planes];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        let line = &indices[y * width as usize..];
        let bpl = bytes_per_line as usize;
        for plane in 0..planes {
            let dst = &mut row[plane * bpl..plane * bpl + bpl];
            pack_1bpp_plane_row(dst, width as usize, |x| (line[x] >> plane) & 1 != 0);
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// 1 bpp × 4 planes EGA with the 48-byte header colormap.
pub(crate) fn write_indexed1x4(
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    indices: &[u8],
    colormap: &[u8; 48],
) -> Result<Vec<u8>> {
    write_bitplanes(meta, width, height, indices, 4, colormap, "EGA")
}

/// 1 bpp × 3 planes EGA RGB from 3-bit indices (`r | g << 1 | b << 2`).
pub(crate) fn write_ega_rgb1x3(
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    indices: &[u8],
) -> Result<Vec<u8>> {
    write_bitplanes(meta, width, height, indices, 3, &[0u8; 48], "rgb")
}

/// The 48-byte CGA colour map: background nibble in header byte 16 =
/// colormap byte 0; C / P / I selector in header byte 19 = colormap
/// byte 3 (manual §"CGA Color Map").
fn cga_colormap(palette_selector: u8, background_index: u8) -> Result<[u8; 48]> {
    if background_index > 0x0F {
        return Err(Error::invalid(format!(
            "PCX encoder: CGA background_index must be 0..15, got {background_index}"
        )));
    }
    let mut ega = [0u8; 48];
    ega[0] = background_index << 4;
    ega[3] = palette_selector;
    Ok(ega)
}

/// 1 bpp × 2 planes CGA from 2-bit indices.
pub(crate) fn write_cga1x2(
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    indices: &[u8],
    palette_selector: u8,
    background_index: u8,
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(indices.len(), width, height, 1, "CGA")?;
    let colormap = cga_colormap(palette_selector, background_index)?;
    write_bitplanes(meta, width, height, indices, 2, &colormap, "CGA")
}

/// 2 bpp × 1 plane CGA from 2-bit indices (4 pixels per byte, MSB
/// first).
pub(crate) fn write_cga2x1(
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    indices: &[u8],
    palette_selector: u8,
    background_index: u8,
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(indices.len(), width, height, 1, "2bpp")?;
    let colormap = cga_colormap(palette_selector, background_index)?;
    let bytes_per_line = round_up_to_even(width.div_ceil(4));
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * height as usize);
    write_header(
        &mut out,
        meta,
        width,
        height,
        2,
        1,
        bytes_per_line,
        &colormap,
        1,
    );
    let mut row = vec![0u8; bytes_per_line as usize];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let v = indices[y * width as usize + x] & 0b11;
            let shift = 6 - 2 * (x % 4);
            row[x / 4] |= v << shift;
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// 4 bpp × 1 plane packed nibbles with the 48-byte header colormap.
pub(crate) fn write_indexed4(
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    indices: &[u8],
    colormap: &[u8; 48],
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(indices.len(), width, height, 1, "4bpp")?;
    let bytes_per_line = round_up_to_even(width.div_ceil(2));
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * height as usize);
    write_header(
        &mut out,
        meta,
        width,
        height,
        4,
        1,
        bytes_per_line,
        colormap,
        1,
    );
    let mut row = vec![0u8; bytes_per_line as usize];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let v = indices[y * width as usize + x] & 0x0F;
            if x % 2 == 0 {
                row[x / 2] |= v << 4;
            } else {
                row[x / 2] |= v;
            }
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

fn colormap48(palette: &[u8]) -> Result<[u8; 48]> {
    if palette.len() != 48 {
        return Err(Error::invalid(format!(
            "PCX encoder: 16-colour palette must be exactly 48 bytes (16 RGB triplets), got {}",
            palette.len()
        )));
    }
    let mut ega = [0u8; 48];
    ega.copy_from_slice(palette);
    Ok(ega)
}

// ---------------------------------------------------------------------------
// Palette analysis shared by the contract encoder and the ladders
// ---------------------------------------------------------------------------

/// Scan packed RGB into `(first-seen indices, palette)` when the image
/// has `≤ 256` distinct colours, else `None`. Palette entry order is the
/// raster-scan discovery order, so the same input always yields the
/// same table.
///
/// Colour → index resolution goes through a `HashMap` keyed on the
/// packed 24-bit colour (a linear probe of the palette would be
/// `O(colours × pixels)`), with a one-entry last-colour cache for the
/// run-heavy inputs PCX RLE exists for.
pub(crate) fn first_seen_indexed(rgb: &[u8]) -> Option<(Vec<u8>, Vec<[u8; 3]>)> {
    use std::collections::HashMap;
    let n_pixels = rgb.len() / 3;
    let mut palette: Vec<[u8; 3]> = Vec::with_capacity(256);
    let mut seen: HashMap<u32, u8> = HashMap::with_capacity(257);
    let mut indices: Vec<u8> = Vec::with_capacity(n_pixels);
    let mut last: Option<(u32, u8)> = None;
    for p in rgb[..n_pixels * 3].chunks_exact(3) {
        let key = u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]);
        if let Some((lk, li)) = last {
            if lk == key {
                indices.push(li);
                continue;
            }
        }
        let idx = match seen.get(&key) {
            Some(&i) => i,
            None => {
                if palette.len() == 256 {
                    return None;
                }
                let i = palette.len() as u8;
                palette.push([p[0], p[1], p[2]]);
                seen.insert(key, i);
                i
            }
        };
        indices.push(idx);
        last = Some((key, idx));
    }
    Some((indices, palette))
}

/// Search the fixed CGA hardware palette space for an exact match of
/// `palette` (≤ 4 entries), for the CGA geometries.
///
/// CGA stores no colour data: header byte 19's upper three C / P / I
/// bits (spec §"CGA Color Map") select one of four fixed chroma
/// palettes or two composite-monochrome grey ramps, and header byte
/// 16's high nibble picks palette entry 0 (the background) out of the
/// 16 standard EGA colours. So a colour set is CGA-representable iff
/// some `(selector, background)` pair yields a 4-entry palette
/// containing every entry. The search space is 6 selectors × 16
/// backgrounds = 96 resolved palettes, each resolved through the
/// *decoder's own* header resolver so encode-side matching and
/// decode-side reconstruction can never drift apart.
///
/// Returns the first match in a fixed scan order (selector `0x60`
/// white-bright, `0x40` white-dim, `0x20` yellow-bright, `0x00`
/// yellow-dim, `0x80` monochrome-dim, `0xA0` monochrome-bright;
/// background `0..=15`) plus a source-index → CGA-index LUT (first
/// matching palette entry). `None` when `palette.len() > 4` or no
/// hardware palette covers the set.
pub(crate) fn cga_match(palette: &[[u8; 3]]) -> Option<(u8, u8, [u8; 4])> {
    if palette.len() > 4 {
        return None;
    }
    // Chroma families first (white-bright is the era's most common
    // palette), then the two composite-monochrome ramps the manual's
    // C bit unlocks — grey quads like 0x00/0x55/0xAA/0xFF are
    // CGA-representable through them.
    for &selector in &[0x60u8, 0x40, 0x20, 0x00, 0x80, 0xA0] {
        for background in 0..16u8 {
            let mut raw = [0u8; 48];
            raw[0] = background << 4;
            raw[3] = selector;
            let pal4 = crate::decoder::cga_palette_from_header(&raw);
            let mut lut = [0u8; 4];
            let mut ok = true;
            for (i, c) in palette.iter().enumerate() {
                match pal4.iter().position(|p| p == c) {
                    Some(j) => lut[i] = j as u8,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                return Some((selector, background, lut));
            }
        }
    }
    None
}

/// Index of an on/off primary in [`crate::decoder::RGB_PRIMARIES_PALETTE`]
/// (`r | g << 1 | b << 2`), or `None` when a channel is neither `0x00`
/// nor `0xFF`.
fn primary_index(c: [u8; 3]) -> Option<u8> {
    let bit = |v: u8| match v {
        0x00 => Some(0u8),
        0xFF => Some(1u8),
        _ => None,
    };
    Some(bit(c[0])? | (bit(c[1])? << 1) | (bit(c[2])? << 2))
}

/// First-seen (or verbatim) indices plus their RGB palette.
type Indexed<'a> = (Cow<'a, [u8]>, Vec<[u8; 3]>);

/// A `PcxImage` reduced to the forms the geometry writers consume.
enum Source<'a> {
    /// Packed RGB (`Rgb24`, or `Rgba` with alpha dropped).
    Rgb(Cow<'a, [u8]>),
    /// Grey bytes.
    Gray(Cow<'a, [u8]>),
    /// Indices + an opaque RGB palette (verbatim from the image).
    Indexed(Cow<'a, [u8]>, Vec<[u8; 3]>),
}

impl Source<'_> {
    /// First-seen indices + palette for any source, or `None` with more
    /// than 256 distinct colours. For an indexed source the image's own
    /// palette is kept verbatim.
    fn indexed(&self) -> Option<Indexed<'_>> {
        match self {
            Source::Indexed(idx, pal) => Some((Cow::Borrowed(idx), pal.clone())),
            Source::Gray(g) => {
                let rgb: Vec<u8> = g.iter().flat_map(|&v| [v, v, v]).collect();
                first_seen_indexed(&rgb).map(|(i, p)| (Cow::Owned(i), p))
            }
            Source::Rgb(rgb) => first_seen_indexed(rgb).map(|(i, p)| (Cow::Owned(i), p)),
        }
    }

    fn rgb(&self) -> Cow<'_, [u8]> {
        match self {
            Source::Rgb(rgb) => Cow::Borrowed(rgb),
            Source::Gray(g) => Cow::Owned(g.iter().flat_map(|&v| [v, v, v]).collect()),
            Source::Indexed(idx, pal) => Cow::Owned(
                idx.iter()
                    .flat_map(|&i| pal.get(usize::from(i)).copied().unwrap_or([0, 0, 0]))
                    .collect(),
            ),
        }
    }
}

/// Tightly pack the image's plane (drop the stride padding) into
/// `width × bytes_per_pixel × height` bytes.
fn tight(image: &PcxImage) -> Cow<'_, [u8]> {
    let w = image.width as usize * image.bytes_per_pixel();
    let stride = image.stride();
    let data = image.data();
    if stride == w {
        return Cow::Borrowed(&data[..w * image.height as usize]);
    }
    let mut out = Vec::with_capacity(w * image.height as usize);
    for y in 0..image.height as usize {
        out.extend_from_slice(&data[y * stride..y * stride + w]);
    }
    Cow::Owned(out)
}

fn source<'a>(image: &'a PcxImage, opts: &EncodeOptions) -> Result<Source<'a>> {
    let data = tight(image);
    Ok(match image.format {
        PcxPixelFormat::Rgb24 => Source::Rgb(data),
        PcxPixelFormat::Rgba => {
            if !opts.drop_alpha {
                return Err(Error::unsupported(
                    "PCX encoder: PCX has no alpha mechanism; convert the image to Rgb24 \
                     or set EncodeOptions::drop_alpha",
                ));
            }
            let rgb: Vec<u8> = data
                .chunks_exact(4)
                .flat_map(|c| [c[0], c[1], c[2]])
                .collect();
            Source::Rgb(Cow::Owned(rgb))
        }
        PcxPixelFormat::Gray8 => Source::Gray(data),
        PcxPixelFormat::Pal8 => {
            let pal = image
                .palette
                .as_ref()
                .ok_or_else(|| Error::invalid("PCX encoder: Pal8 image without a palette"))?;
            if pal.has_alpha() && !opts.drop_alpha {
                return Err(Error::unsupported(
                    "PCX encoder: palette entries carry alpha, which PCX cannot store; set \
                     EncodeOptions::drop_alpha",
                ));
            }
            let rgb: Vec<[u8; 3]> = pal.entries.iter().map(|e| [e[0], e[1], e[2]]).collect();
            Source::Indexed(data, rgb)
        }
    })
}

/// Encode `src` in exactly `layout`, or [`Error::Unsupported`] when the
/// pixels cannot be stored losslessly in that geometry.
fn encode_layout(
    layout: PcxLayout,
    src: &Source<'_>,
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    gray_tail: bool,
) -> Result<Vec<u8>> {
    let unfit = |why: &str| {
        Error::unsupported(format!(
            "PCX encoder: image does not fit the {layout:?} geometry: {why}"
        ))
    };
    match layout {
        PcxLayout::Rgb24 => write_rgb24(meta, width, height, &src.rgb()),
        PcxLayout::Gray8 => match src {
            Source::Gray(g) => write_gray8(meta, width, height, g, gray_tail),
            _ => {
                let rgb = src.rgb();
                let mut gray = Vec::with_capacity(rgb.len() / 3);
                for p in rgb.chunks_exact(3) {
                    if p[0] != p[1] || p[1] != p[2] {
                        return Err(unfit("a pixel is not a pure grey"));
                    }
                    gray.push(p[0]);
                }
                write_gray8(meta, width, height, &gray, gray_tail)
            }
        },
        PcxLayout::Indexed8 => {
            if let Source::Gray(g) = src {
                // The grey level is its own index into the 0..=255 ramp.
                let mut pal = vec![0u8; PCX_VGA_PALETTE_BYTES];
                for (i, e) in pal.chunks_exact_mut(3).enumerate() {
                    e.fill(i as u8);
                }
                return write_indexed8(meta, width, height, g, &pal);
            }
            let (idx, pal) = src
                .indexed()
                .ok_or_else(|| unfit("more than 256 colours"))?;
            let mut pal768 = vec![0u8; PCX_VGA_PALETTE_BYTES];
            for (dst, e) in pal768.chunks_exact_mut(3).zip(pal.iter()) {
                dst.copy_from_slice(e);
            }
            write_indexed8(meta, width, height, &idx, &pal768)
        }
        PcxLayout::Indexed4 | PcxLayout::Indexed1x4 => {
            let (idx, pal) = src
                .indexed()
                .ok_or_else(|| unfit("more than 256 colours"))?;
            if pal.len() > 16 {
                return Err(unfit("more than 16 palette entries"));
            }
            if idx.iter().any(|&i| i > 0x0F) {
                return Err(unfit("a palette index exceeds 15"));
            }
            if pal.iter().flatten().all(|&b| b == 0) {
                // An all-zero 16-entry colormap is indistinguishable from
                // the "unset" header a PCX 3.0+ writer emits, which
                // readers (this crate included) resolve to the EGA
                // hardware default.
                return Err(unfit(
                    "an all-black colormap would read back as the EGA hardware default",
                ));
            }
            let mut cm = [0u8; 48];
            for (dst, e) in cm.chunks_exact_mut(3).zip(pal.iter()) {
                dst.copy_from_slice(e);
            }
            if layout == PcxLayout::Indexed4 {
                write_indexed4(meta, width, height, &idx, &cm)
            } else {
                write_indexed1x4(meta, width, height, &idx, &cm)
            }
        }
        PcxLayout::Mono1 => {
            let (idx, pal) = src
                .indexed()
                .ok_or_else(|| unfit("more than 256 colours"))?;
            if pal.len() > 2 {
                return Err(unfit("more than two colours"));
            }
            // RGB / grey sources take the canonical black / white mapping
            // (bit 1 = white, spec §4.1) and only that: the geometry is
            // "monochrome" to every bit-driven reader, so a derived
            // palette is never widened to arbitrary colours. A `Pal8`
            // source stores its own two entries in colormap 0 / 1
            // verbatim, as the EGFF colormap reading permits.
            if !matches!(src, Source::Indexed(..)) {
                if !pal
                    .iter()
                    .all(|c| *c == [0, 0, 0] || *c == [0xFF, 0xFF, 0xFF])
                {
                    return Err(unfit("a colour is neither black nor white"));
                }
                let lut: Vec<u8> = pal.iter().map(|c| u8::from(*c == [0xFF; 3])).collect();
                let bits: Vec<u8> = idx.iter().map(|&i| lut[usize::from(i)]).collect();
                return write_mono(meta, width, height, &bits, &MONO_COLORMAP);
            }
            let mut cm = [0u8; 48];
            for (dst, e) in cm.chunks_exact_mut(3).zip(pal.iter()) {
                dst.copy_from_slice(e);
            }
            if cm.iter().all(|&b| b == 0) && idx.iter().any(|&i| i != 0) {
                return Err(unfit(
                    "an all-black two-entry colormap would read back as black / white",
                ));
            }
            write_mono(meta, width, height, &idx, &cm)
        }
        PcxLayout::Cga2x1 | PcxLayout::Cga1x2 => {
            let (idx, pal) = src
                .indexed()
                .ok_or_else(|| unfit("more than 256 colours"))?;
            let (selector, background, lut) = cga_match(&pal)
                .ok_or_else(|| unfit("the colours are not a CGA hardware palette"))?;
            let cga: Vec<u8> = idx.iter().map(|&i| lut[usize::from(i)]).collect();
            if layout == PcxLayout::Cga2x1 {
                write_cga2x1(meta, width, height, &cga, selector, background)
            } else {
                write_cga1x2(meta, width, height, &cga, selector, background)
            }
        }
        PcxLayout::EgaRgb1x3 => {
            let (idx, pal) = src
                .indexed()
                .ok_or_else(|| unfit("more than 256 colours"))?;
            let mut lut = Vec::with_capacity(pal.len());
            for c in &pal {
                lut.push(
                    primary_index(*c).ok_or_else(|| {
                        unfit("a colour is not one of the eight on/off primaries")
                    })?,
                );
            }
            let bits3: Vec<u8> = idx.iter().map(|&i| lut[usize::from(i)]).collect();
            write_ega_rgb1x3(meta, width, height, &bits3)
        }
    }
}

/// The compact ladder's candidate order; an earlier candidate keeps an
/// exact size tie.
pub(crate) const COMPACT_LADDER: [PcxLayout; 9] = [
    PcxLayout::Mono1,
    PcxLayout::Cga2x1,
    PcxLayout::Cga1x2,
    PcxLayout::EgaRgb1x3,
    PcxLayout::Indexed4,
    PcxLayout::Indexed1x4,
    PcxLayout::Gray8,
    PcxLayout::Indexed8,
    PcxLayout::Rgb24,
];

/// Resolve the header annotations from the options and the image.
fn header_meta(image: &PcxImage, opts: &EncodeOptions) -> Result<HeaderMeta> {
    if !matches!(opts.version, 0 | 2 | 3 | 4 | 5) {
        return Err(Error::invalid(format!(
            "PCX encoder: version byte {} is not one of the spec's 0 / 2 / 3 / 4 / 5",
            opts.version
        )));
    }
    let dpi = match opts.dpi.or(image.dpi) {
        Some(d) => {
            check_dpi(d)?;
            d
        }
        None => (0, 0),
    };
    let screen_size = match opts.screen_size.or(image.screen_size) {
        Some(s) => {
            check_screen_size(s)?;
            s
        }
        None => (0, 0),
    };
    let (x_min, y_min) = opts.window_origin.or(image.window_origin).unwrap_or((0, 0));
    Ok(HeaderMeta {
        x_min,
        y_min,
        dpi,
        screen_size,
        version: opts.version,
    })
}

fn dims16(image: &PcxImage) -> Result<(u16, u16)> {
    let w: u16 = image
        .width
        .try_into()
        .map_err(|_| Error::unsupported("PCX encoder: width exceeds 65535"))?;
    let h: u16 = image
        .height
        .try_into()
        .map_err(|_| Error::unsupported("PCX encoder: height exceeds 65535"))?;
    Ok((w, h))
}

/// [`crate::encode`]: write `image` per `opts`.
pub(crate) fn encode_image(image: &PcxImage, opts: &EncodeOptions) -> Result<Vec<u8>> {
    let (bytes, _layout) = encode_image_reporting(image, opts)?;
    Ok(bytes)
}

/// [`encode_image`] that also reports the geometry written (the one
/// compact mode picked, or the forced / natural one).
pub(crate) fn encode_image_reporting(
    image: &PcxImage,
    opts: &EncodeOptions,
) -> Result<(Vec<u8>, PcxLayout)> {
    image.validate()?;
    let (w, h) = dims16(image)?;
    let meta = header_meta(image, opts)?;
    check_origin(&meta, w, h)?;
    let src = source(image, opts)?;
    if let Some(layout) = opts.layout {
        return Ok((
            encode_layout(layout, &src, &meta, w, h, opts.gray_tail)?,
            layout,
        ));
    }
    if opts.compact {
        let mut best: Option<(Vec<u8>, PcxLayout)> = None;
        for layout in COMPACT_LADDER {
            let Ok(bytes) = encode_layout(layout, &src, &meta, w, h, opts.gray_tail) else {
                continue;
            };
            let better = match &best {
                None => true,
                Some((b, _)) => bytes.len() < b.len(),
            };
            if better {
                best = Some((bytes, layout));
            }
        }
        return best.ok_or_else(|| Error::unsupported("PCX encoder: no geometry fits the image"));
    }
    let layout = image
        .layout
        .unwrap_or_else(|| PcxLayout::natural_for(image.format, image.palette.as_ref()));
    Ok((
        encode_layout(layout, &src, &meta, w, h, opts.gray_tail)?,
        layout,
    ))
}

// ---------------------------------------------------------------------------
// Pre-contract writers, kept for one release as thin wrappers
// ---------------------------------------------------------------------------

/// Encode `width × height` indexed pixels (one byte per pixel,
/// row-major, top-down) into a PCX 5.0 file with an appended 256-entry
/// VGA palette (`palette` = exactly 768 RGB bytes), 72 × 72 DPI.
#[deprecated(note = "use oxideav_pcx::encode with a Pal8 PcxImage (IMAGE_CRATE_API)")]
pub fn encode_pcx_8bpp_indexed(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
) -> Result<Vec<u8>> {
    write_indexed8(&HeaderMeta::LEGACY, width, height, indices, palette)
}

/// Encode `width × height` packed RGB bytes (3 bytes per pixel,
/// row-major, top-down) into a PCX 5.0 file with three planes (R, G,
/// B) at 8 bpp each, 72 × 72 DPI. No tail palette is appended.
#[deprecated(note = "use oxideav_pcx::encode_rgb8 (IMAGE_CRATE_API)")]
pub fn encode_pcx_24bpp(width: u16, height: u16, rgb: &[u8]) -> Result<Vec<u8>> {
    write_rgb24(&HeaderMeta::LEGACY, width, height, rgb)
}

/// The PCX 5.0 mode the pre-contract compact writers selected, returned
/// alongside the encoded bytes.
#[deprecated(note = "use oxideav_pcx::PcxLayout (IMAGE_CRATE_API)")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcxAutoMode {
    /// `≤ 256` distinct colours: 8 bpp × 1 plane indexed image plus a
    /// 256-entry VGA tail palette. The `usize` is the number of
    /// distinct colours found (`1..=256`).
    Indexed8 { colors: usize },
    /// `> 256` distinct colours: 8 bpp × 3 plane planar RGB, no tail
    /// palette.
    Rgb24,
    /// Every distinct colour is a pure grey: 8 bpp × 1 plane with
    /// `palette_info = 2` and no VGA tail.
    Gray8,
    /// Every distinct colour is pure black or pure white: 1 bpp × 1
    /// plane monochrome.
    Mono1,
    /// Every distinct colour has each channel at `0x00` or `0xFF`: 1 bpp
    /// × 3 planes, no stored palette.
    EgaRgb1x3,
    /// `≤ 16` distinct colours: 4 bpp × 1 plane packed nibbles with the
    /// palette in the header colormap.
    Indexed4 { colors: usize },
    /// `≤ 16` distinct colours in the plane-oriented 1 bpp × 4 planes
    /// geometry.
    Indexed1x4 { colors: usize },
    /// `≤ 4` distinct colours exactly representable by one CGA hardware
    /// palette: 2 bpp × 1 plane packed bits.
    Cga2x1 {
        palette_selector: u8,
        background_index: u8,
    },
    /// The same CGA precondition in the plane-oriented 1 bpp × 2 plane
    /// layout.
    Cga1x2 {
        palette_selector: u8,
        background_index: u8,
    },
}

#[allow(deprecated)]
fn auto_mode(layout: PcxLayout, colors: usize, cga: Option<(u8, u8)>) -> PcxAutoMode {
    match layout {
        PcxLayout::Indexed8 => PcxAutoMode::Indexed8 { colors },
        PcxLayout::Rgb24 => PcxAutoMode::Rgb24,
        PcxLayout::Gray8 => PcxAutoMode::Gray8,
        PcxLayout::Mono1 => PcxAutoMode::Mono1,
        PcxLayout::EgaRgb1x3 => PcxAutoMode::EgaRgb1x3,
        PcxLayout::Indexed4 => PcxAutoMode::Indexed4 { colors },
        PcxLayout::Indexed1x4 => PcxAutoMode::Indexed1x4 { colors },
        PcxLayout::Cga2x1 => {
            let (palette_selector, background_index) = cga.unwrap_or((0, 0));
            PcxAutoMode::Cga2x1 {
                palette_selector,
                background_index,
            }
        }
        PcxLayout::Cga1x2 => {
            let (palette_selector, background_index) = cga.unwrap_or((0, 0));
            PcxAutoMode::Cga1x2 {
                palette_selector,
                background_index,
            }
        }
    }
}

/// The pre-contract compact ladder over packed RGB with the given header
/// annotations: every applicable candidate in [`COMPACT_LADDER`] order,
/// fewest bytes wins, ties keep the earlier.
#[allow(deprecated)]
fn rgb_auto(
    meta: &HeaderMeta,
    width: u16,
    height: u16,
    rgb: &[u8],
) -> Result<(Vec<u8>, PcxAutoMode)> {
    check_dims(width, height)?;
    check_len(rgb.len(), width, height, 3, "rgb")?;
    let rgb = &rgb[..width as usize * height as usize * 3];
    let src = Source::Rgb(Cow::Borrowed(rgb));
    let indexed = first_seen_indexed(rgb);
    let Some((_, palette)) = &indexed else {
        return Ok((write_rgb24(meta, width, height, rgb)?, PcxAutoMode::Rgb24));
    };
    let colors = palette.len();
    let cga = cga_match(palette).map(|(s, b, _)| (s, b));
    let mut best: Option<(Vec<u8>, PcxAutoMode)> = None;
    for layout in COMPACT_LADDER {
        let Ok(bytes) = encode_layout(layout, &src, meta, width, height, false) else {
            continue;
        };
        let better = match &best {
            None => true,
            Some((b, _)) => bytes.len() < b.len(),
        };
        if better {
            best = Some((bytes, auto_mode(layout, colors, cga)));
        }
    }
    Ok(best.expect("the 24-bit candidate always applies"))
}

/// Encode `width × height` packed RGB bytes into the **most compact**
/// valid PCX 5.0 file (72 × 72 DPI): every spec geometry whose
/// losslessness precondition holds — monochrome, both CGA layouts, EGA
/// RGB, both 16-colour layouts, grayscale, 256-colour indexed, 24-bit —
/// is encoded and the fewest bytes win (ties keep the earlier candidate
/// in that order). Exact by construction on every rung.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::compact (IMAGE_CRATE_API)")]
#[allow(deprecated)]
pub fn encode_pcx_rgb_auto(width: u16, height: u16, rgb: &[u8]) -> Result<(Vec<u8>, PcxAutoMode)> {
    rgb_auto(&HeaderMeta::LEGACY, width, height, rgb)
}

/// Encode `width × height` palette indices plus a **caller-supplied**
/// packed-RGB palette (non-empty, a multiple of 3 bytes, ≤ 768) into
/// the most compact PCX 5.0 geometry that stores that palette
/// *verbatim*: the two 16-entry header-colormap rungs (4 bpp × 1 plane
/// / 1 bpp × 4 planes) when the table has ≤ 16 entries, every index is
/// ≤ 15 and at least one palette byte is non-zero, else the 8 bpp +
/// VGA-tail rung. Indices at or beyond the entry count resolve to the
/// zero padding. 72 × 72 DPI.
#[deprecated(note = "use oxideav_pcx::encode with a Pal8 PcxImage (IMAGE_CRATE_API)")]
#[allow(deprecated)]
pub fn encode_pcx_indexed_auto(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
) -> Result<(Vec<u8>, PcxAutoMode)> {
    check_dims(width, height)?;
    let n_pixels = width as usize * height as usize;
    check_len(indices.len(), width, height, 1, "indexed")?;
    if palette.is_empty() || palette.len() % 3 != 0 || palette.len() > PCX_VGA_PALETTE_BYTES {
        return Err(Error::invalid(format!(
            "PCX encoder: caller palette must be packed RGB triplets — non-empty, a multiple \
             of 3 bytes, at most {PCX_VGA_PALETTE_BYTES} (got {})",
            palette.len()
        )));
    }
    let meta = &HeaderMeta::LEGACY;
    let colors = palette.len() / 3;
    let mut candidates: Vec<(Vec<u8>, PcxAutoMode)> = Vec::new();
    let header_rung_ok = colors <= 16
        && indices[..n_pixels].iter().all(|&i| i <= 0x0F)
        && palette.iter().any(|&b| b != 0);
    if header_rung_ok {
        let mut pal48 = [0u8; 48];
        pal48[..palette.len()].copy_from_slice(palette);
        candidates.push((
            write_indexed4(meta, width, height, indices, &pal48)?,
            PcxAutoMode::Indexed4 { colors },
        ));
        candidates.push((
            write_indexed1x4(meta, width, height, indices, &pal48)?,
            PcxAutoMode::Indexed1x4 { colors },
        ));
    }
    let mut pal768 = vec![0u8; PCX_VGA_PALETTE_BYTES];
    pal768[..palette.len()].copy_from_slice(palette);
    candidates.push((
        write_indexed8(meta, width, height, indices, &pal768)?,
        PcxAutoMode::Indexed8 { colors },
    ));
    let mut it = candidates.into_iter();
    let mut best = it.next().expect("candidate ladder is never empty");
    for cand in it {
        if cand.0.len() < best.0.len() {
            best = cand;
        }
    }
    Ok(best)
}

/// Encode `width × height` packed RGB bytes into an 8-colour PCX 5.0
/// file at 1 bpp × 3 planes (72 × 72 DPI). Each input byte is
/// thresholded at `0x80` to decide whether its channel bit is set;
/// plane order is R, G, B.
#[deprecated(
    note = "use oxideav_pcx::encode with EncodeOptions::layout = EgaRgb1x3 (IMAGE_CRATE_API)"
)]
pub fn encode_pcx_1bpp_3planes_ega_rgb(width: u16, height: u16, rgb: &[u8]) -> Result<Vec<u8>> {
    ega_rgb_threshold(&HeaderMeta::LEGACY, width, height, rgb)
}

fn ega_rgb_threshold(meta: &HeaderMeta, width: u16, height: u16, rgb: &[u8]) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(rgb.len(), width, height, 3, "rgb")?;
    let bits3: Vec<u8> = rgb[..width as usize * height as usize * 3]
        .chunks_exact(3)
        .map(|p| {
            u8::from(p[0] >= 0x80) | (u8::from(p[1] >= 0x80) << 1) | (u8::from(p[2] >= 0x80) << 2)
        })
        .collect();
    write_ega_rgb1x3(meta, width, height, &bits3)
}

/// Encode `width × height` 1-bit pixels (one byte per pixel, zero or
/// non-zero) into a PCX 5.0 monochrome file (72 × 72 DPI). Bit 1 =
/// white and bit 0 = black per spec §4.1; the header colormap carries
/// black / white.
#[deprecated(note = "use oxideav_pcx::encode with a two-colour Pal8 PcxImage (IMAGE_CRATE_API)")]
pub fn encode_pcx_1bpp_mono(width: u16, height: u16, pixels: &[u8]) -> Result<Vec<u8>> {
    write_mono(&HeaderMeta::LEGACY, width, height, pixels, &MONO_COLORMAP)
}

/// Encode `width × height` 4-bit-index pixels (low nibble = palette
/// index) into a PCX 5.0 4 bpp packed file with the 16-entry (48-byte)
/// `palette` in the header colormap, 72 × 72 DPI.
#[deprecated(
    note = "use oxideav_pcx::encode with EncodeOptions::layout = Indexed4 (IMAGE_CRATE_API)"
)]
pub fn encode_pcx_4bpp_packed(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(indices.len(), width, height, 1, "4bpp")?;
    let cm = colormap48(palette).map_err(|_| {
        Error::invalid(format!(
            "PCX encoder: 4bpp palette must be exactly 48 bytes (16 RGB triplets), got {}",
            palette.len()
        ))
    })?;
    write_indexed4(&HeaderMeta::LEGACY, width, height, indices, &cm)
}

/// Encode `width × height` 2-bit-index pixels into a PCX 5.0 2 bpp CGA
/// file (72 × 72 DPI). `palette_selector` is header byte 19 (upper
/// three bits C / P / I: `0x60` palette 1 bright, `0x40` palette 1 dim,
/// `0x20` palette 0 bright, `0x00` palette 0 dim, `0x80` / `0xA0`
/// composite-monochrome dim / bright); `background_index` (0..=15) is
/// the EGA index of palette entry 0 (header byte 16 high nibble).
#[deprecated(
    note = "use oxideav_pcx::encode with EncodeOptions::layout = Cga2x1 (IMAGE_CRATE_API)"
)]
pub fn encode_pcx_2bpp_cga(
    width: u16,
    height: u16,
    indices: &[u8],
    palette_selector: u8,
    background_index: u8,
) -> Result<Vec<u8>> {
    write_cga2x1(
        &HeaderMeta::LEGACY,
        width,
        height,
        indices,
        palette_selector,
        background_index,
    )
}

/// [`encode_pcx_2bpp_cga`] taking the C / P / I triple as a
/// [`Pcx2bppCgaCpi`] instead of a raw selector byte.
#[deprecated(
    note = "use oxideav_pcx::encode with EncodeOptions::layout = Cga2x1 (IMAGE_CRATE_API)"
)]
pub fn encode_pcx_2bpp_cga_cpi(
    width: u16,
    height: u16,
    indices: &[u8],
    cpi: Pcx2bppCgaCpi,
    background_index: u8,
) -> Result<Vec<u8>> {
    write_cga2x1(
        &HeaderMeta::LEGACY,
        width,
        height,
        indices,
        cpi.to_byte19(),
        background_index,
    )
}

/// Encode `width × height` 2-bit-index pixels into a PCX 5.0 1 bpp ×
/// 2-plane CGA file (72 × 72 DPI) — the plane-oriented CGA layout; bit
/// `k` of each index goes to plane `k`. Header palette bytes as
/// [`encode_pcx_2bpp_cga`].
#[deprecated(
    note = "use oxideav_pcx::encode with EncodeOptions::layout = Cga1x2 (IMAGE_CRATE_API)"
)]
pub fn encode_pcx_1bpp_2planes_cga(
    width: u16,
    height: u16,
    indices: &[u8],
    palette_selector: u8,
    background_index: u8,
) -> Result<Vec<u8>> {
    write_cga1x2(
        &HeaderMeta::LEGACY,
        width,
        height,
        indices,
        palette_selector,
        background_index,
    )
}

/// Encode `width × height` 4-bit-index pixels into a PCX 5.0 1 bpp ×
/// 4-plane EGA file with the 16-entry (48-byte) `palette` in the header
/// colormap, 72 × 72 DPI. Plane `k` carries bit `k` of the index.
#[deprecated(
    note = "use oxideav_pcx::encode with EncodeOptions::layout = Indexed1x4 (IMAGE_CRATE_API)"
)]
pub fn encode_pcx_1bpp_4planes_ega(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(indices.len(), width, height, 1, "EGA")?;
    let cm = colormap48(palette).map_err(|_| {
        Error::invalid(format!(
            "PCX encoder: EGA palette must be exactly 48 bytes (16 RGB triplets), got {}",
            palette.len()
        ))
    })?;
    write_indexed1x4(&HeaderMeta::LEGACY, width, height, indices, &cm)
}

/// Encode `width × height` 16-bit composite-index pixels into a
/// PCX 5.0 `4 bpp × 4 planes` file (72 × 72 DPI).
///
/// This is the one `(bits_per_pixel, n_planes)` slot the EGFF canonical
/// PCX video-mode matrix does not list as a hardware video mode, but
/// the format is structurally reachable (`MaxNumberOfColors = (1 <<
/// (BitsPerPixel * NumBitPlanes))` = 65536). Each scanline carries
/// plane 0 .. plane 3, each 4 bits per pixel (2 pixels/byte, high
/// nibble first); nibble `k` of the index (`(idx >> (k * 4)) & 0x0F`)
/// goes to plane `k`, so [`crate::parse_pcx_indexed_4bpp_4planes`]
/// round-trips the composite index exactly. No palette is written: the
/// spec defines no 65536-entry palette geometry. There is no
/// [`PcxImage`] layout for this depth, so the writer stays a depth
/// entry point outside the contract vocabulary.
pub fn encode_pcx_4bpp_4planes(width: u16, height: u16, indices: &[u16]) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: 4bpp×4planes input shorter than width × height",
        ));
    }
    let bytes_per_line = round_up_to_even(width.div_ceil(2));
    let mut out =
        Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * 4 * height as usize);
    write_header(
        &mut out,
        &HeaderMeta::LEGACY,
        width,
        height,
        4,
        4,
        bytes_per_line,
        &[0u8; 48],
        1,
    );
    let mut row = vec![0u8; bytes_per_line as usize * 4];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let idx = indices[y * width as usize + x];
            let byte_off = x / 2;
            for plane in 0..4 {
                let nib = ((idx >> (plane * 4)) & 0x0F) as u8;
                let cell = plane * bytes_per_line as usize + byte_off;
                if x % 2 == 0 {
                    row[cell] |= nib << 4;
                } else {
                    row[cell] |= nib;
                }
            }
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode an 8 bpp × 1 plane grayscale PCX with `palette_info = 2` and
/// no tail palette (72 × 72 DPI); `pixels` is one grey byte per pixel.
#[deprecated(note = "use oxideav_pcx::encode with a Gray8 PcxImage (IMAGE_CRATE_API)")]
pub fn encode_pcx_8bpp_grayscale(width: u16, height: u16, pixels: &[u8]) -> Result<Vec<u8>> {
    write_gray8(&HeaderMeta::LEGACY, width, height, pixels, false)
}

/// Encode a 24-bit PCX with a non-zero window origin `(x_min, y_min)`
/// (header metadata only; the pixel buffer is not shifted), 72 × 72 DPI.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::window_origin (IMAGE_CRATE_API)")]
pub fn encode_pcx_24bpp_window(
    x_min: u16,
    y_min: u16,
    width: u16,
    height: u16,
    rgb: &[u8],
) -> Result<Vec<u8>> {
    write_rgb24(
        &HeaderMeta::LEGACY.with_origin(x_min, y_min),
        width,
        height,
        rgb,
    )
}

/// The pre-contract `PcxImage` → 24-bit writer: accepts `Rgb24` and
/// `Rgba` (alpha dropped), rejects paletted / grey images, and threads
/// the image's `dpi` (72 × 72 when `None`), `window_origin` and
/// `screen_size` into the header.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::layout = Rgb24 (IMAGE_CRATE_API)")]
pub fn encode_pcx_24bpp_image(image: &PcxImage) -> Result<Vec<u8>> {
    let (w, h) = dims16(image)?;
    let rgb = legacy_image_rgb(image)?;
    let meta = legacy_meta(image)?;
    write_rgb24(&meta, w, h, &rgb)
}

fn legacy_image_rgb(image: &PcxImage) -> Result<Cow<'_, [u8]>> {
    match image.format {
        PcxPixelFormat::Rgb24 => Ok(tight(image)),
        PcxPixelFormat::Rgba => Ok(Cow::Owned(
            tight(image)
                .chunks_exact(4)
                .flat_map(|c| [c[0], c[1], c[2]])
                .collect(),
        )),
        _ => Err(Error::unsupported(
            "PCX encoder: paletted / grey input needs the contract encode (use oxideav_pcx::encode)",
        )),
    }
}

fn legacy_meta(image: &PcxImage) -> Result<HeaderMeta> {
    let mut meta = HeaderMeta::LEGACY;
    if let Some(dpi) = image.dpi {
        check_dpi(dpi)?;
        meta.dpi = dpi;
    }
    if let Some((x, y)) = image.window_origin {
        meta.x_min = x;
        meta.y_min = y;
    }
    if let Some(s) = image.screen_size {
        check_screen_size(s)?;
        meta.screen_size = s;
    }
    Ok(meta)
}

/// The pre-contract `PcxImage` → compact writer: the
/// [`encode_pcx_rgb_auto`] ladder with the image's DPI threaded through
/// (72 × 72 when `None`); an image carrying a window origin or screen
/// size — fields only the 24-bit header geometry preserved in earlier
/// releases — is written through [`encode_pcx_24bpp_image`] instead and
/// reported as `Rgb24`.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::compact (IMAGE_CRATE_API)")]
#[allow(deprecated)]
pub fn encode_pcx_image_auto(image: &PcxImage) -> Result<(Vec<u8>, PcxAutoMode)> {
    let (w, h) = dims16(image)?;
    let rgb = legacy_image_rgb(image)?;
    if image.window_origin.is_some() || image.screen_size.is_some() {
        let bytes = encode_pcx_24bpp_image(image)?;
        return Ok((bytes, PcxAutoMode::Rgb24));
    }
    let meta = legacy_meta(image)?;
    rgb_auto(&meta, w, h, &rgb)
}

// ---- DPI-bearing variants ----

/// [`encode_pcx_24bpp`] with a custom authoring DPI (both components
/// non-zero).
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::dpi (IMAGE_CRATE_API)")]
pub fn encode_pcx_24bpp_dpi(
    width: u16,
    height: u16,
    rgb: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(rgb.len(), width, height, 3, "rgb")?;
    check_dpi(dpi)?;
    write_rgb24(&HeaderMeta::LEGACY.with_dpi(dpi), width, height, rgb)
}

/// [`encode_pcx_8bpp_indexed`] with a custom authoring DPI.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::dpi (IMAGE_CRATE_API)")]
pub fn encode_pcx_8bpp_indexed_dpi(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(indices.len(), width, height, 1, "indexed")?;
    if palette.len() != PCX_VGA_PALETTE_BYTES {
        return Err(Error::invalid(format!(
            "PCX encoder: 256-colour palette must be exactly {PCX_VGA_PALETTE_BYTES} bytes (got {})",
            palette.len()
        )));
    }
    check_dpi(dpi)?;
    write_indexed8(
        &HeaderMeta::LEGACY.with_dpi(dpi),
        width,
        height,
        indices,
        palette,
    )
}

/// [`encode_pcx_8bpp_grayscale`] with a custom authoring DPI.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::dpi (IMAGE_CRATE_API)")]
pub fn encode_pcx_8bpp_grayscale_dpi(
    width: u16,
    height: u16,
    pixels: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(pixels.len(), width, height, 1, "grayscale")?;
    check_dpi(dpi)?;
    write_gray8(
        &HeaderMeta::LEGACY.with_dpi(dpi),
        width,
        height,
        pixels,
        false,
    )
}

/// [`encode_pcx_1bpp_mono`] with a custom authoring DPI.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::dpi (IMAGE_CRATE_API)")]
pub fn encode_pcx_1bpp_mono_dpi(
    width: u16,
    height: u16,
    pixels: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(pixels.len(), width, height, 1, "1bpp")?;
    check_dpi(dpi)?;
    write_mono(
        &HeaderMeta::LEGACY.with_dpi(dpi),
        width,
        height,
        pixels,
        &MONO_COLORMAP,
    )
}

/// [`encode_pcx_4bpp_packed`] with a custom authoring DPI.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::dpi (IMAGE_CRATE_API)")]
pub fn encode_pcx_4bpp_packed_dpi(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(indices.len(), width, height, 1, "4bpp")?;
    let cm = colormap48(palette).map_err(|_| {
        Error::invalid(format!(
            "PCX encoder: 4bpp palette must be exactly 48 bytes (16 RGB triplets), got {}",
            palette.len()
        ))
    })?;
    check_dpi(dpi)?;
    write_indexed4(
        &HeaderMeta::LEGACY.with_dpi(dpi),
        width,
        height,
        indices,
        &cm,
    )
}

/// [`encode_pcx_2bpp_cga`] with a custom authoring DPI.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::dpi (IMAGE_CRATE_API)")]
pub fn encode_pcx_2bpp_cga_dpi(
    width: u16,
    height: u16,
    indices: &[u8],
    palette_selector: u8,
    background_index: u8,
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(indices.len(), width, height, 1, "2bpp")?;
    cga_colormap(palette_selector, background_index)?;
    check_dpi(dpi)?;
    write_cga2x1(
        &HeaderMeta::LEGACY.with_dpi(dpi),
        width,
        height,
        indices,
        palette_selector,
        background_index,
    )
}

/// [`encode_pcx_1bpp_2planes_cga`] with a custom authoring DPI.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::dpi (IMAGE_CRATE_API)")]
pub fn encode_pcx_1bpp_2planes_cga_dpi(
    width: u16,
    height: u16,
    indices: &[u8],
    palette_selector: u8,
    background_index: u8,
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(indices.len(), width, height, 1, "CGA")?;
    cga_colormap(palette_selector, background_index)?;
    check_dpi(dpi)?;
    write_cga1x2(
        &HeaderMeta::LEGACY.with_dpi(dpi),
        width,
        height,
        indices,
        palette_selector,
        background_index,
    )
}

/// [`encode_pcx_1bpp_3planes_ega_rgb`] with a custom authoring DPI.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::dpi (IMAGE_CRATE_API)")]
pub fn encode_pcx_1bpp_3planes_ega_rgb_dpi(
    width: u16,
    height: u16,
    rgb: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(rgb.len(), width, height, 3, "rgb")?;
    check_dpi(dpi)?;
    ega_rgb_threshold(&HeaderMeta::LEGACY.with_dpi(dpi), width, height, rgb)
}

/// [`encode_pcx_1bpp_4planes_ega`] with a custom authoring DPI.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::dpi (IMAGE_CRATE_API)")]
pub fn encode_pcx_1bpp_4planes_ega_dpi(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(indices.len(), width, height, 1, "EGA")?;
    let cm = colormap48(palette).map_err(|_| {
        Error::invalid(format!(
            "PCX encoder: EGA palette must be exactly 48 bytes (16 RGB triplets), got {}",
            palette.len()
        ))
    })?;
    check_dpi(dpi)?;
    write_indexed1x4(
        &HeaderMeta::LEGACY.with_dpi(dpi),
        width,
        height,
        indices,
        &cm,
    )
}

/// [`encode_pcx_24bpp_window`] with a custom authoring DPI.
#[deprecated(
    note = "use oxideav_pcx::encode with EncodeOptions::window_origin / dpi (IMAGE_CRATE_API)"
)]
pub fn encode_pcx_24bpp_window_dpi(
    x_min: u16,
    y_min: u16,
    width: u16,
    height: u16,
    rgb: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    let meta = HeaderMeta::LEGACY.with_origin(x_min, y_min);
    check_dims(width, height)?;
    check_len(rgb.len(), width, height, 3, "rgb")?;
    check_origin(&meta, width, height)?;
    check_dpi(dpi)?;
    write_rgb24(&meta.with_dpi(dpi), width, height, rgb)
}

/// [`encode_pcx_24bpp`] with a custom PB IV authoring screen size (both
/// components non-zero).
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions::screen_size (IMAGE_CRATE_API)")]
pub fn encode_pcx_24bpp_screen(
    width: u16,
    height: u16,
    rgb: &[u8],
    screen_size: (u16, u16),
) -> Result<Vec<u8>> {
    check_dims(width, height)?;
    check_len(rgb.len(), width, height, 3, "rgb")?;
    check_screen_size(screen_size)?;
    write_rgb24(
        &HeaderMeta::LEGACY.with_screen(screen_size),
        width,
        height,
        rgb,
    )
}

/// [`encode_pcx_24bpp`] with a window origin, a custom DPI and a custom
/// screen size, all three in one call.
#[deprecated(note = "use oxideav_pcx::encode with EncodeOptions (IMAGE_CRATE_API)")]
pub fn encode_pcx_24bpp_window_dpi_screen(
    x_min: u16,
    y_min: u16,
    width: u16,
    height: u16,
    rgb: &[u8],
    dpi: (u16, u16),
    screen_size: (u16, u16),
) -> Result<Vec<u8>> {
    let meta = HeaderMeta::LEGACY.with_origin(x_min, y_min);
    check_dims(width, height)?;
    check_len(rgb.len(), width, height, 3, "rgb")?;
    check_origin(&meta, width, height)?;
    check_dpi(dpi)?;
    check_screen_size(screen_size)?;
    write_rgb24(
        &meta.with_dpi(dpi).with_screen(screen_size),
        width,
        height,
        rgb,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_canonical_mapping_from_rgb_matches_legacy_writer() {
        // black / white RGB in either first-seen order → bit 1 = white.
        let rgb = [0xFF, 0xFF, 0xFF, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0, 0, 0];
        let src = Source::Rgb(Cow::Borrowed(&rgb));
        let got = encode_layout(PcxLayout::Mono1, &src, &HeaderMeta::LEGACY, 4, 1, true).unwrap();
        let want = write_mono(&HeaderMeta::LEGACY, 4, 1, &[1, 0, 1, 0], &MONO_COLORMAP).unwrap();
        assert_eq!(got, want);
    }

    #[test]
    fn cga_match_finds_the_default_white_bright_palette() {
        let pal = [
            [0, 0, 0],
            [0x55, 0xFF, 0xFF],
            [0xFF, 0x55, 0xFF],
            [0xFF, 0xFF, 0xFF],
        ];
        let (sel, bg, lut) = cga_match(&pal).unwrap();
        assert_eq!((sel, bg), (0x60, 0));
        assert_eq!(lut, [0, 1, 2, 3]);
        assert!(cga_match(&[[1, 2, 3]]).is_none());
    }
}
