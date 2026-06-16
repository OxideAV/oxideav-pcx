//! PCX run-length codec, per spec §3.2 (encoding byte 1 is the only
//! one ZSoft ever defined).
//!
//! Decoder rule: read one byte `b`. If the top two bits are both set
//! (`b & 0xC0 == 0xC0`), the low six bits are a repeat count `1..=63`
//! (a count of zero is illegal but tolerated as an empty packet).
//! Read one more byte and emit `b & 0x3F` copies of it. Otherwise `b`
//! is itself a single literal byte.
//!
//! Encoder rule: a literal byte whose top two bits are both set
//! (`>= 0xC0`) MUST be emitted as a length-1 RLE packet `(0xC1, b)`
//! since a bare `b` would otherwise be misinterpreted as a count
//! header. Runs of identical bytes are emitted as RLE packets up to
//! length 63.

use crate::error::{PcxError as Error, Result};

/// Decode `out_len` bytes from a PCX RLE byte stream.
///
/// Returns the number of input bytes consumed. The caller drives this
/// with the *whole-image* byte total (`bytes_per_line × n_planes ×
/// height`), matching the manual's own decode fragment
/// (`docs/image/pcx/pcx-pcgpe.txt` lines 316-326), which consumes the
/// stream straight through `BytesPerLine * Nplanes * (1 + Ymax - Ymin)`
/// bytes with no per-scanline RLE reset. The spec's "decoding break at
/// the end of each scan line" is an encoder convention, not a decode
/// requirement, so a run packet that straddles a row boundary in the
/// input decodes correctly here — `out_len` only caps the total output,
/// not per-row spans.
pub fn decode(input: &[u8], out: &mut Vec<u8>, out_len: usize) -> Result<usize> {
    // `produced` is tracked as a delta from the buffer's starting length
    // so the run-fill fast path below can grow `out` in bulk via
    // `Vec::resize` (the allocator's `memset`) rather than one `push` per
    // emitted byte. `decode` is called once for the whole image against a
    // caller-pre-`reserve`d planar `Vec`, so a bulk grow that stays
    // within the reservation never reallocates.
    let base = out.len();
    let mut produced = 0usize;
    let mut cursor = 0usize;
    let n = input.len();
    while produced < out_len {
        if cursor >= n {
            return Err(Error::invalid(
                "PCX RLE stream truncated (no more packet bytes)",
            ));
        }
        let b = input[cursor];
        if (b & 0xC0) == 0xC0 {
            // Run packet: header byte + one literal, emitted `count`
            // times. `resize` hands the run length to the allocator's
            // memset fast path rather than `push`-ing one byte at a
            // time through a bounds + capacity check per copy.
            let count = (b & 0x3F) as usize;
            if cursor + 1 >= n {
                return Err(Error::invalid(
                    "PCX RLE stream truncated mid-packet (count without literal)",
                ));
            }
            let lit = input[cursor + 1];
            cursor += 2;
            if produced + count > out_len {
                return Err(Error::invalid(format!(
                    "PCX RLE packet overruns image buffer (produced={produced}, packet count={count}, target={out_len})"
                )));
            }
            // Short runs (the common case on high-entropy bit-packed
            // planar layouts) `push` cheaper than they `resize` — the
            // resize bookkeeping doesn't amortise below a few bytes. Long
            // runs (smooth 24-bit channels) take the `resize` memset fast
            // path. The threshold is empirical against the decode bench.
            if count <= 2 {
                for _ in 0..count {
                    out.push(lit);
                }
            } else {
                out.resize(base + produced + count, lit);
            }
            produced += count;
        } else {
            // Literal byte: emit it directly. (A maximal-span
            // `extend_from_slice` copy was measured slower here — the
            // per-span scan loop costs more than it saves on the
            // singleton-heavy literal streams the bit-packed low-bpp
            // planar layouts produce.)
            out.push(b);
            cursor += 1;
            produced += 1;
        }
    }
    Ok(cursor)
}

/// Encode `input` into a PCX RLE byte stream, appending to `out`.
///
/// Runs of identical bytes are coalesced into RLE packets of up to 63
/// bytes each. Singleton bytes whose top two bits are both set
/// (`>= 0xC0`) are escaped into a length-1 RLE packet `(0xC1, b)` so
/// the decoder won't mistake them for run headers.
pub fn encode(input: &[u8], out: &mut Vec<u8>) {
    let mut i = 0usize;
    let n = input.len();
    while i < n {
        let b = input[i];
        let mut run = 1usize;
        while i + run < n && input[i + run] == b && run < 63 {
            run += 1;
        }
        if run >= 2 || (b & 0xC0) == 0xC0 {
            out.push(0xC0 | (run as u8));
            out.push(b);
        } else {
            out.push(b);
        }
        i += run;
    }
}
