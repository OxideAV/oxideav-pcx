//! r231 — authoring screen-size round-trip (header `h_screen_size` /
//! `v_screen_size`).
//!
//! Spec §3 records the rev-5 header words at offsets 70 / 72 as
//! "Horizontal screen size in pixels (new field found only in PB IV /
//! IV Plus)" and "Vertical screen size in pixels (new field found only
//! in PB IV / IV Plus)". These annotate the display resolution the
//! image was authored against — distinct from the printer / scanner
//! DPI in `h_dpi` / `v_dpi`. Prior to r231 the decoder discarded both
//! fields and every writer hard-coded `(0, 0)`, so a tagged PB IV / IV
//! Plus PCX silently lost its authoring screen size across a decode →
//! re-encode pass.
//!
//! r231 surfaces the pair on [`oxideav_pcx::PcxImage::screen_size`]
//! (decoder reports `Some((h, v))` whenever both header words are
//! non-zero, and `None` otherwise — an asymmetric pair collapses to
//! `None` per the spec §3 "unset" sentinel), and adds two new public
//! writers — [`oxideav_pcx::encode_pcx_24bpp_screen`] for the screen-
//! size-only case plus
//! [`oxideav_pcx::encode_pcx_24bpp_window_dpi_screen`] for the
//! maximally-tagged case — so the existing wrapper
//! [`oxideav_pcx::encode_pcx_24bpp_image`] can dispatch across the
//! eight `(window_origin, dpi, screen_size)` `Option` combinations and
//! round-trip every metadata field together.

use oxideav_pcx::types::PCX_HEADER_SIZE;
use oxideav_pcx::{
    encode_pcx_24bpp, encode_pcx_24bpp_dpi, encode_pcx_24bpp_image, encode_pcx_24bpp_screen,
    encode_pcx_24bpp_window, encode_pcx_24bpp_window_dpi, encode_pcx_24bpp_window_dpi_screen,
    parse_pcx, PcxError, PcxImage, PcxPixelFormat,
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
            rgb.push(((x * 13 + y * 5) & 0xFF) as u8);
        }
    }
    rgb
}

// ---------------------------------------------------------------------------
// Decoder surfaces screen_size per spec §3 sentinel
// ---------------------------------------------------------------------------

/// A header with `(h_screen_size, v_screen_size) = (0, 0)` decodes as
/// `screen_size = None` — the pre-PB-IV default the plain writer emits.
#[test]
fn zero_screen_size_decodes_as_none() {
    let rgb = dummy_rgb(4, 4);
    let bytes = encode_pcx_24bpp(4, 4, &rgb).unwrap();
    assert_eq!(read_u16_le(&bytes, 70), 0);
    assert_eq!(read_u16_le(&bytes, 72), 0);
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.screen_size, None);
}

/// A header with both screen-size words non-zero decodes as
/// `Some((h, v))`.
#[test]
fn non_zero_screen_size_decodes_as_some() {
    let rgb = dummy_rgb(4, 4);
    let bytes = encode_pcx_24bpp_screen(4, 4, &rgb, (640, 480)).unwrap();
    assert_eq!(read_u16_le(&bytes, 70), 640);
    assert_eq!(read_u16_le(&bytes, 72), 480);
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.screen_size, Some((640, 480)));
    // Pixel buffer length is unaffected.
    assert_eq!(img.width, 4);
    assert_eq!(img.height, 4);
    assert_eq!(img.data.len(), 4 * 4 * 4);
}

/// Asymmetric fields (only one axis non-zero) collapse to `None` per
/// the spec §3 sentinel — a single 0 makes the pair "unset".
#[test]
fn asymmetric_screen_size_decodes_as_none() {
    let rgb = dummy_rgb(4, 4);
    // Build a tagged file then patch one axis back to 0 in the header.
    let mut bytes = encode_pcx_24bpp_screen(4, 4, &rgb, (1024, 768)).unwrap();
    bytes[70] = 0;
    bytes[71] = 0;
    assert_eq!(read_u16_le(&bytes, 70), 0);
    assert_eq!(read_u16_le(&bytes, 72), 768);
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.screen_size, None);

    let mut bytes2 = encode_pcx_24bpp_screen(4, 4, &rgb, (1024, 768)).unwrap();
    bytes2[72] = 0;
    bytes2[73] = 0;
    let img2 = parse_pcx(&bytes2).unwrap();
    assert_eq!(img2.screen_size, None);
}

// ---------------------------------------------------------------------------
// `encode_pcx_24bpp_screen` — screen-size-only writer
// ---------------------------------------------------------------------------

/// The screen-only writer stamps both screen-size words at offsets
/// 70 / 72 and leaves every other header field at the
/// `encode_pcx_24bpp` default — including DPI at 72×72 and origin at
/// `(0, 0)`.
#[test]
fn screen_writer_stamps_only_screen_size_fields() {
    let rgb = dummy_rgb(4, 2);
    let bytes = encode_pcx_24bpp_screen(4, 2, &rgb, (800, 600)).unwrap();
    assert_eq!(read_u16_le(&bytes, 4), 0); // x_min
    assert_eq!(read_u16_le(&bytes, 6), 0); // y_min
    assert_eq!(read_u16_le(&bytes, 12), 72); // h_dpi default
    assert_eq!(read_u16_le(&bytes, 14), 72); // v_dpi default
    assert_eq!(read_u16_le(&bytes, 70), 800);
    assert_eq!(read_u16_le(&bytes, 72), 600);
}

/// The screen-only writer self-roundtrips: decoding the output surfaces
/// `screen_size = Some(...)` with the original tuple. (DPI lands at
/// the historical 72×72 default because the screen-only writer leaves
/// it untouched, matching the plain `encode_pcx_24bpp` writer's
/// convention.)
#[test]
fn screen_writer_self_roundtrips_through_decoder() {
    let rgb = dummy_rgb(8, 8);
    let bytes = encode_pcx_24bpp_screen(8, 8, &rgb, (1024, 768)).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.screen_size, Some((1024, 768)));
    assert_eq!(img.dpi, Some((72, 72)));
    assert_eq!(img.window_origin, None);
    // Pixel data matches the input.
    for y in 0..8 {
        for x in 0..8 {
            let src = (y * 8 + x) * 3;
            let dst = (y * 8 + x) * 4;
            assert_eq!(img.data[dst], rgb[src]);
            assert_eq!(img.data[dst + 1], rgb[src + 1]);
            assert_eq!(img.data[dst + 2], rgb[src + 2]);
            assert_eq!(img.data[dst + 3], 0xFF);
        }
    }
}

/// Pixel-data invariance: every dimension permutation of the screen-
/// size tuple leaves the on-disk pixel RLE bytes (the bytes after the
/// 128-byte header) unchanged — only the two screen-size words at
/// offsets 70 / 72 differ.
#[test]
fn screen_size_doesnt_perturb_pixel_data() {
    let rgb = dummy_rgb(7, 5);
    let baseline = encode_pcx_24bpp_screen(7, 5, &rgb, (640, 480)).unwrap();
    let other = encode_pcx_24bpp_screen(7, 5, &rgb, (1024, 768)).unwrap();
    assert_eq!(baseline[PCX_HEADER_SIZE..], other[PCX_HEADER_SIZE..]);
    // Only bytes 70..74 (h_screen_size + v_screen_size) differ.
    for i in 0..PCX_HEADER_SIZE {
        if (70..74).contains(&i) {
            continue;
        }
        assert_eq!(
            baseline[i], other[i],
            "header byte {i} unexpectedly differs"
        );
    }
}

/// Zero-component screen-size tuples are rejected at the writer
/// boundary — per spec §3 a 0 means "unset" and emitting it would be
/// indistinguishable from the plain writer's default.
#[test]
fn screen_writer_rejects_zero_component() {
    let rgb = dummy_rgb(4, 2);
    match encode_pcx_24bpp_screen(4, 2, &rgb, (0, 480)) {
        Err(PcxError::InvalidData(_)) => {}
        other => panic!("expected Invalid for h=0, got {:?}", other.map(|v| v.len())),
    }
    match encode_pcx_24bpp_screen(4, 2, &rgb, (640, 0)) {
        Err(PcxError::InvalidData(_)) => {}
        other => panic!("expected Invalid for v=0, got {:?}", other.map(|v| v.len())),
    }
    match encode_pcx_24bpp_screen(4, 2, &rgb, (0, 0)) {
        Err(PcxError::InvalidData(_)) => {}
        other => panic!(
            "expected Invalid for (0,0), got {:?}",
            other.map(|v| v.len())
        ),
    }
}

// ---------------------------------------------------------------------------
// `encode_pcx_24bpp_window_dpi_screen` — combined writer
// ---------------------------------------------------------------------------

/// The maximally-tagged writer stamps all six metadata bytes
/// simultaneously: window origin (offsets 4 / 6), DPI (12 / 14), and
/// screen size (70 / 72).
#[test]
fn window_dpi_screen_writer_stamps_all_six_fields() {
    let rgb = dummy_rgb(4, 2);
    let bytes =
        encode_pcx_24bpp_window_dpi_screen(64, 128, 4, 2, &rgb, (300, 600), (1024, 768)).unwrap();
    assert_eq!(read_u16_le(&bytes, 4), 64);
    assert_eq!(read_u16_le(&bytes, 6), 128);
    assert_eq!(read_u16_le(&bytes, 8), 64 + 4 - 1);
    assert_eq!(read_u16_le(&bytes, 10), 128 + 2 - 1);
    assert_eq!(read_u16_le(&bytes, 12), 300);
    assert_eq!(read_u16_le(&bytes, 14), 600);
    assert_eq!(read_u16_le(&bytes, 70), 1024);
    assert_eq!(read_u16_le(&bytes, 72), 768);
}

/// Maximally-tagged writer self-roundtrips: decoding the output
/// surfaces every metadata `Option` as `Some(...)` with the original
/// values.
#[test]
fn window_dpi_screen_writer_self_roundtrips() {
    let rgb = dummy_rgb(4, 4);
    let bytes =
        encode_pcx_24bpp_window_dpi_screen(50, 100, 4, 4, &rgb, (300, 600), (640, 480)).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.window_origin, Some((50, 100)));
    assert_eq!(img.dpi, Some((300, 600)));
    assert_eq!(img.screen_size, Some((640, 480)));
}

/// Maximally-tagged writer rejects a zero-component screen-size tuple
/// (mirroring [`encode_pcx_24bpp_screen`]'s sentinel rule).
#[test]
fn window_dpi_screen_writer_rejects_zero_screen_size_component() {
    let rgb = dummy_rgb(4, 2);
    match encode_pcx_24bpp_window_dpi_screen(10, 20, 4, 2, &rgb, (300, 300), (0, 480)) {
        Err(PcxError::InvalidData(_)) => {}
        other => panic!(
            "expected Invalid for zero screen-size component, got {:?}",
            other.map(|v| v.len())
        ),
    }
}

/// Maximally-tagged writer rejects a zero-component DPI tuple (the
/// existing check_dpi path applies just as it does for
/// `encode_pcx_24bpp_window_dpi`).
#[test]
fn window_dpi_screen_writer_rejects_zero_dpi_component() {
    let rgb = dummy_rgb(4, 2);
    match encode_pcx_24bpp_window_dpi_screen(10, 20, 4, 2, &rgb, (0, 600), (640, 480)) {
        Err(PcxError::InvalidData(_)) => {}
        other => panic!(
            "expected Invalid for zero DPI component, got {:?}",
            other.map(|v| v.len())
        ),
    }
}

/// Maximally-tagged writer rejects an x_min + width overflow.
#[test]
fn window_dpi_screen_writer_rejects_origin_overflow() {
    let rgb = dummy_rgb(4, 2);
    match encode_pcx_24bpp_window_dpi_screen(u16::MAX - 2, 0, 4, 2, &rgb, (300, 300), (640, 480)) {
        Err(PcxError::InvalidData(_)) => {}
        other => panic!(
            "expected Invalid for origin overflow, got {:?}",
            other.map(|v| v.len())
        ),
    }
}

// ---------------------------------------------------------------------------
// `encode_pcx_24bpp_image` — wrapper dispatch across screen-size axis
// ---------------------------------------------------------------------------

/// Wrapper with `screen_size = None` and every other axis `None` →
/// plain 24bpp writer (bit-identical to the pre-r231 wrapper output).
#[test]
fn wrapper_no_metadata_matches_plain_writer() {
    let rgb = dummy_rgb(4, 2);
    let img = PcxImage {
        width: 4,
        height: 2,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb.clone(),
        pts: None,
        dpi: None,
        window_origin: None,
        screen_size: None,
    };
    let wrapped = encode_pcx_24bpp_image(&img).unwrap();
    let direct = encode_pcx_24bpp(4, 2, &rgb).unwrap();
    assert_eq!(wrapped, direct);
}

/// Wrapper with `screen_size = Some` and other axes `None` → forwards
/// to the screen-only writer with bit-identical bytes.
#[test]
fn wrapper_screen_only_uses_screen_writer() {
    let rgb = dummy_rgb(4, 2);
    let img = PcxImage {
        width: 4,
        height: 2,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb.clone(),
        pts: None,
        dpi: None,
        window_origin: None,
        screen_size: Some((1024, 768)),
    };
    let wrapped = encode_pcx_24bpp_image(&img).unwrap();
    let direct = encode_pcx_24bpp_screen(4, 2, &rgb, (1024, 768)).unwrap();
    assert_eq!(wrapped, direct);
    assert_eq!(read_u16_le(&wrapped, 70), 1024);
    assert_eq!(read_u16_le(&wrapped, 72), 768);
}

/// Wrapper with all three metadata fields `Some` → forwards to the
/// maximally-tagged writer with bit-identical bytes.
#[test]
fn wrapper_all_three_metadata_uses_combined_writer() {
    let rgb = dummy_rgb(4, 2);
    let img = PcxImage {
        width: 4,
        height: 2,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb.clone(),
        pts: None,
        dpi: Some((300, 300)),
        window_origin: Some((50, 100)),
        screen_size: Some((640, 480)),
    };
    let wrapped = encode_pcx_24bpp_image(&img).unwrap();
    let direct =
        encode_pcx_24bpp_window_dpi_screen(50, 100, 4, 2, &rgb, (300, 300), (640, 480)).unwrap();
    assert_eq!(wrapped, direct);
}

/// Wrapper preserves the pre-r231 four sub-cases bit-identically when
/// the new `screen_size` axis stays `None`.
#[test]
fn wrapper_pre_r231_cases_remain_bit_identical() {
    let rgb = dummy_rgb(4, 2);
    // (None, None, None)
    let img_a = PcxImage {
        width: 4,
        height: 2,
        pixel_format: PcxPixelFormat::Rgb24,
        data: rgb.clone(),
        pts: None,
        dpi: None,
        window_origin: None,
        screen_size: None,
    };
    assert_eq!(
        encode_pcx_24bpp_image(&img_a).unwrap(),
        encode_pcx_24bpp(4, 2, &rgb).unwrap()
    );
    // (Some, None, None)
    let img_b = PcxImage {
        window_origin: Some((50, 100)),
        ..img_a.clone()
    };
    assert_eq!(
        encode_pcx_24bpp_image(&img_b).unwrap(),
        encode_pcx_24bpp_window(50, 100, 4, 2, &rgb).unwrap()
    );
    // (None, Some, None)
    let img_c = PcxImage {
        dpi: Some((300, 300)),
        ..img_a.clone()
    };
    assert_eq!(
        encode_pcx_24bpp_image(&img_c).unwrap(),
        encode_pcx_24bpp_dpi(4, 2, &rgb, (300, 300)).unwrap()
    );
    // (Some, Some, None)
    let img_d = PcxImage {
        window_origin: Some((50, 100)),
        dpi: Some((300, 300)),
        ..img_a
    };
    assert_eq!(
        encode_pcx_24bpp_image(&img_d).unwrap(),
        encode_pcx_24bpp_window_dpi(50, 100, 4, 2, &rgb, (300, 300)).unwrap()
    );
}

/// End-to-end round-trip: a screen-size-tagged PCX makes one
/// decode → re-encode pass through `encode_pcx_24bpp_image` and the
/// screen-size annotation survives. (The DPI lands at the historical
/// 72×72 default the screen-only writer emits, so on decode the
/// wrapper sees `dpi = Some((72, 72))`; the round-trip preserves it
/// rather than dropping it back to `None`.)
#[test]
fn end_to_end_roundtrip_preserves_screen_size() {
    let rgb = dummy_rgb(5, 5);
    let bytes = encode_pcx_24bpp_screen(5, 5, &rgb, (1280, 1024)).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.screen_size, Some((1280, 1024)));
    assert_eq!(img.dpi, Some((72, 72)));
    let re = encode_pcx_24bpp_image(&img).unwrap();
    let img2 = parse_pcx(&re).unwrap();
    assert_eq!(img2.screen_size, Some((1280, 1024)));
    assert_eq!(img2.dpi, Some((72, 72)));
    assert_eq!(img2.window_origin, None);
}

/// End-to-end round-trip for the maximally-tagged input: window +
/// DPI + screen all survive through the wrapper.
#[test]
fn end_to_end_roundtrip_preserves_all_three_metadata_fields() {
    let rgb = dummy_rgb(6, 4);
    let bytes =
        encode_pcx_24bpp_window_dpi_screen(20, 40, 6, 4, &rgb, (300, 600), (1280, 1024)).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.window_origin, Some((20, 40)));
    assert_eq!(img.dpi, Some((300, 600)));
    assert_eq!(img.screen_size, Some((1280, 1024)));
    let re = encode_pcx_24bpp_image(&img).unwrap();
    let img2 = parse_pcx(&re).unwrap();
    assert_eq!(img2.window_origin, Some((20, 40)));
    assert_eq!(img2.dpi, Some((300, 600)));
    assert_eq!(img2.screen_size, Some((1280, 1024)));
}
