//! Criterion benchmarks for the PCX encoder + decoder roundtrip — the
//! realistic "encode an image, decode it back" path that an end-to-end
//! consumer (e.g. a thumbnail muxer or DCX page-bundle scanner)
//! exercises.
//!
//! Round 197 (depth-mode benchmarks): pairs each (depth, planes) tuple's
//! encode path with its decode path so future "Lever N+1" changes can be
//! A/B-compared at the pipeline level (not just one half). Each scenario
//! re-decodes the encoder output, so a perf regression that quietly
//! mis-encodes will show up as a panic rather than a silently-cheaper
//! benchmark number.
//!
//! Scenarios:
//!
//!   - **roundtrip_24bpp_320x240**: 320×240 24-bit RGB planar encode →
//!     decode.
//!   - **roundtrip_24bpp_640x480**: 640×480 24-bit RGB planar encode →
//!     decode.
//!   - **roundtrip_8bpp_indexed_320x240**: 320×240 8 bpp × 1 plane
//!     indexed encode (+ 768-byte VGA tail palette) → decode.
//!   - **roundtrip_8bpp_grayscale_512x512**: 512×512 8 bpp × 1 plane
//!     grayscale encode (`palette_info = 2`) → decode.
//!   - **roundtrip_1bpp_mono_512x512**: 512×512 1 bpp × 1 plane mono
//!     encode → decode.
//!   - **roundtrip_4bpp_packed_320x240**: 320×240 4 bpp × 1 plane
//!     packed-bits encode → decode.
//!   - **roundtrip_2bpp_cga_320x240**: 320×240 2 bpp × 1 plane CGA
//!     packed-bits encode → decode.
//!   - **roundtrip_1bpp_4planes_ega_320x240**: 320×240 1 bpp × 4 planes
//!     EGA encode → decode.
//!   - **roundtrip_dcx_4_pages_320x240**: 4-page 320×240 24-bit DCX
//!     bundle encode → decode.
//!
//! Run with:
//!     cargo bench -p oxideav-pcx --bench roundtrip

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_pcx::{
    encode_dcx, encode_pcx_1bpp_4planes_ega, encode_pcx_1bpp_mono, encode_pcx_24bpp,
    encode_pcx_2bpp_cga, encode_pcx_4bpp_packed, encode_pcx_8bpp_grayscale,
    encode_pcx_8bpp_indexed, parse_dcx, parse_pcx,
};

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
    let mut palette = Vec::with_capacity(48);
    for i in 0..16u16 {
        palette.push((i * 17) as u8);
        palette.push((i ^ 0x5) as u8 * 17);
        palette.push((i ^ 0xA) as u8 * 17);
    }
    palette
}

fn build_indexed_4(width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut data = vec![0u8; w * h];
    let mut state: u32 = 0x6789_abcd;
    for px in data.iter_mut() {
        *px = xorshift_byte(&mut state) & 0x03;
    }
    data
}

fn bench_roundtrip_24bpp_320x240(c: &mut Criterion) {
    let rgb = build_rgb(320, 240);
    let mut g = c.benchmark_group("roundtrip_24bpp_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240 * 4));
    g.bench_function(BenchmarkId::from_parameter("rgb24/320x240"), |b| {
        b.iter(|| {
            let bytes =
                encode_pcx_24bpp(320, 240, criterion::black_box(&rgb)).expect("encode_pcx_24bpp");
            parse_pcx(&bytes).expect("parse_pcx")
        });
    });
    g.finish();
}

fn bench_roundtrip_24bpp_640x480(c: &mut Criterion) {
    let rgb = build_rgb(640, 480);
    let mut g = c.benchmark_group("roundtrip_24bpp_640x480");
    g.throughput(Throughput::Bytes((640u64) * 480 * 4));
    g.sample_size(20);
    g.bench_function(BenchmarkId::from_parameter("rgb24/640x480"), |b| {
        b.iter(|| {
            let bytes =
                encode_pcx_24bpp(640, 480, criterion::black_box(&rgb)).expect("encode_pcx_24bpp");
            parse_pcx(&bytes).expect("parse_pcx")
        });
    });
    g.finish();
}

fn bench_roundtrip_8bpp_indexed_320x240(c: &mut Criterion) {
    let indices = build_indexed(320, 240);
    let palette = build_palette_256();
    let mut g = c.benchmark_group("roundtrip_8bpp_indexed_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240 * 4));
    g.bench_function(BenchmarkId::from_parameter("idx8/320x240"), |b| {
        b.iter(|| {
            let bytes = encode_pcx_8bpp_indexed(
                320,
                240,
                criterion::black_box(&indices),
                criterion::black_box(&palette),
            )
            .expect("encode_pcx_8bpp_indexed");
            parse_pcx(&bytes).expect("parse_pcx")
        });
    });
    g.finish();
}

fn bench_roundtrip_8bpp_grayscale_512x512(c: &mut Criterion) {
    let pixels = build_grayscale(512, 512);
    let mut g = c.benchmark_group("roundtrip_8bpp_grayscale_512x512");
    g.throughput(Throughput::Bytes((512u64) * 512 * 4));
    g.bench_function(BenchmarkId::from_parameter("gray8/512x512"), |b| {
        b.iter(|| {
            let bytes = encode_pcx_8bpp_grayscale(512, 512, criterion::black_box(&pixels))
                .expect("encode_pcx_8bpp_grayscale");
            parse_pcx(&bytes).expect("parse_pcx")
        });
    });
    g.finish();
}

fn bench_roundtrip_1bpp_mono_512x512(c: &mut Criterion) {
    let pixels = build_mono(512, 512);
    let mut g = c.benchmark_group("roundtrip_1bpp_mono_512x512");
    g.throughput(Throughput::Bytes((512u64) * 512 * 4));
    g.bench_function(BenchmarkId::from_parameter("mono/512x512"), |b| {
        b.iter(|| {
            let bytes = encode_pcx_1bpp_mono(512, 512, criterion::black_box(&pixels))
                .expect("encode_pcx_1bpp_mono");
            parse_pcx(&bytes).expect("parse_pcx")
        });
    });
    g.finish();
}

fn bench_roundtrip_4bpp_packed_320x240(c: &mut Criterion) {
    let indices = build_indexed_16(320, 240);
    let palette = build_palette_16();
    let mut g = c.benchmark_group("roundtrip_4bpp_packed_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240 * 4));
    g.bench_function(BenchmarkId::from_parameter("idx4/320x240"), |b| {
        b.iter(|| {
            let bytes = encode_pcx_4bpp_packed(
                320,
                240,
                criterion::black_box(&indices),
                criterion::black_box(&palette),
            )
            .expect("encode_pcx_4bpp_packed");
            parse_pcx(&bytes).expect("parse_pcx")
        });
    });
    g.finish();
}

fn bench_roundtrip_2bpp_cga_320x240(c: &mut Criterion) {
    let indices = build_indexed_4(320, 240);
    let mut g = c.benchmark_group("roundtrip_2bpp_cga_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240 * 4));
    g.bench_function(BenchmarkId::from_parameter("cga/320x240"), |b| {
        b.iter(|| {
            let bytes = encode_pcx_2bpp_cga(320, 240, criterion::black_box(&indices), 1, 0)
                .expect("encode_pcx_2bpp_cga");
            parse_pcx(&bytes).expect("parse_pcx")
        });
    });
    g.finish();
}

fn bench_roundtrip_1bpp_4planes_ega_320x240(c: &mut Criterion) {
    let indices = build_indexed_16(320, 240);
    let palette = build_palette_16();
    let mut g = c.benchmark_group("roundtrip_1bpp_4planes_ega_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240 * 4));
    g.bench_function(BenchmarkId::from_parameter("ega/320x240"), |b| {
        b.iter(|| {
            let bytes = encode_pcx_1bpp_4planes_ega(
                320,
                240,
                criterion::black_box(&indices),
                criterion::black_box(&palette),
            )
            .expect("encode_pcx_1bpp_4planes_ega");
            parse_pcx(&bytes).expect("parse_pcx")
        });
    });
    g.finish();
}

fn bench_roundtrip_dcx_4_pages_320x240(c: &mut Criterion) {
    let rgb = build_rgb(320, 240);
    let pages: Vec<Vec<u8>> = (0..4)
        .map(|_| encode_pcx_24bpp(320, 240, &rgb).expect("encode_pcx_24bpp"))
        .collect();
    let mut g = c.benchmark_group("roundtrip_dcx_4_pages_320x240");
    g.throughput(Throughput::Bytes(4 * 320u64 * 240 * 4));
    g.sample_size(10);
    g.bench_function(BenchmarkId::from_parameter("dcx/4x320x240"), |b| {
        b.iter(|| {
            let bytes = encode_dcx(criterion::black_box(&pages)).expect("encode_dcx");
            parse_dcx(&bytes).expect("parse_dcx")
        });
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_roundtrip_24bpp_320x240,
    bench_roundtrip_24bpp_640x480,
    bench_roundtrip_8bpp_indexed_320x240,
    bench_roundtrip_8bpp_grayscale_512x512,
    bench_roundtrip_1bpp_mono_512x512,
    bench_roundtrip_4bpp_packed_320x240,
    bench_roundtrip_2bpp_cga_320x240,
    bench_roundtrip_1bpp_4planes_ega_320x240,
    bench_roundtrip_dcx_4_pages_320x240,
);
criterion_main!(benches);
