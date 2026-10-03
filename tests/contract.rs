//! IMAGE_CRATE_API conformance: root vocabulary, native layouts per
//! geometry, exact conversions, limits / strictness, lossless round
//! trips through every geometry, option fields, DCX `decode_all`, error
//! shape, and the deprecated wrappers' relation to the contract paths.
//! Framework-free: this file runs under `--no-default-features` too.

use oxideav_pcx::{
    decode, decode_all, decode_from, decode_rgb8, decode_rgba8, decode_with, encode, encode_all,
    encode_dcx, encode_rgb8, encode_rgba8, encode_to, header, info, probe, ColorInfo, ColorRange,
    DecodeOptions, EncodeOptions, Error, Frame, Metadata, Palette, PcxError, PcxImage, PcxLayout,
    PcxPixelFormat, PixelFormat, Plane,
};

fn seeded(len: usize, mut seed: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        out.push((seed >> 24) as u8);
    }
    out
}

const W: u32 = 37;
const H: u32 = 11;
const N: usize = (W * H) as usize;

fn rgb() -> PcxImage {
    PcxImage::from_rgb8(W, H, seeded(N * 3, 1)).unwrap()
}

fn gray() -> PcxImage {
    PcxImage::from_gray8(W, H, seeded(N, 2)).unwrap()
}

fn pal(entries: &[[u8; 3]], seed: u32) -> PcxImage {
    let idx: Vec<u8> = seeded(N, seed)
        .into_iter()
        .map(|b| (b as usize % entries.len()) as u8)
        .collect();
    PcxImage::new_indexed(W, H, idx, Palette::from_rgb_triples(entries)).unwrap()
}

fn primaries() -> Vec<[u8; 3]> {
    (0..8u8)
        .map(|i| {
            [
                if i & 1 != 0 { 0xFF } else { 0 },
                if i & 2 != 0 { 0xFF } else { 0 },
                if i & 4 != 0 { 0xFF } else { 0 },
            ]
        })
        .collect()
}

fn cga() -> Vec<[u8; 3]> {
    // Palette 1 bright over background 1 (blue).
    vec![
        [0x00, 0x00, 0xAA],
        [0x55, 0xFF, 0xFF],
        [0xFF, 0x55, 0xFF],
        [0xFF, 0xFF, 0xFF],
    ]
}

fn sixteen() -> Vec<[u8; 3]> {
    seeded(48, 3)
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect()
}

fn two_hundred() -> Vec<[u8; 3]> {
    seeded(600, 4)
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect()
}

fn full256() -> Vec<[u8; 3]> {
    (0..=255u8).map(|i| [i, !i, i ^ 0x55]).collect()
}

// ---- probe / info / header ---------------------------------------------------

#[test]
fn probe_is_total_and_discriminating() {
    assert!(!probe(&[]));
    assert!(!probe(&[0x0A; 127]));
    assert!(!probe(&[0u8; 4096]));
    let f = encode(&rgb(), &EncodeOptions::default()).unwrap();
    assert!(probe(&f));
    let mut bad = f.clone();
    bad[0] = 0x0B;
    assert!(!probe(&bad));
    let mut bad = f.clone();
    bad[2] = 0; // encoding byte
    assert!(!probe(&bad));
    let bundle = encode_dcx(&[f.clone(), f]).unwrap();
    assert!(probe(&bundle));
}

#[test]
fn info_describes_every_geometry_without_decoding() {
    let cases: Vec<(PcxImage, PcxLayout, PixelFormat, usize)> = vec![
        (rgb(), PcxLayout::Rgb24, PixelFormat::Rgb24, 0),
        (gray(), PcxLayout::Gray8, PixelFormat::Gray8, 0),
        (
            pal(&[[0, 0, 0], [0xFF, 0xFF, 0xFF]], 5),
            PcxLayout::Mono1,
            PixelFormat::Pal8,
            2,
        ),
        (pal(&cga(), 6), PcxLayout::Cga2x1, PixelFormat::Pal8, 4),
        (
            pal(&primaries(), 7),
            PcxLayout::EgaRgb1x3,
            PixelFormat::Pal8,
            8,
        ),
        (
            pal(&sixteen(), 8),
            PcxLayout::Indexed4,
            PixelFormat::Pal8,
            16,
        ),
        (
            pal(&two_hundred(), 9),
            PcxLayout::Indexed8,
            PixelFormat::Pal8,
            256,
        ),
    ];
    for (img, layout, format, pal_len) in cases {
        let f = encode(&img, &EncodeOptions::default()).unwrap();
        let i = info(&f).unwrap();
        assert_eq!((i.width, i.height, i.frames), (W, H, 1), "{layout:?}");
        assert_eq!(i.layout, layout);
        assert_eq!(i.format, format);
        assert_eq!(layout.palette_len(), pal_len);
        assert!(!i.has_alpha && !i.has_icc && !i.has_exif && !i.has_xmp);
        assert_eq!(i.color, ColorInfo::pcx_default());
        assert_eq!(i.version, 5);
        assert_eq!((i.bits_per_pixel, i.n_planes), layout.depth_planes());
        assert_eq!(i.bytes_per_line % 2, 0);
        assert_eq!(
            i.palette_info,
            if layout == PcxLayout::Gray8 { 2 } else { 1 }
        );
        // Gray8 carries the grey-ramp tail by default (`gray_tail`).
        assert_eq!(
            i.has_vga_palette,
            matches!(layout, PcxLayout::Indexed8 | PcxLayout::Gray8)
        );
        assert_eq!(i.dpi, None);
        // A truncated file still describes itself: `info` reads the
        // header only.
        let i2 = info(&f[..128]).unwrap();
        assert_eq!(i2.width, W);
        let h = header(&f).unwrap();
        assert_eq!(h.width(), W);
        assert_eq!((h.bits_per_pixel, h.n_planes), layout.depth_planes());
    }
}

#[test]
fn info_reports_header_extras_and_dcx_pages() {
    let opts = EncodeOptions::default()
        .with_dpi((300, 150))
        .with_window_origin((17, 9))
        .with_screen_size((640, 480))
        .with_version(4);
    let f = encode(&rgb(), &opts).unwrap();
    let i = info(&f).unwrap();
    assert_eq!(i.dpi, Some((300, 150)));
    assert_eq!(i.window_origin, Some((17, 9)));
    assert_eq!(i.screen_size, Some((640, 480)));
    assert_eq!(i.version, 4);
    let bundle = encode_dcx(&[f.clone(), f.clone(), f]).unwrap();
    let i = info(&bundle).unwrap();
    assert_eq!(i.frames, 3);
    assert_eq!(i.width, W);
    assert!(matches!(
        info(&[0x0A, 9, 1, 8]),
        Err(PcxError::InvalidData(_))
    ));
}

// ---- decode: native layouts and exact conversions -----------------------------

#[test]
fn decode_returns_native_layouts_with_palettes() {
    let img = pal(&sixteen(), 10);
    let f = encode(&img, &EncodeOptions::default()).unwrap();
    let d = decode(&f).unwrap();
    assert_eq!(d.format, PixelFormat::Pal8);
    assert_eq!(d.layout, Some(PcxLayout::Indexed4));
    assert_eq!(d.planes.len(), 1);
    assert_eq!(d.stride(), W as usize);
    assert_eq!(d.as_bytes(), img.as_bytes());
    assert_eq!(d.palette, img.palette);
    assert_eq!(d.color, ColorInfo::pcx_default());
    assert_eq!(d.color.range, ColorRange::Full);
    assert_eq!(d.metadata, Metadata::default());
    assert_eq!(d.to_rgb8(), img.to_rgb8());
    assert_eq!(d.to_rgba8(), img.to_rgba8());
    assert_eq!(decode_rgb8(&f).unwrap().data, img.to_rgb8());
    assert_eq!(decode_rgba8(&f).unwrap().data, img.to_rgba8());
    assert_eq!(decode_from(&f[..]).unwrap(), d);
    assert_eq!(d.clone().into_raw(), img.as_bytes().unwrap());

    let g = gray();
    let d = decode(&encode(&g, &EncodeOptions::default()).unwrap()).unwrap();
    assert_eq!(d.format, PixelFormat::Gray8);
    assert_eq!(d.palette, None);
    assert_eq!(d.to_rgba8(), g.to_rgba8());
    let want: Vec<u8> = g.data().iter().flat_map(|&v| [v, v, v]).collect();
    assert_eq!(d.to_rgb8(), want);

    let r = rgb();
    let d = decode(&encode(&r, &EncodeOptions::default()).unwrap()).unwrap();
    assert_eq!(d.format, PixelFormat::Rgb24);
    assert_eq!(d.stride(), 3 * W as usize);
    assert_eq!(d.to_rgb8(), r.data());
    let want: Vec<u8> = r
        .data()
        .chunks_exact(3)
        .flat_map(|c| [c[0], c[1], c[2], 255])
        .collect();
    assert_eq!(d.to_rgba8(), want);
}

#[test]
fn palette_expansion_matches_each_geometry() {
    let bw = [[0, 0, 0], [0xFF, 0xFF, 0xFF]];
    for (entries, layout) in [
        (bw.to_vec(), PcxLayout::Mono1),
        (cga(), PcxLayout::Cga2x1),
        (cga(), PcxLayout::Cga1x2),
        (primaries(), PcxLayout::EgaRgb1x3),
        (sixteen(), PcxLayout::Indexed4),
        (sixteen(), PcxLayout::Indexed1x4),
        (two_hundred(), PcxLayout::Indexed8),
    ] {
        let img = pal(&entries, 11);
        let f = encode(&img, &EncodeOptions::default().with_layout(layout)).unwrap();
        let d = decode(&f).unwrap();
        assert_eq!(d.layout, Some(layout));
        assert_eq!(d.as_bytes(), img.as_bytes(), "{layout:?} indices");
        let pal = d.palette.as_ref().unwrap();
        assert_eq!(pal.len(), layout.palette_len(), "{layout:?} palette length");
        let want = Palette::from_rgb_triples(&entries);
        assert_eq!(
            &pal.entries[..entries.len()],
            &want.entries[..],
            "{layout:?} entries"
        );
        assert!(pal.entries[entries.len()..]
            .iter()
            .all(|e| *e == [0, 0, 0, 255]));
        assert_eq!(d.to_rgba8(), img.to_rgba8(), "{layout:?} expansion");
    }
}

#[test]
fn zero_filled_colormaps_fall_back_to_the_documented_defaults() {
    // 16-colour file with an all-zero colormap: EGA hardware default.
    let img = pal(&sixteen(), 12);
    let mut f = encode(&img, &EncodeOptions::default()).unwrap();
    f[16..64].fill(0);
    let d = decode(&f).unwrap();
    let p = d.palette.unwrap();
    assert_eq!(p.entries[0], [0, 0, 0, 255]);
    assert_eq!(p.entries[1], [0, 0, 0xAA, 255]);
    assert_eq!(p.entries[15], [0xFF, 0xFF, 0xFF, 255]);
    // Mono file with an all-zero colormap: black / white.
    let img = pal(&[[0, 0, 0], [0xFF, 0xFF, 0xFF]], 13);
    let mut f = encode(&img, &EncodeOptions::default()).unwrap();
    f[16..64].fill(0);
    let d = decode(&f).unwrap();
    assert_eq!(d.palette, img.palette);
}

#[test]
fn eight_bit_without_tail_or_with_gray_flag_is_gray8() {
    let img = pal(&two_hundred(), 14);
    let f = encode(&img, &EncodeOptions::default()).unwrap();
    assert_eq!(info(&f).unwrap().layout, PcxLayout::Indexed8);
    // Strip the 769-byte tail: no palette → grey levels.
    let no_tail = &f[..f.len() - 769];
    assert_eq!(info(no_tail).unwrap().layout, PcxLayout::Gray8);
    let d = decode(no_tail).unwrap();
    assert_eq!(d.format, PixelFormat::Gray8);
    assert_eq!(d.as_bytes(), img.as_bytes());
    // The grayscale flag wins over a present tail.
    let mut flagged = f.clone();
    flagged[68] = 2;
    assert_eq!(decode(&flagged).unwrap().format, PixelFormat::Gray8);
}

// ---- limits / strict -------------------------------------------------------------

#[test]
fn limits_fail_closed_before_allocation() {
    let f = encode(&rgb(), &EncodeOptions::default()).unwrap();
    let limit = |o: DecodeOptions| matches!(decode_with(&f, &o), Err(Error::LimitExceeded(_)));
    assert!(limit(DecodeOptions::default().with_max_width(W - 1)));
    assert!(limit(DecodeOptions::default().with_max_height(H - 1)));
    assert!(limit(
        DecodeOptions::default().with_max_pixels(u64::from(W * H) - 1)
    ));
    assert!(limit(
        DecodeOptions::default().with_max_bytes(u64::from(W * H * 3) - 1)
    ));
    assert!(decode_with(&f, &DecodeOptions::default().unlimited()).is_ok());
    // A hostile header claiming a huge canvas is refused by the bomb
    // cap / limits, not allocated.
    let mut huge = f[..128].to_vec();
    huge[8..10].copy_from_slice(&0xFFFDu16.to_le_bytes());
    huge[10..12].copy_from_slice(&0xFFFDu16.to_le_bytes());
    huge[66..68].copy_from_slice(&0xFFFEu16.to_le_bytes());
    huge.extend_from_slice(&[0xC1, 0]);
    assert!(decode(&huge).is_err());
    assert!(matches!(
        decode_with(&huge, &DecodeOptions::default().with_max_pixels(1000u64)),
        Err(Error::LimitExceeded(_))
    ));
}

#[test]
fn strict_mode_enforces_the_should_rules() {
    let img = gray();
    let f = encode(&img, &EncodeOptions::default()).unwrap();
    let strict = DecodeOptions::default().with_strict(true);
    assert_eq!(decode_with(&f, &strict).unwrap(), decode(&f).unwrap());
    // Odd bytes_per_line: lenient accepts, strict rejects.
    let mut odd = f.clone();
    odd[66..68].copy_from_slice(&(W as u16).to_le_bytes());
    // Re-encode the rows at the odd width so the stream stays
    // consistent: one literal byte per pixel (values < 0xC0), no runs.
    let mut body = vec![0u8; 0];
    for row in img.data().chunks_exact(W as usize) {
        for &v in row {
            if v >= 0xC0 {
                body.extend_from_slice(&[0xC1, v]);
            } else {
                body.push(v);
            }
        }
    }
    odd.truncate(128);
    odd.extend_from_slice(&body);
    assert!(decode(&odd).is_ok());
    assert!(matches!(
        decode_with(&odd, &strict),
        Err(Error::InvalidData(_))
    ));
    // Non-zero reserved byte.
    let mut reserved = f.clone();
    reserved[64] = 7;
    assert!(decode(&reserved).is_ok());
    assert!(matches!(
        decode_with(&reserved, &strict),
        Err(Error::InvalidData(_))
    ));
    // A run straddling a scanline boundary: 4 × 2 grey image whose
    // eight bytes are one run of 8 → lenient decodes, strict rejects.
    let small = PcxImage::from_gray8(4, 2, vec![9; 8]).unwrap();
    let mut s = encode(&small, &EncodeOptions::default()).unwrap();
    s.truncate(128);
    s.extend_from_slice(&[0xC8, 9]);
    assert_eq!(decode(&s).unwrap().data(), &[9u8; 8][..]);
    assert!(matches!(
        decode_with(&s, &strict),
        Err(Error::InvalidData(_))
    ));
}

// ---- encode: round trips and options --------------------------------------------

#[test]
fn lossless_round_trip_every_layout() {
    let imgs = vec![
        rgb(),
        gray(),
        pal(&[[0, 0, 0], [0xFF, 0xFF, 0xFF]], 20),
        pal(&cga(), 21),
        pal(&primaries(), 22),
        pal(&sixteen(), 23),
        pal(&two_hundred(), 24),
        pal(&full256(), 25),
    ];
    for img in imgs {
        let f = encode(&img, &EncodeOptions::default()).unwrap();
        let d = decode(&f).unwrap();
        // Everything but `layout` (None on a caller-built image,
        // Some on a decoded one) is equal; a second trip is identical.
        let mut d_cmp = d.clone();
        d_cmp.layout = None;
        if img.palette.as_ref().is_some_and(|p| p.len() == 200) {
            // 200 entries ride the 256-entry VGA tail: the palette reads
            // back zero-padded to the geometry's length.
            d_cmp.palette = img.palette.clone();
        }
        assert_eq!(d_cmp, img, "{:?}", d.layout);
        let f2 = encode(&d, &EncodeOptions::default()).unwrap();
        assert_eq!(f, f2, "{:?} byte-identical re-encode", d.layout);
        assert_eq!(decode(&f2).unwrap(), d);
    }
}

#[test]
fn forced_layouts_and_compact_mode() {
    let img = pal(&cga(), 30);
    for layout in [
        PcxLayout::Cga2x1,
        PcxLayout::Cga1x2,
        PcxLayout::Indexed4,
        PcxLayout::Indexed1x4,
        PcxLayout::Indexed8,
        PcxLayout::Rgb24,
    ] {
        let f = encode(&img, &EncodeOptions::default().with_layout(layout)).unwrap();
        let d = decode(&f).unwrap();
        assert_eq!(d.layout, Some(layout));
        assert_eq!(d.to_rgba8(), img.to_rgba8(), "{layout:?}");
    }
    // Unfit geometries are refused, never quantised.
    for layout in [PcxLayout::Mono1, PcxLayout::EgaRgb1x3, PcxLayout::Gray8] {
        assert!(matches!(
            encode(&img, &EncodeOptions::default().with_layout(layout)),
            Err(Error::Unsupported(_))
        ));
    }
    // Compact: the RGB expansion of a 4-colour CGA image lands on a CGA
    // rung (2 bits per pixel beats everything else), losslessly.
    let expanded = PcxImage::from_rgb8(W, H, img.to_rgb8()).unwrap();
    let f = encode(&expanded, &EncodeOptions::default().with_compact(true)).unwrap();
    let d = decode(&f).unwrap();
    assert!(matches!(
        d.layout,
        Some(PcxLayout::Cga2x1 | PcxLayout::Cga1x2)
    ));
    assert_eq!(d.to_rgb8(), img.to_rgb8());
    // Compact on a > 256-colour image is the 24-bit form.
    let f = encode(&rgb(), &EncodeOptions::default().with_compact(true)).unwrap();
    assert_eq!(info(&f).unwrap().layout, PcxLayout::Rgb24);
    // Compact on a black / white RGB image is monochrome.
    let bw = pal(&[[0, 0, 0], [0xFF, 0xFF, 0xFF]], 31);
    let f = encode(
        &PcxImage::from_rgb8(W, H, bw.to_rgb8()).unwrap(),
        &EncodeOptions::default().with_compact(true),
    )
    .unwrap();
    assert_eq!(info(&f).unwrap().layout, PcxLayout::Mono1);
    assert_eq!(decode(&f).unwrap().to_rgb8(), bw.to_rgb8());
}

#[test]
fn header_options_round_trip_and_validate() {
    let img = rgb()
        .with_dpi((300, 300))
        .with_window_origin((5, 6))
        .with_screen_size((800, 600));
    let f = encode(&img, &EncodeOptions::default()).unwrap();
    let d = decode(&f).unwrap();
    assert_eq!(d.dpi, Some((300, 300)));
    assert_eq!(d.window_origin, Some((5, 6)));
    assert_eq!(d.screen_size, Some((800, 600)));
    // Options override the image.
    let f = encode(
        &img,
        &EncodeOptions::default()
            .with_dpi((72, 72))
            .with_window_origin((0, 0))
            .with_screen_size((320, 200)),
    )
    .unwrap();
    let d = decode(&f).unwrap();
    assert_eq!(d.dpi, Some((72, 72)));
    assert_eq!(d.window_origin, None);
    assert_eq!(d.screen_size, Some((320, 200)));
    // No DPI anywhere → the header words are 0 (unset) and read back None.
    let f = encode(&rgb(), &EncodeOptions::default()).unwrap();
    assert_eq!(&f[12..16], &[0, 0, 0, 0]);
    assert_eq!(decode(&f).unwrap().dpi, None);
    // Sentinel-breaking values and bad versions are refused.
    assert!(matches!(
        encode(&rgb(), &EncodeOptions::default().with_dpi((0, 300))),
        Err(Error::InvalidData(_))
    ));
    assert!(matches!(
        encode(&rgb(), &EncodeOptions::default().with_screen_size((640, 0))),
        Err(Error::InvalidData(_))
    ));
    assert!(matches!(
        encode(&rgb(), &EncodeOptions::default().with_version(1)),
        Err(Error::InvalidData(_))
    ));
    assert!(matches!(
        encode(
            &rgb(),
            &EncodeOptions::default().with_window_origin((65_530, 0))
        ),
        Err(Error::InvalidData(_))
    ));
}

#[test]
fn alpha_is_refused_unless_dropped() {
    let rgba = PcxImage::from_rgba8(W, H, seeded(N * 4, 40)).unwrap();
    assert!(matches!(
        encode(&rgba, &EncodeOptions::default()),
        Err(Error::Unsupported(_))
    ));
    let f = encode(&rgba, &EncodeOptions::default().with_drop_alpha(true)).unwrap();
    assert_eq!(decode(&f).unwrap().to_rgb8(), rgba.to_rgb8());
    // encode_rgba8 documents the drop.
    let f2 = encode_rgba8(W, H, rgba.data(), &EncodeOptions::default()).unwrap();
    assert_eq!(f, f2);
    // encode_rgb8 == encode(from_rgb8).
    let r = rgb();
    assert_eq!(
        encode_rgb8(W, H, r.data(), &EncodeOptions::default()).unwrap(),
        encode(&r, &EncodeOptions::default()).unwrap()
    );
    // A translucent palette entry needs drop_alpha too.
    let mut p = pal(&sixteen(), 41);
    p.palette.as_mut().unwrap().entries[3][3] = 7;
    assert!(matches!(
        encode(&p, &EncodeOptions::default()),
        Err(Error::Unsupported(_))
    ));
    assert!(encode(&p, &EncodeOptions::default().with_drop_alpha(true)).is_ok());
    // encode_to writes the same bytes.
    let mut buf = Vec::new();
    encode_to(&r, &EncodeOptions::default(), &mut buf).unwrap();
    assert_eq!(buf, encode(&r, &EncodeOptions::default()).unwrap());
}

#[test]
fn constructors_validate_geometry() {
    assert!(matches!(
        PcxImage::from_rgb8(2, 2, vec![0; 11]),
        Err(Error::InvalidData(_))
    ));
    assert!(PcxImage::from_rgb8(2, 2, vec![0; 12]).is_ok());
    assert!(PcxImage::new(2, 2, PixelFormat::Gray8, vec![]).is_err());
    assert!(PcxImage::new(2, 2, PixelFormat::Gray8, vec![Plane::new(1, vec![0; 4])]).is_err());
    assert!(PcxImage::new(2, 2, PixelFormat::Gray8, vec![Plane::new(3, vec![0; 5])]).is_ok());
    assert!(matches!(
        PcxImage::new_indexed(2, 1, vec![0, 2], Palette::from_rgb(&[0, 0, 0, 1, 1, 1])),
        Err(Error::InvalidData(_))
    ));
    // A Pal8 image without a palette cannot be encoded; to_rgba8 stays
    // total (transparent black) and try_to_rgba8 reports it.
    let no_pal = PcxImage::packed(2, 1, PixelFormat::Pal8, vec![0, 1]).unwrap();
    assert!(matches!(
        encode(&no_pal, &EncodeOptions::default()),
        Err(Error::InvalidData(_))
    ));
    assert_eq!(no_pal.to_rgba8(), vec![0; 8]);
    assert!(no_pal.try_to_rgba8().is_err());
    // Padded strides are honoured by the conversions and the encoder.
    let padded = PcxImage::new(
        1,
        2,
        PixelFormat::Rgb24,
        vec![Plane::new(4, vec![1, 2, 3, 99, 4, 5, 6, 99])],
    )
    .unwrap();
    assert_eq!(padded.to_rgb8(), vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(
        decode(&encode(&padded, &EncodeOptions::default()).unwrap())
            .unwrap()
            .to_rgb8(),
        vec![1, 2, 3, 4, 5, 6]
    );
    // Oversized dimensions are Unsupported.
    let wide = PcxImage::new(
        70_000,
        1,
        PixelFormat::Gray8,
        vec![Plane::new(70_000, vec![0; 70_000])],
    )
    .unwrap();
    assert!(matches!(
        encode(&wide, &EncodeOptions::default()),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn window_origin_reaching_x_max_65535_round_trips() {
    // x_min = 1 with width 65535 puts x_max at exactly 65535: the header
    // arithmetic must not overflow (found by the decode fuzz target's
    // encode → decode round trip; the seed is
    // `fuzz/corpus/decode_pcx/r467-window-origin-xmax-65535`).
    let mut hdr = vec![0u8; 128];
    hdr[0] = 0x0A;
    hdr[1] = 5;
    hdr[2] = 1;
    hdr[3] = 1;
    hdr[4..6].copy_from_slice(&1u16.to_le_bytes());
    hdr[8..10].copy_from_slice(&0xFFFFu16.to_le_bytes());
    hdr[65] = 1;
    hdr[66..68].copy_from_slice(&8192u16.to_le_bytes());
    hdr[68..70].copy_from_slice(&1u16.to_le_bytes());
    let mut rem = 8192u32;
    while rem > 0 {
        let n = rem.min(63);
        hdr.extend_from_slice(&[0xC0 | n as u8, 0]);
        rem -= n;
    }
    let img = decode(&hdr).unwrap();
    assert_eq!((img.width, img.height), (65535, 1));
    assert_eq!(img.window_origin, Some((1, 0)));
    let f = encode(&img, &EncodeOptions::default()).unwrap();
    assert_eq!(&f[8..10], &0xFFFFu16.to_le_bytes());
    assert_eq!(decode(&f).unwrap(), img);
}

// ---- DCX -------------------------------------------------------------------------

#[test]
fn decode_all_walks_dcx_pages() {
    let pages = [rgb(), gray(), pal(&sixteen(), 50)];
    let files: Vec<Vec<u8>> = pages
        .iter()
        .map(|p| encode(p, &EncodeOptions::default()).unwrap())
        .collect();
    let bundle = encode_dcx(&files).unwrap();
    let frames = decode_all(&bundle).unwrap();
    assert_eq!(frames.len(), 3);
    for (i, fr) in frames.iter().enumerate() {
        assert_eq!(fr.index, i as u32);
        assert_eq!(fr.delay, None);
        assert_eq!(fr.image.to_rgba8(), pages[i].to_rgba8());
    }
    assert_eq!(decode(&bundle).unwrap(), frames[0].image);
    // A plain PCX is one frame.
    let one = decode_all(&files[1]).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].image.format, PixelFormat::Gray8);
}

#[test]
fn encode_all_mirrors_decode_all_losslessly() {
    let pages = [rgb(), gray(), pal(&sixteen(), 50), pal(&cga(), 51)];
    let frames: Vec<Frame> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| Frame::new(p.clone(), i as u32))
        .collect();
    let bundle = encode_all(&frames, &EncodeOptions::default()).unwrap();
    assert!(probe(&bundle));
    assert_eq!(info(&bundle).unwrap().frames, 4);
    let back = decode_all(&bundle).unwrap();
    assert_eq!(back.len(), frames.len());
    for (b, f) in back.iter().zip(&frames) {
        // `layout` is None on a caller-built image, Some on a decoded
        // one; everything else round-trips exactly.
        let mut img = b.image.clone();
        img.layout = None;
        assert_eq!(img, f.image);
        assert_eq!(b.index, f.index);
        assert_eq!(b.delay, None);
    }
    // Same bytes as the depth alias over per-page `encode` output.
    let files: Vec<Vec<u8>> = pages
        .iter()
        .map(|p| encode(p, &EncodeOptions::default()).unwrap())
        .collect();
    assert_eq!(bundle, encode_dcx(&files).unwrap());
    // Options apply to every page.
    let dpi = encode_all(&frames, &EncodeOptions::default().with_dpi((300, 300))).unwrap();
    for fr in decode_all(&dpi).unwrap() {
        assert_eq!(fr.image.dpi, Some((300, 300)));
    }
    // One frame is still a (one-page) bundle; none is an error.
    let one = encode_all(&frames[..1], &EncodeOptions::default()).unwrap();
    assert_eq!(decode_all(&one).unwrap().len(), 1);
    assert!(matches!(
        encode_all(&[], &EncodeOptions::default()),
        Err(Error::InvalidData(_))
    ));
    // A page the format cannot carry fails the whole bundle.
    let rgba = Frame::new(PcxImage::from_rgba8(W, H, seeded(N * 4, 52)).unwrap(), 0);
    assert!(matches!(
        encode_all(&[rgba], &EncodeOptions::default()),
        Err(Error::Unsupported(_))
    ));
}

// ---- Error shape -----------------------------------------------------------------

#[test]
fn error_shape() {
    struct Failing;
    impl std::io::Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("nope"))
        }
    }
    struct Sink;
    impl std::io::Write for Sink {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(matches!(decode_from(Failing), Err(Error::Io(_))));
    assert!(matches!(
        encode_to(&rgb(), &EncodeOptions::default(), Sink),
        Err(Error::Io(_))
    ));
    let e: Error = std::io::Error::other("x").into();
    assert!(std::error::Error::source(&e).is_some());
    assert!(matches!(decode(&[0u8; 200]), Err(Error::InvalidData(_))));
    let mut f = encode(&rgb(), &EncodeOptions::default()).unwrap();
    f[2] = 0;
    assert!(matches!(decode(&f), Err(Error::Unsupported(_))));
    assert_eq!(std::mem::size_of::<PcxPixelFormat>(), 1);
    assert_eq!(PcxPixelFormat::Pal8.bytes_per_pixel(), 1);
}

// ---- Deprecated wrappers vs the contract paths ----------------------------------

#[test]
#[allow(deprecated)]
fn deprecated_writers_match_the_contract_paths() {
    let r = rgb();
    let legacy = EncodeOptions::default().with_dpi((72, 72));
    assert_eq!(
        oxideav_pcx::encode_pcx_24bpp(W as u16, H as u16, r.data()).unwrap(),
        encode(&r, &legacy).unwrap()
    );
    let g = gray();
    assert_eq!(
        oxideav_pcx::encode_pcx_8bpp_grayscale(W as u16, H as u16, g.data()).unwrap(),
        encode(&g, &legacy.clone().with_gray_tail(false)).unwrap()
    );
    // With the tail the file is 769 bytes longer and decodes identically.
    let tailed = encode(&g, &legacy).unwrap();
    assert_eq!(
        tailed.len(),
        oxideav_pcx::encode_pcx_8bpp_grayscale(W as u16, H as u16, g.data())
            .unwrap()
            .len()
            + 769
    );
    assert_eq!(decode(&tailed).unwrap().data(), g.data());
    let p = pal(&full256(), 60);
    let pal_rgb = p.palette.as_ref().unwrap().to_rgb();
    assert_eq!(
        oxideav_pcx::encode_pcx_8bpp_indexed(W as u16, H as u16, p.data(), &pal_rgb).unwrap(),
        encode(&p, &legacy).unwrap()
    );
    let p16 = pal(&sixteen(), 61);
    let pal48 = p16.palette.as_ref().unwrap().to_rgb();
    assert_eq!(
        oxideav_pcx::encode_pcx_4bpp_packed(W as u16, H as u16, p16.data(), &pal48).unwrap(),
        encode(&p16, &legacy).unwrap()
    );
    assert_eq!(
        oxideav_pcx::encode_pcx_1bpp_4planes_ega(W as u16, H as u16, p16.data(), &pal48).unwrap(),
        encode(&p16, &legacy.clone().with_layout(PcxLayout::Indexed1x4)).unwrap()
    );
    let bw = pal(&[[0, 0, 0], [0xFF, 0xFF, 0xFF]], 62);
    assert_eq!(
        oxideav_pcx::encode_pcx_1bpp_mono(W as u16, H as u16, bw.data()).unwrap(),
        encode(&bw, &legacy).unwrap()
    );
    // The compact ladder: same choice, same bytes.
    let (auto, _mode) =
        oxideav_pcx::encode_pcx_rgb_auto(W as u16, H as u16, &bw.to_rgb8()).unwrap();
    assert_eq!(
        auto,
        encode(
            &PcxImage::from_rgb8(W, H, bw.to_rgb8()).unwrap(),
            &legacy.clone().with_compact(true)
        )
        .unwrap()
    );
    // parse_pcx is decode + the legacy Rgba expansion.
    let f = encode(&p16, &legacy).unwrap();
    let old = oxideav_pcx::parse_pcx(&f).unwrap();
    assert_eq!(old.format, PixelFormat::Rgba);
    assert_eq!(old.data(), decode(&f).unwrap().to_rgba8());
    assert_eq!(old, decode(&f).unwrap().into_legacy_layout());
    assert_eq!(old.dpi, Some((72, 72)));
    // register_runtime / parse_header keep working.
    assert_eq!(
        oxideav_pcx::parse_header(&f).map(|h| h.width()),
        Some(header(&f).unwrap().width())
    );
}
