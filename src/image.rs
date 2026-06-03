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
