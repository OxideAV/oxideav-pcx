//! Round 88 — `oxideav_core::Encoder` accepts `PixelFormat::Gray8`
//! frames and routes them to the round-82 `encode_pcx_8bpp_grayscale`
//! writer (8 bpp × 1 plane PCX 5.0 with `palette_info = 2` set per
//! spec §3 and no VGA tail palette appended).

#![cfg(feature = "registry")]

use oxideav_core::{
    CodecId, CodecParameters, Frame, MediaType, PixelFormat, VideoFrame, VideoPlane,
};

use oxideav_pcx::encoder::make_encoder;
use oxideav_pcx::types::PCX_HEADER_SIZE;
use oxideav_pcx::{parse_pcx, PcxPixelFormat};

fn make_gray8_frame(width: u32, height: u32, fill: impl Fn(u32, u32) -> u8) -> VideoFrame {
    let stride = width as usize;
    let mut data = vec![0u8; stride * height as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            data[y * stride + x] = fill(x as u32, y as u32);
        }
    }
    VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane { stride, data }],
    }
}

fn params_for(width: u32, height: u32, format: PixelFormat) -> CodecParameters {
    let mut params = CodecParameters::video(CodecId::new("pcx"));
    params.width = Some(width);
    params.height = Some(height);
    params.pixel_format = Some(format);
    params
}

#[test]
fn framework_encoder_accepts_gray8_frame() {
    let params = params_for(8, 4, PixelFormat::Gray8);
    let mut enc = make_encoder(&params).unwrap();
    let frame = make_gray8_frame(8, 4, |x, y| (x * 16 + y * 4) as u8);
    enc.send_frame(&Frame::Video(frame)).unwrap();
    let pkt = enc.receive_packet().unwrap();
    // The emitted PCX must carry the grayscale flag at offset 68.
    let palette_info = u16::from_le_bytes([pkt.data[68], pkt.data[69]]);
    assert_eq!(palette_info, 2, "Gray8 encode must set palette_info = 2");
    // bits_per_pixel = 8, n_planes = 1.
    assert_eq!(pkt.data[3], 8);
    assert_eq!(pkt.data[65], 1);
    // No tail VGA palette: total length stays small (header + a handful
    // of RLE bytes).
    assert!(
        pkt.data.len() < PCX_HEADER_SIZE + 80,
        "Gray8 encode must not append a VGA tail palette; got {} bytes",
        pkt.data.len()
    );
    assert!(pkt.flags.keyframe);
}

#[test]
fn framework_encoder_gray8_roundtrip_pixel_exact() {
    let params = params_for(12, 5, PixelFormat::Gray8);
    let mut enc = make_encoder(&params).unwrap();
    let frame = make_gray8_frame(12, 5, |x, y| ((x * 19 + y * 53) & 0xFF) as u8);
    enc.send_frame(&Frame::Video(frame.clone())).unwrap();
    let pkt = enc.receive_packet().unwrap();
    // Decode back through the standalone parser and compare per-pixel.
    let img = parse_pcx(&pkt.data).unwrap();
    assert_eq!(img.width, 12);
    assert_eq!(img.height, 5);
    assert_eq!(img.pixel_format, PcxPixelFormat::Rgba);
    let plane = &frame.planes[0];
    for y in 0..5usize {
        for x in 0..12usize {
            let v = plane.data[y * plane.stride + x];
            let off = (y * 12 + x) * 4;
            assert_eq!(
                &img.data[off..off + 4],
                &[v, v, v, 0xFF],
                "Gray8 pixel ({x},{y}) v={v} should decode as ({v},{v},{v},255)"
            );
        }
    }
}

#[test]
fn framework_encoder_gray8_honours_non_tight_stride() {
    // Stride strictly greater than width — the encoder must take only
    // the first `width` bytes of each row.
    let width = 6u32;
    let height = 3u32;
    let stride = 10usize;
    let mut data = vec![0u8; stride * height as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            data[y * stride + x] = (x * 30 + y * 7) as u8;
        }
        // Pollute padding so a stride bug would leak into the encoded
        // output and the roundtrip check would fail.
        for x in width as usize..stride {
            data[y * stride + x] = 0xC0;
        }
    }
    let frame = VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane { stride, data }],
    };
    let params = params_for(width, height, PixelFormat::Gray8);
    let mut enc = make_encoder(&params).unwrap();
    enc.send_frame(&Frame::Video(frame)).unwrap();
    let pkt = enc.receive_packet().unwrap();
    let img = parse_pcx(&pkt.data).unwrap();
    for y in 0..height as usize {
        for x in 0..width as usize {
            let v = (x * 30 + y * 7) as u8;
            let off = (y * width as usize + x) * 4;
            assert_eq!(
                &img.data[off..off + 4],
                &[v, v, v, 0xFF],
                "stride-padded Gray8 ({x},{y}) should still roundtrip"
            );
        }
    }
}

#[test]
fn framework_encoder_capability_advertises_gray8() {
    // Build a registry and confirm pcx_sw caps include Gray8.
    let mut registry = oxideav_core::CodecRegistry::new();
    oxideav_pcx::register_codecs(&mut registry);
    let impls = registry.implementations(&CodecId::new("pcx"));
    assert!(!impls.is_empty(), "pcx codec should be registered");
    let caps = &impls[0].caps;
    assert_eq!(caps.media_type, MediaType::Video);
    assert!(
        caps.accepted_pixel_formats.contains(&PixelFormat::Gray8),
        "pcx capabilities should advertise Gray8 since the encoder accepts it; got {:?}",
        caps.accepted_pixel_formats
    );
    assert!(caps.accepted_pixel_formats.contains(&PixelFormat::Rgba));
    assert!(caps.accepted_pixel_formats.contains(&PixelFormat::Rgb24));
}
