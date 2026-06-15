//! r319 — EGA hardware 4-level palette quantisation
//! (spec §"EGA/VGA 16-color palette").
//!
//! The rev-5 ZSoft PCX Technical Reference Manual notes that "on an IBM
//! EGA there are only 4 levels of RGB for each color. Since 256/4 = 64,
//! the following is a list of the settings and levels":
//!
//! | Setting   | Level |
//! | --------- | ----: |
//! | 0–63      | 0     |
//! | 64–127    | 1     |
//! | 128–192   | 2     |
//! | 193–254   | 3     |
//!
//! A PCX header `Colormap` stores 0..=255 component values, but an EGA
//! display only resolves those four levels, so a file authored on (or
//! for) EGA hardware whose header palette carries arbitrary 0..=255
//! values is shown with each component snapped to one of the four EGA
//! DAC output intensities (`0x00 / 0x55 / 0xAA / 0xFF` — exactly the
//! component values the manual's default 16-colour EGA palette is built
//! from).
//!
//! r319 surfaces that spec behaviour, previously unexposed:
//!
//! * [`oxideav_pcx::ega_quantize_level`] — maps a stored 0..=255 byte to
//!   its EGA level `0..=3` per the table above.
//! * [`oxideav_pcx::ega_quantize_component`] — composes the level map
//!   with the EGA DAC output ramp.
//! * [`oxideav_pcx::ega_quantize_palette`] — applies it across a
//!   16-entry palette.
//! * [`oxideav_pcx::parse_pcx_indexed_4bpp_ega_hw`] — the EGA-hardware
//!   sibling of [`oxideav_pcx::parse_pcx_indexed_4bpp`]: same indices and
//!   palette-source tag, palette snapped to the EGA-displayable colours.
//!
//! This file tests:
//!
//! 1. The level table boundaries match the spec exactly (every bucket
//!    edge), including the 255 case the manual's table stops short of.
//! 2. `ega_quantize_component` maps each level to the EGA DAC ramp and is
//!    idempotent for values already on the ramp.
//! 3. `parse_pcx_indexed_4bpp_ega_hw` preserves the indices and the
//!    palette-source tag while snapping the palette.
//! 4. A file whose header palette is already on the EGA ramp is a no-op
//!    (the EGA-hw view equals the raw view).
//! 5. The `Ega16Default` branch (all-zero header palette → spec table
//!    §3.1 default) is unchanged by quantisation — the default is already
//!    on the ramp.
//! 6. The accessor rejects every non-`(4, 1)` (depth, planes) combination.

use oxideav_pcx::{
    ega_quantize_component, ega_quantize_level, ega_quantize_palette, encode_pcx_24bpp,
    encode_pcx_4bpp_packed, encode_pcx_8bpp_grayscale, parse_pcx_indexed_4bpp,
    parse_pcx_indexed_4bpp_ega_hw, Pcx4bppPaletteSource, PcxError,
};

// ---------------------------------------------------------------------------
// 1. Level table boundaries (spec §"EGA/VGA 16-color palette").
// ---------------------------------------------------------------------------

#[test]
fn ega_level_table_boundaries() {
    // 0–63 → 0
    assert_eq!(ega_quantize_level(0), 0);
    assert_eq!(ega_quantize_level(63), 0);
    // 64–127 → 1
    assert_eq!(ega_quantize_level(64), 1);
    assert_eq!(ega_quantize_level(127), 1);
    // 128–192 → 2
    assert_eq!(ega_quantize_level(128), 2);
    assert_eq!(ega_quantize_level(192), 2);
    // 193–254 → 3 (and 255 falls in the same top bucket)
    assert_eq!(ega_quantize_level(193), 3);
    assert_eq!(ega_quantize_level(254), 3);
    assert_eq!(ega_quantize_level(255), 3);
}

#[test]
fn ega_level_is_monotonic_non_decreasing() {
    let mut prev = 0u8;
    for v in 0u16..=255 {
        let lvl = ega_quantize_level(v as u8);
        assert!(lvl >= prev, "level must be non-decreasing at {v}");
        assert!(lvl <= 3, "level must stay in 0..=3 at {v}");
        prev = lvl;
    }
}

// ---------------------------------------------------------------------------
// 2. Component map = level → EGA DAC ramp, and idempotence on the ramp.
// ---------------------------------------------------------------------------

#[test]
fn ega_component_maps_each_level_to_dac_ramp() {
    // Level 0/1/2/3 → 0x00 / 0x55 / 0xAA / 0xFF.
    assert_eq!(ega_quantize_component(0), 0x00); // level 0
    assert_eq!(ega_quantize_component(63), 0x00);
    assert_eq!(ega_quantize_component(64), 0x55); // level 1
    assert_eq!(ega_quantize_component(127), 0x55);
    assert_eq!(ega_quantize_component(128), 0xAA); // level 2
    assert_eq!(ega_quantize_component(192), 0xAA);
    assert_eq!(ega_quantize_component(193), 0xFF); // level 3
    assert_eq!(ega_quantize_component(255), 0xFF);
}

#[test]
fn ega_component_idempotent_on_ramp() {
    for &v in &[0x00u8, 0x55, 0xAA, 0xFF] {
        assert_eq!(
            ega_quantize_component(v),
            v,
            "ramp value {v:#x} must be fixed"
        );
        // A second pass changes nothing.
        assert_eq!(ega_quantize_component(ega_quantize_component(v)), v);
    }
}

#[test]
fn ega_quantize_palette_routes_every_component() {
    // An arbitrary off-ramp palette: each component snaps independently.
    let mut pal = [[0u8; 3]; 16];
    for (i, e) in pal.iter_mut().enumerate() {
        let base = (i * 16) as u8;
        *e = [base, base.wrapping_add(70), base.wrapping_add(200)];
    }
    let q = ega_quantize_palette(&pal);
    for (orig, snapped) in pal.iter().zip(q.iter()) {
        for c in 0..3 {
            assert_eq!(snapped[c], ega_quantize_component(orig[c]));
            assert!(matches!(snapped[c], 0x00 | 0x55 | 0xAA | 0xFF));
        }
    }
}

// ---------------------------------------------------------------------------
// 3 + 4. parse_pcx_indexed_4bpp_ega_hw vs parse_pcx_indexed_4bpp.
// ---------------------------------------------------------------------------

/// Build a 4 bpp × 1 plane PCX with the given flat 48-byte palette.
fn make_4bpp(width: u16, height: u16, palette48: &[u8; 48]) -> Vec<u8> {
    let n = width as usize * height as usize;
    // Use index = pixel-position mod 16 so all 16 entries get exercised.
    let indices: Vec<u8> = (0..n).map(|i| (i % 16) as u8).collect();
    encode_pcx_4bpp_packed(width, height, &indices, palette48).expect("encode 4bpp")
}

#[test]
fn ega_hw_view_snaps_off_ramp_palette_but_keeps_indices() {
    // An off-ramp scanner-style palette (values not on 0x00/0x55/0xAA/0xFF).
    let mut palette48 = [0u8; 48];
    for (i, byte) in palette48.iter_mut().enumerate() {
        // Spread across the four buckets deterministically.
        *byte = ((i * 17 + 30) % 256) as u8;
    }
    // Guarantee at least one non-zero byte so the Ega16InHeader branch fires.
    palette48[0] = 200;

    let pcx = make_4bpp(8, 4, &palette48);
    let raw = parse_pcx_indexed_4bpp(&pcx).expect("raw view");
    let ega = parse_pcx_indexed_4bpp_ega_hw(&pcx).expect("ega-hw view");

    // Same geometry, indices, and palette-source tag.
    assert_eq!(raw.width, ega.width);
    assert_eq!(raw.height, ega.height);
    assert_eq!(raw.indices, ega.indices);
    assert_eq!(raw.palette_source, ega.palette_source);
    assert_eq!(raw.palette_source, Pcx4bppPaletteSource::Ega16InHeader);

    // The EGA-hw palette equals the raw palette routed through the
    // component quantiser, and every component is on the EGA ramp.
    for (r, q) in raw.palette.iter().zip(ega.palette.iter()) {
        for c in 0..3 {
            assert_eq!(q[c], ega_quantize_component(r[c]));
            assert!(matches!(q[c], 0x00 | 0x55 | 0xAA | 0xFF));
        }
    }
    // At least one entry actually moved (the palette was off-ramp).
    assert_ne!(raw.palette, ega.palette);
}

#[test]
fn ega_hw_view_noop_for_on_ramp_palette() {
    // A palette already entirely on the EGA DAC ramp — quantising is a
    // no-op, so the EGA-hw view equals the raw view bit-for-bit.
    let ramp = [0x00u8, 0x55, 0xAA, 0xFF];
    let mut palette48 = [0u8; 48];
    for (i, byte) in palette48.iter_mut().enumerate() {
        *byte = ramp[(i / 3 + i % 3) % 4];
    }
    palette48[0] = 0xFF; // ensure non-zero → Ega16InHeader

    let pcx = make_4bpp(8, 4, &palette48);
    let raw = parse_pcx_indexed_4bpp(&pcx).expect("raw view");
    let ega = parse_pcx_indexed_4bpp_ega_hw(&pcx).expect("ega-hw view");
    assert_eq!(raw.palette, ega.palette);
    assert_eq!(raw.indices, ega.indices);
}

#[test]
fn ega_hw_view_default_palette_unchanged() {
    // All-zero header palette → spec table §3.1 EGA hardware default.
    // That default is built from the EGA ramp, so quantisation is a
    // no-op and the two views agree.
    let palette48 = [0u8; 48];
    let pcx = make_4bpp(8, 4, &palette48);
    let raw = parse_pcx_indexed_4bpp(&pcx).expect("raw view");
    let ega = parse_pcx_indexed_4bpp_ega_hw(&pcx).expect("ega-hw view");
    assert_eq!(raw.palette_source, Pcx4bppPaletteSource::Ega16Default);
    assert_eq!(ega.palette_source, Pcx4bppPaletteSource::Ega16Default);
    assert_eq!(raw.palette, ega.palette);
}

// ---------------------------------------------------------------------------
// 6. (depth, planes) scope — rejects every non-(4, 1) combination.
// ---------------------------------------------------------------------------

#[test]
fn ega_hw_view_rejects_non_4bpp_modes() {
    // 24-bit (8 bpp × 3 planes).
    let rgb = vec![0u8; 8 * 4 * 3];
    let pcx_24 = encode_pcx_24bpp(8, 4, &rgb).expect("encode 24bpp");
    assert!(matches!(
        parse_pcx_indexed_4bpp_ega_hw(&pcx_24),
        Err(PcxError::Unsupported(_))
    ));

    // 8 bpp × 1 plane grayscale.
    let gray = vec![0u8; 8 * 4];
    let pcx_8 = encode_pcx_8bpp_grayscale(8, 4, &gray).expect("encode 8bpp gray");
    assert!(matches!(
        parse_pcx_indexed_4bpp_ega_hw(&pcx_8),
        Err(PcxError::Unsupported(_))
    ));
}
