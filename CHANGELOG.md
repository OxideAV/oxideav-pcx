# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
