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
    encode_pcx_rgb_auto, parse_pcx, parse_pcx_indexed_1bpp_3planes, parse_pcx_indexed_1bpp_4planes,
    parse_pcx_indexed_2bpp_cga, parse_pcx_indexed_4bpp, parse_pcx_indexed_4bpp_4planes,
    parse_pcx_indexed_8bpp,
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
    let _ = encode_pcx_8bpp_grayscale(width, height, payload);
    let _ = encode_pcx_1bpp_mono(width, height, payload);

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
        let _ = parse_pcx_indexed_2bpp_cga(&bytes);
    }
    let _ = encode_pcx_1bpp_2planes_cga(width, height, payload, selector, background);

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
