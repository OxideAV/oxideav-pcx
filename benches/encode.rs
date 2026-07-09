//! Criterion benchmarks for the PCX encoder hot paths.
//!
//! Round 197 (depth-mode benchmarks): the encoder owns the spec §3.2
//! RLE byte-stream packer (with the singleton-byte ≥ `0xC0` escape),
//! the planar row-build for the 8 bpp × 3 planes 24-bit path, and the
//! packed-bits / multi-plane packers for the 1/2/4 bpp legacy paths,
//! plus the DCX bundle assembler. These benches make each path's cost
//! measurable so a future "Lever N+1" round can A/B-compare against the
//! r197 baseline.
//!
//! Scenarios (all freshly synthesised, no committed fixtures):
//!
//!   - **encode_24bpp_1920x1080**: 1920×1080 24-bit RGB planar encode —
//!     the 1080p baseline.
//!   - **encode_24bpp_320x240**: 320×240 24-bit RGB planar encode —
//!     smaller "thumbnail" baseline.
//!   - **encode_24bpp_640x480**: 640×480 24-bit RGB planar encode —
//!     the VGA case.
//!   - **encode_8bpp_indexed_320x240**: 320×240 8 bpp × 1 plane indexed
//!     encode + 768-byte VGA tail palette.
//!   - **encode_8bpp_grayscale_512x512**: 512×512 8 bpp × 1 plane
//!     grayscale encode — exercises the spec §3 `palette_info = 2` fast
//!     path (no tail palette appended).
//!   - **encode_1bpp_mono_512x512**: 512×512 1 bpp × 1 plane monochrome
//!     encode — exercises the 1-bit MSB-first packer.
//!   - **encode_4bpp_packed_320x240**: 320×240 4 bpp × 1 plane
//!     packed-bits encode — exercises the 2-pixels-per-byte packer with
//!     in-header EGA palette.
//!   - **encode_2bpp_cga_320x240**: 320×240 2 bpp × 1 plane CGA
//!     packed-bits encode — exercises the 4-pixels-per-byte packer and
//!     legacy CGA-palette select via header bytes 16 / 19.
//!   - **encode_1bpp_4planes_ega_320x240**: 320×240 1 bpp × 4 planes
//!     EGA encode — exercises the planar 4-plane bit-shuffle write path.
//!   - **encode_rgb_auto_lowcolor_640x480**: 640×480 16-colour RGB
//!     through `encode_pcx_rgb_auto` — exercises the distinct-colour
//!     scan to saturation plus both candidate encodes and the size
//!     compare (the indexed-branch hot path).
//!   - **encode_rgb_auto_truecolor_640x480**: 640×480 natural-gradient
//!     RGB through `encode_pcx_rgb_auto` — exercises the colour scan's
//!     early bail-out (> 256 colours) plus the single planar encode.
//!   - **encode_dcx_4_pages_320x240**: 4-page 320×240 24-bit DCX bundle
//!     encode — exercises the offset-table writer + magic write on top
//!     of the per-page 24bpp encoder.
//!
//! Run with:
//!     cargo bench -p oxideav-pcx --bench encode

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_pcx::{
    encode_dcx, encode_pcx_1bpp_4planes_ega, encode_pcx_1bpp_mono, encode_pcx_24bpp,
    encode_pcx_2bpp_cga, encode_pcx_4bpp_packed, encode_pcx_8bpp_grayscale,
    encode_pcx_8bpp_indexed, encode_pcx_rgb_auto,
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

/// Packed RGB with only 16 distinct colours, so `encode_pcx_rgb_auto`
/// takes the indexed branch (the colour-scan saturates the palette early
/// and the indexed candidate wins the size compare).
fn build_rgb_low_color(width: u32, height: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    // A fixed 16-entry RGB palette; pixels cycle through it by position +
    // a little noise so runs aren't trivially long.
    let pal: [[u8; 3]; 16] = [
        [0, 0, 0],
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [0, 255, 255],
        [255, 0, 255],
        [255, 255, 255],
        [128, 0, 0],
        [0, 128, 0],
        [0, 0, 128],
        [128, 128, 0],
        [0, 128, 128],
        [128, 0, 128],
        [128, 128, 128],
        [64, 64, 64],
    ];
    let mut data = vec![0u8; w * h * 3];
    let mut state: u32 = 0x0BAD_F00D;
    for r in 0..h {
        for c in 0..w {
            let pick = ((r + c) as u32).wrapping_add(xorshift_byte(&mut state) as u32) % 16;
            let p = pal[pick as usize];
            let idx = (r * w + c) * 3;
            data[idx] = p[0];
            data[idx + 1] = p[1];
            data[idx + 2] = p[2];
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

fn bench_encode_24bpp_1920x1080(c: &mut Criterion) {
    let rgb = build_rgb(1920, 1080);
    let mut g = c.benchmark_group("encode_24bpp_1920x1080");
    g.throughput(Throughput::Bytes((1920u64) * 1080 * 3));
    g.sample_size(10);
    g.bench_function(BenchmarkId::from_parameter("rgb24/1920x1080"), |b| {
        b.iter(|| {
            encode_pcx_24bpp(1920, 1080, criterion::black_box(&rgb)).expect("encode_pcx_24bpp")
        });
    });
    g.finish();
}

fn bench_encode_24bpp_320x240(c: &mut Criterion) {
    let rgb = build_rgb(320, 240);
    let mut g = c.benchmark_group("encode_24bpp_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240 * 3));
    g.bench_function(BenchmarkId::from_parameter("rgb24/320x240"), |b| {
        b.iter(|| {
            encode_pcx_24bpp(320, 240, criterion::black_box(&rgb)).expect("encode_pcx_24bpp")
        });
    });
    g.finish();
}

fn bench_encode_24bpp_640x480(c: &mut Criterion) {
    let rgb = build_rgb(640, 480);
    let mut g = c.benchmark_group("encode_24bpp_640x480");
    g.throughput(Throughput::Bytes((640u64) * 480 * 3));
    g.sample_size(20);
    g.bench_function(BenchmarkId::from_parameter("rgb24/640x480"), |b| {
        b.iter(|| {
            encode_pcx_24bpp(640, 480, criterion::black_box(&rgb)).expect("encode_pcx_24bpp")
        });
    });
    g.finish();
}

fn bench_encode_8bpp_indexed_320x240(c: &mut Criterion) {
    let indices = build_indexed(320, 240);
    let palette = build_palette_256();
    let mut g = c.benchmark_group("encode_8bpp_indexed_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240));
    g.bench_function(BenchmarkId::from_parameter("idx8/320x240"), |b| {
        b.iter(|| {
            encode_pcx_8bpp_indexed(
                320,
                240,
                criterion::black_box(&indices),
                criterion::black_box(&palette),
            )
            .expect("encode_pcx_8bpp_indexed")
        });
    });
    g.finish();
}

fn bench_encode_8bpp_grayscale_512x512(c: &mut Criterion) {
    let pixels = build_grayscale(512, 512);
    let mut g = c.benchmark_group("encode_8bpp_grayscale_512x512");
    g.throughput(Throughput::Bytes((512u64) * 512));
    g.bench_function(BenchmarkId::from_parameter("gray8/512x512"), |b| {
        b.iter(|| {
            encode_pcx_8bpp_grayscale(512, 512, criterion::black_box(&pixels))
                .expect("encode_pcx_8bpp_grayscale")
        });
    });
    g.finish();
}

fn bench_encode_1bpp_mono_512x512(c: &mut Criterion) {
    let pixels = build_mono(512, 512);
    let mut g = c.benchmark_group("encode_1bpp_mono_512x512");
    g.throughput(Throughput::Bytes((512u64) * 512));
    g.bench_function(BenchmarkId::from_parameter("mono/512x512"), |b| {
        b.iter(|| {
            encode_pcx_1bpp_mono(512, 512, criterion::black_box(&pixels))
                .expect("encode_pcx_1bpp_mono")
        });
    });
    g.finish();
}

fn bench_encode_4bpp_packed_320x240(c: &mut Criterion) {
    let indices = build_indexed_16(320, 240);
    let palette = build_palette_16();
    let mut g = c.benchmark_group("encode_4bpp_packed_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240));
    g.bench_function(BenchmarkId::from_parameter("idx4/320x240"), |b| {
        b.iter(|| {
            encode_pcx_4bpp_packed(
                320,
                240,
                criterion::black_box(&indices),
                criterion::black_box(&palette),
            )
            .expect("encode_pcx_4bpp_packed")
        });
    });
    g.finish();
}

fn bench_encode_2bpp_cga_320x240(c: &mut Criterion) {
    let indices = build_indexed_4(320, 240);
    let mut g = c.benchmark_group("encode_2bpp_cga_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240));
    g.bench_function(BenchmarkId::from_parameter("cga/320x240"), |b| {
        b.iter(|| {
            encode_pcx_2bpp_cga(320, 240, criterion::black_box(&indices), 1, 0)
                .expect("encode_pcx_2bpp_cga")
        });
    });
    g.finish();
}

fn bench_encode_1bpp_4planes_ega_320x240(c: &mut Criterion) {
    let indices = build_indexed_16(320, 240);
    let palette = build_palette_16();
    let mut g = c.benchmark_group("encode_1bpp_4planes_ega_320x240");
    g.throughput(Throughput::Bytes((320u64) * 240));
    g.bench_function(BenchmarkId::from_parameter("ega/320x240"), |b| {
        b.iter(|| {
            encode_pcx_1bpp_4planes_ega(
                320,
                240,
                criterion::black_box(&indices),
                criterion::black_box(&palette),
            )
            .expect("encode_pcx_1bpp_4planes_ega")
        });
    });
    g.finish();
}

fn bench_encode_dcx_4_pages_320x240(c: &mut Criterion) {
    let rgb = build_rgb(320, 240);
    let pages: Vec<Vec<u8>> = (0..4)
        .map(|_| encode_pcx_24bpp(320, 240, &rgb).expect("encode_pcx_24bpp"))
        .collect();
    let mut g = c.benchmark_group("encode_dcx_4_pages_320x240");
    // Throughput counts the four pre-encoded PCX payload bytes the DCX
    // assembler concatenates + indexes, not the per-page encode cost.
    let payload_bytes: u64 = pages.iter().map(|p| p.len() as u64).sum();
    g.throughput(Throughput::Bytes(payload_bytes));
    g.bench_function(BenchmarkId::from_parameter("dcx/4x320x240"), |b| {
        b.iter(|| encode_dcx(criterion::black_box(&pages)).expect("encode_dcx"));
    });
    g.finish();
}

fn bench_encode_rgb_auto_lowcolor_640x480(c: &mut Criterion) {
    // 16-colour 640×480 RGB → the auto writer's colour scan saturates the
    // palette and the indexed candidate wins. Times the full scan + both
    // candidate encodes + the size compare (the indexed-branch hot path).
    let rgb = build_rgb_low_color(640, 480);
    let mut g = c.benchmark_group("encode_rgb_auto_lowcolor_640x480");
    g.throughput(Throughput::Bytes((640 * 480 * 3) as u64));
    g.bench_function(BenchmarkId::from_parameter("auto-idx/640x480"), |b| {
        b.iter(|| encode_pcx_rgb_auto(640, 480, criterion::black_box(&rgb)).expect("auto"));
    });
    g.finish();
}

fn bench_encode_rgb_auto_truecolor_640x480(c: &mut Criterion) {
    // Natural-gradient 640×480 RGB → far more than 256 colours, so the
    // scan bails early to the planar branch. Times the colour scan's
    // early-exit cost plus the single planar encode.
    let rgb = build_rgb(640, 480);
    let mut g = c.benchmark_group("encode_rgb_auto_truecolor_640x480");
    g.throughput(Throughput::Bytes((640 * 480 * 3) as u64));
    g.bench_function(BenchmarkId::from_parameter("auto-planar/640x480"), |b| {
        b.iter(|| encode_pcx_rgb_auto(640, 480, criterion::black_box(&rgb)).expect("auto"));
    });
    g.finish();
}

fn bench_encode_rgb_auto_bilevel_640x480(c: &mut Criterion) {
    // Black/white 640×480 text-like content → the r401 ladder's worst
    // case for candidate count: mono, both CGA forms, EGA-RGB, both
    // 4-bit forms, grayscale, indexed and planar ALL apply (nine
    // encodes + the scan) before the byte count picks Mono1. Times the
    // full-ladder overhead a caller pays for the smallest-file
    // guarantee on bilevel input.
    let mut rgb = Vec::with_capacity(640 * 480 * 3);
    for i in 0..640usize * 480 {
        let v = if (i / 3 + i / 640) % 2 == 0 {
            0x00
        } else {
            0xFF
        };
        rgb.extend_from_slice(&[v, v, v]);
    }
    let mut g = c.benchmark_group("encode_rgb_auto_bilevel_640x480");
    g.throughput(Throughput::Bytes((640 * 480 * 3) as u64));
    g.bench_function(BenchmarkId::from_parameter("auto-mono/640x480"), |b| {
        b.iter(|| encode_pcx_rgb_auto(640, 480, criterion::black_box(&rgb)).expect("auto"));
    });
    g.finish();
}

fn bench_encode_rgb_auto_grayscale_640x480(c: &mut Criterion) {
    // Pure-grey gradient (176 levels, all below the RLE escape
    // threshold) → Gray8 wins over Indexed8 by the 769-byte tail. Times
    // the scan + Gray8 + Indexed8 + planar candidate set.
    let mut rgb = Vec::with_capacity(640 * 480 * 3);
    for i in 0..640usize * 480 {
        let g8 = ((i * 3) % 0xB0) as u8;
        rgb.extend_from_slice(&[g8, g8, g8]);
    }
    let mut g = c.benchmark_group("encode_rgb_auto_grayscale_640x480");
    g.throughput(Throughput::Bytes((640 * 480 * 3) as u64));
    g.bench_function(BenchmarkId::from_parameter("auto-gray/640x480"), |b| {
        b.iter(|| encode_pcx_rgb_auto(640, 480, criterion::black_box(&rgb)).expect("auto"));
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_encode_24bpp_1920x1080,
    bench_encode_24bpp_320x240,
    bench_encode_24bpp_640x480,
    bench_encode_8bpp_indexed_320x240,
    bench_encode_8bpp_grayscale_512x512,
    bench_encode_1bpp_mono_512x512,
    bench_encode_4bpp_packed_320x240,
    bench_encode_2bpp_cga_320x240,
    bench_encode_1bpp_4planes_ega_320x240,
    bench_encode_rgb_auto_lowcolor_640x480,
    bench_encode_rgb_auto_truecolor_640x480,
    bench_encode_rgb_auto_bilevel_640x480,
    bench_encode_rgb_auto_grayscale_640x480,
    bench_encode_dcx_4_pages_320x240,
);
criterion_main!(benches);
