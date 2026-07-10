//! Round 405 — 1 bpp × 1 plane monochrome bit-polarity conformance,
//! pinned to the reference doc's errata (Issue #227, spec §"Palette
//! Information"): the bit value is used **directly as the colormap
//! index**, and `colormap[0]` = black / `colormap[1]` = white is the
//! standard monochrome-palette convention — so **bit 1 = white**
//! (foreground) and bit 0 = black.
//!
//! Both directions are pinned at the raw-byte level so a polarity
//! regression in either the writer or the reader cannot cancel out in
//! a round-trip test:
//!
//! * writer: the exact packed/RLE bytes and the exact colormap the
//!   mono writer puts on disk;
//! * reader: bit → colormap-index resolution for the black-on-white,
//!   white-on-black (inverted colormap), and zero-colormap cases;
//! * the `Mono1` auto-ladder rung, including the palette-order trap
//!   (white seen first must still emit bit 1, because polarity comes
//!   from the colour value, never from the first-seen palette index);
//! * the CGA interplay: a bilevel image whose "dark" colour is not
//!   pure black must skip `Mono1` and resolve exactly through a CGA
//!   rung's header-palette machinery instead;
//! * the framework registry rung: `MonoBlack` input and its
//!   bit-complemented `MonoWhite` twin must land on byte-identical
//!   spec-polarity files.

use oxideav_pcx::{encode_pcx_1bpp_mono, encode_pcx_rgb_auto, parse_pcx, PcxAutoMode};

const HDR: usize = 128;
const WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const BLACK: [u8; 4] = [0x00, 0x00, 0x00, 0xFF];

/// 8×1, left half white: pixels `[1, 1, 1, 1, 0, 0, 0, 0]`.
///
/// `bytes_per_line` = 2 (ceil(8 / 8) = 1, rounded up to even), so the
/// packed row is `[0xF0, 0x00]`: bit 1 = white in the four MSB-first
/// high bits. RLE: `0xF0` has its top two bits set and must be escaped
/// as a 1-run (`0xC1 0xF0`); the pad byte `0x00` stays literal.
const LEFT_WHITE_PIXELS: [u8; 8] = [1, 1, 1, 1, 0, 0, 0, 0];
const LEFT_WHITE_RLE: [u8; 3] = [0xC1, 0xF0, 0x00];

#[test]
fn mono_writer_pins_bit1_white_and_self_describing_colormap() {
    let bytes = encode_pcx_1bpp_mono(8, 1, &LEFT_WHITE_PIXELS).unwrap();
    assert_eq!(bytes.len(), HDR + LEFT_WHITE_RLE.len());
    assert_eq!(
        &bytes[HDR..],
        &LEFT_WHITE_RLE,
        "white input pixels must be emitted as bit value 1 (MSB-first)"
    );
    // The colormap self-description: entry 0 (bit 0) = black, entry 1
    // (bit 1) = white, remaining 14 entries zero.
    assert_eq!(
        &bytes[16..22],
        &[0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF],
        "colormap[0] must be black and colormap[1] white"
    );
    assert!(
        bytes[22..64].iter().all(|&b| b == 0),
        "unused colormap entries stay zero"
    );
}

#[test]
fn mono_decoder_uses_bit_as_colormap_index() {
    // Case 1 — writer's own colormap (black, white): bit 1 → white.
    let mut bytes = encode_pcx_1bpp_mono(8, 1, &LEFT_WHITE_PIXELS).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(&img.data[0..4], &WHITE, "bit 1 → colormap[1] = white");
    assert_eq!(&img.data[16..20], &BLACK, "bit 0 → colormap[0] = black");

    // Case 2 — inverted colormap (white, black), the exact polarity
    // question: the reader must keep resolving bit → index, so the
    // same bit pattern now decodes with swapped colours. A reader that
    // hard-codes bit 1 = white would pass case 1 and fail here.
    bytes[16..19].copy_from_slice(&[0xFF, 0xFF, 0xFF]); // entry 0 = white
    bytes[19..22].copy_from_slice(&[0x00, 0x00, 0x00]); // entry 1 = black
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(&img.data[0..4], &BLACK, "bit 1 → colormap[1] = black");
    assert_eq!(&img.data[16..20], &WHITE, "bit 0 → colormap[0] = white");

    // Case 3 — zero-filled colormap (common PCX 3.0+ form): falls back
    // to the classic convention, which is the same polarity the errata
    // pins (bit 1 = white).
    for b in bytes[16..64].iter_mut() {
        *b = 0;
    }
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(&img.data[0..4], &WHITE, "zero colormap: bit 1 = white");
    assert_eq!(&img.data[16..20], &BLACK, "zero colormap: bit 0 = black");
}

#[test]
fn mono_roundtrip_is_bit_exact_across_row_phases() {
    // Width 13: bits end mid-byte and `bytes_per_line` = 2, so both the
    // intra-byte cutoff and the even-padding byte are exercised. The
    // pattern differs per row so every bit phase appears.
    let (w, h) = (13u16, 5u16);
    let pixels: Vec<u8> = (0..w as usize * h as usize)
        .map(|i| {
            let (x, y) = (i % w as usize, i / w as usize);
            ((x * (y + 1) + y) % 3 == 0) as u8
        })
        .collect();
    let bytes = encode_pcx_1bpp_mono(w, h, &pixels).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!((img.width, img.height), (w as u32, h as u32));
    for (i, &p) in pixels.iter().enumerate() {
        let expect = if p == 1 { WHITE } else { BLACK };
        assert_eq!(&img.data[i * 4..i * 4 + 4], &expect, "pixel {i}");
    }
    // Re-encoding the decoded RGBA through the auto ladder must take
    // the Mono1 rung and land on the byte-identical file: same bits,
    // same colormap, no polarity drift anywhere in the loop.
    let rgb: Vec<u8> = img
        .data
        .chunks_exact(4)
        .flat_map(|c| c[..3].to_vec())
        .collect();
    let (auto_bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Mono1);
    assert_eq!(
        auto_bytes, bytes,
        "decode → auto re-encode must be byte-identical"
    );
}

#[test]
fn auto_ladder_mono1_rung_pins_bit1_white_on_disk() {
    // 8×2 packed RGB: row 0 = white×4 + black×4, row 1 = all black.
    let mut rgb = vec![0u8; 8 * 2 * 3];
    for px in 0..4 {
        rgb[px * 3..px * 3 + 3].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(8, 2, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Mono1);
    // Row 0 = [0xF0, 0x00] → C1 F0 00; row 1 = [0x00, 0x00] → C2 00.
    assert_eq!(&bytes[HDR..], &[0xC1, 0xF0, 0x00, 0xC2, 0x00]);
    assert_eq!(&bytes[16..22], &[0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF]);
}

#[test]
fn auto_ladder_mono1_polarity_ignores_first_seen_palette_order() {
    // White is the FIRST colour the raster scan sees, so white gets
    // first-seen palette index 0. Polarity must come from the colour
    // value (white → bit 1), never from the palette index — an
    // index-as-bit implementation would emit this file inverted.
    let mut rgb = vec![0xFFu8; 8 * 3];
    for px in 4..8 {
        rgb[px * 3..px * 3 + 3].copy_from_slice(&[0x00, 0x00, 0x00]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(8, 1, &rgb).unwrap();
    assert_eq!(mode, PcxAutoMode::Mono1);
    // Row = [0xF0, 0x00]: the white left half is the four high bits.
    assert_eq!(&bytes[HDR..], &LEFT_WHITE_RLE);
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(&img.data[0..4], &WHITE);
    assert_eq!(&img.data[16..20], &BLACK);
}

#[test]
fn bilevel_non_black_dark_colour_skips_mono1_and_stays_exact() {
    // White-on-blue: bilevel, but blue is not pure black, so the Mono1
    // rung's precondition fails and the ladder must fall through to a
    // palette-carrying rung (here CGA: {blue, white} is covered by
    // background = blue + the white-bright hardware palette). The
    // decoded colours must survive exactly — polarity conventions
    // never apply to a non-black/white pair.
    let mut rgb = vec![0u8; 8 * 3];
    for px in 0..4 {
        rgb[px * 3..px * 3 + 3].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    }
    for px in 4..8 {
        rgb[px * 3..px * 3 + 3].copy_from_slice(&[0x00, 0x00, 0xAA]);
    }
    let (bytes, mode) = encode_pcx_rgb_auto(8, 1, &rgb).unwrap();
    assert_ne!(
        mode,
        PcxAutoMode::Mono1,
        "blue is not black: Mono1 must not fire"
    );
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(&img.data[0..4], &WHITE);
    assert_eq!(&img.data[16..20], &[0x00, 0x00, 0xAA, 0xFF]);
}

#[cfg(feature = "registry")]
mod registry {
    use super::{HDR, LEFT_WHITE_RLE};
    use oxideav_core::{CodecId, CodecParameters, Frame, PixelFormat, VideoFrame, VideoPlane};
    use oxideav_pcx::encoder::make_encoder;

    fn encode_mono_frame(format: PixelFormat, row: u8) -> Vec<u8> {
        let mut params = CodecParameters::video(CodecId::new("pcx"));
        params.width = Some(8);
        params.height = Some(1);
        params.pixel_format = Some(format);
        let frame = VideoFrame {
            pts: Some(0),
            planes: vec![VideoPlane {
                stride: 1,
                data: vec![row],
            }],
        };
        let mut enc = make_encoder(&params).unwrap();
        enc.send_frame(&Frame::Video(frame)).unwrap();
        enc.receive_packet().unwrap().data
    }

    /// `MonoBlack` (0 = black) with bits `1111_0000` and `MonoWhite`
    /// (0 = white) with the complemented bits `0000_1111` describe the
    /// same image (left half white). Both must produce byte-identical
    /// files in the spec polarity: bit 1 = white on disk.
    #[test]
    fn monoblack_and_monowhite_twins_land_on_identical_spec_polarity_bytes() {
        let from_black = encode_mono_frame(PixelFormat::MonoBlack, 0b1111_0000);
        let from_white = encode_mono_frame(PixelFormat::MonoWhite, 0b0000_1111);
        assert_eq!(
            from_black, from_white,
            "the two mono conventions must converge on one on-disk polarity"
        );
        assert_eq!(&from_black[HDR..], &LEFT_WHITE_RLE);
        assert_eq!(&from_black[16..22], &[0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF]);
    }
}
