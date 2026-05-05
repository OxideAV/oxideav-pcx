//! Round 2 — write paths for the indexed/EGA/CGA/mono combos plus the
//! 2 bpp + 4 bpp packed-bits decoder additions, plus DCX multi-page
//! container demux + mux roundtrips.

use oxideav_pcx::{
    encode_dcx, encode_pcx_1bpp_4planes_ega, encode_pcx_1bpp_mono, encode_pcx_24bpp,
    encode_pcx_2bpp_cga, encode_pcx_4bpp_packed, parse_dcx, parse_pcx, DCX_MAGIC, DCX_MAX_PAGES,
};

// ---------------------------------------------------------------------------
// 1-bpp mono writer self-roundtrip
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_1bpp_mono_writer() {
    // 8x2 image: row 0 = alternating, row 1 = 4 white then 4 black.
    let pixels = vec![
        1, 0, 1, 0, 1, 0, 1, 0, // row 0
        1, 1, 1, 1, 0, 0, 0, 0, // row 1
    ];
    let bytes = encode_pcx_1bpp_mono(8, 2, &pixels).unwrap();
    assert_eq!(bytes[3], 1, "bits per pixel should be 1");
    assert_eq!(bytes[65], 1, "n_planes should be 1");
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.width, 8);
    assert_eq!(img.height, 2);
    // White = 0xFF, black = 0x00.
    for (x, &p) in pixels.iter().take(8).enumerate() {
        let exp = if p != 0 { 0xFF } else { 0x00 };
        assert_eq!(img.data[x * 4], exp, "row 0 pixel {x} should be {:#x}", exp);
    }
    for (x, &p) in pixels.iter().skip(8).take(8).enumerate() {
        let exp = if p != 0 { 0xFF } else { 0x00 };
        assert_eq!(
            img.data[(8 + x) * 4],
            exp,
            "row 1 pixel {x} should be {:#x}",
            exp
        );
    }
}

#[test]
fn roundtrip_1bpp_mono_odd_width() {
    // Width 5 forces ceil(5/8) = 1 byte/line, then rounded up to 2 even.
    let pixels = vec![1u8, 0, 1, 0, 1];
    let bytes = encode_pcx_1bpp_mono(5, 1, &pixels).unwrap();
    let bpl = u16::from_le_bytes([bytes[66], bytes[67]]);
    assert_eq!(bpl, 2, "bytes_per_line should be padded to even");
    let img = parse_pcx(&bytes).unwrap();
    for (x, &p) in pixels.iter().take(5).enumerate() {
        let exp = if p != 0 { 0xFF } else { 0x00 };
        assert_eq!(img.data[x * 4], exp);
    }
}

// ---------------------------------------------------------------------------
// 4 bpp × 1 plane packed-bits decoder + writer roundtrip
// ---------------------------------------------------------------------------

fn ega_palette_explicit() -> Vec<u8> {
    // 16 distinct RGB triplets so the roundtrip can verify each entry.
    let mut p = Vec::with_capacity(48);
    for i in 0..16u8 {
        p.push(i * 17);
        p.push(255 - i * 17);
        p.push(i.wrapping_mul(31));
    }
    p
}

#[test]
fn roundtrip_4bpp_packed_writer_and_decoder() {
    let palette = ega_palette_explicit();
    // 5x3: each pixel is a different palette index covering 0..15 plus
    // a wrap so the test catches off-by-ones in nibble packing.
    let w = 5u16;
    let h = 3u16;
    let mut indices = Vec::with_capacity(w as usize * h as usize);
    for i in 0..(w as usize * h as usize) {
        indices.push((i % 16) as u8);
    }
    let bytes = encode_pcx_4bpp_packed(w, h, &indices, &palette).unwrap();
    assert_eq!(bytes[3], 4);
    assert_eq!(bytes[65], 1);
    // bytes_per_line = ceil(5/2) = 3, padded even = 4
    let bpl = u16::from_le_bytes([bytes[66], bytes[67]]);
    assert_eq!(bpl, 4);
    let img = parse_pcx(&bytes).unwrap();
    for (i, &idx) in indices.iter().enumerate() {
        let off = idx as usize * 3;
        let want = [palette[off], palette[off + 1], palette[off + 2], 0xFF];
        assert_eq!(img.data[i * 4..i * 4 + 4], want, "pixel {i} idx {idx}");
    }
}

#[test]
fn roundtrip_4bpp_packed_solid_compresses() {
    let palette = ega_palette_explicit();
    let indices = vec![7u8; 100 * 100];
    let bytes = encode_pcx_4bpp_packed(100, 100, &indices, &palette).unwrap();
    assert!(
        bytes.len() < 100 * 100 / 8 + 256,
        "solid 100x100 4bpp should crush, got {} bytes",
        bytes.len()
    );
    let img = parse_pcx(&bytes).unwrap();
    let want = [palette[7 * 3], palette[7 * 3 + 1], palette[7 * 3 + 2], 0xFF];
    for px in img.data.chunks_exact(4) {
        assert_eq!(px, &want);
    }
}

// ---------------------------------------------------------------------------
// 2 bpp × 1 plane CGA decoder + writer roundtrip
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_2bpp_cga_palette_1_high() {
    // Default selector 0x00 → palette 1 high-intensity (cyan/magenta/white).
    // Palette: bg = black, 1 = light cyan, 2 = light magenta, 3 = white.
    let w = 8u16;
    let h = 1u16;
    let indices = vec![0u8, 1, 2, 3, 0, 1, 2, 3];
    let bytes = encode_pcx_2bpp_cga(w, h, &indices, 0x00, 0).unwrap();
    assert_eq!(bytes[3], 2);
    assert_eq!(bytes[65], 1);
    let img = parse_pcx(&bytes).unwrap();
    let expected: [[u8; 3]; 4] = [
        [0x00, 0x00, 0x00],
        [0x55, 0xFF, 0xFF],
        [0xFF, 0x55, 0xFF],
        [0xFF, 0xFF, 0xFF],
    ];
    for (i, &idx) in indices.iter().enumerate() {
        let want = expected[idx as usize];
        assert_eq!(
            img.data[i * 4..i * 4 + 4],
            [want[0], want[1], want[2], 0xFF],
            "pixel {i}"
        );
    }
}

#[test]
fn roundtrip_2bpp_cga_palette_0_low_with_bg() {
    // Selector 0xC0 = palette 0 low-intensity (green/red/brown). bg = blue (1).
    let w = 4u16;
    let h = 1u16;
    let indices = vec![0u8, 1, 2, 3];
    let bytes = encode_pcx_2bpp_cga(w, h, &indices, 0xC0, 1).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    let expected: [[u8; 3]; 4] = [
        [0x00, 0x00, 0xAA], // bg = blue (EGA index 1)
        [0x00, 0xAA, 0x00], // green
        [0xAA, 0x00, 0x00], // red
        [0xAA, 0x55, 0x00], // brown
    ];
    for (i, &idx) in indices.iter().enumerate() {
        let want = expected[idx as usize];
        assert_eq!(
            img.data[i * 4..i * 4 + 4],
            [want[0], want[1], want[2], 0xFF],
            "pixel {i}"
        );
    }
}

#[test]
fn roundtrip_2bpp_cga_odd_width_padding() {
    // Width 5 → ceil(5/4) = 2, padded even = 2.
    let indices = vec![0u8, 1, 2, 3, 0];
    let bytes = encode_pcx_2bpp_cga(5, 1, &indices, 0x00, 0).unwrap();
    let bpl = u16::from_le_bytes([bytes[66], bytes[67]]);
    assert_eq!(bpl, 2);
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.width, 5);
}

// ---------------------------------------------------------------------------
// 1 bpp × 4 planes EGA writer + decoder roundtrip
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_1bpp_4planes_ega_writer() {
    let palette = ega_palette_explicit();
    // 16x1 image: each pixel = a different EGA index 0..15.
    let w = 16u16;
    let h = 1u16;
    let mut indices = Vec::with_capacity(16);
    for i in 0..16u8 {
        indices.push(i);
    }
    let bytes = encode_pcx_1bpp_4planes_ega(w, h, &indices, &palette).unwrap();
    assert_eq!(bytes[3], 1);
    assert_eq!(bytes[65], 4);
    let img = parse_pcx(&bytes).unwrap();
    for (i, &idx) in indices.iter().enumerate() {
        let off = idx as usize * 3;
        let want = [palette[off], palette[off + 1], palette[off + 2], 0xFF];
        assert_eq!(img.data[i * 4..i * 4 + 4], want, "pixel {i} idx {idx}");
    }
}

#[test]
fn roundtrip_1bpp_4planes_ega_multirow() {
    let palette = ega_palette_explicit();
    let w = 24u16;
    let h = 4u16;
    let mut indices = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h {
        for x in 0..w {
            indices.push((x as u8 + y as u8) % 16);
        }
    }
    let bytes = encode_pcx_1bpp_4planes_ega(w, h, &indices, &palette).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    for (i, &idx) in indices.iter().enumerate() {
        let off = idx as usize * 3;
        let want = [palette[off], palette[off + 1], palette[off + 2], 0xFF];
        assert_eq!(img.data[i * 4..i * 4 + 4], want, "pixel {i} idx {idx}");
    }
}

// ---------------------------------------------------------------------------
// DCX multi-page container roundtrip
// ---------------------------------------------------------------------------

fn solid_24bpp(w: u16, h: u16, rgb: [u8; 3]) -> Vec<u8> {
    let mut data = Vec::with_capacity(w as usize * h as usize * 3);
    for _ in 0..(w as usize * h as usize) {
        data.extend_from_slice(&rgb);
    }
    encode_pcx_24bpp(w, h, &data).unwrap()
}

#[test]
fn dcx_roundtrip_three_pages() {
    let p1 = solid_24bpp(8, 8, [255, 0, 0]);
    let p2 = solid_24bpp(8, 8, [0, 255, 0]);
    let p3 = solid_24bpp(8, 8, [0, 0, 255]);
    let dcx = encode_dcx(&[p1, p2, p3]).unwrap();
    // Magic.
    assert_eq!(
        u32::from_le_bytes([dcx[0], dcx[1], dcx[2], dcx[3]]),
        DCX_MAGIC
    );
    let parsed = parse_dcx(&dcx).unwrap();
    assert_eq!(parsed.pages.len(), 3);
    assert_eq!(parsed.pages[0].width, 8);
    // First pixel of each page is the solid colour.
    assert_eq!(&parsed.pages[0].data[..3], &[255, 0, 0]);
    assert_eq!(&parsed.pages[1].data[..3], &[0, 255, 0]);
    assert_eq!(&parsed.pages[2].data[..3], &[0, 0, 255]);
}

#[test]
fn dcx_roundtrip_one_page() {
    let p1 = solid_24bpp(4, 4, [128, 64, 32]);
    let dcx = encode_dcx(&[p1]).unwrap();
    let parsed = parse_dcx(&dcx).unwrap();
    assert_eq!(parsed.pages.len(), 1);
    assert_eq!(parsed.pages[0].width, 4);
    assert_eq!(parsed.pages[0].height, 4);
    assert_eq!(&parsed.pages[0].data[..3], &[128, 64, 32]);
}

#[test]
fn dcx_rejects_bad_magic() {
    let mut dcx = encode_dcx(&[solid_24bpp(2, 2, [0, 0, 0])]).unwrap();
    dcx[0] = 0xDE;
    dcx[1] = 0xAD;
    assert!(parse_dcx(&dcx).is_err());
}

#[test]
fn dcx_rejects_too_short() {
    assert!(parse_dcx(&[1u8, 2, 3]).is_err());
}

#[test]
fn dcx_rejects_zero_pages() {
    assert!(encode_dcx(&[]).is_err());
}

#[test]
fn dcx_rejects_too_many_pages() {
    let bogus: Vec<Vec<u8>> = (0..(DCX_MAX_PAGES + 1)).map(|_| vec![0u8; 4]).collect();
    assert!(encode_dcx(&bogus).is_err());
}

#[test]
fn dcx_rejects_offset_out_of_bounds() {
    // Hand-build a DCX where an offset points past EOF.
    let mut dcx = Vec::new();
    dcx.extend_from_slice(&DCX_MAGIC.to_le_bytes());
    dcx.extend_from_slice(&999_999u32.to_le_bytes()); // bogus offset
    dcx.extend_from_slice(&0u32.to_le_bytes()); // sentinel
    assert!(parse_dcx(&dcx).is_err());
}

#[test]
fn dcx_rejects_non_monotonic_offsets() {
    let mut dcx = Vec::new();
    dcx.extend_from_slice(&DCX_MAGIC.to_le_bytes());
    dcx.extend_from_slice(&20u32.to_le_bytes());
    dcx.extend_from_slice(&15u32.to_le_bytes()); // backward
    dcx.extend_from_slice(&0u32.to_le_bytes());
    dcx.resize(30, 0);
    assert!(parse_dcx(&dcx).is_err());
}

// ---------------------------------------------------------------------------
// Decoder rejects out-of-range cases (sanity)
// ---------------------------------------------------------------------------

#[test]
fn writer_rejects_zero_dim() {
    assert!(encode_pcx_1bpp_mono(0, 1, &[]).is_err());
    assert!(encode_pcx_4bpp_packed(0, 1, &[], &[0u8; 48]).is_err());
    assert!(encode_pcx_2bpp_cga(0, 1, &[], 0, 0).is_err());
    assert!(encode_pcx_1bpp_4planes_ega(0, 1, &[], &[0u8; 48]).is_err());
}

#[test]
fn writer_rejects_short_input() {
    assert!(encode_pcx_1bpp_mono(2, 2, &[1u8, 2]).is_err());
    assert!(encode_pcx_4bpp_packed(2, 2, &[0u8, 1], &[0u8; 48]).is_err());
    assert!(encode_pcx_2bpp_cga(2, 2, &[0u8, 1], 0, 0).is_err());
    assert!(encode_pcx_1bpp_4planes_ega(2, 2, &[0u8, 1], &[0u8; 48]).is_err());
}

#[test]
fn writer_rejects_bad_palette_size() {
    assert!(encode_pcx_4bpp_packed(2, 2, &[0u8; 4], &[0u8; 47]).is_err());
    assert!(encode_pcx_1bpp_4planes_ega(2, 2, &[0u8; 4], &[0u8; 47]).is_err());
}

#[test]
fn writer_rejects_cga_bad_bg() {
    assert!(encode_pcx_2bpp_cga(2, 2, &[0u8; 4], 0, 16).is_err());
}
