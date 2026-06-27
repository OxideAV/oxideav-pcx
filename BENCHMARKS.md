# oxideav-pcx benchmark suite

This crate ships three Criterion bench harnesses driven by
`cargo bench -p oxideav-pcx`:

| Harness     | Scope                                                                                          | Run                                            |
| ----------- | ---------------------------------------------------------------------------------------------- | ---------------------------------------------- |
| `decode`    | Full PCX decoder hot paths (RLE byte-stream + per-plane assembly) across every (depth, planes) layout + the DCX multi-page path, plus two **phase-split** probes that time the RLE-decode phase in isolation. | `cargo bench -p oxideav-pcx --bench decode`    |
| `encode`    | All 8 encoder write paths (24bpp / 8bpp-indexed / 8bpp-grayscale / 1bpp-mono / 4bpp-packed / 2bpp-CGA / 1bpp×4-planes-EGA) + the DCX bundle assembler. | `cargo bench -p oxideav-pcx --bench encode`    |
| `roundtrip` | Build → encode → decode end-to-end loops for each (depth, planes) tuple + DCX.                  | `cargo bench -p oxideav-pcx --bench roundtrip` |

Each harness is self-contained: every scenario synthesises its pixel
data with a deterministic `xorshift32` generator inside the harness, so
no fixture files are committed and the run is reproducible across
machines. The xorshift mix is deliberate — it keeps the RLE byte stream
from collapsing into one giant run, so the decode side exercises the
literal + run mix the spec §3.2 codec sees on real images.

## The two decode phases

`parse_pcx` is two sequential phases:

1. **RLE-decode** — header validation + the spec §3.2 run-length
   byte-stream decode (`rle::decode`, called per scanline by
   `decode_planar_scanlines`) into the planar scanline buffer
   (`n_planes × bytes_per_line × height` bytes).
2. **Per-plane assembly** — repack the planar buffer into packed RGBA
   per (depth, planes) tuple (the `unpack_*` family; the r209 row-slice
   rewrite already lifted this phase to 6.5–7.2 GiB/s).

The `decode_phase_rle_*` benches call `__bench_decode_planar_len`, a
`#[doc(hidden)]` probe that runs *only* phase (1) — the exact
`decode_planar_scanlines` the production decoder calls, no parallel
code path. Timing it next to the full `parse_pcx` on the same input
attributes the cost to each phase: **assembly cost ≈ (full parse − RLE
phase)**. This makes the hotspot ranking below a measured split rather
than an inference. The probe returns only a byte count (not the buffer
or the private `PcxHeader`), so the crate's public type surface is
unchanged; `src/` decode/encode bytes are byte-identical to the
pre-r286 tree.

## Round 311 decode baseline (Apple M-series, `--measurement-time 3.0`)

Median throughput / time reported by Criterion **after the r311
`rle::decode` run-fill optimisation** (see "Round 311" below). Single
dev-machine numbers — use them as a regression guard, not cross-platform
specs. Decode/roundtrip `Throughput::Bytes` counts output RGBA bytes
(`w·h·4`); the `decode_phase_rle_*` probes count the planar-buffer bytes
produced. The `Δ vs r286` column is the wall-clock change against the
r286 row beside it (run-to-run variance ≈ ±3% for the sub-100 µs rows).

### Decode

| Scenario                              | Time (r311) | Throughput (r311) | Δ vs r286 |
| ------------------------------------- | ----------- | ----------------- | --------- |
| `decode_24bpp_1920x1080`              | 4.11 ms     | 1.88 GiB/s        | −13.5%    |
| `decode_24bpp_640x480`                | 627 µs      | 1.83 GiB/s        | −11.8%    |
| `decode_24bpp_320x240`                | 135 µs      | 2.11 GiB/s        | −25.5%    |
| `decode_8bpp_indexed_320x240`         | 79 µs       | 3.62 GiB/s        | −12.6%    |
| `decode_8bpp_grayscale_512x512`       | 317 µs      | 3.08 GiB/s        | neutral   |
| `decode_1bpp_mono_512x512`            | 173 µs      | 5.65 GiB/s        | neutral   |
| `decode_4bpp_packed_320x240`          | 69 µs       | 4.13 GiB/s        | −5.2%     |
| `decode_2bpp_cga_320x240`             | 46 µs       | 6.22 GiB/s        | neutral   |
| `decode_1bpp_4planes_ega_320x240`     | 119 µs      | 2.41 GiB/s        | −2.6%     |
| `decode_dcx_4_pages_320x240`          | 534 µs      | 2.14 GiB/s        | −20.5%    |
| **phase-split:** `decode_phase_rle_24bpp_640x480`         | 595 µs | 1.44 GiB/s (planar) | −12.9% |
| **phase-split:** `decode_phase_rle_8bpp_grayscale_512x512`| 263 µs | 950 MiB/s (planar)  | −7.1%  |

## Round 286 baseline (Apple M-series, `--measurement-time 2.0`)

Median throughput / time reported by Criterion. Single dev-machine
numbers — use them as a regression guard, not cross-platform specs.
Decode/roundtrip `Throughput::Bytes` counts output RGBA bytes
(`w·h·4`); encode counts input pixel bytes; the `decode_phase_rle_*`
probes count the planar-buffer bytes produced.

### Decode

| Scenario                              | Time      | Throughput   |
| ------------------------------------- | --------- | ------------ |
| `decode_24bpp_1920x1080`              | 4.94 ms   | 1.56 GiB/s   |
| `decode_24bpp_640x480`                | 742 µs    | 1.54 GiB/s   |
| `decode_24bpp_320x240`                | 189 µs    | 1.52 GiB/s   |
| `decode_8bpp_indexed_320x240`         | 96 µs     | 2.97 GiB/s   |
| `decode_8bpp_grayscale_512x512`       | 366 µs    | 2.67 GiB/s   |
| `decode_1bpp_mono_512x512`            | 176 µs    | 5.56 GiB/s   |
| `decode_4bpp_packed_320x240`          | 73 µs     | 3.91 GiB/s   |
| `decode_2bpp_cga_320x240`             | 48 µs     | 5.96 GiB/s   |
| `decode_1bpp_4planes_ega_320x240`     | 120 µs    | 2.38 GiB/s   |
| `decode_dcx_4_pages_320x240`          | 762 µs    | 1.50 GiB/s   |
| **phase-split:** `decode_phase_rle_24bpp_640x480`         | 704 µs | 1.22 GiB/s (planar) |
| **phase-split:** `decode_phase_rle_8bpp_grayscale_512x512`| 311 µs | 804 MiB/s (planar)  |

### Encode

| Scenario                              | Time      | Throughput   |
| ------------------------------------- | --------- | ------------ |
| `encode_24bpp_1920x1080`              | 7.95 ms   | 746 MiB/s    |
| `encode_24bpp_640x480`                | 1.23 ms   | 715 MiB/s    |
| `encode_24bpp_320x240`                | 322 µs    | 682 MiB/s    |
| `encode_8bpp_indexed_320x240`         | 92 µs     | 795 MiB/s    |
| `encode_8bpp_grayscale_512x512`       | 358 µs    | 698 MiB/s    |
| `encode_1bpp_mono_512x512`            | 797 µs    | 314 MiB/s    |
| `encode_4bpp_packed_320x240`          | 93 µs     | 787 MiB/s    |
| `encode_2bpp_cga_320x240`             | 118 µs    | 621 MiB/s    |
| `encode_1bpp_4planes_ega_320x240`     | 669 µs    | 110 MiB/s    |
| `encode_rgb_auto_lowcolor_640x480`    | ~6.0 ms   | ~146 MiB/s   |
| `encode_rgb_auto_truecolor_640x480`   | (scan-bound) | —         |
| `encode_dcx_4_pages_320x240`          | 11 µs     | 60.6 GiB/s   |

> The two `encode_rgb_auto_*` rows measure `encode_pcx_rgb_auto`, whose
> cost is dominated by the per-pixel distinct-colour scan (a linear probe
> over a ≤256-entry first-seen palette), not the RLE encode the other
> rows isolate. The *lowcolor* case saturates the palette and runs the
> scan over every pixel plus both candidate encodes + the size compare;
> the *truecolor* case bails out of the scan as soon as a 257th colour
> appears and then does a single planar encode, so its cost is the
> early-exit scan plus one `encode_24bpp`. These rows exist to track the
> scan's cost as a regression guard, not as a throughput headline — a
> caller that already knows its colour count should call the specific
> writer directly.

### Roundtrip (encode → decode)

| Scenario                              | Time      | Throughput   |
| ------------------------------------- | --------- | ------------ |
| `roundtrip_24bpp_640x480`             | 2.04 ms   | 574 MiB/s    |
| `roundtrip_24bpp_320x240`             | 526 µs    | 557 MiB/s    |
| `roundtrip_8bpp_indexed_320x240`      | 186 µs    | 1.54 GiB/s   |
| `roundtrip_8bpp_grayscale_512x512`    | 728 µs    | 1.34 GiB/s   |
| `roundtrip_1bpp_mono_512x512`         | 1.01 ms   | 991 MiB/s    |
| `roundtrip_4bpp_packed_320x240`       | 170 µs    | 1.69 GiB/s   |
| `roundtrip_2bpp_cga_320x240`          | 175 µs    | 1.63 GiB/s   |
| `roundtrip_1bpp_4planes_ega_320x240`  | 856 µs    | 342 MiB/s    |
| `roundtrip_dcx_4_pages_320x240`       | 710 µs    | 1.61 GiB/s   |

## Ranked hotspot table — where the time goes

Ranked by attributable cost, normalised so cross-layout comparison is
meaningful. The decode phase split is the headline finding: across the
24bpp and 8bpp-grayscale extremes, **the RLE byte-stream codec
(`rle::decode` / `rle::encode`) is the dominant cost, and per-plane
assembly is already cheap** (the r209 row-slice rewrite did its job).

| Rank | Hot path                                   | Evidence                                                                                                                   | Share of its scenario |
| ---- | ------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------- | --------------------- |
| 1    | **`rle::decode` (per-byte `out.push` loop)** | `decode_phase_rle_24bpp_640x480` = 704 µs of the 742 µs full `parse_pcx`. Assembly is only ~38 µs (~5%).                  | ~95% of 24bpp decode  |
| 2    | **`rle::encode` (run-finder + per-byte emit)** | After the r362 bit-pack landing the 1-bpp paths are no longer scatter-bound; `rle::encode`'s run-finder + per-byte emit is now the residual 1-bpp encode cost. | residual 1bpp encode  |
| 3    | ~~**EGA 4-plane bit-scatter**~~ **(LANDED r362)** | Was the worst encoder (110 MiB/s): per pixel did 4 indexed `row[plane·bpl + x/8] \|= 1 << (7−x%8)` scatter stores. r362 replaced it with the 8-pixel whole-byte packer → **629 MiB/s (5.6×)**; `encode_1bpp_mono` 321 → **2.94 GiB/s (9.3×)**. See "Round 362" below. | resolved |
| 4    | per-plane assembly (`unpack_*`)            | full − RLE: ~38 µs (24bpp 640×480), ~55 µs (8bpp gray 512²). Already fast post-r209; **not** the next target.            | ~5% of decode         |
| 5    | DCX offset-table walk / assembler          | `encode_dcx` 60 GiB/s (memcpy-bound concat), `parse_dcx` ≈ 4× `parse_pcx` (page-parse-bound, no per-bundle overhead).     | negligible            |

### Round 311 — `rle::decode` run-fill (LANDED)

The r286 #1 target landed in r311. The per-scanline run-fill now grows
the planar `Vec` in one `Vec::resize` for runs of `count > 2` (the
allocator's `memset` fast path), while runs of `count <= 2` keep the
cheaper `push`. The caller already pre-`reserve`s the planar `Vec` to
its exact `total_planar` size, so a resize never reallocates. The
length threshold matters: high-entropy bit-packed planar layouts
(`mono` / `2bpp` / `4-plane EGA`) are dominated by very short runs +
singleton literals, where the resize bookkeeping doesn't amortise and
an unconditional resize *regressed* those rows ~5–10%; the threshold
keeps them neutral while the run-heavy 24bpp / DCX / 8bpp paths take
the memset path (24bpp 320×240 −25.5%, DCX −20.5%, 8bpp-indexed
−12.6%, phase-split RLE 24bpp −12.9%). The literal path was left as a
per-byte `push` — a maximal-span `extend_from_slice` copy was measured
slower because the per-span scan loop costs more than it saves on the
singleton-heavy literal streams the low-bpp modes produce. Output bytes
are bit-identical (roundtrip / cross_validate green).

### Round 362 — 1-bpp-per-plane bit-pack (LANDED)

The r286 #2/#3 encode target landed in r362. All four 1-bit-per-plane
encode paths (`encode_pcx_1bpp_mono`, `encode_pcx_1bpp_2planes_cga`,
`encode_pcx_1bpp_3planes_ega_rgb`, `encode_pcx_1bpp_4planes_ega`) shared
a per-pixel scatter inner loop: one branch-guarded indexed
read-modify-write `row[plane·bpl + x/8] |= 1 << (7 − x%8)` store per set
bit. They now route through a single `pack_1bpp_plane_row` helper that
folds eight consecutive pixels into one accumulator (shift-OR) and
writes each output byte once — no per-pixel array index, no per-pixel
branch into `dst`, no read-modify-write. Measured (this machine):

| Bench                              | Before    | After      | Speedup |
| ---------------------------------- | --------- | ---------- | ------- |
| `encode_1bpp_mono_512x512`         | 321 MiB/s | 2.94 GiB/s | ~9.3×   |
| `encode_1bpp_4planes_ega_320x240`  | 113 MiB/s | 629 MiB/s  | ~5.6×   |

The output is **byte-identical** to the scatter form — bit `7 − k` of
output byte `b` holds pixel `8·b + k`, the sub-8-pixel scanline tail
contributes the same trailing zeros, and the even-stride padding byte
stays at its zeroed value. `tests/round362_bitpack.rs` pins this across
every `width % 8` residue + a whole-file byte-exact comparison against an
independent scatter reference; the full round-trip sweeps stay green.

### Next PROFILE-OPT target

With both `rle::decode` (r311) and the 1-bpp bit-scatter (r362)
optimised, the residual encode cost is **`rle::encode`'s run-finder +
per-byte emit** (now the dominant component of the 1-bpp paths, since the
bit-pack is no longer the bottleneck). It is pure compute on the RLE
inner loop and needs no spec changes.

## Reproducing

```sh
CARGO_TARGET_DIR=/tmp/oxideav-pcx-target \
  cargo bench -p oxideav-pcx --bench decode
CARGO_TARGET_DIR=/tmp/oxideav-pcx-target \
  cargo bench -p oxideav-pcx --bench encode
CARGO_TARGET_DIR=/tmp/oxideav-pcx-target \
  cargo bench -p oxideav-pcx --bench roundtrip
```

Append `-- --quick` for a fast smoke run, or
`-- --save-baseline r311` to capture a named baseline a later round can
`--baseline r311` against.
