# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.2](https://github.com/OxideAV/oxideav-pcx/compare/v0.0.1...v0.0.2) - 2026-05-05

### Other

- clippy 1.95 — drop identity_op + erasing_op from row-index arithmetic

### Added

- Round 2 decoder: **2 bpp × 1 plane** packed-bits with the legacy
  CGA 4-colour palette (selected via header bytes 16 / 19 per the
  CGA hardware register layout); **4 bpp × 1 plane** packed-bits
  with the in-header EGA palette (or default fallback). 4 pixels
  per byte for 2 bpp; 2 pixels per byte for 4 bpp.
- Round 2 encoder: indexed/EGA/CGA/mono write paths.
  `encode_pcx_1bpp_mono` (1 bpp × 1 plane), `encode_pcx_4bpp_packed`
  (16-colour packed), `encode_pcx_2bpp_cga` (CGA), and
  `encode_pcx_1bpp_4planes_ega` (EGA 4-plane). All write PCX 5.0
  with `bytes_per_line` rounded up to even per spec §1.
- DCX multi-page container: `parse_dcx` + `encode_dcx` handle the
  Microsoft FAX `0x3ADE_68B1`-magic wrapper (up to 1023 PCX pages
  with a u32 LE offset table terminated by a zero sentinel).
- Cross-validation against `magick identify` for every new write
  path (1 bpp mono, 4 bpp packed, 2 bpp CGA, 1 bpp × 4 EGA, 8 bpp
  indexed) plus a magick-decodes-our-4bpp-to-PPM pixel check.
- Round 1: clean-room ZSoft PCX (PC Paintbrush) reader/writer per the
  public **ZSoft PCX File Format Technical Reference Manual**,
  Revision 5 (1991).
- 128-byte fixed header parse: manufacturer (0x0A) / version
  (0/2/3/4/5) / encoding (1 = RLE) / bits-per-pixel (1/2/4/8) /
  window x/y min+max (u16 LE) / DPI / 48-byte EGA palette /
  n_planes (1/3/4) / bytes-per-line / palette_info / screen size /
  54-byte filler.
- RLE byte-stream decoder per spec §3.2: `0xC0..0xFF` is a repeat
  count (low 6 bits = 1..63) followed by one literal byte;
  any other byte is a single literal.
- Planar → packed pixel re-pack: each on-disk scanline is
  `n_planes × bytes_per_line` bytes with planes laid out one after
  another within the row (NOT interleaved per pixel).
- 256-colour VGA palette: appended to the file as a 0x0C marker byte
  + 768 RGB bytes (PCX 3.0+, version ≥ 5) located 769 bytes from EOF.
- Pixel layout coverage:
  - 1 bpp × 1 plane → 1-bit monochrome (expanded to 8-bit grayscale).
  - 1 bpp × 4 planes → 16-colour EGA (palette taken from the header).
  - 8 bpp × 1 plane → 256-colour palette.
  - 8 bpp × 3 planes → 24-bit RGB (one plane per channel).
- Encoder: writes PCX 5.0 — `encode_pcx_8bpp_indexed` (8 bpp × 1
  plane + 256-colour VGA palette) and `encode_pcx_24bpp` (8 bpp × 3
  planes, planar RGB). RLE escapes any literal byte ≥ `0xC0` even
  when its run length is 1.
- Default-on `registry` cargo feature gating the `oxideav-core`
  `Decoder` / `Encoder` / container glue; standalone (no-`registry`)
  build exposes only the framework-free API surface.
