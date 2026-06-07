#![no_main]

//! Decode arbitrary fuzz-supplied bytes through the PCX and DCX
//! decoders. Both must always return a `Result` and never panic /
//! abort / OOM, regardless of how malformed the input is.
//!
//! The contract under test is purely that the calls *return*: a
//! malformed stream yields `Err(PcxError::…)`, a well-formed one yields
//! `Ok(PcxImage)` / `Ok(DcxImage)`, and neither path may panic,
//! integer-overflow (in a debug build), index out of bounds, or
//! pre-allocate an attacker-claimed `n_planes * bytes_per_line *
//! height` pixel buffer that exceeds what the input could possibly
//! back. The return values are intentionally discarded.
//!
//! Four entry points are fuzzed off the same input bytes because they
//! are independent public surfaces with distinct offset / allocation
//! maths:
//!
//!   * [`parse_pcx`] — a single stand-alone PCX file: 128-byte header
//!     (bits_per_pixel / n_planes / window / bytes_per_line all
//!     attacker-controlled) + RLE planar pixel data + optional trailing
//!     VGA palette block located by scanning back from EOF.
//!   * [`parse_dcx`] — the multi-page wrapper: 4-byte magic + u32 LE
//!     offset table that slices the bundle into per-page PCX streams,
//!     each handed to `parse_pcx`. The offset arithmetic (range
//!     computation, monotonicity, bounds) is its own surface.
//!   * [`parse_pcx_indexed_8bpp`] — the typed paletted accessor for
//!     8 bpp × 1 plane PCX. Shares the validation + RLE surface with
//!     `parse_pcx` and additionally strips per-row padding into a
//!     `width × height` index buffer + resolves a 256-entry palette;
//!     fuzzed off the same input bytes so the depth/planes mismatch
//!     reject path, the padding-strip slicing, and the palette source
//!     dispatch are all attacker-driven.
//!   * [`parse_pcx_indexed_4bpp`] — the symmetric typed accessor for
//!     4 bpp × 1 plane (16-colour EGA/VGA packed-bits) PCX. Shares the
//!     same validation + RLE surface as `parse_pcx` and additionally
//!     unpacks nibbles into a `width × height` index buffer + resolves
//!     a 16-entry palette out of the in-header `ega_palette` field (or
//!     the spec table §3.1 hardware default when the field is all-zero);
//!     fuzzed off the same input bytes so the depth/planes mismatch
//!     reject path, the per-row padding strip, the nibble extraction,
//!     and the palette-source dispatch are all attacker-driven.
//!   * [`parse_pcx_indexed_1bpp_4planes`] — the third 16-colour typed
//!     accessor, covering the spec §4.1 EGA bit-plane on-disk layout
//!     where each scanline carries four 1-bit planes whose stacked bits
//!     form a 4-bit palette index per pixel. Shares the validation +
//!     RLE surface with `parse_pcx` and additionally resolves the
//!     stacked index into a `width × height` byte buffer + the same
//!     16-entry palette dispatch (in-header `ega_palette` vs spec
//!     table §3.1 default); fuzzed off the same input bytes so the
//!     depth/planes mismatch reject path, the per-row padding strip,
//!     the four-plane bit stacking, and the palette-source dispatch
//!     are all attacker-driven.
//!
//! Sharing one byte stream across all five entry points keeps the
//! fuzzer's mutator focused: a single byte flip can simultaneously
//! probe the canonical flattener AND each typed accessor's rejection
//! geometry, which is much more efficient than splitting the harness
//! into separate targets.

use libfuzzer_sys::fuzz_target;
use oxideav_pcx::{
    parse_dcx, parse_pcx, parse_pcx_indexed_1bpp_4planes, parse_pcx_indexed_4bpp,
    parse_pcx_indexed_8bpp,
};

fuzz_target!(|data: &[u8]| {
    let _ = parse_pcx(data);
    let _ = parse_dcx(data);
    let _ = parse_pcx_indexed_8bpp(data);
    let _ = parse_pcx_indexed_4bpp(data);
    let _ = parse_pcx_indexed_1bpp_4planes(data);
});
