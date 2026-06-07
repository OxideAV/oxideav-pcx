# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
