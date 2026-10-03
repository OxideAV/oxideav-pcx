//! The root vocabulary of the image-crate contract (`IMAGE_CRATE_API`):
//! `probe` / `info` / `decode*` / `encode*`, all framework-free, plus
//! the typed [`header`] depth accessor.

use std::io::{Read, Write};

use crate::decoder::{decode_image, header_info, probe_bytes, validate_header};
use crate::encoder::encode_image;
use crate::error::{PcxError as Error, Result};
use crate::image::{Frame, ImageInfo, PcxImage, RgbImage, RgbaImage};
use crate::options::{DecodeOptions, EncodeOptions};
use crate::types::PcxHeader;

/// `true` when `bytes` looks like a PCX file (manufacturer byte `0x0A`,
/// a known version, encoding `1`, a spec depth / plane count and a
/// non-inverted window — PCX has no magic beyond that) or a DCX bundle
/// (its 4-byte magic). Allocation-free; `false` on short input.
pub fn probe(bytes: &[u8]) -> bool {
    probe_bytes(bytes)
}

/// Describe a PCX from its 128-byte header (plus the VGA tail marker)
/// without decoding pixels: dimensions, the native layout [`decode`]
/// would return, the on-disk geometry, version and header annotations.
/// For a DCX bundle: the first page, with `frames` = the page count.
/// Fails with the same header errors [`decode`] would.
pub fn info(bytes: &[u8]) -> Result<ImageInfo> {
    header_info(bytes)
}

/// The typed 128-byte header, validated as [`decode`] validates it
/// (manufacturer, version, encoding, geometry). The depth accessor
/// behind [`info`]; the raw fields are on [`PcxHeader`].
pub fn header(bytes: &[u8]) -> Result<PcxHeader> {
    validate_header(bytes, false).map(|v| v.header)
}

/// Decode a PCX (or the first page of a DCX bundle) into its native
/// layout with [`DecodeOptions::default`].
pub fn decode(bytes: &[u8]) -> Result<PcxImage> {
    decode_image(bytes, &DecodeOptions::default())
}

/// [`decode`] under explicit limits / strictness.
pub fn decode_with(bytes: &[u8], opts: &DecodeOptions) -> Result<PcxImage> {
    decode_image(bytes, opts)
}

/// Decode straight to tightly packed 8-bit RGB. See
/// [`PcxImage::to_rgb8`] for the per-layout kernels.
pub fn decode_rgb8(bytes: &[u8]) -> Result<RgbImage> {
    let img = decode(bytes)?;
    Ok(RgbImage::new(img.width, img.height, img.to_rgb8()))
}

/// Decode straight to tightly packed 8-bit RGBA (alpha `255`: PCX has
/// no alpha). See [`PcxImage::to_rgba8`] for the per-layout kernels.
pub fn decode_rgba8(bytes: &[u8]) -> Result<RgbaImage> {
    let img = decode(bytes)?;
    Ok(RgbaImage::new(img.width, img.height, img.to_rgba8()))
}

/// Every image in the file: one [`Frame`] for a PCX, one per page for a
/// DCX bundle (`delay` always `None`, `index` = page number).
pub fn decode_all(bytes: &[u8]) -> Result<Vec<Frame>> {
    crate::dcx::decode_all_with(bytes, &DecodeOptions::default())
}

/// [`decode_all`] under explicit limits / strictness (applied per
/// page).
pub fn decode_all_with(bytes: &[u8], opts: &DecodeOptions) -> Result<Vec<Frame>> {
    crate::dcx::decode_all_with(bytes, opts)
}

/// Read `r` to its end and [`decode`] the bytes.
pub fn decode_from<R: Read>(mut r: R) -> Result<PcxImage> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    decode(&buf)
}

/// Encode `image` as a PCX file. See [`EncodeOptions`] for the layout →
/// geometry mapping; the image is written in its own geometry (`Rgb24`
/// as 24-bit, `Gray8` as grayscale, `Pal8` + palette in the smallest
/// palette carrier that stores it verbatim) — never a silent
/// conversion. `Rgba` input, or a palette with alpha, is
/// [`Error::Unsupported`] unless [`EncodeOptions::drop_alpha`] is set;
/// a `Pal8` image without a palette, or with an index outside it, is
/// [`Error::InvalidData`]; dimensions over 65535 are
/// [`Error::Unsupported`].
pub fn encode(image: &PcxImage, opts: &EncodeOptions) -> Result<Vec<u8>> {
    encode_image(image, opts)
}

/// Encode tightly packed 8-bit RGB (`3 × width × height` bytes) as a
/// 24-bit PCX (8 bpp × 3 planes).
pub fn encode_rgb8(width: u32, height: u32, rgb: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    check_raw_len(width, height, 3, rgb.len())?;
    encode_image(&PcxImage::from_rgb8(width, height, rgb.to_vec())?, opts)
}

/// Encode tightly packed 8-bit RGBA (`4 × width × height` bytes) as a
/// 24-bit PCX. PCX has no alpha mechanism: **the alpha channel is
/// dropped** (this is the one documented exception to the no-silent-
/// conversion rule, as the contract allows for formats without alpha).
pub fn encode_rgba8(width: u32, height: u32, rgba: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    check_raw_len(width, height, 4, rgba.len())?;
    let opts = opts.clone().with_drop_alpha(true);
    encode_image(&PcxImage::from_rgba8(width, height, rgba.to_vec())?, &opts)
}

/// [`encode`] straight into a writer.
pub fn encode_to<W: Write>(image: &PcxImage, opts: &EncodeOptions, mut w: W) -> Result<()> {
    let bytes = encode_image(image, opts)?;
    w.write_all(&bytes)?;
    Ok(())
}

fn check_raw_len(width: u32, height: u32, bpp: usize, len: usize) -> Result<()> {
    let need = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(bpp))
        .ok_or_else(|| Error::invalid("PCX encoder: dimensions overflow"))?;
    if len < need {
        return Err(Error::invalid(format!(
            "PCX encoder: {width}x{height} at {bpp} bytes/pixel needs {need} bytes, got {len}"
        )));
    }
    Ok(())
}
