//! Criterion benchmarks for the PCX decoder hot paths.
//!
//! Round 197 (depth-mode benchmarks): `oxideav-pcx` is saturated per the
//! workspace README — every spec §4.1 (depth, planes) tuple decodes, the
//! VGA tail palette + spec §3 `palette_info = 2` grayscale flag are
//! honoured, the CGA / EGA legacy palette paths are wired through, the
//! DCX multi-page wrapper round-trips, and the r136 fuzz harness has
//! racked 40M+ executions with zero crashes. Per the workspace
//! "saturated → fuzz/bench/profile" memo, this round adds Criterion
//! benches mirroring the png / bmp / gif / tiff shape so future
//! optimisation rounds can A/B-test changes to the decoder hot paths
//! against a stable r197 baseline.
//!
//! This file covers the **decoder**; sibling files cover `encode` and
//! `roundtrip`.
//!
//! Each scenario synthesises a fresh PCX on the fly via the public
//! encoder API and iterates [`parse_pcx`] / [`parse_dcx`] on the encoded
//! bytes. No fixture files are committed; the bench inputs are entirely
//! reproducible from the bench source.
//!
//!   - **decode_24bpp_1920x1080**: 1920×1080 24-bit RGB planar decode —
//!     the natural-image baseline. Exercises the 8 bpp × 3 planes
//!     planar→packed re-pack on a 1080p surface along with the RLE
//!     byte-stream inner loop.
//!   - **decode_24bpp_320x240**: 320×240 24-bit RGB planar decode — the
//!     smaller "thumbnail" baseline that pairs with the encode bench of
//!     the same dimensions.
//!   - **decode_24bpp_640x480**: 640×480 24-bit RGB planar decode — the
//!     VGA case.
//!   - **decode_8bpp_indexed_320x240**: 320×240 8 bpp × 1 plane indexed
//!     decode — exercises the VGA tail palette lookup + the
//!     scan-back-from-EOF marker probe.
//!   - **decode_8bpp_grayscale_512x512**: 512×512 8 bpp × 1 plane
//!     grayscale decode — exercises the spec §3 `palette_info = 2`
//!     fast path (no tail palette lookup, direct `(g,g,g,0xFF)` blit).
//!   - **decode_1bpp_mono_512x512**: 512×512 1 bpp × 1 plane monochrome
//!     decode — exercises the 1-bit expansion path.
//!   - **decode_4bpp_packed_320x240**: 320×240 4 bpp × 1 plane
//!     packed-bits decode — exercises the 2-pixels-per-byte unpack with
//!     in-header 16-entry palette lookup.
//!   - **decode_2bpp_cga_320x240**: 320×240 2 bpp × 1 plane CGA
//!     packed-bits decode — exercises the 4-pixels-per-byte unpack with
//!     the legacy CGA-palette select via header bytes 16 / 19.
//!   - **decode_1bpp_4planes_ega_320x240**: 320×240 1 bpp × 4 planes
//!     EGA decode — exercises the planar 4-plane bit-shuffle path
//!     (most planar-shuffle work per output pixel of the (depth,planes)
//!     space).
//!   - **decode_dcx_4_pages_320x240**: a 4-page DCX bundle (each page
//!     a 320×240 24 bpp RGB) — exercises the DCX magic check, offset
//!     table walk, per-page slice, and four per-page `parse_pcx`
//!     invocations.
//!
//! Run with:
//!     cargo bench -p oxideav-pcx --bench decode

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_pcx::{
    encode_dcx, encode_pcx_1bpp_4planes_ega, encode_pcx_1bpp_mono, encode_pcx_24bpp,
    encode_pcx_2bpp_cga, encode_pcx_4bpp_packed, encode_pcx_8bpp_grayscale,
    encode_pcx_8bpp_indexed, parse_dcx, parse_pcx,
};

/// Cheap deterministic xorshift32 — keeps bench inputs from being
/// trivially compressible / branch-predictable so the RLE decode side
/// actually exercises the literal + run mix instead of one giant run.
fn xorshift_byte(state: &mut u32) -> u8 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state & 0xff) as u8
}

fn natural_byte(r: usize, c: usize, h: usize, w: usize, state: &mut u32) -> u8 {
    let base_y = ((r * 255) / h.max(1)) as u32;
    let base_x = ((c * 255) / w.max(1)) as u32;
    let v = ((base_x + base_y) / 2).min(255) as u8;
    v.wrapping_add(xorshift_byte(state) & 0x07)
}

fn build_rgb(width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0u8; w * h * 3];
    let mut state: u32 = 0x1234_5678;
    for r in 0..h {
        for c in 0..w {
            let idx = (r * w + c) * 3;
            let base_y = ((r * 255) / h.max(1)) as u32;
            let base_x = ((c * 255) / w.max(1)) as u32;
            data[idx] = natural_byte(r, c, h, w, &mut state);
            data[idx + 1] = base_y.min(255) as u8;
            data[idx + 2] = base_x.min(255) as u8;
        }
    }
    data
}

fn build_indexed(width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0u8; w * h];
    let mut state: u32 = 0x2345_6789;
    for r in 0..h {
        for c in 0..w {
            data[r * w + c] = natural_byte(r, c, h, w, &mut state);
        }
    }
    data
}

fn build_palette_256() -> Vec<u8> {
    let mut palette = Vec::with_capacity(768);
    for i in 0..256u16 {
        palette.push(i as u8);
        palette.push((i ^ 0x55) as u8);
        palette.push((i ^ 0xaa) as u8);
    }
    palette
}

fn build_grayscale(width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0u8; w * h];
    let mut state: u32 = 0x3456_789a;
    for r in 0..h {
        for c in 0..w {
            data[r * w + c] = natural_byte(r, c, h, w, &mut state);
        }
    }
    data
}

fn build_mono(width: u32, height: u32) -> Vec<u8> {
    // One byte per pixel, 0 or 1 — the `encode_pcx_1bpp_mono` writer
    // packs into the on-disk 1-bit stride.
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0u8; w * h];
    let mut state: u32 = 0x4567_89ab;
    for px in data.iter_mut() {
        *px = xorshift_byte(&mut state) & 1;
    }
    data
}

fn build_indexed_16(width: u32, height: u32) -> Vec<u8> {
    // One byte per pixel, low nibble used — the `encode_pcx_4bpp_packed`
    // writer packs 2 pixels into each output byte.
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0u8; w * h];
    let mut state: u32 = 0x5678_9abc;
    for px in data.iter_mut() {
        *px = xorshift_byte(&mut state) & 0x0f;
    }
    data
}

fn build_palette_16() -> Vec<u8> {
    // 48-byte EGA palette (16 RGB triplets).
    let mut palette = Vec::with_capacity(48);
    for i in 0..16u16 {
        palette.push((i * 17) as u8);
        palette.push((i ^ 0x5) as u8 * 17);
        palette.push((i ^ 0xA) as u8 * 17);
    }
    palette
}

fn build_indexed_4(width: u32, height: u32) -> Vec<u8> {
    // One byte per pixel, two bits used — the `encode_pcx_2bpp_cga`
    // writer packs 4 pixels into each output byte.
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0u8; w * h];
    let mut state: u32 = 0x6789_abcd;
    for px in data.iter_mut() {
        *px = xorshift_byte(&mut state) & 0x03;
    }
    data
}

fn bench_decode_24bpp_1920x1080(c: &mut Criterion) {
    let rgb = build_rgb(1920, 1080);
    let bytes = encode_pcx_24bpp(1920, 1080, &rgb).expect("encode_pcx_24bpp");
    let mut g = c.benchmark_group("decode_24bpp_1920x1080");
    g.throughput(Throughput::Bytes((1920u64) * 1080 * 4));
    g.sample_size(10);
    g.bench_function(BenchmarkId::from_parameter("rgb24/1920x1080"), |b| {
        b.iter(|| parse_pcx(criterion::black_box(&bytes)).expect("parse_pcx"));
    });
    g.finish();
}

fn bench_decode_24bpp_320x240(c: &mut Criterion) {
    let rgb = build_rgb(320, 240);
    let bytes = encode_pcx_24bpp(320, 240, &rgb).expect("encode_pcx_24bpp");
    let mut g = c.benchmark_group("decode_24bpp_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240 * 4));
    g.bench_function(BenchmarkId::from_parameter("rgb24/320x240"), |b| {
        b.iter(|| parse_pcx(criterion::black_box(&bytes)).expect("parse_pcx"));
    });
    g.finish();
}

fn bench_decode_24bpp_640x480(c: &mut Criterion) {
    let rgb = build_rgb(640, 480);
    let bytes = encode_pcx_24bpp(640, 480, &rgb).expect("encode_pcx_24bpp");
    let mut g = c.benchmark_group("decode_24bpp_640x480");
    g.throughput(Throughput::Bytes((640u64) * 480 * 4));
    g.sample_size(20);
    g.bench_function(BenchmarkId::from_parameter("rgb24/640x480"), |b| {
        b.iter(|| parse_pcx(criterion::black_box(&bytes)).expect("parse_pcx"));
    });
    g.finish();
}

fn bench_decode_8bpp_indexed_320x240(c: &mut Criterion) {
    let indices = build_indexed(320, 240);
    let palette = build_palette_256();
    let bytes =
        encode_pcx_8bpp_indexed(320, 240, &indices, &palette).expect("encode_pcx_8bpp_indexed");
    let mut g = c.benchmark_group("decode_8bpp_indexed_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240 * 4));
    g.bench_function(BenchmarkId::from_parameter("idx8/320x240"), |b| {
        b.iter(|| parse_pcx(criterion::black_box(&bytes)).expect("parse_pcx"));
    });
    g.finish();
}

fn bench_decode_8bpp_grayscale_512x512(c: &mut Criterion) {
    let pixels = build_grayscale(512, 512);
    let bytes = encode_pcx_8bpp_grayscale(512, 512, &pixels).expect("encode_pcx_8bpp_grayscale");
    let mut g = c.benchmark_group("decode_8bpp_grayscale_512x512");
    g.throughput(Throughput::Bytes((512u64) * 512 * 4));
    g.bench_function(BenchmarkId::from_parameter("gray8/512x512"), |b| {
        b.iter(|| parse_pcx(criterion::black_box(&bytes)).expect("parse_pcx"));
    });
    g.finish();
}

fn bench_decode_1bpp_mono_512x512(c: &mut Criterion) {
    let pixels = build_mono(512, 512);
    let bytes = encode_pcx_1bpp_mono(512, 512, &pixels).expect("encode_pcx_1bpp_mono");
    let mut g = c.benchmark_group("decode_1bpp_mono_512x512");
    g.throughput(Throughput::Bytes((512u64) * 512 * 4));
    g.bench_function(BenchmarkId::from_parameter("mono/512x512"), |b| {
        b.iter(|| parse_pcx(criterion::black_box(&bytes)).expect("parse_pcx"));
    });
    g.finish();
}

fn bench_decode_4bpp_packed_320x240(c: &mut Criterion) {
    let indices = build_indexed_16(320, 240);
    let palette = build_palette_16();
    let bytes =
        encode_pcx_4bpp_packed(320, 240, &indices, &palette).expect("encode_pcx_4bpp_packed");
    let mut g = c.benchmark_group("decode_4bpp_packed_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240 * 4));
    g.bench_function(BenchmarkId::from_parameter("idx4/320x240"), |b| {
        b.iter(|| parse_pcx(criterion::black_box(&bytes)).expect("parse_pcx"));
    });
    g.finish();
}

fn bench_decode_2bpp_cga_320x240(c: &mut Criterion) {
    let indices = build_indexed_4(320, 240);
    // `palette_selector`, `background_index` — pick the cyan/magenta/
    // white palette 1 high-intensity with background colour index 0.
    let bytes = encode_pcx_2bpp_cga(320, 240, &indices, 1, 0).expect("encode_pcx_2bpp_cga");
    let mut g = c.benchmark_group("decode_2bpp_cga_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240 * 4));
    g.bench_function(BenchmarkId::from_parameter("cga/320x240"), |b| {
        b.iter(|| parse_pcx(criterion::black_box(&bytes)).expect("parse_pcx"));
    });
    g.finish();
}

fn bench_decode_1bpp_4planes_ega_320x240(c: &mut Criterion) {
    let indices = build_indexed_16(320, 240);
    let palette = build_palette_16();
    let bytes = encode_pcx_1bpp_4planes_ega(320, 240, &indices, &palette)
        .expect("encode_pcx_1bpp_4planes_ega");
    let mut g = c.benchmark_group("decode_1bpp_4planes_ega_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240 * 4));
    g.bench_function(BenchmarkId::from_parameter("ega/320x240"), |b| {
        b.iter(|| parse_pcx(criterion::black_box(&bytes)).expect("parse_pcx"));
    });
    g.finish();
}

fn bench_decode_dcx_4_pages_320x240(c: &mut Criterion) {
    // Four 320×240 24 bpp PCX pages bundled into a single DCX file.
    // Throughput counts the four pages' RGBA-equivalent output bytes
    // so a future change that, say, batches the per-page `parse_pcx`
    // setup is visible as a throughput improvement.
    let mut pages = Vec::with_capacity(4);
    for _ in 0..4 {
        let rgb = build_rgb(320, 240);
        pages.push(encode_pcx_24bpp(320, 240, &rgb).expect("encode_pcx_24bpp"));
    }
    let bytes = encode_dcx(&pages).expect("encode_dcx");
    let mut g = c.benchmark_group("decode_dcx_4_pages_320x240");
    g.throughput(Throughput::Bytes(4 * 320u64 * 240 * 4));
    g.sample_size(10);
    g.bench_function(BenchmarkId::from_parameter("dcx/4x320x240"), |b| {
        b.iter(|| parse_dcx(criterion::black_box(&bytes)).expect("parse_dcx"));
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_decode_24bpp_1920x1080,
    bench_decode_24bpp_320x240,
    bench_decode_24bpp_640x480,
    bench_decode_8bpp_indexed_320x240,
    bench_decode_8bpp_grayscale_512x512,
    bench_decode_1bpp_mono_512x512,
    bench_decode_4bpp_packed_320x240,
    bench_decode_2bpp_cga_320x240,
    bench_decode_1bpp_4planes_ega_320x240,
    bench_decode_dcx_4_pages_320x240,
);
criterion_main!(benches);
