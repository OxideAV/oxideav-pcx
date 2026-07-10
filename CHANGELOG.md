# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- *(tests)* **1 bpp monochrome bit polarity conformance-verified
  against the reference doc's new errata (Issue #227) and pinned at
  the raw-byte level** (`tests/round405_mono_polarity.rs`). The
  errata resolves the polarity the ZSoft manual never states: the bit
  value is used directly as the colormap index, with `colormap[0]` =
  black / `colormap[1]` = white as the standard monochrome
  convention — bit 1 = white. The crate's decoder (colormap-indexed
  resolution with the classic bit 1 = white fallback for zero-filled
  colormaps), the `encode_pcx_1bpp_mono` writer, the `Mono1`
  auto-ladder rung (including the first-seen-palette-order trap and
  the non-black bilevel fall-through to CGA), and the framework
  `MonoBlack` / `MonoWhite` rungs were all verified conformant — no
  inversion found, no behaviour change. A new black-box
  cross-validation test confirms the writer's MSB-first bit geometry
  modulo one global polarity complement and documents that a
  mainstream reader hard-codes the opposite (non-conformant) polarity
  and ignores the mono colormap (see the README interop caveat).
- *(fuzz)* **Four encode-fuzz surfaces upgraded from no-panic checks
  to semantic round-trip oracles**: the mono writer now asserts the
  errata polarity (non-zero input byte → white, zero → black) on
  every fuzz-chosen geometry, the grayscale writer asserts
  `g → (g, g, g)`, and both CGA writers (packed 2 bpp and
  plane-oriented 1 bpp × 2) assert `index & 0x03` plus the background
  nibble round-trip through their typed accessors. Five named
  polarity seeds added (inverted two-entry colormap, zero colormap,
  foreign odd-`bytes_per_line` shape with a redundant VGA tail,
  bilevel and CGA-index encode payloads); both corpora re-minimised
  after the run. ~2.4M encode + ~33M decode iterations with the new
  oracles live: zero findings.

### Fixed

- *(decode/encode)* **CGA palette bytes were read/written 16 positions
  too deep, with an inverted two-bit selector convention.** The
  manual's "CGA Color Map" places the background nibble in *header*
  byte 16 and the C / P / I selector in *header* byte 19 — the
  colormap's bytes 0 and 3, as the EGFF cross-reference's extraction
  code (`EgaPalette[0]`, `EgaPalette[3]`) confirms. The crate read and
  wrote colormap bytes 16 / 19 (header bytes 32 / 35) instead, and the
  legacy resolver decoded only bits 7 / 6 with palette-select /
  intensity meanings that do not match the manual's C (color burst) /
  P (palette family) / I (intensity) bits. Round-trips were
  self-consistent so no in-crate test could see it, but every foreign
  spec-conforming CGA file decoded with the default palette, and
  spec-conforming readers saw ours the same way. Both the offset and
  the bit convention are fixed across all CGA paths: the canonical
  `parse_pcx` flatten (both `2×1` and `1×2` layouts) now resolves the
  full C / P / I decomposition — including the composite-monochrome
  ramps — through the same resolver as the CPI accessors, the legacy
  writers emit the manual's byte encoding, `palette_selector` values
  are now genuine byte-19 values (`0x60` white-bright, `0x40`
  white-dim, `0x20` yellow-bright, `0x00` yellow-dim, `0x80` / `0xA0`
  monochrome), and `Pcx2bppCgaPaletteSource` gains `MonochromeDim` /
  `MonochromeBright`. A foreign-layout fixture test pins the on-disk
  offsets directly so a paired encoder+decoder slip can never hide
  again; the auto ladder's CGA match search gains the two monochrome
  ramps (grey quads like `0x00/0x55/0xAA/0xFF` now fit in 2 bpp).

### Added

- *(decode/encode)* **Monochrome files are now self-describing
  2-colour paletted images.** The EGFF cross-reference's canonical
  mode matrix treats `1 bpp × 1 plane` as the 2-colour case of the
  header colormap, so `encode_pcx_1bpp_mono` (and its DPI variant) now
  writes entries 0 / 1 as pure black / pure white, and the decoder
  resolves bit 0 / bit 1 through a *non-zero* colormap's first two
  triples — a foreign white-on-blue mono file now decodes faithfully
  instead of being forced to black/white. A zero-filled colormap (the
  common PCX 3.0+ form and this crate's own pre-r401 output) keeps the
  classic bit 1 = white convention, byte-for-byte.

- *(tests)* **Black-box cross-validation of the auto ladder through
  ImageMagick.** Every ladder geometry ImageMagick reads per the
  manual (Indexed4 / Indexed1x4 / Indexed8 / Rgb24) is decoded by it
  to raw RGB and compared pixel-exactly against the source. Three
  divergences are documented rather than chased: it resolves CGA
  colours from the colormap triples instead of the manual's byte-16/19
  selector, hard-codes the opposite 1-bpp polarity while ignoring the
  mono colormap, and refuses the spec's tail-less `palette_info = 2`
  grayscale form outright (callers needing such readers can emit the
  ramp through `encode_pcx_8bpp_indexed` — the Indexed8 rung is
  exactly that file).

- *(encode)* **Auto ladder: `Cga2x1` + `Cga1x2` fixed-palette CGA
  candidates.** When the image has `≤ 4` distinct colours, the ladder
  searches the entire CGA header encoding space — 4 palette-family ×
  intensity selectors (header byte 19 bits 7/6, spec §"CGA Color Map")
  × 16 background colours (header byte 16 high nibble, resolved
  through the standard EGA table) = 64 fixed palettes — for one that
  covers the colour set *exactly*. On a match both two-bit geometries
  are tried (2 bpp × 1 plane packed and the plane-oriented 1 bpp × 2
  planes) and the byte count decides; no match (or a 5th colour) skips
  the rungs entirely — the ladder never quantises. Candidate palettes
  are resolved through the decoder's own header resolver so encode-side
  matching and decode-side reconstruction cannot drift. `PcxAutoMode`
  gains `Cga2x1` / `Cga1x2` (each recording the chosen
  `palette_selector` / `background_index`); both thread authoring DPI
  through `encode_pcx_image_auto`.

- *(encode)* **Auto ladder: `Indexed4` + `Indexed1x4` 16-colour
  header-palette candidates.** When the image has `≤ 16` distinct
  colours the ladder now tries both four-bit geometries: 4 bpp × 1
  plane packed nibbles and the plane-oriented 1 bpp × 4 planes (spec
  table §3.1), each carrying the exact first-seen palette in the
  48-byte header `Colormap` field (spec §3) — half a byte per pixel and
  no 769-byte VGA tail. The two forms hold identical bits but RLE sees
  them differently: packed nibbles win on noise (no bit-plane
  periodicity, and index bytes stay below the `0xC0` escape threshold),
  while striped/periodic content collapses whole bit-plane rows into
  single RLE packets and hands the win to the planar form — the ladder
  encodes both and lets the byte count decide. The all-zero-palette
  corner (single pure-black colour) stays exact because the decoder's
  hardware-EGA substitution maps index 0 to black too. `PcxAutoMode`
  gains `Indexed4 { colors }` / `Indexed1x4 { colors }`; both thread
  authoring DPI through `encode_pcx_image_auto`.

- *(encode)* **Auto ladder: `Mono1` + `EgaRgb1x3` candidates.** When
  every distinct colour is pure black / pure white, the ladder now
  derives the 1 bpp × 1 plane monochrome form (spec §4.1, bit 1 =
  white) — one bit per pixel, the smallest geometry PCX defines. When
  every channel of every colour is `0x00` / `0xFF` (the eight EGA RGB
  primaries), the 1 bpp × 3 plane form (spec §4 bit-plane example)
  enters the contest at three bits per pixel with no stored palette.
  Both are exact by construction (the mono LUT admits only
  `#000000` / `#FFFFFF`; the EGA writer's `>= 0x80` channel threshold
  is the identity on primary values) and both thread authoring DPI
  through `encode_pcx_image_auto`. A bilevel image qualifies for both;
  Mono1's single plane wins the byte count. `PcxAutoMode` gains the
  `Mono1` and `EgaRgb1x3` variants.

- *(encode)* **Auto ladder: `Gray8` candidate.** `encode_pcx_rgb_auto` /
  `encode_pcx_image_auto` now derive an 8 bpp × 1 plane grayscale
  candidate (spec §3 `palette_info = 2`, no VGA tail) whenever every
  distinct colour is a pure grey (`r == g == b`), and the r376
  two-candidate size comparison is generalised into a
  fixed-preference-order candidate ladder. Dropping the fixed 769-byte
  tail wins for typical grayscale content; when RLE escape economics
  favour first-seen indices instead (many literals ≥ `0xC0`) the indexed
  form still wins the byte count — the ladder compares, never assumes.
  `PcxAutoMode` gains the `Gray8` variant (breaking for exhaustive
  matches); `encode_pcx_image_auto` threads authoring DPI through the new
  branch via `encode_pcx_8bpp_grayscale_dpi`.

- *(encode)* **`encode_pcx_rgb_auto` — compact-mode auto-selecting RGB
  writer.** Given packed `width × height × 3` RGB, it emits the smallest
  *lossless* PCX 5.0 file the way PC Paintbrush did. A single raster scan
  assigns each distinct colour a first-seen index and bails the moment a
  257th colour appears: with `> 256` colours the only lossless option is
  the 8 bpp × 3 plane planar 24-bit form (spec §"24-bit .PCX files",
  byte-identical to `encode_pcx_24bpp`); with `≤ 256` it encodes **both**
  the 8 bpp × 1 plane indexed candidate (256-entry VGA tail palette, spec
  §"VGA 256-color palette") and the planar candidate and keeps whichever
  is **fewer bytes**. The indexed form wins decisively for any
  non-trivial low-colour image (~⅓ the planar size for synthetic / UI /
  low-colour art), but for a *tiny* image the fixed 769-byte palette tail
  can exceed the whole planar file, so planar is returned instead — the
  "most compact" guarantee holds at every size, not just the large-image
  asymptote. Both candidates are exact (no quantisation), so the file
  decodes back to the original RGB bit-for-bit regardless of branch.
  Returns the chosen `PcxAutoMode` (`Indexed8 { colors }` / `Rgb24`)
  alongside the bytes; the first-seen palette order and the
  indexed-wins-an-exact-tie rule keep the output deterministic. No new
  on-disk geometry — only the size-minimising choice between two existing
  spec modes. New public exports: `encode_pcx_rgb_auto`, `PcxAutoMode`.

- *(encode)* **`encode_pcx_image_auto` — `PcxImage`-level compact-mode
  wrapper.** The image-level companion to `encode_pcx_rgb_auto`,
  mirroring the existing `encode_pcx_24bpp_image` convenience wrapper. It
  flattens an `Rgba` / `Rgb24` `PcxImage` (dropping alpha; rejecting
  `Indexed8`) and emits the smaller of the indexed / planar candidates,
  returning the chosen `PcxAutoMode`. Header metadata is preserved
  losslessly per branch: the planar branch delegates to
  `encode_pcx_24bpp_image` so the full `(window_origin, dpi,
  screen_size)` triple round-trips, and the indexed branch threads
  authoring DPI through `encode_pcx_8bpp_indexed_dpi`. Because the
  indexed geometry has no window-origin / screen-size header variant, an
  image that carries either of those fields falls back to the planar
  branch rather than silently dropping the metadata — so the wrapper is
  lossless on both pixels *and* the requested header annotations. New
  public export: `encode_pcx_image_auto`.

### Changed

- *(encode, perf)* **1-bit-per-plane encoders now pack whole bytes**
  instead of scattering one bit at a time. All four 1-bpp-per-plane
  paths (`encode_pcx_1bpp_mono`, `encode_pcx_1bpp_2planes_cga`,
  `encode_pcx_1bpp_3planes_ega_rgb`, `encode_pcx_1bpp_4planes_ega`)
  shared a per-pixel scatter inner loop — one branch-guarded indexed
  read-modify-write `row[plane·bpl + x/8] |= 1 << (7 − x%8)` store per
  set bit, the documented encoder hotspot (`BENCHMARKS.md` rank #2/#3).
  They now route through a single `pack_1bpp_plane_row` helper that folds
  eight consecutive pixels into one accumulator (shift-OR) and writes
  each output byte once, eliminating the per-pixel array index, the
  per-pixel branch into the destination, and the read-modify-write.
  Output is **byte-identical** to the scatter form: bit `7 − k` of
  output byte `b` holds pixel `8·b + k`, the sub-8-pixel scanline tail
  contributes the same trailing zeros, and the even-stride padding byte
  stays at its zeroed value (spec §"Image File (.PCX) Format": "each line
  of the image is stored by color plane", MSB-first bit order). Measured:
  `encode_1bpp_mono_512x512` 321 MiB/s → 2.94 GiB/s (~9.3×),
  `encode_1bpp_4planes_ega_320x240` 113 MiB/s → 629 MiB/s (~5.6×). No
  spec-behaviour change; every round-trip sweep stays bit-exact.

### Tests

- *(encode)* **Byte-exact bit-pack regression suite**
  (`tests/round362_bitpack.rs`). Pins the new `pack_1bpp_plane_row`
  packer byte-identical to the per-pixel scatter it replaced across the
  risky dimension — the scanline tail when `width` is not a multiple of
  8 (the last output byte carries < 8 pixels) and the even-stride padding
  byte that must stay zero. Re-encodes through all four 1-bpp-per-plane
  paths at a width sweep hitting every `width % 8` residue (1..=8) plus
  the multi-byte and odd-`div_ceil(8)` cases, asserting exact recovery
  via the matching typed accessor; one test additionally rebuilds the
  whole PCX file (header + RLE planar region) from an independent scatter
  reference and compares it byte-for-byte to the encoder's output, so a
  stray tail or padding bit that a round-trip could mask (decode strips
  padding) is caught at the byte level. 5 tests.

- *(decode)* New over-padded-`bytes_per_line` robustness suite
  (`tests/round354.rs`). The crate's own encoders always emit the
  *minimal* even per-plane stride, so every prior round-trip sweep only
  exercised the decoder at the tightest possible `bytes_per_line`. The
  format explicitly permits a scanline to carry arbitrary trailing
  padding beyond the picture window — the ZSoft manual's "Do NOT
  calculate from Xmax-Xmin" and the cross-reference summary's
  `LinePaddingSize = ((BytesPerLine × NumBitPlanes) × (8 / BitsPerPixel))
  - ((XEnd - XStart) + 1)` ("Any PCX image may contain extra bytes of
  padding at the end of each scan line"). The suite hand-builds raw PCX
  bitstreams with a deliberately over-padded stride (surplus 0/2/4/6/16
  bytes, sentinel-filled) across every decoded `(bits_per_pixel,
  n_planes)` mode (1/2/4/8 bpp × 1 plane plus the multi-plane EGA/CGA/
  24-bit/composite forms) and asserts both `parse_pcx` packed RGBA and
  every typed accessor recover exactly the `width × height` window with
  the padding stripped — a previously-untested decode dimension.
  Additionally pins three edges of the same dimension: an over-padded
  file decodes byte-identically to its minimal-stride twin
  (`overpadded_24bpp_equals_minimal_stride`); a pathologically large
  stride that claims a multi-MiB planar buffer behind a handful of RLE
  bytes is rejected by the decompression-bomb guard rather than OOM'd
  (`pathological_overpad_is_rejected_not_bombed`); and a stride one byte
  below the picture-window minimum is rejected as mis-framing
  (`under_minimum_stride_is_rejected`) — so the accepted over-pad band is
  bounded both above (by what the RLE input can back) and below (by the
  real width requirement). 12 tests total.

### Changed

- *(decode)* RLE decode is now **continuous across the whole image**
  instead of resetting at every scanline. The manual's own sample
  reader (`docs/image/pcx/pcx-pcgpe.txt` lines 316-326) consumes the
  stream straight through `BytesPerLine × NPlanes × (1 + Ymax - Ymin)`
  bytes with no per-scanline break, and the spec's "decoding break at
  the end of each scan line" is an encoder convention, not a decode-time
  requirement. A PCX whose run packet straddles a scanline, plane, or
  trailing-padding boundary — produced by encoders that don't break runs
  at row ends — now decodes byte-identically to its row-broken
  equivalent rather than being rejected mid-row. A run that would
  overrun the whole-image byte total is still rejected; only the per-row
  cap was relaxed. Output for every spec-conformant file (where runs
  never cross the boundary) is unchanged.

### Added

- *(tests)* **Exhaustive cross-dimensional round-trip property sweep**
  (`tests/round345.rs`). For every one of the ten encode→decode paths
  (8 bpp indexed, 8 bpp grayscale, 4 bpp packed, 1 bpp × 4 EGA, 1 bpp ×
  3 EGA-RGB, 2 bpp CGA, 1 bpp × 2 CGA, 24-bit, 4 bpp × 4 composite,
  1 bpp mono) it walks a Cartesian product of 13 widths × 6 heights and
  asserts a **bit-exact** index / sample recovery. The width set is
  chosen to straddle every packing seam the format has: odd widths that
  force the even-`bytes_per_line` padding (spec §"ZSoft .PCX File Header
  Format"), widths landing exactly on and one-past each sub-byte chunk
  boundary (8 px/byte for 1 bpp, 4 for 2 bpp, 2 for 4 bpp), and the
  degenerate 1- and 2-column cases. ~780 full round-trips total. Payloads
  come from a tiny in-file xorshift so the sweep is deterministic and
  dependency-free.
- *(tests)* **RLE codec algebraic property tests** (`tests/round345_rle.rs`)
  exercising `oxideav_pcx::rle::{encode, decode}` directly: the
  encode/decode inverse on random streams (including every `0xC0..=0xFF`
  high byte the spec requires escaping), the 2-byte high-byte escape, the
  63-byte run cap with multi-packet splitting, mid-packet / short-stream
  truncation rejection, the `out_len` overrun cap, the count-zero header
  tolerance (matching the manual's `encget`), append-to-existing-buffer
  semantics, and a packet-grammar invariant that the encoded stream never
  emits a bare high-byte literal.
- *(fuzz)* New **`encode_pcx` cargo-fuzz target** — the symmetric
  encoder counterpart to `decode_pcx`. It carves two `u16` dimensions
  off the front of the fuzz input (masked to a `0x1FF` ceiling so the
  packing-geometry edge cases — odd vs even widths, the 2 / 4 / 8
  pixels-per-byte sub-byte chunk boundaries, `bytes_per_line` rounding —
  are all exercised without the fuzzer trivially driving a multi-gigabyte
  allocation off a `0xFFFF × 0xFFFF` claim) and feeds the remaining bytes
  as the pixel/index payload to **every** public encoder
  (`encode_pcx_8bpp_indexed`, `…_8bpp_grayscale`, `…_1bpp_mono`,
  `…_4bpp_packed`, `…_1bpp_4planes_ega`, `…_2bpp_cga`,
  `…_1bpp_2planes_cga`, `…_24bpp`, `…_1bpp_3planes_ega_rgb`,
  `…_4bpp_4planes`). Each encoder must return `Ok`/`Err` without
  panicking, going out of bounds, or overflowing (debug); where it
  succeeds, the bytes it emits are fed straight back through `parse_pcx`
  and the matching typed accessor so the **encode→decode seam** is under
  the same no-panic contract. A 40-second run executed 958k+ iterations
  with zero crashes. The `fuzz` crate is no longer decode-only; the
  manifest now declares two `[[bin]]` targets. Harness-only change; no
  encode behaviour altered.

- *(tests)* New **`round337` encoder property sweep** in the CI-run
  `tests/` harness (the fuzz target above only runs under a manual fuzz
  session; this proves the contract green on every push). It sweeps a
  12-entry dimension matrix spanning the packing edge cases (1×1, 1-tall,
  1-wide, odd / even widths, sub-byte chunk boundaries) across the seven
  index/sample-based encoders and asserts two properties: (1) every
  encoder either returns `Ok` for a correctly-sized input or a typed
  `Err` for a deliberately-undersized one, a zero dimension, a
  wrong-length palette, or an out-of-range CGA background — never a
  panic; (2) **lossless round-trip through the typed accessor** —
  `parse_*indexed*(encode(indices)).indices == indices` after masking to
  each mode's bit width (2-bit CGA, 4-bit EGA, 8-bit VGA, 16-bit
  composite), plus pixel-exact flatten checks for the 24-bit and
  grayscale paths. Crate test total 216 → 224.

- *(fuzz)* The `decode_pcx` cargo-fuzz target now feeds arbitrary bytes to
  **every** public decode entry point (twelve surfaces). It previously
  covered five (`parse_pcx`, `parse_dcx`, `parse_pcx_indexed_8bpp`,
  `parse_pcx_indexed_4bpp`, `parse_pcx_indexed_1bpp_4planes`); the seven
  newer typed accessors added since — `parse_pcx_indexed_4bpp_ega_hw`,
  `parse_pcx_indexed_2bpp_cga`, `parse_pcx_indexed_1bpp_2planes_cga`,
  `parse_pcx_indexed_2bpp_cga_cpi`, `parse_pcx_cga_cpi`,
  `parse_pcx_indexed_1bpp_3planes`, `parse_pcx_indexed_4bpp_4planes` —
  each carry distinct offset / allocation / index-stacking maths (the CGA
  byte-16/19 selector dispatch, the EGA-hardware 4-level quantiser, the
  3-plane / 4×4 bit stacking, the `u16`-per-pixel `(4, 4)` buffer) that
  were not previously under the no-panic / no-OOB / no-overflow contract.
  A 45-second run over the shared corpus executed 4.5M+ iterations with
  zero crashes and added new coverage units, confirming the new surfaces
  reach fresh code paths. Harness-only change; no decode behaviour altered.

- *(decode/encode)* 4 bpp × 4 planes composite-index mode — the one
  `(bpp, planes)` slot the EGFF canonical PCX video-mode matrix does not
  list as a hardware video mode but which the format is structurally able
  to describe (`MaxNumberOfColors = 1 << (BitsPerPixel * NumBitPlanes) =
  1 << (4 × 4) = 65536`). New `encode_pcx_4bpp_4planes` writes the
  plane-oriented 16-bit composite indices (plane `k` carries nibble `k`,
  2 pixels/byte high-nibble-first per the standard PCX plane layout) and
  `parse_pcx_indexed_4bpp_4planes` surfaces them back into a new
  `PcxIndexed4x4` view (one `u16` per pixel, top-down, padding stripped).
  No palette is read or written: the spec defines no 65536-entry palette
  geometry for this mode, so the indices are surfaced raw and
  `parse_pcx`'s `Rgba` flatten path continues to reject `(4, 4)` rather
  than invent a colour mapping. Strictly additive — all existing decode /
  encode paths are unchanged. This closes the last `(bpp, planes)` slot
  named in the README "Lacks" tail.

- *(decode)* EGA hardware 4-level palette quantisation per spec
  §"EGA/VGA 16-color palette" (the rev-5 manual's "on an IBM EGA there
  are only 4 levels of RGB for each color" table). New public helpers
  `ega_quantize_level` (stored 0..=255 component → EGA level 0..=3),
  `ega_quantize_component` (level → EGA DAC output ramp
  `0x00 / 0x55 / 0xAA / 0xFF`), and `ega_quantize_palette` (whole
  16-entry palette), plus the typed accessor
  `parse_pcx_indexed_4bpp_ega_hw` — the EGA-hardware sibling of
  `parse_pcx_indexed_4bpp` that surfaces the same indices and
  palette-source tag with the palette snapped to the colours an IBM EGA
  actually displays. Strictly additive — the raw `parse_pcx_indexed_4bpp`
  view and the canonical `parse_pcx` flatten path are unchanged.
- *(decode)* `parse_pcx_cga_cpi` — spec-faithful CGA flatten-to-`Rgba`
  entry point honouring the full C / P / I decomposition of header byte 19
  (incl. the color-burst monochrome composite-grey ramp) across both the
  `2 bpp × 1 plane` packed and `1 bpp × 2 planes` planar CGA layouts; the
  flatten sibling of `parse_pcx_indexed_2bpp_cga_cpi`. Strictly additive —
  the legacy `parse_pcx` flatten path is unchanged.

### Fixed

- *(fuzz)* The `fuzz/` sub-crate's `Cargo.lock` is now committed. The
  crate-root `.gitignore` carried a bare `Cargo.lock` pattern which, with
  no leading slash, matches at any depth and silently swallowed
  `fuzz/Cargo.lock` — leaving the binary fuzz crate without a pinned
  lockfile in version control. The pattern is now anchored to the crate
  root (`/Cargo.lock`), so the library lockfile is still ignored (it must
  not be committed) while the `fuzz/` binary crate's lockfile is tracked
  as a reproducible-build artefact.

## [0.0.3](https://github.com/OxideAV/oxideav-pcx/compare/v0.0.2...v0.0.3) - 2026-06-15

### Fixed

- *(decode)* confine VGA tail-palette probe to 8 bpp × 1 plane mode

### Other

- *(rle)* bulk run-fill via Vec::resize in spec §3.2 RLE decode
- 4-colour CGA in the plane-oriented 1 bpp × 2 planes layout
- authoring-DPI override for the EGA/CGA palette-mode writers
- Round 286: decode phase-split bench probe + ranked-hotspot BENCHMARKS.md
- Round 275: spec-faithful CGA C/P/I selector + color-burst monochrome mode
- untrack fuzz/Cargo.lock (was committed + gitignored, blocking release-plz)
- Round 267: typed 1 bpp × 3 planes 8-colour EGA RGB paletted accessor
- Round 257: typed 2 bpp × 1 plane CGA paletted accessor
- Round 252: typed 1 bpp × 4 planes paletted accessor
- drop release-plz.toml — use release-plz defaults across the workspace
- Round 244: fuzz target extends to parse_pcx_indexed_4bpp
- Round 241: typed 4 bpp × 1 plane paletted accessor
- Round 237: typed 8 bpp × 1 plane paletted accessor
- Round 231: authoring screen-size (h_screen_size / v_screen_size) round-trip
- Round 225: window-origin ('x_min' / 'y_min') round-trip
- Round 219: authoring DPI (h_dpi / v_dpi) round-trip
- Round 215: 1 bpp × 3 planes (8-colour EGA RGB) decode + encode
- Round 209: planar-unpack hot-path rewrite — 10-39% decode speedup
- Round 203: scrub enumerated-denial prose in lib.rs + README
- Round 197: Criterion bench harness (decode + encode + roundtrip)
- Round 185: framework Encoder accepts Bgr24/Bgra/MonoBlack/MonoWhite
- add decode_pcx cargo-fuzz target + fix 2 decoder crashes
- Round 88: framework Encoder accepts Gray8 frames
- Round 82: palette_info=2 grayscale flag, windowed 24bpp writer, bytes_per_line guard
- Round 75: DCX container as a registered Demuxer/Muxer
- move round-2 entry under [Unreleased] (rebase fix)
- Round 2: 2/4 bpp packed-bits decode + CGA palette + DCX + indexed/EGA writers

### Fixed

- Round 308: confine the appended VGA tail-palette probe to the
  256-colour Extended VGA mode (`8 bpp × 1 plane`). Spec §"VGA 256-color
  palette" introduces the appended 768-byte block only for images with
  "more than 16 colors", and spec §"24-bit .PCX files" states 24-bit
  (8 bpp × 3 plane) images "do **not** contain a palette"; every
  sub-256-colour mode carries its palette in the header `Colormap`
  field. The decoder previously ran the `0x0C`-marker-769-bytes-from-EOF
  probe for *every* `(bpp, planes)` mode, so a 24-bit / EGA / CGA stream
  whose RLE payload happened to carry that byte pattern had 769 bytes of
  real pixel data mis-claimed as a palette and stripped from the RLE
  region — corrupting the decode or failing it as a truncated stream.
  This is exactly the coincidental-marker hazard the EGFF cross-reference
  flags for v3.0 files ("24-bit PCX images are always marked as v3.0,
  yet never have an attached color palette" / the marker "might be 0Ch
  by coincidence"). Regression-pinned by `tests/round308.rs` with a
  hand-crafted 24-bit fixture that plants the marker inside the real
  pixel region.

### Added

- Round 301: 4-colour CGA in the plane-oriented `1 bpp × 2 planes`
  layout — the last uncovered row of the EGFF canonical PCX video-mode
  matrix (`BitsPerPixel = 1, NumBitPlanes = 2`), the bit-plane sibling
  of the packed `2 bpp × 1 plane` CGA mode the crate already had.
  * New decode arm in `parse_pcx` (each scanline carries plane 0 then
    plane 1; the bit at each x-position stacks into the 2-bit index
    `p0 | p1 << 1`, the same bit ordering the 1 bpp × 4 planes EGA path
    uses). The 4-entry CGA palette resolution from header bytes 16 / 19
    is shared verbatim with the packed mode, so identical indices
    flatten to identical pixels through either layout.
  * New typed accessor `parse_pcx_indexed_1bpp_2planes_cga` returning
    `PcxIndexed1x2Cga` (indices + resolved palette + background index +
    the shared `Pcx2bppCgaPaletteSource` tag), mirroring the packed
    `parse_pcx_indexed_2bpp_cga` accessor.
  * New writers `encode_pcx_1bpp_2planes_cga` and
    `encode_pcx_1bpp_2planes_cga_dpi`, mirroring the
    `encode_pcx_2bpp_cga` / `_dpi` pair but emitting two 1-bit planes
    per scanline. A decode → re-encode round-trip is byte-identical.
  * Every row of the EGFF canonical mode matrix (monochrome / CGA / EGA
    / EGA-VGA / Extended-VGA / Extended-VGA-XGA) is now covered on both
    decode and encode.

- Round 295: authoring-DPI override for the four EGA / CGA palette-mode
  writers, completing the `*_dpi` writer suite the r219 work started.
  * New `encode_pcx_4bpp_packed_dpi`, `encode_pcx_2bpp_cga_dpi`,
    `encode_pcx_1bpp_3planes_ega_rgb_dpi`, and
    `encode_pcx_1bpp_4planes_ega_dpi`. Each mirrors its non-DPI sibling
    byte-for-byte except for the header `h_dpi` / `v_dpi` words (spec §3
    offsets 12 / 14). The DPI field is a format-independent header word
    per spec §3 — "the resolutions at which the image was created
    (printer or scanner)" — so a 16-colour EGA / 4-colour CGA image
    scanned at e.g. 300 × 300 is as spec-conformant as a 24-bit one. A
    decode → re-encode of a scanned palette-mode PCX now preserves the
    authoring resolution instead of flattening it to the historical
    72×72 default. A 0 in either component is rejected at the writer
    boundary per the spec §3 "0 = unset" sentinel, matching the existing
    `_dpi` writers. The pixel-data region is untouched, so the decode
    path and the typed paletted accessors produce identical output;
    `src/` decode/encode output bytes for the non-DPI paths are
    byte-identical to the pre-r295 tree.

- Round 286 (depth-mode benchmark): a phase-split probe in the `decode`
  Criterion harness plus a ranked-hotspot `BENCHMARKS.md`.
  * New `decode_phase_rle_24bpp_640x480` / `decode_phase_rle_8bpp_grayscale_512x512`
    benches call a new `#[doc(hidden)]` `__bench_decode_planar_len`
    accessor that runs only the header-validation + RLE-decode phase
    (`decode_planar_scanlines`) — the exact code the production decoder
    calls, returning just the planar-buffer byte count so the public
    type surface is unchanged. Timing it next to the full `parse_pcx`
    attributes decode cost to each phase (RLE-decode vs per-plane
    assembly). `src/` decode/encode output bytes are byte-identical to
    the pre-r286 tree.
  * `BENCHMARKS.md`: the full r286 baseline (decode / encode /
    roundtrip across every (depth, planes) layout + DCX) and a ranked
    hotspot table. Finding: the spec §3.2 RLE codec (`rle::decode`) is
    ~95% of 24bpp decode time while per-plane assembly is already cheap
    post-r209 — `rle::decode` is named the next profile-optimisation
    target, with `encode_1bpp_4planes_ega` the close secondary.

- Round 275: spec-faithful CGA C / P / I selector decode + the
  color-burst monochrome mode. The verbatim ZSoft PCX Technical Reference
  Manual, Revision 5 ("CGA Color Map", Header Byte #19) defines the CGA
  palette byte as three significant bits ordered C, P, I — `C` (bit 7,
  color burst: 0 = color / 1 = monochrome), `P` (bit 6, palette family),
  `I` (bit 5, intensity: 0 = dim / 1 = bright). The pre-r275
  `parse_pcx_indexed_2bpp_cga` accessor reads only bits 7 / 6 and never
  the intensity bit at position 5, so it could not represent the manual's
  `color burst = monochrome` mode nor the dim/bright distinction.
  * New `parse_pcx_indexed_2bpp_cga_cpi(input) -> Result<PcxIndexed2x1CgaCpi>`
    decode accessor and `encode_pcx_2bpp_cga_cpi(w, h, &indices, cpi, bg)`
    writer, exchanging a `Pcx2bppCgaCpi { monochrome, palette_white,
    intensity_bright }` triple (with `from_byte19` / `to_byte19` helpers
    that mask off the lower five "ignored" bits per the manual).
  * The monochrome (color-burst) mode resolves a four-level composite-grey
    ramp derived from the spec's own EGA quantisation table ("EGA/VGA
    16-color palette", four signal levels), in dim and bright flavours;
    entry 0 is still overridden by the header byte 16 background nibble.
    Round-trips byte-for-byte through the new writer.
  * The legacy `parse_pcx_indexed_2bpp_cga` accessor, `parse_pcx`'s `(2, 1)`
    flatten path, and the `encode_pcx_2bpp_cga` writer are unchanged — the
    new C / P / I pair is strictly additive.

- Round 267: typed 1 bpp × 3 planes 8-colour EGA RGB paletted accessor —
  the fifth (and final) paletted typed view, covering the 8-colour EGA
  RGB mode described in spec §4 (each scanline carries three 1-bit planes
  laid out one after another within the row, plane order R, G, B). The
  four pre-r267 typed accessors (`parse_pcx_indexed_8bpp` /
  `parse_pcx_indexed_4bpp` / `parse_pcx_indexed_1bpp_4planes` /
  `parse_pcx_indexed_2bpp_cga`) covered every 8 bpp / 16-colour / CGA
  mode; r267 closes the last paletted gap (8-colour RGB).
  * New `parse_pcx_indexed_1bpp_3planes(input: &[u8]) -> Result<PcxIndexed1x3>`
    public entry point. The `PcxIndexed1x3` shape mirrors the pre-existing
    typed views: a `width × height` byte buffer of resolved colour indices
    (low three bits = colour index `0..=7` in the order `r | g << 1 | b
    << 2`, top-down, padding stripped) plus the fixed 8-entry on/off-
    primary RGB palette and a new `Pcx1bpp3PlanesPaletteSource` tag.
  * Unlike the 16-colour EGA / 256-colour VGA / CGA modes — which read a
    palette out of the header `ega_palette` field or a VGA tail block —
    the 8-colour RGB mode carries no on-disk palette: each plane bit
    directly toggles its channel between `0x00` and `0xFF`, so the eight
    colours are intrinsic to the plane bits (spec §4 bit-plane example).
    `Pcx1bpp3PlanesPaletteSource` therefore has a single `FixedPrimaries`
    arm — present for API symmetry with the other `*PaletteSource` tags
    and to document the no-header-palette property explicitly.
  * The typed view is a strict rearrangement (NOT a divergence) of the
    canonical `parse_pcx` RGBA flattener: flattening the surfaced indices
    through the surfaced palette reproduces the exact bytes `parse_pcx`
    emits (covered by `typed_view_agrees_with_canonical_flattener`). A
    decode → re-encode pass through `encode_pcx_1bpp_3planes_ega_rgb`
    round-trips byte-exactly when the source is already on the
    `{0x00, 0xFF}` channel cut.
  * Any (depth, planes) combination other than `(1, 3)` is rejected with
    `PcxError::Unsupported` (covered by `rejects_non_1bpp_3planes_inputs`
    against the 24-bit / mono / grayscale / 4 bpp / 1 bpp × 4 planes /
    2 bpp CGA modes). Per-row padding for non-byte-aligned widths is
    stripped (`strips_per_row_padding_for_non_byte_aligned_width`), and
    the accessor shares `parse_pcx`'s validation surface
    (`shares_validation_surface_with_parse_pcx`).
  * Six-test `tests/round267.rs` harness.

- Round 257: typed 2 bpp × 1 plane CGA paletted accessor — the fourth
  paletted typed view, covering the 4-colour CGA mode listed in spec
  §4.1 (single plane of 2 bpp packed-bits data, 4 pixels/byte, palette
  selected from `ega_palette` bytes 16 / 19 per CGA hardware semantics).
  The three pre-r257 typed accessors (`parse_pcx_indexed_8bpp` /
  `parse_pcx_indexed_4bpp` / `parse_pcx_indexed_1bpp_4planes`) covered
  every 8 bpp and 16-colour mode; r257 closes the last paletted gap.
  * New `parse_pcx_indexed_2bpp_cga(input: &[u8]) -> Result<PcxIndexed2x1Cga>`
    public entry point. The `PcxIndexed2x1Cga` shape mirrors the
    pre-existing typed views: a `width × height` byte buffer of resolved
    indices (low two bits = palette index `0..=3`, top-down, padding
    stripped) plus a resolved 4-entry RGB palette, the resolved
    `background_index` (`0..=15`) read from `ega_palette[16]`'s high
    nibble, and a new `Pcx2bppCgaPaletteSource` tag with four arms
    (`Palette1HighIntensity` / `Palette1LowIntensity` /
    `Palette0HighIntensity` / `Palette0LowIntensity`) recording which
    CGA palette family the decoder landed on per `ega_palette[19]` bits
    7/6.
  * `Pcx2bppCgaPaletteSource::palette_selector()` helper reconstructs
    the byte 19 selector pattern (0x00 / 0x40 / 0x80 / 0xC0) so a
    round-trip caller can hand the typed view straight back into
    `encode_pcx_2bpp_cga` without re-deriving the bit positions —
    decode → re-encode produces a byte-identical PCX file across all
    four palette families (covered by
    `palette_selector_helper_round_trips_to_byte_identical_output`).
  * Any (depth, planes) combination other than `(2, 1)` is rejected
    with `PcxError::Unsupported` — the 16-colour packed-bits path has
    its own typed accessor `parse_pcx_indexed_4bpp`; the 8 bpp paletted
    path has `parse_pcx_indexed_8bpp`; the EGA bit-plane path has
    `parse_pcx_indexed_1bpp_4planes`.
  * The accessor shares its validation surface with `parse_pcx`
    (manufacturer byte, version table, encoding byte, dimension
    underflow, `bytes_per_line < min_bpl` mis-framing, `scanline ×
    height` overflow, decompression-bomb cap) via the existing
    `decode_planar_scanlines` factoring established by the r237 / r241
    / r252 typed accessors.
  * Six new tests in `tests/round257.rs` validate (i) the round-trip
    through `encode_pcx_2bpp_cga` for all four CGA palette families ×
    background indices, (ii) the typed-view-agrees-with-canonical-
    flattener consistency check across four selectors × three
    background indices = 12 fixtures, (iii) the `palette_selector`
    helper round-trips to a byte-identical file across all four
    families × three backgrounds, (iv) `(8, 1)` / `(8, 3)` / `(1, 1)` /
    `(4, 1)` / `(1, 4)` rejection paths, (v) per-row padding stripping
    for a width-13 fixture where `bytes_per_line` is rounded up to even,
    and (vi) shared validation surface with `parse_pcx` on
    truncated / bad-manufacturer fixtures.

- Round 252: typed 1 bpp × 4 planes paletted accessor — the third
  16-colour typed view, covering the spec §4.1 EGA bit-plane on-disk
  layout where each scanline carries four 1-bit planes laid out one
  after another within the row (plane 0, plane 1, plane 2, plane 3).
  The four bits at the same x-position across the four planes stack
  into a 4-bit palette index (`p0 | p1<<1 | p2<<2 | p3<<3`).
  * New `parse_pcx_indexed_1bpp_4planes(input: &[u8]) -> Result<PcxIndexed1x4>`
    public entry point. The `PcxIndexed1x4` shape mirrors `PcxIndexed4`:
    a `width × height` byte buffer of resolved nibble indices (low
    nibble = palette index `0..=15`, top-down, padding stripped) plus a
    resolved 16-entry RGB palette and a `Pcx1bpp4PlanesPaletteSource`
    tag recording whether the header `ega_palette` field carried
    non-zero bytes (`Ega16InHeader`) or the spec table §3.1 hardware
    default was substituted (`Ega16Default`). Useful for round-tripping
    a 16-colour EGA PCX through `encode_pcx_1bpp_4planes_ega` without
    re-quantising the pixels, or for applying palette-swap operations
    on the indices directly.
  * Any (depth, planes) combination other than `(1, 4)` is rejected
    with `PcxError::Unsupported` — the 4 bpp × 1 plane path has its
    own typed accessor `parse_pcx_indexed_4bpp`; the 8 bpp paletted
    path has `parse_pcx_indexed_8bpp`. The three paletted views share
    the same nibble / byte index convention so a downstream pipeline
    can hand any of them to a 16-colour-or-greater code path without
    branching on the on-disk depth.
  * The accessor shares its validation surface with `parse_pcx`
    (manufacturer byte, version table, encoding byte, dimension
    underflow, `bytes_per_line < min_bpl` mis-framing, `scanline ×
    height` overflow, decompression-bomb cap) via the existing
    `decode_planar_scanlines` factoring established by the r237 / r241
    typed accessors.
  * Six new tests in `tests/round252.rs` validate (i) the in-header
    palette round-trip through `encode_pcx_1bpp_4planes_ega`, (ii) the
    all-zero `ega_palette` fallback to the spec table §3.1 hardware
    palette, (iii) the typed-view-agrees-with-canonical-flattener
    consistency check across both palette sources, (iv) `(8, 1)` /
    `(8, 3)` / `(1, 1)` / `(2, 1)` / `(4, 1)` rejection paths, (v)
    per-row padding stripping for widths that don't fall on a byte
    boundary, and (vi) shared validation surface with `parse_pcx` on
    truncated / bad-manufacturer fixtures.
  * `fuzz/fuzz_targets/decode_pcx.rs` now calls
    `parse_pcx_indexed_1bpp_4planes` on the same byte stream as the
    other four public entry points so a single attacker mutation
    simultaneously probes the canonical flattener AND every typed
    accessor's rejection geometry.

- Round 244: fuzz target reaches the typed 4 bpp × 1 plane paletted
  accessor. The r136 `fuzz/decode_pcx` cargo-fuzz harness drives the
  canonical `parse_pcx` / `parse_dcx` entry points; r237 added
  `parse_pcx_indexed_8bpp` to the same harness so its 256-entry palette
  dispatch + padding-strip surface runs at fuzz cadence. r241 added the
  symmetric `parse_pcx_indexed_4bpp` accessor (16-entry palette, EGFF
  "4 bpp / 1 plane / 16 colours / EGA and VGA" mode) but did not extend
  the fuzz target — meaning the nibble-unpack hot path, the (depth,
  planes) mismatch reject, and the `Ega16InHeader` / `Ega16Default`
  palette dispatch were attacker-driven only through the canonical
  `parse_pcx` flattener's RGBA byte stream, not the typed view's
  surfaced indices / palette / source tag.
  * `fuzz/fuzz_targets/decode_pcx.rs` now calls `parse_pcx_indexed_4bpp`
    on the same byte stream as the other three public entry points so a
    single attacker mutation simultaneously probes the canonical
    flattener AND every typed accessor's rejection geometry. Each call
    discards its result — the contract under test is purely that the
    call returns (never panics, OOMs, aborts, indexes out of bounds, or
    pre-allocates an attacker-claimed pixel buffer beyond what the
    input can back).
  * New seed corpus file
    `fuzz/corpus/decode_pcx/packed4_default_palette_8x4.pcx`
    (148 bytes) drives the `Ega16Default` branch — the pre-existing
    `packed4_8x4.pcx` seed has a non-zero `ega_palette` field so it
    drives `Ega16InHeader`. Both `Pcx4bppPaletteSource` arms are now
    reached on the seed alone, before the fuzzer has mutated anything.
  * Eight new tests in `tests/round244.rs` validate the fuzz target's
    contract from the in-tree test runner (the fuzz crate is a
    separate `cargo-fuzz` workspace not built by `cargo test`):
    * `every_seed_returns_a_result` walks every committed seed
      through `parse_pcx_indexed_4bpp` and asserts the call returns
      rather than panicking — the direct contract under test in the
      fuzz target's `fuzz_target!` invocation.
    * `seed_packed4_in_header_branch` pins the existing
      `packed4_8x4.pcx` seed to `Ega16InHeader`.
    * `seed_packed4_default_palette_branch` pins the new
      `packed4_default_palette_8x4.pcx` seed to `Ega16Default` and
      asserts the surfaced palette equals the spec table §3.1 EGA
      hardware palette exactly.
    * `seed_packed4_default_palette_header_geometry` pins the new
      seed's header layout (manufacturer 0x0A, version 5, encoding 1,
      bits_per_pixel 4, n_planes 1, ega_palette all zero,
      bytes_per_line = round_up_to_even(ceil(width / 2))).
    * `non_4_1_seeds_reject_with_unsupported` walks every non-(4, 1)
      seed (mono / CGA / 1bpp×4 EGA / gray / idx8 / RGB24 / RGB24
      windowed) and asserts the typed accessor rejects with
      `Err(PcxError::Unsupported(_))`.
    * `malformed_seeds_reject` walks the DCX / magic-only / empty
      seeds and asserts both the typed accessor and the canonical
      flattener reject — the shared validation surface holds.
    * `seeds_typed_view_matches_canonical_flatten` pins the typed
      view as a pure rearrangement of the canonical flattener for
      both `(4, 1)` seeds (indices flattened through the surfaced
      palette reproduce `parse_pcx`'s RGBA bytes exactly).
    * `synthetic_non_4_1_combinations_reject_with_unsupported`
      complements the seed-corpus check by generating fresh
      mono / CGA / gray / RGB24 fixtures through the public encoder
      API and asserting the typed accessor rejects each.
  * Both the default registry build and the standalone
    `--no-default-features` build stay green; `cargo fmt --check` +
    `cargo clippy --all-targets --no-deps -- -D warnings` clean.

- Round 241: typed 4 bpp × 1 plane paletted accessor. EGFF video-mode
  table entry "4 bpp / 1 plane / 16 colours / EGA and VGA" describes a
  16-colour packed-bits PCX where each on-disk byte holds two pixels
  (high nibble = even-x, low nibble = odd-x) and the 16-entry palette
  rides in the header's 48-byte `ega_palette` field — with the rev-5
  manual noting that PCX 3.0+ writers commonly leave the field at
  all-zeros, in which case the decoder substitutes the standard EGA
  hardware palette from spec table §3.1. The canonical [`parse_pcx`]
  entry point flattens this to packed `Rgba`, which discards the on-
  disk nibble indices the file actually carries.
  * New `parse_pcx_indexed_4bpp(input) -> Result<PcxIndexed4>` typed
    accessor returns the unpacked `width × height` index buffer (one
    byte per pixel, low nibble = palette index `0..=15`, top-down,
    with the spec §1 `bytes_per_line` even-rounding padding stripped)
    alongside the resolved 16-entry RGB palette and a
    `Pcx4bppPaletteSource` tag (`Ega16InHeader` / `Ega16Default`)
    recording whether the on-disk `ega_palette` field or the spec
    table §3.1 hardware default produced the palette. Symmetric to
    `parse_pcx_indexed_8bpp` for the 256-colour `(8, 1)` mode added in
    round 237.
  * Any (depth, planes) combination other than `(4, 1)` is rejected
    with `PcxError::Unsupported` — the 8 bpp paletted path has its own
    typed accessor; the 1 bpp × 4 planes EGA path shares the 16-entry
    palette geometry but the on-disk plane shape is different and is
    out of scope for this accessor.
  * Reuses the existing `decode_planar_scanlines` validation helper
    (manufacturer byte, version table, encoding byte, dimension
    underflow, `bytes_per_line < min_bpl` mis-framing, `scanline ×
    height` overflow, decompression-bomb cap) so the typed view's
    failure surface matches `parse_pcx` line-for-line.
  * Typed view is pinned as a strict rearrangement of the canonical
    flattener: flattening the surfaced indices through the surfaced
    palette reproduces `parse_pcx`'s RGBA bytes exactly across both
    the in-header and default-palette branches.

- Round 237: typed 8 bpp × 1 plane paletted accessor. Spec §4.1
  ("256 colour") + §3 ("Palette Information") describe an 8 bpp × 1
  plane PCX as carrying either a `palette_info = 2` grayscale flag
  (the synthetic `0..=255` ramp), an optional 256-entry VGA tail
  palette block (marker `0x0C` 769 bytes from EOF, followed by 768 RGB
  bytes), or neither. The canonical [`parse_pcx`] entry point flattens
  every case to packed `Rgba`, which is convenient for display
  pipelines but discards the on-disk palette indices.
  * New `parse_pcx_indexed_8bpp(input) -> Result<PcxIndexed8>` typed
    accessor returns the raw `width × height` index buffer (one byte
    per pixel, top-down, with the spec §1 `bytes_per_line` even-rounding
    padding stripped) alongside the resolved 256-entry RGB palette and
    a `PcxPaletteSource` tag (`VgaTail` / `GrayscaleFlag` /
    `GrayscaleFallback`) recording which spec §3 branch produced the
    palette. Useful for round-tripping a paletted PCX through
    `encode_pcx_8bpp_indexed` without re-quantising or for applying
    palette-swap operations on the indices directly.
  * Any (depth, planes) combination other than `(8, 1)` is rejected
    with `PcxError::Unsupported` — the 24-bit `(8, 3)` planar path and
    the 1/2/4 bpp EGA/CGA paths have different palette geometries and
    are out of scope for this accessor.
  * Internal `decode_planar_scanlines` helper centralises every
    clean-room guard (manufacturer byte, version table, encoding byte,
    dimension underflow, `bytes_per_line < min_bpl` mis-framing,
    `scanline × height` overflow, decompression-bomb cap, RLE
    truncation) so [`parse_pcx`] and `parse_pcx_indexed_8bpp` stay in
    lockstep on input validation — a malformed file rejected by one is
    rejected by the other.
  * The fuzz target `decode_pcx` now exercises the typed accessor on
    every input alongside `parse_pcx` / `parse_dcx`, so the
    depth/planes mismatch reject path, the padding-strip slicing, and
    the palette-source dispatch are all attacker-driven for free.
  * Eight new tests in `tests/round237.rs` cover the three palette
    sources (VGA tail / grayscale flag / grayscale fallback), the
    typed-view-equals-canonical-flattener consistency check
    (the surfaced indices flattened through the surfaced palette
    match the byte stream `parse_pcx` produces), padding strip on
    odd-width fixtures (spec §1 `bytes_per_line = 6` for `width = 5`),
    rejection of every non-(8, 1) combo (1 bpp / 2 bpp / 4 bpp /
    24-bit), shared validation surface (truncated header + bad
    manufacturer byte rejected identically), and the invariant that
    `parse_pcx` still returns `PcxPixelFormat::Rgba` (the typed
    accessor is purely additive).
- Round 231: authoring screen-size (`h_screen_size` / `v_screen_size`)
  round-trip support. Spec §3 records the header's two 16-bit screen-
  size words at offsets 70 / 72 as "Horizontal / Vertical screen size
  in pixels (new field found only in PB IV / IV Plus)" — an
  authoring-time annotation about the display resolution the image was
  composed for, distinct from the printer / scanner DPI in `h_dpi` /
  `v_dpi`. Prior to r231 the decoder discarded both fields and every
  writer hard-coded `(0, 0)`, so a tagged PB IV / IV Plus PCX silently
  lost its authoring screen size across a decode → re-encode pass.
  * `PcxImage` gains an `Option<(u16, u16)>` `screen_size` field. The
    decoder fills it with `Some((h, v))` whenever both header words
    are non-zero, and `None` otherwise (asymmetric or zeroed headers
    collapse to `None` per the spec §3 sentinel — a 0 means "unset",
    which the pre-PB-IV writers leave the field at).
  * Two new writers stamp a custom authoring screen size into the
    header in place of the historical zero-fill:
    `encode_pcx_24bpp_screen(w, h, &rgb, (h_screen, v_screen))` (24-bit
    RGB with screen-size annotation only — DPI stays at the 72×72
    default, origin at `(0, 0)`) and
    `encode_pcx_24bpp_window_dpi_screen(x_min, y_min, w, h, &rgb,
    (h_dpi, v_dpi), (h_screen, v_screen))` (the maximally-tagged
    writer combining window origin + DPI + screen size in one call).
    Each rejects a tuple with either component zero so the spec §3
    "unset" semantic stays intact at the writer boundary too.
  * Internal `write_header_full` gained a `(u16, u16)` `screen_size`
    parameter; the existing helpers (`write_header`,
    `write_header_with_palette`) and every pre-r231 public writer pass
    a new `DEFAULT_SCREEN_SIZE = (0, 0)` constant so the on-disk
    pixel-data RLE byte stream is bit-identical to the pre-r231 output
    for every legacy writer.
  * The wrapper `encode_pcx_24bpp_image` now dispatches across the
    eight `(window_origin, dpi, screen_size)` `Option` combinations
    (eight match arms covering every Some/None mix) so a decoded
    fully-tagged PCX round-trips every metadata field through one
    call instead of forcing the caller to flatten the metadata by
    hand. The four pre-r231 `(window_origin, dpi)` arms keep their
    existing dispatch paths intact, so the wrapper's output stays
    bit-identical for any input where `screen_size = None`.
  * Eighteen new tests in `tests/round231.rs` cover header offset
    placement, the spec §3 sentinel rule (zero / asymmetric collapses
    to `None`, both-non-zero surfaces as `Some(...)`), pixel-data
    invariance across the screen-size dimension, decoder→encoder→
    decoder round-trip through `PcxImage` for both screen-only and
    maximally-tagged inputs, wrapper dispatch bit-identical to each
    standalone writer in its sub-case, rejection of zero-component
    screen-size and DPI tuples, and rejection of x_min + width
    overflow. All 120 existing + new tests stay green on both the
    default and `--no-default-features` standalone builds.
- Round 225: window-origin (`x_min` / `y_min`) round-trip support.
  Spec §3 defines the image window via `(x_min, y_min)` / `(x_max,
  y_max)`, with the visible width / height derived as `x_max - x_min +
  1` and `y_max - y_min + 1`; PCX 3.0+ supports a non-zero origin so an
  editor can record the source crop region a pixel buffer came from.
  Prior to r225 the standalone `PcxImage` discarded the decoded origin
  and the `encode_pcx_24bpp_image` wrapper always re-emitted `(0, 0)`,
  so a windowed PCX silently lost its crop metadata across a decode →
  re-encode pass.
  * `PcxImage` gains an `Option<(u16, u16)>` `window_origin` field. The
    decoder fills it with `Some((x, y))` whenever either header word
    is non-zero, and `None` for the conventional zero-origin
    screen-authored case (so the re-encode wrapper doesn't restate an
    implicit `(0, 0)` as data).
  * `encode_pcx_24bpp_window_dpi(x_min, y_min, w, h, &rgb,
    (h_dpi, v_dpi))` combines the existing window-only
    (`encode_pcx_24bpp_window`) and DPI-only (`encode_pcx_24bpp_dpi`)
    writers into one call so the wrapper can round-trip both metadata
    fields at once without forking the body of the 24-bit writer for a
    fourth time.
  * `encode_pcx_24bpp_image` now dispatches across the four
    `(window_origin, dpi)` combinations (`None`/`None` → plain,
    `Some`/`None` → window, `None`/`Some` → dpi, `Some`/`Some` →
    window_dpi). The packed RGB buffer is built once up front so the
    `Rgba`-vs-`Rgb24` byte-swap stays untouched.
  * Thirteen new tests in `tests/round225.rs` cover decoder reporting
    of `Some/None` per the asymmetric / zero / non-zero origin cases,
    header-offset placement of the new combined writer, self-roundtrip
    + end-to-end round-trip through the wrapper, wrapper dispatch
    bit-identical to each standalone writer in its sub-case, and the
    new combined writer's input-rejection paths (zero-component DPI,
    origin + dimension overflow). All 102 existing + new tests stay
    green on both the default and `--no-default-features` standalone
    builds.
- Round 219: authoring DPI (`h_dpi` / `v_dpi`) round-trip support.
  Spec §3 records the header's two 16-bit DPI words at offsets 12 / 14
  as "the resolutions at which the image was created (printer or
  scanner); e.g. a scan might store 300, 300", and treats a 0 in
  either field as the documented "unset" sentinel. Prior to r219 the
  decoder discarded both fields after parsing, and every writer hard-
  coded the historical PC Paintbrush 72×72 "screen DPI" convention —
  a decode → re-encode pass therefore silently destroyed the
  authoring resolution of a scanned input.
  * `PcxImage` gains an `Option<(u16, u16)>` `dpi` field. The decoder
    fills it with `Some((h, v))` whenever both header fields are
    non-zero, and `None` otherwise (asymmetric or zeroed headers
    collapse to `None` per the spec §3 sentinel).
  * Four new writer entry points stamp a custom authoring resolution
    into the header in place of the 72×72 default:
    `encode_pcx_24bpp_dpi(w, h, &rgb, (h_dpi, v_dpi))`,
    `encode_pcx_8bpp_indexed_dpi(w, h, &indices, &palette, dpi)`,
    `encode_pcx_8bpp_grayscale_dpi(w, h, &pixels, dpi)`, and
    `encode_pcx_1bpp_mono_dpi(w, h, &pixels, dpi)`. Each rejects a
    tuple with a 0 in either component so the spec §3 "unset"
    semantic is preserved at the writer boundary.
  * The convenience wrapper `encode_pcx_24bpp_image(&img)` now
    automatically threads `img.dpi` into the header when present, so
    `parse_pcx → encode_pcx_24bpp_image` preserves the scanner DPI
    end-to-end without the caller flattening + restamping by hand.
  * Internal `write_header_full` gained a `(u16, u16)` `dpi`
    parameter; the existing helpers (`write_header`,
    `write_header_with_palette`) pass the `DEFAULT_DPI = (72, 72)`
    constant so the on-disk pixel-data RLE byte stream is
    bit-identical to the pre-r219 output for every legacy writer.
  * Thirteen new tests in `tests/round219.rs` cover header offset
    placement, asymmetric-axes encoding, decoder reporting of
    `Some/None` per the spec §3 sentinel rule, pixel-data invariance
    across the DPI dimension, decoder→encoder→decoder round-trip
    through the `PcxImage` wrapper, rejection of zero-component DPI
    tuples, and rejection of bad palette length in the new
    `_dpi`-indexed writer. All 89 existing + new tests stay green on
    both the default and `--no-default-features` standalone builds.
- Round 215: 1 bpp × 3 planes (8-colour EGA RGB) decode + encode.
  This `(bpp, planes)` combination is one of the six formal video
  modes listed in the EGFF PCX file-format summary (3 planes / 1
  bpp / 8 colours / EGA mode); the rev-5 ZSoft technical reference
  §4 bit-plane example (lines 46-58) gives the plane order R, G, B.
  Each input channel byte is thresholded at 0x80 to set its plane
  bit on encode; on decode each plane bit toggles its channel
  between 0x00 and 0xFF, producing the eight on/off primaries.
  `parse_pcx` accepts `(bits_per_pixel = 1, n_planes = 3)` and
  routes through a new `unpack_1bpp_3planes` path with the same
  `chunks_exact_mut` row-walking idiom as the other multi-plane
  paths. `encode_pcx_1bpp_3planes_ega_rgb(w, h, &rgb)` writes a
  PCX 5.0 stream with `bytes_per_line = round_up_to_even(ceil(w /
  8))` and three planar bit-planes per scanline. Seven new tests
  in `tests/round215.rs` cover all eight primaries, a 32×16 stripe
  pattern, odd-width scanline padding, the 0x80 threshold
  behaviour, a hand-crafted byte-stream decode, and the two
  encoder-input rejection paths. Both default and standalone
  (`--no-default-features`) builds carry the new entry point.

### Changed

- Round 311 (depth-mode profile / optimisation): reworked the spec §3.2
  RLE byte-stream decoder (`rle::decode` in `src/rle.rs`) — the #1
  measured decode hotspot the r286 `BENCHMARKS.md` phase-split named
  (~95% of 24bpp decode time). The per-scanline run-fill previously
  emitted a run of `count` identical bytes via a scalar
  `for _ in 0..count { out.push(lit) }` loop, paying a length + capacity
  check per byte even though the caller pre-`reserve`s the planar `Vec`
  to its exact size (so no run-fill ever reallocates). The run-fill now
  takes a length-thresholded path: runs of `count > 2` grow the buffer
  in one `Vec::resize`, letting the allocator's `memset` fast path fill
  the run, while runs of `count <= 2` keep the cheaper `push` (the
  resize bookkeeping doesn't amortise below a few bytes — the threshold
  is empirical against the decode bench, where high-entropy bit-packed
  planar layouts are dominated by very short runs + singleton literals).
  The literal path is unchanged (`push` per byte; a maximal-span
  `extend_from_slice` copy was measured slower because the per-span scan
  loop costs more than it saves on the singleton-heavy literal streams
  the low-bpp modes produce). Output bytes are bit-identical to the
  pre-r311 implementation — all 187 registry tests + the standalone
  `--no-default-features` build (roundtrip / cross_validate cover the
  byte-exact contract) stay green unmodified. r286 `decode` Criterion
  bench, median wall-clock (3 s measurement / longer runs for the
  bit-packed paths), against the r311 pre-change baseline:
  decode_24bpp_320×240 −25.5%, decode_24bpp_640×480 −11.8%,
  decode_24bpp_1920×1080 −13.5%, decode_8bpp_indexed_320×240 −12.6%,
  decode_dcx_4_pages −20.5%, phase-split decode_phase_rle_24bpp_640×480
  −12.9%, phase-split decode_phase_rle_8bpp_grayscale_512×512 −7.1%,
  decode_4bpp_packed_320×240 −5.2%, decode_1bpp_4planes_ega_320×240
  −2.6%; decode_2bpp_cga / decode_1bpp_mono within run-to-run variance
  (≤ ~1% in an uncontended run). No new dependencies, no `unsafe`, no
  SIMD intrinsics. Encoder + per-plane-assembly hot paths unchanged.
- Round 209: Restructured the six planar-unpack hot paths in
  `src/decoder.rs` to walk both source scanlines and the destination
  RGBA buffer via `chunks_exact_mut`, with pre-sliced per-plane row
  references for the multi-plane variants and pre-baked
  `[r, g, b, 0xFF]` RGBA palettes for the 8 / 4 / 2 bpp paths. The
  destination split gives the optimiser enough provenance information
  to drop the per-pixel bounds checks against `out` and lay each
  pixel's four-byte RGBA store down as a single aligned move. Output
  bytes remain bit-identical to the pre-r209 implementation — all 69
  existing tests (cross-validate / round82 / round88 / round185 /
  round2 / roundtrip + lib unit) stay green unmodified. r197 Criterion
  decode bench, median wall-clock on the same machine (3 s
  measurement, 30 samples, fresh `CARGO_TARGET_DIR` per side):
  decode_24bpp_1920×1080 6.63 ms → 5.04 ms (−24.0 %, 1.16 → 1.53 GiB/s),
  decode_24bpp_640×480 879 µs → 731 µs (−16.8 %, 1.30 → 1.57 GiB/s),
  decode_24bpp_320×240 206 µs → 185 µs (−10.2 %, 1.39 → 1.55 GiB/s),
  decode_8bpp_indexed_320×240 128 µs → 92 µs (−28.1 %, 2.24 → 3.12
  GiB/s), decode_8bpp_grayscale_512×512 491 µs → 366 µs (−25.4 %,
  1.99 → 2.67 GiB/s), decode_1bpp_mono_512×512 226 µs → 182 µs
  (−19.5 %, 4.32 → 5.38 GiB/s), decode_4bpp_packed_320×240 105 µs →
  74 µs (−29.4 %, 2.72 → 3.86 GiB/s), decode_2bpp_cga_320×240 84 µs →
  51 µs (−39.0 %, 3.42 → 5.59 GiB/s), decode_1bpp_4planes_ega_320×240
  148 µs → 114 µs (−22.8 %, 1.94 → 2.51 GiB/s). Geometric-mean
  speedup across the nine single-frame paths is ≈ 22.6 %.  No new
  dependencies, no `unsafe`, no SIMD intrinsics. Encoder hot paths
  unchanged (encode bench within run-to-run variance).
- Round 203: Rephrased the crate-level provenance prose in
  `src/lib.rs` and `README.md` to a positive sole-source-of-truth
  statement against the ZSoft PCX File Format Technical Reference
  Manual Rev 5 (1991). The wording no longer enumerates external
  PCX implementations by name.

### Added

- Round 197: Criterion benchmark harness covering decode + encode +
  roundtrip hot paths. Three new `benches/{decode,encode,roundtrip}.rs`
  harnesses mirror the png / bmp / gif / tiff shape — each scenario
  synthesises a fresh PCX on the fly via the public encoder API and
  iterates the matching decoder path (or both, for the roundtrip
  harness). Scenarios cover every spec §4.1 (depth, planes) tuple
  (1 bpp mono / 1 bpp × 4 EGA / 2 bpp CGA / 4 bpp packed / 8 bpp
  palette / 8 bpp grayscale / 8 bpp × 3 RGB), plus the DCX multi-page
  wrapper. No fixture files committed; the bench inputs are entirely
  reproducible from the bench source (deterministic xorshift32 fill).
  Headline single-threaded apple-silicon numbers on the smoke run
  include ~192 µs per 320×240 24bpp decode, ~5.18 ms per 1920×1080
  24bpp decode, ~520 µs per 512×512 8bpp grayscale decode, ~759 µs per
  4-page 320×240 DCX bundle decode, and ~9 µs per DCX assembler call
  (the assembler just concatenates pre-encoded PCX page payloads + writes
  the offset table). Saturated-crate depth-mode work per the workspace
  README's "saturated → fuzz/bench/profile" memo (the fuzz harness
  landed in r136; this round adds the bench arm).
- Round 185: framework `oxideav_core::Encoder` accepts four new
  `PixelFormat` variants on top of the round-88 `Rgba` / `Rgb24` /
  `Gray8` surface — `Bgr24`, `Bgra`, `MonoBlack`, and `MonoWhite`.
  `Bgr*` variants are per-pixel byte-swapped to RGB before encode
  (alpha dropped from `Bgra`); `Mono*` variants unpack the MSB-first
  1-bit stride into one byte per pixel and route to
  `encode_pcx_1bpp_mono`, with `MonoWhite` inverted so the on-disk
  PCX still carries the spec §4.1 bit-1 = white polarity. The codec
  capabilities advertise all seven accepted formats so pipeline
  pickers see them before construction. New `tests/round185.rs`
  covers byte-swap, alpha-drop, both monochrome polarities,
  non-tight strides on mono input, undersized-data refusal, and the
  capability advertisement.
- Round 136: `fuzz/` cargo-fuzz harness (`decode_pcx` target) driving
  `parse_pcx` + `parse_dcx` on arbitrary byte buffers, with a 12-entry
  seed corpus covering all six (depth, planes) combinations, grayscale,
  a windowed-origin file, a DCX bundle, and degenerate inputs. 40M
  executions, zero crashes after the fixes below. Daily 30-minute CI
  fuzz workflow.

### Fixed

- Round 136 (fuzz): `PcxHeader::width()` / `height()` no longer panic
  with an integer-underflow on a malformed header where `x_max < x_min`
  (or `y_max < y_min`). The accessors are public and may be called on an
  unvalidated header straight out of `parse_header`, so they now use
  saturating subtraction (yielding `0`); `parse_pcx` still rejects such
  headers with a typed error.
- Round 136 (fuzz): decompression-bomb guard in `parse_pcx`. A header
  claiming enormous dimensions backed by only a few RLE bytes used to
  eagerly `Vec::with_capacity(scanline × height)` and OOM-abort (a
  ~398 GB reservation for a tiny input was observed). The decoder now
  computes `scanline × height` with `checked_mul` and rejects any claim
  exceeding what the available pixel bytes could decode (PCX RLE expands
  at most ~31.5:1), so the initial reservation is bounded by the actual
  input size.

### Added (prior rounds)

- Round 88 encoder: framework `oxideav_core::Encoder` now accepts
  `PixelFormat::Gray8` video frames and routes them through the
  round-82 `encode_pcx_8bpp_grayscale` writer (8 bpp × 1 plane,
  `palette_info = 2` per spec §3, no VGA tail palette). The
  `pcx_sw` codec capabilities advertise `Gray8` alongside the
  existing `Rgba` / `Rgb24` so pipeline pickers see the format
  before construction.
- Round 82 decoder: honour the spec §3 `palette_info = 2` grayscale
  flag for 8 bpp × 1 plane images — the decoder emits a grayscale
  triple `(g, g, g, 0xFF)` per pixel regardless of any 256-colour
  VGA tail palette in the file. Some scanner / FAX-era PCX writers
  emit the flag without a tail palette; some emit both.
- Round 82 decoder: range-check `bytes_per_line` against the visible
  width × `bits_per_pixel` and reject under-set values up front
  (previously the decoder would silently mis-frame planar→packed
  reconstruction on malformed inputs).
- Round 82 encoder: `encode_pcx_8bpp_grayscale(w, h, &pixels)` —
  8 bpp × 1 plane PCX 5.0 with `palette_info = 2` set and no
  tail palette appended.
- Round 82 encoder: `encode_pcx_24bpp_window(x_min, y_min, w, h,
  &rgb)` — 8 bpp × 3 planes PCX with a non-zero `(x_min, y_min)`
  window origin for the PCX 3.0+ pixel-region edge case.
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

## [0.0.2](https://github.com/OxideAV/oxideav-pcx/compare/v0.0.1...v0.0.2) - 2026-05-05

### Other

- clippy 1.95 — drop identity_op + erasing_op from row-index arithmetic

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
