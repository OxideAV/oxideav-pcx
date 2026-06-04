//! r219 — authoring DPI round-trip (header `h_dpi` / `v_dpi`).
//!
//! Spec §3 records the header's `h_dpi` / `v_dpi` words as "the
//! resolutions at which the image was created (printer or scanner);
//! e.g. a scan might store 300, 300". Prior to r219 every writer in the
//! crate hard-coded the historical 72×72 "screen DPI" PC Paintbrush
//! convention and the standalone [`oxideav_pcx::PcxImage`] threw the
//! decoded value on the floor — a decode → re-encode cycle therefore
//! silently destroyed the authoring resolution metadata of a scanned
//! input.
//!
//! r219 surfaces the DPI on [`oxideav_pcx::PcxImage::dpi`] (the decoder
//! reports `Some((h, v))` whenever both header fields are non-zero, and
//! `None` otherwise per the spec §3 "0 = unset" sentinel), and adds
//! four new `*_dpi` writer entry points so callers can stamp a custom
//! authoring resolution into the header without forking the rest of
//! the writer. The wrapper [`oxideav_pcx::encode_pcx_24bpp_image`] also
//! threads `PcxImage::dpi` through automatically so a round-trip
//! through that helper preserves the metadata end-to-end.

use oxideav_pcx::types::PCX_HEADER_SIZE;
use oxideav_pcx::{
    encode_pcx_1bpp_mono, encode_pcx_1bpp_mono_dpi, encode_pcx_24bpp, encode_pcx_24bpp_dpi,
    encode_pcx_24bpp_image, encode_pcx_8bpp_grayscale, encode_pcx_8bpp_grayscale_dpi,
    encode_pcx_8bpp_indexed, encode_pcx_8bpp_indexed_dpi, parse_pcx, PcxError, PcxImage,
    PcxPixelFormat,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_u16_le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn dummy_rgb(w: usize, h: usize) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            rgb.push(((x ^ y) & 0xFF) as u8);
            rgb.push(((x + y) & 0xFF) as u8);
            rgb.push(((x * 7 + y * 5) & 0xFF) as u8);
        }
    }
    rgb
}

fn dummy_palette() -> Vec<u8> {
    let mut p = Vec::with_capacity(768);
    for i in 0..256u32 {
        p.push(i as u8);
        p.push((255 - i) as u8);
        p.push((i ^ 0x5A) as u8);
    }
    p
}

// ---------------------------------------------------------------------------
// Header DPI placement
// ---------------------------------------------------------------------------

/// The header's `h_dpi` lives at offset 12, `v_dpi` at offset 14 (both
/// 16-bit little-endian). Verify the new `_dpi` variants stamp the
/// requested values there, and that the legacy entry points still emit
/// the documented 72×72 default.
#[test]
fn dpi_lands_at_header_offsets_12_14() {
    let rgb = dummy_rgb(4, 2);
    let default = encode_pcx_24bpp(4, 2, &rgb).unwrap();
    assert_eq!(read_u16_le(&default, 12), 72);
    assert_eq!(read_u16_le(&default, 14), 72);

    let scanner = encode_pcx_24bpp_dpi(4, 2, &rgb, (300, 300)).unwrap();
    assert_eq!(read_u16_le(&scanner, 12), 300);
    assert_eq!(read_u16_le(&scanner, 14), 300);
}

/// Asymmetric authoring DPI (a printer-DPI scan would commonly hit
/// 300×600 on a half-toned interleaved-pass scan). Both axes are
/// written independently.
#[test]
fn dpi_asymmetric_axes() {
    let pixels: Vec<u8> = (0..(8 * 4)).map(|i| (i * 7) as u8).collect();
    let bytes = encode_pcx_8bpp_grayscale_dpi(8, 4, &pixels, (300, 600)).unwrap();
    assert_eq!(read_u16_le(&bytes, 12), 300);
    assert_eq!(read_u16_le(&bytes, 14), 600);
}

// ---------------------------------------------------------------------------
// Decoder surfaces the DPI on PcxImage
// ---------------------------------------------------------------------------

#[test]
fn decoder_surfaces_dpi_when_header_carries_non_zero() {
    let rgb = dummy_rgb(8, 4);
    let bytes = encode_pcx_24bpp_dpi(8, 4, &rgb, (300, 300)).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.dpi, Some((300, 300)));
}

#[test]
fn decoder_dpi_is_some_for_default_72_dpi() {
    // The plain writer emits 72×72; the decoder must still report it as
    // a Some(...) — 72 is a valid non-zero authoring resolution, not a
    // sentinel.
    let rgb = dummy_rgb(8, 4);
    let bytes = encode_pcx_24bpp(8, 4, &rgb).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.dpi, Some((72, 72)));
}

#[test]
fn decoder_dpi_is_none_when_either_field_zero() {
    // Hand-craft a PCX whose h_dpi is 0 and v_dpi is non-zero: per spec
    // §3 the 0 sentinel means "unset" and the decoder must collapse the
    // asymmetric tuple to None rather than reporting (0, 300).
    let rgb = dummy_rgb(4, 2);
    let mut bytes = encode_pcx_24bpp(4, 2, &rgb).unwrap();
    bytes[12] = 0;
    bytes[13] = 0;
    // v_dpi stays at 72 = 0x48 from the writer.
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.dpi, None);

    // Mirror: v_dpi = 0, h_dpi non-zero → still None.
    let mut bytes = encode_pcx_24bpp(4, 2, &rgb).unwrap();
    bytes[14] = 0;
    bytes[15] = 0;
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.dpi, None);

    // Both zero → None.
    let mut bytes = encode_pcx_24bpp(4, 2, &rgb).unwrap();
    bytes[12] = 0;
    bytes[13] = 0;
    bytes[14] = 0;
    bytes[15] = 0;
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.dpi, None);
}

// ---------------------------------------------------------------------------
// Decode → re-encode round-trip through the PcxImage wrapper preserves DPI
// ---------------------------------------------------------------------------

#[test]
fn pcximage_wrapper_threads_dpi_through_encode_round_trip() {
    let rgb = dummy_rgb(16, 8);
    let scanner_bytes = encode_pcx_24bpp_dpi(16, 8, &rgb, (300, 300)).unwrap();
    let decoded = parse_pcx(&scanner_bytes).unwrap();
    assert_eq!(decoded.dpi, Some((300, 300)));

    // Re-encode via the wrapper: PcxImage::dpi must thread through into
    // the new header at bytes 12-15.
    let re_encoded = encode_pcx_24bpp_image(&decoded).unwrap();
    assert_eq!(read_u16_le(&re_encoded, 12), 300);
    assert_eq!(read_u16_le(&re_encoded, 14), 300);
    let re_decoded = parse_pcx(&re_encoded).unwrap();
    assert_eq!(re_decoded.dpi, Some((300, 300)));
    // Pixel data still round-trips bit-identically.
    assert_eq!(re_decoded.data, decoded.data);
}

#[test]
fn pcximage_wrapper_uses_default_72_when_dpi_is_none() {
    // A manually-constructed PcxImage with `dpi: None` re-encodes
    // through the wrapper at the 72×72 default — the wrapper doesn't
    // bake in 0×0.
    let rgb = dummy_rgb(4, 2);
    let img = PcxImage {
        width: 4,
        height: 2,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb,
        pts: None,
        dpi: None,
        window_origin: None,
        screen_size: None,
    };
    let bytes = encode_pcx_24bpp_image(&img).unwrap();
    assert_eq!(read_u16_le(&bytes, 12), 72);
    assert_eq!(read_u16_le(&bytes, 14), 72);
}

// ---------------------------------------------------------------------------
// Pixel-data invariance: changing the DPI never alters decoded pixels.
// ---------------------------------------------------------------------------

#[test]
fn dpi_writers_pixel_identical_to_legacy_72_dpi_writers() {
    // For every writer pair (legacy vs `_dpi` at (72, 72)), the on-disk
    // RLE pixel stream after the header must be byte-identical — the
    // DPI lives entirely in header bytes 12-15.
    let rgb = dummy_rgb(8, 4);
    let a = encode_pcx_24bpp(8, 4, &rgb).unwrap();
    let b = encode_pcx_24bpp_dpi(8, 4, &rgb, (72, 72)).unwrap();
    assert_eq!(a, b);

    let palette = dummy_palette();
    let pixels: Vec<u8> = (0..(8 * 4)).map(|i| (i * 13) as u8).collect();
    let a = encode_pcx_8bpp_indexed(8, 4, &pixels, &palette).unwrap();
    let b = encode_pcx_8bpp_indexed_dpi(8, 4, &pixels, &palette, (72, 72)).unwrap();
    assert_eq!(a, b);

    let pixels: Vec<u8> = (0..(8 * 4)).map(|i| (i * 7) as u8).collect();
    let a = encode_pcx_8bpp_grayscale(8, 4, &pixels).unwrap();
    let b = encode_pcx_8bpp_grayscale_dpi(8, 4, &pixels, (72, 72)).unwrap();
    assert_eq!(a, b);

    let mono: Vec<u8> = (0..(8 * 4)).map(|i| (i & 1) as u8).collect();
    let a = encode_pcx_1bpp_mono(8, 4, &mono).unwrap();
    let b = encode_pcx_1bpp_mono_dpi(8, 4, &mono, (72, 72)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn dpi_writer_round_trip_preserves_decoded_pixels() {
    // The grayscale `_dpi` variant must still produce a parse-able PCX
    // whose decoded pixels match the input (decoder emits (g, g, g,
    // 0xFF) per pixel because the writer also stamps palette_info = 2).
    let pixels: Vec<u8> = (0..(8 * 4)).map(|i| (i * 7) as u8).collect();
    let bytes = encode_pcx_8bpp_grayscale_dpi(8, 4, &pixels, (200, 200)).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.dpi, Some((200, 200)));
    assert_eq!(img.width, 8);
    assert_eq!(img.height, 4);
    assert_eq!(img.pixel_format, PcxPixelFormat::Rgba);
    for (i, chunk) in img.data.chunks_exact(4).enumerate() {
        let expected = pixels[i];
        assert_eq!(chunk[0], expected);
        assert_eq!(chunk[1], expected);
        assert_eq!(chunk[2], expected);
        assert_eq!(chunk[3], 0xFF);
    }
}

// ---------------------------------------------------------------------------
// Rejection paths
// ---------------------------------------------------------------------------

#[test]
fn dpi_writer_rejects_zero_h_dpi() {
    let rgb = dummy_rgb(4, 2);
    let err = encode_pcx_24bpp_dpi(4, 2, &rgb, (0, 72)).unwrap_err();
    assert!(matches!(err, PcxError::InvalidData(_)));
    let msg = format!("{err}");
    assert!(msg.contains("non-zero"), "got: {msg}");
}

#[test]
fn dpi_writer_rejects_zero_v_dpi() {
    let pixels: Vec<u8> = (0..(8 * 4)).map(|i| i as u8).collect();
    let err = encode_pcx_8bpp_grayscale_dpi(8, 4, &pixels, (72, 0)).unwrap_err();
    assert!(matches!(err, PcxError::InvalidData(_)));
}

#[test]
fn dpi_indexed_writer_rejects_bad_palette_length() {
    // Same palette-length rejection path as the non-DPI variant.
    let pixels: Vec<u8> = (0..(8 * 4)).map(|i| i as u8).collect();
    let palette = vec![0u8; 100];
    let err = encode_pcx_8bpp_indexed_dpi(8, 4, &pixels, &palette, (300, 300)).unwrap_err();
    assert!(matches!(err, PcxError::InvalidData(_)));
}

// ---------------------------------------------------------------------------
// Header length stays at 128 bytes regardless of DPI value.
// ---------------------------------------------------------------------------

#[test]
fn header_length_unchanged_under_arbitrary_dpi() {
    let rgb = dummy_rgb(2, 2);
    let bytes = encode_pcx_24bpp_dpi(2, 2, &rgb, (65535, 65535)).unwrap();
    // First 128 bytes are the header. Manufacturer + version + encoding
    // + bpp at offsets 0..4 still in place; bytes_per_line at offset 66.
    assert_eq!(bytes[0], 0x0A);
    assert_eq!(bytes[1], 5);
    assert_eq!(bytes[2], 1);
    assert_eq!(bytes[65], 3);
    // bytes_per_line round_up_to_even(2) = 2
    assert_eq!(read_u16_le(&bytes, 66), 2);
    // RLE data starts at PCX_HEADER_SIZE = 128.
    assert!(bytes.len() > PCX_HEADER_SIZE);
}
