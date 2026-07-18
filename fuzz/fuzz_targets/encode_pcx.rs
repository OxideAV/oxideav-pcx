#![no_main]

//! Drive every public PCX *encoder* off attacker-controlled dimensions
//! and a fuzz-supplied pixel/index buffer, then feed the bytes each one
//! produces back through the decoders.
//!
//! The decode harness (`decode_pcx`) proves the *reader* never panics on
//! hostile input. This is the symmetric encoder contract: each
//! `encode_pcx_*` surface stamps a 128-byte header from caller-supplied
//! `(width, height)`, computes a per-plane `bytes_per_line`, allocates a
//! row buffer, packs the input (nibble / 2-bit / 1-bit-plane / planar
//! byte / `u16`-composite — each its own arithmetic), and RLE-runs the
//! result. None of that may panic, integer-overflow (debug), index out
//! of bounds, or pre-allocate beyond what the inputs back, regardless of
//! how the fuzzer chooses `(width, height)` versus the buffer length. An
//! encoder must either return `Ok(Vec<u8>)` or `Err(PcxError::…)`.
//!
//! Two `u16` dimensions are carved off the front of the fuzz input and
//! masked down to a sane on-disk ceiling so the harness exercises the
//! interesting packing geometry (odd widths exercising the even-
//! `bytes_per_line` padding, multi-pixel-per-byte chunk boundaries)
//! without the fuzzer trivially driving a multi-gigabyte allocation off
//! a `0xFFFF × 0xFFFF` claim — the encoders accept any `u16`, but a
//! 4-billion-pixel buffer is an out-of-memory test of the allocator, not
//! of PCX logic. The remainder of the input is the pixel/index payload;
//! each encoder slices the prefix it needs and ignores the rest.
//!
//! Where an encoder succeeds, its bytes are fed straight back into
//! `parse_pcx` and the matching typed accessor: an encoder that emits a
//! header the decoder then panics on would be just as much a defect as a
//! decoder panic on a third-party file, so the encode→decode seam is
//! under the same no-panic contract here.
//!
//! Built `default-features = false` (crate-local `PcxError`, no
//! `oxideav-core`), matching the decode harness.

use libfuzzer_sys::fuzz_target;
use oxideav_pcx::{
    encode_pcx_1bpp_2planes_cga, encode_pcx_1bpp_3planes_ega_rgb, encode_pcx_1bpp_4planes_ega,
    encode_pcx_1bpp_mono, encode_pcx_24bpp, encode_pcx_2bpp_cga, encode_pcx_4bpp_4planes,
    encode_pcx_4bpp_packed, encode_pcx_8bpp_grayscale, encode_pcx_8bpp_indexed,
    encode_pcx_indexed_auto, encode_pcx_rgb_auto, parse_pcx, parse_pcx_indexed_1bpp_3planes,
    parse_pcx_indexed_1bpp_4planes, parse_pcx_indexed_2bpp_cga, parse_pcx_indexed_4bpp,
    parse_pcx_indexed_4bpp_4planes, parse_pcx_indexed_8bpp, PcxAutoMode, PcxPaletteSource,
};

/// Cap each dimension so the dimension×dimension pixel count stays well
/// under what a fuzz iteration can reasonably allocate. 0x1FF keeps the
/// worst case (`24bpp`, 3 bytes/pixel) at ~768 KiB per encode while still
/// spanning every packing edge: odd vs even widths, sub-byte chunk
/// boundaries (2 / 4 / 8 pixels per byte), and `bytes_per_line` rounding.
const DIM_MASK: u16 = 0x01FF;

fuzz_target!(|data: &[u8]| {
    // Need at least four bytes for the two dimension words; below that
    // there is nothing useful to encode.
    if data.len() < 4 {
        return;
    }
    let width = u16::from_le_bytes([data[0], data[1]]) & DIM_MASK;
    let height = u16::from_le_bytes([data[2], data[3]]) & DIM_MASK;
    let payload = &data[4..];

    // Each encoder slices the prefix of `payload` it needs; a payload
    // shorter than `width × height × bytes_per_pixel` makes the encoder
    // take its documented "input shorter than …" reject path, which is
    // itself part of the surface under test.

    // Single-byte-per-pixel index / sample inputs.
    if let Ok(bytes) = encode_pcx_8bpp_indexed(width, height, payload, &gray_ramp_768()) {
        let _ = parse_pcx(&bytes);
        let _ = parse_pcx_indexed_8bpp(&bytes);
    }
    // Grayscale: `palette_info = 2` decodes each sample byte `g`
    // straight to `(g, g, g)`, so the fuzz payload itself is the
    // oracle. The decode may legitimately `Err` when the RLE stream
    // happens to place the `0x0C` tail-palette marker byte exactly 769
    // bytes from EOF (the (8, 1) tail probe then mis-claims real pixel
    // data — see the probe-confinement note in `decoder.rs`), so only
    // an `Ok` decode is held to the pixel oracle.
    if let Ok(bytes) = encode_pcx_8bpp_grayscale(width, height, payload) {
        if let Ok(img) = parse_pcx(&bytes) {
            for (i, px) in img.data.chunks_exact(4).enumerate() {
                let g = payload[i];
                assert_eq!(
                    &px[..3],
                    &[g, g, g],
                    "grayscale sample must decode as (g, g, g)"
                );
            }
        }
    }
    // Mono: bit polarity is pinned by the reference doc's errata
    // (Issue #227) — the bit value is the colormap index, and the
    // writer stores black / white in entries 0 / 1, so a non-zero
    // input byte (bit 1) must decode white and a zero byte black.
    // Attacker-driven `(width, height)` sweeps the polarity oracle
    // across every row-phase / padding geometry.
    if let Ok(bytes) = encode_pcx_1bpp_mono(width, height, payload) {
        let img = parse_pcx(&bytes).expect("mono writer output must decode");
        for (i, px) in img.data.chunks_exact(4).enumerate() {
            let want = if payload[i] != 0 { 0xFF } else { 0x00 };
            assert_eq!(
                &px[..3],
                &[want, want, want],
                "mono polarity: bit 1 = white / bit 0 = black"
            );
        }
    }

    if let Ok(bytes) = encode_pcx_4bpp_packed(width, height, payload, &ega_default_48()) {
        let _ = parse_pcx(&bytes);
        let _ = parse_pcx_indexed_4bpp(&bytes);
    }
    if let Ok(bytes) = encode_pcx_1bpp_4planes_ega(width, height, payload, &ega_default_48()) {
        let _ = parse_pcx(&bytes);
        let _ = parse_pcx_indexed_1bpp_4planes(&bytes);
    }

    // CGA selector / background bytes pulled from the payload so the
    // selector dispatch is attacker-driven; background masked to its
    // documented 0..=15 range so the value-range reject path doesn't
    // swallow every iteration.
    let selector = payload.first().copied().unwrap_or(0);
    let background = payload.get(1).copied().unwrap_or(0) & 0x0F;
    if let Ok(bytes) = encode_pcx_2bpp_cga(width, height, payload, selector, background) {
        let _ = parse_pcx(&bytes);
        // Packed CGA stores the low two bits of each input byte at
        // four pixels per byte — same index oracle as the
        // plane-oriented layout below.
        let cga = parse_pcx_indexed_2bpp_cga(&bytes).expect("CGA 2bpp writer output must decode");
        assert_eq!(
            cga.background_index, background,
            "CGA background must round-trip"
        );
        for (i, &idx) in cga.indices.iter().enumerate() {
            assert_eq!(idx, payload[i] & 0x03, "CGA 2bpp index must round-trip");
        }
    }
    // Plane-oriented CGA: the packer takes the low two bits of each
    // input byte (bit 0 → plane 0, bit 1 → plane 1), so the typed
    // accessor must hand back exactly `input & 0x03` per pixel plus
    // the selector geometry it was given.
    if let Ok(bytes) = encode_pcx_1bpp_2planes_cga(width, height, payload, selector, background) {
        let _ = parse_pcx(&bytes);
        let cga = oxideav_pcx::parse_pcx_indexed_1bpp_2planes_cga(&bytes)
            .expect("CGA 1bpp×2 writer output must decode");
        assert_eq!(
            cga.background_index, background,
            "CGA background must round-trip"
        );
        for (i, &idx) in cga.indices.iter().enumerate() {
            assert_eq!(idx, payload[i] & 0x03, "CGA 1bpp×2 index must round-trip");
        }
    }

    // Three-bytes-per-pixel packed-RGB inputs.
    if let Ok(bytes) = encode_pcx_24bpp(width, height, payload) {
        let _ = parse_pcx(&bytes);
    }
    // Compact-mode auto writer: whichever branch it takes (indexed or
    // planar), the produced file must (a) decode without panicking and
    // (b) round-trip the source RGB *exactly* — both candidates are
    // lossless by construction, so any divergence is a defect. Only
    // assert the round-trip when the encoder consumed the whole
    // `width × height × 3` prefix (it borrows the first that many bytes).
    if let Ok((bytes, _mode)) = encode_pcx_rgb_auto(width, height, payload) {
        if let Ok(img) = parse_pcx(&bytes) {
            let n = width as usize * height as usize;
            if payload.len() >= n * 3 {
                let mut ok = true;
                for (i, px) in img.data.chunks_exact(4).enumerate() {
                    let src = &payload[i * 3..i * 3 + 3];
                    if &px[..3] != src {
                        ok = false;
                        break;
                    }
                }
                assert!(ok, "encode_pcx_rgb_auto must round-trip RGB exactly");
            }
        }
    }
    // Low-colour variant of the same auto-ladder invariant: raw fuzz
    // payloads are high-entropy (usually > 256 distinct colours), so
    // the compact rungs (mono / EGA-RGB / CGA / 4-bit / grayscale /
    // indexed) would almost never fire off `payload` directly. Quantise
    // down to a tiny palette whose SIZE and CONTENT the fuzzer
    // controls: entries come from payload bytes, per-pixel picks from
    // the payload head, and half the entries are biased onto the
    // special levels the compact rungs key on (0x00 / 0x55 / 0xAA /
    // 0xFF), so bilevel / primary / grey / CGA-shaped palettes are all
    // reachable. The round-trip must stay exact for every rung the
    // ladder picks — pixel capacity is capped so the up-to-nine
    // candidate encodes stay cheap per iteration.
    {
        let n = width as usize * height as usize;
        if payload.len() >= 4 && n > 0 && n <= 1 << 14 {
            let pal_len = 1 + (payload[0] as usize % 17); // 1..=17 colours
            let mut palette = Vec::with_capacity(pal_len);
            for k in 0..pal_len {
                let base = (k * 3) % (payload.len() - 2);
                let px = [payload[base], payload[base + 1], payload[base + 2]];
                let px = if k % 2 == 0 {
                    px.map(|v| [0x00u8, 0x55, 0xAA, 0xFF][(v >> 6) as usize])
                } else {
                    px
                };
                palette.push(px);
            }
            let mut rgb = Vec::with_capacity(n * 3);
            for i in 0..n {
                let sel = payload[1 + i % (payload.len() - 1)] as usize % pal_len;
                rgb.extend_from_slice(&palette[sel]);
            }
            let (bytes, _mode) =
                encode_pcx_rgb_auto(width, height, &rgb).expect("low-colour auto encode");
            let img = parse_pcx(&bytes).expect("low-colour auto output must decode");
            for (i, px) in img.data.chunks_exact(4).enumerate() {
                assert_eq!(
                    &px[..3],
                    &rgb[i * 3..i * 3 + 3],
                    "low-colour auto ladder must round-trip exactly"
                );
            }
        }
    }
    // Caller-palette rung (r417): `encode_pcx_indexed_auto` must store a
    // fuzz-chosen palette VERBATIM in one of the three palette-verbatim
    // geometries (4 bpp packed / 1 bpp × 4 planes header colormap, or
    // 8 bpp + VGA tail) and round-trip both the indices and the palette
    // prefix exactly through the typed accessor matching the reported
    // mode. Palette SIZE (1..=256 entries) and CONTENT are attacker
    // data; the index buffer is masked through a fuzz-selected width
    // (1 / 2 / 4 / 8 significant bits) so the ≤ 15-index precondition of
    // the header rungs fires often instead of almost never on
    // high-entropy bytes — and the unmasked arm keeps the out-of-table
    // index path (indices at/beyond the entry count resolve to the zero
    // padding) under fuzz too.
    {
        let n = width as usize * height as usize;
        if payload.len() >= 3 && n > 0 && n <= 1 << 14 {
            let entries = 1 + payload[0] as usize; // 1..=256
            let mask = [0x01u8, 0x03, 0x0F, 0xFF][(payload[1] & 3) as usize];
            let pal: Vec<u8> = (0..entries * 3)
                .map(|i| payload[2 + i % (payload.len() - 2)])
                .collect();
            let idx: Vec<u8> = (0..n).map(|i| payload[i % payload.len()] & mask).collect();
            let (bytes, mode) =
                encode_pcx_indexed_auto(width, height, &idx, &pal).expect("caller-palette encode");
            let (got_idx, got_pal): (Vec<u8>, Vec<u8>) = match mode {
                PcxAutoMode::Indexed4 { .. } => {
                    let v = parse_pcx_indexed_4bpp(&bytes)
                        .expect("caller-palette 4bpp output must decode");
                    (v.indices, v.palette.iter().flatten().copied().collect())
                }
                PcxAutoMode::Indexed1x4 { .. } => {
                    let v = parse_pcx_indexed_1bpp_4planes(&bytes)
                        .expect("caller-palette 1bpp×4 output must decode");
                    (v.indices, v.palette.iter().flatten().copied().collect())
                }
                PcxAutoMode::Indexed8 { .. } => {
                    let v = parse_pcx_indexed_8bpp(&bytes)
                        .expect("caller-palette 8bpp output must decode");
                    assert_eq!(
                        v.palette_source,
                        PcxPaletteSource::VgaTail,
                        "the VGA-tail rung must be tail-resolved"
                    );
                    (v.indices, v.palette.iter().flatten().copied().collect())
                }
                other => panic!("caller-palette ladder emitted a non-verbatim rung: {other:?}"),
            };
            assert_eq!(got_idx, idx, "caller-palette indices must round-trip");
            assert_eq!(
                &got_pal[..pal.len()],
                &pal[..],
                "caller palette entries must round-trip verbatim"
            );
            assert!(
                got_pal[pal.len()..].iter().all(|&b| b == 0),
                "on-disk palette padding must stay zero"
            );
        }
    }
    if let Ok(bytes) = encode_pcx_1bpp_3planes_ega_rgb(width, height, payload) {
        let _ = parse_pcx(&bytes);
        let _ = parse_pcx_indexed_1bpp_3planes(&bytes);
    }

    // `u16`-per-pixel composite-index input for the (4, 4) mode: two
    // payload bytes per pixel, little-endian.
    let composite: Vec<u16> = payload
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    if let Ok(bytes) = encode_pcx_4bpp_4planes(width, height, &composite) {
        let _ = parse_pcx_indexed_4bpp_4planes(&bytes);
    }
});

/// 256-entry grayscale-ramp VGA palette (768 bytes) for the 8-bpp
/// indexed encoder, which requires an exactly-768-byte palette argument.
fn gray_ramp_768() -> [u8; 768] {
    let mut p = [0u8; 768];
    for (i, chunk) in p.chunks_exact_mut(3).enumerate() {
        let v = i as u8;
        chunk[0] = v;
        chunk[1] = v;
        chunk[2] = v;
    }
    p
}

/// 16-entry (48-byte) EGA palette for the 4-bpp / 1bpp×4 encoders, which
/// require an exactly-48-byte palette argument. The exact colours are
/// irrelevant to the no-panic contract; a deterministic ramp suffices.
fn ega_default_48() -> [u8; 48] {
    let mut p = [0u8; 48];
    for (i, chunk) in p.chunks_exact_mut(3).enumerate() {
        let v = (i as u8) << 4;
        chunk[0] = v;
        chunk[1] = v;
        chunk[2] = v;
    }
    p
}
