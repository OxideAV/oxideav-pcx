# oxideav-pcx

Pure-Rust ZSoft PCX (PC Paintbrush) reader/writer for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework.

Clean-room implementation of the public **ZSoft PCX File Format
Technical Reference Manual**, Revision 5 (1991). No `image` crate's
PCX submodule, GIMP PCX plugin, FreeImage, DevIL, or libpcx source
consulted, paraphrased, or cross-checked.

## Decode

| bits/pixel | n_planes | Source meaning                | Output |
| ---------- | -------- | ----------------------------- | ------ |
| 1          | 1        | Monochrome (1-bit)            | `Rgba` |
| 1          | 4        | 16-colour EGA                 | `Rgba` |
| 8          | 1        | 256-colour palette (VGA tail) | `Rgba` |
| 8          | 3        | 24-bit RGB (planar)           | `Rgba` |

* Per-row layout is planar: each on-disk scanline is `n_planes ×
  bytes_per_line` bytes, with planes laid one after the other within
  the row (NOT interleaved per pixel). The decoder re-packs planes
  into packed RGBA at decode time.
* The 256-colour VGA tail palette (PCX 3.0+, leading `0x0C` marker
  769 bytes from EOF) is honoured for 8 bpp × 1 plane images. When
  absent, an 8-bit grayscale ramp is used as a fallback.
* The 16-entry header EGA palette is used for `1 bpp × 4 planes`
  inputs; if the header field is all zeros (which PCX 3.0+ files
  often emit) the standard hardware EGA palette per spec table §3.1
  is substituted.

## Encode

* `encode_pcx_8bpp_indexed(w, h, &indices, &palette)` — 8 bpp × 1
  plane plus a 768-byte VGA tail palette.
* `encode_pcx_24bpp(w, h, &rgb)` — 8 bpp × 3 planes, planar RGB.

Both write **PCX 5.0** with `bytes_per_line` rounded up to even per
spec §1. The RLE encoder coalesces runs of ≤ 63 identical bytes and
escapes any singleton byte ≥ `0xC0` into a length-1 packet so the
decoder won't mistake it for a run header.

```rust
use oxideav_pcx::{encode_pcx_24bpp, parse_pcx};

let rgb: Vec<u8> = /* 3 × W × H bytes, top-down RGB */ vec![/* … */];
let bytes = encode_pcx_24bpp(width, height, &rgb)?;
let img = parse_pcx(&bytes)?;
assert_eq!(img.width as usize * img.height as usize * 4, img.data.len());
# Ok::<(), oxideav_pcx::PcxError>(())
```

## Standalone vs registry-integrated

The crate's default `registry` Cargo feature pulls in `oxideav-core`
and exposes the framework `Decoder` / `Encoder` trait surface plus a
`registry::register` entry point for the `oxideav` umbrella crate.
Disable the feature for an `oxideav-core`-free build:

```toml
oxideav-pcx = { version = "0.0", default-features = false }
```

The standalone build still exposes `parse_pcx`, `encode_pcx_24bpp`,
`encode_pcx_8bpp_indexed`, plus crate-local `PcxImage` /
`PcxPixelFormat` / `PcxError` types.

## Registration

```rust
let mut codecs = oxideav_core::CodecRegistry::new();
let mut containers = oxideav_core::ContainerRegistry::new();
oxideav_pcx::register(&mut codecs, &mut containers);
```

## Lacks

Round 1 limits — followups for round 2:

* 2 bpp / 4 bpp packed-bits combinations (CGA 4-colour and EGA
  packed). Both are rare in real-world files but called out in spec
  table §4.1.
* CGA 4-colour palette decoding via the `palette_info` byte at offset
  68 + spec table §3.2 (the CGA palette is encoded as an index into
  one of the four canonical hardware palettes rather than carried
  in-line).
* DCX multi-page container variant (Microsoft FAX-style PCX bundle):
  4-byte `0x3ADE_68B1` magic + up to 1023 page-offset u32 LEs,
  followed by individual PCX files concatenated.
* Indexed / EGA write paths from arbitrary input (round 1 only ships
  `encode_pcx_8bpp_indexed` for the 256-colour case).
