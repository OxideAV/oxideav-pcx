//! r327 — RLE runs that straddle the scanline boundary.
//!
//! The ZSoft rev-5 manual's prose says "there should always be a
//! decoding break at the end of each scan line"
//! (`docs/image/pcx/zsoft-pcx-technical-reference-rev5.md`, §"Decoding
//! .PCX Files"). That is an *encoder* convention — a "should", not a
//! decode-time requirement. The manual's own sample decoder
//! (`docs/image/pcx/pcx-pcgpe.txt` lines 316-326) proves it: the
//! `for (l = 0; l < lsize; )` loop consumes the RLE stream straight
//! through `lsize = BytesPerLine * Nplanes * (1 + Ymax - Ymin)` bytes
//! with **no per-scanline reset**. A run packet whose count carries the
//! output past a row boundary therefore decodes correctly under the
//! canonical reader — it is the total image byte count, not the per-row
//! span, that bounds a packet.
//!
//! Before r327 the decoder ran `rle::decode` once per scanline with the
//! per-row stride as the hard cap, so a file written by an encoder that
//! let a run straddle the boundary was rejected mid-row with "PCX RLE
//! packet overruns scanline". r327 decodes the whole image as one
//! continuous RLE stream (matching the manual's reader), so such files
//! now decode byte-identically to their row-broken equivalents.
//!
//! This file tests:
//!
//! 1. A hand-built 8 bpp grayscale PCX whose single run crosses the row
//!    boundary decodes to the same pixels as the row-broken encoding.
//! 2. The straddle is exercised with trailing per-row padding present
//!    (`bytes_per_line > width`), where the run spans the previous row's
//!    padding into the next row's first pixels.
//! 3. A run straddling a *plane* boundary inside one multi-plane (24-bit)
//!    scanline still decodes correctly (this was always allowed — the
//!    spec says there is no decode break between planes within a row —
//!    and remains so under the continuous decode).
//! 4. A run whose count would overrun the *whole image* buffer is still
//!    rejected (the total-bytes cap is preserved; only the per-row cap
//!    was relaxed).
//! 5. The canonical writer's output (which never straddles) is
//!    unchanged: a normal encode → decode round-trip is byte-for-byte
//!    identical to before.

use oxideav_pcx::{encode_pcx_24bpp, encode_pcx_8bpp_grayscale, parse_pcx, PCX_HEADER_SIZE};

/// Build a minimal 128-byte PCX header for an 8 bpp × `n_planes` image
/// of the given pixel dimensions and `bytes_per_line` (per-plane row
/// stride). Grayscale flag is left at the default colour (`palette_info
/// = 1`); callers that want the grayscale ramp set it themselves.
fn header_8bpp(width: u16, height: u16, n_planes: u8, bytes_per_line: u16) -> Vec<u8> {
    let mut h = vec![0u8; PCX_HEADER_SIZE];
    h[0] = 0x0A; // manufacturer
    h[1] = 5; // version 5 (PCX 3.0+)
    h[2] = 1; // encoding = RLE
    h[3] = 8; // bits_per_pixel
    h[4..6].copy_from_slice(&0u16.to_le_bytes()); // x_min
    h[6..8].copy_from_slice(&0u16.to_le_bytes()); // y_min
    h[8..10].copy_from_slice(&(width - 1).to_le_bytes()); // x_max
    h[10..12].copy_from_slice(&(height - 1).to_le_bytes()); // y_max
    h[65] = n_planes;
    h[66..68].copy_from_slice(&bytes_per_line.to_le_bytes());
    h[68..70].copy_from_slice(&1u16.to_le_bytes()); // palette_info = colour/BW
    h
}

/// Encode a raw planar byte buffer (already laid out as
/// `n_planes × bytes_per_line × height`) to a PCX RLE body, breaking
/// runs at the END of each scanline — the canonical writer convention.
fn rle_body_row_broken(planar: &[u8], scanline: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for row in planar.chunks_exact(scanline) {
        encode_one_run_stream(row, &mut out);
    }
    out
}

/// Encode a raw planar byte buffer as ONE continuous RLE stream with no
/// per-row break, so runs of identical bytes straddle scanline (and
/// padding) boundaries wherever the data allows.
fn rle_body_continuous(planar: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    encode_one_run_stream(planar, &mut out);
    out
}

/// Minimal PCX RLE encoder over an arbitrary byte span (runs ≤ 63,
/// `>= 0xC0` singletons escaped). Independent of the crate's internal
/// encoder so the test fixes the on-disk bytes itself.
fn encode_one_run_stream(data: &[u8], out: &mut Vec<u8>) {
    let mut i = 0usize;
    while i < data.len() {
        let b = data[i];
        let mut run = 1usize;
        while i + run < data.len() && data[i + run] == b && run < 63 {
            run += 1;
        }
        if run >= 2 || (b & 0xC0) == 0xC0 {
            out.push(0xC0 | run as u8);
            out.push(b);
        } else {
            out.push(b);
        }
        i += run;
    }
}

// ---------------------------------------------------------------------------
// 1. Run straddling a scanline boundary, no padding.
// ---------------------------------------------------------------------------

#[test]
fn run_straddling_row_boundary_decodes_like_row_broken() {
    // Even width → bytes_per_line == width, no per-row padding.
    let (w, h) = (8u16, 4u16);
    // Single plane, so the scanline stride equals the width.
    let scanline = w as usize;
    // Constant value 0x42 over the whole image: the continuous encoder
    // coalesces it into runs that cross every row boundary, while the
    // row-broken encoder splits one run per row at the boundary.
    let planar = vec![0x42u8; scanline * h as usize];

    let hdr = header_8bpp(w, h, 1, scanline as u16);

    let mut file_broken = hdr.clone();
    file_broken.extend_from_slice(&rle_body_row_broken(&planar, scanline));

    let mut file_straddle = hdr;
    file_straddle.extend_from_slice(&rle_body_continuous(&planar));

    // The two RLE bodies are genuinely different on disk…
    assert_ne!(
        file_broken, file_straddle,
        "test fixture is trivial: both encodings identical"
    );

    let img_broken = parse_pcx(&file_broken).expect("row-broken decodes");
    let img_straddle = parse_pcx(&file_straddle).expect("straddling run decodes");

    // …yet decode to identical pixels.
    assert_eq!(img_broken.width, w as u32);
    assert_eq!(img_broken.height, h as u32);
    assert_eq!(
        img_straddle.data, img_broken.data,
        "straddling-run decode must match the row-broken decode"
    );
    // Every pixel is (0x42, 0x42, 0x42, 0xFF) under the default colour
    // flatten (header palette index 0x42 → entry 0x42).
    assert_eq!(img_straddle.data.len(), w as usize * h as usize * 4);
}

// ---------------------------------------------------------------------------
// 2. Straddle across trailing per-row padding.
// ---------------------------------------------------------------------------

#[test]
fn run_straddling_padding_then_next_row() {
    // Odd width 7 with bytes_per_line 8 → each row carries one trailing
    // padding byte. A constant-value image makes a single run span the
    // previous row's last visible byte, its padding byte, and the next
    // row's first visible bytes — a triple straddle.
    let (w, h) = (7u16, 5u16);
    let bpl = 8usize; // even, one padding byte per row
    let scanline = bpl; // 1 plane
    let planar = vec![0x99u8; scanline * h as usize];

    let hdr = header_8bpp(w, h, 1, bpl as u16);

    let mut file_broken = hdr.clone();
    file_broken.extend_from_slice(&rle_body_row_broken(&planar, scanline));

    let mut file_straddle = hdr;
    file_straddle.extend_from_slice(&rle_body_continuous(&planar));

    assert_ne!(file_broken, file_straddle);

    let img_broken = parse_pcx(&file_broken).expect("row-broken decodes");
    let img_straddle = parse_pcx(&file_straddle).expect("padding-straddle decodes");

    assert_eq!(img_straddle.width, w as u32);
    assert_eq!(img_straddle.height, h as u32);
    assert_eq!(
        img_straddle.data, img_broken.data,
        "padding-straddling run must decode identically to the row-broken form"
    );
    // Visible pixels are the padding-stripped 7×5 region.
    assert_eq!(img_straddle.data.len(), w as usize * h as usize * 4);
}

// ---------------------------------------------------------------------------
// 3. Straddle across a plane boundary within one 24-bit scanline.
// ---------------------------------------------------------------------------

#[test]
fn run_straddling_plane_boundary_within_scanline() {
    // 24-bit (8 bpp × 3 plane): one scanline is R-plane | G-plane |
    // B-plane laid end to end. A constant grey makes all three planes
    // equal, so a continuous run spans the R→G and G→B plane seams
    // within the row. The spec explicitly permits this ("there will not
    // be a decoding break at the end of each plane within each scan
    // line"). Confirm it decodes to the same grey image as the writer's
    // own output.
    let (w, h) = (6u16, 3u16);
    let rgb: Vec<u8> = vec![0x55u8; w as usize * h as usize * 3];
    let canonical = encode_pcx_24bpp(w, h, &rgb).expect("encode 24bpp");
    let img_canonical = parse_pcx(&canonical).expect("canonical decodes");

    // Hand-build the same image with a fully-continuous RLE body.
    let scanline = w as usize * 3; // 3 planes, bytes_per_line == width
    let mut planar = Vec::with_capacity(scanline * h as usize);
    for _ in 0..h {
        // R plane, G plane, B plane — all 0x55.
        planar.extend(std::iter::repeat_n(0x55u8, w as usize));
        planar.extend(std::iter::repeat_n(0x55u8, w as usize));
        planar.extend(std::iter::repeat_n(0x55u8, w as usize));
    }
    let mut hdr = header_8bpp(w, h, 3, w);
    hdr.extend_from_slice(&rle_body_continuous(&planar));

    let img_continuous = parse_pcx(&hdr).expect("plane-straddle decodes");
    assert_eq!(
        img_continuous.data, img_canonical.data,
        "continuous 24-bit decode must equal the canonical writer's output"
    );
}

// ---------------------------------------------------------------------------
// 4. A run overrunning the whole image buffer is still rejected.
// ---------------------------------------------------------------------------

#[test]
fn run_overrunning_image_total_is_rejected() {
    // 4×2 grayscale = 8 total planar bytes. Emit a single run packet
    // claiming 63 copies — far past the 8-byte total — and confirm the
    // decoder rejects it rather than reading out of bounds. (The per-row
    // cap was relaxed; the image-total cap is preserved.)
    let (w, h) = (4u16, 2u16);
    let mut file = header_8bpp(w, h, 1, w);
    file.push(0xC0 | 63); // run header, count 63
    file.push(0x00); // literal
    file.push(0xC0 | 63); // pad with more so truncation isn't the failure
    file.push(0x00);

    let err = parse_pcx(&file).expect_err("must reject image-buffer overrun");
    let msg = format!("{err}");
    assert!(
        msg.contains("overruns image buffer"),
        "unexpected error message: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 5. Canonical writer round-trip is unchanged.
// ---------------------------------------------------------------------------

#[test]
fn canonical_writer_roundtrip_unchanged() {
    let (w, h) = (17u16, 11u16);
    // A varied grayscale gradient so there are real runs AND literals,
    // and the visible width is odd (so per-row padding is in play).
    let mut pixels = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            pixels.push(((x * 7 + y * 13) & 0xFF) as u8);
        }
    }
    let bytes = encode_pcx_8bpp_grayscale(w, h, &pixels).expect("encode grayscale");
    let img = parse_pcx(&bytes).expect("decode grayscale");
    assert_eq!(img.width, w as u32);
    assert_eq!(img.height, h as u32);
    // Grayscale flatten: each pixel is (g, g, g, 0xFF).
    for (i, px) in img.data.chunks_exact(4).enumerate() {
        let g = pixels[i];
        assert_eq!(px, &[g, g, g, 0xFF], "pixel {i} mismatch");
    }
}
