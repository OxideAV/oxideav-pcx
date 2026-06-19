//! Round 345 — RLE codec algebraic property tests.
//!
//! The `oxideav_pcx::rle` module is the heart of the format (spec §3.2,
//! the only encoding ZSoft ever defined: byte 2 of the header = `1`).
//! The higher-level round-trip sweeps in `round345.rs` exercise it
//! indirectly through full encode→decode; this file pins the codec's
//! *algebraic* properties directly against `rle::encode` / `rle::decode`
//! so a regression in the run-length core surfaces at the unit level
//! rather than only as a pixel mismatch three layers up.
//!
//! Properties under test:
//!
//!   1. **Inverse**: `decode(encode(x)) == x` for arbitrary byte
//!      streams, including the `>= 0xC0` high bytes that the spec
//!      requires be escaped into a 1-count packet
//!      (`docs/image/pcx/zsoft-pcx-technical-reference-rev5.md`,
//!      §"Run-length encoding (RLE) method").
//!   2. **High-byte escape**: any singleton literal in `0xC0..=0xFF`
//!      is emitted as a 2-byte packet `(0xC0 | 1, b)`, never as a bare
//!      byte the decoder would mis-read as a run header.
//!   3. **Run cap**: a run longer than 63 identical bytes is split into
//!      multiple ≤63 packets (the 6-bit count field's ceiling).
//!   4. **Truncation rejection**: a stream that ends mid-packet (a run
//!      header with no following literal) or short of `out_len` is
//!      rejected with a typed `Err`, never a panic or partial buffer.
//!   5. **Overrun rejection**: a packet whose count would carry the
//!      output past `out_len` is rejected (the whole-image byte cap).
//!   6. **Count-zero tolerance**: a `0xC0` header (count 0) emits no
//!      bytes and consumes the 2-byte packet, matching the manual's
//!      `encget` (a zero count is illegal to *write* but tolerated on
//!      read).
//!
//! Payloads come from an in-file xorshift so the test is deterministic
//! and dependency-free (clean-room: no external crate).

use oxideav_pcx::rle;

struct Lcg(u64);
impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed | 1)
    }
    fn byte(&mut self) -> u8 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 24) as u8
    }
}

/// Encode then decode `data`, returning the recovered bytes. Asserts the
/// decode consumed exactly the encoded length (no trailing slop).
fn roundtrip(data: &[u8]) -> Vec<u8> {
    let mut enc = Vec::new();
    rle::encode(data, &mut enc);
    let mut dec = Vec::new();
    let consumed = rle::decode(&enc, &mut dec, data.len()).expect("decode ok");
    assert_eq!(
        consumed,
        enc.len(),
        "decode must consume the whole packet stream"
    );
    dec
}

#[test]
fn inverse_on_random_streams() {
    for seed in 0..64u64 {
        let mut rng = Lcg::new(0xC0DE ^ (seed << 8));
        let len = (rng.byte() as usize) * 4 + 1; // 1..=1021
        let data: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
        assert_eq!(roundtrip(&data), data, "seed={seed} len={len}");
    }
}

#[test]
fn inverse_on_all_high_bytes() {
    // Every byte 0xC0..=0xFF in isolation must round-trip — these are the
    // ones the naive encoder would mis-frame as run headers.
    for b in 0xC0u8..=0xFF {
        let data = vec![b];
        assert_eq!(roundtrip(&data), data, "byte {b:#04x}");
        // And a run of them.
        let run = vec![b; 5];
        assert_eq!(roundtrip(&run), run, "run of {b:#04x}");
    }
}

#[test]
fn inverse_on_low_high_alternation() {
    // Alternating low/high bytes maximises packet-boundary churn.
    let data: Vec<u8> = (0..200)
        .map(|i| if i % 2 == 0 { 0x10 } else { 0xC5 })
        .collect();
    assert_eq!(roundtrip(&data), data);
}

#[test]
fn single_high_byte_escaped_as_two_byte_packet() {
    let mut enc = Vec::new();
    rle::encode(&[0xC0], &mut enc);
    // Must be exactly (0xC0 | 1, 0xC0) — a 1-count run, not a bare byte.
    assert_eq!(enc, vec![0xC1, 0xC0]);
}

#[test]
fn low_singleton_emitted_bare() {
    let mut enc = Vec::new();
    rle::encode(&[0x42], &mut enc);
    // A low byte with count 1 needs no escape.
    assert_eq!(enc, vec![0x42]);
}

#[test]
fn run_longer_than_63_splits_into_capped_packets() {
    // 130 identical low bytes → 63 + 63 + 4.
    let data = vec![0x07u8; 130];
    let mut enc = Vec::new();
    rle::encode(&data, &mut enc);
    // Three packets, each (0xC0|count, 0x07).
    assert_eq!(enc.len(), 6, "expected 3 two-byte packets");
    assert_eq!(enc[0], 0xC0 | 63);
    assert_eq!(enc[2], 0xC0 | 63);
    assert_eq!(enc[4], 0xC0 | 4);
    assert_eq!(roundtrip(&data), data);
}

#[test]
fn run_of_exactly_63_is_one_packet() {
    let data = vec![0x33u8; 63];
    let mut enc = Vec::new();
    rle::encode(&data, &mut enc);
    assert_eq!(enc, vec![0xC0 | 63, 0x33]);
    assert_eq!(roundtrip(&data), data);
}

#[test]
fn truncated_mid_packet_is_rejected_not_panicked() {
    // A lone run header with no following literal.
    let enc = vec![0xC5u8];
    let mut dec = Vec::new();
    let r = rle::decode(&enc, &mut dec, 5);
    assert!(r.is_err(), "run header without literal must Err");
}

#[test]
fn stream_shorter_than_out_len_is_rejected() {
    // Two literal bytes but the caller demands five.
    let enc = vec![0x01u8, 0x02];
    let mut dec = Vec::new();
    let r = rle::decode(&enc, &mut dec, 5);
    assert!(r.is_err(), "stream exhausted before out_len must Err");
}

#[test]
fn packet_overrunning_out_len_is_rejected() {
    // A 10-count run but only room for 3 output bytes.
    let enc = vec![0xC0 | 10, 0xAB];
    let mut dec = Vec::new();
    let r = rle::decode(&enc, &mut dec, 3);
    assert!(r.is_err(), "run overrunning out_len must Err");
}

#[test]
fn count_zero_header_emits_nothing_and_consumes_two_bytes() {
    // (0xC0, 0x99) = count 0: per the manual's encget, tolerated on read,
    // emits nothing, consumes 2 bytes. Then a real literal completes the
    // single requested output byte.
    let enc = vec![0xC0, 0x99, 0x42];
    let mut dec = Vec::new();
    let consumed = rle::decode(&enc, &mut dec, 1).expect("decode ok");
    assert_eq!(dec, vec![0x42]);
    assert_eq!(consumed, 3, "the empty packet's 2 bytes are still consumed");
}

#[test]
fn empty_input_with_zero_out_len_is_ok() {
    let mut dec = Vec::new();
    let consumed = rle::decode(&[], &mut dec, 0).expect("zero-length decode ok");
    assert_eq!(consumed, 0);
    assert!(dec.is_empty());
}

#[test]
fn decode_appends_to_existing_buffer() {
    // `decode` documents that it appends from the buffer's current length
    // (the planar reconstruction relies on this). Pre-fill and confirm.
    let mut enc = Vec::new();
    rle::encode(&[0xAA, 0xBB, 0xCC], &mut enc);
    let mut dec = vec![0x01, 0x02];
    let _ = rle::decode(&enc, &mut dec, 3).expect("decode ok");
    assert_eq!(dec, vec![0x01, 0x02, 0xAA, 0xBB, 0xCC]);
}

#[test]
fn encode_never_emits_bare_high_byte() {
    // Property: for any input, the *encoded* stream never contains a bare
    // high-byte literal in a literal position — every byte >= 0xC0 in the
    // output is either a run-header byte (immediately followed by its
    // literal) or a literal that itself follows a header. We verify the
    // structural invariant by walking the packet grammar: each step is
    // either a 1-byte low literal (< 0xC0) or a 2-byte (header, literal)
    // packet.
    for seed in 0..32u64 {
        let mut rng = Lcg::new(0xBEEF ^ (seed << 4));
        let len = (rng.byte() as usize) + 1;
        let data: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
        let mut enc = Vec::new();
        rle::encode(&data, &mut enc);
        let mut i = 0;
        while i < enc.len() {
            if enc[i] & 0xC0 == 0xC0 {
                // Header: must have a following literal byte in range.
                assert!(i + 1 < enc.len(), "header at end of stream seed={seed}");
                i += 2;
            } else {
                i += 1;
            }
        }
        assert_eq!(
            i,
            enc.len(),
            "packet grammar must consume exactly seed={seed}"
        );
    }
}
