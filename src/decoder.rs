//! PCX decode: header validation, RLE expansion and the per-geometry
//! unpack into the native layout ([`PcxImage`]).
//!
//! Supports the (depth, planes) combinations called out by spec §4.1
//! and the EGFF mode table:
//!
//! * 1 bpp × 1 plane — monochrome (each bit = one pixel): `Pal8` with
//!   the two header-colormap colours (black / white when zero-filled).
//! * 1 bpp × 2 planes — 4-colour CGA, plane-oriented (the EGFF
//!   canonical mode matrix lists CGA as `BitsPerPixel = 1,
//!   NumBitPlanes = 2`): `Pal8` with the 4-entry CGA palette.
//! * 1 bpp × 3 planes — 8-colour EGA RGB, one plane per primary (plane
//!   order R, G, B per spec §4): `Pal8` with the fixed primaries.
//! * 1 bpp × 4 planes — 16-colour EGA bit-planes: `Pal8` with the
//!   header colormap (or the EGA hardware default when zero-filled).
//! * 2 bpp × 1 plane — 4-colour CGA, packed (4 pixels/byte): `Pal8`,
//!   palette from header byte 16 (background nibble) + header byte 19
//!   (C / P / I bits, "CGA Color Map" selector).
//! * 4 bpp × 1 plane — 16-colour packed-bits (2 pixels/byte): `Pal8`
//!   with the header colormap (or the EGA default).
//! * 8 bpp × 1 plane — 256-colour palette from the 768-byte block at
//!   end-of-file when the byte 769 from EOF is `0x0C` (`Pal8`); with
//!   `palette_info = 2`, or when no tail is present, the pixel byte is
//!   the grey level (`Gray8`).
//! * 8 bpp × 3 planes — 24-bit truecolour, plane order R, G, B
//!   (`Rgb24`).
//!
//! The typed paletted accessors (`parse_pcx_indexed_*`) are the depth
//! layer below the contract: they return the raw indices plus a
//! palette-source tag per geometry. The registry adapter lives in
//! `crate::registry`.

use crate::error::{PcxError as Error, Result};
use crate::image::{
    ImageInfo, Palette, Pcx1bpp3PlanesPaletteSource, Pcx1bpp4PlanesPaletteSource, Pcx2bppCgaCpi,
    Pcx2bppCgaPaletteSource, Pcx4bppPaletteSource, PcxImage, PcxIndexed1x2Cga, PcxIndexed1x3,
    PcxIndexed1x4, PcxIndexed2x1Cga, PcxIndexed2x1CgaCpi, PcxIndexed4, PcxIndexed4x4, PcxIndexed8,
    PcxLayout, PcxPaletteSource, PcxPixelFormat, Plane,
};
use crate::options::DecodeOptions;
use crate::rle;
use crate::types::*;

// ---------------------------------------------------------------------------
// Header validation (shared by probe / info / header / decode)
// ---------------------------------------------------------------------------

/// `true` when `bytes` starts with a plausible PCX header: manufacturer
/// `0x0A`, a known version byte, encoding `1`, a spec depth and plane
/// count, and a non-inverted window. Also `true` for a DCX bundle (the
/// 4-byte magic), since [`crate::decode_all`] opens those. Allocation-
/// free; `false` on short input.
pub(crate) fn probe_bytes(bytes: &[u8]) -> bool {
    if crate::dcx::is_dcx(bytes) {
        return true;
    }
    let Some(h) = read_header(bytes) else {
        return false;
    };
    h.manufacturer == PCX_MANUFACTURER
        && matches!(h.version, 0 | 2 | 3 | 4 | 5)
        && h.encoding == PCX_ENCODING_RLE
        && matches!(h.bits_per_pixel, 1 | 2 | 4 | 8)
        && matches!(h.n_planes, 1..=4)
        && h.x_max >= h.x_min
        && h.y_max >= h.y_min
}

/// A header that passed every check [`crate::decode`] applies before
/// touching pixel data, plus the resolved VGA tail and geometry.
pub(crate) struct Validated<'a> {
    pub header: PcxHeader,
    /// The 768 palette bytes of the VGA tail block, when the file is
    /// `8 bpp × 1 plane` and ends in one.
    pub vga_palette: Option<&'a [u8]>,
    /// The spec geometry, or `None` for the `4 bpp × 4 planes`
    /// composite slot (structurally valid, no palette geometry — only
    /// the typed accessor reads it).
    pub layout: Option<PcxLayout>,
}

/// Validate the 128-byte header: manufacturer byte, version table,
/// encoding byte, dimension underflow, zero dimensions, zero planes /
/// `bytes_per_line`, `bytes_per_line < min_bpl` mis-framing, and a
/// geometry the spec defines. In `strict` mode the spec's *should*
/// rules are enforced too: an even `bytes_per_line` and a zero
/// reserved byte.
pub(crate) fn validate_header(input: &[u8], strict: bool) -> Result<Validated<'_>> {
    let header = read_header(input).ok_or_else(|| Error::invalid("PCX: header truncated"))?;
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
    if strict {
        if header.bytes_per_line % 2 != 0 {
            return Err(Error::invalid(format!(
                "PCX (strict): bytes_per_line={} is odd; spec §3 says it MUST be even",
                header.bytes_per_line
            )));
        }
        if header.reserved != 0 {
            return Err(Error::invalid(format!(
                "PCX (strict): reserved byte 64 is 0x{:02X}, should be 0",
                header.reserved
            )));
        }
    }
    // The appended 768-byte VGA palette (marker `0x0C` 769 bytes from EOF)
    // belongs to the 256-colour Extended VGA mode *only* — spec §"VGA
    // 256-color palette" introduces it as the carrier for "more than 16
    // colors", and spec §"24-bit .PCX files" states 24-bit (8 bpp ×
    // 3-plane) images "do **not** contain a palette". Every sub-256-colour
    // mode (mono / CGA / EGA / 16-colour) carries its palette in the header
    // `Colormap` field, never as a tail block. So the tail-palette probe is
    // confined to `(8 bpp, 1 plane)`. The cross-reference summary
    // (`docs/image/pcx/pcx-egff-fileformat-info.html`) flags exactly why
    // this matters: "24-bit PCX images are always marked as v3.0, yet never
    // have an attached color palette" and the `0x0C` marker byte "might be
    // 0Ch by coincidence" — a 24-bit (or CGA/EGA) stream whose RLE data
    // happens to end with that pattern would otherwise have 769 bytes of
    // real pixel data mis-claimed as a palette and stripped from the RLE
    // region, corrupting the decode.
    let vga_palette = if (header.bits_per_pixel, header.n_planes) == (8, 1) {
        find_vga_palette(input)
    } else {
        None
    };
    let layout = header.layout(vga_palette.is_some());
    if layout.is_none() && (header.bits_per_pixel, header.n_planes) != (4, 4) {
        return Err(unsupported_geometry(&header));
    }
    Ok(Validated {
        header,
        vga_palette,
        layout,
    })
}

fn unsupported_geometry(header: &PcxHeader) -> Error {
    Error::unsupported(format!(
        "PCX: (bits_per_pixel={}, n_planes={}) combination not supported",
        header.bits_per_pixel, header.n_planes
    ))
}

/// [`crate::info`]: the header-only description of a PCX file, or of a
/// DCX bundle's first page (with `frames` = the page count).
pub(crate) fn header_info(input: &[u8]) -> Result<ImageInfo> {
    if crate::dcx::is_dcx(input) {
        let pages = crate::dcx::page_slices(input)?;
        let first = pages
            .first()
            .ok_or_else(|| Error::invalid("DCX: bundle has no pages"))?;
        let mut info = header_info(first)?;
        info.frames = u32::try_from(pages.len()).unwrap_or(u32::MAX);
        return Ok(info);
    }
    let v = validate_header(input, false)?;
    let h = &v.header;
    let layout = v.layout.ok_or_else(|| unsupported_geometry(h))?;
    let (dpi, window_origin, screen_size) = surface_header_metadata(h);
    let mut info = ImageInfo::new(h.width(), h.height(), layout);
    info.version = h.version;
    info.bits_per_pixel = h.bits_per_pixel;
    info.n_planes = h.n_planes;
    info.bytes_per_line = h.bytes_per_line;
    info.palette_info = h.palette_info;
    info.has_vga_palette = v.vga_palette.is_some();
    info.dpi = dpi;
    info.window_origin = window_origin;
    info.screen_size = screen_size;
    Ok(info)
}

/// The three optional authoring-metadata pairs surfaced on a decoded
/// [`PcxImage`]: `(dpi, window_origin, screen_size)`, each `Some((h, v))`
/// or `None` per the spec §3 sentinel rules in [`surface_header_metadata`].
type HeaderMetadata = (Option<(u16, u16)>, Option<(u16, u16)>, Option<(u16, u16)>);

/// Resolve the three optional authoring-metadata pairs PCX records in its
/// header — printer/scanner DPI (`h_dpi` / `v_dpi`), the source crop
/// origin (`x_min` / `y_min`), and the PB IV authoring screen size
/// (`h_screen_size` / `v_screen_size`) — applying the spec §3 "0 = unset"
/// sentinel uniformly.
///
/// * DPI and screen size require BOTH components non-zero: per spec §3 a
///   0 in either means "unset" (many drawing programs leave the field at
///   zero rather than 72×72), so an asymmetric `(0, 300)` would not be a
///   sensible reading.
/// * Window origin surfaces when EITHER `x_min` / `y_min` is non-zero —
///   PCX 3.0+ allows a non-zero origin to record the source crop region
///   (spec §3 derives visible width/height as `x_max - x_min + 1` /
///   `y_max - y_min + 1`); the common screen-authored `(0, 0)` collapses
///   to `None`.
pub(crate) fn surface_header_metadata(header: &PcxHeader) -> HeaderMetadata {
    let dpi = if header.h_dpi != 0 && header.v_dpi != 0 {
        Some((header.h_dpi, header.v_dpi))
    } else {
        None
    };
    let window_origin = if header.x_min != 0 || header.y_min != 0 {
        Some((header.x_min, header.y_min))
    } else {
        None
    };
    let screen_size = if header.h_screen_size != 0 && header.v_screen_size != 0 {
        Some((header.h_screen_size, header.v_screen_size))
    } else {
        None
    };
    (dpi, window_origin, screen_size)
}

// ---------------------------------------------------------------------------
// RLE expansion
// ---------------------------------------------------------------------------

/// Return shape of [`decode_planar_scanlines`]: the validated header
/// (+ VGA tail + layout) and the fully-RLE-decoded planar pixel buffer
/// (`n_planes × bytes_per_line × height` bytes).
type PlanarDecode<'a> = (Validated<'a>, Vec<u8>);

/// Shared header-validation + RLE-decode step that produces the planar
/// scanline buffer (`n_planes × bytes_per_line × height` bytes).
/// Centralising the validation keeps [`decode_image`] and the typed
/// accessors in lockstep on every clean-room guard: manufacturer byte,
/// version table, encoding byte, dimension underflow, `bytes_per_line <
/// min_bpl` mis-framing, `scanline × height` overflow, the
/// [`DecodeOptions`] limits (checked before any allocation) and the
/// decompression-bomb cap.
fn decode_planar_scanlines<'a>(input: &'a [u8], opts: &DecodeOptions) -> Result<PlanarDecode<'a>> {
    let v = validate_header(input, opts.strict)?;
    let header = &v.header;
    let width = header.width();
    let height = header.height();

    let scanline = header.scanline_bytes();
    let total_planar = scanline
        .checked_mul(height as usize)
        .ok_or_else(|| Error::invalid("PCX: scanline × height overflows usize"))?;
    // Limits first: the planar buffer and the native output plane are
    // the two allocations a decode makes; each must fit `max_bytes`.
    let native_bpp = v
        .layout
        .map(|l| l.pixel_format().bytes_per_pixel())
        .unwrap_or(2);
    let native_bytes = u64::from(width) * u64::from(height) * native_bpp as u64;
    opts.check(width, height, (total_planar as u64).max(native_bytes))?;

    let cursor = PCX_HEADER_SIZE;
    let rle_end = if v.vga_palette.is_some() {
        input.len() - PCX_VGA_PALETTE_BLOCK_BYTES
    } else {
        input.len()
    };
    if rle_end < cursor {
        return Err(Error::invalid("PCX: pixel data section is empty"));
    }
    let available = rle_end - cursor;
    let max_plausible_output = available.saturating_mul(63);
    if total_planar > max_plausible_output {
        return Err(Error::invalid(format!(
            "PCX: claimed pixel data ({total_planar} bytes) exceeds what {available} RLE bytes can decode"
        )));
    }
    let mut pixels_planar = Vec::with_capacity(total_planar);
    let stream = &input[cursor..rle_end];
    if opts.strict {
        // Spec §"Decoding .PCX Files": "there should always be a
        // decoding break at the end of each scan line" — strict mode
        // holds the writer to it by decoding one scanline at a time and
        // rejecting a run packet that would spill into the next row.
        let mut pos = 0usize;
        for y in 0..height as usize {
            let consumed = rle::decode(&stream[pos..], &mut pixels_planar, scanline)
                .map_err(|e| Error::invalid(format!("PCX (strict): scanline {y}: {e}")))?;
            pos += consumed;
        }
    } else {
        // Decode the whole image as a single continuous RLE stream of
        // `total_planar = scanline × height` bytes, exactly as the manual's
        // own decode fragment does (`pcx-pcgpe.txt` lines 316-326: the
        // `for (l = 0; l < lsize; )` loop runs over `BytesPerLine * Nplanes *
        // (1 + Ymax - Ymin)` with no per-scanline RLE reset). The prose
        // "there should always be a decoding break at the end of each scan
        // line" (spec §"Decoding .PCX Files") is an *encoder* convention —
        // a "should", not a decode-time requirement — and the manual's C
        // reader honours it by consuming the stream straight through. A
        // file written by an encoder that lets a run packet straddle the
        // row boundary therefore decodes identically here, instead of being
        // rejected mid-row. The flat `total_planar` buffer is re-split into
        // per-row `chunks_exact(bytes_per_line)` slices by the plane-unpack
        // paths downstream, so a continuous decode yields a byte-identical
        // buffer to a per-scanline loop for any spec-conformant file.
        rle::decode(stream, &mut pixels_planar, total_planar)?;
    }
    Ok((v, pixels_planar))
}

/// The lenient-default planar decode the typed accessors use.
fn planar_default(input: &[u8]) -> Result<PlanarDecode<'_>> {
    decode_planar_scanlines(input, &DecodeOptions::default())
}

/// Benchmark probe: run only the header-validation + RLE-decode phase
/// of [`decode_image`] and return the length of the resulting planar
/// scanline buffer (`n_planes × bytes_per_line × height` bytes).
///
/// This exists so the Criterion suite can time the RLE-unpack phase in
/// isolation from the per-plane assembly phase, making the BENCHMARKS.md
/// hotspot ranking a measured split rather than an inference. It runs
/// the exact same `decode_planar_scanlines` the production decoder
/// calls — no parallel code path — so the timing is faithful. Not part
/// of the stable API; hidden from docs and intended for benches only.
#[doc(hidden)]
pub fn __bench_decode_planar_len(input: &[u8]) -> Result<usize> {
    let (_v, pixels_planar) = planar_default(input)?;
    Ok(pixels_planar.len())
}

// ---------------------------------------------------------------------------
// Native decode
// ---------------------------------------------------------------------------

/// [`crate::decode_with`]: the native-layout image.
pub(crate) fn decode_image(input: &[u8], opts: &DecodeOptions) -> Result<PcxImage> {
    if crate::dcx::is_dcx(input) {
        let pages = crate::dcx::page_slices(input)?;
        let first = pages
            .first()
            .ok_or_else(|| Error::invalid("DCX: bundle has no pages"))?;
        return decode_page(first, opts);
    }
    decode_page(input, opts)
}

/// [`decode_image`] for one stand-alone PCX stream (a DCX bundle is
/// rejected at the manufacturer byte).
pub(crate) fn decode_page(input: &[u8], opts: &DecodeOptions) -> Result<PcxImage> {
    let (v, planar) = decode_planar_scanlines(input, opts)?;
    let header = &v.header;
    let layout = v.layout.ok_or_else(|| unsupported_geometry(header))?;
    let width = header.width();
    let height = header.height();
    let (format, data, palette): (PcxPixelFormat, Vec<u8>, Option<Palette>) = match layout {
        PcxLayout::Mono1 => (
            PcxPixelFormat::Pal8,
            indices_1bpp_1plane(header, &planar),
            Some(Palette::from_rgb_triples(&mono_colormap(header))),
        ),
        PcxLayout::Cga1x2 => (
            PcxPixelFormat::Pal8,
            indices_1bpp_2planes(header, &planar),
            Some(Palette::from_rgb_triples(&cga_palette_from_header(
                &header.ega_palette,
            ))),
        ),
        PcxLayout::Cga2x1 => (
            PcxPixelFormat::Pal8,
            indices_2bpp_1plane(header, &planar),
            Some(Palette::from_rgb_triples(&cga_palette_from_header(
                &header.ega_palette,
            ))),
        ),
        PcxLayout::EgaRgb1x3 => (
            PcxPixelFormat::Pal8,
            indices_1bpp_3planes(header, &planar),
            Some(Palette::from_rgb_triples(&RGB_PRIMARIES_PALETTE)),
        ),
        PcxLayout::Indexed1x4 => (
            PcxPixelFormat::Pal8,
            indices_1bpp_4planes(header, &planar),
            Some(Palette::from_rgb_triples(&ega_palette_or_default(
                &header.ega_palette,
            ))),
        ),
        PcxLayout::Indexed4 => (
            PcxPixelFormat::Pal8,
            indices_4bpp_1plane(header, &planar),
            Some(Palette::from_rgb_triples(&ega_palette_or_default(
                &header.ega_palette,
            ))),
        ),
        PcxLayout::Indexed8 => {
            let tail = v
                .vga_palette
                .expect("Indexed8 layout implies a VGA tail block");
            (
                PcxPixelFormat::Pal8,
                bytes_8bpp_1plane(header, &planar),
                Some(Palette::from_rgb(tail)),
            )
        }
        // `palette_info == 2` (spec §3) forces the grayscale
        // interpretation regardless of whether a tail palette is
        // present. Some scanner / FAX-era tools emit a grayscale PCX
        // with `palette_info=2` and no VGA tail; some emit the flag AND
        // a redundant tail palette. We honour the flag in both cases,
        // and a tail-less 8 bpp file with no flag is read the same way
        // (the pixel byte is the grey level).
        PcxLayout::Gray8 => (
            PcxPixelFormat::Gray8,
            bytes_8bpp_1plane(header, &planar),
            None,
        ),
        PcxLayout::Rgb24 => (
            PcxPixelFormat::Rgb24,
            rgb_8bpp_3planes(header, &planar),
            None,
        ),
    };
    let (dpi, window_origin, screen_size) = surface_header_metadata(header);
    let stride = width as usize * format.bytes_per_pixel();
    let mut img = PcxImage::unchecked(width, height, format, vec![Plane::new(stride, data)]);
    img.palette = palette;
    img.dpi = dpi;
    img.window_origin = window_origin;
    img.screen_size = screen_size;
    img.layout = Some(layout);
    Ok(img)
}

/// The pre-contract flatten: one PCX stream decoded with the default
/// options then [`PcxImage::into_legacy_layout`] (packed `Rgba`). A
/// DCX bundle is rejected, as it always was here.
pub(crate) fn decode_legacy(input: &[u8]) -> Result<PcxImage> {
    Ok(decode_page(input, &DecodeOptions::default())?.into_legacy_layout())
}

/// Decode a complete PCX file into a packed-`Rgba` [`PcxImage`] — the
/// pre-contract flatten. [`crate::decode`] returns the native layout
/// instead (and also opens a DCX bundle's first page);
/// `decode(..)?.to_rgba8()` is the same pixel buffer.
#[deprecated(note = "use oxideav_pcx::decode (IMAGE_CRATE_API)")]
pub fn parse_pcx(input: &[u8]) -> Result<PcxImage> {
    decode_legacy(input)
}

/// Flatten a 4-colour CGA PCX (either `(2, 1)` packed or `(1, 2)`
/// planar) to packed `Rgba` through the full C / P / I decomposition of
/// header byte 19. [`crate::decode`] resolves CGA palettes the same way
/// for every caller, so this is now [`parse_pcx`] restricted to the two
/// CGA geometries (any other geometry is [`Error::Unsupported`], as
/// before).
#[deprecated(note = "use oxideav_pcx::decode (IMAGE_CRATE_API)")]
pub fn parse_pcx_cga_cpi(input: &[u8]) -> Result<PcxImage> {
    let v = validate_header(input, false)?;
    if !matches!(v.layout, Some(PcxLayout::Cga2x1 | PcxLayout::Cga1x2)) {
        return Err(Error::unsupported(format!(
            "PCX: parse_pcx_cga_cpi expects a CGA layout ((2, 1) packed or (1, 2) planar), found {} bpp × {} planes",
            v.header.bits_per_pixel, v.header.n_planes
        )));
    }
    decode_legacy(input)
}

// ---------------------------------------------------------------------------
// Per-geometry unpack into indices / samples (padding stripped)
// ---------------------------------------------------------------------------

/// Monochrome colormap rule (EGFF canonical mode matrix: `1 bpp × 1
/// plane` is the 2-colour paletted case of the header colormap): a
/// non-zero colormap's first two triples ARE the bit-0 / bit-1 colours
/// (a foreign writer may legitimately store, say, white-on-blue); a
/// zero-filled colormap — what PCX 3.0+ writers commonly emit — falls
/// back to the classic convention bit 1 = white, bit 0 = black (spec
/// §4.1 monochrome example, pinned by the docs' Issue #227 erratum).
pub(crate) fn mono_colormap(header: &PcxHeader) -> [[u8; 3]; 2] {
    if header.ega_palette.iter().any(|&b| b != 0) {
        let p = &header.ega_palette;
        [[p[0], p[1], p[2]], [p[3], p[4], p[5]]]
    } else {
        [[0x00; 3], [0xFF; 3]]
    }
}

fn indices_1bpp_1plane(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let mut out = Vec::with_capacity(w * h);
    for row in planar.chunks_exact(bpl) {
        for x in 0..w {
            out.push((row[x >> 3] >> (7 - (x & 7))) & 1);
        }
    }
    out
}

fn indices_1bpp_2planes(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    // Plane 0 then plane 1 within the row; the bit at the same
    // x-position in each plane stacks into the 2-bit index
    // (`p0 | p1 << 1`), matching the 4-plane EGA ordering (plane k
    // contributes bit k).
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let mut out = Vec::with_capacity(w * h);
    for row in planar.chunks_exact(bpl * 2) {
        let (p0, p1) = row.split_at(bpl);
        for x in 0..w {
            let byte = x >> 3;
            let shift = 7 - (x & 7);
            out.push(((p0[byte] >> shift) & 1) | (((p1[byte] >> shift) & 1) << 1));
        }
    }
    out
}

fn indices_1bpp_3planes(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    // 8-colour EGA RGB: one bit-plane per primary, plane order R, G, B
    // (spec §4 bit-plane example). Index `r | g << 1 | b << 2` into
    // [`RGB_PRIMARIES_PALETTE`].
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let mut out = Vec::with_capacity(w * h);
    for row in planar.chunks_exact(bpl * 3) {
        let (rp, rest) = row.split_at(bpl);
        let (gp, bp) = rest.split_at(bpl);
        for x in 0..w {
            let byte = x >> 3;
            let shift = 7 - (x & 7);
            out.push(
                ((rp[byte] >> shift) & 1)
                    | (((gp[byte] >> shift) & 1) << 1)
                    | (((bp[byte] >> shift) & 1) << 2),
            );
        }
    }
    out
}

fn indices_1bpp_4planes(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    // Plane order is bit 0 → bit 3 (B, G, R, I in classical EGA
    // hardware terms). Each plane contributes one bit of the 4-bit
    // palette index.
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let mut out = Vec::with_capacity(w * h);
    for row in planar.chunks_exact(bpl * 4) {
        let (p0, rest) = row.split_at(bpl);
        let (p1, rest) = rest.split_at(bpl);
        let (p2, p3) = rest.split_at(bpl);
        for x in 0..w {
            let byte = x >> 3;
            let shift = 7 - (x & 7);
            out.push(
                ((p0[byte] >> shift) & 1)
                    | (((p1[byte] >> shift) & 1) << 1)
                    | (((p2[byte] >> shift) & 1) << 2)
                    | (((p3[byte] >> shift) & 1) << 3),
            );
        }
    }
    out
}

fn indices_2bpp_1plane(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    // 2 bpp packed: 4 pixels per byte, MSB first (top two bits = pixel 0).
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let mut out = Vec::with_capacity(w * h);
    for row in planar.chunks_exact(bpl) {
        for x in 0..w {
            let shift = 6 - 2 * (x & 3);
            out.push((row[x >> 2] >> shift) & 0b11);
        }
    }
    out
}

fn indices_4bpp_1plane(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    // 4 bpp packed: 2 pixels per byte, high nibble first.
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let mut out = Vec::with_capacity(w * h);
    for row in planar.chunks_exact(bpl) {
        for x in 0..w {
            let byte = row[x >> 1];
            out.push(if x & 1 == 0 {
                (byte >> 4) & 0x0F
            } else {
                byte & 0x0F
            });
        }
    }
    out
}

fn bytes_8bpp_1plane(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    // Strip per-row padding: spec §1 rounds `bytes_per_line` up to an
    // even number, so the on-disk scanline can carry one trailing byte
    // beyond the visible width.
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let mut out = Vec::with_capacity(w * h);
    for row in planar.chunks_exact(bpl) {
        out.extend_from_slice(&row[..w]);
    }
    out
}

fn rgb_8bpp_3planes(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let mut out = vec![0u8; w * h * 3];
    let src_rows = planar.chunks_exact(bpl * 3);
    let dst_rows = out.chunks_exact_mut(w * 3);
    for (row, dst_row) in src_rows.zip(dst_rows) {
        // Pre-slice the R/G/B plane sub-rows once and bound each plane
        // slice to exactly `w` bytes so the zip-of-three iterators
        // advance with no bounds checks against anything but the
        // destination chunks.
        let (rp, rest) = row.split_at(bpl);
        let (gp, bp) = rest.split_at(bpl);
        let r_iter = rp[..w].iter();
        let g_iter = gp[..w].iter();
        let b_iter = bp[..w].iter();
        for (((&r, &g), &b), dst) in r_iter
            .zip(g_iter)
            .zip(b_iter)
            .zip(dst_row.chunks_exact_mut(3))
        {
            dst[0] = r;
            dst[1] = g;
            dst[2] = b;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Typed paletted accessors (depth layer)
// ---------------------------------------------------------------------------

fn expect_geometry(v: &Validated<'_>, want: (u8, u8), name: &str) -> Result<()> {
    let got = (v.header.bits_per_pixel, v.header.n_planes);
    if got != want {
        return Err(Error::unsupported(format!(
            "PCX: {name} expects {} bpp × {} planes, found {} bpp × {} planes",
            want.0, want.1, got.0, got.1
        )));
    }
    Ok(())
}

fn ega16_from_header(raw: &[u8; 48]) -> [[u8; 3]; 16] {
    let mut out = [[0u8; 3]; 16];
    for (i, e) in out.iter_mut().enumerate() {
        *e = [raw[i * 3], raw[i * 3 + 1], raw[i * 3 + 2]];
    }
    out
}

/// Decode an 8 bpp × 1 plane PCX into a typed paletted view (indices +
/// resolved 256-entry palette).
///
/// The returned [`PcxIndexed8`] surfaces the `width × height` index
/// buffer (one byte per pixel, top-down, padding stripped) alongside the
/// resolved 256-entry RGB palette and a [`PcxPaletteSource`] tag that
/// records which spec §3 branch produced it: `palette_info = 2` forces
/// the grayscale ramp (even if a VGA tail block is also present),
/// otherwise the tail block is honoured, otherwise the deterministic
/// `0..=255` ramp is the fallback. Where [`crate::decode`] returns
/// `Gray8` for the two ramp cases, this view keeps the index-plus-ramp
/// shape.
///
/// Rejects any (depth, planes) combination other than `(8, 1)` with
/// [`Error::Unsupported`].
pub fn parse_pcx_indexed_8bpp(input: &[u8]) -> Result<PcxIndexed8> {
    let (v, scanlines) = planar_default(input)?;
    expect_geometry(&v, (8, 1), "parse_pcx_indexed_8bpp")?;
    let header = &v.header;
    let indices = bytes_8bpp_1plane(header, &scanlines);
    let (palette, palette_source) = if header.palette_info == 2 {
        (grayscale_palette_256(), PcxPaletteSource::GrayscaleFlag)
    } else if let Some(p) = v.vga_palette {
        let mut out = [[0u8; 3]; 256];
        for (i, e) in out.iter_mut().enumerate() {
            *e = [p[i * 3], p[i * 3 + 1], p[i * 3 + 2]];
        }
        (out, PcxPaletteSource::VgaTail)
    } else {
        (grayscale_palette_256(), PcxPaletteSource::GrayscaleFallback)
    };
    Ok(PcxIndexed8 {
        width: header.width(),
        height: header.height(),
        indices,
        palette,
        palette_source,
    })
}

/// Decode a 4 bpp × 1 plane PCX into a typed paletted view (16-colour
/// nibble indices + resolved 16-entry palette).
///
/// The returned [`PcxIndexed4`] surfaces one byte per pixel (low nibble
/// = palette index `0..=15`, top-down, padding stripped) alongside the
/// resolved 16-entry RGB palette and a [`Pcx4bppPaletteSource`] tag
/// recording whether the header carried a non-zero `ega_palette` field
/// or the spec table §3.1 hardware default was substituted.
///
/// Rejects any (depth, planes) combination other than `(4, 1)` with
/// [`Error::Unsupported`].
pub fn parse_pcx_indexed_4bpp(input: &[u8]) -> Result<PcxIndexed4> {
    let (v, scanlines) = planar_default(input)?;
    expect_geometry(&v, (4, 1), "parse_pcx_indexed_4bpp")?;
    let header = &v.header;
    let indices = indices_4bpp_1plane(header, &scanlines);
    let (palette, palette_source) = if header.ega_palette.iter().any(|&b| b != 0) {
        (
            ega16_from_header(&header.ega_palette),
            Pcx4bppPaletteSource::Ega16InHeader,
        )
    } else {
        (EGA_DEFAULT_PALETTE, Pcx4bppPaletteSource::Ega16Default)
    };
    Ok(PcxIndexed4 {
        width: header.width(),
        height: header.height(),
        indices,
        palette,
        palette_source,
    })
}

/// Snap a stored 0..=255 RGB component to the EGA hardware level it
/// resolves to per the spec §"EGA/VGA 16-color palette" quantisation
/// table.
///
/// The rev-5 manual notes that "on an IBM EGA there are only 4 levels of
/// RGB for each color. Since 256/4 = 64, the following is a list of the
/// settings and levels":
///
/// | Setting   | Level |
/// | --------- | ----: |
/// | 0–63      | 0     |
/// | 64–127    | 1     |
/// | 128–192   | 2     |
/// | 193–254   | 3     |
///
/// A PCX `Colormap` triple stores 0..=255 component values, but the EGA
/// display only has the four levels above, so the value the hardware
/// actually shows is one of four buckets. The manual's table stops at
/// 254; value `255` falls in the same top bucket as `193–254` (it is
/// above the level-3 threshold), so this function maps `193..=255` to
/// level 3.
///
/// Returns the **level** (`0..=3`). For the level → output-intensity
/// mapping the EGA DAC uses, see [`ega_quantize_component`].
#[must_use]
pub fn ega_quantize_level(value: u8) -> u8 {
    match value {
        0..=63 => 0,
        64..=127 => 1,
        128..=192 => 2,
        // The manual's table ends at 254; 255 is above the level-3
        // threshold and lands in the same top bucket.
        193..=255 => 3,
    }
}

/// EGA DAC output intensities for the four levels in
/// [`ega_quantize_level`].
///
/// The spec §"EGA/VGA 16-color palette" table defines the four input
/// buckets (the *levels*) but not the analogue intensity each level
/// drives. The standard EGA hardware palette the rev-5 manual lists as
/// the default 16-colour set (the one this crate exposes as
/// [`Pcx4bppPaletteSource::Ega16Default`]) is built entirely from the
/// four evenly-spaced byte values `0x00`, `0x55`, `0xAA`, `0xFF` — those
/// are exactly the analogue levels the EGA DAC emits for levels
/// `0`, `1`, `2`, `3`. So the level → component map is that even ramp.
const EGA_LEVEL_OUTPUT: [u8; 4] = [0x00, 0x55, 0xAA, 0xFF];

/// Snap a stored 0..=255 RGB component to the byte value an IBM EGA
/// display actually shows for it.
///
/// This is [`ega_quantize_level`] composed with the EGA DAC output ramp
/// (`0x00 / 0x55 / 0xAA / 0xFF`). A scanner or editor may store an
/// arbitrary 0..=255 component in the header `Colormap`, but on real EGA
/// hardware only the four levels are displayable, so the on-screen colour
/// is the quantised one. Round-tripping a stored value already on the
/// ramp is idempotent.
#[must_use]
pub fn ega_quantize_component(value: u8) -> u8 {
    EGA_LEVEL_OUTPUT[ega_quantize_level(value) as usize]
}

/// Snap a 16-entry RGB palette to the colours an IBM EGA display shows,
/// per spec §"EGA/VGA 16-color palette" — every component routed through
/// [`ega_quantize_component`].
#[must_use]
pub fn ega_quantize_palette(palette: &[[u8; 3]; 16]) -> [[u8; 3]; 16] {
    let mut out = [[0u8; 3]; 16];
    for (dst, src) in out.iter_mut().zip(palette.iter()) {
        *dst = [
            ega_quantize_component(src[0]),
            ega_quantize_component(src[1]),
            ega_quantize_component(src[2]),
        ];
    }
    out
}

/// Decode a 4 bpp × 1 plane PCX into a typed paletted view whose palette
/// is snapped to the colours an IBM EGA display actually shows.
///
/// [`parse_pcx_indexed_4bpp`] surfaces the raw header `ega_palette`
/// triples verbatim (0..=255 per component). The rev-5 manual's
/// §"EGA/VGA 16-color palette" section notes that an IBM EGA can only
/// display four levels per channel, so a file authored on (or for) EGA
/// hardware whose header palette stores arbitrary 0..=255 values is shown
/// with each component snapped to one of `0x00 / 0x55 / 0xAA / 0xFF`.
/// This accessor returns the EGA-hardware-accurate palette by routing
/// every component of the resolved palette through
/// [`ega_quantize_component`]; the indices and the
/// [`Pcx4bppPaletteSource`] tag are identical to
/// [`parse_pcx_indexed_4bpp`].
///
/// When the header field is all-zeros the spec table §3.1 default is
/// substituted first (exactly as [`parse_pcx_indexed_4bpp`] does); that
/// default is already on the EGA ramp, so quantising it is a no-op — the
/// difference only shows for the `Ega16InHeader` branch carrying
/// off-ramp scanner / editor values.
///
/// Rejects any (depth, planes) combination other than `(4, 1)` with
/// [`Error::unsupported`], the same scope as [`parse_pcx_indexed_4bpp`].
pub fn parse_pcx_indexed_4bpp_ega_hw(input: &[u8]) -> Result<PcxIndexed4> {
    let mut view = parse_pcx_indexed_4bpp(input)?;
    view.palette = ega_quantize_palette(&view.palette);
    Ok(view)
}

/// Decode a 1 bpp × 4 planes PCX into a typed paletted view (16-colour
/// indices + resolved 16-entry palette).
///
/// Spec §4.1's 16-colour EGA bit-plane mode: each scanline carries four
/// 1-bit planes laid out one after another; the four bits at the same
/// x-position stack into the 4-bit palette index (`plane0 | plane1 << 1
/// | plane2 << 2 | plane3 << 3`). The returned [`PcxIndexed1x4`]
/// surfaces one byte per pixel (low nibble, top-down, padding stripped)
/// alongside the resolved 16-entry RGB palette and a
/// [`Pcx1bpp4PlanesPaletteSource`] tag. The nibble values share the
/// [`PcxIndexed4`] convention.
///
/// Rejects any (depth, planes) combination other than `(1, 4)` with
/// [`Error::Unsupported`].
pub fn parse_pcx_indexed_1bpp_4planes(input: &[u8]) -> Result<PcxIndexed1x4> {
    let (v, scanlines) = planar_default(input)?;
    expect_geometry(&v, (1, 4), "parse_pcx_indexed_1bpp_4planes")?;
    let header = &v.header;
    let indices = indices_1bpp_4planes(header, &scanlines);
    let (palette, palette_source) = if header.ega_palette.iter().any(|&b| b != 0) {
        (
            ega16_from_header(&header.ega_palette),
            Pcx1bpp4PlanesPaletteSource::Ega16InHeader,
        )
    } else {
        (
            EGA_DEFAULT_PALETTE,
            Pcx1bpp4PlanesPaletteSource::Ega16Default,
        )
    };
    Ok(PcxIndexed1x4 {
        width: header.width(),
        height: header.height(),
        indices,
        palette,
        palette_source,
    })
}

/// Decode a 2 bpp × 1 plane CGA PCX into a typed paletted view
/// (4-colour indices + resolved 4-entry RGB palette + the CGA palette
/// family the decoder landed on).
///
/// Spec §4.1 describes the 4-colour CGA mode as a single plane of 2 bpp
/// packed-bits data (4 pixels/byte, the top two bits = pixel 0). The
/// 4-entry palette is selected per the manual's "CGA Color Map": header
/// byte 16's high nibble = the EGA index of palette entry 0 (the
/// "background"), header byte 19's upper three bits = C / P / I. The
/// returned [`PcxIndexed2x1Cga`] carries the indices (low two bits,
/// top-down, padding stripped), the palette, the `background_index`
/// and a [`Pcx2bppCgaPaletteSource`] tag whose
/// [`palette_selector`](Pcx2bppCgaPaletteSource::palette_selector)
/// reconstructs byte 19 for a re-encode.
///
/// Rejects any (depth, planes) combination other than `(2, 1)` with
/// [`Error::Unsupported`].
pub fn parse_pcx_indexed_2bpp_cga(input: &[u8]) -> Result<PcxIndexed2x1Cga> {
    let (v, scanlines) = planar_default(input)?;
    expect_geometry(&v, (2, 1), "parse_pcx_indexed_2bpp_cga")?;
    let header = &v.header;
    Ok(PcxIndexed2x1Cga {
        width: header.width(),
        height: header.height(),
        indices: indices_2bpp_1plane(header, &scanlines),
        palette: cga_palette_from_header(&header.ega_palette),
        background_index: (header.ega_palette[0] >> 4) & 0x0F,
        palette_source: cga_legacy_source(header.ega_palette[3]),
    })
}

/// Decode a 1 bpp × 2 planes CGA PCX into a typed paletted view
/// (indices + resolved 4-entry palette).
///
/// The plane-oriented sibling of [`parse_pcx_indexed_2bpp_cga`]: the
/// EGFF canonical PCX mode matrix lists 4-colour CGA as `BitsPerPixel =
/// 1, NumBitPlanes = 2`. Each on-disk scanline carries plane 0 then
/// plane 1; the bit at the same x-position in each plane stacks into the
/// 2-bit palette index (`p0 | p1 << 1`). Palette resolution is identical
/// to the packed accessor, so the returned [`PcxIndexed1x2Cga`] reuses
/// the [`Pcx2bppCgaPaletteSource`] tag and surfaces the same
/// `background_index`.
///
/// Rejects any (depth, planes) combination other than `(1, 2)` with
/// [`Error::Unsupported`].
pub fn parse_pcx_indexed_1bpp_2planes_cga(input: &[u8]) -> Result<PcxIndexed1x2Cga> {
    let (v, scanlines) = planar_default(input)?;
    expect_geometry(&v, (1, 2), "parse_pcx_indexed_1bpp_2planes_cga")?;
    let header = &v.header;
    Ok(PcxIndexed1x2Cga {
        width: header.width(),
        height: header.height(),
        indices: indices_1bpp_2planes(header, &scanlines),
        palette: cga_palette_from_header(&header.ega_palette),
        background_index: (header.ega_palette[0] >> 4) & 0x0F,
        palette_source: cga_legacy_source(header.ega_palette[3]),
    })
}

/// Decode a 2 bpp × 1 plane CGA PCX into a typed paletted view that
/// surfaces all three C / P / I bits of header byte 19 per the verbatim
/// ZSoft manual ("CGA Color Map") as a [`Pcx2bppCgaCpi`] — `C` (bit 7,
/// color burst), `P` (bit 6, palette family), `I` (bit 5, intensity) —
/// alongside the resolved palette (including the four-level
/// composite-grey ramp the monochrome mode produces), the indices and
/// the `background_index`. [`Pcx2bppCgaCpi::to_byte19`] reconstructs
/// the header byte for a re-encode.
///
/// Rejects any (depth, planes) combination other than `(2, 1)` with
/// [`Error::Unsupported`].
pub fn parse_pcx_indexed_2bpp_cga_cpi(input: &[u8]) -> Result<PcxIndexed2x1CgaCpi> {
    let (v, scanlines) = planar_default(input)?;
    expect_geometry(&v, (2, 1), "parse_pcx_indexed_2bpp_cga_cpi")?;
    let header = &v.header;
    let cpi = Pcx2bppCgaCpi::from_byte19(header.ega_palette[3]);
    Ok(PcxIndexed2x1CgaCpi {
        width: header.width(),
        height: header.height(),
        indices: indices_2bpp_1plane(header, &scanlines),
        palette: cga_palette_from_cpi(&header.ega_palette, cpi),
        background_index: (header.ega_palette[0] >> 4) & 0x0F,
        cpi,
    })
}

/// Decode a 1 bpp × 3 planes PCX into a typed paletted view (8-colour
/// EGA RGB indices + the fixed 8-entry on/off-primary palette).
///
/// Spec §4's 8-colour EGA RGB mode: each scanline carries three 1-bit
/// planes (plane order R, G, B); the three bits at the same x-position
/// stack into a 3-bit index (`r | g << 1 | b << 2`) into the fixed
/// primaries. No on-disk palette is consulted, so the
/// [`Pcx1bpp3PlanesPaletteSource`] tag has a single arm.
///
/// Rejects any (depth, planes) combination other than `(1, 3)` with
/// [`Error::Unsupported`].
pub fn parse_pcx_indexed_1bpp_3planes(input: &[u8]) -> Result<PcxIndexed1x3> {
    let (v, scanlines) = planar_default(input)?;
    expect_geometry(&v, (1, 3), "parse_pcx_indexed_1bpp_3planes")?;
    let header = &v.header;
    Ok(PcxIndexed1x3 {
        width: header.width(),
        height: header.height(),
        indices: indices_1bpp_3planes(header, &scanlines),
        palette: RGB_PRIMARIES_PALETTE,
        palette_source: Pcx1bpp3PlanesPaletteSource::FixedPrimaries,
    })
}

/// Decode a 4 bpp × 4 planes PCX into a typed composite-index view
/// (one `u16` per pixel).
///
/// This is the one `(bits_per_pixel, n_planes)` slot the EGFF canonical
/// PCX video-mode matrix does not list as a hardware video mode, but
/// the format is *structurally* reachable: the cross-reference's
/// colour-count formula `MaxNumberOfColors = (1 << (BitsPerPixel *
/// NumBitPlanes))` evaluates to `65536`, and the on-disk scanline
/// layout is the standard plane-oriented form. Each plane holds 4 bits
/// per pixel (2 pixels/byte, high nibble first); the nibble at the same
/// x-position across the four planes stacks into a 16-bit composite
/// index (`p0 | p1 << 4 | p2 << 8 | p3 << 12`).
///
/// No palette is surfaced — the spec defines palette geometries only for
/// the ≤ 256-colour modes — so [`crate::decode`] rejects `(4, 4)` with
/// [`Error::Unsupported`] rather than inventing a colour mapping; this
/// accessor hands the raw composite indices to the caller.
///
/// Rejects any `(depth, planes)` combination other than `(4, 4)` with
/// [`Error::Unsupported`].
pub fn parse_pcx_indexed_4bpp_4planes(input: &[u8]) -> Result<PcxIndexed4x4> {
    let (v, scanlines) = planar_default(input)?;
    expect_geometry(&v, (4, 4), "parse_pcx_indexed_4bpp_4planes")?;
    let header = &v.header;
    let width = header.width() as usize;
    let height = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let mut indices = Vec::with_capacity(width * height);
    for row in scanlines.chunks_exact(bpl * 4) {
        let (p0, rest) = row.split_at(bpl);
        let (p1, rest) = rest.split_at(bpl);
        let (p2, p3) = rest.split_at(bpl);
        for x in 0..width {
            let byte = x >> 1;
            let nib = |p: &[u8]| -> u16 {
                let b = p[byte];
                (if x & 1 == 0 { b >> 4 } else { b & 0x0F }) as u16
            };
            indices.push(nib(p0) | (nib(p1) << 4) | (nib(p2) << 8) | (nib(p3) << 12));
        }
    }
    Ok(PcxIndexed4x4 {
        width: header.width(),
        height: header.height(),
        indices,
    })
}

/// The fixed 8-entry RGB palette of on/off primaries the 1 bpp × 3
/// planes 8-colour EGA RGB mode resolves to (spec §4 bit-plane example).
/// Entry `i` has channel `c` set to `0xFF` iff the matching plane bit is
/// set: `r = i & 1`, `g = i & 2`, `b = i & 4`.
pub(crate) const RGB_PRIMARIES_PALETTE: [[u8; 3]; 8] = [
    [0x00, 0x00, 0x00], // 0: black
    [0xFF, 0x00, 0x00], // 1: red
    [0x00, 0xFF, 0x00], // 2: green
    [0xFF, 0xFF, 0x00], // 3: yellow
    [0x00, 0x00, 0xFF], // 4: blue
    [0xFF, 0x00, 0xFF], // 5: magenta
    [0x00, 0xFF, 0xFF], // 6: cyan
    [0xFF, 0xFF, 0xFF], // 7: white
];

fn grayscale_palette_256() -> [[u8; 3]; 256] {
    let mut out = [[0u8; 3]; 256];
    for (i, e) in out.iter_mut().enumerate() {
        let v = i as u8;
        *e = [v, v, v];
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
pub(crate) const EGA_DEFAULT_PALETTE: [[u8; 3]; 16] = [
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

/// Resolve a CGA 4-colour palette from the in-header bytes per the
/// ZSoft manual's "CGA Color Map" (Header Byte #16 / Header Byte #19).
///
/// PCX repurposes the start of the 48-byte colormap region for CGA mode
/// (see [`crate::encode_pcx_2bpp_cga`] for the matching writer). The
/// manual numbers the two significant bytes by their offset in the
/// 128-byte header, and the colormap itself starts at header offset 16,
/// so within the `ega_palette` field they are bytes 0 and 3 (the EGFF
/// cross-reference's extraction code reads `EgaPalette[0]` /
/// `EgaPalette[3]` accordingly):
/// * colormap byte 0 (header byte 16) — high nibble = background colour
///   (EGA index 0..15 used as palette entry 0).
/// * colormap byte 3 (header byte 19) — upper three bits are `C` (bit
///   7, color burst: 0 = color / 1 = monochrome), `P` (bit 6, palette:
///   0 = yellow family / 1 = white family) and `I` (bit 5, intensity:
///   0 = dim / 1 = bright); the lower five bits are ignored.
///
/// The full C / P / I decomposition is delegated to
/// [`cga_palette_from_cpi`] so this legacy-named resolver and the typed
/// CPI accessors can never drift apart. Until r401 this function read
/// colormap bytes 16 / 19 (header bytes 32 / 35 — an off-by-16 slip
/// from reading the manual's "Header Byte #16/#19" as colormap-relative
/// indices) and decoded only two selector bits with an inverted palette
/// convention; foreign CGA files were therefore always shown with the
/// default palette. Both errors are fixed here.
pub(crate) fn cga_palette_from_header(raw: &[u8; 48]) -> [[u8; 3]; 4] {
    cga_palette_from_cpi(raw, crate::image::Pcx2bppCgaCpi::from_byte19(raw[3]))
}

/// Map the manual's C / P / I decomposition of colormap byte 3 (header
/// byte 19) onto the legacy [`Pcx2bppCgaPaletteSource`] family tag the
/// r-era typed accessors surface. One place, so both CGA typed views
/// derive the tag identically.
fn cga_legacy_source(byte3: u8) -> Pcx2bppCgaPaletteSource {
    let cpi = Pcx2bppCgaCpi::from_byte19(byte3);
    if cpi.monochrome {
        if cpi.intensity_bright {
            Pcx2bppCgaPaletteSource::MonochromeBright
        } else {
            Pcx2bppCgaPaletteSource::MonochromeDim
        }
    } else {
        match (cpi.palette_white, cpi.intensity_bright) {
            (true, true) => Pcx2bppCgaPaletteSource::Palette1HighIntensity,
            (true, false) => Pcx2bppCgaPaletteSource::Palette1LowIntensity,
            (false, true) => Pcx2bppCgaPaletteSource::Palette0HighIntensity,
            (false, false) => Pcx2bppCgaPaletteSource::Palette0LowIntensity,
        }
    }
}

/// Four-level CGA composite-monochrome ramp.
///
/// When the CGA color-burst bit is set (header byte 19 bit 7 `C = 1`,
/// "color burst enable - 1 = monochrome" per the verbatim ZSoft manual,
/// "CGA Color Map"), the CGA display drives a composite-monochrome signal
/// rather than the chroma palettes, so the four 2-bit indices map to a
/// four-level grey ramp. The manual does not tabulate RGB values for this
/// mode, but the spec's own EGA quantisation table ("EGA/VGA 16-color
/// palette") defines four signal levels (0 / 1 / 2 / 3); mapping those
/// four levels onto the byte range gives the evenly-spaced ramp
/// `0x00 / 0x55 / 0xAA / 0xFF`. The `bright` flavour (intensity bit
/// `I = 1`) lifts the two darker mid-levels toward white, matching the
/// dim/bright intensity axis the manual places on bit 5; the `dim`
/// flavour keeps the even ramp.
const CGA_MONO_DIM: [[u8; 3]; 4] = [
    [0x00, 0x00, 0x00],
    [0x55, 0x55, 0x55],
    [0xAA, 0xAA, 0xAA],
    [0xFF, 0xFF, 0xFF],
];
const CGA_MONO_BRIGHT: [[u8; 3]; 4] = [
    [0x00, 0x00, 0x00],
    [0x80, 0x80, 0x80],
    [0xD4, 0xD4, 0xD4],
    [0xFF, 0xFF, 0xFF],
];

/// Resolve a CGA 4-colour palette from the in-header bytes per the
/// verbatim ZSoft manual's authoritative byte-19 C / P / I decomposition
/// ("CGA Color Map", Header Byte #19): bit 7 = `C` (color burst,
/// 0 = color / 1 = monochrome), bit 6 = `P` (palette, 0 = yellow family /
/// 1 = white family), bit 5 = `I` (intensity, 0 = dim / 1 = bright).
///
/// This is the spec-faithful sibling of [`cga_palette_from_header`],
/// which reads only bits 7 / 6 and cannot represent the monochrome axis
/// nor the intensity bit at position 5. Used by the
/// [`parse_pcx_indexed_2bpp_cga_cpi`] typed accessor and the
/// [`parse_pcx_cga_cpi`] flatten entry point (which together cover the
/// `color burst = monochrome` mode neither legacy path can express).
///
/// Palette entry 0 is overridden by the header byte 16 high-nibble
/// background colour in both the colour and monochrome cases, matching
/// the legacy resolver and the manual's "background color is determined
/// in the upper four bits" rule.
pub(crate) fn cga_palette_from_cpi(
    raw: &[u8; 48],
    cpi: crate::image::Pcx2bppCgaCpi,
) -> [[u8; 3]; 4] {
    // Manual "CGA Color Map": the background nibble lives in Header
    // Byte #16 = colormap byte 0 (r401 conformance fix; this read sat
    // at colormap byte 16 = header byte 32 before).
    let bg_idx = (raw[0] >> 4) as usize;
    let mut p = if cpi.monochrome {
        if cpi.intensity_bright {
            CGA_MONO_BRIGHT
        } else {
            CGA_MONO_DIM
        }
    } else {
        match (cpi.palette_white, cpi.intensity_bright) {
            // P = 1 (white family) = cyan / magenta / white.
            (true, true) => CGA_PALETTE_1_HIGH,
            (true, false) => CGA_PALETTE_1_LOW,
            // P = 0 (yellow family) = green / red / brown.
            (false, true) => CGA_PALETTE_0_HIGH,
            (false, false) => CGA_PALETTE_0_LOW,
        }
    };
    p[0] = EGA_DEFAULT_PALETTE[bg_idx];
    p
}

/// Extract a 16-entry RGB palette from the 48-byte `ega_palette`
/// header field. If the field is all zeros (which PCX 3.0+ files may
/// emit even for EGA data), fall back to the standard EGA hardware
/// palette listed in spec table §3.1.
pub(crate) fn ega_palette_or_default(raw: &[u8; 48]) -> [[u8; 3]; 16] {
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
