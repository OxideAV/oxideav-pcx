//! Decode-side limits ([`DecodeOptions`]) and encode-side behaviour
//! ([`EncodeOptions`]) for the `IMAGE_CRATE_API` root functions.

use crate::error::{PcxError, Result};
use crate::image::PcxLayout;

/// Limits and strictness for [`crate::decode_with`].
///
/// Every limit is checked against the 128-byte header **before** any
/// pixel buffer is allocated, so a hostile header fails with
/// [`PcxError::LimitExceeded`] instead of committing memory. The
/// defaults are: no dimension / pixel-count limit (PCX dimensions are
/// `u16`, so the geometry is bounded by the format itself), decoded
/// bytes capped at [`DecodeOptions::DEFAULT_MAX_BYTES`] (1 GiB),
/// `strict = false`.
///
/// `max_bytes` bounds both buffers a decode allocates: the RLE-expanded
/// planar scanline buffer (`n_planes × bytes_per_line × height`) and
/// the native output plane (`width × bytes_per_pixel × height`); each
/// must fit on its own.
///
/// `strict` governs the spec's *should* rules the lenient decoder
/// tolerates:
///
/// * always (both modes): manufacturer `0x0A`, a known version byte
///   (0 / 2 / 3 / 4 / 5), encoding `1`, a known `(bits_per_pixel,
///   n_planes)` geometry, non-zero dimensions with `x_max ≥ x_min`,
///   `bytes_per_line` large enough for the width, an RLE stream that
///   backs the claimed pixel count;
/// * `strict = false` (default): the RLE stream is consumed as one run
///   over the whole image (the manual's own decode fragment), so a run
///   packet straddling a scanline boundary is accepted; an odd
///   `bytes_per_line` ("MUST be EVEN", spec §3) and a non-zero reserved
///   byte 64 are ignored;
/// * `strict = true`: a run packet that crosses a scanline boundary, an
///   odd `bytes_per_line` and a non-zero reserved byte are each
///   [`PcxError::InvalidData`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// Reject images wider than this (pixels).
    pub max_width: Option<u32>,
    /// Reject images taller than this (pixels).
    pub max_height: Option<u32>,
    /// Reject images with more than this many pixels (`width ×
    /// height`).
    pub max_pixels: Option<u64>,
    /// Reject images whose planar scanline buffer or native output
    /// plane would exceed this many bytes.
    pub max_bytes: Option<u64>,
    /// Enforce the spec's *should* rules (see the type docs).
    pub strict: bool,
}

impl DecodeOptions {
    /// Default [`Self::max_bytes`]: 1 GiB of decoded bytes.
    pub const DEFAULT_MAX_BYTES: u64 = 1 << 30;

    /// The defaults (see the type docs).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set (or lift with `None`) the width limit.
    pub fn with_max_width(mut self, max_width: impl Into<Option<u32>>) -> Self {
        self.max_width = max_width.into();
        self
    }

    /// Set (or lift with `None`) the height limit.
    pub fn with_max_height(mut self, max_height: impl Into<Option<u32>>) -> Self {
        self.max_height = max_height.into();
        self
    }

    /// Set (or lift with `None`) the pixel-count limit.
    pub fn with_max_pixels(mut self, max_pixels: impl Into<Option<u64>>) -> Self {
        self.max_pixels = max_pixels.into();
        self
    }

    /// Set (or lift with `None`) the decoded-bytes limit.
    pub fn with_max_bytes(mut self, max_bytes: impl Into<Option<u64>>) -> Self {
        self.max_bytes = max_bytes.into();
        self
    }

    /// Set strict mode.
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Lift every limit (`max_*` all `None`).
    pub fn unlimited(mut self) -> Self {
        self.max_width = None;
        self.max_height = None;
        self.max_pixels = None;
        self.max_bytes = None;
        self
    }

    /// Check a header's geometry against the limits. `bytes` is the
    /// largest buffer the decode would allocate.
    pub(crate) fn check(&self, width: u32, height: u32, bytes: u64) -> Result<()> {
        if let Some(m) = self.max_width {
            if width > m {
                return Err(PcxError::limit(format!(
                    "PCX: width {width} exceeds max_width {m}"
                )));
            }
        }
        if let Some(m) = self.max_height {
            if height > m {
                return Err(PcxError::limit(format!(
                    "PCX: height {height} exceeds max_height {m}"
                )));
            }
        }
        let pixels = u64::from(width) * u64::from(height);
        if let Some(m) = self.max_pixels {
            if pixels > m {
                return Err(PcxError::limit(format!(
                    "PCX: {pixels} pixels exceed max_pixels {m}"
                )));
            }
        }
        if let Some(m) = self.max_bytes {
            if bytes > m {
                return Err(PcxError::limit(format!(
                    "PCX: decoded buffer of {bytes} bytes exceeds max_bytes {m}"
                )));
            }
        }
        Ok(())
    }
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            max_width: None,
            max_height: None,
            max_pixels: None,
            max_bytes: Some(Self::DEFAULT_MAX_BYTES),
            strict: false,
        }
    }
}

/// Behaviour of [`crate::encode`] / [`crate::encode_rgb8`] /
/// [`crate::encode_rgba8`] / [`crate::encode_to`].
///
/// One struct, every variant a field: the on-disk geometry
/// (`layout`), the compact ladder (`compact`), the header annotations
/// (`dpi`, `window_origin`, `screen_size`, `version`) and whether
/// alpha may be dropped (`drop_alpha`). PCX defines a single encoding
/// byte value (`1`, run-length; spec §3 and the EGFF cross-reference:
/// "the only valid value for the encoding field is 1"), so there is no
/// RLE toggle — every file is RLE-compressed.
///
/// How an image maps to the wire with the defaults (never a silent
/// conversion — see [`PcxLayout::natural_for`] for the palette rules):
///
/// | Layout | Palette | Geometry written |
/// |---|---|---|
/// | `Rgb24` | — | 8 bpp × 3 planes |
/// | `Gray8` | — | 8 bpp × 1 plane, `palette_info = 2`, grey-ramp tail (see `gray_tail`) |
/// | `Pal8` | `[black, white]` | 1 bpp × 1 plane |
/// | `Pal8` | ≤ 4 CGA colours | 2 bpp × 1 plane CGA |
/// | `Pal8` | the 8 primaries | 1 bpp × 3 planes |
/// | `Pal8` | ≤ 16 entries | 4 bpp × 1 plane, header colormap |
/// | `Pal8` | ≤ 256 entries | 8 bpp × 1 plane + VGA tail |
/// | `Rgba` | — | [`PcxError::Unsupported`] unless `drop_alpha` |
///
/// An image decoded by this crate carries its source geometry on
/// [`PcxImage::layout`](crate::PcxImage::layout) and is written back
/// in it, so `decode(encode(img)) == img` holds; `layout` here
/// overrides that.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncodeOptions {
    /// Force an on-disk geometry. `None` (default): the image's own
    /// [`layout`](crate::PcxImage::layout), else the natural geometry
    /// for its format and palette. An image that cannot be stored
    /// losslessly in the forced geometry is
    /// [`PcxError::Unsupported`].
    pub layout: Option<PcxLayout>,
    /// Compact mode: encode every geometry whose losslessness
    /// precondition holds for the pixels — monochrome, both CGA
    /// layouts, EGA RGB, both 16-colour layouts, grayscale, 256-colour
    /// indexed and 24-bit — and keep the fewest bytes (ties keep the
    /// earlier candidate in that order). The decoded layout then
    /// follows the chosen geometry rather than the input's, so this is
    /// opt-in. Ignored when `layout` is `Some`. Default `false`.
    pub compact: bool,
    /// Header `h_dpi` / `v_dpi` override. `None` (default) writes the
    /// image's [`dpi`](crate::PcxImage::dpi), or `0 / 0` ("unset", spec
    /// §3) when the image has none. `Some` must have both components
    /// non-zero ([`PcxError::InvalidData`] otherwise).
    pub dpi: Option<(u16, u16)>,
    /// Header `x_min` / `y_min` override. `None` (default) writes the
    /// image's [`window_origin`](crate::PcxImage::window_origin), or
    /// `0 / 0`. `x_min + width` and `y_min + height` must stay within
    /// `u16` ([`PcxError::InvalidData`] otherwise).
    pub window_origin: Option<(u16, u16)>,
    /// Header `h_screen_size` / `v_screen_size` override. `None`
    /// (default) writes the image's
    /// [`screen_size`](crate::PcxImage::screen_size), or `0 / 0`.
    /// `Some` must have both components non-zero.
    pub screen_size: Option<(u16, u16)>,
    /// Header version byte. Default `5` (PCX 3.0+, the only version
    /// defined for 24-bit data and the VGA tail palette). Any of the
    /// spec's values (0 / 2 / 3 / 4 / 5) is written as given; others
    /// are [`PcxError::InvalidData`].
    pub version: u8,
    /// Allow an `Rgba` image (or a palette with non-opaque entries) to
    /// be written with its alpha discarded. Default `false`: such input
    /// is [`PcxError::Unsupported`], PCX having no alpha mechanism.
    pub drop_alpha: bool,
    /// Append the 256-entry grey-ramp VGA palette block to a `Gray8`
    /// file (`palette_info = 2`). Default `true`: the spec flag alone
    /// is enough for this crate (the flag wins over the tail on
    /// decode, so the file reads back identically), but readers that
    /// require a VGA block on every 8 bpp × 1 plane file reject a
    /// tail-less one — the black-box validator does. `false` saves the
    /// fixed 769 bytes, which is what the pre-contract grayscale writer
    /// did.
    pub gray_tail: bool,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            layout: None,
            compact: false,
            dpi: None,
            window_origin: None,
            screen_size: None,
            version: 5,
            drop_alpha: false,
            gray_tail: true,
        }
    }
}

impl EncodeOptions {
    /// The defaults: the image's own geometry, no compact ladder,
    /// header annotations from the image, version 5, alpha refused.
    pub fn new() -> Self {
        Self::default()
    }

    /// Force (or stop forcing) an on-disk geometry.
    pub fn with_layout(mut self, layout: impl Into<Option<PcxLayout>>) -> Self {
        self.layout = layout.into();
        self
    }

    /// Set compact mode.
    pub fn with_compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    /// Set (or clear) the DPI override.
    pub fn with_dpi(mut self, dpi: impl Into<Option<(u16, u16)>>) -> Self {
        self.dpi = dpi.into();
        self
    }

    /// Set (or clear) the window-origin override.
    pub fn with_window_origin(mut self, origin: impl Into<Option<(u16, u16)>>) -> Self {
        self.window_origin = origin.into();
        self
    }

    /// Set (or clear) the screen-size override.
    pub fn with_screen_size(mut self, screen_size: impl Into<Option<(u16, u16)>>) -> Self {
        self.screen_size = screen_size.into();
        self
    }

    /// Set the header version byte.
    pub fn with_version(mut self, version: u8) -> Self {
        self.version = version;
        self
    }

    /// Allow (or refuse) dropping alpha.
    pub fn with_drop_alpha(mut self, drop_alpha: bool) -> Self {
        self.drop_alpha = drop_alpha;
        self
    }

    /// Append (or omit) the grey-ramp tail on `Gray8` files.
    pub fn with_gray_tail(mut self, gray_tail: bool) -> Self {
        self.gray_tail = gray_tail;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_fire_in_order() {
        let o = DecodeOptions::default()
            .with_max_width(10u32)
            .with_max_height(10u32)
            .with_max_pixels(50u64)
            .with_max_bytes(100u64);
        assert!(o.check(5, 5, 75).is_ok());
        assert!(matches!(o.check(11, 1, 1), Err(PcxError::LimitExceeded(_))));
        assert!(matches!(o.check(1, 11, 1), Err(PcxError::LimitExceeded(_))));
        assert!(matches!(o.check(8, 8, 1), Err(PcxError::LimitExceeded(_))));
        assert!(matches!(
            o.check(5, 5, 101),
            Err(PcxError::LimitExceeded(_))
        ));
        assert!(o.unlimited().check(u32::MAX, u32::MAX, u64::MAX).is_ok());
    }

    #[test]
    fn encode_defaults() {
        let o = EncodeOptions::default();
        assert_eq!(o.layout, None);
        assert!(!o.compact);
        assert_eq!(o.dpi, None);
        assert_eq!(o.version, 5);
        assert!(!o.drop_alpha);
        assert!(o.gray_tail);
    }
}
