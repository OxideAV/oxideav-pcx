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
/// Returns the number of input bytes consumed so the caller can
/// continue reading subsequent scanlines. PCX RLE doesn't cross
/// scanline boundaries in well-formed files, but the decoder doesn't
/// enforce that — it just reads exactly `out_len` output bytes.
pub fn decode(input: &[u8], out: &mut Vec<u8>, out_len: usize) -> Result<usize> {
    let mut produced = 0usize;
    let mut cursor = 0usize;
    while produced < out_len {
        if cursor >= input.len() {
            return Err(Error::invalid(
                "PCX RLE stream truncated (no more packet bytes)",
            ));
        }
        let b = input[cursor];
        cursor += 1;
        if (b & 0xC0) == 0xC0 {
            let count = (b & 0x3F) as usize;
            if cursor >= input.len() {
                return Err(Error::invalid(
                    "PCX RLE stream truncated mid-packet (count without literal)",
                ));
            }
            let lit = input[cursor];
            cursor += 1;
            if produced + count > out_len {
                return Err(Error::invalid(format!(
                    "PCX RLE packet overruns scanline (produced={produced}, packet count={count}, target={out_len})"
                )));
            }
            for _ in 0..count {
                out.push(lit);
            }
            produced += count;
        } else {
            out.push(b);
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
