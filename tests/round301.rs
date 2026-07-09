//! r301 — 4-colour CGA in the plane-oriented `1 bpp × 2 planes` layout.
//!
//! The *Encyclopedia of Graphics File Formats* PCX entry
//! (`docs/image/pcx/pcx-egff-fileformat-info.html`) tabulates the
//! canonical PCX `(BitsPerPixel, NumBitPlanes)` video-mode matrix:
//!
//! | BitsPerPixel | NumBitPlanes | Colours | Video mode |
//! | ------------ | ------------ | ------- | ---------- |
//! | 1            | 1            | 2       | Monochrome |
//! | 1            | 2            | 4       | CGA        |
//! | 1            | 3            | 8       | EGA        |
//! | 1            | 4            | 16      | EGA / VGA  |
//! | 8            | 1            | 256     | Ext. VGA   |
//! | 8            | 3            | 16.7M   | Ext. VGA / XGA |
//!
//! The crate already covered every row except the **`1 bpp × 2 planes`
//! CGA** mode — it implemented CGA only as the packed `2 bpp × 1 plane`
//! layout (4 pixels/byte). This is the plane-oriented sibling: each
//! scanline carries plane 0 then plane 1 one after another within the
//! row, and the bit at the same x-position in each plane stacks into the
//! 2-bit palette index (`p0 | p1 << 1`), exactly the bit ordering the
//! `1 bpp × 4 planes` EGA path already uses (plane k → bit k).
//!
//! r301 adds:
//!
//! * the `(1, 2)` arm in the canonical [`parse_pcx`] flatten path,
//! * the typed accessor [`parse_pcx_indexed_1bpp_2planes_cga`] returning
//!   [`PcxIndexed1x2Cga`],
//! * the writers [`encode_pcx_1bpp_2planes_cga`] and
//!   [`encode_pcx_1bpp_2planes_cga_dpi`].
//!
//! The CGA palette resolution (header byte 16 high nibble = background,
//! byte 19 bits 7/6 = palette family + intensity) is shared verbatim
//! with the packed `2 bpp × 1 plane` path, so the same physical colours
//! come out either way.

use oxideav_pcx::types::PCX_HEADER_SIZE;
use oxideav_pcx::{
    encode_pcx_1bpp_2planes_cga, encode_pcx_1bpp_2planes_cga_dpi, parse_pcx,
    parse_pcx_indexed_1bpp_2planes_cga, Pcx2bppCgaPaletteSource,
};

/// A small deterministic 2-bit-index pattern (`0,1,2,3` cycling) of the
/// given dimensions.
fn ramp_indices(w: usize, h: usize) -> Vec<u8> {
    (0..w * h).map(|i| (i % 4) as u8).collect()
}

#[test]
fn header_is_1bpp_2planes_cga() {
    let (w, h) = (13u16, 5u16);
    let idx = ramp_indices(w as usize, h as usize);
    // selector 0x00 = palette 1 high-intensity (cyan/magenta/white).
    let pcx = encode_pcx_1bpp_2planes_cga(w, h, &idx, 0x00, 0).unwrap();

    assert_eq!(pcx[0], 0x0A, "manufacturer byte");
    assert_eq!(pcx[1], 5, "version 5 (PCX 5.0)");
    assert_eq!(pcx[2], 1, "RLE encoding");
    assert_eq!(pcx[3], 1, "bits_per_pixel = 1");
    assert_eq!(pcx[65], 2, "n_planes = 2");
    // bytes_per_line is ceil(w/8) rounded up to even = ceil(13/8)=2 -> 2.
    let bpl = u16::from_le_bytes([pcx[66], pcx[67]]);
    assert_eq!(bpl, 2, "bytes_per_line for 13px at 1bpp");
    assert_eq!(bpl % 2, 0, "bytes_per_line must be even (spec §1)");
}

#[test]
fn typed_accessor_round_trips_indices() {
    let (w, h) = (17u16, 9u16);
    let idx = ramp_indices(w as usize, h as usize);
    let pcx = encode_pcx_1bpp_2planes_cga(w, h, &idx, 0x00, 0).unwrap();

    let view = parse_pcx_indexed_1bpp_2planes_cga(&pcx).unwrap();
    assert_eq!(view.width, w as u32);
    assert_eq!(view.height, h as u32);
    assert_eq!(view.indices.len(), w as usize * h as usize);
    assert_eq!(view.indices, idx, "indices survive encode -> decode");
    assert_eq!(view.stride(), w as usize);
}

#[test]
fn flatten_matches_typed_palette() {
    // Every palette family + the background override.
    for selector in [0x00u8, 0x40, 0x80, 0xC0] {
        for bg in [0u8, 4, 15] {
            let (w, h) = (8u16, 4u16);
            let idx = ramp_indices(w as usize, h as usize);
            let pcx = encode_pcx_1bpp_2planes_cga(w, h, &idx, selector, bg).unwrap();

            let view = parse_pcx_indexed_1bpp_2planes_cga(&pcx).unwrap();
            let img = parse_pcx(&pcx).unwrap();
            assert_eq!(img.width, w as u32);
            assert_eq!(img.height, h as u32);
            assert_eq!(view.background_index, bg);

            // Each flattened RGBA pixel must equal the typed view's
            // palette entry for that index (with 0xFF alpha).
            for (i, &index) in view.indices.iter().enumerate() {
                let [r, g, b] = view.palette[index as usize];
                assert_eq!(
                    &img.data[i * 4..i * 4 + 4],
                    &[r, g, b, 0xFF],
                    "selector={selector:#x} bg={bg} pixel={i}: flatten must match typed palette"
                );
            }
        }
    }
}

#[test]
fn palette_source_tracks_selector_bits() {
    // Manual C / P / I selector encodings (r401 conformance fix),
    // including the two composite-monochrome ramps the C bit unlocks.
    let cases = [
        (0x60u8, Pcx2bppCgaPaletteSource::Palette1HighIntensity),
        (0x40, Pcx2bppCgaPaletteSource::Palette1LowIntensity),
        (0x20, Pcx2bppCgaPaletteSource::Palette0HighIntensity),
        (0x00, Pcx2bppCgaPaletteSource::Palette0LowIntensity),
        (0x80, Pcx2bppCgaPaletteSource::MonochromeDim),
        (0xA0, Pcx2bppCgaPaletteSource::MonochromeBright),
    ];
    for (selector, expected) in cases {
        let pcx = encode_pcx_1bpp_2planes_cga(4, 2, &ramp_indices(4, 2), selector, 0).unwrap();
        let view = parse_pcx_indexed_1bpp_2planes_cga(&pcx).unwrap();
        assert_eq!(view.palette_source, expected, "selector {selector:#x}");
        // The helper round-trips the selector byte (top two bits).
        assert_eq!(view.palette_source.palette_selector(), selector);
    }
}

#[test]
fn same_colours_as_packed_2bpp_cga() {
    // The plane-oriented `1 bpp × 2 planes` mode and the packed
    // `2 bpp × 1 plane` mode resolve the same CGA palette from the same
    // header bytes, so identical indices must produce identical RGBA.
    let (w, h) = (12u16, 3u16);
    let idx = ramp_indices(w as usize, h as usize);
    let planar = encode_pcx_1bpp_2planes_cga(w, h, &idx, 0x40, 7).unwrap();
    let packed = oxideav_pcx::encode_pcx_2bpp_cga(w, h, &idx, 0x40, 7).unwrap();

    let a = parse_pcx(&planar).unwrap();
    let b = parse_pcx(&packed).unwrap();
    assert_eq!(
        a.data, b.data,
        "plane-oriented and packed CGA must flatten to identical pixels"
    );
}

#[test]
fn dpi_writer_stamps_offsets_12_14() {
    let (w, h) = (9u16, 9u16);
    let idx = ramp_indices(w as usize, h as usize);
    let plain = encode_pcx_1bpp_2planes_cga(w, h, &idx, 0x00, 0).unwrap();
    let dpi = encode_pcx_1bpp_2planes_cga_dpi(w, h, &idx, 0x00, 0, (300, 300)).unwrap();

    assert_eq!(u16::from_le_bytes([dpi[12], dpi[13]]), 300, "h_dpi");
    assert_eq!(u16::from_le_bytes([dpi[14], dpi[15]]), 300, "v_dpi");

    // Apart from the DPI words (offsets 12..16) the two outputs must be
    // byte-identical: the pixel region is untouched.
    assert_eq!(plain.len(), dpi.len());
    for (i, (&p, &d)) in plain.iter().zip(dpi.iter()).enumerate() {
        if (12..16).contains(&i) {
            continue;
        }
        assert_eq!(p, d, "byte {i} must match (only DPI words differ)");
    }

    // The stamped DPI surfaces on the decoded image.
    let img = parse_pcx(&dpi).unwrap();
    assert_eq!(img.dpi, Some((300, 300)));
    // The plain writer leaves the historical 72×72 default, which is
    // surfaced (non-zero) too.
    let plain_img = parse_pcx(&plain).unwrap();
    assert_eq!(plain_img.dpi, Some((72, 72)));
}

#[test]
fn dpi_writer_rejects_zero_component() {
    let idx = ramp_indices(4, 4);
    assert!(encode_pcx_1bpp_2planes_cga_dpi(4, 4, &idx, 0x00, 0, (0, 300)).is_err());
    assert!(encode_pcx_1bpp_2planes_cga_dpi(4, 4, &idx, 0x00, 0, (300, 0)).is_err());
}

#[test]
fn writer_rejects_bad_inputs() {
    let idx = ramp_indices(4, 4);
    // Zero dimension.
    assert!(encode_pcx_1bpp_2planes_cga(0, 4, &idx, 0x00, 0).is_err());
    assert!(encode_pcx_1bpp_2planes_cga(4, 0, &idx, 0x00, 0).is_err());
    // Short input.
    assert!(encode_pcx_1bpp_2planes_cga(4, 4, &idx[..4], 0x00, 0).is_err());
    // background_index out of range.
    assert!(encode_pcx_1bpp_2planes_cga(4, 4, &idx, 0x00, 16).is_err());
}

#[test]
fn typed_accessor_rejects_wrong_geometry() {
    // A packed 2 bpp × 1 plane CGA file must be rejected by the
    // 1 bpp × 2 planes accessor (and vice-versa).
    let idx = ramp_indices(4, 4);
    let packed = oxideav_pcx::encode_pcx_2bpp_cga(4, 4, &idx, 0x00, 0).unwrap();
    assert!(parse_pcx_indexed_1bpp_2planes_cga(&packed).is_err());

    let planar = encode_pcx_1bpp_2planes_cga(4, 4, &idx, 0x00, 0).unwrap();
    assert!(oxideav_pcx::parse_pcx_indexed_2bpp_cga(&planar).is_err());
}

#[test]
fn padding_stripped_for_odd_widths() {
    // Width 1: bytes_per_line rounds up to 2 (even), so each plane has a
    // padding byte. The visible index must still be exactly 1 per row.
    let (w, h) = (1u16, 3u16);
    let idx = vec![2u8, 3, 1];
    let pcx = encode_pcx_1bpp_2planes_cga(w, h, &idx, 0x00, 0).unwrap();
    let view = parse_pcx_indexed_1bpp_2planes_cga(&pcx).unwrap();
    assert_eq!(view.indices, idx, "1px-wide rows survive even-BPL padding");

    let bpl = u16::from_le_bytes([pcx[66], pcx[67]]);
    assert_eq!(bpl, 2, "1px -> ceil(1/8)=1 -> rounded up to even = 2");
    assert!(pcx.len() > PCX_HEADER_SIZE);
}
