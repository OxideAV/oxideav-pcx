//! r308 — confine the appended VGA tail-palette probe to the 256-colour
//! Extended VGA mode (`8 bpp × 1 plane`).
//!
//! The ZSoft PCX Technical Reference, Revision 5
//! (`docs/image/pcx/zsoft-pcx-technical-reference-rev5.md`) introduces the
//! appended 768-byte palette under §"VGA 256-color palette" as the carrier
//! for images with "more than 16 colors", and §"24-bit .PCX files" states
//! that 24-bit (8 bpp × 3-plane) images "do **not** contain a palette".
//! Every sub-256-colour mode (monochrome / CGA / EGA / 16-colour) carries
//! its palette in the header `Colormap` field, never as a tail block.
//!
//! The cross-reference summary
//! (`docs/image/pcx/pcx-egff-fileformat-info.html`) spells out the hazard
//! this round closes: "24-bit PCX images are always marked as v3.0, yet
//! never have an attached color palette" and the `0x0C` marker byte 769
//! bytes from EOF "might be 0Ch by coincidence". Before this round the
//! decoder ran the tail-palette probe for *every* `(bpp, planes)` mode, so
//! a 24-bit (or CGA / EGA) stream whose RLE data happened to end with that
//! pattern had 769 bytes of real pixel data mis-claimed as a palette and
//! stripped from the RLE region — corrupting the decode (or failing it
//! outright as a truncated RLE stream).
//!
//! The fix gates the probe on `(8 bpp, 1 plane)`; this round pins the
//! behaviour with a hand-crafted 24-bit fixture that plants the
//! coincidental `0x0C` marker 769 bytes from EOF — inside the real pixel
//! region — so a probe that stripped it would truncate the RLE stream.

use oxideav_pcx::types::{PCX_HEADER_SIZE, PCX_VGA_PALETTE_BLOCK_BYTES};
use oxideav_pcx::{encode_pcx_24bpp, parse_pcx};

/// Build a minimal valid PCX header (128 bytes) for the given geometry.
fn header(w: u16, h: u16, bpp: u8, planes: u8, bytes_per_line: u16) -> Vec<u8> {
    let mut hdr = vec![0u8; PCX_HEADER_SIZE];
    hdr[0] = 0x0A; // manufacturer
    hdr[1] = 5; // version 5 (PCX 5.0)
    hdr[2] = 1; // RLE encoding
    hdr[3] = bpp; // bits per pixel per plane
                  // window: x_min, y_min = 0; x_max = w-1; y_max = h-1
    hdr[8..10].copy_from_slice(&(w - 1).to_le_bytes());
    hdr[10..12].copy_from_slice(&(h - 1).to_le_bytes());
    hdr[65] = planes; // n_planes
    hdr[66..68].copy_from_slice(&bytes_per_line.to_le_bytes());
    hdr[68..70].copy_from_slice(&1u16.to_le_bytes()); // palette_info = 1 (colour)
    hdr
}

/// Hand-craft an 8 bpp × 3-plane (24-bit) PCX whose RLE payload is a run
/// of bare literal bytes (pixel values below the `0xC0` run-marker
/// threshold each encode to one literal byte), sized so the total pixel
/// region is well over 769 bytes, with the value `0x0C` planted at file
/// offset `len - 769` — exactly where the appended VGA palette's marker
/// byte would sit. Because the marker lands *inside* the pixel region
/// (not in trailing garbage after the last scanline), a pre-r308 decoder
/// that stripped the last 769 bytes lost real pixel data and the RLE
/// stream ran short mid-decode.
fn crafted_24bit_with_coincidental_marker() -> (Vec<u8>, u16, u16, Vec<u8>) {
    // 64px-wide × 8-row 24-bit image: bytes_per_line = 64 (already even),
    // scanline = 3 × 64 = 192 bytes, total planar = 192 × 8 = 1536 bytes —
    // comfortably more than the 769-byte palette block, so the marker is
    // forced into the middle of the pixel stream.
    let (w, h) = (64u16, 8u16);
    let bpl = 64u16;
    let planes = 3u8;
    let scanline = (bpl as usize) * (planes as usize);
    let total_planar = scanline * (h as usize);

    // Planar pixel bytes: a deterministic ramp kept strictly below 0xC0 so
    // every byte is a bare literal in the RLE stream (1 literal byte each,
    // so the RLE region length equals `total_planar`). Avoiding runs of
    // length ≥ 2 also keeps the encoder from coalescing, which would make
    // the offset arithmetic depend on the data.
    let mut planar: Vec<u8> = (0..total_planar).map(|i| (i % 0xC0) as u8).collect();

    // The file is `128 (header) + total_planar` bytes. Plant `0x0C` at
    // the planar offset that maps to file offset `len - 769`.
    let file_len = PCX_HEADER_SIZE + total_planar;
    let marker_file_off = file_len - PCX_VGA_PALETTE_BLOCK_BYTES;
    let marker_planar_off = marker_file_off - PCX_HEADER_SIZE;
    planar[marker_planar_off] = 0x0C;

    let mut file = header(w, h, 8, planes, bpl);
    file.extend_from_slice(&planar); // all literals -> RLE == planar bytes

    // Sanity: the byte 769 from EOF is the planted `0x0C` marker, and the
    // 768 bytes after it are genuine trailing pixel data.
    assert_eq!(file.len(), file_len);
    assert_eq!(
        file[marker_file_off], 0x0C,
        "fixture must plant the coincidental marker inside the pixel region"
    );

    // Reconstruct the expected flattened RGBA: planar planes are R, G, B,
    // visible width = w (= bpl here, no padding). Plane p of row y starts
    // at `y*scanline + p*bpl`.
    let mut expected = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        let base = y * scanline;
        for x in 0..w as usize {
            let r = planar[base + x];
            let g = planar[base + bpl as usize + x];
            let b = planar[base + 2 * bpl as usize + x];
            expected.extend_from_slice(&[r, g, b, 0xFF]);
        }
    }

    (file, w, h, expected)
}

#[test]
fn coincidental_marker_does_not_strip_24bit_pixels() {
    let (file, w, h, expected) = crafted_24bit_with_coincidental_marker();

    // Pre-r308 this errored as a truncated RLE stream (or decoded wrong
    // pixels) because 769 bytes of real pixel data were stripped before
    // the RLE phase. Post-r308 the probe is skipped for 24-bit data, so
    // the full pixel region is available and the decode is bit-exact.
    let img = parse_pcx(&file).expect("24-bit decode must not be derailed by a coincidental 0x0C");
    assert_eq!(img.width, w as u32);
    assert_eq!(img.height, h as u32);
    assert_eq!(
        img.data, expected,
        "every 24-bit pixel must survive a coincidental 0x0C at len-769"
    );
}

#[test]
fn genuine_24bit_roundtrip_unaffected() {
    // A normal 24-bit encode/decode still works (the encoder never appends
    // a tail palette to 24-bit output, so the probe being gated is a no-op
    // for the common path).
    let (w, h) = (5u16, 4u16);
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for i in 0..(w as usize * h as usize) {
        rgb.push((i * 7) as u8);
        rgb.push((i * 13) as u8);
        rgb.push((i * 29) as u8);
    }
    let pcx = encode_pcx_24bpp(w, h, &rgb).unwrap();
    let img = parse_pcx(&pcx).unwrap();
    assert_eq!(img.width, w as u32);
    assert_eq!(img.height, h as u32);
    for (i, chunk) in rgb.chunks_exact(3).enumerate() {
        assert_eq!(
            &img.data[i * 4..i * 4 + 4],
            &[chunk[0], chunk[1], chunk[2], 0xFF],
            "pixel {i} must survive 24-bit round-trip"
        );
    }
}

#[test]
fn eight_bpp_one_plane_tail_palette_still_honoured() {
    // The 256-colour Extended VGA mode is the one mode the probe must
    // still run for: a genuine VGA tail palette must be picked up.
    let (w, h) = (4u16, 3u16);
    let indices: Vec<u8> = (0..(w as usize * h as usize))
        .map(|i| (i % 256) as u8)
        .collect();
    let mut palette = vec![0u8; 768];
    // Distinctive palette so the lookup is observable.
    for (i, entry) in palette.chunks_exact_mut(3).enumerate() {
        entry[0] = i as u8;
        entry[1] = (255 - i) as u8;
        entry[2] = (i * 3) as u8;
    }
    let pcx = oxideav_pcx::encode_pcx_8bpp_indexed(w, h, &indices, &palette).unwrap();
    let img = parse_pcx(&pcx).unwrap();
    for (i, &idx) in indices.iter().enumerate() {
        let r = palette[idx as usize * 3];
        let g = palette[idx as usize * 3 + 1];
        let b = palette[idx as usize * 3 + 2];
        assert_eq!(
            &img.data[i * 4..i * 4 + 4],
            &[r, g, b, 0xFF],
            "8 bpp × 1 plane tail palette must still be honoured (pixel {i})"
        );
    }
}
