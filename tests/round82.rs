//! Round 82 — `palette_info=2` grayscale flag, non-zero window origin
//! writer, and the `bytes_per_line` sanity guard.

use oxideav_pcx::types::PCX_HEADER_SIZE;
use oxideav_pcx::{
    encode_pcx_24bpp, encode_pcx_24bpp_window, encode_pcx_8bpp_grayscale, encode_pcx_8bpp_indexed,
    parse_pcx, PcxError, PcxPixelFormat,
};

// ---------------------------------------------------------------------------
// `palette_info=2` (grayscale flag, spec §3)
// ---------------------------------------------------------------------------

#[test]
fn grayscale_writer_omits_vga_tail_palette() {
    // 4x1 grayscale ramp.
    let pixels = vec![0u8, 64, 128, 255];
    let bytes = encode_pcx_8bpp_grayscale(4, 1, &pixels).unwrap();
    // Header byte 68 (palette_info) must be 2 = grayscale.
    let palette_info = u16::from_le_bytes([bytes[68], bytes[69]]);
    assert_eq!(palette_info, 2, "palette_info should be 2 = grayscale");
    // No tail palette: bytes_per_line=4 (rounded even), height=1 → at most
    // ~6 RLE bytes follow the header. Total size must be well under
    // header + 768 + 1.
    assert!(
        bytes.len() < PCX_HEADER_SIZE + 64,
        "grayscale writer should not append a VGA tail palette; got {} bytes",
        bytes.len()
    );
}

#[test]
fn grayscale_writer_self_roundtrip() {
    let pixels: Vec<u8> = (0..(16 * 8)).map(|i| (i * 2) as u8).collect();
    let bytes = encode_pcx_8bpp_grayscale(16, 8, &pixels).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.width, 16);
    assert_eq!(img.height, 8);
    assert_eq!(img.pixel_format, PcxPixelFormat::Rgba);
    for (i, &p) in pixels.iter().enumerate() {
        assert_eq!(
            &img.data[i * 4..i * 4 + 4],
            &[p, p, p, 0xFF],
            "pixel {i} should decode as ({p},{p},{p},255)"
        );
    }
}

#[test]
fn grayscale_flag_overrides_tail_palette_on_decode() {
    // Build an 8bpp PCX with both a tail VGA palette (a rainbow) AND
    // palette_info=2. The decoder must honour the flag and emit
    // grayscale, NOT the rainbow.
    let mut palette = Vec::with_capacity(768);
    for i in 0..256u32 {
        palette.push(i as u8);
        palette.push((255 - i) as u8);
        palette.push(((i ^ 0x5A) & 0xFF) as u8);
    }
    let indices: Vec<u8> = (0..16).map(|i| i * 16).collect();
    // Build via the regular indexed writer (which sets palette_info=1
    // and appends a tail palette), then patch byte 68 to 2.
    let mut bytes = encode_pcx_8bpp_indexed(16, 1, &indices, &palette).unwrap();
    bytes[68] = 2;
    bytes[69] = 0;
    let img = parse_pcx(&bytes).unwrap();
    // Each pixel must be a grayscale (i, i, i, 0xFF) — the rainbow tail
    // palette is ignored because of the flag.
    for (x, &i) in indices.iter().enumerate() {
        assert_eq!(
            &img.data[x * 4..x * 4 + 4],
            &[i, i, i, 0xFF],
            "x={x} idx={i} should ignore tail palette and decode as grayscale"
        );
    }
}

#[test]
fn grayscale_writer_rejects_zero_dim() {
    assert!(encode_pcx_8bpp_grayscale(0, 1, &[]).is_err());
    assert!(encode_pcx_8bpp_grayscale(1, 0, &[]).is_err());
}

#[test]
fn grayscale_writer_rejects_short_input() {
    assert!(encode_pcx_8bpp_grayscale(4, 4, &[0u8; 8]).is_err());
}

#[test]
fn grayscale_writer_odd_width_pads_scanline() {
    // Width 5 → bytes_per_line padded to 6.
    let pixels: Vec<u8> = vec![10, 20, 30, 40, 50];
    let bytes = encode_pcx_8bpp_grayscale(5, 1, &pixels).unwrap();
    let bpl = u16::from_le_bytes([bytes[66], bytes[67]]);
    assert_eq!(bpl, 6, "bytes_per_line must round up to even");
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.width, 5);
    for (x, &p) in pixels.iter().enumerate() {
        assert_eq!(&img.data[x * 4..x * 4 + 4], &[p, p, p, 0xFF]);
    }
}

// ---------------------------------------------------------------------------
// Non-zero window origin (PCX 3.0+ edge case)
// ---------------------------------------------------------------------------

fn solid_rgb(w: u16, h: u16, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(w as usize * h as usize * 3);
    for _ in 0..(w as usize * h as usize) {
        out.extend_from_slice(&rgb);
    }
    out
}

#[test]
fn windowed_writer_sets_header_origin() {
    let rgb = solid_rgb(8, 4, [200, 100, 50]);
    let bytes = encode_pcx_24bpp_window(100, 50, 8, 4, &rgb).unwrap();
    let x_min = u16::from_le_bytes([bytes[4], bytes[5]]);
    let y_min = u16::from_le_bytes([bytes[6], bytes[7]]);
    let x_max = u16::from_le_bytes([bytes[8], bytes[9]]);
    let y_max = u16::from_le_bytes([bytes[10], bytes[11]]);
    assert_eq!(x_min, 100);
    assert_eq!(y_min, 50);
    assert_eq!(x_max, 107, "x_max = x_min + width - 1");
    assert_eq!(y_max, 53, "y_max = y_min + height - 1");
    let img = parse_pcx(&bytes).unwrap();
    // Decoder reports the visible w/h (not absolute coordinates).
    assert_eq!(img.width, 8);
    assert_eq!(img.height, 4);
    for i in 0..(8 * 4) {
        assert_eq!(&img.data[i * 4..i * 4 + 4], &[200, 100, 50, 0xFF]);
    }
}

#[test]
fn windowed_writer_rejects_origin_overflow() {
    // x_min + width must fit in u16.
    let rgb = solid_rgb(4, 4, [0, 0, 0]);
    assert!(encode_pcx_24bpp_window(0xFFFE, 0, 4, 4, &rgb).is_err());
    assert!(encode_pcx_24bpp_window(0, 0xFFFE, 4, 4, &rgb).is_err());
}

#[test]
fn windowed_writer_at_zero_matches_plain_24bpp_writer() {
    // x_min=y_min=0 → byte-for-byte same as encode_pcx_24bpp.
    let rgb = solid_rgb(6, 5, [9, 99, 199]);
    let a = encode_pcx_24bpp(6, 5, &rgb).unwrap();
    let b = encode_pcx_24bpp_window(0, 0, 6, 5, &rgb).unwrap();
    assert_eq!(a, b);
}

// ---------------------------------------------------------------------------
// `bytes_per_line` sanity guard
// ---------------------------------------------------------------------------

#[test]
fn rejects_bytes_per_line_too_small() {
    // Build an 8bpp×1 PCX with bytes_per_line=2 but width=8 (needs ≥8).
    // The decoder used to silently misframe; now it must reject.
    let mut bytes = vec![0u8; PCX_HEADER_SIZE];
    bytes[0] = 0x0A;
    bytes[1] = 5;
    bytes[2] = 1; // RLE
    bytes[3] = 8; // bpp
                  // x_min/y_min already zero. x_max = 7, y_max = 0.
    bytes[8] = 7;
    bytes[10] = 0;
    bytes[65] = 1; // n_planes
                   // bytes_per_line = 2 (too small for width 8 at 8 bpp).
    bytes[66] = 2;
    bytes[67] = 0;
    bytes[68] = 1; // palette_info
                   // Tail: a few RLE bytes (won't be reached).
    bytes.push(0xC1);
    bytes.push(0x00);
    let err = parse_pcx(&bytes).unwrap_err();
    assert!(matches!(err, PcxError::InvalidData(_)));
    let msg = match err {
        PcxError::InvalidData(s) => s,
        _ => unreachable!(),
    };
    assert!(
        msg.contains("bytes_per_line"),
        "error should mention bytes_per_line, got: {msg}"
    );
}

#[test]
fn accepts_bytes_per_line_padded_up() {
    // bytes_per_line strictly larger than the natural minimum is fine —
    // the spec rounds up to even and some writers round up further.
    // Width=5 at 8 bpp → min=5, padded=6 is normal, padded=10 also OK.
    let mut bytes = vec![0u8; PCX_HEADER_SIZE];
    bytes[0] = 0x0A;
    bytes[1] = 5;
    bytes[2] = 1;
    bytes[3] = 8;
    bytes[8] = 4; // x_max = 4 → width 5
    bytes[65] = 1;
    bytes[66] = 10; // way oversized but legal
    bytes[68] = 1;
    // Encode 10 bytes of pixel data (the 5 visible + 5 of padding).
    let mut raw = Vec::new();
    raw.extend_from_slice(&[1u8, 2, 3, 4, 5, 0, 0, 0, 0, 0]);
    oxideav_pcx::rle::encode(&raw, &mut bytes);
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(img.width, 5);
    // Decoder reads only the first `width` pixels of the scanline.
    for x in 0..5usize {
        let v = (x + 1) as u8;
        assert_eq!(&img.data[x * 4..x * 4 + 4], &[v, v, v, 0xFF]);
    }
}
