//! Round 417: the `Pal8` palette side-channel — closing the framework
//! encoder's last pixel-format gap.
//!
//! `oxideav-core` 0.1.30 added an in-band palette side-channel to
//! `VideoFrame`: a trailing stride-0 plane carrying packed 3-byte RGB
//! entries, with typed accessors (`palette` / `set_palette` /
//! `with_palette` / `take_palette` / `image_planes`). This round wires
//! it up on both sides of the PCX codec:
//!
//! * **Encode** — `PixelFormat::Pal8` frames whose palette rides the
//!   side-channel route through the new standalone
//!   `encode_pcx_indexed_auto`, which stores the caller's table
//!   *verbatim* (never re-derived, re-ordered, or quantised) in the
//!   smallest applicable spec geometry: the two 16-entry header
//!   `Colormap` rungs (spec §3 — 4 bpp × 1 plane packed nibbles and
//!   the 1 bpp × 4 planes spec-table-§3.1 sibling, whichever RLEs
//!   smaller) when the table has ≤ 16 entries, every index fits in 4
//!   bits, and the zero-padded colormap would not collide with the
//!   all-zero "unset" header sentinel; the 8 bpp × 1 plane + 768-byte
//!   VGA tail (spec §"VGA 256-color palette") otherwise.
//! * **Decode** — constructing the framework `Decoder` with
//!   `CodecParameters.pixel_format = Some(Pal8)` returns raw palette
//!   indices with the file's own palette attached to the side-channel,
//!   for every paletted `(bpp, planes)` geometry the crate reads. The
//!   default `Rgba` expansion (what the container demuxer requests) is
//!   byte-for-byte unchanged — the Pal8 path is purely additive.
//!
//! Round-trip contract pinned here: Pal8 + palette → PCX → Pal8 +
//! palette is index-exact always and palette-exact up to the caller's
//! entry count (the on-disk tables are fixed-size, so the tail beyond
//! the caller's entries is the documented zero padding).

use oxideav_core::{CodecId, CodecParameters, Frame, PixelFormat, VideoFrame, VideoPlane};
use oxideav_pcx::{
    encode_pcx_1bpp_2planes_cga, encode_pcx_1bpp_3planes_ega_rgb, encode_pcx_1bpp_4planes_ega,
    encode_pcx_1bpp_mono, encode_pcx_24bpp, encode_pcx_2bpp_cga, encode_pcx_4bpp_packed,
    encode_pcx_8bpp_grayscale, encode_pcx_8bpp_indexed, encode_pcx_indexed_auto, parse_header,
    parse_pcx, parse_pcx_indexed_1bpp_2planes_cga, parse_pcx_indexed_1bpp_4planes,
    parse_pcx_indexed_2bpp_cga_cpi, parse_pcx_indexed_4bpp, parse_pcx_indexed_8bpp, PcxAutoMode,
    PcxPaletteSource,
};

// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------

/// Deterministic full 256-entry (768-byte) palette: entry `i` is
/// `(i, !i, i ^ 0x55)` — no two entries equal, no grayscale structure,
/// so any palette re-derivation or re-ordering would be caught.
fn palette_256() -> Vec<u8> {
    let mut p = Vec::with_capacity(768);
    for i in 0..=255u8 {
        p.extend_from_slice(&[i, !i, i ^ 0x55]);
    }
    p
}

/// Deterministic index buffer covering the full 0..=255 range with a
/// non-trivial spatial pattern (runs AND singletons so both RLE arms
/// fire). Width is odd in the callers so the even-`bytes_per_line`
/// padding path is exercised too.
fn indices_wide(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 7 + 3) % 256) as u8).collect()
}

/// Deterministic small-index buffer (0..=15 only) for the header-rung
/// tests.
fn indices_nibble(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 5 + 1) % 16) as u8).collect()
}

fn pal8_params(w: u32, h: u32) -> CodecParameters {
    let mut params = CodecParameters::video(CodecId::new("pcx"));
    params.width = Some(w);
    params.height = Some(h);
    params.pixel_format = Some(PixelFormat::Pal8);
    params
}

/// Encode a Pal8 frame (index plane + side-channel palette) through the
/// framework `Encoder` and return the produced PCX bytes.
fn encode_pal8_frame(w: u32, h: u32, stride: usize, plane: Vec<u8>, palette: Vec<u8>) -> Vec<u8> {
    let params = pal8_params(w, h);
    let frame = VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane {
            stride,
            data: plane,
        }],
    }
    .with_palette(palette);
    let mut enc = oxideav_pcx::encoder::make_encoder(&params).unwrap();
    enc.send_frame(&Frame::Video(frame)).unwrap();
    enc.receive_packet().unwrap().data
}

/// Decode PCX bytes through the framework `Decoder` constructed with
/// Pal8-requested parameters; returns the produced frame.
fn decode_pal8(bytes: &[u8], w: u32, h: u32) -> VideoFrame {
    let params = pal8_params(w, h);
    let mut dec = oxideav_pcx::decoder::make_decoder(&params).unwrap();
    let pkt = oxideav_core::Packet::new(0, oxideav_core::TimeBase::new(1, 1), bytes.to_vec());
    dec.send_packet(&pkt).unwrap();
    match dec.receive_frame().unwrap() {
        Frame::Video(v) => v,
        other => panic!("expected video frame, got {other:?}"),
    }
}

/// `(bits_per_pixel, n_planes)` of an encoded file, for asserting which
/// rung the ladder picked at the raw-header level.
fn geometry(bytes: &[u8]) -> (u8, u8) {
    let h = parse_header(bytes).expect("encoded output must carry a parseable header");
    (h.bits_per_pixel, h.n_planes)
}

// ---------------------------------------------------------------------------
// Standalone `encode_pcx_indexed_auto`
// ---------------------------------------------------------------------------

/// A full 256-entry palette can only ride the VGA tail; indices and all
/// 768 palette bytes must round-trip bit-exactly through the typed
/// 8 bpp accessor.
#[test]
fn indexed_auto_full_palette_takes_vga_tail_and_round_trips_exactly() {
    let (w, h) = (13u16, 7u16);
    let idx = indices_wide(w as usize * h as usize);
    let pal = palette_256();
    let (bytes, mode) = encode_pcx_indexed_auto(w, h, &idx, &pal).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed8 { colors: 256 });
    assert_eq!(geometry(&bytes), (8, 1));
    let v = parse_pcx_indexed_8bpp(&bytes).unwrap();
    assert_eq!(v.palette_source, PcxPaletteSource::VgaTail);
    assert_eq!(v.indices, idx, "indices must round-trip byte-exactly");
    let flat: Vec<u8> = v.palette.iter().flatten().copied().collect();
    assert_eq!(flat, pal, "all 768 palette bytes must round-trip");
}

/// A 16-entry table fits the header colormap; the ladder must pick the
/// smaller of the two 16-colour rungs and preserve indices + palette.
#[test]
fn indexed_auto_16_entry_palette_takes_header_colormap_rung() {
    let (w, h) = (23u16, 9u16);
    let idx = indices_nibble(w as usize * h as usize);
    let pal: Vec<u8> = (0..16u8)
        .flat_map(|i| [i * 16 + 1, 255 - i, i * 3])
        .collect();
    let (bytes, mode) = encode_pcx_indexed_auto(w, h, &idx, &pal).unwrap();
    // The chosen rung must be one of the two header-palette forms and
    // must match the fewest-byte candidate exactly.
    let mut pal48 = [0u8; 48];
    pal48.copy_from_slice(&pal);
    let c4 = encode_pcx_4bpp_packed(w, h, &idx, &pal48).unwrap();
    let c1x4 = encode_pcx_1bpp_4planes_ega(w, h, &idx, &pal48).unwrap();
    match mode {
        PcxAutoMode::Indexed4 { colors: 16 } => {
            assert_eq!(bytes, c4);
            assert!(
                c4.len() <= c1x4.len(),
                "ladder must have picked the smaller rung"
            );
        }
        PcxAutoMode::Indexed1x4 { colors: 16 } => {
            assert_eq!(bytes, c1x4);
            assert!(
                c1x4.len() < c4.len(),
                "1x4 may only win strictly (tie keeps Indexed4)"
            );
        }
        other => panic!("16-entry palette must land on a header rung, got {other:?}"),
    }
    // Round-trip through the matching typed accessor.
    let (got_idx, got_pal): (Vec<u8>, Vec<u8>) = match geometry(&bytes) {
        (4, 1) => {
            let v = parse_pcx_indexed_4bpp(&bytes).unwrap();
            (v.indices, v.palette.iter().flatten().copied().collect())
        }
        (1, 4) => {
            let v = parse_pcx_indexed_1bpp_4planes(&bytes).unwrap();
            (v.indices, v.palette.iter().flatten().copied().collect())
        }
        g => panic!("unexpected geometry {g:?}"),
    };
    assert_eq!(got_idx, idx);
    assert_eq!(got_pal, pal);
}

/// A shorter-than-capacity table zero-pads the on-disk colormap; the
/// caller's entries stay verbatim and the padding is all-zero.
#[test]
fn indexed_auto_partial_palette_zero_pads_the_colormap() {
    let (w, h) = (10u16, 4u16);
    let pal: Vec<u8> = vec![
        10, 20, 30, //
        40, 50, 60, //
        70, 80, 90, //
        100, 110, 120, //
        130, 140, 150,
    ]; // 5 entries
    let idx: Vec<u8> = (0..40).map(|i| (i % 5) as u8).collect();
    let (bytes, mode) = encode_pcx_indexed_auto(w, h, &idx, &pal).unwrap();
    assert!(matches!(
        mode,
        PcxAutoMode::Indexed4 { colors: 5 } | PcxAutoMode::Indexed1x4 { colors: 5 }
    ));
    let (got_idx, flat): (Vec<u8>, Vec<u8>) = match geometry(&bytes) {
        (4, 1) => {
            let v = parse_pcx_indexed_4bpp(&bytes).unwrap();
            (v.indices, v.palette.iter().flatten().copied().collect())
        }
        (1, 4) => {
            let v = parse_pcx_indexed_1bpp_4planes(&bytes).unwrap();
            (v.indices, v.palette.iter().flatten().copied().collect())
        }
        g => panic!("expected a header-colormap rung, got {g:?}"),
    };
    assert_eq!(&flat[..15], &pal[..], "caller entries verbatim");
    assert!(flat[15..].iter().all(|&b| b == 0), "padding must be zero");
    assert_eq!(got_idx, idx);
}

/// An all-black small table would zero-pad into the all-zero colormap —
/// indistinguishable from the "unset" sentinel that makes readers
/// substitute the spec table §3.1 hardware default. The ladder must
/// route such tables onto the VGA tail, where the round-trip stays
/// byte-exact.
#[test]
fn indexed_auto_all_black_palette_routes_to_vga_tail() {
    let (w, h) = (8u16, 8u16);
    let pal = vec![0u8, 0, 0]; // single black entry
    let idx = vec![0u8; 64];
    let (bytes, mode) = encode_pcx_indexed_auto(w, h, &idx, &pal).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed8 { colors: 1 });
    assert_eq!(geometry(&bytes), (8, 1));
    let v = parse_pcx_indexed_8bpp(&bytes).unwrap();
    assert_eq!(v.palette_source, PcxPaletteSource::VgaTail);
    assert_eq!(v.palette[0], [0, 0, 0]);
    assert_eq!(v.indices, idx);
}

/// An index above 15 cannot be stored in 4 bits, whatever the entry
/// count — the ladder must fall back to the VGA tail and keep the
/// out-of-range index verbatim (it resolves to the zero padding, the
/// documented missing-entry policy).
#[test]
fn indexed_auto_index_above_15_forces_vga_tail() {
    let (w, h) = (6u16, 2u16);
    let pal = vec![1u8, 2, 3, 4, 5, 6]; // 2 entries
    let mut idx = vec![0u8, 1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 0];
    idx[5] = 200; // out-of-range index
    let (bytes, mode) = encode_pcx_indexed_auto(w, h, &idx, &pal).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed8 { colors: 2 });
    let v = parse_pcx_indexed_8bpp(&bytes).unwrap();
    assert_eq!(v.indices, idx, "out-of-range index must survive verbatim");
    assert_eq!(
        v.palette[200],
        [0, 0, 0],
        "beyond-table entries are the zero padding"
    );
    assert_eq!(v.palette[0], [1, 2, 3]);
    assert_eq!(v.palette[1], [4, 5, 6]);
}

/// Palette argument validation: empty, non-multiple-of-3, and
/// over-768-byte tables are rejected up front.
#[test]
fn indexed_auto_rejects_malformed_palettes() {
    let idx = vec![0u8; 4];
    assert!(encode_pcx_indexed_auto(2, 2, &idx, &[]).is_err());
    assert!(encode_pcx_indexed_auto(2, 2, &idx, &[1, 2, 3, 4]).is_err());
    assert!(encode_pcx_indexed_auto(2, 2, &idx, &[0u8; 771]).is_err());
    // Boundary: exactly 768 bytes is fine.
    assert!(encode_pcx_indexed_auto(2, 2, &idx, &[1u8; 768]).is_ok());
}

/// Determinism + fewest-byte contract across a spread of shapes: the
/// returned bytes always equal the smallest applicable candidate
/// (re-derived here independently), and an exact size tie keeps the
/// earlier rung in (Indexed4, Indexed1x4, Indexed8) order.
#[test]
fn indexed_auto_always_returns_the_fewest_byte_candidate() {
    for (w, h) in [(1u16, 1u16), (7, 3), (16, 16), (33, 2), (2, 33), (63, 5)] {
        let n = w as usize * h as usize;
        let idx = indices_nibble(n);
        let pal: Vec<u8> = (0..12u8).flat_map(|i| [i, i * 2, 255 - i]).collect(); // 12 entries
        let (bytes, mode) = encode_pcx_indexed_auto(w, h, &idx, &pal).unwrap();
        let mut pal48 = [0u8; 48];
        pal48[..pal.len()].copy_from_slice(&pal);
        let mut pal768 = vec![0u8; 768];
        pal768[..pal.len()].copy_from_slice(&pal);
        let c4 = encode_pcx_4bpp_packed(w, h, &idx, &pal48).unwrap();
        let c1x4 = encode_pcx_1bpp_4planes_ega(w, h, &idx, &pal48).unwrap();
        let c8 = encode_pcx_8bpp_indexed(w, h, &idx, &pal768).unwrap();
        let min = c4.len().min(c1x4.len()).min(c8.len());
        assert_eq!(
            bytes.len(),
            min,
            "{w}x{h}: must emit the fewest-byte candidate"
        );
        // Tie-break determinism: earlier rung keeps an exact tie.
        let expected_mode = if c4.len() == min {
            PcxAutoMode::Indexed4 { colors: 12 }
        } else if c1x4.len() == min {
            PcxAutoMode::Indexed1x4 { colors: 12 }
        } else {
            PcxAutoMode::Indexed8 { colors: 12 }
        };
        assert_eq!(
            mode, expected_mode,
            "{w}x{h}: tie must keep the earlier rung"
        );
        // Same input twice → byte-identical output.
        let (bytes2, mode2) = encode_pcx_indexed_auto(w, h, &idx, &pal).unwrap();
        assert_eq!(bytes, bytes2);
        assert_eq!(mode, mode2);
    }
}

/// Whatever rung the ladder picks, flattening the file through
/// `parse_pcx` must reproduce the palette colours the indices select —
/// the ladder never quantises.
#[test]
fn indexed_auto_flatten_reproduces_palette_colours() {
    let (w, h) = (11u16, 6u16);
    let n = w as usize * h as usize;
    let pal: Vec<u8> = (0..7u8)
        .flat_map(|i| [i * 31, i * 17, 255 - i * 40])
        .collect();
    let idx: Vec<u8> = (0..n).map(|i| (i % 7) as u8).collect();
    let (bytes, _mode) = encode_pcx_indexed_auto(w, h, &idx, &pal).unwrap();
    let img = parse_pcx(&bytes).unwrap();
    for (i, px) in img.data.chunks_exact(4).enumerate() {
        let e = idx[i] as usize * 3;
        assert_eq!(
            &px[..3],
            &pal[e..e + 3],
            "pixel {i} must resolve the caller entry"
        );
        assert_eq!(px[3], 0xFF);
    }
}

// ---------------------------------------------------------------------------
// Framework Pal8 encode → decode round-trips
// ---------------------------------------------------------------------------

/// Full-table Pal8 frame → PCX → Pal8 frame: index- and palette-exact,
/// including all 768 side-channel bytes.
#[test]
fn pal8_frame_full_palette_round_trips_exactly() {
    let (w, h) = (13u32, 7u32);
    let idx = indices_wide((w * h) as usize);
    let pal = palette_256();
    let bytes = encode_pal8_frame(w, h, w as usize, idx.clone(), pal.clone());
    assert_eq!(geometry(&bytes), (8, 1));
    let frame = decode_pal8(&bytes, w, h);
    assert_eq!(frame.image_plane_count(), 1);
    let plane = &frame.image_planes()[0];
    assert_eq!(plane.stride, w as usize);
    assert_eq!(plane.data, idx, "indices must round-trip byte-exactly");
    assert_eq!(
        frame.palette(),
        Some(&pal[..]),
        "palette must round-trip byte-exactly"
    );
}

/// Small-table Pal8 frame: rides a 16-colour header rung on disk, and
/// the decoded side-channel is the file's 48-byte colormap whose prefix
/// is the caller's table verbatim (tail = zero padding).
#[test]
fn pal8_frame_small_palette_round_trips_prefix_exact() {
    let (w, h) = (17u32, 5u32);
    let idx: Vec<u8> = (0..w * h).map(|i| (i % 7) as u8).collect();
    let pal: Vec<u8> = (0..7u8).flat_map(|i| [i + 1, i * 20, 200 - i]).collect();
    let bytes = encode_pal8_frame(w, h, w as usize, idx.clone(), pal.clone());
    let (bpp, planes) = geometry(&bytes);
    assert!(
        (bpp, planes) == (4, 1) || (bpp, planes) == (1, 4),
        "≤16-entry table must ride a header-colormap rung, got ({bpp}, {planes})"
    );
    let frame = decode_pal8(&bytes, w, h);
    assert_eq!(frame.image_planes()[0].data, idx);
    let got = frame
        .palette()
        .expect("decoded Pal8 frame must carry the file palette");
    assert_eq!(got.len(), 48, "header colormap is 16 entries on disk");
    assert_eq!(&got[..pal.len()], &pal[..], "caller entries verbatim");
    assert!(
        got[pal.len()..].iter().all(|&b| b == 0),
        "padding must be zero"
    );
    // The typed sugar accessor resolves entries through the same bytes.
    assert_eq!(frame.palette_rgb(1), Some([2, 20, 199]));
}

/// A padded index-plane stride collapses to tight rows before encode —
/// same bytes as the tight-stride twin.
#[test]
fn pal8_frame_with_padded_stride_matches_tight_twin() {
    let (w, h) = (9u32, 4u32);
    let idx = indices_nibble((w * h) as usize);
    let pal: Vec<u8> = (0..16u8)
        .flat_map(|i| [i, i, i.wrapping_mul(31) | 1])
        .collect();
    let stride = w as usize + 3;
    let mut padded = vec![0xEEu8; stride * h as usize];
    for y in 0..h as usize {
        padded[y * stride..y * stride + w as usize]
            .copy_from_slice(&idx[y * w as usize..(y + 1) * w as usize]);
    }
    let tight = encode_pal8_frame(w, h, w as usize, idx.clone(), pal.clone());
    let from_padded = encode_pal8_frame(w, h, stride, padded, pal);
    assert_eq!(
        tight, from_padded,
        "stride padding must not leak into the file"
    );
}

/// The framework Pal8 path is a thin shim over the standalone ladder:
/// byte-identical output.
#[test]
fn framework_pal8_encode_matches_standalone_indexed_auto() {
    let (w, h) = (21u32, 11u32);
    let idx = indices_wide((w * h) as usize);
    let pal = palette_256();
    let framework = encode_pal8_frame(w, h, w as usize, idx.clone(), pal.clone());
    let (standalone, _mode) = encode_pcx_indexed_auto(w as u16, h as u16, &idx, &pal).unwrap();
    assert_eq!(framework, standalone);
}

/// A Pal8 frame with no side-channel palette is rejected with a clear
/// error (there is nothing conformant to write).
#[test]
fn pal8_frame_without_palette_is_rejected() {
    let params = pal8_params(4, 4);
    let frame = VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane {
            stride: 4,
            data: vec![0u8; 16],
        }],
    };
    let mut enc = oxideav_pcx::encoder::make_encoder(&params).unwrap();
    let err = enc.send_frame(&Frame::Video(frame)).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("palette side-channel"),
        "unexpected error: {msg}"
    );
}

/// A malformed side-channel table (length not a multiple of 3) is
/// rejected at the encoder boundary.
#[test]
fn pal8_frame_with_malformed_palette_is_rejected() {
    let params = pal8_params(4, 4);
    let frame = VideoFrame {
        pts: Some(0),
        planes: vec![VideoPlane {
            stride: 4,
            data: vec![0u8; 16],
        }],
    }
    .with_palette(vec![1, 2, 3, 4]);
    let mut enc = oxideav_pcx::encoder::make_encoder(&params).unwrap();
    assert!(enc.send_frame(&Frame::Video(frame)).is_err());
}

// ---------------------------------------------------------------------------
// Pal8-requested decode across every paletted geometry
// ---------------------------------------------------------------------------

/// Monochrome (1 bpp × 1 plane): bit value = colormap index, and the
/// side-channel carries the two-entry black / white table the crate's
/// writer stores.
#[test]
fn pal8_decode_mono_attaches_two_entry_palette() {
    let (w, h) = (10u32, 3u32);
    let pixels: Vec<u8> = (0..w * h).map(|i| (i % 3 == 0) as u8).collect();
    let bytes = encode_pcx_1bpp_mono(w as u16, h as u16, &pixels).unwrap();
    let frame = decode_pal8(&bytes, w, h);
    assert_eq!(frame.image_planes()[0].data, pixels);
    assert_eq!(frame.palette(), Some(&[0u8, 0, 0, 0xFF, 0xFF, 0xFF][..]));
}

/// Packed CGA (2 bpp × 1 plane): indices are the low two bits, palette
/// is the resolved 4-entry family — identical to the spec-faithful
/// C / P / I typed accessor.
#[test]
fn pal8_decode_cga_packed_attaches_resolved_family() {
    let (w, h) = (12u32, 4u32);
    let idx: Vec<u8> = (0..w * h).map(|i| (i % 4) as u8).collect();
    let bytes = encode_pcx_2bpp_cga(w as u16, h as u16, &idx, 0x60, 3).unwrap();
    let frame = decode_pal8(&bytes, w, h);
    assert_eq!(frame.image_planes()[0].data, idx);
    let v = parse_pcx_indexed_2bpp_cga_cpi(&bytes).unwrap();
    let flat: Vec<u8> = v.palette.iter().flatten().copied().collect();
    assert_eq!(frame.palette(), Some(&flat[..]));
    assert_eq!(flat.len(), 12);
}

/// Plane-oriented CGA (1 bpp × 2 planes): same contract through the
/// planar layout.
#[test]
fn pal8_decode_cga_planar_attaches_resolved_family() {
    let (w, h) = (9u32, 5u32);
    let idx: Vec<u8> = (0..w * h).map(|i| ((i * 3) % 4) as u8).collect();
    let bytes = encode_pcx_1bpp_2planes_cga(w as u16, h as u16, &idx, 0x40, 1).unwrap();
    let frame = decode_pal8(&bytes, w, h);
    assert_eq!(frame.image_planes()[0].data, idx);
    let v = parse_pcx_indexed_1bpp_2planes_cga(&bytes).unwrap();
    let flat: Vec<u8> = v.palette.iter().flatten().copied().collect();
    assert_eq!(frame.palette(), Some(&flat[..]));
}

/// 8-colour EGA RGB (1 bpp × 3 planes): the fixed on/off-primary table
/// rides the side-channel (24 bytes), indices are the 3-bit plane
/// stacks.
#[test]
fn pal8_decode_ega_rgb_attaches_fixed_primaries() {
    let (w, h) = (8u32, 2u32);
    // One pixel of each primary combination, twice.
    let rgb: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let k = i % 8;
            [
                if k & 1 != 0 { 0xFF } else { 0x00 },
                if k & 2 != 0 { 0xFF } else { 0x00 },
                if k & 4 != 0 { 0xFF } else { 0x00 },
            ]
        })
        .collect();
    let bytes = encode_pcx_1bpp_3planes_ega_rgb(w as u16, h as u16, &rgb).unwrap();
    let frame = decode_pal8(&bytes, w, h);
    let expect_idx: Vec<u8> = (0..w * h).map(|i| (i % 8) as u8).collect();
    assert_eq!(frame.image_planes()[0].data, expect_idx);
    let pal = frame.palette().unwrap();
    assert_eq!(pal.len(), 24);
    for k in 0..8u8 {
        assert_eq!(
            frame.palette_rgb(k),
            Some([
                if k & 1 != 0 { 0xFF } else { 0x00 },
                if k & 2 != 0 { 0xFF } else { 0x00 },
                if k & 4 != 0 { 0xFF } else { 0x00 },
            ])
        );
    }
}

/// Grayscale-flag files (`palette_info = 2`): the side-channel carries
/// the synthetic 256-entry ramp and the indices are the sample bytes.
#[test]
fn pal8_decode_grayscale_flag_attaches_ramp() {
    let (w, h) = (16u32, 2u32);
    let pixels: Vec<u8> = (0..w * h).map(|i| (i * 8) as u8).collect();
    let bytes = encode_pcx_8bpp_grayscale(w as u16, h as u16, &pixels).unwrap();
    let frame = decode_pal8(&bytes, w, h);
    assert_eq!(frame.image_planes()[0].data, pixels);
    let pal = frame.palette().unwrap();
    assert_eq!(pal.len(), 768);
    for i in 0..=255u8 {
        assert_eq!(frame.palette_rgb(i), Some([i, i, i]));
    }
}

/// The palette-free 24-bit mode cannot be represented as Pal8 and is
/// rejected rather than silently quantised.
#[test]
fn pal8_decode_rejects_24bit_files() {
    let rgb: Vec<u8> = (0..4 * 4 * 3).map(|i| i as u8).collect();
    let bytes = encode_pcx_24bpp(4, 4, &rgb).unwrap();
    let params = pal8_params(4, 4);
    let mut dec = oxideav_pcx::decoder::make_decoder(&params).unwrap();
    let pkt = oxideav_core::Packet::new(0, oxideav_core::TimeBase::new(1, 1), bytes);
    let err = dec.send_packet(&pkt).unwrap_err();
    assert!(err.to_string().contains("Pal8"), "unexpected error: {err}");
}

// ---------------------------------------------------------------------------
// Compatibility: the default Rgba path is unchanged and palette-free
// ---------------------------------------------------------------------------

/// Decoding without requesting Pal8 (what the container demuxer does)
/// still produces the historical single-plane packed-Rgba frame with no
/// side-channel attached — the Pal8 path is purely additive.
#[test]
fn default_rgba_decode_is_unchanged_and_carries_no_palette() {
    let (w, h) = (6u32, 6u32);
    let idx: Vec<u8> = (0..w * h).map(|i| (i % 5) as u8).collect();
    let pal = palette_256();
    let bytes = encode_pal8_frame(w, h, w as usize, idx.clone(), pal.clone());
    let mut params = CodecParameters::video(CodecId::new("pcx"));
    params.width = Some(w);
    params.height = Some(h);
    params.pixel_format = Some(PixelFormat::Rgba);
    let mut dec = oxideav_pcx::decoder::make_decoder(&params).unwrap();
    let pkt = oxideav_core::Packet::new(0, oxideav_core::TimeBase::new(1, 1), bytes.clone());
    dec.send_packet(&pkt).unwrap();
    let frame = match dec.receive_frame().unwrap() {
        Frame::Video(v) => v,
        other => panic!("expected video frame, got {other:?}"),
    };
    assert_eq!(frame.planes.len(), 1, "no side-channel on the Rgba path");
    assert_eq!(frame.palette(), None);
    let img = parse_pcx(&bytes).unwrap();
    assert_eq!(frame.planes[0].data, img.data, "Rgba expansion unchanged");
    // And the expansion resolves the caller palette per pixel.
    for (i, px) in frame.planes[0].data.chunks_exact(4).enumerate() {
        let e = idx[i] as usize * 3;
        assert_eq!(&px[..3], &pal[e..e + 3]);
    }
}

/// The codec registration advertises Pal8 alongside the seven
/// historical formats so pipelines that pick from
/// `accepted_pixel_formats` can hand PCX indexed frames directly.
#[test]
fn registry_advertises_pal8() {
    let mut reg = oxideav_core::CodecRegistry::new();
    oxideav_pcx::register_codecs(&mut reg);
    let impls = reg.implementations(&CodecId::new("pcx"));
    assert!(!impls.is_empty());
    assert!(impls
        .iter()
        .any(|i| i.caps.accepted_pixel_formats.contains(&PixelFormat::Pal8)));
}
