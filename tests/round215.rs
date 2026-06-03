//! r215 — 1 bpp × 3 planes (8-colour EGA RGB) decode + encode.
//!
//! The (1, 3) combination is a formally enumerated PCX video mode: per
//! the rev-5 technical reference §4 bit-plane example (lines 46–58),
//! when "more than one color plane is stored in the file, each line of
//! the image is stored by color plane (generally ordered red, green,
//! blue, intensity)", and `EGFF`'s PCX summary lists 3 planes / 1 bpp
//! as the 8-colour EGA RGB mode. With three planes the index bits map
//! to R / G / B and each channel is binary (0x00 or 0xFF), giving the
//! eight on/off primaries.

use oxideav_pcx::rle;
use oxideav_pcx::types::PCX_HEADER_SIZE;
use oxideav_pcx::{encode_pcx_1bpp_3planes_ega_rgb, parse_pcx, PcxError, PcxPixelFormat};

fn rgba_from_rgb(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for c in rgb.chunks_exact(3) {
        out.extend_from_slice(&[c[0], c[1], c[2], 0xFF]);
    }
    out
}

// ---------------------------------------------------------------------------
// Self-roundtrip across the eight on/off primaries
// ---------------------------------------------------------------------------

/// All 8 primaries laid out as a 1-row swatch of 8 pixels: black,
/// red, green, yellow, blue, magenta, cyan, white. Encode → parse →
/// compare bit-for-bit. Exercises every (R, G, B) bit combination.
#[test]
fn roundtrip_1bpp_3planes_all_eight_primaries() {
    // bit layout (R, G, B):  000 → black, 100 → red, …, 111 → white.
    let primaries: [[u8; 3]; 8] = [
        [0x00, 0x00, 0x00], // 000 black
        [0xFF, 0x00, 0x00], // R   red
        [0x00, 0xFF, 0x00], // G   green
        [0xFF, 0xFF, 0x00], // RG  yellow
        [0x00, 0x00, 0xFF], // B   blue
        [0xFF, 0x00, 0xFF], // RB  magenta
        [0x00, 0xFF, 0xFF], // GB  cyan
        [0xFF, 0xFF, 0xFF], // RGB white
    ];
    let w = 8u16;
    let h = 1u16;
    let mut rgb = Vec::with_capacity(24);
    for p in &primaries {
        rgb.extend_from_slice(p);
    }
    let bytes = encode_pcx_1bpp_3planes_ega_rgb(w, h, &rgb).unwrap();
    // Header sanity: PCX 5.0, RLE, 1 bpp, 3 planes.
    assert_eq!(bytes[0], 0x0A);
    assert_eq!(bytes[1], 5);
    assert_eq!(bytes[2], 1);
    assert_eq!(bytes[3], 1);
    assert_eq!(bytes[65], 3);
    // bytes_per_line at offset 66 (u16 LE) must be even and at least
    // ceil(w/8). For w=8 the natural ceil is 1, rounded to 2.
    let bpl = u16::from_le_bytes([bytes[66], bytes[67]]);
    assert_eq!(bpl, 2);

    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.width, w as u32);
    assert_eq!(img.height, h as u32);
    assert_eq!(img.pixel_format, PcxPixelFormat::Rgba);
    assert_eq!(img.data, rgba_from_rgb(&rgb));
}

/// 32×16 horizontal-stripe checker: every column gets a different
/// primary picked from a deterministic 8-cycle. Exercises both
/// multi-byte scanlines (32 px → 4 bytes per plane → padded to 4
/// already even) and multi-row layout (16 rows).
#[test]
fn roundtrip_1bpp_3planes_stripe_pattern() {
    let primaries: [[u8; 3]; 8] = [
        [0x00, 0x00, 0x00],
        [0xFF, 0x00, 0x00],
        [0x00, 0xFF, 0x00],
        [0xFF, 0xFF, 0x00],
        [0x00, 0x00, 0xFF],
        [0xFF, 0x00, 0xFF],
        [0x00, 0xFF, 0xFF],
        [0xFF, 0xFF, 0xFF],
    ];
    let w = 32u16;
    let h = 16u16;
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let idx = (x + y) % 8;
            rgb.extend_from_slice(&primaries[idx]);
        }
    }
    let bytes = encode_pcx_1bpp_3planes_ega_rgb(w, h, &rgb).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.width, w as u32);
    assert_eq!(img.height, h as u32);
    assert_eq!(img.data, rgba_from_rgb(&rgb));
}

/// Odd width forces `bytes_per_line` to round up to even per spec §1.
/// w = 7 → ceil(7/8) = 1 → rounded to 2. Verify the spare bit columns
/// inside the on-disk byte don't bleed into the decoded `width × 1`
/// visible region.
#[test]
fn roundtrip_1bpp_3planes_odd_width_pads_scanline() {
    let w = 7u16;
    let h = 3u16;
    // R, G, B, white, black, red, white.
    let row: [[u8; 3]; 7] = [
        [0xFF, 0x00, 0x00],
        [0x00, 0xFF, 0x00],
        [0x00, 0x00, 0xFF],
        [0xFF, 0xFF, 0xFF],
        [0x00, 0x00, 0x00],
        [0xFF, 0x00, 0x00],
        [0xFF, 0xFF, 0xFF],
    ];
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for _ in 0..h as usize {
        for p in &row {
            rgb.extend_from_slice(p);
        }
    }
    let bytes = encode_pcx_1bpp_3planes_ega_rgb(w, h, &rgb).unwrap();
    let bpl = u16::from_le_bytes([bytes[66], bytes[67]]);
    assert_eq!(bpl, 2, "expected bytes_per_line padded to even");
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.data, rgba_from_rgb(&rgb));
}

/// Encoder threshold rule: input bytes ≥ 0x80 set the plane bit,
/// bytes < 0x80 clear it. So intermediate values round to one of the
/// eight primaries on the round-trip. This is the documented behaviour
/// and lets callers pass non-pre-thresholded inputs.
#[test]
fn encode_thresholds_at_0x80() {
    // 4 input pixels: R-only-low, R-only-high, all-just-at-threshold, all-just-below.
    let rgb: [u8; 12] = [
        0x7F, 0x00, 0x00, // 0,0,0 → black (R below threshold)
        0x80, 0x00, 0x00, // R,0,0 → red (R at threshold = set)
        0x80, 0x80, 0x80, // R,G,B → white
        0x7F, 0x7F, 0x7F, // 0,0,0 → black
    ];
    let bytes = encode_pcx_1bpp_3planes_ega_rgb(4, 1, &rgb).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    // 4 pixels × 4 bytes RGBA each.
    assert_eq!(
        img.data,
        vec![
            0x00, 0x00, 0x00, 0xFF, // pixel 0 → black
            0xFF, 0x00, 0x00, 0xFF, // pixel 1 → red
            0xFF, 0xFF, 0xFF, 0xFF, // pixel 2 → white
            0x00, 0x00, 0x00, 0xFF, // pixel 3 → black
        ]
    );
}

/// Decode a hand-rolled (1, 3) byte stream where the RLE-encoded
/// pixel data is constructed independently of the encoder. Guards
/// against the decoder mirroring a same-direction encoder bug.
#[test]
fn parse_handcrafted_1bpp_3planes() {
    // 8 pixels wide × 1 row tall, displaying R, G, B, white, then four
    // blacks. R plane = 10001000 (0x88), G plane = 01001000 (0x48),
    // B plane = 00101000 (0x28).
    //
    // Wait — we want 4-pixel-wide black trailing. Recompute:
    //  pixel | R G B
    //   0    | 1 0 0   (red)
    //   1    | 0 1 0   (green)
    //   2    | 0 0 1   (blue)
    //   3    | 1 1 1   (white)
    //   4..7 | 0 0 0   (black)
    //
    // R-plane bits MSB-first across pixels 0..7: 10010000 = 0x90
    // G-plane bits:                              01010000 = 0x50
    // B-plane bits:                              00110000 = 0x30
    let w = 8u16;
    let h = 1u16;
    let bpl: u16 = 2; // ceil(8/8)=1 → padded to 2
    let mut bytes = Vec::with_capacity(PCX_HEADER_SIZE + 64);
    bytes.push(0x0A); // manufacturer 0
    bytes.push(5); // version 1
    bytes.push(1); // RLE 2
    bytes.push(1); // bits_per_pixel 3
    bytes.extend_from_slice(&0u16.to_le_bytes()); // x_min 4
    bytes.extend_from_slice(&0u16.to_le_bytes()); // y_min 6
    bytes.extend_from_slice(&(w - 1).to_le_bytes()); // x_max 8
    bytes.extend_from_slice(&(h - 1).to_le_bytes()); // y_max 10
    bytes.extend_from_slice(&72u16.to_le_bytes()); // h_dpi 12
    bytes.extend_from_slice(&72u16.to_le_bytes()); // v_dpi 14
    bytes.extend_from_slice(&[0u8; 48]); // ega_palette 16
    bytes.push(0); // reserved 64
    bytes.push(3); // n_planes 65
    bytes.extend_from_slice(&bpl.to_le_bytes()); // bytes_per_line 66
    bytes.extend_from_slice(&1u16.to_le_bytes()); // palette_info 68
    bytes.extend_from_slice(&[0u8; 58]); // pad to 128

    // Build the planar uncompressed row: 6 bytes total. Each plane is
    // bpl=2 bytes: data byte followed by zero pad.
    let planar_row: [u8; 6] = [
        0x90, 0x00, // R plane (with pad)
        0x50, 0x00, // G plane (with pad)
        0x30, 0x00, // B plane (with pad)
    ];
    rle::encode(&planar_row, &mut bytes);

    let img = parse_pcx(&bytes).unwrap();
    let mut want = Vec::with_capacity(8 * 4);
    want.extend_from_slice(&[0xFF, 0x00, 0x00, 0xFF]); // red
    want.extend_from_slice(&[0x00, 0xFF, 0x00, 0xFF]); // green
    want.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]); // blue
    want.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // white
    for _ in 0..4 {
        want.extend_from_slice(&[0x00, 0x00, 0x00, 0xFF]); // black trailing
    }
    assert_eq!(img.width, 8);
    assert_eq!(img.height, 1);
    assert_eq!(img.data, want);
}

// ---------------------------------------------------------------------------
// Error / edge cases
// ---------------------------------------------------------------------------

#[test]
fn encode_rejects_zero_dim() {
    assert!(matches!(
        encode_pcx_1bpp_3planes_ega_rgb(0, 1, &[0u8; 3]),
        Err(PcxError::InvalidData(_))
    ));
    assert!(matches!(
        encode_pcx_1bpp_3planes_ega_rgb(1, 0, &[0u8; 3]),
        Err(PcxError::InvalidData(_))
    ));
}

#[test]
fn encode_rejects_short_input() {
    // Need w*h*3 = 4*2*3 = 24 bytes, supply only 12.
    assert!(matches!(
        encode_pcx_1bpp_3planes_ega_rgb(4, 2, &[0u8; 12]),
        Err(PcxError::InvalidData(_))
    ));
}
