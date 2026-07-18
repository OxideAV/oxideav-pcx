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
//! accepts video frames in eight [`oxideav_core::PixelFormat`]
//! variants: `Rgba` / `Rgb24` / `Bgr24` / `Bgra` route to
//! [`encode_pcx_24bpp`] (`Bgr*` per-pixel byte-swapped to RGB,
//! alpha dropped from `Rgba` / `Bgra`); `Gray8` routes to
//! [`encode_pcx_8bpp_grayscale`] (8 bpp × 1 plane, `palette_info =
//! 2`, no VGA tail palette per spec §3); `MonoBlack` / `MonoWhite`
//! unpack the MSB-first 1-bit stride into one byte per pixel and
//! route to [`encode_pcx_1bpp_mono`] (with `MonoWhite` bit-inverted
//! so the on-disk PCX retains the spec §4.1 bit-1 = white polarity);
//! `Pal8` reads the caller's colour table off the `VideoFrame`
//! palette side-channel (trailing stride-0 plane) and routes to
//! [`encode_pcx_indexed_auto`], which stores that table verbatim in
//! the smallest applicable geometry (16-entry header colormap at
//! ≤ 16 entries, 768-byte VGA tail otherwise).
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
            // Palette-indexed input: one index byte per pixel with the
            // colour table riding the `VideoFrame` palette side-channel
            // (trailing stride-0 plane, packed 3-byte RGB entries). The
            // caller's palette is stored verbatim in the smallest PCX
            // geometry that can carry it — see `encode_pcx_indexed_auto`
            // for the rung ladder (16-entry header colormap vs 768-byte
            // VGA tail) and the round-trip contract.
            PixelFormat::Pal8 => {
                let pal = vf.palette().ok_or_else(|| {
                    oxideav_core::Error::invalid(
                        "PCX encoder: Pal8 frame carries no palette side-channel \
                         (attach one via VideoFrame::set_palette)",
                    )
                })?;
                let idx_plane = vf.image_planes().first().ok_or_else(|| {
                    oxideav_core::Error::invalid("PCX encoder: Pal8 frame has no index plane")
                })?;
                let tight = tighten_packed(idx_plane, width as usize, height as usize, 1)?;
                let (bytes, _mode) = encode_pcx_indexed_auto(w16, h16, &tight, pal)?;
                bytes
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

/// The PCX 5.0 mode [`encode_pcx_rgb_auto`] selected for a given RGB
/// input, returned alongside the encoded bytes so a caller can record
/// or assert which on-disk geometry was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcxAutoMode {
    /// `≤ 256` distinct colours: 8 bpp × 1 plane indexed image plus a
    /// 256-entry VGA tail palette (spec §"VGA 256-color palette"). The
    /// `usize` is the number of distinct colours found (`1..=256`),
    /// which equals the count of meaningful palette entries (the
    /// remainder are zero-padded).
    Indexed8 { colors: usize },
    /// `> 256` distinct colours: 8 bpp × 3 plane planar RGB (spec
    /// §"24-bit .PCX files"), no tail palette.
    Rgb24,
    /// Every distinct colour is a pure grey (`r == g == b`): 8 bpp ×
    /// 1 plane with the spec §3 `palette_info = 2` grayscale flag and
    /// **no** VGA tail palette. The pixel byte *is* the grey level, so
    /// the decode ramp (`index → (i, i, i)`) reproduces the input
    /// exactly while saving the fixed 769-byte tail the
    /// [`PcxAutoMode::Indexed8`] form would carry.
    Gray8,
    /// Every distinct colour is pure black or pure white: 1 bpp ×
    /// 1 plane monochrome (spec §4.1, bit 1 = white / bit 0 = black) —
    /// one bit per pixel, the smallest geometry PCX defines.
    Mono1,
    /// Every distinct colour has each channel at `0x00` or `0xFF` (the
    /// eight EGA RGB primaries): 1 bpp × 3 planes (spec §4 bit-plane
    /// example — one bit-plane per primary, plane order R, G, B). No
    /// palette is stored anywhere; three bits per pixel on disk.
    EgaRgb1x3,
    /// `≤ 16` distinct colours: 4 bpp × 1 plane packed nibbles with the
    /// exact palette carried in the 48-byte header `Colormap` field
    /// (spec §3) — four bits per pixel and **no** VGA tail. The `usize`
    /// is the number of distinct colours found (`1..=16`).
    Indexed4 { colors: usize },
    /// `≤ 16` distinct colours in the plane-oriented sibling geometry:
    /// 1 bpp × 4 planes (spec table §3.1 bit-plane layout, plane `k`
    /// carries index bit `k`), same 48-byte header palette, same four
    /// bits per pixel — but RLE sees each bit-plane as its own byte
    /// run, which can compress differently from packed nibbles. The
    /// `usize` is the number of distinct colours found (`1..=16`).
    Indexed1x4 { colors: usize },
    /// `≤ 4` distinct colours, ALL of which are exactly representable
    /// by one of the fixed CGA hardware palettes (spec §"CGA Color
    /// Map": header byte 19 bits 7/6 select the palette family +
    /// intensity, header byte 16's high nibble picks the background
    /// colour from the 16 EGA entries): 2 bpp × 1 plane packed bits —
    /// two bits per pixel, no stored colour data beyond the two header
    /// bytes. `palette_selector` / `background_index` record the header
    /// encoding the match search chose.
    Cga2x1 {
        palette_selector: u8,
        background_index: u8,
    },
    /// The same CGA-representability precondition in the
    /// plane-oriented 1 bpp × 2 plane layout (the EGFF canonical CGA
    /// mode matrix's `BitsPerPixel = 1, NumBitPlanes = 2` row). Same
    /// two bits per pixel; different RLE behaviour, so both CGA forms
    /// are tried and the byte count decides.
    Cga1x2 {
        palette_selector: u8,
        background_index: u8,
    },
}

/// Encode `width × height` packed RGB bytes (3 bytes per pixel,
/// row-major, top-down) into the **most compact** valid PCX 5.0 file.
///
/// PC Paintbrush and the era's editors picked the smallest on-disk
/// geometry that could represent the image losslessly. This writer
/// mirrors that: it scans the input's distinct colours and
///
/// * if there are `> 256` of them, emits the 8 bpp × 3 plane planar
///   24-bit form (spec §"24-bit .PCX files", identical bytes to
///   [`encode_pcx_24bpp`]) — the only lossless option, since no PCX
///   palette holds more than 256 entries;
/// * if there are `≤ 256`, it builds every **applicable** compact
///   candidate from the crate's existing spec modes, encodes each, and
///   returns whichever is the **fewest bytes**:
///
///   * **Mono1** — every distinct colour is pure black or pure white:
///     1 bpp × 1 plane monochrome (spec §4.1), one bit per pixel — the
///     smallest geometry PCX defines.
///   * **Cga2x1 / Cga1x2** — `≤ 4` distinct colours all exactly
///     representable by a fixed CGA hardware palette (spec §"CGA Color
///     Map"; entry 0 = any of the 16 EGA colours via header byte 16):
///     2 bpp × 1 plane packed bits and its plane-oriented 1 bpp × 2
///     plane sibling — two bits per pixel, both tried.
///   * **EgaRgb1x3** — every channel of every colour is `0x00` or
///     `0xFF` (the eight EGA RGB primaries): 1 bpp × 3 planes (spec §4
///     bit-plane example), three bits per pixel, no stored palette.
///   * **Indexed4** — `≤ 16` distinct colours: 4 bpp × 1 plane packed
///     nibbles with the exact palette in the 48-byte header `Colormap`
///     (spec §3), no VGA tail.
///   * **Indexed1x4** — the same `≤ 16`-colour precondition in the
///     plane-oriented 1 bpp × 4 plane geometry (spec table §3.1);
///     identical bits per pixel but different RLE behaviour, so both
///     forms are tried and the byte count decides.
///   * **Gray8** — every distinct colour is a pure grey (`r == g ==
///     b`): 8 bpp × 1 plane with `palette_info = 2` (spec §3), no VGA
///     tail. The pixel byte is the grey level, so this drops the fixed
///     769-byte tail the indexed form would carry.
///   * **Indexed8** — one index byte per pixel plus a 768-byte VGA
///     tail palette (spec §"VGA 256-color palette"). Always
///     applicable at `≤ 256` colours.
///   * **Rgb24** — the planar 24-bit form, always applicable. For a
///     *tiny* image the fixed 769-byte palette tail can exceed the
///     whole planar file, so this candidate keeps the "most compact"
///     guarantee at every size, not just the large-image asymptote.
///
/// The returned [`PcxAutoMode`] records which on-disk geometry was
/// actually emitted. Every candidate is exact (no quantisation
/// anywhere: each candidate's palette / pixel derivation reproduces
/// the source colours bit-for-bit), so the file decodes through
/// [`crate::parse_pcx`] back to the original packed RGB regardless of
/// branch. Palette entry order is first-seen (a deterministic raster
/// scan) and the size tie-break is deterministic — candidates are
/// compared in the fixed order listed above and an earlier candidate
/// keeps an exact tie (in particular the r376 "indexed wins an exact
/// tie against planar" contract is preserved) — so the same input
/// always yields byte-identical output.
///
/// This is a pure encode-time optimisation built entirely from
/// existing spec modes; it introduces no new on-disk geometry.
pub fn encode_pcx_rgb_auto(width: u16, height: u16, rgb: &[u8]) -> Result<(Vec<u8>, PcxAutoMode)> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    let n_pixels = width as usize * height as usize;
    if rgb.len() < n_pixels * 3 {
        return Err(Error::invalid(
            "PCX encoder: rgb input shorter than width × height × 3",
        ));
    }
    // Single raster scan ([`build_indexed_payload`]): assign each
    // distinct colour a first-seen index, returning `None` the moment a
    // 257th colour appears. With `> 256` colours the planar 24-bit form
    // is the only lossless option.
    let Some((indices, palette)) = build_indexed_payload(width, height, rgb) else {
        let bytes = encode_pcx_24bpp(width, height, rgb)?;
        return Ok((bytes, PcxAutoMode::Rgb24));
    };
    // `colors` = the count of meaningful (non-padding) palette entries:
    // the distinct-colour total. The palette zero-fills the rest, which
    // the index buffer never references.
    let colors = indices
        .iter()
        .copied()
        .max()
        .map(|m| m as usize + 1)
        .unwrap_or(0);
    // Candidate ladder, in fixed preference order (earlier candidate
    // keeps an exact size tie). Each candidate is only encoded when its
    // losslessness precondition holds, so every entry in `candidates`
    // round-trips exactly by construction.
    let mut candidates: Vec<(Vec<u8>, PcxAutoMode)> = Vec::new();
    if let Some(mono) = auto_mono_pixels(&indices, &palette, colors) {
        candidates.push((
            encode_pcx_1bpp_mono(width, height, &mono)?,
            PcxAutoMode::Mono1,
        ));
    }
    if let Some((palette_selector, background_index, lut)) = auto_cga_match(&palette, colors) {
        let cga_indices: Vec<u8> = indices.iter().map(|&i| lut[i as usize]).collect();
        candidates.push((
            encode_pcx_2bpp_cga(
                width,
                height,
                &cga_indices,
                palette_selector,
                background_index,
            )?,
            PcxAutoMode::Cga2x1 {
                palette_selector,
                background_index,
            },
        ));
        candidates.push((
            encode_pcx_1bpp_2planes_cga(
                width,
                height,
                &cga_indices,
                palette_selector,
                background_index,
            )?,
            PcxAutoMode::Cga1x2 {
                palette_selector,
                background_index,
            },
        ));
    }
    if auto_is_ega_primaries(&palette, colors) {
        candidates.push((
            encode_pcx_1bpp_3planes_ega_rgb(width, height, rgb)?,
            PcxAutoMode::EgaRgb1x3,
        ));
    }
    if let Some(pal48) = auto_palette48(&palette, colors) {
        candidates.push((
            encode_pcx_4bpp_packed(width, height, &indices, &pal48)?,
            PcxAutoMode::Indexed4 { colors },
        ));
        candidates.push((
            encode_pcx_1bpp_4planes_ega(width, height, &indices, &pal48)?,
            PcxAutoMode::Indexed1x4 { colors },
        ));
    }
    if let Some(gray) = auto_gray_pixels(&indices, &palette, colors) {
        candidates.push((
            encode_pcx_8bpp_grayscale(width, height, &gray)?,
            PcxAutoMode::Gray8,
        ));
    }
    candidates.push((
        encode_pcx_8bpp_indexed(width, height, &indices, &palette)?,
        PcxAutoMode::Indexed8 { colors },
    ));
    candidates.push((encode_pcx_24bpp(width, height, rgb)?, PcxAutoMode::Rgb24));
    Ok(pick_smallest_candidate(candidates))
}

/// Encode `width × height` palette indices (one byte per pixel,
/// row-major, top-down) plus a **caller-supplied** packed-RGB palette
/// into the most compact PCX 5.0 geometry that stores that palette
/// *verbatim*.
///
/// This is the caller-palette sibling of [`encode_pcx_rgb_auto`]: where
/// the RGB auto writer derives a first-seen palette from the pixels,
/// this writer treats the palette as caller-owned data — entry order,
/// entry values, and the index → entry association are preserved
/// exactly, never re-derived, re-ordered, or quantised. `palette` is
/// packed 3-byte RGB entries (entry `i` at bytes `3*i .. 3*i + 3`),
/// non-empty, a multiple of 3, and at most [`PCX_VGA_PALETTE_BYTES`]
/// (768) bytes long — the same layout the `oxideav_core::VideoFrame`
/// palette side-channel carries, which is how the framework `Encoder`'s
/// `Pal8` path reaches this function.
///
/// Only the spec geometries that carry a stored palette **verbatim**
/// are candidates. The CGA modes store a palette *family selector*
/// (manual §"CGA Color Map"), the monochrome and EGA-RGB modes store no
/// arbitrary palette at all — routing through any of those would lose
/// the caller's table, so unlike [`encode_pcx_rgb_auto`] they are never
/// tried here:
///
/// * **Indexed4 / Indexed1x4** — the two 16-entry header `Colormap`
///   rungs (spec §3; packed nibbles and the plane-oriented spec table
///   §3.1 sibling). Applicable when the caller's table has ≤ 16
///   entries, every index is ≤ 15 (a larger index cannot be stored in
///   4 bits), and at least one palette byte is non-zero — an all-zero
///   16-entry colormap is indistinguishable from the "unset" header a
///   PCX 3.0+ writer emits, which readers (including this crate's
///   [`crate::parse_pcx_indexed_4bpp`]) resolve to the spec table §3.1
///   hardware default; the VGA-tail rung below has no such sentinel
///   collision, so all-black tables route there and stay byte-exact.
///   Colormap entries beyond the caller's count are zero-padded.
/// * **Indexed8** — 8 bpp × 1 plane plus the 768-byte VGA tail (spec
///   §"VGA 256-color palette"), always applicable; the caller's
///   entries are written first and the remainder of the 256-entry
///   block is zero-padded.
///
/// Every applicable candidate is encoded and the fewest-byte file wins;
/// exact size ties keep the earlier candidate in the fixed order above
/// (Indexed4, Indexed1x4, Indexed8), so identical input always yields
/// byte-identical output — the same determinism contract as
/// [`encode_pcx_rgb_auto`]. The returned [`PcxAutoMode`] reports the
/// chosen geometry with `colors` = the caller's entry count.
///
/// Round-trip contract: decoding the produced file through the typed
/// accessor matching the reported mode
/// ([`crate::parse_pcx_indexed_4bpp`] /
/// [`crate::parse_pcx_indexed_1bpp_4planes`] /
/// [`crate::parse_pcx_indexed_8bpp`]) returns the caller's indices
/// byte-exactly and a palette whose first `palette.len()` bytes equal
/// the caller's table (the tail of the fixed-size on-disk table is the
/// zero padding). Indices at or beyond the caller's entry count are
/// accepted — they resolve to the zero padding (black), matching the
/// missing-entry policy the `VideoFrame` palette side-channel
/// documents.
pub fn encode_pcx_indexed_auto(
    width: u16,
    height: u16,
    indices: &[u8],
    palette: &[u8],
) -> Result<(Vec<u8>, PcxAutoMode)> {
    if width == 0 || height == 0 {
        return Err(Error::invalid("PCX encoder: zero dimension"));
    }
    let n_pixels = width as usize * height as usize;
    if indices.len() < n_pixels {
        return Err(Error::invalid(
            "PCX encoder: indexed input shorter than width × height",
        ));
    }
    if palette.is_empty() || palette.len() % 3 != 0 || palette.len() > PCX_VGA_PALETTE_BYTES {
        return Err(Error::invalid(format!(
            "PCX encoder: caller palette must be packed RGB triplets — non-empty, a multiple \
             of 3 bytes, at most {PCX_VGA_PALETTE_BYTES} (got {})",
            palette.len()
        )));
    }
    let colors = palette.len() / 3;
    let mut candidates: Vec<(Vec<u8>, PcxAutoMode)> = Vec::new();
    // The 16-entry header-colormap rungs: see the doc comment for the
    // three-part precondition (entry count, index width, and the
    // all-zero-colormap sentinel collision that forces all-black tables
    // onto the VGA-tail rung).
    let header_rung_ok = colors <= 16
        && indices[..n_pixels].iter().all(|&i| i <= 0x0F)
        && palette.iter().any(|&b| b != 0);
    if header_rung_ok {
        let mut pal48 = [0u8; 48];
        pal48[..palette.len()].copy_from_slice(palette);
        candidates.push((
            encode_pcx_4bpp_packed(width, height, indices, &pal48)?,
            PcxAutoMode::Indexed4 { colors },
        ));
        candidates.push((
            encode_pcx_1bpp_4planes_ega(width, height, indices, &pal48)?,
            PcxAutoMode::Indexed1x4 { colors },
        ));
    }
    let mut pal768 = vec![0u8; PCX_VGA_PALETTE_BYTES];
    pal768[..palette.len()].copy_from_slice(palette);
    candidates.push((
        encode_pcx_8bpp_indexed(width, height, indices, &pal768)?,
        PcxAutoMode::Indexed8 { colors },
    ));
    Ok(pick_smallest_candidate(candidates))
}

/// Reduce a non-empty candidate ladder to the smallest encoding,
/// resolving exact size ties in favour of the earlier (more-preferred)
/// candidate so the auto writers stay deterministic.
fn pick_smallest_candidate(candidates: Vec<(Vec<u8>, PcxAutoMode)>) -> (Vec<u8>, PcxAutoMode) {
    let mut it = candidates.into_iter();
    let mut best = it.next().expect("candidate ladder is never empty");
    for cand in it {
        if cand.0.len() < best.0.len() {
            best = cand;
        }
    }
    best
}

/// Derive the one-byte-per-pixel grey buffer for the [`PcxAutoMode::Gray8`]
/// candidate, or `None` when any meaningful palette entry is not a pure
/// grey (`r == g == b`).
///
/// Works from the first-seen index buffer + palette that
/// [`build_indexed_payload`] already produced, so no second scan of the
/// RGB input is needed: a 256-entry index→grey LUT is built from the
/// `colors` meaningful palette entries and applied per pixel. The
/// grayscale decode path (`palette_info = 2`, spec §3) maps pixel byte
/// `g` to `(g, g, g)`, so using the grey level itself as the pixel byte
/// round-trips exactly.
fn auto_gray_pixels(indices: &[u8], palette: &[u8], colors: usize) -> Option<Vec<u8>> {
    let mut lut = [0u8; 256];
    for (i, slot) in lut.iter_mut().enumerate().take(colors) {
        let (r, g, b) = (palette[i * 3], palette[i * 3 + 1], palette[i * 3 + 2]);
        if r != g || g != b {
            return None;
        }
        *slot = r;
    }
    Some(indices.iter().map(|&i| lut[i as usize]).collect())
}

/// Derive the one-byte-per-pixel bilevel buffer (0 = black, 1 = white)
/// for the [`PcxAutoMode::Mono1`] candidate, or `None` when any
/// meaningful palette entry is neither pure black nor pure white.
///
/// The monochrome decode path (spec §4.1) maps bit 1 → white
/// (`0xFF, 0xFF, 0xFF`) and bit 0 → black (`0x00, 0x00, 0x00`), so the
/// candidate is exact precisely when those two colours are the whole
/// palette.
fn auto_mono_pixels(indices: &[u8], palette: &[u8], colors: usize) -> Option<Vec<u8>> {
    let mut lut = [0u8; 256];
    for (i, slot) in lut.iter_mut().enumerate().take(colors) {
        let entry = &palette[i * 3..i * 3 + 3];
        *slot = match entry {
            [0x00, 0x00, 0x00] => 0,
            [0xFF, 0xFF, 0xFF] => 1,
            _ => return None,
        };
    }
    Some(indices.iter().map(|&i| lut[i as usize]).collect())
}

/// Whether every meaningful palette entry has each channel at `0x00`
/// or `0xFF` — the eight EGA RGB primaries the 1 bpp × 3 plane mode
/// (spec §4 bit-plane example) reproduces exactly. When true, the
/// packed RGB input can go straight into
/// [`encode_pcx_1bpp_3planes_ega_rgb`]: its `>= 0x80` channel
/// threshold is the identity on `{0x00, 0xFF}` values, so the
/// round-trip is bit-exact.
fn auto_is_ega_primaries(palette: &[u8], colors: usize) -> bool {
    palette[..colors * 3]
        .iter()
        .all(|&c| c == 0x00 || c == 0xFF)
}

/// Build the 48-byte header palette (16 RGB triplets, spec §3
/// `Colormap`) for the [`PcxAutoMode::Indexed4`] /
/// [`PcxAutoMode::Indexed1x4`] candidates, or `None` when the image has
/// more than 16 distinct colours.
///
/// The first `colors` triplets are copied verbatim from the first-seen
/// scan; the rest stay zero (the index buffer never references them).
/// Exactness note for the decode side: the 16-colour paths substitute
/// the standard EGA hardware palette (spec table §3.1) only when the
/// header field is **all zeros**, which here can only happen when the
/// single distinct colour is pure black — and the hardware palette's
/// entry 0 *is* pure black, so index 0 still decodes to the source
/// colour and the round-trip stays exact in that corner too.
fn auto_palette48(palette: &[u8], colors: usize) -> Option<[u8; 48]> {
    if colors > 16 {
        return None;
    }
    let mut out = [0u8; 48];
    out[..colors * 3].copy_from_slice(&palette[..colors * 3]);
    Some(out)
}

/// Search the fixed CGA hardware palette space for an exact match of
/// the image's `≤ 4` distinct colours, for the [`PcxAutoMode::Cga2x1`]
/// / [`PcxAutoMode::Cga1x2`] candidates.
///
/// CGA stores no colour data: header byte 19's upper three C / P / I
/// bits (spec §"CGA Color Map") select one of four fixed chroma
/// palettes or two composite-monochrome grey ramps, and header byte
/// 16's high nibble picks palette entry 0 (the background) out of the
/// 16 standard EGA colours. So a colour set is CGA-representable iff
/// some `(selector, background)` pair yields a 4-entry palette
/// containing every distinct colour. The search space is 6 selectors ×
/// 16 backgrounds = 96 resolved palettes, each resolved through the
/// *decoder's own* header resolver
/// ([`crate::decoder::cga_palette_from_header`]) so encode-side
/// matching and decode-side reconstruction can never drift apart.
///
/// Returns the first match in a fixed scan order (selector `0x60`
/// white-bright, `0x40` white-dim, `0x20` yellow-bright, `0x00`
/// yellow-dim, `0x80` monochrome-dim, `0xA0` monochrome-bright;
/// background `0..=15`) plus a
/// source-index → CGA-index LUT (first matching palette entry, so ties
/// inside a palette are deterministic too). `None` when `colors > 4`
/// or no palette covers the set — the ladder never quantises.
fn auto_cga_match(palette: &[u8], colors: usize) -> Option<(u8, u8, [u8; 4])> {
    if colors > 4 {
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
            for i in 0..colors {
                let c = [palette[i * 3], palette[i * 3 + 1], palette[i * 3 + 2]];
                match pal4.iter().position(|p| *p == c) {
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
        let row_base = y * width as usize * 3;
        let bpl = bytes_per_line as usize;
        // 0x80 threshold matches the decode round-trip: any input byte ≥
        // 0x80 sets the bit, anything below clears it. The decoder always
        // emits 0x00 / 0xFF, so this is the cut that round-trips exactly
        // when the source is already in {0x00, 0xFF} per channel. Each
        // plane's scanline slice is packed eight pixels at a time.
        for plane in 0..3 {
            let dst = &mut row[plane * bpl..plane * bpl + bpl];
            pack_1bpp_plane_row(dst, width as usize, |x| {
                rgb[row_base + x * 3 + plane] >= 0x80
            });
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

/// Pack `width` 1-bit samples MSB-first into one plane's scanline slice
/// `dst`, eight pixels per output byte.
///
/// The PCX 1-bit-per-plane on-disk layout (spec §"Image File (.PCX)
/// Format") places pixel `x` at bit `7 - (x % 8)` of byte `x / 8`
/// within the plane. The naïve form scatters one branch-guarded
/// `dst[x/8] |= 1 << (7 - x%8)` store per set pixel — the documented
/// 1-bpp encoder hotspot (`BENCHMARKS.md` rank #2/#3: the per-pixel
/// index recompute + branch dominates `encode_1bpp_*`). This packs a
/// whole byte at a time instead: each group of up to eight pixels is
/// folded into one accumulator with a shift-OR and written once, so the
/// inner loop has no per-pixel array index, no per-pixel branch into the
/// destination, and no read-modify-write on `dst`.
///
/// `get_bit(x)` returns whether pixel `x` (`0..width`) sets this plane's
/// bit. The result is **byte-identical** to the scatter form: bit
/// `7 - k` of output byte `b` holds pixel `8·b + k`, absent tail pixels
/// (when `width` is not a multiple of 8) contribute a 0 bit exactly as
/// the scatter loop left them, and `dst` bytes beyond
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
    write_header_with_palette(
        &mut out,
        width,
        height,
        1,
        1,
        bytes_per_line,
        &MONO_COLORMAP,
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
/// `palette_selector` is the header byte 19 value (manual §"CGA Color
/// Map": upper three bits are C = color burst, P = palette family,
/// I = intensity; the lower five bits are ignored by readers):
/// * `0x60` → palette 1 bright (cyan/magenta/white).
/// * `0x40` → palette 1 dim (cyan/magenta/light gray).
/// * `0x20` → palette 0 bright (light green/light red/yellow).
/// * `0x00` → palette 0 dim (green/red/brown) — also where zero-filled
///   legacy headers land.
/// * `0x80` / `0xA0` → composite-monochrome ramps (dim / bright).
///
/// `background_index` is the EGA index used for palette entry 0; the
/// high nibble of header byte 16 (the colormap's first byte).
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
    // Manual "CGA Color Map": background nibble in header byte 16 =
    // colormap byte 0; C/P/I selector in header byte 19 = colormap
    // byte 3 (r401 conformance fix — both sat 16 bytes too deep).
    ega[0] = background_index << 4;
    ega[3] = palette_selector;
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
    ega[0] = background_index << 4;
    ega[3] = cpi.to_byte19();
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
    // Manual "CGA Color Map": background nibble in header byte 16 =
    // colormap byte 0; C/P/I selector in header byte 19 = colormap
    // byte 3 (r401 conformance fix — both sat 16 bytes too deep).
    ega[0] = background_index << 4;
    ega[3] = palette_selector;
    let mut out =
        Vec::with_capacity(PCX_HEADER_SIZE + (bytes_per_line as usize) * 2 * height as usize);
    write_header_with_palette(&mut out, width, height, 1, 2, bytes_per_line, &ega);
    let mut row = vec![0u8; bytes_per_line as usize * 2];
    for y in 0..height as usize {
        for v in row.iter_mut() {
            *v = 0;
        }
        let line = &indices[y * width as usize..];
        let bpl = bytes_per_line as usize;
        for plane in 0..2 {
            let dst = &mut row[plane * bpl..plane * bpl + bpl];
            pack_1bpp_plane_row(dst, width as usize, |x| (line[x] >> plane) & 1 != 0);
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
        let line = &indices[y * width as usize..];
        let bpl = bytes_per_line as usize;
        for plane in 0..4 {
            let dst = &mut row[plane * bpl..plane * bpl + bpl];
            pack_1bpp_plane_row(dst, width as usize, |x| (line[x] >> plane) & 1 != 0);
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

/// Build the packed `width × height × 3` RGB buffer a `PcxImage` carries,
/// dropping alpha for `Rgba` and borrowing in place for `Rgb24`. Rejects
/// `Indexed8` (which has no embedded palette to flatten). Shared by the
/// `PcxImage`-level wrapper writers.
fn pcx_image_rgb(image: &PcxImage) -> Result<std::borrow::Cow<'_, [u8]>> {
    use std::borrow::Cow;
    match image.pixel_format {
        PcxPixelFormat::Rgba => {
            let mut rgb = Vec::with_capacity(image.data.len() / 4 * 3);
            for c in image.data.chunks_exact(4) {
                rgb.extend_from_slice(&c[..3]);
            }
            Ok(Cow::Owned(rgb))
        }
        PcxPixelFormat::Rgb24 => Ok(Cow::Borrowed(&image.data)),
        PcxPixelFormat::Indexed8 => Err(Error::unsupported(
            "PCX encoder: Indexed8 input needs explicit palette \
             (use encode_pcx_8bpp_indexed)",
        )),
    }
}

/// Convenience wrapper that emits the **most compact** lossless PCX for a
/// [`PcxImage`], mirroring [`encode_pcx_rgb_auto`] at the image level.
///
/// Like [`encode_pcx_24bpp_image`] it flattens an `Rgba` / `Rgb24`
/// `PcxImage` to packed RGB (dropping alpha; rejecting `Indexed8`), but
/// instead of always writing the 24-bit planar form it runs the
/// [`encode_pcx_rgb_auto`] colour scan and emits the smaller of the
/// indexed and planar candidates (see that function for the size-compare
/// contract). It returns the chosen [`PcxAutoMode`] alongside the bytes.
///
/// Metadata threading differs by branch because only the planar 24-bit
/// writers carry the full `(window_origin, dpi, screen_size)` header
/// triple:
///
/// * **Planar branch** → delegates to [`encode_pcx_24bpp_image`], so all
///   three metadata fields round-trip exactly as that wrapper documents.
/// * **Indexed branch** → the 8 bpp indexed writers carry authoring DPI
///   ([`encode_pcx_8bpp_indexed_dpi`]) but have no window-origin /
///   screen-size variant. So when the image carries *only* DPI (or no
///   metadata) the indexed form is emitted with that DPI preserved; when
///   it additionally carries a window origin or screen size — fields the
///   indexed geometry cannot represent — the writer falls back to the
///   planar branch so none of the requested metadata is silently
///   dropped. This keeps the wrapper lossless on **both** pixels *and*
///   the header metadata the caller asked to preserve.
pub fn encode_pcx_image_auto(image: &PcxImage) -> Result<(Vec<u8>, PcxAutoMode)> {
    let w: u16 = image
        .width
        .try_into()
        .map_err(|_| Error::invalid("PCX encoder: width exceeds 65535"))?;
    let h: u16 = image
        .height
        .try_into()
        .map_err(|_| Error::invalid("PCX encoder: height exceeds 65535"))?;
    let rgb = pcx_image_rgb(image)?;
    // If the caller asked to preserve a window origin or screen size,
    // those live only in the planar 24-bit header geometry. Honour the
    // metadata over the size win: route through the metadata-preserving
    // planar wrapper and report `Rgb24`.
    if image.window_origin.is_some() || image.screen_size.is_some() {
        let bytes = encode_pcx_24bpp_image(image)?;
        return Ok((bytes, PcxAutoMode::Rgb24));
    }
    // No window / screen metadata: run the colour scan. The auto writer
    // returns the smaller of indexed / planar; we then re-apply DPI on the
    // chosen geometry (both the indexed and planar writers have a DPI
    // variant) so a tagged authoring resolution survives either branch.
    let (auto_bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb)?;
    let Some(dpi) = image.dpi else {
        // No DPI to thread either — the auto output already carries the
        // default header and is the final answer.
        return Ok((auto_bytes, mode));
    };
    // DPI present: re-emit the *chosen* geometry through its DPI variant
    // so the size decision the scan already made is preserved while the
    // header carries the authoring resolution.
    let bytes = if let PcxAutoMode::Rgb24 = mode {
        encode_pcx_24bpp_dpi(w, h, &rgb, dpi)?
    } else {
        // Every non-planar mode was derived from the ≤256-colour scan.
        // Rebuild the indexed payload + palette once under the DPI
        // writer — re-running the scan is cheap relative to the encode
        // and keeps this branch from duplicating the palette-build
        // logic — then re-derive the chosen mode's input the same way
        // the ladder did.
        let (indices, palette) = build_indexed_payload(w, h, &rgb)
            .expect("auto already proved ≤256 colours for this input");
        let colors = indices
            .iter()
            .copied()
            .max()
            .map(|m| m as usize + 1)
            .unwrap_or(0);
        match mode {
            PcxAutoMode::Rgb24 => unreachable!("handled above"),
            PcxAutoMode::Indexed8 { .. } => {
                encode_pcx_8bpp_indexed_dpi(w, h, &indices, &palette, dpi)?
            }
            PcxAutoMode::Gray8 => {
                let gray = auto_gray_pixels(&indices, &palette, colors)
                    .expect("auto already proved every colour is a pure grey");
                encode_pcx_8bpp_grayscale_dpi(w, h, &gray, dpi)?
            }
            PcxAutoMode::Mono1 => {
                let mono = auto_mono_pixels(&indices, &palette, colors)
                    .expect("auto already proved every colour is black or white");
                encode_pcx_1bpp_mono_dpi(w, h, &mono, dpi)?
            }
            PcxAutoMode::EgaRgb1x3 => encode_pcx_1bpp_3planes_ega_rgb_dpi(w, h, &rgb, dpi)?,
            PcxAutoMode::Indexed4 { .. } => {
                let pal48 = auto_palette48(&palette, colors)
                    .expect("auto already proved ≤16 colours for this input");
                encode_pcx_4bpp_packed_dpi(w, h, &indices, &pal48, dpi)?
            }
            PcxAutoMode::Indexed1x4 { .. } => {
                let pal48 = auto_palette48(&palette, colors)
                    .expect("auto already proved ≤16 colours for this input");
                encode_pcx_1bpp_4planes_ega_dpi(w, h, &indices, &pal48, dpi)?
            }
            PcxAutoMode::Cga2x1 {
                palette_selector,
                background_index,
            } => {
                let (_, _, lut) = auto_cga_match(&palette, colors)
                    .expect("auto already proved a CGA palette match for this input");
                let cga_indices: Vec<u8> = indices.iter().map(|&i| lut[i as usize]).collect();
                encode_pcx_2bpp_cga_dpi(
                    w,
                    h,
                    &cga_indices,
                    palette_selector,
                    background_index,
                    dpi,
                )?
            }
            PcxAutoMode::Cga1x2 {
                palette_selector,
                background_index,
            } => {
                let (_, _, lut) = auto_cga_match(&palette, colors)
                    .expect("auto already proved a CGA palette match for this input");
                let cga_indices: Vec<u8> = indices.iter().map(|&i| lut[i as usize]).collect();
                encode_pcx_1bpp_2planes_cga_dpi(
                    w,
                    h,
                    &cga_indices,
                    palette_selector,
                    background_index,
                    dpi,
                )?
            }
        }
    };
    Ok((bytes, mode))
}

/// Scan packed RGB into `(indices, 768-byte VGA palette)` when the image
/// has `≤ 256` distinct colours, else `None`. Shared by
/// [`encode_pcx_rgb_auto`] and the DPI-bearing indexed branch of
/// [`encode_pcx_image_auto`] so the first-seen palette assignment is
/// defined in exactly one place.
///
/// Lookup structure (r401): colour → index resolution goes through a
/// `HashMap` keyed on the packed 24-bit colour instead of a linear
/// `position()` probe of the palette vector. The linear probe made the
/// scan `O(colours × pixels)` — the profiled hot spot of the whole
/// auto ladder once the palette grows past a handful of entries (a
/// 176-level grayscale 640×480 scan was ~54M byte-triple compares).
/// First-seen assignment order is unchanged: the palette vector is
/// still pushed in raster-scan discovery order and the map is only an
/// index accelerator, so output bytes are identical.
fn build_indexed_payload(width: u16, height: u16, rgb: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    use std::collections::HashMap;
    let n_pixels = width as usize * height as usize;
    if rgb.len() < n_pixels * 3 {
        return None;
    }
    let mut palette_rgb: Vec<[u8; 3]> = Vec::with_capacity(256);
    let mut seen: HashMap<u32, u8> = HashMap::with_capacity(257);
    let mut indices: Vec<u8> = Vec::with_capacity(n_pixels);
    // Consecutive pixels are frequently equal (runs are what PCX RLE
    // exists for), so a one-entry last-colour cache short-circuits the
    // hash for the common case.
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
                if palette_rgb.len() == 256 {
                    return None;
                }
                let i = palette_rgb.len() as u8;
                palette_rgb.push([p[0], p[1], p[2]]);
                seen.insert(key, i);
                i
            }
        };
        indices.push(idx);
        last = Some((key, idx));
    }
    let mut palette = vec![0u8; PCX_VGA_PALETTE_BYTES];
    for (i, c) in palette_rgb.iter().enumerate() {
        palette[i * 3] = c[0];
        palette[i * 3 + 1] = c[1];
        palette[i * 3 + 2] = c[2];
    }
    Some((indices, palette))
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
        &MONO_COLORMAP,
        1,
        dpi,
        DEFAULT_SCREEN_SIZE,
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
    // Manual "CGA Color Map": background nibble in header byte 16 =
    // colormap byte 0; C/P/I selector in header byte 19 = colormap
    // byte 3 (r401 conformance fix — both sat 16 bytes too deep).
    ega[0] = background_index << 4;
    ega[3] = palette_selector;
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
    // Manual "CGA Color Map": background nibble in header byte 16 =
    // colormap byte 0; C/P/I selector in header byte 19 = colormap
    // byte 3 (r401 conformance fix — both sat 16 bytes too deep).
    ega[0] = background_index << 4;
    ega[3] = palette_selector;
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
        let line = &indices[y * width as usize..];
        let bpl = bytes_per_line as usize;
        for plane in 0..2 {
            let dst = &mut row[plane * bpl..plane * bpl + bpl];
            pack_1bpp_plane_row(dst, width as usize, |x| (line[x] >> plane) & 1 != 0);
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
        let line = &indices[y * width as usize..];
        let bpl = bytes_per_line as usize;
        for plane in 0..4 {
            let dst = &mut row[plane * bpl..plane * bpl + bpl];
            pack_1bpp_plane_row(dst, width as usize, |x| (line[x] >> plane) & 1 != 0);
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
