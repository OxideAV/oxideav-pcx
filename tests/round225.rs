//! r225 — window-origin round-trip (header `x_min` / `y_min`).
//!
//! Spec §3 defines the image window via `(x_min, y_min) ... (x_max, y_max)`, with the visible width / height derived as `x_max - x_min + 1` / `y_max - y_min + 1`. PCX 3.0+ allows a non-zero origin so an editor can record the source crop region the pixel buffer came from; prior to r225 the standalone [`oxideav_pcx::PcxImage`] threw the decoded origin on the floor and [`oxideav_pcx::encode_pcx_24bpp_image`] always re-emitted `(0, 0)` — a windowed PCX silently lost its crop metadata across a decode → re-encode pass.
//!
//! r225 surfaces the origin on [`oxideav_pcx::PcxImage::window_origin`] (the decoder reports `Some((x, y))` whenever at least one of the two header words is non-zero, and `None` for the conventional zero-origin screen-author case), and adds the combined [`oxideav_pcx::encode_pcx_24bpp_window_dpi`] writer plus matching plumbing in `encode_pcx_24bpp_image` so a windowed-and-DPI-tagged source round-trips both metadata fields end-to-end through that one wrapper call.

use oxideav_pcx::types::PCX_HEADER_SIZE;
use oxideav_pcx::{
    encode_pcx_24bpp, encode_pcx_24bpp_dpi, encode_pcx_24bpp_image, encode_pcx_24bpp_window,
    encode_pcx_24bpp_window_dpi, parse_pcx, PcxError, PcxImage, PcxPixelFormat,
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
            rgb.push(((x * 11 + y * 7) & 0xFF) as u8);
        }
    }
    rgb
}

// ---------------------------------------------------------------------------
// Decoder surfaces window origin per spec §3
// ---------------------------------------------------------------------------

/// A zero-origin file decodes with `window_origin = None`: most
/// screen-authored PCX files leave `(x_min, y_min) = (0, 0)`.
#[test]
fn zero_origin_decodes_as_window_origin_none() {
    let rgb = dummy_rgb(4, 4);
    let bytes = encode_pcx_24bpp(4, 4, &rgb).unwrap();
    assert_eq!(read_u16_le(&bytes, 4), 0);
    assert_eq!(read_u16_le(&bytes, 6), 0);
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.window_origin, None);
}

/// A non-zero origin decodes with `window_origin = Some((x, y))`.
#[test]
fn non_zero_origin_decodes_as_window_origin_some() {
    let rgb = dummy_rgb(4, 4);
    let bytes = encode_pcx_24bpp_window(100, 200, 4, 4, &rgb).unwrap();
    assert_eq!(read_u16_le(&bytes, 4), 100);
    assert_eq!(read_u16_le(&bytes, 6), 200);
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.window_origin, Some((100, 200)));
    // Width / height stay derived from `x_max - x_min + 1` / `y_max -
    // y_min + 1` — the origin shifts the window but not the buffer
    // length.
    assert_eq!(img.width, 4);
    assert_eq!(img.height, 4);
    assert_eq!(img.data.len(), 4 * 4 * 4);
}

/// Asymmetric origins (only one axis non-zero) still surface as `Some`
/// — per spec §3 either axis carries a valid crop origin independently.
#[test]
fn asymmetric_origin_decodes_as_some() {
    let rgb = dummy_rgb(4, 4);
    let only_x = encode_pcx_24bpp_window(50, 0, 4, 4, &rgb).unwrap();
    let img_x = parse_pcx(&only_x).unwrap();
    assert_eq!(img_x.window_origin, Some((50, 0)));

    let only_y = encode_pcx_24bpp_window(0, 75, 4, 4, &rgb).unwrap();
    let img_y = parse_pcx(&only_y).unwrap();
    assert_eq!(img_y.window_origin, Some((0, 75)));
}

// ---------------------------------------------------------------------------
// `encode_pcx_24bpp_window_dpi` — combined writer
// ---------------------------------------------------------------------------

/// The combined writer stamps both the origin words (offsets 4/6) and
/// the DPI words (offsets 12/14) into the header.
#[test]
fn window_dpi_writer_stamps_both_header_fields() {
    let rgb = dummy_rgb(4, 2);
    let bytes = encode_pcx_24bpp_window_dpi(64, 128, 4, 2, &rgb, (300, 600)).unwrap();
    assert_eq!(read_u16_le(&bytes, 4), 64);
    assert_eq!(read_u16_le(&bytes, 6), 128);
    assert_eq!(read_u16_le(&bytes, 12), 300);
    assert_eq!(read_u16_le(&bytes, 14), 600);
    // x_max / y_max derived from origin + dimensions - 1.
    assert_eq!(read_u16_le(&bytes, 8), 64 + 4 - 1);
    assert_eq!(read_u16_le(&bytes, 10), 128 + 2 - 1);
}

/// Combined writer self-roundtrips: decoding the output surfaces both
/// `window_origin` and `dpi` as `Some(...)` with the original values.
#[test]
fn window_dpi_writer_self_roundtrip() {
    let rgb = dummy_rgb(8, 8);
    let bytes = encode_pcx_24bpp_window_dpi(10, 20, 8, 8, &rgb, (200, 400)).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.window_origin, Some((10, 20)));
    assert_eq!(img.dpi, Some((200, 400)));
}

/// Combined writer rejects `(0, _)` / `(_, 0)` DPI per the same spec §3
/// "0 = unset" rule the other `_dpi` writers enforce.
#[test]
fn window_dpi_writer_rejects_zero_component_dpi() {
    let rgb = dummy_rgb(4, 4);
    let err = encode_pcx_24bpp_window_dpi(10, 10, 4, 4, &rgb, (0, 300)).unwrap_err();
    assert!(matches!(err, PcxError::InvalidData(_)));
    let err = encode_pcx_24bpp_window_dpi(10, 10, 4, 4, &rgb, (300, 0)).unwrap_err();
    assert!(matches!(err, PcxError::InvalidData(_)));
}

/// Combined writer rejects origin overflow (x_min + width > u16::MAX +
/// 1, same as `encode_pcx_24bpp_window`).
#[test]
fn window_dpi_writer_rejects_origin_overflow() {
    let rgb = dummy_rgb(8, 8);
    let err = encode_pcx_24bpp_window_dpi(0xFFF8, 0, 16, 8, &rgb, (300, 300)).unwrap_err();
    assert!(matches!(err, PcxError::InvalidData(_)));
    let err = encode_pcx_24bpp_window_dpi(0, 0xFFF8, 8, 16, &rgb, (300, 300)).unwrap_err();
    assert!(matches!(err, PcxError::InvalidData(_)));
}

// ---------------------------------------------------------------------------
// `encode_pcx_24bpp_image` wrapper — four (dpi × window_origin) cases
// ---------------------------------------------------------------------------

/// `dpi = None`, `window_origin = None` → plain 24bpp writer (72×72,
/// `(0, 0)` origin).
#[test]
fn wrapper_neither_uses_plain_writer() {
    let rgb = dummy_rgb(4, 2);
    let img = PcxImage {
        width: 4,
        height: 2,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb,
        pts: None,
        dpi: None,
        window_origin: None,
    };
    let bytes = encode_pcx_24bpp_image(&img).unwrap();
    assert_eq!(read_u16_le(&bytes, 4), 0);
    assert_eq!(read_u16_le(&bytes, 6), 0);
    assert_eq!(read_u16_le(&bytes, 12), 72);
    assert_eq!(read_u16_le(&bytes, 14), 72);
}

/// `dpi = Some`, `window_origin = None` → 24bpp_dpi writer.
#[test]
fn wrapper_dpi_only_uses_dpi_writer() {
    let rgb = dummy_rgb(4, 2);
    let img = PcxImage {
        width: 4,
        height: 2,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb.clone(),
        pts: None,
        dpi: Some((300, 300)),
        window_origin: None,
    };
    let bytes = encode_pcx_24bpp_image(&img).unwrap();
    assert_eq!(read_u16_le(&bytes, 4), 0);
    assert_eq!(read_u16_le(&bytes, 6), 0);
    assert_eq!(read_u16_le(&bytes, 12), 300);
    assert_eq!(read_u16_le(&bytes, 14), 300);
    // Bit-identical to the standalone DPI writer.
    let direct = encode_pcx_24bpp_dpi(4, 2, &rgb, (300, 300)).unwrap();
    assert_eq!(bytes, direct);
}

/// `dpi = None`, `window_origin = Some` → 24bpp_window writer.
#[test]
fn wrapper_window_only_uses_window_writer() {
    let rgb = dummy_rgb(4, 2);
    let img = PcxImage {
        width: 4,
        height: 2,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb.clone(),
        pts: None,
        dpi: None,
        window_origin: Some((50, 100)),
    };
    let bytes = encode_pcx_24bpp_image(&img).unwrap();
    assert_eq!(read_u16_le(&bytes, 4), 50);
    assert_eq!(read_u16_le(&bytes, 6), 100);
    assert_eq!(read_u16_le(&bytes, 12), 72);
    assert_eq!(read_u16_le(&bytes, 14), 72);
    // Bit-identical to the standalone window writer.
    let direct = encode_pcx_24bpp_window(50, 100, 4, 2, &rgb).unwrap();
    assert_eq!(bytes, direct);
}

/// `dpi = Some`, `window_origin = Some` → combined window_dpi writer.
#[test]
fn wrapper_both_uses_window_dpi_writer() {
    let rgb = dummy_rgb(4, 2);
    let img = PcxImage {
        width: 4,
        height: 2,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb.clone(),
        pts: None,
        dpi: Some((300, 300)),
        window_origin: Some((50, 100)),
    };
    let bytes = encode_pcx_24bpp_image(&img).unwrap();
    assert_eq!(read_u16_le(&bytes, 4), 50);
    assert_eq!(read_u16_le(&bytes, 6), 100);
    assert_eq!(read_u16_le(&bytes, 12), 300);
    assert_eq!(read_u16_le(&bytes, 14), 300);
    let direct = encode_pcx_24bpp_window_dpi(50, 100, 4, 2, &rgb, (300, 300)).unwrap();
    assert_eq!(bytes, direct);
}

// ---------------------------------------------------------------------------
// End-to-end round-trip: decode → re-encode preserves origin
// ---------------------------------------------------------------------------

/// A windowed-and-DPI-tagged PCX makes one round-trip through the
/// decoder + `encode_pcx_24bpp_image` wrapper without losing either
/// metadata field. Pixel bytes also round-trip bit-identically.
#[test]
fn end_to_end_windowed_dpi_roundtrip_preserves_metadata() {
    let rgb = dummy_rgb(16, 12);
    let bytes_in = encode_pcx_24bpp_window_dpi(75, 50, 16, 12, &rgb, (300, 300)).unwrap();
    let decoded = parse_pcx(&bytes_in).unwrap();
    assert_eq!(decoded.window_origin, Some((75, 50)));
    assert_eq!(decoded.dpi, Some((300, 300)));
    let bytes_out = encode_pcx_24bpp_image(&decoded).unwrap();
    let re_decoded = parse_pcx(&bytes_out).unwrap();
    assert_eq!(re_decoded.window_origin, Some((75, 50)));
    assert_eq!(re_decoded.dpi, Some((300, 300)));
    assert_eq!(re_decoded.data, decoded.data);
}

/// A plain zero-origin PCX (default 72×72 DPI) makes the round-trip
/// without gaining a spurious non-zero origin — the wrapper's
/// `window_origin = None` case routes through the plain writer rather
/// than restating an implicit `(0, 0)` as data. The decoded DPI sits
/// at `Some((72, 72))` because the plain writer emits the historical
/// PC Paintbrush convention, and the round-trip preserves it.
#[test]
fn end_to_end_zero_origin_roundtrip_stays_zero() {
    let rgb = dummy_rgb(8, 8);
    let bytes_in = encode_pcx_24bpp(8, 8, &rgb).unwrap();
    let decoded = parse_pcx(&bytes_in).unwrap();
    assert_eq!(decoded.window_origin, None);
    assert_eq!(decoded.dpi, Some((72, 72)));
    let bytes_out = encode_pcx_24bpp_image(&decoded).unwrap();
    assert!(bytes_in.len() > PCX_HEADER_SIZE);
    let re_decoded = parse_pcx(&bytes_out).unwrap();
    assert_eq!(re_decoded.window_origin, None);
    assert_eq!(re_decoded.dpi, Some((72, 72)));
    assert_eq!(re_decoded.data, decoded.data);
}
