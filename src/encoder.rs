//! PCX encode. Always emits PCX 5.0.
//!
//! Write paths:
//!
//! * [`encode_pcx_8bpp_indexed`] — 8 bpp × 1 plane, palette index data
//!   plus a 768-byte VGA palette appended after the pixel block (with
//!   the leading `0x0C` marker).
//! * [`encode_pcx_24bpp`] — 8 bpp × 3 planes, planar RGB. No tail
//!   palette.
//! * [`encode_pcx_1bpp_mono`] — 1 bpp × 1 plane monochrome. Bit 1 =
//!   white, bit 0 = black per spec §4.1.
//! * [`encode_pcx_4bpp_packed`] — 4 bpp × 1 plane packed-bits with a
//!   16-entry EGA palette in the header.
//! * [`encode_pcx_2bpp_cga`] — 2 bpp × 1 plane CGA packed-bits using
//!   the legacy 4-colour palette selector (header byte 16 / 19).
//! * [`encode_pcx_1bpp_3planes_ega_rgb`] — 1 bpp × 3 planes 8-colour
//!   EGA RGB. Each input channel is thresholded at 0x80 into the
//!   matching bit-plane; plane order is R, G, B per spec §4.
//! * [`encode_pcx_1bpp_4planes_ega`] — 1 bpp × 4 planes EGA with a
//!   16-entry palette in the header.
//!
//! The framework-side `Encoder` constructed via [`make_encoder`]
//! accepts video frames in seven [`oxideav_core::PixelFormat`]
//! variants: `Rgba` / `Rgb24` / `Bgr24` / `Bgra` route to
//! [`encode_pcx_24bpp`] (`Bgr*` per-pixel byte-swapped to RGB,
//! alpha dropped from `Rgba` / `Bgra`); `Gray8` routes to
//! [`encode_pcx_8bpp_grayscale`] (8 bpp × 1 plane, `palette_info =
//! 2`, no VGA tail palette per spec §3); `MonoBlack` / `MonoWhite`
//! unpack the MSB-first 1-bit stride into one byte per pixel and
//! route to [`encode_pcx_1bpp_mono`] (with `MonoWhite` bit-inverted
//! so the on-disk PCX retains the spec §4.1 bit-1 = white polarity).
//!
//! The RLE encoder coalesces runs of identical bytes (≤ 63 each) and
//! escapes any singleton byte ≥ `0xC0` into a length-1 packet so the
//! decoder won't mistake it for a run header.

use crate::error::{PcxError as Error, Result};
use crate::image::{Pcx2bppCgaCpi, PcxImage, PcxPixelFormat};
use crate::rle;
use crate::types::*;

#[cfg(feature = "registry")]
use oxideav_core::Encoder;
#[cfg(feature = "registry")]
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, PixelFormat, TimeBase};

#[cfg(feature = "registry")]
pub fn make_encoder(params: &CodecParameters) -> oxideav_core::Result<Box<dyn Encoder>> {
    let mut out_params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    out_params.width = params.width;
    out_params.height = params.height;
    out_params.pixel_format = params.pixel_format;
    Ok(Box::new(PcxEncoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        out_params,
        pending: None,
        eof: false,
    }))
}

#[cfg(feature = "registry")]
struct PcxEncoder {
    codec_id: CodecId,
    out_params: CodecParameters,
    pending: Option<Vec<u8>>,
    eof: bool,
}

#[cfg(feature = "registry")]
impl Encoder for PcxEncoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn output_params(&self) -> &CodecParameters {
        &self.out_params
    }
    fn send_frame(&mut self, frame: &Frame) -> oxideav_core::Result<()> {
        let vf = match frame {
            Frame::Video(v) => v,
            _ => {
                return Err(oxideav_core::Error::invalid(
                    "PCX encoder: expected video frame",
                ))
            }
        };
        let format = self.out_params.pixel_format.ok_or_else(|| {
            oxideav_core::Error::invalid("PCX encoder: pixel_format missing in CodecParameters")
        })?;
        let width = self.out_params.width.ok_or_else(|| {
            oxideav_core::Error::invalid("PCX encoder: width missing in CodecParameters")
        })?;
        let height = self.out_params.height.ok_or_else(|| {
            oxideav_core::Error::invalid("PCX encoder: height missing in CodecParameters")
        })?;
        if vf.planes.is_empty() {
            return Err(oxideav_core::Error::invalid(
                "PCX encoder: empty frame plane",
            ));
        }
        let plane = &vf.planes[0];
        let w16: u16 = width
            .try_into()
            .map_err(|_| oxideav_core::Error::invalid("PCX encoder: width exceeds 65535"))?;
        let h16: u16 = height
            .try_into()
            .map_err(|_| oxideav_core::Error::invalid("PCX encoder: height exceeds 65535"))?;
        // The byte-per-pixel formats (`Rgb*`, `Bgr*`, `Gray8`) and the
        // packed 1-bit formats (`Mono*`) need different row-tightening
        // strategies. Compute a tight row buffer with stride collapsed
        // for each format, then dispatch to the matching writer.
        let bytes = match format {
            PixelFormat::Rgba => {
                let tight = tighten_packed(plane, width as usize, height as usize, 4)?;
                // Drop alpha — PCX has no alpha channel.
                let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
                for c in tight.chunks_exact(4) {
                    rgb.extend_from_slice(&c[..3]);
                }
                encode_pcx_24bpp(w16, h16, &rgb)?
            }
            PixelFormat::Rgb24 => {
                let tight = tighten_packed(plane, width as usize, height as usize, 3)?;
                encode_pcx_24bpp(w16, h16, &tight)?
            }
            PixelFormat::Bgr24 => {
                // BGR -> swap to RGB before handing off. The frame is
                // 3 bytes/pixel packed; reorder per pixel.
                let tight = tighten_packed(plane, width as usize, height as usize, 3)?;
                let mut rgb = Vec::with_capacity(tight.len());
                for c in tight.chunks_exact(3) {
                    rgb.extend_from_slice(&[c[2], c[1], c[0]]);
                }
                encode_pcx_24bpp(w16, h16, &rgb)?
            }
            PixelFormat::Bgra => {
                // BGRA -> swap to RGB and drop alpha.
                let tight = tighten_packed(plane, width as usize, height as usize, 4)?;
                let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
                for c in tight.chunks_exact(4) {
                    rgb.extend_from_slice(&[c[2], c[1], c[0]]);
                }
                encode_pcx_24bpp(w16, h16, &rgb)?
            }
            // 8 bpp × 1 plane PCX 5.0 with `palette_info = 2` (spec §3
            // grayscale flag); no VGA tail palette is appended. The
            // crate's decoder honours the flag and emits `(g, g, g,
            // 0xFF)` per pixel regardless of any tail palette.
            PixelFormat::Gray8 => {
                let tight = tighten_packed(plane, width as usize, height as usize, 1)?;
                encode_pcx_8bpp_grayscale(w16, h16, &tight)?
            }
            // 1-bit monochrome packed MSB-first per `oxideav-core`'s
            // `MonoBlack` (0 = black, 1 = white) / `MonoWhite` (0 =
            // white, 1 = black) convention. PCX spec §4.1 monochrome
            // uses bit-1 = white, so `MonoBlack` is a direct map and
            // `MonoWhite` requires bit inversion.
            PixelFormat::MonoBlack => {
                let pixels = unpack_msb_mono(
                    plane,
                    width as usize,
                    height as usize,
                    /*invert=*/ false,
                )?;
                encode_pcx_1bpp_mono(w16, h16, &pixels)?
            }
            PixelFormat::MonoWhite => {
                let pixels = unpack_msb_mono(
                    plane,
                    width as usize,
                    height as usize,
                    /*invert=*/ true,
                )?;
                encode_pcx_1bpp_mono(w16, h16, &pixels)?
            }
            other => {
                return Err(oxideav_core::Error::invalid(format!(
                    "PCX encoder: unsupported pixel format {other:?}"
                )))
            }
        };
        self.pending = Some(bytes);
        Ok(())
    }
    fn receive_packet(&mut self) -> oxideav_core::Result<Packet> {
        match self.pending.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
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
fn tighten_packed(
    plane: &oxideav_core::VideoPlane,
    width: usize,
    height: usize,
    bytes_per_pixel: usize,
) -> oxideav_core::Result<Vec<u8>> {
    let want = width * bytes_per_pixel;
    if plane.stride < want {
        return Err(oxideav_core::Error::invalid(format!(
            "PCX encoder: plane stride {} smaller than width × bytes-per-pixel {}",
            plane.stride, want
        )));
    }
    if plane.data.len() < plane.stride * height {
        return Err(oxideav_core::Error::invalid(
            "PCX encoder: plane data shorter than stride × height",
        ));
    }
    let mut tight = Vec::with_capacity(want * height);
    for y in 0..height {
        let off = y * plane.stride;
        tight.extend_from_slice(&plane.data[off..off + want]);
    }
    Ok(tight)
}

#[cfg(feature = "registry")]
fn unpack_msb_mono(
    plane: &oxideav_core::VideoPlane,
    width: usize,
    height: usize,
    invert: bool,
) -> oxideav_core::Result<Vec<u8>> {
    // Per `oxideav_core::PixelFormat::MonoBlack` / `MonoWhite`: 1 bit
    // per pixel packed MSB-first; rows are padded to a byte boundary
    // (and stride may be larger than `ceil(width / 8)`).
    let row_bytes = width.div_ceil(8);
    if plane.stride < row_bytes {
        return Err(oxideav_core::Error::invalid(format!(
            "PCX encoder: mono plane stride {} smaller than width's row-byte count {}",
            plane.stride, row_bytes
        )));
    }
    if plane.data.len() < plane.stride * height {
        return Err(oxideav_core::Error::invalid(
            "PCX encoder: mono plane data shorter than stride × height",
        ));
    }
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        let row = &plane.data[y * plane.stride..y * plane.stride + row_bytes];
        for x in 0..width {
            // bit 0 in MonoBlack source = black per core docs; the
            // round-2 `encode_pcx_1bpp_mono` writer treats input value 1
            // as white and 0 as black (per spec §4.1, bit 1 = white).
            // MonoBlack thus maps straight through; MonoWhite inverts.
            let bit = (row[x / 8] >> (7 - (x % 8))) & 1;
            let v = if invert { 1 - bit } else { bit };
            out.push(v);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Public standalone API
// ---------------------------------------------------------------------------

/// Encode `width × height` indexed pixels (one byte per pixel,
/// row-major, top-down) into a PCX 5.0 file with an appended 256-entry
/// VGA palette.
///
/// `palette` must be exactly 256 RGB triplets (768 bytes). The
/// returned buffer carries a single-plane 8 bpp image followed by the
/// `0x0C` palette marker and the 768 palette bytes.
pub fn encode_pcx_8bpp_indexed(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: indexed input shorter than width × height",
        ));
    }
    if palette.len() != PCX_VGA_PALETTE_BYTES {
        return Err(Error::invalid(format!(
            "PCX encoder: 256-colour palette must be exactly {PCX_VGA_PALETTE_BYTES} bytes (got {})",
            palette.len()
        )));
    }
    // Bytes-per-line is the on-disk per-plane row width, rounded up to
    // an even number per spec §1 ("the value must be even"). For
    // 8-bpp single-plane data the natural row width is `width`; we
    // round up if it's odd.
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + indices.len() / 2 + PCX_VGA_PALETTE_BYTES);
    write_header(&mut out, width, height, 8, 1, bytes_per_line);
    // RLE-encode each scanline. If `bytes_per_line > width`, pad with
    // zero bytes so the decoded scanline length matches.
    let mut row = Vec::with_capacity(bytes_per_line as usize);
    for y in 0..height as usize {
        row.clear();
        row.extend_from_slice(&indices[y * width as usize..y * width as usize + width as usize]);
        row.resize(bytes_per_line as usize, 0);
        rle::encode(&row, &mut out);
    }
    // Tail VGA palette block.
    out.push(PCX_VGA_PALETTE_MARKER);
    out.extend_from_slice(palette);
    Ok(out)
}

/// Encode `width × height` packed RGB bytes (3 bytes per pixel,
/// row-major, top-down) into a PCX 5.0 file with three planes (R, G,
/// B) at 8 bpp each. No tail palette is appended.
pub fn encode_pcx_24bpp(width: u16, height: u16, rgb: &[u8]) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if rgb.len() < width as usize * height as usize * 3 {
        return Err(Error::invalid(
            "PCX encoder: rgb input shorter than width × height × 3",
        ));
    }
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + rgb.len() / 2);
    write_header(&mut out, width, height, 8, 3, bytes_per_line);
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
            // Pad this plane out to `bytes_per_line` bytes.
            row.resize((plane + 1) * bytes_per_line as usize, 0);
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode `width × height` packed RGB bytes (3 bytes per pixel,
/// row-major, top-down) into an 8-colour PCX 5.0 file at 1 bpp ×
/// 3 planes.
///
/// Each input byte is thresholded at 0x80 to decide whether its
/// channel bit is set; the resulting on/off triplet maps onto the
/// eight standard EGA RGB primaries. Plane order is R, G, B (spec §4
/// bit-plane example at lines 46-58 of the rev-5 technical
/// reference). `bytes_per_line` is the natural `ceil(width / 8)`
/// rounded up to the next even count.
///
/// The decoder's symmetric `(1, 3)` path produces the same eight
/// primaries: 0x00 / 0xFF per channel, alpha 0xFF.
pub fn encode_pcx_1bpp_3planes_ega_rgb(width: u16, height: u16, rgb: &[u8]) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if rgb.len() < width as usize * height as usize * 3 {
        return Err(Error::invalid(
            "PCX encoder: rgb input shorter than width × height × 3",
        ));
    }
    let bytes_per_line = round_up_to_even(width.div_ceil(8));
    let mut out =
        Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * 3 * height as usize);
    write_header(&mut out, width, height, 1, 3, bytes_per_line);
    let mut row = vec![0u8; bytes_per_line as usize * 3];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 3;
            // 0x80 threshold matches the decode round-trip: any input
            // byte ≥ 0x80 sets the bit, anything below clears it. The
            // decoder always emits 0x00 / 0xFF, so this is the cut
            // that round-trips exactly when the source is already in
            // {0x00, 0xFF} per channel.
            for (plane, &b) in rgb[off..off + 3].iter().enumerate() {
                if b >= 0x80 {
                    row[plane * bytes_per_line as usize + x / 8] |= 1 << (7 - (x % 8));
                }
            }
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

#[inline]
fn round_up_to_even(v: u16) -> u16 {
    if v % 2 == 0 {
        v
    } else {
        v + 1
    }
}

/// Default authoring DPI written into the header when the caller does
/// not supply one. 72×72 matches the "screen DPI" convention PC
/// Paintbrush and the rev-5 manual's example header carry; scanner
/// software that emits PCX typically overrides this to 300/300.
pub(crate) const DEFAULT_DPI: (u16, u16) = (72, 72);

/// Default authoring screen size written into the header when the
/// caller does not supply one. `(0, 0)` matches the rev-5 manual's
/// guidance for the `h_screen_size` / `v_screen_size` fields ("Set all
/// bytes to 0" for the filler block applies to these PB IV / IV Plus
/// additions on every writer that pre-dates the PB IV release), which
/// the decoder surfaces as [`PcxImage::screen_size`] = `None`.
pub(crate) const DEFAULT_SCREEN_SIZE: (u16, u16) = (0, 0);

fn write_header(
    out: &mut Vec<u8>,
    width: u16,
    height: u16,
    bits_per_pixel: u8,
    n_planes: u8,
    bytes_per_line: u16,
) {
    write_header_full(
        out,
        0,
        0,
        width,
        height,
        bits_per_pixel,
        n_planes,
        bytes_per_line,
        &[0u8; 48],
        1,
        DEFAULT_DPI,
        DEFAULT_SCREEN_SIZE,
    );
}

fn write_header_with_palette(
    out: &mut Vec<u8>,
    width: u16,
    height: u16,
    bits_per_pixel: u8,
    n_planes: u8,
    bytes_per_line: u16,
    ega_palette: &[u8; 48],
) {
    write_header_full(
        out,
        0,
        0,
        width,
        height,
        bits_per_pixel,
        n_planes,
        bytes_per_line,
        ega_palette,
        1,
        DEFAULT_DPI,
        DEFAULT_SCREEN_SIZE,
    );
}

/// Full-control header writer. Used by the specialised helpers that
/// need to set a non-zero window origin (PCX 3.0+ pixel-region edge
/// case), override `palette_info` (1 = colour / BW, 2 = grayscale —
/// per spec §3 the latter forces the decoder onto a grayscale
/// interpretation regardless of any tail palette), carry a custom
/// authoring DPI (spec §3 records `h_dpi` / `v_dpi` as "the resolutions
/// at which the image was created — e.g. a scan might store 300, 300"),
/// or stamp a PB IV / IV Plus authoring screen size into the header's
/// `h_screen_size` / `v_screen_size` words (spec §3 offsets 70 / 72).
#[allow(clippy::too_many_arguments)]
fn write_header_full(
    out: &mut Vec<u8>,
    x_min: u16,
    y_min: u16,
    width: u16,
    height: u16,
    bits_per_pixel: u8,
    n_planes: u8,
    bytes_per_line: u16,
    ega_palette: &[u8; 48],
    palette_info: u16,
    dpi: (u16, u16),
    screen_size: (u16, u16),
) {
    let start = out.len();
    let x_max = x_min + width - 1;
    let y_max = y_min + height - 1;
    out.push(PCX_MANUFACTURER); // 0
    out.push(5); // version 5 (PCX 5.0) — 1
    out.push(PCX_ENCODING_RLE); // 2
    out.push(bits_per_pixel); // 3
    out.extend_from_slice(&x_min.to_le_bytes()); // x_min  4
    out.extend_from_slice(&y_min.to_le_bytes()); // y_min  6
    out.extend_from_slice(&x_max.to_le_bytes()); // x_max  8
    out.extend_from_slice(&y_max.to_le_bytes()); // y_max 10
    out.extend_from_slice(&dpi.0.to_le_bytes()); // h_dpi  12
    out.extend_from_slice(&dpi.1.to_le_bytes()); // v_dpi  14
    out.extend_from_slice(ega_palette); // ega_palette  16..64
    out.push(0); // reserved 64
    out.push(n_planes); // n_planes 65
    out.extend_from_slice(&bytes_per_line.to_le_bytes()); // bytes_per_line 66
    out.extend_from_slice(&palette_info.to_le_bytes()); // palette_info 68
    out.extend_from_slice(&screen_size.0.to_le_bytes()); // h_screen_size 70
    out.extend_from_slice(&screen_size.1.to_le_bytes()); // v_screen_size 72
    out.extend_from_slice(&[0u8; 54]); // filler 74..128
    debug_assert_eq!(out.len() - start, PCX_HEADER_SIZE);
}

/// Encode `width × height` 1-bit pixels (one byte per pixel, value 0 or
/// 1, row-major, top-down) into a PCX 5.0 monochrome file.
///
/// Bit 1 = white, bit 0 = black per spec §4.1. `bytes_per_line` is the
/// natural ceil(width / 8) rounded up to the next even count.
pub fn encode_pcx_1bpp_mono(width: u16, height: u16, pixels: &[u8]) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if pixels.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: 1bpp input shorter than width × height",
        ));
    }
    let bytes_per_line = round_up_to_even(width.div_ceil(8));
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * height as usize);
    write_header(&mut out, width, height, 1, 1, bytes_per_line);
    let mut row = vec![0u8; bytes_per_line as usize];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let v = pixels[y * width as usize + x];
            if v != 0 {
                row[x / 8] |= 1 << (7 - (x % 8));
            }
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode `width × height` 4-bit-index pixels (one byte per pixel, low
/// nibble = palette index 0..15, row-major, top-down) into a PCX 5.0
/// 4 bpp packed file.
///
/// `palette` is a 16-entry RGB triplet (48 bytes) written into the
/// header `ega_palette` field.
pub fn encode_pcx_4bpp_packed(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: 4bpp input shorter than width × height",
        ));
    }
    if palette.len() != 48 {
        return Err(Error::invalid(format!(
            "PCX encoder: 4bpp palette must be exactly 48 bytes (16 RGB triplets), got {}",
            palette.len()
        )));
    }
    let bytes_per_line = round_up_to_even(width.div_ceil(2));
    let mut ega = [0u8; 48];
    ega.copy_from_slice(palette);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * height as usize);
    write_header_with_palette(&mut out, width, height, 4, 1, bytes_per_line, &ega);
    let mut row = vec![0u8; bytes_per_line as usize];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let v = indices[y * width as usize + x] & 0x0F;
            let byte_off = x / 2;
            if x % 2 == 0 {
                row[byte_off] |= v << 4;
            } else {
                row[byte_off] |= v;
            }
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode `width × height` 2-bit-index pixels (low 2 bits = palette
/// index 0..3, row-major, top-down) into a PCX 5.0 2 bpp CGA file.
///
/// `palette_selector` selects the CGA palette in the header byte 19:
/// * `0x00` → palette 1 high-intensity (cyan/magenta/white) — the
///   default the decoder also assumes for legacy zero-filled headers.
/// * `0x40` → palette 1 low-intensity.
/// * `0x80` → palette 0 high-intensity.
/// * `0xC0` → palette 0 low-intensity.
///
/// `background_index` is the EGA index used for palette entry 0; the
/// high nibble of header byte 16.
pub fn encode_pcx_2bpp_cga(
    width: u16,
    height: u16,
    indices: &[u8],
    palette_selector: u8,
    background_index: u8,
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: 2bpp input shorter than width × height",
        ));
    }
    if background_index > 0x0F {
        return Err(Error::invalid(format!(
            "PCX encoder: CGA background_index must be 0..15, got {background_index}"
        )));
    }
    let bytes_per_line = round_up_to_even(width.div_ceil(4));
    let mut ega = [0u8; 48];
    ega[16] = background_index << 4;
    ega[19] = palette_selector;
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * height as usize);
    write_header_with_palette(&mut out, width, height, 2, 1, bytes_per_line, &ega);
    let mut row = vec![0u8; bytes_per_line as usize];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let v = indices[y * width as usize + x] & 0b11;
            let byte_off = x / 4;
            let pix_in_byte = x % 4;
            let shift = 6 - 2 * pix_in_byte;
            row[byte_off] |= v << shift;
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode `width × height` 2-bit-index pixels (low 2 bits = palette
/// index 0..3, row-major, top-down) into a PCX 5.0 2 bpp CGA file,
/// stamping the full C / P / I selector triple into header byte 19 per
/// the verbatim ZSoft manual ("CGA Color Map").
///
/// This is the spec-faithful sibling of [`encode_pcx_2bpp_cga`]. Where
/// that writer takes a raw two-bit (bits 7 / 6) `palette_selector` byte,
/// this one takes a [`Pcx2bppCgaCpi`] and writes all three significant
/// bits: `C` (bit 7, color burst — set for the monochrome composite-grey
/// ramp), `P` (bit 6, palette family), `I` (bit 5, intensity). The
/// resulting file round-trips through
/// [`crate::parse_pcx_indexed_2bpp_cga_cpi`] with the same C / P / I
/// bits and resolved palette.
///
/// `background_index` is the EGA index used for palette entry 0 (header
/// byte 16 high nibble).
pub fn encode_pcx_2bpp_cga_cpi(
    width: u16,
    height: u16,
    indices: &[u8],
    cpi: Pcx2bppCgaCpi,
    background_index: u8,
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: 2bpp input shorter than width × height",
        ));
    }
    if background_index > 0x0F {
        return Err(Error::invalid(format!(
            "PCX encoder: CGA background_index must be 0..15, got {background_index}"
        )));
    }
    let bytes_per_line = round_up_to_even(width.div_ceil(4));
    let mut ega = [0u8; 48];
    ega[16] = background_index << 4;
    ega[19] = cpi.to_byte19();
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * height as usize);
    write_header_with_palette(&mut out, width, height, 2, 1, bytes_per_line, &ega);
    let mut row = vec![0u8; bytes_per_line as usize];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let v = indices[y * width as usize + x] & 0b11;
            let byte_off = x / 4;
            let pix_in_byte = x % 4;
            let shift = 6 - 2 * pix_in_byte;
            row[byte_off] |= v << shift;
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode `width × height` 2-bit-index pixels (low 2 bits = palette
/// index 0..3, row-major, top-down) into a PCX 5.0 1 bpp × 2-plane CGA
/// file — the plane-oriented CGA layout the EGFF canonical mode matrix
/// lists as `BitsPerPixel = 1, NumBitPlanes = 2`.
///
/// This is the plane-oriented sibling of [`encode_pcx_2bpp_cga`]: where
/// that writer packs four pixels per byte into a single 2 bpp plane,
/// this one writes two 1-bit planes per scanline (plane 0 then plane 1
/// within the row). Bit `k` of each index goes to plane `k`, so palette
/// index `p0 | p1 << 1` round-trips through
/// [`crate::parse_pcx_indexed_1bpp_2planes_cga`] and the canonical
/// [`crate::parse_pcx`] flatten path. The CGA palette is carried in the
/// header exactly as [`encode_pcx_2bpp_cga`] does: `background_index`
/// (`0..=15`) into byte 16's high nibble, `palette_selector` into byte
/// 19 (bits 7/6 = palette family + intensity).
pub fn encode_pcx_1bpp_2planes_cga(
    width: u16,
    height: u16,
    indices: &[u8],
    palette_selector: u8,
    background_index: u8,
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: CGA input shorter than width × height",
        ));
    }
    if background_index > 0x0F {
        return Err(Error::invalid(format!(
            "PCX encoder: CGA background_index must be 0..15, got {background_index}"
        )));
    }
    let bytes_per_line = round_up_to_even(width.div_ceil(8));
    let mut ega = [0u8; 48];
    ega[16] = background_index << 4;
    ega[19] = palette_selector;
    let mut out =
        Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * 2 * height as usize);
    write_header_with_palette(&mut out, width, height, 1, 2, bytes_per_line, &ega);
    let mut row = vec![0u8; bytes_per_line as usize * 2];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let idx = indices[y * width as usize + x] & 0b11;
            for plane in 0..2 {
                let bit = (idx >> plane) & 1;
                if bit != 0 {
                    row[plane * bytes_per_line as usize + x / 8] |= 1 << (7 - (x % 8));
                }
            }
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode `width × height` 4-bit-index pixels (low nibble = palette
/// index 0..15, row-major, top-down) into a PCX 5.0 1 bpp × 4-plane
/// EGA file.
///
/// `palette` is 16 RGB triplets (48 bytes) written into the header
/// `ega_palette` field. Plane 0 carries bit 0, plane 1 bit 1, plane 2
/// bit 2, plane 3 bit 3 of the index — matching the BGR-IRGB layout
/// the decoder reads (spec table §3.1).
pub fn encode_pcx_1bpp_4planes_ega(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: EGA input shorter than width × height",
        ));
    }
    if palette.len() != 48 {
        return Err(Error::invalid(format!(
            "PCX encoder: EGA palette must be exactly 48 bytes (16 RGB triplets), got {}",
            palette.len()
        )));
    }
    let bytes_per_line = round_up_to_even(width.div_ceil(8));
    let mut ega = [0u8; 48];
    ega.copy_from_slice(palette);
    let mut out =
        Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * 4 * height as usize);
    write_header_with_palette(&mut out, width, height, 1, 4, bytes_per_line, &ega);
    let mut row = vec![0u8; bytes_per_line as usize * 4];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let idx = indices[y * width as usize + x] & 0x0F;
            for plane in 0..4 {
                let bit = (idx >> plane) & 1;
                if bit != 0 {
                    row[plane * bytes_per_line as usize + x / 8] |= 1 << (7 - (x % 8));
                }
            }
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode `width × height` 16-bit composite-index pixels into a
/// PCX 5.0 `4 bpp × 4 planes` file.
///
/// This is the one `(bits_per_pixel, n_planes)` slot the EGFF canonical
/// PCX video-mode matrix
/// (`docs/image/pcx/pcx-egff-fileformat-info.html`) does not list as a
/// hardware video mode, but the format is structurally reachable: the
/// cross-reference summary's colour-count formula
/// `MaxNumberOfColors = (1 << (BitsPerPixel * NumBitPlanes))` evaluates
/// to `1 << (4 * 4) = 65536` here. The on-disk layout is the standard
/// plane-oriented PCX form (spec §"Image File (.PCX) Format": "each line
/// of the image is stored by color plane"): each scanline carries plane
/// 0, plane 1, plane 2, plane 3 one after another, each a
/// `bytes_per_line`-byte slice holding 4 bits per pixel (2 pixels/byte,
/// high nibble first — the same packing [`encode_pcx_4bpp_packed`] uses).
///
/// `indices` is one `u16` per pixel, row-major top-down. Nibble `k`
/// (`(idx >> (k * 4)) & 0x0F`) is written to plane `k`, so the
/// [`crate::parse_pcx_indexed_4bpp_4planes`] accessor round-trips the
/// composite index exactly (`p0 | p1 << 4 | p2 << 8 | p3 << 12`).
///
/// No palette is written: the spec defines no 65536-entry palette
/// geometry for this mode, so the header `Colormap` field is left at the
/// zero default and only the composite indices are carried.
pub fn encode_pcx_4bpp_4planes(width: u16, height: u16, indices: &[u16]) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: 4bpp×4planes input shorter than width × height",
        ));
    }
    let bytes_per_line = round_up_to_even(width.div_ceil(2));
    let mut out =
        Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * 4 * height as usize);
    write_header_with_palette(&mut out, width, height, 4, 4, bytes_per_line, &[0u8; 48]);
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

/// Encode an 8 bpp × 1 plane grayscale PCX with `palette_info = 2`
/// (the spec §3 grayscale flag) and no tail palette.
///
/// `pixels` is one byte per pixel (the grayscale intensity, 0..255),
/// row-major, top-down. The decoder honours `palette_info = 2` by
/// emitting `(g, g, g, 0xFF)` per pixel regardless of any tail
/// palette, so this writer omits the 768-byte VGA block and the file
/// stays compact.
pub fn encode_pcx_8bpp_grayscale(width: u16, height: u16, pixels: &[u8]) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if pixels.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: grayscale input shorter than width × height",
        ));
    }
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + bytes_per_line as usize * height as usize);
    write_header_full(
        &mut out,
        0,
        0,
        width,
        height,
        8,
        1,
        bytes_per_line,
        &[0u8; 48],
        2, // palette_info = 2 → grayscale per spec §3
        DEFAULT_DPI,
        DEFAULT_SCREEN_SIZE,
    );
    let mut row = Vec::with_capacity(bytes_per_line as usize);
    for y in 0..height as usize {
        row.clear();
        row.extend_from_slice(&pixels[y * width as usize..y * width as usize + width as usize]);
        row.resize(bytes_per_line as usize, 0);
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode a 24-bit PCX with a non-zero window origin.
///
/// PCX 3.0+ pixel-region semantics put the origin at `(x_min, y_min)`
/// and the bottom-right at `(x_max, y_max)`; per spec §3 the visible
/// `width / height` are computed as `x_max - x_min + 1` and `y_max -
/// y_min + 1`. The standard [`encode_pcx_24bpp`] always writes
/// `(x_min, y_min) = (0, 0)`. Callers that want to mirror an editor's
/// non-zero crop window — e.g. for round-tripping a windowed PCX —
/// use this helper.
///
/// Pixels are top-down packed RGB (`width × height × 3` bytes); the
/// `x_min` / `y_min` values are header metadata only and do NOT shift
/// the pixel buffer.
pub fn encode_pcx_24bpp_window(
    x_min: u16,
    y_min: u16,
    width: u16,
    height: u16,
    rgb: &[u8],
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if rgb.len() < width as usize * height as usize * 3 {
        return Err(Error::invalid(
            "PCX encoder: rgb input shorter than width × height × 3",
        ));
    }
    // u16 overflow on x_max / y_max: x_min + width - 1 must fit in u16.
    if (x_min as u32 + width as u32) > u16::MAX as u32 + 1 {
        return Err(Error::invalid(
            "PCX encoder: x_min + width exceeds u16::MAX + 1",
        ));
    }
    if (y_min as u32 + height as u32) > u16::MAX as u32 + 1 {
        return Err(Error::invalid(
            "PCX encoder: y_min + height exceeds u16::MAX + 1",
        ));
    }
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + rgb.len() / 2);
    write_header_full(
        &mut out,
        x_min,
        y_min,
        width,
        height,
        8,
        3,
        bytes_per_line,
        &[0u8; 48],
        1,
        DEFAULT_DPI,
        DEFAULT_SCREEN_SIZE,
    );
    let mut row = Vec::with_capacity(bytes_per_line as usize * 3);
    for y in 0..height as usize {
        row.clear();
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

/// Wrapper so callers with a [`PcxImage`] don't need to flatten by
/// hand. Picks `encode_pcx_24bpp` for `Rgba` (alpha is dropped) and
/// `Rgb24` inputs; rejects `Indexed8` (which needs an explicit
/// palette argument).
///
/// When `image.dpi` is `Some((h, v))`, the same `(h_dpi, v_dpi)` is
/// threaded into the header so a decode → re-encode pass preserves the
/// authoring resolution metadata from spec §3. When
/// `image.window_origin` is `Some((x, y))`, the same `(x_min, y_min)`
/// is threaded into the header (via [`encode_pcx_24bpp_window`] or its
/// matching DPI-bearing variant [`encode_pcx_24bpp_window_dpi`]) so a
/// decoded windowed PCX round-trips its crop origin instead of having
/// it silently zeroed out. When `image.screen_size` is `Some((h, v))`,
/// the same `(h_screen_size, v_screen_size)` is threaded through the
/// matching `_screen` / `_window_dpi_screen` writer so a tagged PB IV /
/// IV Plus authoring display resolution survives the round-trip as
/// well.
pub fn encode_pcx_24bpp_image(image: &PcxImage) -> Result<Vec<u8>> {
    let w: u16 = image
        .width
        .try_into()
        .map_err(|_| Error::invalid("PCX encoder: width exceeds 65535"))?;
    let h: u16 = image
        .height
        .try_into()
        .map_err(|_| Error::invalid("PCX encoder: height exceeds 65535"))?;
    // Build the packed RGB buffer once so the eight (window_origin ×
    // dpi × screen_size) sub-cases below all share the same input.
    let rgb_owned: Option<Vec<u8>> = match image.pixel_format {
        PcxPixelFormat::Rgba => {
            let mut rgb = Vec::with_capacity(image.data.len() / 4 * 3);
            for c in image.data.chunks_exact(4) {
                rgb.extend_from_slice(&c[..3]);
            }
            Some(rgb)
        }
        PcxPixelFormat::Rgb24 => None,
        PcxPixelFormat::Indexed8 => {
            return Err(Error::unsupported(
                "PCX encoder: Indexed8 input needs explicit palette \
                 (use encode_pcx_8bpp_indexed)",
            ))
        }
    };
    let rgb: &[u8] = rgb_owned.as_deref().unwrap_or(&image.data);
    match (image.window_origin, image.dpi, image.screen_size) {
        // All three present → maximally-tagged writer.
        (Some((x_min, y_min)), Some(dpi), Some(screen)) => {
            encode_pcx_24bpp_window_dpi_screen(x_min, y_min, w, h, rgb, dpi, screen)
        }
        // Screen-only on top of window + dpi → fold the missing axis to
        // the maximally-tagged writer by leaving the other-default
        // sentinel for the absent axis. Since `_screen` is a sentinel
        // ("both non-zero" or absent), the natural composition is to
        // raise the request to the next-more-tagged writer.
        (Some((x_min, y_min)), None, Some(screen)) => {
            // window + screen_size, no DPI override. The simplest path
            // is to call the window writer then patch the screen-size
            // bytes; instead, emit via the combined writer with
            // DEFAULT_DPI so the header stays self-consistent.
            encode_pcx_24bpp_window_dpi_screen(x_min, y_min, w, h, rgb, DEFAULT_DPI, screen)
        }
        (None, Some(dpi), Some(screen)) => {
            // dpi + screen_size, no window. Same idea — combined writer
            // with zero origin.
            encode_pcx_24bpp_window_dpi_screen(0, 0, w, h, rgb, dpi, screen)
        }
        (None, None, Some(screen)) => encode_pcx_24bpp_screen(w, h, rgb, screen),
        // No screen-size → keep the existing dispatch the round-225
        // wrapper used. The output is bit-identical to pre-r231 for
        // every untagged input.
        (Some((x_min, y_min)), Some(dpi), None) => {
            encode_pcx_24bpp_window_dpi(x_min, y_min, w, h, rgb, dpi)
        }
        (Some((x_min, y_min)), None, None) => encode_pcx_24bpp_window(x_min, y_min, w, h, rgb),
        (None, Some(dpi), None) => encode_pcx_24bpp_dpi(w, h, rgb, dpi),
        (None, None, None) => encode_pcx_24bpp(w, h, rgb),
    }
}

// ---------------------------------------------------------------------------
// Public DPI-bearing variants
// ---------------------------------------------------------------------------
//
// Spec §3 records `h_dpi` / `v_dpi` as "the resolutions at which the
// image was created (printer or scanner); e.g. a scan might store
// 300, 300". The plain `encode_pcx_*` writers fix this at the 72×72
// "screen DPI" convention the PC Paintbrush family historically wrote.
// These _dpi variants let a caller round-trip a scanner's authoring
// resolution through decode → re-encode without losing the metadata,
// or stamp a destination DPI that downstream printing software will
// honour. Both fields must be non-zero — per spec §3 a 0 means "unset"
// and a decoder would surface `PcxImage::dpi = None` for such a file.

/// Encode 24-bit packed RGB to PCX 5.0 with a custom authoring DPI.
///
/// Identical to [`encode_pcx_24bpp`] except the header's `h_dpi` /
/// `v_dpi` fields carry the supplied `(h, v)` rather than the default
/// 72×72. `(0, 0)` and any tuple where either component is zero is
/// rejected — spec §3 treats a 0 as "unset" and a decoder will surface
/// the image with [`PcxImage::dpi`] = `None`.
pub fn encode_pcx_24bpp_dpi(
    width: u16,
    height: u16,
    rgb: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if rgb.len() < width as usize * height as usize * 3 {
        return Err(Error::invalid(
            "PCX encoder: rgb input shorter than width × height × 3",
        ));
    }
    check_dpi(dpi)?;
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + rgb.len() / 2);
    write_header_full(
        &mut out,
        0,
        0,
        width,
        height,
        8,
        3,
        bytes_per_line,
        &[0u8; 48],
        1,
        dpi,
        DEFAULT_SCREEN_SIZE,
    );
    let mut row = Vec::with_capacity(bytes_per_line as usize * 3);
    for y in 0..height as usize {
        row.clear();
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

/// Encode an 8 bpp indexed PCX with custom DPI plus a 256-entry VGA
/// tail palette. Mirrors [`encode_pcx_8bpp_indexed`] except for the
/// authoring DPI fields.
pub fn encode_pcx_8bpp_indexed_dpi(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: indexed input shorter than width × height",
        ));
    }
    if palette.len() != PCX_VGA_PALETTE_BYTES {
        return Err(Error::invalid(format!(
            "PCX encoder: 256-colour palette must be exactly {PCX_VGA_PALETTE_BYTES} bytes (got {})",
            palette.len()
        )));
    }
    check_dpi(dpi)?;
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + indices.len() / 2 + PCX_VGA_PALETTE_BYTES);
    write_header_full(
        &mut out,
        0,
        0,
        width,
        height,
        8,
        1,
        bytes_per_line,
        &[0u8; 48],
        1,
        dpi,
        DEFAULT_SCREEN_SIZE,
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

/// Encode an 8 bpp × 1 plane grayscale PCX (spec §3 `palette_info = 2`,
/// no tail palette) with a custom authoring DPI. Mirrors
/// [`encode_pcx_8bpp_grayscale`] except for the DPI fields.
pub fn encode_pcx_8bpp_grayscale_dpi(
    width: u16,
    height: u16,
    pixels: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if pixels.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: grayscale input shorter than width × height",
        ));
    }
    check_dpi(dpi)?;
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + bytes_per_line as usize * height as usize);
    write_header_full(
        &mut out,
        0,
        0,
        width,
        height,
        8,
        1,
        bytes_per_line,
        &[0u8; 48],
        2,
        dpi,
        DEFAULT_SCREEN_SIZE,
    );
    let mut row = Vec::with_capacity(bytes_per_line as usize);
    for y in 0..height as usize {
        row.clear();
        row.extend_from_slice(&pixels[y * width as usize..y * width as usize + width as usize]);
        row.resize(bytes_per_line as usize, 0);
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode a 1 bpp × 1 plane monochrome PCX with a custom authoring DPI.
/// Mirrors [`encode_pcx_1bpp_mono`] except for the DPI fields. Bit 1 =
/// white, bit 0 = black per spec §4.1.
pub fn encode_pcx_1bpp_mono_dpi(
    width: u16,
    height: u16,
    pixels: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if pixels.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: 1bpp input shorter than width × height",
        ));
    }
    check_dpi(dpi)?;
    let bytes_per_line = round_up_to_even(width.div_ceil(8));
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * height as usize);
    write_header_full(
        &mut out,
        0,
        0,
        width,
        height,
        1,
        1,
        bytes_per_line,
        &[0u8; 48],
        1,
        dpi,
        DEFAULT_SCREEN_SIZE,
    );
    let mut row = vec![0u8; bytes_per_line as usize];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let v = pixels[y * width as usize + x];
            if v != 0 {
                row[x / 8] |= 1 << (7 - (x % 8));
            }
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode a 4 bpp × 1 plane 16-colour packed-bits PCX with a custom
/// authoring DPI. Mirrors [`encode_pcx_4bpp_packed`] except the header
/// `h_dpi` / `v_dpi` words (spec §3 offsets 12 / 14 — "the resolutions
/// at which the image was created (printer or scanner)") carry the
/// caller's pair instead of the historical 72×72 default. The DPI field
/// is a format-independent header word per spec §3, so a 16-colour image
/// scanned at e.g. 300 × 300 is just as valid as a 24-bit one.
pub fn encode_pcx_4bpp_packed_dpi(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: 4bpp input shorter than width × height",
        ));
    }
    if palette.len() != 48 {
        return Err(Error::invalid(format!(
            "PCX encoder: 4bpp palette must be exactly 48 bytes (16 RGB triplets), got {}",
            palette.len()
        )));
    }
    check_dpi(dpi)?;
    let bytes_per_line = round_up_to_even(width.div_ceil(2));
    let mut ega = [0u8; 48];
    ega.copy_from_slice(palette);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * height as usize);
    write_header_full(
        &mut out,
        0,
        0,
        width,
        height,
        4,
        1,
        bytes_per_line,
        &ega,
        1,
        dpi,
        DEFAULT_SCREEN_SIZE,
    );
    let mut row = vec![0u8; bytes_per_line as usize];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let v = indices[y * width as usize + x] & 0x0F;
            let byte_off = x / 2;
            if x % 2 == 0 {
                row[byte_off] |= v << 4;
            } else {
                row[byte_off] |= v;
            }
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode a 2 bpp × 1 plane 4-colour CGA PCX with a custom authoring
/// DPI. Mirrors [`encode_pcx_2bpp_cga`] except the header `h_dpi` /
/// `v_dpi` words (spec §3 offsets 12 / 14) carry the caller's pair
/// instead of the historical 72×72 default. The CGA palette selector
/// (header byte 19) and background index (header byte 16 high nibble)
/// are stamped exactly as the non-DPI writer does.
pub fn encode_pcx_2bpp_cga_dpi(
    width: u16,
    height: u16,
    indices: &[u8],
    palette_selector: u8,
    background_index: u8,
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: 2bpp input shorter than width × height",
        ));
    }
    if background_index > 0x0F {
        return Err(Error::invalid(format!(
            "PCX encoder: CGA background_index must be 0..15, got {background_index}"
        )));
    }
    check_dpi(dpi)?;
    let bytes_per_line = round_up_to_even(width.div_ceil(4));
    let mut ega = [0u8; 48];
    ega[16] = background_index << 4;
    ega[19] = palette_selector;
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * height as usize);
    write_header_full(
        &mut out,
        0,
        0,
        width,
        height,
        2,
        1,
        bytes_per_line,
        &ega,
        1,
        dpi,
        DEFAULT_SCREEN_SIZE,
    );
    let mut row = vec![0u8; bytes_per_line as usize];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let v = indices[y * width as usize + x] & 0b11;
            let byte_off = x / 4;
            let pix_in_byte = x % 4;
            let shift = 6 - 2 * pix_in_byte;
            row[byte_off] |= v << shift;
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode a 4-colour CGA PCX at 1 bpp × 2 planes with a custom
/// authoring DPI. Mirrors [`encode_pcx_1bpp_2planes_cga`] except the
/// header `h_dpi` / `v_dpi` words (spec §3 offsets 12 / 14) carry the
/// caller's pair instead of the historical 72×72 default.
pub fn encode_pcx_1bpp_2planes_cga_dpi(
    width: u16,
    height: u16,
    indices: &[u8],
    palette_selector: u8,
    background_index: u8,
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: CGA input shorter than width × height",
        ));
    }
    if background_index > 0x0F {
        return Err(Error::invalid(format!(
            "PCX encoder: CGA background_index must be 0..15, got {background_index}"
        )));
    }
    check_dpi(dpi)?;
    let bytes_per_line = round_up_to_even(width.div_ceil(8));
    let mut ega = [0u8; 48];
    ega[16] = background_index << 4;
    ega[19] = palette_selector;
    let mut out =
        Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * 2 * height as usize);
    write_header_full(
        &mut out,
        0,
        0,
        width,
        height,
        1,
        2,
        bytes_per_line,
        &ega,
        1,
        dpi,
        DEFAULT_SCREEN_SIZE,
    );
    let mut row = vec![0u8; bytes_per_line as usize * 2];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let idx = indices[y * width as usize + x] & 0b11;
            for plane in 0..2 {
                let bit = (idx >> plane) & 1;
                if bit != 0 {
                    row[plane * bytes_per_line as usize + x / 8] |= 1 << (7 - (x % 8));
                }
            }
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode an 8-colour EGA RGB PCX at 1 bpp × 3 planes with a custom
/// authoring DPI. Mirrors [`encode_pcx_1bpp_3planes_ega_rgb`] except the
/// header `h_dpi` / `v_dpi` words (spec §3 offsets 12 / 14) carry the
/// caller's pair instead of the historical 72×72 default. Plane order is
/// R, G, B (spec §4) and each input channel is thresholded at 0x80.
pub fn encode_pcx_1bpp_3planes_ega_rgb_dpi(
    width: u16,
    height: u16,
    rgb: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if rgb.len() < width as usize * height as usize * 3 {
        return Err(Error::invalid(
            "PCX encoder: rgb input shorter than width × height × 3",
        ));
    }
    check_dpi(dpi)?;
    let bytes_per_line = round_up_to_even(width.div_ceil(8));
    let mut out =
        Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * 3 * height as usize);
    write_header_full(
        &mut out,
        0,
        0,
        width,
        height,
        1,
        3,
        bytes_per_line,
        &[0u8; 48],
        1,
        dpi,
        DEFAULT_SCREEN_SIZE,
    );
    let mut row = vec![0u8; bytes_per_line as usize * 3];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 3;
            for (plane, &b) in rgb[off..off + 3].iter().enumerate() {
                if b >= 0x80 {
                    row[plane * bytes_per_line as usize + x / 8] |= 1 << (7 - (x % 8));
                }
            }
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode a 16-colour EGA PCX at 1 bpp × 4 planes with a custom
/// authoring DPI. Mirrors [`encode_pcx_1bpp_4planes_ega`] except the
/// header `h_dpi` / `v_dpi` words (spec §3 offsets 12 / 14) carry the
/// caller's pair instead of the historical 72×72 default. The 16-entry
/// palette is stamped into the header `ega_palette` field exactly as the
/// non-DPI writer does.
pub fn encode_pcx_1bpp_4planes_ega_dpi(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if indices.len() < width as usize * height as usize {
        return Err(Error::invalid(
            "PCX encoder: EGA input shorter than width × height",
        ));
    }
    if palette.len() != 48 {
        return Err(Error::invalid(format!(
            "PCX encoder: EGA palette must be exactly 48 bytes (16 RGB triplets), got {}",
            palette.len()
        )));
    }
    check_dpi(dpi)?;
    let bytes_per_line = round_up_to_even(width.div_ceil(8));
    let mut ega = [0u8; 48];
    ega.copy_from_slice(palette);
    let mut out =
        Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * 4 * height as usize);
    write_header_full(
        &mut out,
        0,
        0,
        width,
        height,
        1,
        4,
        bytes_per_line,
        &ega,
        1,
        dpi,
        DEFAULT_SCREEN_SIZE,
    );
    let mut row = vec![0u8; bytes_per_line as usize * 4];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        for x in 0..width as usize {
            let idx = indices[y * width as usize + x] & 0x0F;
            for plane in 0..4 {
                let bit = (idx >> plane) & 1;
                if bit != 0 {
                    row[plane * bytes_per_line as usize + x / 8] |= 1 << (7 - (x % 8));
                }
            }
        }
        rle::encode(&row, &mut out);
    }
    Ok(out)
}

/// Encode 24-bit packed RGB to PCX 5.0 with both a non-zero window
/// origin AND a custom authoring DPI in one call. Mirrors the
/// combination of [`encode_pcx_24bpp_window`] (window origin from spec
/// §3) and [`encode_pcx_24bpp_dpi`] (authoring DPI from spec §3); used
/// by [`encode_pcx_24bpp_image`] when the decoded source carried both
/// metadata fields so a round-trip preserves them together.
pub fn encode_pcx_24bpp_window_dpi(
    x_min: u16,
    y_min: u16,
    width: u16,
    height: u16,
    rgb: &[u8],
    dpi: (u16, u16),
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if rgb.len() < width as usize * height as usize * 3 {
        return Err(Error::invalid(
            "PCX encoder: rgb input shorter than width × height × 3",
        ));
    }
    if (x_min as u32 + width as u32) > u16::MAX as u32 + 1 {
        return Err(Error::invalid(
            "PCX encoder: x_min + width exceeds u16::MAX + 1",
        ));
    }
    if (y_min as u32 + height as u32) > u16::MAX as u32 + 1 {
        return Err(Error::invalid(
            "PCX encoder: y_min + height exceeds u16::MAX + 1",
        ));
    }
    check_dpi(dpi)?;
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + rgb.len() / 2);
    write_header_full(
        &mut out,
        x_min,
        y_min,
        width,
        height,
        8,
        3,
        bytes_per_line,
        &[0u8; 48],
        1,
        dpi,
        DEFAULT_SCREEN_SIZE,
    );
    let mut row = Vec::with_capacity(bytes_per_line as usize * 3);
    for y in 0..height as usize {
        row.clear();
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

// ---------------------------------------------------------------------------
// Public screen-size-bearing variants
// ---------------------------------------------------------------------------
//
// Spec §3 records `h_screen_size` / `v_screen_size` at offsets 70 / 72
// as "Horizontal / Vertical screen size in pixels (new field found
// only in PB IV / IV Plus)". These fields annotate the display
// resolution the image was authored for and are distinct from the
// printer / scanner DPI carried in `h_dpi` / `v_dpi`. Pre-PB-IV
// writers leave the fields at zero, which the decoder surfaces as
// [`PcxImage::screen_size`] = `None`; the variants below let a caller
// stamp a non-zero pair so a tagged authoring screen size survives
// decode + re-encode. Both components must be non-zero — an
// asymmetric (0, 600) header would not be a meaningful annotation
// and is rejected at the writer boundary.

/// Encode 24-bit packed RGB to PCX 5.0 with a custom authoring screen
/// size in the header.
///
/// Identical to [`encode_pcx_24bpp`] except the header's
/// `h_screen_size` / `v_screen_size` fields (spec §3 offsets 70 / 72)
/// carry the supplied `(h, v)` rather than the default `(0, 0)`. A
/// tuple with either component zero is rejected — the decoder surfaces
/// such a header with [`PcxImage::screen_size`] = `None`, matching the
/// spec §3 "unset" semantic, so emitting it would be redundant.
pub fn encode_pcx_24bpp_screen(
    width: u16,
    height: u16,
    rgb: &[u8],
    screen_size: (u16, u16),
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if rgb.len() < width as usize * height as usize * 3 {
        return Err(Error::invalid(
            "PCX encoder: rgb input shorter than width × height × 3",
        ));
    }
    check_screen_size(screen_size)?;
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + rgb.len() / 2);
    write_header_full(
        &mut out,
        0,
        0,
        width,
        height,
        8,
        3,
        bytes_per_line,
        &[0u8; 48],
        1,
        DEFAULT_DPI,
        screen_size,
    );
    let mut row = Vec::with_capacity(bytes_per_line as usize * 3);
    for y in 0..height as usize {
        row.clear();
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

/// Encode 24-bit packed RGB to PCX 5.0 with a non-zero window origin,
/// custom authoring DPI, and custom authoring screen size — all three
/// in one call.
///
/// Mirrors the union of [`encode_pcx_24bpp_window`] /
/// [`encode_pcx_24bpp_dpi`] / [`encode_pcx_24bpp_screen`]. Used by
/// [`encode_pcx_24bpp_image`] when the decoded source carried every
/// metadata field so a round-trip preserves them together. The DPI
/// tuple and the screen-size tuple are both validated against the
/// "both components non-zero" sentinel rule (spec §3).
pub fn encode_pcx_24bpp_window_dpi_screen(
    x_min: u16,
    y_min: u16,
    width: u16,
    height: u16,
    rgb: &[u8],
    dpi: (u16, u16),
    screen_size: (u16, u16),
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    if rgb.len() < width as usize * height as usize * 3 {
        return Err(Error::invalid(
            "PCX encoder: rgb input shorter than width × height × 3",
        ));
    }
    if (x_min as u32 + width as u32) > u16::MAX as u32 + 1 {
        return Err(Error::invalid(
            "PCX encoder: x_min + width exceeds u16::MAX + 1",
        ));
    }
    if (y_min as u32 + height as u32) > u16::MAX as u32 + 1 {
        return Err(Error::invalid(
            "PCX encoder: y_min + height exceeds u16::MAX + 1",
        ));
    }
    check_dpi(dpi)?;
    check_screen_size(screen_size)?;
    let bytes_per_line = round_up_to_even(width);
    let mut out = Vec::with_capacity(PCX_HEADER_SIZE + rgb.len() / 2);
    write_header_full(
        &mut out,
        x_min,
        y_min,
        width,
        height,
        8,
        3,
        bytes_per_line,
        &[0u8; 48],
        1,
        dpi,
        screen_size,
    );
    let mut row = Vec::with_capacity(bytes_per_line as usize * 3);
    for y in 0..height as usize {
        row.clear();
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
