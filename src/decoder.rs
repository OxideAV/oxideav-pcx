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
//! * 1 bpp × 2 planes — 4-colour CGA, plane-oriented (the EGFF
//!   canonical mode matrix lists CGA as `BitsPerPixel = 1,
//!   NumBitPlanes = 2`). Each scanline carries plane 0 then plane 1;
//!   the bit at the same x-position in each plane stacks into the 2-bit
//!   palette index (`p0 | p1 << 1`). Same CGA palette resolution as the
//!   packed `2 bpp × 1 plane` mode.
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
use crate::image::{
    Pcx1bpp3PlanesPaletteSource, Pcx1bpp4PlanesPaletteSource, Pcx2bppCgaCpi,
    Pcx2bppCgaPaletteSource, Pcx4bppPaletteSource, PcxImage, PcxIndexed1x2Cga, PcxIndexed1x3,
    PcxIndexed1x4, PcxIndexed2x1Cga, PcxIndexed2x1CgaCpi, PcxIndexed4, PcxIndexed8,
    PcxPaletteSource, PcxPixelFormat,
};
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
    // Shared validation + RLE decode (see `decode_planar_scanlines`):
    // every clean-room guard — manufacturer byte, version table,
    // encoding byte, dimension underflow, `bytes_per_line < min_bpl`
    // mis-framing, `scanline × height` overflow, decompression-bomb
    // cap — lives in one place so the typed paletted accessors (e.g.
    // `parse_pcx_indexed_8bpp`) stay in lockstep with this entry point.
    let (header, pixels_planar, vga_palette) = decode_planar_scanlines(input)?;
    let width = header.width();
    let height = header.height();

    // Re-pack planar scanlines into packed RGBA pixels per
    // (depth, n_planes) combination.
    let data = match (header.bits_per_pixel, header.n_planes) {
        (1, 1) => unpack_1bpp_1plane(&header, &pixels_planar),
        (1, 2) => unpack_1bpp_2planes_cga(&header, &pixels_planar),
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

    // Surface the window origin only when at least one of `x_min` /
    // `y_min` is non-zero: PCX 3.0+ allows a non-zero origin to record
    // the source crop region the pixel buffer came from (spec §3 derives
    // visible width / height as `x_max - x_min + 1` / `y_max - y_min +
    // 1`). The overwhelmingly common screen-authored case sits at
    // `(0, 0)` — surfacing that as `None` keeps the re-encode wrapper
    // emitting the conventional zero-origin header rather than restating
    // an implicit default.
    let window_origin = if header.x_min != 0 || header.y_min != 0 {
        Some((header.x_min, header.y_min))
    } else {
        None
    };

    // Surface the authoring screen size only when BOTH header words
    // carry a non-zero value: per spec §3 the `h_screen_size` /
    // `v_screen_size` fields are PB IV / IV Plus additions and pre-PB-IV
    // writers leave them at zero, so an asymmetric (0, 600) header
    // would not be a meaningful display-size annotation. Treating both-
    // non-zero as the sentinel keeps the round-trip wrapper from
    // restating an implicit default.
    let screen_size = if header.h_screen_size != 0 && header.v_screen_size != 0 {
        Some((header.h_screen_size, header.v_screen_size))
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
        window_origin,
        screen_size,
    })
}

/// Decode an 8 bpp × 1 plane PCX into a typed paletted view (indices +
/// resolved 256-entry palette).
///
/// The standard [`parse_pcx`] entry point always flattens the on-disk
/// image to packed `Rgba`, which is convenient for display pipelines
/// but discards the palette indices the file actually carries. This
/// typed accessor preserves them: the returned [`PcxIndexed8`] surfaces
/// the `width × height` index buffer (one byte per pixel, top-down)
/// alongside the resolved 256-entry RGB palette and a
/// [`PcxPaletteSource`] tag that records which spec §3 branch produced
/// it. Useful for round-tripping a paletted PCX through
/// [`crate::encode_pcx_8bpp_indexed`] without re-quantising the
/// pixels.
///
/// Rejects any (depth, planes) combination other than `(8, 1)` with
/// [`Error::unsupported`]: 24-bit PCX and the various EGA/CGA paletted
/// modes have different palette geometries and are best served by
/// dedicated typed accessors (the 24-bit `(8, 3)` planar path has its
/// own RGB shape; the 1/2/4 bpp paths share a sub-byte unpacking step
/// that doesn't fit a one-byte-per-pixel index buffer).
pub fn parse_pcx_indexed_8bpp(input: &[u8]) -> Result<PcxIndexed8> {
    let (header, scanlines, vga_palette) = decode_planar_scanlines(input)?;
    if (header.bits_per_pixel, header.n_planes) != (8, 1) {
        return Err(Error::unsupported(format!(
            "PCX: parse_pcx_indexed_8bpp expects 8 bpp × 1 plane, found {} bpp × {} planes",
            header.bits_per_pixel, header.n_planes
        )));
    }
    let width = header.width() as usize;
    let height = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    // Strip per-row padding: spec §1 rounds `bytes_per_line` up to an
    // even number, so the on-disk scanline can carry one trailing byte
    // beyond the visible width that the writer set to a don't-care
    // value. The typed view surfaces only the visible pixels so the
    // caller's index buffer matches `width × height` exactly.
    let mut indices = Vec::with_capacity(width * height);
    for row in scanlines.chunks_exact(bpl) {
        indices.extend_from_slice(&row[..width]);
    }
    debug_assert_eq!(indices.len(), width * height);

    // Resolve the palette: `palette_info = 2` forces the grayscale
    // interpretation per spec §3 (even if a VGA tail block is also
    // present); otherwise honour the tail block; otherwise fall back to
    // the deterministic `0..=255` ramp so the field is never left
    // implementation-defined.
    let (palette, palette_source) = if header.palette_info == 2 {
        (grayscale_palette_256(), PcxPaletteSource::GrayscaleFlag)
    } else if let Some(p) = vga_palette {
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
/// The standard [`parse_pcx`] entry point always flattens the on-disk
/// image to packed `Rgba`, which is convenient for display pipelines
/// but discards the palette indices the file actually carries. This
/// typed accessor preserves them for the 16-colour packed-bits mode
/// listed in EGFF table entry "4 bpp / 1 plane / 16 colours / EGA and
/// VGA": the returned [`PcxIndexed4`] surfaces one byte per pixel (low
/// nibble = palette index `0..=15`, top-down, padding stripped)
/// alongside the resolved 16-entry RGB palette and a
/// [`Pcx4bppPaletteSource`] tag recording whether the header carried a
/// non-zero `ega_palette` field or the spec table §3.1 hardware default
/// was substituted.
///
/// Useful for round-tripping a 16-colour PCX through
/// [`crate::encode_pcx_4bpp_packed`] without re-quantising the pixels,
/// or for applying palette-swap operations on the indices directly.
///
/// Rejects any (depth, planes) combination other than `(4, 1)` with
/// [`Error::unsupported`]: the 8 bpp paletted path has its own typed
/// accessor [`parse_pcx_indexed_8bpp`]; the 1 bpp × 4 planes path
/// shares the 16-colour palette geometry but the on-disk plane shape
/// is different and is not covered by this accessor.
pub fn parse_pcx_indexed_4bpp(input: &[u8]) -> Result<PcxIndexed4> {
    let (header, scanlines, _vga_palette) = decode_planar_scanlines(input)?;
    if (header.bits_per_pixel, header.n_planes) != (4, 1) {
        return Err(Error::unsupported(format!(
            "PCX: parse_pcx_indexed_4bpp expects 4 bpp × 1 plane, found {} bpp × {} planes",
            header.bits_per_pixel, header.n_planes
        )));
    }
    let width = header.width() as usize;
    let height = header.height() as usize;
    let bpl = header.bytes_per_line as usize;

    // Unpack each scanline: 4 bpp packs two pixels per byte (high
    // nibble = even-x pixel, low nibble = odd-x pixel) per the spec
    // §4.1 packed-bits layout. The on-disk row may carry trailing
    // padding bytes beyond the visible width (spec §1 rounds
    // `bytes_per_line` up to an even number); the typed view surfaces
    // only the visible pixels so the caller's index buffer matches
    // `width × height` exactly.
    let mut indices = Vec::with_capacity(width * height);
    for row in scanlines.chunks_exact(bpl) {
        for x in 0..width {
            let byte = row[x >> 1];
            let nib = if x & 1 == 0 {
                (byte >> 4) & 0x0F
            } else {
                byte & 0x0F
            };
            indices.push(nib);
        }
    }
    debug_assert_eq!(indices.len(), width * height);

    // Resolve the 16-entry palette: if the header `ega_palette` field
    // carries at least one non-zero byte, surface those 48 bytes as 16
    // RGB triplets. Otherwise fall back to the spec table §3.1
    // hardware default — matching the symmetric branch
    // [`ega_palette_or_default`] takes inside the canonical RGBA
    // flattener so the typed view never diverges.
    let (palette, palette_source) = if header.ega_palette.iter().any(|&b| b != 0) {
        let mut out = [[0u8; 3]; 16];
        for (i, e) in out.iter_mut().enumerate() {
            *e = [
                header.ega_palette[i * 3],
                header.ega_palette[i * 3 + 1],
                header.ega_palette[i * 3 + 2],
            ];
        }
        (out, Pcx4bppPaletteSource::Ega16InHeader)
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

/// Decode a 1 bpp × 4 planes PCX into a typed paletted view (16-colour
/// nibble indices + resolved 16-entry palette).
///
/// The standard [`parse_pcx`] entry point always flattens the on-disk
/// image to packed `Rgba`, which is convenient for display pipelines
/// but discards the per-plane bits the file actually carries. This
/// typed accessor preserves the resolved 4-bit palette index per pixel
/// for the 16-colour EGA bit-plane mode described in spec §4.1: the
/// returned [`PcxIndexed1x4`] surfaces one byte per pixel (low nibble
/// = palette index `0..=15`, top-down, padding stripped) alongside the
/// resolved 16-entry RGB palette and a
/// [`Pcx1bpp4PlanesPaletteSource`] tag recording whether the header
/// carried a non-zero `ega_palette` field or the spec table §3.1
/// hardware default was substituted.
///
/// Useful for round-tripping a 16-colour EGA PCX through
/// [`crate::encode_pcx_1bpp_4planes_ega`] without re-quantising the
/// pixels, or for applying palette-swap operations on the indices
/// directly. The surfaced nibble values share the [`PcxIndexed4`]
/// convention so a caller can hand either view to a 16-colour pipeline
/// without branching on the on-disk depth.
///
/// Rejects any (depth, planes) combination other than `(1, 4)` with
/// [`Error::unsupported`]: the 4 bpp × 1 plane path has its own typed
/// accessor [`parse_pcx_indexed_4bpp`]; the 8 bpp paletted path has
/// [`parse_pcx_indexed_8bpp`]. Both share the 16-colour palette
/// geometry but the on-disk plane shape differs.
pub fn parse_pcx_indexed_1bpp_4planes(input: &[u8]) -> Result<PcxIndexed1x4> {
    let (header, scanlines, _vga_palette) = decode_planar_scanlines(input)?;
    if (header.bits_per_pixel, header.n_planes) != (1, 4) {
        return Err(Error::unsupported(format!(
            "PCX: parse_pcx_indexed_1bpp_4planes expects 1 bpp × 4 planes, found {} bpp × {} planes",
            header.bits_per_pixel, header.n_planes
        )));
    }
    let width = header.width() as usize;
    let height = header.height() as usize;
    let bpl = header.bytes_per_line as usize;

    // For each scanline read four `bpl`-byte plane slices in order
    // (plane 0, plane 1, plane 2, plane 3). Per spec §4.1 the bit at
    // x-position `(x>>3, 7 - (x&7))` of each plane contributes one bit
    // to the 4-bit palette index; the stack order matches the
    // canonical RGBA flattener `unpack_1bpp_4planes` so the typed view
    // never diverges from the byte stream `parse_pcx` produces. The
    // on-disk row may carry trailing padding bits beyond the visible
    // width (spec §1 rounds `bytes_per_line` up to an even number); the
    // typed view surfaces only the visible pixels.
    let mut indices = Vec::with_capacity(width * height);
    for row in scanlines.chunks_exact(bpl * 4) {
        let (p0, rest) = row.split_at(bpl);
        let (p1, rest) = rest.split_at(bpl);
        let (p2, p3) = rest.split_at(bpl);
        for x in 0..width {
            let byte = x >> 3;
            let shift = 7 - (x & 7);
            let idx = ((p0[byte] >> shift) & 1)
                | (((p1[byte] >> shift) & 1) << 1)
                | (((p2[byte] >> shift) & 1) << 2)
                | (((p3[byte] >> shift) & 1) << 3);
            indices.push(idx);
        }
    }
    debug_assert_eq!(indices.len(), width * height);

    // Resolve the 16-entry palette using the same `Ega16InHeader` /
    // `Ega16Default` decision the 4 bpp × 1 plane accessor uses; the
    // two modes share the spec §3 palette geometry even though the
    // on-disk plane shape is different.
    let (palette, palette_source) = if header.ega_palette.iter().any(|&b| b != 0) {
        let mut out = [[0u8; 3]; 16];
        for (i, e) in out.iter_mut().enumerate() {
            *e = [
                header.ega_palette[i * 3],
                header.ega_palette[i * 3 + 1],
                header.ega_palette[i * 3 + 2],
            ];
        }
        (out, Pcx1bpp4PlanesPaletteSource::Ega16InHeader)
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
/// The standard [`parse_pcx`] entry point always flattens the on-disk
/// image to packed `Rgba` by walking the palette per pixel and dropping
/// the resolved indices. This typed accessor preserves them for the
/// 4-colour CGA mode described in spec §4.1 (single plane of 2 bpp
/// packed-bits data, 4 pixels/byte, palette selected from `ega_palette`
/// bytes 16 / 19 per CGA hardware semantics).
///
/// The returned [`PcxIndexed2x1Cga`] surfaces one byte per pixel (low
/// two bits = palette index `0..=3`, top-down, padding stripped)
/// alongside the resolved 4-entry RGB palette, the resolved
/// `background_index` (`0..=15`) used for palette entry 0, and a
/// [`Pcx2bppCgaPaletteSource`] tag recording which CGA palette family
/// the decoder landed on. The
/// [`Pcx2bppCgaPaletteSource::palette_selector`] helper reconstructs
/// the byte 19 selector pattern so a round-trip caller can hand it
/// straight to [`crate::encode_pcx_2bpp_cga`] without re-deriving the
/// bit positions.
///
/// Useful for round-tripping a 4-colour CGA PCX through
/// [`crate::encode_pcx_2bpp_cga`] without re-quantising the indices, or
/// for applying palette-swap operations on the indices directly.
///
/// Rejects any (depth, planes) combination other than `(2, 1)` with
/// [`Error::unsupported`]: the 16-colour packed-bits path has its own
/// typed accessor [`parse_pcx_indexed_4bpp`]; the 8 bpp paletted path
/// has [`parse_pcx_indexed_8bpp`]; the EGA bit-plane path has
/// [`parse_pcx_indexed_1bpp_4planes`].
pub fn parse_pcx_indexed_2bpp_cga(input: &[u8]) -> Result<PcxIndexed2x1Cga> {
    let (header, scanlines, _vga_palette) = decode_planar_scanlines(input)?;
    if (header.bits_per_pixel, header.n_planes) != (2, 1) {
        return Err(Error::unsupported(format!(
            "PCX: parse_pcx_indexed_2bpp_cga expects 2 bpp × 1 plane, found {} bpp × {} planes",
            header.bits_per_pixel, header.n_planes
        )));
    }
    let width = header.width() as usize;
    let height = header.height() as usize;
    let bpl = header.bytes_per_line as usize;

    // Unpack each scanline: 2 bpp packs four pixels per byte (top two
    // bits = pixel 0, then 2/3, 4/5, 6/7) per the spec §4.1 packed-bits
    // layout. The on-disk row may carry trailing padding bytes beyond
    // the visible width (spec §1 rounds `bytes_per_line` up to an even
    // number); the typed view surfaces only the visible pixels so the
    // caller's index buffer matches `width × height` exactly.
    let mut indices = Vec::with_capacity(width * height);
    for row in scanlines.chunks_exact(bpl) {
        for x in 0..width {
            let byte = row[x >> 2];
            let shift = 6 - 2 * (x & 3);
            indices.push((byte >> shift) & 0b11);
        }
    }
    debug_assert_eq!(indices.len(), width * height);

    // Resolve the 4-entry palette: same dispatch as the canonical
    // flattener [`unpack_2bpp_1plane_cga`]. `ega_palette[16]` high
    // nibble = EGA index for palette entry 0 (the "background"
    // colour); `ega_palette[19]` bits 7/6 = palette select + intensity
    // per CGA hardware semantics.
    let palette = cga_palette_from_header(&header.ega_palette);
    let background_index = (header.ega_palette[16] >> 4) & 0x0F;
    let selector_bits = header.ega_palette[19] & 0xC0;
    let palette_source = match selector_bits {
        0x00 => Pcx2bppCgaPaletteSource::Palette1HighIntensity,
        0x40 => Pcx2bppCgaPaletteSource::Palette1LowIntensity,
        0x80 => Pcx2bppCgaPaletteSource::Palette0HighIntensity,
        0xC0 => Pcx2bppCgaPaletteSource::Palette0LowIntensity,
        // The match above is exhaustive over the two-bit selector
        // field, but the compiler can't prove it from `& 0xC0` alone.
        _ => unreachable!("selector_bits is masked with 0xC0"),
    };

    Ok(PcxIndexed2x1Cga {
        width: header.width(),
        height: header.height(),
        indices,
        palette,
        background_index,
        palette_source,
    })
}

/// Decode a 1 bpp × 2 planes CGA PCX into a typed paletted view
/// (indices + resolved 4-entry palette).
///
/// This is the plane-oriented sibling of [`parse_pcx_indexed_2bpp_cga`]:
/// the EGFF canonical PCX mode matrix lists 4-colour CGA as
/// `BitsPerPixel = 1, NumBitPlanes = 2`, the bit-plane layout that
/// matches CGA hardware's two display planes, distinct from the
/// `2 bpp × 1 plane` packed-bits layout the other accessor reads. Each
/// on-disk scanline carries plane 0 then plane 1 one after another; the
/// bit at the same x-position in each plane stacks into the 2-bit
/// palette index (`p0 | p1 << 1`), the same bit ordering the 4-plane
/// EGA path uses (plane k contributes bit k).
///
/// The 4-entry palette resolution is identical to
/// [`parse_pcx_indexed_2bpp_cga`] — `ega_palette[16]` high nibble =
/// background EGA index for palette entry 0, `ega_palette[19]` bits 7/6
/// = palette family + intensity — so the returned [`PcxIndexed1x2Cga`]
/// reuses the [`Pcx2bppCgaPaletteSource`] tag and surfaces the same
/// `background_index`. The [`Pcx2bppCgaPaletteSource::palette_selector`]
/// helper reconstructs the byte 19 selector pattern so a round-trip
/// caller can hand the view straight back to
/// [`crate::encode_pcx_1bpp_2planes_cga`].
///
/// Rejects any (depth, planes) combination other than `(1, 2)` with
/// [`Error::unsupported`]: the packed `2 bpp × 1 plane` CGA layout has
/// its own typed accessor [`parse_pcx_indexed_2bpp_cga`].
pub fn parse_pcx_indexed_1bpp_2planes_cga(input: &[u8]) -> Result<PcxIndexed1x2Cga> {
    let (header, scanlines, _vga_palette) = decode_planar_scanlines(input)?;
    if (header.bits_per_pixel, header.n_planes) != (1, 2) {
        return Err(Error::unsupported(format!(
            "PCX: parse_pcx_indexed_1bpp_2planes_cga expects 1 bpp × 2 planes, found {} bpp × {} planes",
            header.bits_per_pixel, header.n_planes
        )));
    }
    let width = header.width() as usize;
    let height = header.height() as usize;
    let bpl = header.bytes_per_line as usize;

    // Unpack each scanline: plane 0 followed by plane 1 within the row.
    // The bit at the same x-position in each plane stacks into the 2-bit
    // palette index (`p0 | p1 << 1`). On-disk rows may carry trailing
    // padding bytes beyond the visible width (spec §1 rounds
    // `bytes_per_line` up to an even number); the typed view surfaces
    // only the visible pixels.
    let mut indices = Vec::with_capacity(width * height);
    for row in scanlines.chunks_exact(bpl * 2) {
        let (p0, p1) = row.split_at(bpl);
        for x in 0..width {
            let byte = x >> 3;
            let shift = 7 - (x & 7);
            let idx = ((p0[byte] >> shift) & 1) | (((p1[byte] >> shift) & 1) << 1);
            indices.push(idx);
        }
    }
    debug_assert_eq!(indices.len(), width * height);

    // Resolve the 4-entry palette: identical dispatch to the packed
    // `2 bpp × 1 plane` accessor (see `cga_palette_from_header`).
    let palette = cga_palette_from_header(&header.ega_palette);
    let background_index = (header.ega_palette[16] >> 4) & 0x0F;
    let selector_bits = header.ega_palette[19] & 0xC0;
    let palette_source = match selector_bits {
        0x00 => Pcx2bppCgaPaletteSource::Palette1HighIntensity,
        0x40 => Pcx2bppCgaPaletteSource::Palette1LowIntensity,
        0x80 => Pcx2bppCgaPaletteSource::Palette0HighIntensity,
        0xC0 => Pcx2bppCgaPaletteSource::Palette0LowIntensity,
        _ => unreachable!("selector_bits is masked with 0xC0"),
    };

    Ok(PcxIndexed1x2Cga {
        width: header.width(),
        height: header.height(),
        indices,
        palette,
        background_index,
        palette_source,
    })
}

/// Decode a 2 bpp × 1 plane CGA PCX into a typed paletted view that
/// honours all three C / P / I bits of header byte 19 per the verbatim
/// ZSoft manual ("CGA Color Map").
///
/// This is the spec-faithful sibling of [`parse_pcx_indexed_2bpp_cga`].
/// The older accessor reads only header byte 19 bits 7 / 6 (a
/// `(palette-select, intensity)` two-bit model), so it cannot represent
/// the manual's `color burst = monochrome` mode (bit 7 set) nor the
/// intensity bit the manual places at position 5. This accessor decodes
/// the full [`Pcx2bppCgaCpi`] triple — `C` (bit 7, color burst), `P`
/// (bit 6, palette family), `I` (bit 5, intensity) — and resolves the
/// matching palette, including the four-level composite-grey ramp the
/// monochrome mode produces.
///
/// The returned [`PcxIndexed2x1CgaCpi`] surfaces one byte per pixel (low
/// two bits = palette index `0..=3`, top-down, padding stripped)
/// alongside the resolved 4-entry RGB palette, the `background_index`
/// (`0..=15`) read from header byte 16's high nibble, and the decoded
/// [`Pcx2bppCgaCpi`] bits. [`Pcx2bppCgaCpi::to_byte19`] reconstructs the
/// header byte so a round-trip caller can hand the view straight back to
/// [`crate::encode_pcx_2bpp_cga_cpi`].
///
/// Rejects any (depth, planes) combination other than `(2, 1)` with
/// [`Error::unsupported`].
pub fn parse_pcx_indexed_2bpp_cga_cpi(input: &[u8]) -> Result<PcxIndexed2x1CgaCpi> {
    let (header, scanlines, _vga_palette) = decode_planar_scanlines(input)?;
    if (header.bits_per_pixel, header.n_planes) != (2, 1) {
        return Err(Error::unsupported(format!(
            "PCX: parse_pcx_indexed_2bpp_cga_cpi expects 2 bpp × 1 plane, found {} bpp × {} planes",
            header.bits_per_pixel, header.n_planes
        )));
    }
    let width = header.width() as usize;
    let height = header.height() as usize;
    let bpl = header.bytes_per_line as usize;

    // Unpack each scanline: 2 bpp packs four pixels per byte (top two
    // bits = pixel 0) per the spec §4.1 packed-bits layout. Trailing
    // padding beyond the visible width (spec §1 rounds `bytes_per_line`
    // up to even) is stripped so the index buffer matches `width ×
    // height` exactly.
    let mut indices = Vec::with_capacity(width * height);
    for row in scanlines.chunks_exact(bpl) {
        for x in 0..width {
            let byte = row[x >> 2];
            let shift = 6 - 2 * (x & 3);
            indices.push((byte >> shift) & 0b11);
        }
    }
    debug_assert_eq!(indices.len(), width * height);

    let cpi = Pcx2bppCgaCpi::from_byte19(header.ega_palette[19]);
    let palette = cga_palette_from_cpi(&header.ega_palette, cpi);
    let background_index = (header.ega_palette[16] >> 4) & 0x0F;

    Ok(PcxIndexed2x1CgaCpi {
        width: header.width(),
        height: header.height(),
        indices,
        palette,
        background_index,
        cpi,
    })
}

/// Decode a 1 bpp × 3 planes PCX into a typed paletted view (8-colour
/// EGA RGB indices + the fixed 8-entry on/off-primary palette).
///
/// The standard [`parse_pcx`] entry point always flattens the on-disk
/// image to packed `Rgba` by toggling each channel per plane bit and
/// dropping the resolved colour index. This typed accessor preserves it
/// for the 8-colour EGA RGB mode described in spec §4 (each scanline
/// carries three 1-bit planes laid out one after another within the row,
/// plane order R, G, B — the same order
/// [`crate::encode_pcx_1bpp_3planes_ega_rgb`] writes). The three bits at
/// the same x-position stack into a 3-bit index (`r | g << 1 | b << 2`).
///
/// The returned [`PcxIndexed1x3`] surfaces one byte per pixel (low three
/// bits = colour index `0..=7`, top-down, padding stripped) alongside
/// the fixed 8-entry RGB palette and a [`Pcx1bpp3PlanesPaletteSource`]
/// tag. Unlike the other paletted accessors, this mode carries no
/// on-disk palette — the eight colours are the on/off primaries
/// enumerated by the plane bits themselves, so the source tag has a
/// single [`Pcx1bpp3PlanesPaletteSource::FixedPrimaries`] arm.
///
/// Useful for round-tripping an 8-colour EGA RGB PCX through
/// [`crate::encode_pcx_1bpp_3planes_ega_rgb`] without re-thresholding,
/// or for applying colour-swap operations on the indices directly.
///
/// Rejects any (depth, planes) combination other than `(1, 3)` with
/// [`Error::unsupported`]: the 16-colour bit-plane path has its own
/// typed accessor [`parse_pcx_indexed_1bpp_4planes`]; the 8 bpp paletted
/// path has [`parse_pcx_indexed_8bpp`]; the 4 bpp path has
/// [`parse_pcx_indexed_4bpp`]; the CGA path has
/// [`parse_pcx_indexed_2bpp_cga`].
pub fn parse_pcx_indexed_1bpp_3planes(input: &[u8]) -> Result<PcxIndexed1x3> {
    let (header, scanlines, _vga_palette) = decode_planar_scanlines(input)?;
    if (header.bits_per_pixel, header.n_planes) != (1, 3) {
        return Err(Error::unsupported(format!(
            "PCX: parse_pcx_indexed_1bpp_3planes expects 1 bpp × 3 planes, found {} bpp × {} planes",
            header.bits_per_pixel, header.n_planes
        )));
    }
    let width = header.width() as usize;
    let height = header.height() as usize;
    let bpl = header.bytes_per_line as usize;

    // For each scanline read three `bpl`-byte plane slices in order
    // (plane 0 = R, plane 1 = G, plane 2 = B). Per spec §4 the bit at
    // x-position `(x>>3, 7 - (x&7))` of each plane contributes one bit to
    // the 3-bit colour index in the order `r | g << 1 | b << 2`; the
    // stack order matches the canonical RGBA flattener
    // `unpack_1bpp_3planes` so the typed view never diverges from the
    // byte stream `parse_pcx` produces. The on-disk row may carry
    // trailing padding bits beyond the visible width (spec §1 rounds
    // `bytes_per_line` up to an even number); the typed view surfaces
    // only the visible pixels.
    let mut indices = Vec::with_capacity(width * height);
    for row in scanlines.chunks_exact(bpl * 3) {
        let (rp, rest) = row.split_at(bpl);
        let (gp, bp) = rest.split_at(bpl);
        for x in 0..width {
            let byte = x >> 3;
            let shift = 7 - (x & 7);
            let idx = ((rp[byte] >> shift) & 1)
                | (((gp[byte] >> shift) & 1) << 1)
                | (((bp[byte] >> shift) & 1) << 2);
            indices.push(idx);
        }
    }
    debug_assert_eq!(indices.len(), width * height);

    Ok(PcxIndexed1x3 {
        width: header.width(),
        height: header.height(),
        indices,
        palette: RGB_PRIMARIES_PALETTE,
        palette_source: Pcx1bpp3PlanesPaletteSource::FixedPrimaries,
    })
}

/// The fixed 8-entry RGB palette of on/off primaries the 1 bpp × 3
/// planes 8-colour EGA RGB mode resolves to (spec §4 bit-plane example).
/// Entry `i` has channel `c` set to `0xFF` iff the matching plane bit is
/// set: `r = i & 1`, `g = i & 2`, `b = i & 4`. Matches the per-pixel
/// `0x00` / `0xFF` toggling in [`unpack_1bpp_3planes`] so the typed view
/// flattens to byte-identical RGBA.
const RGB_PRIMARIES_PALETTE: [[u8; 3]; 8] = [
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

/// Return shape of [`decode_planar_scanlines`]: the parsed header, the
/// fully-RLE-decoded planar pixel buffer (`n_planes × bytes_per_line ×
/// height` bytes), and the resolved optional VGA tail palette slice.
type PlanarDecode<'a> = (PcxHeader, Vec<u8>, Option<&'a [u8]>);

/// Shared header-validation + RLE-decode step that produces the planar
/// scanline buffer (`n_planes × bytes_per_line × height` bytes) plus
/// the resolved VGA palette slice, if any. Centralising the validation
/// keeps [`parse_pcx`] and the typed accessors (e.g.
/// [`parse_pcx_indexed_8bpp`]) in lockstep on every clean-room guard
/// established in earlier rounds: manufacturer byte, version table,
/// encoding byte, dimension underflow, `bytes_per_line < min_bpl`
/// mis-framing, `scanline × height` overflow, and the
/// decompression-bomb cap.
fn decode_planar_scanlines(input: &[u8]) -> Result<PlanarDecode<'_>> {
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

    let scanline = header.scanline_bytes();
    let total_planar = scanline
        .checked_mul(height as usize)
        .ok_or_else(|| Error::invalid("PCX: scanline × height overflows usize"))?;
    let mut cursor = PCX_HEADER_SIZE;
    let vga_palette = find_vga_palette(input);
    let rle_end = if vga_palette.is_some() {
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
    for _ in 0..height as usize {
        let consumed = rle::decode(&input[cursor..rle_end], &mut pixels_planar, scanline)?;
        cursor += consumed;
    }
    Ok((header, pixels_planar, vga_palette))
}

/// Benchmark probe: run only the header-validation + RLE-decode phase
/// of [`parse_pcx`] and return the length of the resulting planar
/// scanline buffer (`n_planes × bytes_per_line × height` bytes).
///
/// This exists so the Criterion suite can time the RLE-unpack phase in
/// isolation from the per-plane assembly phase, making the BENCHMARKS.md
/// hotspot ranking a measured split rather than an inference. It runs
/// the exact same `decode_planar_scanlines` the production decoder
/// calls — no parallel code path — so the timing is faithful. Returning
/// only the byte count (not the buffer or the private `PcxHeader`) keeps
/// the crate's public type surface unchanged. Not part of the stable
/// API; hidden from docs and intended for benches only.
#[doc(hidden)]
pub fn __bench_decode_planar_len(input: &[u8]) -> Result<usize> {
    let (_header, pixels_planar, _vga) = decode_planar_scanlines(input)?;
    Ok(pixels_planar.len())
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

fn unpack_1bpp_2planes_cga(header: &PcxHeader, planar: &[u8]) -> Vec<u8> {
    // 4-colour CGA stored as TWO 1-bit planes (the EGFF canonical mode
    // matrix lists CGA as `BitsPerPixel = 1, NumBitPlanes = 2`, the
    // plane-oriented sibling of the `2 bpp × 1 plane` packed-bits CGA
    // layout). Each on-disk scanline carries plane 0 then plane 1, one
    // after another within the row. The bit at the same x-position in
    // each plane stacks into the 2-bit palette index
    // (`p0 | p1 << 1`), matching the bit ordering the 4-plane EGA path
    // uses (plane k contributes bit k of the index). The 4-entry CGA
    // palette is resolved from the same header bytes 16/19 the packed
    // `2 bpp × 1 plane` path uses (see `cga_palette_from_header`).
    let w = header.width() as usize;
    let h = header.height() as usize;
    let bpl = header.bytes_per_line as usize;
    let cga = cga_palette_from_header(&header.ega_palette);
    let palette: [[u8; 4]; 4] = [
        [cga[0][0], cga[0][1], cga[0][2], 0xFF],
        [cga[1][0], cga[1][1], cga[1][2], 0xFF],
        [cga[2][0], cga[2][1], cga[2][2], 0xFF],
        [cga[3][0], cga[3][1], cga[3][2], 0xFF],
    ];
    let mut out = vec![0u8; w * h * 4];
    let src_rows = planar.chunks_exact(bpl * 2);
    let dst_rows = out.chunks_exact_mut(w * 4);
    for (row, dst_row) in src_rows.zip(dst_rows) {
        let (p0, p1) = row.split_at(bpl);
        for (x, dst) in dst_row.chunks_exact_mut(4).enumerate() {
            let byte = x >> 3;
            let shift = 7 - (x & 7);
            let idx = (((p0[byte] >> shift) & 1) | (((p1[byte] >> shift) & 1) << 1)) as usize;
            dst.copy_from_slice(&palette[idx]);
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
/// [`parse_pcx_indexed_2bpp_cga_cpi`] accessor and the
/// `color burst = monochrome` flatten path.
///
/// Palette entry 0 is overridden by the header byte 16 high-nibble
/// background colour in both the colour and monochrome cases, matching
/// the legacy resolver and the manual's "background color is determined
/// in the upper four bits" rule.
pub(crate) fn cga_palette_from_cpi(
    raw: &[u8; 48],
    cpi: crate::image::Pcx2bppCgaCpi,
) -> [[u8; 3]; 4] {
    let bg_idx = (raw[16] >> 4) as usize;
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
