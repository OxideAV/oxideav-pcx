//! Cross-validation against `magick identify` / `magick convert`.
//!
//! Skipped (returns ok with a message) when ImageMagick isn't on the
//! `PATH`. When it is, we:
//!
//! * Encode an image with our writer + ask `magick identify` to confirm
//!   it parses (read-our-write).
//! * Encode an image with `magick convert` to PCX, then re-decode with
//!   our reader (read-their-write).

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use oxideav_pcx::{
    encode_pcx_1bpp_4planes_ega, encode_pcx_1bpp_mono, encode_pcx_24bpp, encode_pcx_2bpp_cga,
    encode_pcx_4bpp_packed, encode_pcx_8bpp_indexed, parse_pcx,
};

fn have_magick() -> bool {
    Command::new("magick")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tmp(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("oxideav-pcx-cross-{}-{}", std::process::id(), name));
    p
}

fn checker_rgb(w: u16, h: u16) -> Vec<u8> {
    let mut data = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let q = (x & 1) + 2 * (y & 1);
            let p: [u8; 3] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [128, 128, 128]][q];
            data.extend_from_slice(&p);
        }
    }
    data
}

#[test]
fn magick_identifies_our_24bpp_output() {
    if !have_magick() {
        eprintln!("skipping: ImageMagick not on PATH");
        return;
    }
    let rgb = checker_rgb(16, 16);
    let bytes = encode_pcx_24bpp(16, 16, &rgb).unwrap();
    let path = tmp("24bpp.pcx");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    let out = Command::new("magick")
        .arg("identify")
        .arg(&path)
        .output()
        .expect("magick identify");
    assert!(
        out.status.success(),
        "magick identify failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("PCX"), "expected 'PCX' in {s}");
    assert!(s.contains("16x16"), "expected '16x16' in {s}");
    let _ = std::fs::remove_file(&path);
}

fn ega_palette_explicit() -> Vec<u8> {
    let mut p = Vec::with_capacity(48);
    for i in 0..16u8 {
        p.push(i * 17);
        p.push(255 - i * 17);
        p.push(i.wrapping_mul(31));
    }
    p
}

#[test]
fn magick_identifies_our_1bpp_mono_output() {
    if !have_magick() {
        eprintln!("skipping: ImageMagick not on PATH");
        return;
    }
    let pixels: Vec<u8> = (0..(16 * 16)).map(|i| ((i / 16 + i) & 1) as u8).collect();
    let bytes = encode_pcx_1bpp_mono(16, 16, &pixels).unwrap();
    let path = tmp("1bpp.pcx");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    let out = Command::new("magick")
        .arg("identify")
        .arg(&path)
        .output()
        .expect("magick identify");
    assert!(
        out.status.success(),
        "magick identify failed on 1bpp mono: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("PCX"), "expected 'PCX' in {s}");
    assert!(s.contains("16x16"), "expected '16x16' in {s}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn magick_identifies_our_4bpp_packed_output() {
    if !have_magick() {
        eprintln!("skipping: ImageMagick not on PATH");
        return;
    }
    let palette = ega_palette_explicit();
    let indices: Vec<u8> = (0..(16 * 16)).map(|i| (i % 16) as u8).collect();
    let bytes = encode_pcx_4bpp_packed(16, 16, &indices, &palette).unwrap();
    let path = tmp("4bpp.pcx");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    let out = Command::new("magick")
        .arg("identify")
        .arg(&path)
        .output()
        .expect("magick identify");
    assert!(
        out.status.success(),
        "magick identify failed on 4bpp packed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("PCX"), "expected 'PCX' in {s}");
    assert!(s.contains("16x16"), "expected '16x16' in {s}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn magick_identifies_our_2bpp_cga_output() {
    if !have_magick() {
        eprintln!("skipping: ImageMagick not on PATH");
        return;
    }
    let indices: Vec<u8> = (0..(16 * 16)).map(|i| (i % 4) as u8).collect();
    let bytes = encode_pcx_2bpp_cga(16, 16, &indices, 0x00, 0).unwrap();
    let path = tmp("2bpp.pcx");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    let out = Command::new("magick")
        .arg("identify")
        .arg(&path)
        .output()
        .expect("magick identify");
    assert!(
        out.status.success(),
        "magick identify failed on 2bpp CGA: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("PCX"), "expected 'PCX' in {s}");
    assert!(s.contains("16x16"), "expected '16x16' in {s}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn magick_identifies_our_ega_4plane_output() {
    if !have_magick() {
        eprintln!("skipping: ImageMagick not on PATH");
        return;
    }
    let palette = ega_palette_explicit();
    let indices: Vec<u8> = (0..(16 * 16)).map(|i| (i % 16) as u8).collect();
    let bytes = encode_pcx_1bpp_4planes_ega(16, 16, &indices, &palette).unwrap();
    let path = tmp("ega.pcx");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    let out = Command::new("magick")
        .arg("identify")
        .arg(&path)
        .output()
        .expect("magick identify");
    assert!(
        out.status.success(),
        "magick identify failed on EGA 1bppx4: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("PCX"), "expected 'PCX' in {s}");
    assert!(s.contains("16x16"), "expected '16x16' in {s}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn magick_identifies_our_8bpp_indexed_output() {
    if !have_magick() {
        eprintln!("skipping: ImageMagick not on PATH");
        return;
    }
    // 256-entry hue ramp.
    let mut palette = Vec::with_capacity(768);
    for i in 0..256u32 {
        palette.push(i as u8);
        palette.push((255 - i) as u8);
        palette.push(((i * 13) & 0xFF) as u8);
    }
    let indices: Vec<u8> = (0..(16 * 16)).map(|i| (i % 256) as u8).collect();
    let bytes = encode_pcx_8bpp_indexed(16, 16, &indices, &palette).unwrap();
    let path = tmp("8bpp.pcx");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    let out = Command::new("magick")
        .arg("identify")
        .arg(&path)
        .output()
        .expect("magick identify");
    assert!(
        out.status.success(),
        "magick identify failed on 8bpp indexed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("PCX"), "expected 'PCX' in {s}");
    assert!(s.contains("16x16"), "expected '16x16' in {s}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn magick_re_decodes_our_4bpp_to_ppm_with_correct_pixels() {
    // Encode a 4 bpp packed PCX with a unique palette, ask magick to
    // convert it to PPM, then check the first row of pixels matches
    // what our palette indexes resolve to. Catches palette-parse bugs
    // in our writer.
    if !have_magick() {
        eprintln!("skipping: ImageMagick not on PATH");
        return;
    }
    let palette = ega_palette_explicit();
    // 4x1 image with index 1, 2, 3, 4.
    let indices: Vec<u8> = vec![1, 2, 3, 4];
    let bytes = encode_pcx_4bpp_packed(4, 1, &indices, &palette).unwrap();
    let pcx_path = tmp("4bpp_to_ppm.pcx");
    std::fs::File::create(&pcx_path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    let ppm_path = tmp("4bpp_to_ppm.ppm");
    let status = Command::new("magick")
        .arg(&pcx_path)
        .arg(ppm_path.to_str().unwrap())
        .status()
        .expect("magick convert");
    assert!(status.success(), "magick convert failed");
    let ppm = std::fs::read(&ppm_path).unwrap();
    // PPM header is `P6` + 3 whitespace-separated tokens (width, height,
    // maxval) + exactly one whitespace byte + raw pixel bytes. Magick
    // writes maxval = 15 for a 4 bpp source even though the palette
    // entries we wrote are full 8-bit values, so we have to read maxval
    // and undo the per-channel `(value * maxval + 127) / 255` rescale
    // when comparing against the palette.
    assert!(ppm.starts_with(b"P6"), "not a binary PPM");
    let mut cursor = 2usize;
    let mut tokens: Vec<&[u8]> = Vec::with_capacity(3);
    while tokens.len() < 3 {
        while cursor < ppm.len() && ppm[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        while cursor < ppm.len() && ppm[cursor] == b'#' {
            while cursor < ppm.len() && ppm[cursor] != b'\n' {
                cursor += 1;
            }
        }
        let start = cursor;
        while cursor < ppm.len() && !ppm[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        tokens.push(&ppm[start..cursor]);
    }
    let maxval: u32 = std::str::from_utf8(tokens[2]).unwrap().parse().unwrap();
    assert!(cursor < ppm.len() && ppm[cursor].is_ascii_whitespace());
    let body_start = cursor + 1;
    let pixels = &ppm[body_start..body_start + 12];
    // Round-trip rescale our reference palette through `maxval` so the
    // comparison matches Magick's quantisation.
    let q = |v: u8| -> u8 { ((v as u32 * maxval + 127) / 255) as u8 };
    for (i, &idx) in indices.iter().enumerate() {
        let off = idx as usize * 3;
        let want = [q(palette[off]), q(palette[off + 1]), q(palette[off + 2])];
        assert_eq!(
            &pixels[i * 3..i * 3 + 3],
            &want,
            "magick-decoded pixel {i} (palette idx {idx}); maxval = {maxval}"
        );
    }
    let _ = std::fs::remove_file(&pcx_path);
    let _ = std::fs::remove_file(&ppm_path);
}

#[test]
fn we_decode_magick_authored_pcx() {
    if !have_magick() {
        eprintln!("skipping: ImageMagick not on PATH");
        return;
    }
    // magick uses `xc:red` to synthesise an 8x8 solid red canvas and
    // writes it as PCX.
    let path = tmp("magick.pcx");
    let status = Command::new("magick")
        .arg("-size")
        .arg("8x8")
        .arg("xc:red")
        .arg(path.to_str().unwrap())
        .status()
        .expect("magick convert");
    assert!(status.success());
    let bytes = std::fs::read(&path).unwrap();
    let img = parse_pcx(&bytes).unwrap_or_else(|e| {
        panic!(
            "parse_pcx failed on magick-authored PCX: {e}; bytes={}",
            bytes.len()
        )
    });
    assert_eq!(img.width, 8);
    assert_eq!(img.height, 8);
    // First pixel should be pure red (alpha may be 0xFF).
    assert_eq!(&img.data[0..3], &[255, 0, 0]);
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// r401 — auto-ladder outputs cross-validated pixel-exactly through magick
// ---------------------------------------------------------------------------

/// Ask magick to decode `pcx_bytes` to raw 8-bit packed RGB and return
/// the bytes. `-depth 8` pins the output scale so palette entries and
/// grey levels come back exactly as written.
fn magick_to_raw_rgb(name: &str, pcx_bytes: &[u8], w: u16, h: u16) -> Vec<u8> {
    let pcx_path = tmp(&format!("{name}.pcx"));
    let raw_path = tmp(&format!("{name}.raw"));
    std::fs::File::create(&pcx_path)
        .unwrap()
        .write_all(pcx_bytes)
        .unwrap();
    let status = Command::new("magick")
        .arg(pcx_path.to_str().unwrap())
        .arg("-depth")
        .arg("8")
        .arg(format!("rgb:{}", raw_path.to_str().unwrap()))
        .status()
        .expect("magick convert to raw rgb");
    assert!(status.success(), "magick convert failed for {name}");
    let raw = std::fs::read(&raw_path).unwrap();
    assert_eq!(
        raw.len(),
        w as usize * h as usize * 3,
        "unexpected raw size for {name}"
    );
    let _ = std::fs::remove_file(&pcx_path);
    let _ = std::fs::remove_file(&raw_path);
    raw
}

/// Every geometry the r401 auto ladder can emit that ImageMagick reads
/// per the manual — Indexed4, Indexed1x4, Indexed8, Rgb24 — must decode
/// through it to the exact source pixels. Three geometry families are
/// deliberately excluded from the pixel-exact check and covered by the
/// structural identify checks above instead:
///
/// * the CGA pair — the manual's "CGA Color Map" (header bytes 16 /
///   19) is a palette *selector*, and ImageMagick instead reads the
///   colormap's leading triples directly, so its CGA pixel output
///   diverges from the spec by construction;
/// * Mono1 — ImageMagick hard-codes the opposite 1-bpp polarity
///   (bit 1 = black) and ignores the two-entry colormap our writer
///   stores; since r405 the reference doc's errata (Issue #227) pins
///   the conformant reading — bit = colormap index, bit 1 = white —
///   so the divergence is on ImageMagick's side. The bit *geometry*
///   is still cross-checked below in
///   [`magick_confirms_mono_bit_geometry_modulo_polarity`];
/// * Gray8 — ImageMagick unconditionally demands the appended VGA tail
///   on 8 bpp × 1 plane files and errors on the spec's
///   `palette_info = 2` tail-less form (the EGFF cross-reference notes
///   most programs ignore that flag); callers who need such readers to
///   consume their grayscale output can use `encode_pcx_8bpp_indexed`
///   with a ramp palette — the Indexed8 rung is exactly that file.
#[test]
fn magick_re_decodes_every_unambiguous_ladder_geometry_exactly() {
    if !have_magick() {
        eprintln!("skipping: ImageMagick not on PATH");
        return;
    }
    use oxideav_pcx::{encode_pcx_rgb_auto, PcxAutoMode};
    let (w, h) = (64u16, 10u16);
    let n = w as usize * h as usize;

    // (name, pixel generator) — one flavour per unambiguous rung.
    let mut cases: Vec<(&str, Vec<u8>, PcxAutoMode)> = Vec::new();
    // Indexed4 (noise over 16 non-grey colours).
    let mut st = 0x1DE4u32;
    let mut xs = || {
        st ^= st << 13;
        st ^= st >> 17;
        st ^= st << 5;
        st
    };
    let idx4: Vec<u8> = (0..n)
        .flat_map(|_| {
            let k = (xs() % 16) as u8;
            [13 + k * 11, 29 + k * 7, 47 + k * 5]
        })
        .collect();
    cases.push(("indexed4", idx4, PcxAutoMode::Indexed4 { colors: 16 }));
    // Indexed1x4 (plane-periodic stripes, 3 colours).
    let stripes: Vec<u8> = (0..n)
        .flat_map(|i| {
            let pal: [[u8; 3]; 3] = [[10, 20, 30], [200, 30, 30], [30, 200, 30]];
            pal[[0usize, 1, 0, 2][i % 4]]
        })
        .collect();
    cases.push(("indexed1x4", stripes, PcxAutoMode::Indexed1x4 { colors: 3 }));
    // Indexed8 (20 colours: too many for the 4-bit rungs).
    let idx8: Vec<u8> = (0..n)
        .flat_map(|i| {
            let k = (i % 20) as u8;
            [15 + k * 8, 40 + k * 6, 70 + k * 4]
        })
        .collect();
    cases.push(("indexed8", idx8, PcxAutoMode::Indexed8 { colors: 20 }));
    // Rgb24 (every pixel a distinct colour: 240 ≤ 256, so force the
    // planar branch with > 256 distinct colours on a wider canvas is
    // overkill — instead use a 300-colour 30×10 canvas).
    let (w2, h2) = (30u16, 10u16);
    let rgb24: Vec<u8> = (0..(w2 as usize * h2 as usize))
        .flat_map(|i| [(i & 0xFF) as u8, (i >> 8) as u8, 0x33])
        .collect();

    for (name, rgb, want_mode) in cases {
        let (bytes, mode) = encode_pcx_rgb_auto(w, h, &rgb).unwrap();
        assert_eq!(mode, want_mode, "{name}: unexpected ladder mode");
        let raw = magick_to_raw_rgb(name, &bytes, w, h);
        assert_eq!(raw, rgb, "{name}: magick pixels differ from source");
    }
    let (bytes, mode) = encode_pcx_rgb_auto(w2, h2, &rgb24).unwrap();
    assert_eq!(mode, PcxAutoMode::Rgb24);
    let raw = magick_to_raw_rgb("rgb24", &bytes, w2, h2);
    assert_eq!(raw, rgb24, "rgb24: magick pixels differ from source");
}

// ---------------------------------------------------------------------------
// r405 — mono bit geometry cross-validated modulo the known polarity split
// ---------------------------------------------------------------------------

/// Black-box confirmation of the 1 bpp writer's *bit geometry* —
/// MSB-first packing, the mid-byte row cutoff, and the even
/// `bytes_per_line` pad — while staying robust to the one point where
/// ImageMagick and the reference doc's errata (Issue #227) disagree:
/// the errata pins bit = colormap index (so bit 1 = white via the
/// stored black/white colormap), whereas ImageMagick hard-codes
/// bit 1 = black and ignores the colormap (measured on ImageMagick
/// 7.1.2). If the two readings agree pixel-for-pixel up to ONE global
/// complement, every bit landed in the right position; anything else
/// (a shifted bit, a pad bit bleeding into the image, a per-row flip)
/// fails both arms.
#[test]
fn magick_confirms_mono_bit_geometry_modulo_polarity() {
    if !have_magick() {
        eprintln!("skipping: ImageMagick not on PATH");
        return;
    }
    // Width 13 forces a mid-byte row end AND an even-padding byte;
    // the per-row phase shift makes every bit position load-bearing.
    let (w, h) = (13u16, 5u16);
    let n = w as usize * h as usize;
    let pixels: Vec<u8> = (0..n)
        .map(|i| {
            let (x, y) = (i % w as usize, i / w as usize);
            ((x * (y + 1) + y) % 3 == 0) as u8
        })
        .collect();
    let bytes = encode_pcx_1bpp_mono(w, h, &pixels).unwrap();
    let raw = magick_to_raw_rgb("mono-geometry", &bytes, w, h);
    // Classify magick's readback as bilevel 0/1 per pixel.
    let theirs: Vec<u8> = raw
        .chunks_exact(3)
        .map(|c| match c {
            [0xFF, 0xFF, 0xFF] => 1u8,
            [0x00, 0x00, 0x00] => 0u8,
            other => panic!("non-bilevel readback pixel {other:?}"),
        })
        .collect();
    let complement: Vec<u8> = pixels.iter().map(|&p| 1 - p).collect();
    assert!(
        theirs == pixels || theirs == complement,
        "bit geometry mismatch: readback is neither the source nor its global complement"
    );
    // Pin today's measured behaviour so a future ImageMagick that
    // starts honouring the colormap (flipping to the errata's
    // conformant reading) is noticed here rather than silently
    // changing what this test proves.
    assert_eq!(
        theirs, complement,
        "ImageMagick now decodes 1 bpp in the errata polarity — update this canary \
         and consider promoting Mono1 into the pixel-exact ladder test above"
    );
}

// ---------------------------------------------------------------------------
// r417 — caller-palette (Pal8 side-channel) ladder outputs cross-validated
// ---------------------------------------------------------------------------

/// The VGA-tail rung of `encode_pcx_indexed_auto` (a caller table of
/// more than 16 entries) must be readable by an independent black-box
/// decoder with the caller's palette applied verbatim: every readback
/// pixel is the exact entry its index selects.
#[test]
fn magick_re_decodes_caller_palette_vga_tail_exactly() {
    if !have_magick() {
        eprintln!("skipping: ImageMagick not on PATH");
        return;
    }
    use oxideav_pcx::{encode_pcx_indexed_auto, PcxAutoMode};
    let (w, h) = (23u16, 6u16);
    let n = w as usize * h as usize;
    // 20 entries (> 16 forces the VGA tail), values chosen with no
    // arithmetic relation to the index so a palette re-derivation or
    // off-by-one would show up in the readback.
    let pal: Vec<u8> = (0..20u8)
        .flat_map(|i| [i * 12 + 5, 250 - i * 9, i.wrapping_mul(29) ^ 0x11])
        .collect();
    let idx: Vec<u8> = (0..n).map(|i| ((i * 3 + 1) % 20) as u8).collect();
    let (bytes, mode) = encode_pcx_indexed_auto(w, h, &idx, &pal).unwrap();
    assert_eq!(mode, PcxAutoMode::Indexed8 { colors: 20 });
    let raw = magick_to_raw_rgb("r417-tail", &bytes, w, h);
    for (i, px) in raw.chunks_exact(3).enumerate() {
        let e = idx[i] as usize * 3;
        assert_eq!(
            px,
            &pal[e..e + 3],
            "pixel {i} (index {}) differs from the caller entry",
            idx[i]
        );
    }
}

/// The header-colormap rung of `encode_pcx_indexed_auto` (≤ 16 caller
/// entries) under the same black-box contract: the independent decoder
/// must resolve every pixel through the caller's 48-byte header table.
#[test]
fn magick_re_decodes_caller_palette_header_rung_exactly() {
    if !have_magick() {
        eprintln!("skipping: ImageMagick not on PATH");
        return;
    }
    use oxideav_pcx::{encode_pcx_indexed_auto, PcxAutoMode};
    let (w, h) = (17u16, 9u16);
    let n = w as usize * h as usize;
    let pal: Vec<u8> = (0..11u8)
        .flat_map(|i| [7 + i * 19, 240 - i * 13, i.wrapping_mul(41) ^ 0x2C])
        .collect();
    let idx: Vec<u8> = (0..n).map(|i| ((i * 5 + 2) % 11) as u8).collect();
    let (bytes, mode) = encode_pcx_indexed_auto(w, h, &idx, &pal).unwrap();
    assert!(
        matches!(
            mode,
            PcxAutoMode::Indexed4 { colors: 11 } | PcxAutoMode::Indexed1x4 { colors: 11 }
        ),
        "11-entry table must ride a header rung, got {mode:?}"
    );
    let raw = magick_to_raw_rgb("r417-header", &bytes, w, h);
    for (i, px) in raw.chunks_exact(3).enumerate() {
        let e = idx[i] as usize * 3;
        assert_eq!(
            px,
            &pal[e..e + 3],
            "pixel {i} (index {}) differs from the caller entry",
            idx[i]
        );
    }
}
