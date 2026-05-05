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
