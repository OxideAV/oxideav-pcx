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
//! Two entry points are fuzzed off the same input bytes because they
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

use libfuzzer_sys::fuzz_target;
use oxideav_pcx::{parse_dcx, parse_pcx};

fuzz_target!(|data: &[u8]| {
    let _ = parse_pcx(data);
    let _ = parse_dcx(data);
});
