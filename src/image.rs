//! Standalone image container returned by `oxideav-pcx`'s framework-free
//! decode API and accepted by the standalone encode API.
//!
//! Defined here (rather than reusing `oxideav_core::VideoFrame`) so the
//! crate can be built with the default `registry` feature off — i.e.
//! without depending on `oxideav-core` at all. When the `registry`
//! feature is on the [`crate::registry`] module provides the matching
//! [`PcxPixelFormat`] ↔ `oxideav_core::PixelFormat` mapping so the
//! trait-side `Decoder` / `Encoder` impls keep working unchanged.

/// Pixel layout used by [`PcxImage`].
///
/// The decoder always normalises monochrome (1 bpp × 1 plane) and
/// EGA-palette (1 bpp × 4 planes) and 8-bpp-indexed (8 bpp × 1 plane)
/// inputs to packed [`PcxPixelFormat::Rgba`], with palette lookup +
/// 1-bit expansion done at decode time. 24-bit (8 bpp × 3 planes)
/// inputs decode to packed [`PcxPixelFormat::Rgba`] with α = 0xFF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcxPixelFormat {
    /// 8-bit packed RGBA, 4 bytes per pixel.
    Rgba,
    /// 8-bit packed RGB, 3 bytes per pixel (encode input only).
    Rgb24,
    /// 8-bit single-channel indexed (encode input only — pairs with
    /// the 256-colour palette passed to [`crate::encode_pcx_8bpp_indexed`]).
    Indexed8,
}

/// One decoded PCX frame, framework-free shape.
///
/// `pts` is `None` for the standalone [`crate::parse_pcx`] entry point.
/// The registry-backed `Decoder` impl still passes `pts` through from
/// the surrounding `Packet`.
#[derive(Debug, Clone)]
pub struct PcxImage {
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
    /// Pixel layout the `data` carries. Decode always produces
    /// [`PcxPixelFormat::Rgba`].
    pub pixel_format: PcxPixelFormat,
    /// Row-major pixel bytes, `width × bytes_per_pixel(pixel_format) ×
    /// height` long. Top-left origin.
    pub data: Vec<u8>,
    /// Optional presentation timestamp. Always `None` from the
    /// standalone decode path.
    pub pts: Option<i64>,
    /// Source authoring resolution as `(h_dpi, v_dpi)` if the header
    /// carried non-zero values for both fields. Spec §3 records this as
    /// "the resolutions at which the image was created (printer or
    /// scanner); e.g. a scan might store 300, 300."
    ///
    /// The decoder reports `Some((h, v))` whenever both header fields
    /// are non-zero, and `None` otherwise (a 0 in either field per the
    /// rev-5 manual means "unset" — many drawing-program writers leave
    /// the field at zero rather than the 72×72 convention some scanner
    /// software emits). The standalone re-encode helpers
    /// [`crate::encode_pcx_24bpp_dpi`] /
    /// [`crate::encode_pcx_8bpp_indexed_dpi`] /
    /// [`crate::encode_pcx_8bpp_grayscale_dpi`] /
    /// [`crate::encode_pcx_1bpp_mono_dpi`] consume the same tuple so a
    /// caller can round-trip the scanner DPI through decode + re-encode
    /// without losing the metadata.
    pub dpi: Option<(u16, u16)>,
    /// Header `(x_min, y_min)` window origin from spec §3. PCX 3.0+
    /// supports a non-zero origin to record the source crop region the
    /// pixel buffer came from (per spec §3 the visible width / height
    /// are `x_max - x_min + 1` and `y_max - y_min + 1`).
    ///
    /// The decoder reports `Some((x, y))` whenever either component is
    /// non-zero, and `None` when the header carries `(0, 0)` — the
    /// overwhelmingly common case for screen-authored PCX files. The
    /// re-encode wrapper [`crate::encode_pcx_24bpp_image`] threads a
    /// `Some(...)` value through [`crate::encode_pcx_24bpp_window`] so
    /// a windowed PCX round-trips its crop origin end-to-end instead of
    /// having it silently zeroed.
    pub window_origin: Option<(u16, u16)>,
    /// Header `(h_screen_size, v_screen_size)` words from spec §3
    /// (offsets 70 / 72). The rev-5 manual records these as "Horizontal
    /// screen size in pixels (new field found only in PB IV / IV Plus)"
    /// and "Vertical screen size in pixels (new field found only in PB
    /// IV / IV Plus)" — a hint about the display resolution at the time
    /// the image was authored, distinct from the printer/scanner DPI in
    /// `h_dpi` / `v_dpi`.
    ///
    /// The decoder reports `Some((h, v))` whenever both components are
    /// non-zero, and `None` otherwise (an in-the-wild zero in either
    /// component means the field was left at the default by an older
    /// PCX writer that pre-dates PB IV — many of which keep the bytes
    /// at the historical zero fill). The re-encode wrapper
    /// [`crate::encode_pcx_24bpp_image`] threads a `Some(...)` value
    /// into the header so a tagged PCX round-trips its authoring screen
    /// size end-to-end instead of having it silently zeroed.
    pub screen_size: Option<(u16, u16)>,
}

impl PcxImage {
    /// Bytes-per-pixel implied by `pixel_format`.
    pub fn bytes_per_pixel(&self) -> usize {
        match self.pixel_format {
            PcxPixelFormat::Rgba => 4,
            PcxPixelFormat::Rgb24 => 3,
            PcxPixelFormat::Indexed8 => 1,
        }
    }

    /// Bytes per row.
    pub fn stride(&self) -> usize {
        self.width as usize * self.bytes_per_pixel()
    }
}

/// Origin of the 256-entry palette resolved by
/// [`crate::parse_pcx_indexed_8bpp`] for an 8 bpp × 1 plane PCX.
///
/// Surfaces which spec §3 / §4.1 branch the decoder took to fill the
/// `palette` field, so a consumer that re-encodes via
/// [`crate::encode_pcx_8bpp_indexed`] (VGA tail) versus
/// [`crate::encode_pcx_8bpp_grayscale`] (`palette_info = 2`) can pick
/// the matching writer rather than guessing from the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcxPaletteSource {
    /// Header `palette_info` field carried the value `2` (spec §3
    /// grayscale flag). The palette is the synthetic `0..=255`
    /// grayscale ramp — the spec §3 rule forces this interpretation
    /// regardless of whether the file also carries a VGA tail block.
    GrayscaleFlag,
    /// Optional 256-colour VGA palette block was present at the end of
    /// the file (spec §3: 769 bytes from EOF starts with the `0x0C`
    /// marker, followed by 768 RGB bytes).
    VgaTail,
    /// Neither `palette_info = 2` nor a VGA tail block was present. The
    /// decoder fills the palette with the synthetic `0..=255` grayscale
    /// ramp as a deterministic fallback for files that omit colour
    /// information entirely.
    GrayscaleFallback,
}

/// Typed 8 bpp × 1 plane paletted view returned by
/// [`crate::parse_pcx_indexed_8bpp`].
///
/// The standard [`crate::parse_pcx`] entry point always materialises an
/// `Rgba` buffer by walking the palette per pixel and dropping the
/// on-disk indices. For consumers that need the *indices themselves* —
/// to re-encode without re-quantising, to apply a palette swap, or to
/// hand the data to an indexed-image pipeline — this typed accessor
/// returns the raw 8-bit index buffer alongside the resolved palette
/// and a [`PcxPaletteSource`] tag so the caller knows which spec §3
/// branch produced the palette.
#[derive(Debug, Clone)]
pub struct PcxIndexed8 {
    /// Picture width in pixels (derived from spec §3 `x_max - x_min +
    /// 1`, matching [`PcxImage::width`]).
    pub width: u32,
    /// Picture height in pixels (derived from spec §3 `y_max - y_min +
    /// 1`, matching [`PcxImage::height`]).
    pub height: u32,
    /// `width × height` palette indices, row-major top-down. Padding
    /// bytes that the encoder added to round `bytes_per_line` up to an
    /// even number per spec §1 are NOT included; only the visible
    /// pixels of each scanline are surfaced.
    pub indices: Vec<u8>,
    /// 256-entry RGB palette. The source (VGA tail / grayscale flag /
    /// fallback) is recorded in [`Self::palette_source`].
    pub palette: [[u8; 3]; 256],
    /// Origin of the [`Self::palette`] entries — useful when picking
    /// the matching writer for a round-trip re-encode.
    pub palette_source: PcxPaletteSource,
}

impl PcxIndexed8 {
    /// Bytes per row (= `width`, one byte per pixel).
    pub fn stride(&self) -> usize {
        self.width as usize
    }
}

/// Origin of the 16-entry palette resolved by
/// [`crate::parse_pcx_indexed_4bpp`] for a 4 bpp × 1 plane PCX (the
/// 16-colour packed-bits / EGA mode listed in EGFF table line 442 as
/// "4 bpp / 1 plane / 16 colours / EGA and VGA").
///
/// The 48-byte header `ega_palette` field carries the on-disk palette.
/// Per spec §3 the rev-5 manual notes that PCX 3.0+ writers commonly
/// leave the field at all-zeros even for EGA-paletted data; in that
/// case the decoder substitutes the standard 16-entry EGA hardware
/// palette listed in spec table §3.1. The tag below records which of
/// the two branches the decoder took so a re-encode caller can decide
/// whether to round-trip the header palette unchanged or rewrite it
/// against the canonical hardware palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pcx4bppPaletteSource {
    /// The 48-byte header `ega_palette` field carried at least one
    /// non-zero byte. The 16-entry palette surfaced on
    /// [`PcxIndexed4::palette`] is read straight from those 48 bytes
    /// (one RGB triplet per entry, in the on-disk order).
    Ega16InHeader,
    /// The header `ega_palette` field was all-zeros. Per the rev-5
    /// manual the decoder substitutes the standard 16-entry EGA
    /// hardware palette from spec table §3.1, which is what
    /// [`PcxIndexed4::palette`] surfaces.
    Ega16Default,
}

/// Typed 4 bpp × 1 plane paletted view returned by
/// [`crate::parse_pcx_indexed_4bpp`].
///
/// Mirrors [`PcxIndexed8`] for the 16-colour packed-bits mode (EGFF
/// table entry "4, 1, 16, EGA and VGA"). The standard
/// [`crate::parse_pcx`] entry point always materialises an `Rgba`
/// buffer by walking the palette per pixel and dropping the on-disk
/// nibble indices. This typed accessor preserves them: the returned
/// [`PcxIndexed4`] carries one byte per pixel (the low-nibble palette
/// index in `0..=15`, top-down, padding stripped) alongside the
/// resolved 16-entry RGB palette and a [`Pcx4bppPaletteSource`] tag
/// recording which spec §3 branch produced the palette.
///
/// Useful for round-tripping a 16-colour PCX through
/// [`crate::encode_pcx_4bpp_packed`] without re-quantising, or for
/// applying palette-swap operations on the indices directly.
#[derive(Debug, Clone)]
pub struct PcxIndexed4 {
    /// Picture width in pixels (derived from spec §3 `x_max - x_min +
    /// 1`, matching [`PcxImage::width`]).
    pub width: u32,
    /// Picture height in pixels (derived from spec §3 `y_max - y_min +
    /// 1`, matching [`PcxImage::height`]).
    pub height: u32,
    /// `width × height` palette indices, row-major top-down, one byte
    /// per pixel with the index in the low nibble (`0..=15`). The
    /// 4-bpp on-disk format packs two pixels per byte (high nibble =
    /// even-x pixel, low nibble = odd-x pixel); this accessor unpacks
    /// them to one byte per pixel. Per-row padding that the encoder
    /// added to round `bytes_per_line` up to an even number per spec
    /// §1 is NOT included.
    pub indices: Vec<u8>,
    /// 16-entry RGB palette. The source (header `ega_palette` field
    /// vs. the spec table §3.1 default) is recorded in
    /// [`Self::palette_source`].
    pub palette: [[u8; 3]; 16],
    /// Origin of the [`Self::palette`] entries.
    pub palette_source: Pcx4bppPaletteSource,
}

impl PcxIndexed4 {
    /// Bytes per row (= `width`, one byte per pixel after unpacking).
    pub fn stride(&self) -> usize {
        self.width as usize
    }
}

/// Origin of the 16-entry palette resolved by
/// [`crate::parse_pcx_indexed_1bpp_4planes`] for a 1 bpp × 4 planes PCX
/// (the 16-colour EGA bit-plane mode described in spec §4.1 — each
/// scanline carries four 1-bit planes whose stacked bits form a 4-bit
/// palette index per pixel).
///
/// Same palette geometry as [`Pcx4bppPaletteSource`] — both modes draw
/// from the same 16-entry RGB table — but the on-disk plane shape is
/// different, so the typed accessors are kept separate. The 48-byte
/// header `ega_palette` field is the source of record; when it is
/// all-zeros (which PCX 3.0+ writers commonly emit even for EGA data)
/// the decoder substitutes the standard 16-entry EGA hardware palette
/// from spec table §3.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pcx1bpp4PlanesPaletteSource {
    /// The 48-byte header `ega_palette` field carried at least one
    /// non-zero byte. The 16-entry palette surfaced on
    /// [`PcxIndexed1x4`] `palette` is read straight from those 48
    /// bytes (one RGB triplet per entry, in the on-disk order).
    Ega16InHeader,
    /// The header `ega_palette` field was all-zeros. Per the rev-5
    /// manual the decoder substitutes the standard 16-entry EGA
    /// hardware palette from spec table §3.1, which is what
    /// [`PcxIndexed1x4`] `palette` surfaces.
    Ega16Default,
}

/// Typed 1 bpp × 4 planes paletted view returned by
/// [`crate::parse_pcx_indexed_1bpp_4planes`].
///
/// Spec §4.1 describes the 16-colour EGA bit-plane mode where each
/// scanline carries four 1-bit planes laid out one after another within
/// the row (plane 0, plane 1, plane 2, plane 3). The four bits at the
/// same x-position across the four planes stack into a 4-bit palette
/// index (`plane0 | plane1 << 1 | plane2 << 2 | plane3 << 3`).
///
/// The standard [`crate::parse_pcx`] entry point always materialises an
/// `Rgba` buffer by walking the palette per pixel and dropping the
/// per-plane bits. This typed accessor preserves the resolved index:
/// the returned [`PcxIndexed1x4`] carries one byte per pixel (low
/// nibble = palette index `0..=15`, top-down, padding stripped)
/// alongside the resolved 16-entry RGB palette and a
/// [`Pcx1bpp4PlanesPaletteSource`] tag recording which spec §3 branch
/// produced the palette.
///
/// Useful for round-tripping a 16-colour EGA PCX through
/// [`crate::encode_pcx_1bpp_4planes_ega`] without re-quantising, or
/// for applying palette-swap operations on the indices directly. The
/// nibble values share the [`PcxIndexed4`] convention so a caller can
/// hand either typed view to a 16-colour pipeline without branching on
/// the on-disk depth.
#[derive(Debug, Clone)]
pub struct PcxIndexed1x4 {
    /// Picture width in pixels (derived from spec §3 `x_max - x_min +
    /// 1`, matching [`PcxImage::width`]).
    pub width: u32,
    /// Picture height in pixels (derived from spec §3 `y_max - y_min +
    /// 1`, matching [`PcxImage::height`]).
    pub height: u32,
    /// `width × height` palette indices, row-major top-down, one byte
    /// per pixel with the index in the low nibble (`0..=15`). The
    /// 1 bpp × 4 planes on-disk format stacks the same x-position bit
    /// from each of the four planes into a 4-bit value; this accessor
    /// pre-resolves that stacking so the caller receives one index per
    /// pixel. Per-row padding bits beyond `width` (spec §1 rounds
    /// `bytes_per_line` up to an even number) are NOT included.
    pub indices: Vec<u8>,
    /// 16-entry RGB palette. The source (header `ega_palette` field
    /// vs. the spec table §3.1 default) is recorded in
    /// [`Self::palette_source`].
    pub palette: [[u8; 3]; 16],
    /// Origin of the [`Self::palette`] entries.
    pub palette_source: Pcx1bpp4PlanesPaletteSource,
}

impl PcxIndexed1x4 {
    /// Bytes per row (= `width`, one byte per pixel after unpacking).
    pub fn stride(&self) -> usize {
        self.width as usize
    }
}
