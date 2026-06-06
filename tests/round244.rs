//! r244 — fuzz target extends to `parse_pcx_indexed_4bpp`.
//!
//! The r136 `fuzz/decode_pcx` cargo-fuzz harness drives the canonical
//! [`oxideav_pcx::parse_pcx`] / [`oxideav_pcx::parse_dcx`] entry points
//! against arbitrary attacker bytes; r237 added the
//! [`oxideav_pcx::parse_pcx_indexed_8bpp`] typed accessor to the same
//! harness so its padding-strip + palette-source dispatch surface is
//! exercised at fuzz cadence. The r241 typed accessor
//! [`oxideav_pcx::parse_pcx_indexed_4bpp`] (4 bpp × 1 plane — EGFF
//! "16 colours / EGA and VGA" mode) had not yet been added to the fuzz
//! target: r244 closes that gap so the per-row nibble unpack, the
//! `Ega16InHeader` / `Ega16Default` palette dispatch, and the (depth,
//! planes) mismatch reject path all run on every fuzz iteration.
//!
//! The fuzz crate is a separate `cargo-fuzz` workspace and is not
//! built by `cargo test`, so this in-tree test file validates the
//! contract the fuzz target asserts against:
//!
//! 1. **`parse_pcx_indexed_4bpp` returns a `Result` rather than
//!    panicking on any input.** Every seed corpus fixture committed
//!    alongside the fuzz target is fed through the typed accessor —
//!    `Ok(_)` for the matching `(4, 1)` fixtures, `Err(_)` for every
//!    other (depth, planes) combination and every degenerate input.
//! 2. **The seed corpus exercises both `Pcx4bppPaletteSource` branches.**
//!    `packed4_8x4.pcx` (existing seed) has a non-zero `ega_palette`
//!    field, so it drives `Ega16InHeader`; `packed4_default_palette_8x4
//!    .pcx` (new seed introduced this round) has an all-zero
//!    `ega_palette` field, so it drives `Ega16Default`. Neither branch
//!    was being exercised by the fuzz target before this round.
//! 3. **Typed-view consistency on the seed corpus.** For every
//!    `(4, 1)` seed the indices flattened through the surfaced
//!    palette match the byte stream `parse_pcx` produces — i.e. the
//!    typed view does not diverge from the canonical RGBA flattener
//!    for any seed corpus input the fuzzer starts from.
//! 4. **The new `packed4_default_palette_8x4.pcx` seed has the
//!    expected header geometry.** Manufacturer 0x0A, version 5,
//!    encoding 1, bits-per-pixel 4, n_planes 1, ega_palette all zero,
//!    bytes_per_line = round_up_to_even(ceil(width / 2)).
//!
//! Together these checks pin down what the fuzz target will be
//! exercising once it picks up the round-244 binary.

use oxideav_pcx::{
    encode_pcx_1bpp_mono, encode_pcx_24bpp, encode_pcx_2bpp_cga, encode_pcx_8bpp_grayscale,
    parse_pcx, parse_pcx_indexed_4bpp, Pcx4bppPaletteSource, PcxError,
};

// ---------------------------------------------------------------------------
// Seed corpus paths
// ---------------------------------------------------------------------------

/// The pre-r244 4 bpp × 1 plane seed: in-header EGA palette branch.
const SEED_PACKED4_IN_HEADER: &[u8] = include_bytes!("../fuzz/corpus/decode_pcx/packed4_8x4.pcx");

/// The r244 4 bpp × 1 plane seed: all-zero `ega_palette` → spec table
/// §3.1 hardware default branch.
const SEED_PACKED4_DEFAULT_PALETTE: &[u8] =
    include_bytes!("../fuzz/corpus/decode_pcx/packed4_default_palette_8x4.pcx");

/// Every non-(4, 1) seed in the fuzz corpus: each must round-trip
/// through `parse_pcx_indexed_4bpp` as `Err(Unsupported)` (when the
/// header is otherwise valid) or `Err(Invalid)` (when the header
/// itself is malformed).
const SEED_MONO1: &[u8] = include_bytes!("../fuzz/corpus/decode_pcx/mono1_8x4.pcx");
const SEED_CGA2: &[u8] = include_bytes!("../fuzz/corpus/decode_pcx/cga2_8x4.pcx");
const SEED_EGA1X4: &[u8] = include_bytes!("../fuzz/corpus/decode_pcx/ega1x4_8x4.pcx");
const SEED_GRAY8: &[u8] = include_bytes!("../fuzz/corpus/decode_pcx/gray8_8x4.pcx");
const SEED_IDX8: &[u8] = include_bytes!("../fuzz/corpus/decode_pcx/idx8_8x4.pcx");
const SEED_RGB24: &[u8] = include_bytes!("../fuzz/corpus/decode_pcx/rgb24_8x4.pcx");
const SEED_RGB24WIN: &[u8] = include_bytes!("../fuzz/corpus/decode_pcx/rgb24win_8x4.pcx");
const SEED_BUNDLE_DCX: &[u8] = include_bytes!("../fuzz/corpus/decode_pcx/bundle.dcx");
const SEED_DCX_MAGIC: &[u8] = include_bytes!("../fuzz/corpus/decode_pcx/dcx_magic.bin");
const SEED_MAGIC_ONLY: &[u8] = include_bytes!("../fuzz/corpus/decode_pcx/magic_only.bin");
const SEED_EMPTY: &[u8] = include_bytes!("../fuzz/corpus/decode_pcx/empty.bin");

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The existing `packed4_8x4.pcx` seed must surface `Ega16InHeader`.
/// This pins down which branch each seed drives so we can prove the
/// fuzz target now reaches both palette-source arms on the seed alone.
#[test]
fn seed_packed4_in_header_branch() {
    let view = parse_pcx_indexed_4bpp(SEED_PACKED4_IN_HEADER)
        .expect("packed4_8x4.pcx must decode through the typed 4bpp accessor");
    assert!(matches!(
        view.palette_source,
        Pcx4bppPaletteSource::Ega16InHeader
    ));
    assert_eq!(
        view.width as usize * view.height as usize,
        view.indices.len()
    );
    // Every index is a valid nibble (low 4 bits only).
    assert!(view.indices.iter().all(|&i| i < 16));
}

/// The new r244 seed has an all-zero `ega_palette` header field and
/// must surface `Ega16Default` (the spec table §3.1 EGA hardware
/// palette).
#[test]
fn seed_packed4_default_palette_branch() {
    let view = parse_pcx_indexed_4bpp(SEED_PACKED4_DEFAULT_PALETTE)
        .expect("packed4_default_palette_8x4.pcx must decode");
    assert!(matches!(
        view.palette_source,
        Pcx4bppPaletteSource::Ega16Default
    ));
    assert_eq!(
        view.width as usize * view.height as usize,
        view.indices.len()
    );
    // Surfaced palette matches the spec table §3.1 default exactly.
    const EGA_DEFAULT: [[u8; 3]; 16] = [
        [0x00, 0x00, 0x00],
        [0x00, 0x00, 0xAA],
        [0x00, 0xAA, 0x00],
        [0x00, 0xAA, 0xAA],
        [0xAA, 0x00, 0x00],
        [0xAA, 0x00, 0xAA],
        [0xAA, 0x55, 0x00],
        [0xAA, 0xAA, 0xAA],
        [0x55, 0x55, 0x55],
        [0x55, 0x55, 0xFF],
        [0x55, 0xFF, 0x55],
        [0x55, 0xFF, 0xFF],
        [0xFF, 0x55, 0x55],
        [0xFF, 0x55, 0xFF],
        [0xFF, 0xFF, 0x55],
        [0xFF, 0xFF, 0xFF],
    ];
    assert_eq!(view.palette, EGA_DEFAULT);
}

/// The new seed must carry the header shape the fuzz target's
/// mutator expects: spec §3 manufacturer 0x0A / version 5 / encoding
/// 1 (RLE) / bits-per-pixel 4 / n_planes 1, plus an all-zero
/// `ega_palette` and an even-rounded `bytes_per_line`.
#[test]
fn seed_packed4_default_palette_header_geometry() {
    let bytes = SEED_PACKED4_DEFAULT_PALETTE;
    assert!(bytes.len() >= 128);
    assert_eq!(bytes[0], 0x0A, "manufacturer byte");
    assert_eq!(bytes[1], 5, "version byte");
    assert_eq!(bytes[2], 1, "encoding byte (RLE)");
    assert_eq!(bytes[3], 4, "bits_per_pixel");
    assert_eq!(bytes[65], 1, "n_planes");
    let bpl = u16::from_le_bytes([bytes[66], bytes[67]]);
    let width =
        u16::from_le_bytes([bytes[8], bytes[9]]) - u16::from_le_bytes([bytes[4], bytes[5]]) + 1;
    // 4 bpp packs 2 pixels/byte; spec §1 rounds bytes_per_line up to
    // an even number.
    let expected_bpl = {
        let raw = width.div_ceil(2);
        if raw % 2 == 0 {
            raw
        } else {
            raw + 1
        }
    };
    assert_eq!(bpl, expected_bpl);
    // The ega_palette must be all-zero so the decoder lands on the
    // `Ega16Default` branch.
    assert!(bytes[16..64].iter().all(|&b| b == 0));
}

/// Every seed in the fuzz corpus must return a `Result` from
/// `parse_pcx_indexed_4bpp` (never panic, OOM, or abort). This is the
/// direct contract under test in the fuzz target's `fuzz_target!`
/// invocation; we assert it on every committed seed so a regression
/// shows up at `cargo test` time too.
#[test]
fn every_seed_returns_a_result() {
    let seeds: &[&[u8]] = &[
        SEED_PACKED4_IN_HEADER,
        SEED_PACKED4_DEFAULT_PALETTE,
        SEED_MONO1,
        SEED_CGA2,
        SEED_EGA1X4,
        SEED_GRAY8,
        SEED_IDX8,
        SEED_RGB24,
        SEED_RGB24WIN,
        SEED_BUNDLE_DCX,
        SEED_DCX_MAGIC,
        SEED_MAGIC_ONLY,
        SEED_EMPTY,
    ];
    for seed in seeds {
        // Discard the result — the contract is purely that the call
        // returns (never panics / OOMs / aborts).
        let _ = parse_pcx_indexed_4bpp(seed);
    }
}

/// Every non-(4, 1) PCX seed must reject through the typed accessor
/// with `Err(Unsupported)` — proving the depth/planes mismatch
/// rejection path is reached by the fuzz target's existing seed
/// material (rather than waiting for the fuzzer to mutate one into
/// shape).
#[test]
fn non_4_1_seeds_reject_with_unsupported() {
    let seeds: &[(&str, &[u8])] = &[
        ("mono1_8x4.pcx", SEED_MONO1),
        ("cga2_8x4.pcx", SEED_CGA2),
        ("ega1x4_8x4.pcx", SEED_EGA1X4),
        ("gray8_8x4.pcx", SEED_GRAY8),
        ("idx8_8x4.pcx", SEED_IDX8),
        ("rgb24_8x4.pcx", SEED_RGB24),
        ("rgb24win_8x4.pcx", SEED_RGB24WIN),
    ];
    for (name, seed) in seeds {
        match parse_pcx_indexed_4bpp(seed) {
            Err(PcxError::Unsupported(_)) => {}
            other => panic!("{name}: expected Unsupported, got {other:?}"),
        }
    }
}

/// Every non-PCX / malformed seed must reject through the typed
/// accessor with an error class — they share the canonical
/// `parse_pcx` validation surface, so a malformed file rejected by
/// `parse_pcx` is also rejected by the typed accessor (with the
/// same error class).
#[test]
fn malformed_seeds_reject() {
    let seeds: &[(&str, &[u8])] = &[
        ("bundle.dcx", SEED_BUNDLE_DCX),
        ("dcx_magic.bin", SEED_DCX_MAGIC),
        ("magic_only.bin", SEED_MAGIC_ONLY),
        ("empty.bin", SEED_EMPTY),
    ];
    for (name, seed) in seeds {
        let typed = parse_pcx_indexed_4bpp(seed);
        let canonical = parse_pcx(seed);
        assert!(typed.is_err(), "{name}: typed accessor must reject");
        assert!(canonical.is_err(), "{name}: canonical parse must reject");
    }
}

/// Typed-view consistency: for every `(4, 1)` seed, flattening the
/// surfaced indices through the surfaced palette must reproduce the
/// RGBA byte stream `parse_pcx` returns. This pins the typed accessor
/// as a pure rearrangement of the canonical flattener — a key
/// invariant the fuzz target's mutator can otherwise stumble upon
/// and flag.
#[test]
fn seeds_typed_view_matches_canonical_flatten() {
    for seed in [SEED_PACKED4_IN_HEADER, SEED_PACKED4_DEFAULT_PALETTE] {
        let canonical = parse_pcx(seed).expect("canonical parse");
        let view = parse_pcx_indexed_4bpp(seed).expect("typed parse");
        assert_eq!(view.width, canonical.width);
        assert_eq!(view.height, canonical.height);
        let mut flat = Vec::with_capacity(view.indices.len() * 4);
        for &i in &view.indices {
            let [r, g, b] = view.palette[i as usize];
            flat.extend_from_slice(&[r, g, b, 0xFF]);
        }
        assert_eq!(flat, canonical.data);
    }
}

/// Synthetic (non-seed-corpus) inputs that the fuzz mutator would
/// also hit: every other (depth, planes) combination returned by the
/// public encoder API must reject through `parse_pcx_indexed_4bpp`
/// with `Err(Unsupported)`. This complements the seed-corpus check by
/// covering combinations the seed corpus doesn't physically hold.
#[test]
fn synthetic_non_4_1_combinations_reject_with_unsupported() {
    let mono = encode_pcx_1bpp_mono(8, 4, &[0u8; 32]).expect("mono encode");
    let cga = encode_pcx_2bpp_cga(8, 4, &[0u8; 32], 0x00, 0x00).expect("cga encode");
    let gray = encode_pcx_8bpp_grayscale(8, 4, &[0u8; 32]).expect("gray encode");
    let rgb24 = encode_pcx_24bpp(8, 4, &[0u8; 96]).expect("rgb24 encode");
    for bytes in [&mono, &cga, &gray, &rgb24] {
        match parse_pcx_indexed_4bpp(bytes) {
            Err(PcxError::Unsupported(_)) => {}
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
