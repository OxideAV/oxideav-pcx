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
| `encode_dcx_4_pages_320x240`          | 11 µs     | 60.6 GiB/s   |

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
| 2    | **`rle::encode` (run-finder + per-byte emit)** | `encode_1bpp_4planes_ega` (110 MiB/s) and `encode_1bpp_mono` (314 MiB/s) are 6–7× slower than 24bpp; both are bit-scatter + 4×/1× RLE passes over the widened planar stride. | dominates 1bpp encode |
| 3    | **EGA 4-plane bit-scatter** (`encode_1bpp_4planes_ega` inner loop) | Per pixel does 4 indexed `row[plane·bpl + x/8] \|= 1 << (7−x%8)` scatter stores → 4 RLE passes over a 4× buffer. Worst encoder by a wide margin (110 MiB/s vs 715 MiB/s 24bpp). | ~7× the per-byte cost |
| 4    | per-plane assembly (`unpack_*`)            | full − RLE: ~38 µs (24bpp 640×480), ~55 µs (8bpp gray 512²). Already fast post-r209; **not** the next target.            | ~5% of decode         |
| 5    | DCX offset-table walk / assembler          | `encode_dcx` 60 GiB/s (memcpy-bound concat), `parse_dcx` ≈ 4× `parse_pcx` (page-parse-bound, no per-bundle overhead).     | negligible            |

### Next PROFILE-OPT target

**`rle::decode` — the per-scanline run-length decode loop in
`src/rle.rs`.** It is the #1 measured cost of every decode scenario
(~95% of 24bpp decode time) and the optimiser cannot vectorise its
current shape because the inner run-fill is a scalar `for _ in
0..count { out.push(lit) }` against a `Vec` that may reallocate, and
the literal path is a single `out.push(b)` per byte. Two concrete
levers for the next round to A/B against this r286 baseline:

* **Run-fill via `resize` / `extend(iter::repeat)` instead of a
  push loop** — let the allocator's `memset` fast path fill runs, and
  pre-`reserve` the planar `Vec` to its exact `total_planar` size
  (already computed in `decode_planar_scanlines`) so no run-fill ever
  triggers a realloc + bounds re-check.
* **Bulk literal copy** — when a literal byte is followed by more
  non-header bytes, copy the literal span with `extend_from_slice`
  rather than one `push` per byte.

A close secondary target is **`encode_1bpp_4planes_ega`** (#3): replace
the per-pixel 4-plane scatter with a per-byte (8-pixel) shuffle that
builds all four plane bytes in registers before the indexed store,
mirroring the gain the r209 decode-side row-slice rewrite captured.
Both targets are pure compute on the RLE / bit-shuffle inner loops and
need no spec changes.

## Reproducing

```sh
CARGO_TARGET_DIR=/tmp/oxideav-pcx-target \
  cargo bench -p oxideav-pcx --bench decode
CARGO_TARGET_DIR=/tmp/oxideav-pcx-target \
  cargo bench -p oxideav-pcx --bench encode
CARGO_TARGET_DIR=/tmp/oxideav-pcx-target \
  cargo bench -p oxideav-pcx --bench roundtrip
```

Append `-- --quick` for a fast smoke run, or `-- --save-baseline r286`
to capture a named baseline a later round can `--baseline r286`
against.
