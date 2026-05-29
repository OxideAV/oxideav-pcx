//! Round 185 — `oxideav_core::Encoder` accepts `PixelFormat::Bgr24`,
//! `Bgra`, `MonoBlack`, and `MonoWhite` video frames in addition to
//! the round-88 surface (`Rgba` / `Rgb24` / `Gray8`).
//!
//! `Bgr24` / `Bgra` route to the 24-bit RGB writer with a per-pixel
//! byte swap (and alpha drop for `Bgra`); `MonoBlack` / `MonoWhite`
//! route to `encode_pcx_1bpp_mono` after unpacking the MSB-first
//! 1-bit stride into one byte per pixel (and inverting for the
//! `MonoWhite` polarity per the `oxideav-core` convention).

#![cfg(feature = "registry")]

use oxideav_core::{
    CodecId, CodecParameters, Frame, MediaType, PixelFormat, VideoFrame, VideoPlane,
};

use oxideav_pcx::encoder::make_encoder;
use oxideav_pcx::{parse_pcx, PcxPixelFormat};

fn params_for(width: u32, height: u32, format: PixelFormat) -> CodecParameters {
    let mut params = CodecParameters::video(CodecId::new("pcx"));
    params.width = Some(width);
    params.height = Some(height);
    params.pixel_format = Some(format);
    params
}

fn make_packed_frame(
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
    fill: impl Fn(u32, u32) -> Vec<u8>,
) -> VideoFrame {
    let stride = width as usize * bytes_per_pixel;
    let mut data = vec![0u8; stride * height as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let v = fill(x as u32, y as u32);
            assert_eq!(v.len(), bytes_per_pixel);
            let off = y * stride + x * bytes_per_pixel;
            data[off..off + bytes_per_pixel].copy_from_slice(&v);
        }
    }
    VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane { stride, data }],
    }
}

#[test]
fn framework_encoder_accepts_bgr24_frame_with_byte_swap() {
    let params = params_for(6, 3, PixelFormat::Bgr24);
    let mut enc = make_encoder(&params).unwrap();
    // Distinctive per-pixel BGR values so the swap is observable.
    let frame = make_packed_frame(6, 3, 3, |x, y| {
        let r = (x * 30) as u8;
        let g = (y * 60) as u8;
        let b = ((x + y) * 20) as u8;
        // BGR memory order on the source plane.
        vec![b, g, r]
    });
    enc.send_frame(&Frame::Video(frame)).unwrap();
    let pkt = enc.receive_packet().unwrap();
    let img = parse_pcx(&pkt.data).unwrap();
    assert_eq!(img.width, 6);
    assert_eq!(img.height, 3);
    assert_eq!(img.pixel_format, PcxPixelFormat::Rgba);
    for y in 0..3usize {
        for x in 0..6usize {
            let off = (y * 6 + x) * 4;
            let r = (x as u32 * 30) as u8;
            let g = (y as u32 * 60) as u8;
            let b = ((x as u32 + y as u32) * 20) as u8;
            // After BGR -> RGB byte-swap inside the encoder, the
            // decoded image must surface the original (r, g, b).
            assert_eq!(
                &img.data[off..off + 4],
                &[r, g, b, 0xFF],
                "Bgr24 ({x},{y}) expected ({r},{g},{b}) after swap"
            );
        }
    }
    // bits_per_pixel = 8, n_planes = 3 (24-bit PCX).
    assert_eq!(pkt.data[3], 8);
    assert_eq!(pkt.data[65], 3);
}

#[test]
fn framework_encoder_accepts_bgra_frame_drops_alpha_and_swaps() {
    let params = params_for(4, 4, PixelFormat::Bgra);
    let mut enc = make_encoder(&params).unwrap();
    let frame = make_packed_frame(4, 4, 4, |x, y| {
        let r = (10 + x * 50) as u8;
        let g = (20 + y * 50) as u8;
        let b = (30 + (x ^ y) * 50) as u8;
        let a = 0x77; // distinctive alpha; the encoder must drop this
        vec![b, g, r, a]
    });
    enc.send_frame(&Frame::Video(frame)).unwrap();
    let pkt = enc.receive_packet().unwrap();
    let img = parse_pcx(&pkt.data).unwrap();
    for y in 0..4usize {
        for x in 0..4usize {
            let off = (y * 4 + x) * 4;
            let r = (10 + x as u32 * 50) as u8;
            let g = (20 + y as u32 * 50) as u8;
            let b = (30 + (x as u32 ^ y as u32) * 50) as u8;
            assert_eq!(
                &img.data[off..off + 4],
                &[r, g, b, 0xFF],
                "Bgra ({x},{y}) expected ({r},{g},{b}) after swap+drop"
            );
        }
    }
}

#[test]
fn framework_encoder_accepts_monoblack_frame() {
    // 8-pixel-wide checkerboard so we can stride one byte per row.
    let width = 8u32;
    let height = 3u32;
    let stride = 1usize;
    // MonoBlack: 0 = black, 1 = white. Pattern: alternating bits MSB first
    // per row, slightly different per row.
    let mut data = vec![0u8; stride * height as usize];
    data[0] = 0b1010_1010;
    data[1] = 0b1100_1100;
    data[2] = 0b1111_0000;
    let frame = VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane { stride, data }],
    };
    let params = params_for(width, height, PixelFormat::MonoBlack);
    let mut enc = make_encoder(&params).unwrap();
    enc.send_frame(&Frame::Video(frame)).unwrap();
    let pkt = enc.receive_packet().unwrap();
    // bits_per_pixel = 1, n_planes = 1.
    assert_eq!(pkt.data[3], 1);
    assert_eq!(pkt.data[65], 1);
    let img = parse_pcx(&pkt.data).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
    // Decoder convention: bit 1 = white (0xFF triple), bit 0 = black
    // (0x00 triple). MonoBlack passes through unchanged.
    let pattern = [0b1010_1010u8, 0b1100_1100u8, 0b1111_0000u8];
    for (y, &row_bits) in pattern.iter().enumerate() {
        for x in 0..width as usize {
            let bit = (row_bits >> (7 - x)) & 1;
            let off = (y * width as usize + x) * 4;
            let expected = if bit == 1 { 0xFF } else { 0x00 };
            assert_eq!(
                &img.data[off..off + 4],
                &[expected, expected, expected, 0xFF],
                "MonoBlack ({x},{y}) bit={bit} expected {expected:02X}"
            );
        }
    }
}

#[test]
fn framework_encoder_accepts_monowhite_frame_inverts_polarity() {
    // Same source bit pattern as the MonoBlack test, but tagged as
    // MonoWhite (0 = white, 1 = black). The decoded output should be
    // the inverse.
    let width = 8u32;
    let height = 1u32;
    let mut data = vec![0u8; 1];
    data[0] = 0b1010_0000;
    let frame = VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane { stride: 1, data }],
    };
    let params = params_for(width, height, PixelFormat::MonoWhite);
    let mut enc = make_encoder(&params).unwrap();
    enc.send_frame(&Frame::Video(frame)).unwrap();
    let pkt = enc.receive_packet().unwrap();
    let img = parse_pcx(&pkt.data).unwrap();
    // Source bit 1 should decode to BLACK (inverted), bit 0 to WHITE.
    let expected_per_pixel = [0x00u8, 0xFF, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
    for (x, &v) in expected_per_pixel.iter().enumerate() {
        let off = x * 4;
        assert_eq!(
            &img.data[off..off + 4],
            &[v, v, v, 0xFF],
            "MonoWhite x={x} expected {v:02X}"
        );
    }
}

#[test]
fn framework_encoder_mono_handles_non_tight_stride_and_padding() {
    // Width 5 → ceil(5/8) = 1 byte per row, but provide stride 4 (lots
    // of padding) and pad bits past width with garbage. The encoder
    // must ignore both.
    let width = 5u32;
    let height = 2u32;
    let stride = 4usize;
    let mut data = vec![0xFFu8; stride * height as usize];
    // Row 0: pixels (1,0,1,1,0)  = 0b10110xxx; top 5 bits = 10110.
    // Padding 3 LSBs (111) must be ignored by the encoder.
    data[0] = 0b1011_0111;
    // Row 1: pixels (0,1,0,1,0).
    data[stride] = 0b0101_0111;
    let frame = VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane { stride, data }],
    };
    let params = params_for(width, height, PixelFormat::MonoBlack);
    let mut enc = make_encoder(&params).unwrap();
    enc.send_frame(&Frame::Video(frame)).unwrap();
    let pkt = enc.receive_packet().unwrap();
    let img = parse_pcx(&pkt.data).unwrap();
    let expected_rows = [[1u8, 0, 1, 1, 0], [0, 1, 0, 1, 0]];
    for (y, row) in expected_rows.iter().enumerate() {
        for (x, &bit) in row.iter().enumerate() {
            let off = (y * 5 + x) * 4;
            let v = if bit == 1 { 0xFF } else { 0x00 };
            assert_eq!(
                &img.data[off..off + 4],
                &[v, v, v, 0xFF],
                "stride-padded MonoBlack ({x},{y}) expected {v:02X}"
            );
        }
    }
}

#[test]
fn framework_encoder_capability_advertises_new_formats() {
    let mut registry = oxideav_core::CodecRegistry::new();
    oxideav_pcx::register_codecs(&mut registry);
    let impls = registry.implementations(&CodecId::new("pcx"));
    assert!(!impls.is_empty());
    let caps = &impls[0].caps;
    assert_eq!(caps.media_type, MediaType::Video);
    for f in [
        PixelFormat::Rgba,
        PixelFormat::Rgb24,
        PixelFormat::Bgr24,
        PixelFormat::Bgra,
        PixelFormat::Gray8,
        PixelFormat::MonoBlack,
        PixelFormat::MonoWhite,
    ] {
        assert!(
            caps.accepted_pixel_formats.contains(&f),
            "pcx capabilities should advertise {f:?}"
        );
    }
}

#[test]
fn framework_encoder_rejects_format_with_zero_planes() {
    // Sanity: empty `planes` Vec still yields the typed error.
    let params = params_for(2, 2, PixelFormat::Bgr24);
    let mut enc = make_encoder(&params).unwrap();
    let frame = VideoFrame {
        pts: Some(0),
        planes: vec![],
    };
    let res = enc.send_frame(&Frame::Video(frame));
    assert!(res.is_err(), "empty planes Vec should error");
}

#[test]
fn framework_encoder_rejects_undersized_packed_data() {
    // Stride smaller than width × bpp → error rather than panic.
    let width = 4u32;
    let height = 2u32;
    let stride = 6usize; // expected width(4) * 3 = 12 bytes per row
    let data = vec![0u8; stride * height as usize];
    let frame = VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane { stride, data }],
    };
    let params = params_for(width, height, PixelFormat::Bgr24);
    let mut enc = make_encoder(&params).unwrap();
    let res = enc.send_frame(&Frame::Video(frame));
    assert!(res.is_err(), "stride smaller than required must error");
}
