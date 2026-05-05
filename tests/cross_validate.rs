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

use oxideav_pcx::{encode_pcx_24bpp, parse_pcx};

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
