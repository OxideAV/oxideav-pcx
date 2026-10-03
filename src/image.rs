//! The standalone image types: the shapes every `oxideav-<format>`
//! image crate shares (`IMAGE_CRATE_API`), specialised for PCX.
//!
//! * [`PcxImage`] — the native-layout image [`crate::decode`] returns
//!   and [`crate::encode`] consumes: dimensions, a [`PixelFormat`] tag,
//!   one packed [`Plane`], [`ColorInfo`], [`Metadata`], an optional
//!   [`Palette`] (every sub-24-bit PCX geometry is paletted) plus the
//!   PCX header extras — authoring DPI, window origin, PB IV screen
//!   size and the on-disk [`PcxLayout`] the pixels came from.
//! * [`RgbImage`] / [`RgbaImage`] — the tightly packed 8-bit raw paths
//!   ([`crate::decode_rgb8`] / [`crate::decode_rgba8`],
//!   [`PcxImage::to_rgb8`] / [`PcxImage::to_rgba8`]).
//! * [`ImageInfo`] — what [`crate::info`] reads from the 128-byte header
//!   (and the VGA tail marker) without touching a pixel.
//! * [`Frame`] — one page of a DCX bundle for [`crate::decode_all`].
//!
//! Defined here (rather than reusing `oxideav_core::VideoFrame`) so the
//! crate builds with the default `registry` feature off — i.e. without
//! depending on `oxideav-core` at all. With `registry` on,
//! `crate::registry` adds the `From<PcxImage> for VideoFrame`
//! conversion and its inverse so the framework `Decoder` / `Encoder`
//! are thin adapters over the same functions.
//!
//! The typed paletted views ([`PcxIndexed8`], [`PcxIndexed4`], …) that
//! the depth accessors `parse_pcx_indexed_*` return live here too; they
//! are the format-specific floor below the contract and keep their
//! names.

use std::time::Duration;

use crate::error::{PcxError, Result};

/// Pixel layouts the standalone `oxideav-pcx` API can produce / consume.
///
/// Variant names mirror `oxideav_core::PixelFormat` exactly, so the
/// `crate::registry` conversion layer is a 1:1 match-and-rebuild
/// rather than a re-pack. Every PCX layout is packed (one plane).
///
/// What [`crate::decode`] produces per spec geometry
/// (`bits_per_pixel × n_planes`):
///
/// | Geometry | Native layout | Palette |
/// |---|---|---|
/// | 1 × 1 (monochrome) | [`Pal8`](Self::Pal8) | 2 entries: header colormap triples 0 / 1, or black / white when the colormap is zero-filled |
/// | 1 × 2, 2 × 1 (CGA) | [`Pal8`](Self::Pal8) | 4 entries resolved from the header's CGA colour map (background nibble + C / P / I selector) |
/// | 1 × 3 (EGA RGB) | [`Pal8`](Self::Pal8) | 8 fixed on/off primaries, index `r \| g << 1 \| b << 2` |
/// | 1 × 4, 4 × 1 (16-colour) | [`Pal8`](Self::Pal8) | 16 entries: header colormap, or the EGA hardware default when zero-filled |
/// | 8 × 1 with a VGA tail (`palette_info ≠ 2`) | [`Pal8`](Self::Pal8) | 256 entries from the tail block |
/// | 8 × 1 with `palette_info = 2`, or no tail | [`Gray8`](Self::Gray8) | — (the pixel byte is the grey level) |
/// | 8 × 3 (24-bit) | [`Rgb24`](Self::Rgb24) | — |
///
/// [`Rgba`](Self::Rgba) is an **input** layout only (PCX has no alpha
/// mechanism): [`crate::encode`] rejects it with
/// [`PcxError::Unsupported`] unless
/// [`EncodeOptions::drop_alpha`](crate::EncodeOptions::drop_alpha) is
/// set, and [`crate::encode_rgba8`] documents that it drops alpha.
/// The deprecated `parse_pcx` flatten reader returns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PcxPixelFormat {
    /// 8-bit RGBA, 4 bytes per pixel. Input only; never decoded.
    Rgba,
    /// 8-bit RGB, 3 bytes per pixel (24-bit files).
    Rgb24,
    /// 8-bit single-channel grayscale, 1 byte per pixel (8 bpp × 1
    /// plane files flagged `palette_info = 2`, or with no VGA tail).
    Gray8,
    /// 8-bit palette index, 1 byte per pixel. The colour table lives on
    /// [`PcxImage::palette`]; every sub-24-bit geometry decodes to it.
    Pal8,
}

/// The contract name for [`PcxPixelFormat`].
pub type PixelFormat = PcxPixelFormat;

impl PcxPixelFormat {
    /// Bytes per pixel for the layout.
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba => 4,
            Self::Rgb24 => 3,
            Self::Gray8 | Self::Pal8 => 1,
        }
    }

    /// `true` when the layout carries an alpha channel of its own
    /// (`Rgba`).
    pub fn has_alpha(self) -> bool {
        matches!(self, Self::Rgba)
    }
}

/// The on-disk geometry of a PCX file — the `(bits_per_pixel,
/// n_planes)` pair plus the palette carrier the spec defines for it.
///
/// [`crate::decode`] records the geometry it read on
/// [`PcxImage::layout`] and [`ImageInfo::layout`]; [`crate::encode`]
/// writes the geometry [`EncodeOptions::layout`](crate::EncodeOptions::layout)
/// forces, else the image's own `layout`, else the natural geometry for
/// the image's [`PixelFormat`] and palette (see
/// [`PcxLayout::natural_for`]). Every variant is lossless for the
/// inputs it accepts; [`crate::encode`] returns
/// [`PcxError::Unsupported`] when an image does not fit the requested
/// geometry rather than quantising.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PcxLayout {
    /// 1 bpp × 1 plane monochrome: two colours in header colormap
    /// entries 0 / 1 (spec §4.1 convention black / white when the
    /// palette is `[black, white]`), one bit per pixel.
    Mono1,
    /// 2 bpp × 1 plane 4-colour CGA, packed four pixels per byte; the
    /// palette is a CGA hardware family selected by header byte 19
    /// (C / P / I) with the background colour in header byte 16.
    Cga2x1,
    /// 1 bpp × 2 planes 4-colour CGA, plane-oriented (EGFF canonical
    /// CGA mode); same header palette selector as [`Self::Cga2x1`].
    Cga1x2,
    /// 1 bpp × 3 planes 8-colour EGA RGB: one bit-plane per primary,
    /// no stored palette (the eight on/off primaries are intrinsic).
    EgaRgb1x3,
    /// 4 bpp × 1 plane 16-colour packed nibbles with the palette in the
    /// 48-byte header colormap.
    Indexed4,
    /// 1 bpp × 4 planes 16-colour EGA bit-planes with the palette in
    /// the 48-byte header colormap.
    Indexed1x4,
    /// 8 bpp × 1 plane 256-colour indices with the 768-byte VGA tail
    /// palette (marker `0x0C`).
    Indexed8,
    /// 8 bpp × 1 plane grayscale: `palette_info = 2`, no tail; the
    /// pixel byte is the grey level.
    Gray8,
    /// 8 bpp × 3 planes 24-bit RGB, no palette.
    Rgb24,
}

impl PcxLayout {
    /// `(bits_per_pixel, n_planes)` the geometry writes into the header.
    pub fn depth_planes(self) -> (u8, u8) {
        match self {
            Self::Mono1 => (1, 1),
            Self::Cga2x1 => (2, 1),
            Self::Cga1x2 => (1, 2),
            Self::EgaRgb1x3 => (1, 3),
            Self::Indexed4 => (4, 1),
            Self::Indexed1x4 => (1, 4),
            Self::Indexed8 | Self::Gray8 => (8, 1),
            Self::Rgb24 => (8, 3),
        }
    }

    /// The native [`PixelFormat`] [`crate::decode`] returns for files
    /// in this geometry.
    pub fn pixel_format(self) -> PixelFormat {
        match self {
            Self::Rgb24 => PixelFormat::Rgb24,
            Self::Gray8 => PixelFormat::Gray8,
            _ => PixelFormat::Pal8,
        }
    }

    /// Number of palette entries a decoded image of this geometry
    /// carries (`0` for the palette-free layouts).
    pub fn palette_len(self) -> usize {
        match self {
            Self::Mono1 => 2,
            Self::Cga2x1 | Self::Cga1x2 => 4,
            Self::EgaRgb1x3 => 8,
            Self::Indexed4 | Self::Indexed1x4 => 16,
            Self::Indexed8 => 256,
            Self::Gray8 | Self::Rgb24 => 0,
        }
    }

    /// The geometry [`crate::encode`] picks for an image that carries
    /// no [`PcxImage::layout`] and whose options force none:
    ///
    /// * `Rgb24` → [`Self::Rgb24`]; `Gray8` → [`Self::Gray8`];
    ///   `Rgba` → [`Self::Rgb24`] (alpha dropped only when the options
    ///   allow it).
    /// * `Pal8` by its palette: exactly `[black, white]` →
    ///   [`Self::Mono1`]; ≤ 4 entries all found in one CGA hardware
    ///   palette → [`Self::Cga2x1`]; exactly the eight on/off primaries
    ///   in index order → [`Self::EgaRgb1x3`]; ≤ 16 entries with at
    ///   least one non-zero byte → [`Self::Indexed4`]; otherwise →
    ///   [`Self::Indexed8`] (an all-zero 16-entry colormap would read
    ///   back as the EGA hardware default, so all-black tables take the
    ///   VGA tail, which has no such sentinel).
    ///
    /// A decoded image re-encodes in these geometries with the same
    /// palette length it was decoded with, so `decode(encode(img)) ==
    /// img` holds for everything [`crate::decode`] produces.
    pub fn natural_for(format: PixelFormat, palette: Option<&Palette>) -> Self {
        match format {
            PixelFormat::Rgb24 | PixelFormat::Rgba => Self::Rgb24,
            PixelFormat::Gray8 => Self::Gray8,
            PixelFormat::Pal8 => {
                let Some(p) = palette else {
                    return Self::Indexed8;
                };
                let rgb: Vec<[u8; 3]> = p.entries.iter().map(|e| [e[0], e[1], e[2]]).collect();
                if rgb.len() == 2 && rgb[0] == [0, 0, 0] && rgb[1] == [0xFF, 0xFF, 0xFF] {
                    return Self::Mono1;
                }
                if rgb.len() <= 4 && crate::encoder::cga_match(&rgb).is_some() {
                    return Self::Cga2x1;
                }
                if rgb.len() == 8 && rgb == crate::decoder::RGB_PRIMARIES_PALETTE {
                    return Self::EgaRgb1x3;
                }
                if rgb.len() <= 16 && rgb.iter().flatten().any(|&b| b != 0) {
                    return Self::Indexed4;
                }
                Self::Indexed8
            }
        }
    }
}

/// One pixel plane: `stride` bytes per row, `data` holding at least
/// `stride × height` bytes (rows may carry padding past the visible
/// width). PCX layouts are packed, so a [`PcxImage`] has exactly one.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Plane {
    /// Bytes per row.
    pub stride: usize,
    /// Row-major bytes, `stride × height` long.
    pub data: Vec<u8>,
}

impl Plane {
    /// Wrap a plane buffer with its row stride.
    pub fn new(stride: usize, data: Vec<u8>) -> Self {
        Self { stride, data }
    }
}

/// Nominal sample range (H.273 `VideoFullRangeFlag`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ColorRange {
    /// No range was signalled.
    #[default]
    Unspecified,
    /// Limited (video / studio) range: `VideoFullRangeFlag == 0`.
    Limited,
    /// Full (PC) range: `VideoFullRangeFlag == 1`.
    Full,
}

/// Colour signalling of an image: the sample range plus the H.273
/// `ColourPrimaries` / `TransferCharacteristics` /
/// `MatrixCoefficients` code points (`2` = unspecified).
///
/// PCX has no colour-space signalling of any kind (no ICC, no
/// primaries, no gamma — the header carries only device palettes and
/// authoring DPI), so [`crate::decode`] always fills
/// [`ColorInfo::pcx_default`]: full-range RGB (`matrix` 0) with
/// unspecified primaries and transfer. This is the crate's documented
/// convention, not a value read from the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ColorInfo {
    /// Sample range.
    pub range: ColorRange,
    /// H.273 `ColourPrimaries` code point (`1` = BT.709 / sRGB, `2` =
    /// unspecified).
    pub primaries: u8,
    /// H.273 `TransferCharacteristics` code point (`13` = sRGB, `2` =
    /// unspecified).
    pub transfer: u8,
    /// H.273 `MatrixCoefficients` code point (`0` = identity / RGB).
    pub matrix: u8,
}

impl ColorInfo {
    /// H.273 "unspecified" code point.
    pub const UNSPECIFIED: u8 = 2;
    /// H.273 `MatrixCoefficients` identity (RGB / GBR) code point.
    pub const MATRIX_IDENTITY: u8 = 0;
    /// H.273 `ColourPrimaries` BT.709 / sRGB code point.
    pub const PRIMARIES_BT709: u8 = 1;
    /// H.273 `TransferCharacteristics` IEC 61966-2-1 sRGB code point.
    pub const TRANSFER_SRGB: u8 = 13;

    /// Build a description from its four parts.
    pub const fn new(range: ColorRange, primaries: u8, transfer: u8, matrix: u8) -> Self {
        Self {
            range,
            primaries,
            transfer,
            matrix,
        }
    }

    /// Every field unspecified.
    pub const fn unspecified() -> Self {
        Self::new(
            ColorRange::Unspecified,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
        )
    }

    /// PCX's documented default (the format signals no colour space):
    /// full-range RGB (`matrix` 0) with unspecified primaries and
    /// transfer.
    pub const fn pcx_default() -> Self {
        Self::new(
            ColorRange::Full,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
            Self::MATRIX_IDENTITY,
        )
    }

    /// sRGB (IEC 61966-2-1): BT.709 primaries, sRGB transfer, identity
    /// matrix, full range.
    pub const fn srgb() -> Self {
        Self::new(
            ColorRange::Full,
            Self::PRIMARIES_BT709,
            Self::TRANSFER_SRGB,
            Self::MATRIX_IDENTITY,
        )
    }

    /// Set the range.
    pub fn with_range(mut self, range: ColorRange) -> Self {
        self.range = range;
        self
    }

    /// Set the primaries code point.
    pub fn with_primaries(mut self, primaries: u8) -> Self {
        self.primaries = primaries;
        self
    }

    /// Set the transfer code point.
    pub fn with_transfer(mut self, transfer: u8) -> Self {
        self.transfer = transfer;
        self
    }

    /// Set the matrix code point.
    pub fn with_matrix(mut self, matrix: u8) -> Self {
        self.matrix = matrix;
        self
    }

    /// `true` when both primaries and transfer are specified (`!= 2`).
    pub fn is_specified(&self) -> bool {
        self.primaries != Self::UNSPECIFIED && self.transfer != Self::UNSPECIFIED
    }
}

impl Default for ColorInfo {
    /// [`ColorInfo::pcx_default`].
    fn default() -> Self {
        Self::pcx_default()
    }
}

/// The metadata blobs every image crate surfaces: an ICC profile, an
/// Exif payload, an XMP packet and a file gamma. PCX has no carrier
/// for any of them: all four are always `None` from [`crate::decode`]
/// and ignored by [`crate::encode`]. The header's own annotations
/// (authoring DPI, window origin, screen size) are typed extras on
/// [`PcxImage`] instead.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Metadata {
    /// ICC profile bytes. PCX has no carrier; always `None` on decode.
    pub icc: Option<Vec<u8>>,
    /// Exif payload. PCX has no carrier; always `None` on decode.
    pub exif: Option<Vec<u8>>,
    /// XMP packet. PCX has no carrier; always `None` on decode.
    pub xmp: Option<Vec<u8>>,
    /// Encoding gamma. PCX has no carrier; always `None` on decode.
    pub gamma: Option<f32>,
}

impl Metadata {
    /// Empty metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set (or clear) the ICC profile.
    pub fn with_icc(mut self, icc: impl Into<Option<Vec<u8>>>) -> Self {
        self.icc = icc.into();
        self
    }

    /// Set (or clear) the Exif payload.
    pub fn with_exif(mut self, exif: impl Into<Option<Vec<u8>>>) -> Self {
        self.exif = exif.into();
        self
    }

    /// Set (or clear) the XMP packet.
    pub fn with_xmp(mut self, xmp: impl Into<Option<Vec<u8>>>) -> Self {
        self.xmp = xmp.into();
        self
    }

    /// Set (or clear) the file gamma.
    pub fn with_gamma(mut self, gamma: impl Into<Option<f32>>) -> Self {
        self.gamma = gamma.into();
        self
    }

    /// `true` when no field is set.
    pub fn is_empty(&self) -> bool {
        self.icc.is_none() && self.exif.is_none() && self.xmp.is_none() && self.gamma.is_none()
    }
}

/// Colour table of an indexed ([`Pal8`](PcxPixelFormat::Pal8)) image:
/// RGBA entries, index `i` at `entries[i]`. PCX stores no alpha, so
/// every decoded entry is opaque (`255`), and [`crate::encode`] rejects
/// a non-opaque entry unless
/// [`EncodeOptions::drop_alpha`](crate::EncodeOptions::drop_alpha) is
/// set. The length is the geometry's (2 / 4 / 8 / 16 / 256, see
/// [`PcxLayout::palette_len`]); a shorter caller palette is zero-padded
/// on disk and reads back at the geometry's length.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Palette {
    /// `[r, g, b, a]` per entry, at most 256 entries for an 8-bit index.
    pub entries: Vec<[u8; 4]>,
}

impl Palette {
    /// Wrap a list of RGBA entries.
    pub fn new(entries: Vec<[u8; 4]>) -> Self {
        Self { entries }
    }

    /// Build from packed RGB triples (every entry opaque). A trailing
    /// partial triple is dropped.
    pub fn from_rgb(rgb: &[u8]) -> Self {
        let entries = rgb
            .chunks_exact(3)
            .map(|e| [e[0], e[1], e[2], 255])
            .collect();
        Self { entries }
    }

    /// Build from an array of RGB triples (every entry opaque).
    pub fn from_rgb_triples(rgb: &[[u8; 3]]) -> Self {
        Self {
            entries: rgb.iter().map(|e| [e[0], e[1], e[2], 255]).collect(),
        }
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when the palette has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entry `index`, if present.
    pub fn get(&self, index: u8) -> Option<[u8; 4]> {
        self.entries.get(usize::from(index)).copied()
    }

    /// Packed RGB triples (alpha dropped).
    pub fn to_rgb(&self) -> Vec<u8> {
        self.entries
            .iter()
            .flat_map(|e| [e[0], e[1], e[2]])
            .collect()
    }

    /// `true` when any entry is not fully opaque.
    pub fn has_alpha(&self) -> bool {
        self.entries.iter().any(|e| e[3] != 255)
    }
}

/// Decoded PCX image in its native layout, as returned by
/// [`crate::decode`] and consumed by [`crate::encode`].
///
/// `planes` holds exactly one packed plane (every PCX layout is
/// packed) with the row stride equal to `width × bytes_per_pixel` and
/// the spec §1 even-`bytes_per_line` padding stripped; `color` is
/// [`ColorInfo::pcx_default`]; `metadata` is empty (PCX has no
/// carrier); `palette` is `Some` for [`Pal8`](PcxPixelFormat::Pal8).
/// The header extras `dpi`, `window_origin`, `screen_size` and
/// `layout` are filled from the file and written back by
/// [`crate::encode`] unless the [`EncodeOptions`](crate::EncodeOptions)
/// override them.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct PcxImage {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Native pixel layout.
    pub format: PixelFormat,
    /// Pixel planes — exactly one for PCX.
    pub planes: Vec<Plane>,
    /// Colour signalling (range + H.273 code points).
    pub color: ColorInfo,
    /// ICC / Exif / XMP / gamma — always empty for PCX.
    pub metadata: Metadata,
    /// Colour table for `Pal8`.
    pub palette: Option<Palette>,
    /// Source authoring resolution as `(h_dpi, v_dpi)` if the header
    /// carried non-zero values for both fields. Spec §3 records this as
    /// "the resolutions at which the image was created (printer or
    /// scanner); e.g. a scan might store 300, 300." A 0 in either field
    /// means "unset" and surfaces as `None`.
    pub dpi: Option<(u16, u16)>,
    /// Header `(x_min, y_min)` window origin from spec §3 — the source
    /// crop region the pixel buffer came from. `Some` whenever either
    /// component is non-zero, `None` for the conventional zero origin.
    /// Header metadata only: it never shifts the pixel buffer.
    pub window_origin: Option<(u16, u16)>,
    /// Header `(h_screen_size, v_screen_size)` words (spec §3 offsets
    /// 70 / 72, "new field found only in PB IV / IV Plus"): the display
    /// resolution the image was authored on. `Some` iff both are
    /// non-zero.
    pub screen_size: Option<(u16, u16)>,
    /// The on-disk geometry this image was decoded from, which
    /// [`crate::encode`] writes it back in unless
    /// [`EncodeOptions::layout`](crate::EncodeOptions::layout) forces
    /// another. `None` for a caller-assembled image (the natural
    /// geometry is used, see [`PcxLayout::natural_for`]).
    pub layout: Option<PcxLayout>,
}

impl PcxImage {
    /// Assemble an image from its geometry, layout and planes (one for
    /// PCX). Colour is [`ColorInfo::pcx_default`], metadata empty, no
    /// palette, no header extras; the `with_*` builders fill those in.
    ///
    /// The plane geometry is validated so an invalid image cannot be
    /// built here: exactly one plane, a stride of at least `width ×
    /// bytes_per_pixel`, and at least `stride × height` bytes of data
    /// ([`PcxError::InvalidData`] otherwise). A `Pal8` image also
    /// needs a palette; add it with [`PcxImage::with_palette`] or build
    /// the image with [`PcxImage::new_indexed`].
    pub fn new(width: u32, height: u32, format: PixelFormat, planes: Vec<Plane>) -> Result<Self> {
        let img = Self::unchecked(width, height, format, planes);
        img.validate_planes()?;
        Ok(img)
    }

    /// [`PcxImage::new`] for an indexed image: `Pal8` indices (stride
    /// `width`) plus their palette, validated together (geometry,
    /// palette non-empty and ≤ 256 entries, every index inside it).
    pub fn new_indexed(
        width: u32,
        height: u32,
        indices: Vec<u8>,
        palette: Palette,
    ) -> Result<Self> {
        let img = Self::unchecked(
            width,
            height,
            PixelFormat::Pal8,
            vec![Plane::new(width as usize, indices)],
        )
        .with_palette(palette);
        img.validate()?;
        Ok(img)
    }

    /// [`PcxImage::new`] without the geometry check (the colour and
    /// metadata defaults are the same).
    pub(crate) fn unchecked(
        width: u32,
        height: u32,
        format: PixelFormat,
        planes: Vec<Plane>,
    ) -> Self {
        Self {
            width,
            height,
            format,
            planes,
            color: ColorInfo::pcx_default(),
            metadata: Metadata::default(),
            palette: None,
            dpi: None,
            window_origin: None,
            screen_size: None,
            layout: None,
        }
    }

    /// A packed image over `data` with the layout's tight stride
    /// (`width × bytes_per_pixel`), validated like [`PcxImage::new`].
    /// A `Pal8` image built this way still needs
    /// [`PcxImage::with_palette`].
    pub fn packed(width: u32, height: u32, format: PixelFormat, data: Vec<u8>) -> Result<Self> {
        let stride = (width as usize)
            .checked_mul(format.bytes_per_pixel())
            .ok_or_else(|| PcxError::invalid("PCX: row size overflows"))?;
        Self::new(width, height, format, vec![Plane::new(stride, data)])
    }

    /// A packed `Rgb24` image over `data` (`3 × width × height` bytes,
    /// row-major, stride `3 × width`); [`PcxError::InvalidData`] when
    /// the buffer is shorter than that.
    pub fn from_rgb8(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        Self::packed(width, height, PixelFormat::Rgb24, data)
    }

    /// A packed `Rgba` image over `data` (`4 × width × height` bytes).
    /// PCX cannot store alpha: see [`PcxPixelFormat::Rgba`] for what
    /// [`crate::encode`] does with it.
    pub fn from_rgba8(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        Self::packed(width, height, PixelFormat::Rgba, data)
    }

    /// A packed `Gray8` image over `data` (`width × height` bytes).
    pub fn from_gray8(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        Self::packed(width, height, PixelFormat::Gray8, data)
    }

    /// Set the colour description.
    pub fn with_color(mut self, color: ColorInfo) -> Self {
        self.color = color;
        self
    }

    /// Set the metadata.
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set (or clear) the palette.
    pub fn with_palette(mut self, palette: impl Into<Option<Palette>>) -> Self {
        self.palette = palette.into();
        self
    }

    /// Set (or clear) the authoring DPI extra.
    pub fn with_dpi(mut self, dpi: impl Into<Option<(u16, u16)>>) -> Self {
        self.dpi = dpi.into();
        self
    }

    /// Set (or clear) the window-origin extra.
    pub fn with_window_origin(mut self, origin: impl Into<Option<(u16, u16)>>) -> Self {
        self.window_origin = origin.into();
        self
    }

    /// Set (or clear) the screen-size extra.
    pub fn with_screen_size(mut self, screen_size: impl Into<Option<(u16, u16)>>) -> Self {
        self.screen_size = screen_size.into();
        self
    }

    /// Set (or clear) the on-disk geometry hint.
    pub fn with_layout(mut self, layout: impl Into<Option<PcxLayout>>) -> Self {
        self.layout = layout.into();
        self
    }

    /// Image width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Native pixel layout.
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Bytes-per-pixel implied by `format`.
    pub fn bytes_per_pixel(&self) -> usize {
        self.format.bytes_per_pixel()
    }

    /// Bytes per row of the (single) plane; `0` when there is none.
    pub fn stride(&self) -> usize {
        self.planes.first().map(|p| p.stride).unwrap_or(0)
    }

    /// The single packed plane's bytes. Always `Some` for an image this
    /// crate decoded (every PCX layout is packed); `None` only for a
    /// caller-assembled image with no plane.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        self.planes.first().map(|p| p.data.as_slice())
    }

    /// The pixel bytes (the single plane), or an empty slice when there
    /// is no plane. Rows are `stride()` bytes apart.
    pub fn data(&self) -> &[u8] {
        self.as_bytes().unwrap_or(&[])
    }

    /// Mutable view of the pixel bytes (see [`PcxImage::data`]).
    pub fn data_mut(&mut self) -> &mut [u8] {
        match self.planes.first_mut() {
            Some(p) => p.data.as_mut_slice(),
            None => &mut [],
        }
    }

    /// Consume the image, returning its pixel bytes: the plane for the
    /// packed layouts (every PCX layout).
    pub fn into_raw(self) -> Vec<u8> {
        let mut planes = self.planes.into_iter();
        let mut out = planes.next().map(|p| p.data).unwrap_or_default();
        for p in planes {
            out.extend_from_slice(&p.data);
        }
        out
    }

    /// `true` when the image carries transparency: an `Rgba` layout, or
    /// a palette with a non-opaque entry. Never for a decoded PCX.
    pub fn has_alpha(&self) -> bool {
        self.format.has_alpha() || self.palette.as_ref().is_some_and(Palette::has_alpha)
    }

    /// Check the plane geometry: exactly one plane, stride at least
    /// `width × bytes_per_pixel`, data at least `stride × (height - 1)
    /// + row` bytes.
    pub(crate) fn validate_planes(&self) -> Result<()> {
        if self.planes.len() != 1 {
            return Err(PcxError::invalid(format!(
                "PCX: expected exactly one packed plane, got {}",
                self.planes.len()
            )));
        }
        let plane = &self.planes[0];
        let row = (self.width as usize)
            .checked_mul(self.bytes_per_pixel())
            .ok_or_else(|| PcxError::invalid("PCX: row size overflows"))?;
        if plane.stride < row {
            return Err(PcxError::invalid(format!(
                "PCX: stride {} shorter than a {}-pixel row of {} bytes",
                plane.stride, self.width, row
            )));
        }
        let need = if self.height == 0 {
            0
        } else {
            plane
                .stride
                .checked_mul(self.height as usize - 1)
                .and_then(|n| n.checked_add(row))
                .ok_or_else(|| PcxError::invalid("PCX: plane size overflows"))?
        };
        if plane.data.len() < need {
            return Err(PcxError::invalid(format!(
                "PCX: plane holds {} bytes, {}x{} at stride {} needs {}",
                plane.data.len(),
                self.width,
                self.height,
                plane.stride,
                need
            )));
        }
        Ok(())
    }

    /// Check the image is self-consistent: the plane geometry
    /// ([`PcxImage::new`]'s rule), plus for `Pal8` that a non-empty
    /// palette of at most 256 entries is attached and covers every
    /// index used.
    pub fn validate(&self) -> Result<()> {
        self.validate_planes()?;
        if self.format == PixelFormat::Pal8 {
            let pal = self
                .palette
                .as_ref()
                .ok_or_else(|| PcxError::invalid("PCX: Pal8 image without a palette"))?;
            if pal.is_empty() {
                return Err(PcxError::invalid("PCX: Pal8 image with an empty palette"));
            }
            if pal.len() > 256 {
                return Err(PcxError::invalid(format!(
                    "PCX: palette has {} entries; an 8-bit index addresses at most 256",
                    pal.len()
                )));
            }
            let n = pal.len();
            let w = self.width as usize;
            let stride = self.stride();
            let data = self.data();
            for y in 0..self.height as usize {
                let row = &data[y * stride..y * stride + w];
                if let Some(&bad) = row.iter().find(|&&i| usize::from(i) >= n) {
                    return Err(PcxError::invalid(format!(
                        "PCX: palette index {bad} out of range (palette has {n} entries)"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Tightly packed RGBA, 4 bytes per pixel, row-major: `Rgba`
    /// copied, `Rgb24` widened with alpha `255`, `Gray8` replicated to
    /// R = G = B with alpha `255`, `Pal8` expanded through the palette
    /// (an index the palette does not cover, or a missing palette,
    /// yields transparent black). Exact for every layout this crate
    /// decodes — byte-identical to the pre-contract flatten reader; a
    /// caller-assembled image with a short buffer is padded with black
    /// (see [`PcxImage::try_to_rgba8`] to detect that instead).
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.convert(true)
    }

    /// Tightly packed RGB, 3 bytes per pixel, row-major: alpha dropped
    /// (`Rgba` / palette alpha), otherwise as [`PcxImage::to_rgba8`].
    pub fn to_rgb8(&self) -> Vec<u8> {
        self.convert(false)
    }

    /// [`PcxImage::to_rgba8`] reporting a bad plane geometry / missing
    /// palette / out-of-range index instead of substituting black.
    pub fn try_to_rgba8(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(self.convert(true))
    }

    /// [`PcxImage::to_rgb8`] reporting a bad plane geometry / missing
    /// palette / out-of-range index instead of substituting black.
    pub fn try_to_rgb8(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(self.convert(false))
    }

    fn convert(&self, alpha: bool) -> Vec<u8> {
        let w = self.width as usize;
        let h = self.height as usize;
        let out_bpp = if alpha { 4 } else { 3 };
        let mut out = vec![0u8; w * h * out_bpp];
        if alpha {
            out.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
        }
        if w == 0 || h == 0 {
            return out;
        }
        let bpp = self.bytes_per_pixel();
        let stride = self.stride();
        let src = self.data();
        let row_bytes = w * bpp;

        // Palette lookup table: 256 RGBA cells; entries the palette
        // does not cover are transparent black.
        let lut: [[u8; 4]; 256] = match (&self.palette, self.format) {
            (Some(p), PixelFormat::Pal8) => {
                let mut lut = [[0u8; 4]; 256];
                for (slot, e) in lut.iter_mut().zip(p.entries.iter()) {
                    *slot = *e;
                }
                lut
            }
            _ => [[0u8; 4]; 256],
        };

        for y in 0..h {
            let Some(row) = src.get(y * stride..y * stride + row_bytes) else {
                break;
            };
            let dst = &mut out[y * w * out_bpp..(y + 1) * w * out_bpp];
            match self.format {
                PixelFormat::Rgba => {
                    for (s, d) in row.chunks_exact(4).zip(dst.chunks_exact_mut(out_bpp)) {
                        d.copy_from_slice(&s[..out_bpp]);
                    }
                }
                PixelFormat::Rgb24 => {
                    for (s, d) in row.chunks_exact(3).zip(dst.chunks_exact_mut(out_bpp)) {
                        d[..3].copy_from_slice(s);
                    }
                }
                PixelFormat::Gray8 => {
                    for (&g, d) in row.iter().zip(dst.chunks_exact_mut(out_bpp)) {
                        d[0] = g;
                        d[1] = g;
                        d[2] = g;
                    }
                }
                PixelFormat::Pal8 => {
                    for (&i, d) in row.iter().zip(dst.chunks_exact_mut(out_bpp)) {
                        d.copy_from_slice(&lut[usize::from(i)][..out_bpp]);
                    }
                }
            }
        }
        out
    }

    /// The pre-contract decode layout: packed `Rgba` via
    /// [`PcxImage::to_rgba8`], whatever the native layout. This is what
    /// the deprecated `parse_pcx` returns, byte for byte what earlier
    /// releases produced. The header extras are carried over; the
    /// palette is dropped (it has been applied) and `layout` is kept so
    /// a re-encode can still target the source geometry.
    pub fn into_legacy_layout(self) -> Self {
        if self.format == PixelFormat::Rgba {
            return self;
        }
        let rgba = self.to_rgba8();
        let mut out = Self::unchecked(
            self.width,
            self.height,
            PixelFormat::Rgba,
            vec![Plane::new(self.width as usize * 4, rgba)],
        );
        out.color = self.color;
        out.metadata = self.metadata;
        out.dpi = self.dpi;
        out.window_origin = self.window_origin;
        out.screen_size = self.screen_size;
        out.layout = self.layout;
        out
    }

    /// Re-index an image to [`Pal8`](PcxPixelFormat::Pal8): the unique
    /// RGB colours become the palette in first-seen raster order
    /// (`Gray8` sources are widened first; alpha is dropped from
    /// `Rgba`; a `Pal8` source is returned unchanged). Returns
    /// [`PcxError::Unsupported`] when the image has more than 256
    /// distinct colours. This is the conversion the compact encode
    /// ladder uses; [`crate::encode`] never performs it implicitly.
    pub fn to_indexed(&self) -> Result<Self> {
        if self.format == PixelFormat::Pal8 {
            return Ok(self.clone());
        }
        let rgb = self.to_rgb8();
        let (indices, palette) = crate::encoder::first_seen_indexed(&rgb).ok_or_else(|| {
            PcxError::unsupported(
                "PCX indexed encoder: input has > 256 unique colours \
                 (encode it as 24-bit instead)",
            )
        })?;
        let mut out = Self::unchecked(
            self.width,
            self.height,
            PixelFormat::Pal8,
            vec![Plane::new(self.width as usize, indices)],
        )
        .with_palette(Palette::from_rgb_triples(&palette));
        out.color = self.color;
        out.metadata = self.metadata.clone();
        out.dpi = self.dpi;
        out.window_origin = self.window_origin;
        out.screen_size = self.screen_size;
        Ok(out)
    }
}

/// Tightly packed 8-bit RGB (3 bytes per pixel, row-major), the
/// [`crate::decode_rgb8`] result. Same definition in every image
/// crate.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RgbImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `3 × width × height` bytes.
    pub data: Vec<u8>,
}

impl RgbImage {
    /// Wrap a packed RGB buffer.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    /// The pixel bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consume into the pixel bytes.
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }

    /// Bytes per row (`3 × width`).
    pub fn stride(&self) -> usize {
        self.width as usize * 3
    }
}

/// Tightly packed 8-bit RGBA (4 bytes per pixel, row-major), the
/// [`crate::decode_rgba8`] result. Same definition in every image
/// crate.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `4 × width × height` bytes.
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// Wrap a packed RGBA buffer.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    /// The pixel bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consume into the pixel bytes.
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }

    /// Bytes per row (`4 × width`).
    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }
}

/// What [`crate::info`] reads from the 128-byte header (and the VGA
/// tail marker, for 8 bpp × 1 plane files), without decoding a pixel.
/// For a DCX bundle the fields describe the first page and `frames` is
/// the page count.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ImageInfo {
    /// Width in pixels (`x_max - x_min + 1`).
    pub width: u32,
    /// Height in pixels (`y_max - y_min + 1`).
    pub height: u32,
    /// The native layout [`crate::decode`] would return.
    pub format: PixelFormat,
    /// Number of images: `1` for a PCX file, the page count for a DCX
    /// bundle.
    pub frames: u32,
    /// PCX has no alpha mechanism; always `false`.
    pub has_alpha: bool,
    /// Colour signalling; always [`ColorInfo::pcx_default`].
    pub color: ColorInfo,
    /// PCX has no ICC carrier; always `false`.
    pub has_icc: bool,
    /// PCX has no Exif carrier; always `false`.
    pub has_exif: bool,
    /// PCX has no XMP carrier; always `false`.
    pub has_xmp: bool,
    /// The on-disk geometry.
    pub layout: PcxLayout,
    /// Header version byte (0 / 2 / 3 / 4 / 5).
    pub version: u8,
    /// Header `bits_per_pixel` (per plane).
    pub bits_per_pixel: u8,
    /// Header `n_planes`.
    pub n_planes: u8,
    /// Header `bytes_per_line` (per plane, spec says even).
    pub bytes_per_line: u16,
    /// Header `palette_info` (1 = colour / BW, 2 = grayscale).
    pub palette_info: u16,
    /// `true` when an 8 bpp × 1 plane file carries the 769-byte VGA
    /// tail block.
    pub has_vga_palette: bool,
    /// Authoring DPI, `Some` iff both header words are non-zero.
    pub dpi: Option<(u16, u16)>,
    /// Window origin, `Some` iff either header word is non-zero.
    pub window_origin: Option<(u16, u16)>,
    /// PB IV screen size, `Some` iff both header words are non-zero.
    pub screen_size: Option<(u16, u16)>,
}

impl ImageInfo {
    /// A still image in `layout` with the given geometry; every other
    /// field at its "absent" value.
    pub fn new(width: u32, height: u32, layout: PcxLayout) -> Self {
        let (bits_per_pixel, n_planes) = layout.depth_planes();
        Self {
            width,
            height,
            format: layout.pixel_format(),
            frames: 1,
            has_alpha: false,
            color: ColorInfo::pcx_default(),
            has_icc: false,
            has_exif: false,
            has_xmp: false,
            layout,
            version: 5,
            bits_per_pixel,
            n_planes,
            bytes_per_line: 0,
            palette_info: if layout == PcxLayout::Gray8 { 2 } else { 1 },
            has_vga_palette: layout == PcxLayout::Indexed8,
            dpi: None,
            window_origin: None,
            screen_size: None,
        }
    }
}

/// One image of a multi-image file, as returned by [`crate::decode_all`]:
/// a PCX file yields one frame, a DCX bundle one per page.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Frame {
    /// The page, native layout.
    pub image: PcxImage,
    /// Display delay; always `None` (DCX pages are documents, not an
    /// animation).
    pub delay: Option<Duration>,
    /// Zero-based page index inside the bundle (`0` for a plain PCX).
    pub index: u32,
}

impl Frame {
    /// Wrap a page.
    pub fn new(image: PcxImage, index: u32) -> Self {
        Self {
            image,
            delay: None,
            index,
        }
    }
}

// ---------------------------------------------------------------------------
// Typed paletted views (depth accessors below the contract)
// ---------------------------------------------------------------------------

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

/// Origin of the 4-entry palette resolved by
/// [`crate::parse_pcx_indexed_2bpp_cga`] for a 2 bpp × 1 plane PCX (the
/// 4-colour CGA mode described in spec §4.1, packed 4 pixels/byte with
/// the palette selected from the `ega_palette` header bytes 16 / 19 per
/// CGA hardware semantics).
///
/// PCX repurposes the start of the 48-byte colormap region for CGA mode
/// (manual §"CGA Color Map"): header byte 16 — the colormap's byte 0 —
/// holds the EGA index used for palette entry 0 (the "background /
/// border" colour) in its high nibble, and header byte 19 — colormap
/// byte 3 — carries the C / P / I selector bits (`C` bit 7 color burst
/// 0 = color / 1 = monochrome, `P` bit 6 palette 0 = yellow family /
/// 1 = white family, `I` bit 5 intensity 0 = dim / 1 = bright). The tag
/// below records which resolved palette family the decoder landed on,
/// so a re-encode caller can pass the matching `palette_selector` byte
/// back into [`crate::encode_pcx_2bpp_cga`].
///
/// r401 conformance note: before r401 this tag was derived from
/// colormap bytes 16 / 19 (header bytes 32 / 35 — an off-by-16 reading
/// of the manual's "Header Byte #16/#19") using only two selector bits
/// with an inverted palette convention, and the monochrome axis was not
/// representable. The tag now mirrors the manual's C / P / I exactly;
/// "high/low intensity" in the variant names corresponds to the
/// manual's bright/dim `I` bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pcx2bppCgaPaletteSource {
    /// Palette 1 ("white" family), bright: cyan / magenta / white.
    /// `C = 0, P = 1, I = 1` (byte 19 upper bits `0x60`). This is the
    /// most common CGA palette for game screenshots of the era.
    Palette1HighIntensity,
    /// Palette 1 ("white" family), dim: cyan / magenta / light gray.
    /// `C = 0, P = 1, I = 0` (byte 19 upper bits `0x40`).
    Palette1LowIntensity,
    /// Palette 0 ("yellow" family), bright: light green / light red /
    /// yellow. `C = 0, P = 0, I = 1` (byte 19 upper bits `0x20`).
    Palette0HighIntensity,
    /// Palette 0 ("yellow" family), dim: green / red / brown.
    /// `C = 0, P = 0, I = 0` (byte 19 upper bits `0x00`). This is also
    /// where a PCX 3.0+ writer that zero-fills the colormap lands.
    Palette0LowIntensity,
    /// Composite-monochrome ramp, dim (`C = 1, I = 0`, byte 19 upper
    /// bits `0x80`; the `P` bit is ignored in monochrome). Four-level
    /// grey ramp `0x00 / 0x55 / 0xAA / 0xFF`.
    MonochromeDim,
    /// Composite-monochrome ramp, bright (`C = 1, I = 1`, byte 19 upper
    /// bits `0xA0`). Lifted ramp `0x00 / 0x80 / 0xD4 / 0xFF`.
    MonochromeBright,
}

impl Pcx2bppCgaPaletteSource {
    /// Reconstruct the `palette_selector` byte
    /// [`crate::encode_pcx_2bpp_cga`] expects from the resolved source
    /// tag, so a round-trip caller can hand it straight back to the
    /// writer without re-deriving the bit pattern.
    pub fn palette_selector(self) -> u8 {
        match self {
            Self::Palette1HighIntensity => 0x60,
            Self::Palette1LowIntensity => 0x40,
            Self::Palette0HighIntensity => 0x20,
            Self::Palette0LowIntensity => 0x00,
            Self::MonochromeDim => 0x80,
            Self::MonochromeBright => 0xA0,
        }
    }
}

/// Typed 2 bpp × 1 plane CGA paletted view returned by
/// [`crate::parse_pcx_indexed_2bpp_cga`].
///
/// Spec §4.1 describes the 4-colour CGA mode as a single plane of 2 bpp
/// packed-bits data (4 pixels/byte, the top two bits = pixel 0). The
/// 4-entry palette is selected from `ega_palette` byte 16 (high nibble
/// = EGA index for palette entry 0, the "background" colour) and byte
/// 19 (bits 7/6 = palette select + intensity per CGA hardware
/// semantics).
///
/// The standard [`crate::parse_pcx`] entry point always flattens the
/// on-disk image to packed `Rgba` by walking the palette per pixel and
/// dropping the resolved indices. This typed accessor preserves them:
/// the returned [`PcxIndexed2x1Cga`] surfaces one byte per pixel (low
/// two bits = palette index `0..=3`, top-down, padding stripped)
/// alongside the resolved 4-entry RGB palette, the resolved
/// `background_index` (`0..=15`) used for palette entry 0, and a
/// [`Pcx2bppCgaPaletteSource`] tag recording which CGA palette family
/// the decoder landed on.
///
/// Useful for round-tripping a 4-colour CGA PCX through
/// [`crate::encode_pcx_2bpp_cga`] without re-quantising the indices.
#[derive(Debug, Clone)]
pub struct PcxIndexed2x1Cga {
    /// Picture width in pixels (derived from spec §3 `x_max - x_min +
    /// 1`, matching [`PcxImage::width`]).
    pub width: u32,
    /// Picture height in pixels (derived from spec §3 `y_max - y_min +
    /// 1`, matching [`PcxImage::height`]).
    pub height: u32,
    /// `width × height` palette indices, row-major top-down, one byte
    /// per pixel with the index in the low two bits (`0..=3`). The
    /// 2-bpp on-disk format packs four pixels per byte (top two bits =
    /// pixel 0, then 2/3, etc.); this accessor unpacks them to one
    /// byte per pixel. Per-row padding bytes the encoder added to
    /// round `bytes_per_line` up to an even number per spec §1 are NOT
    /// included.
    pub indices: Vec<u8>,
    /// 4-entry RGB palette. Entry 0 is the resolved
    /// [`Self::background_index`] EGA colour; entries 1..=3 come from
    /// the CGA palette family selected by
    /// [`Self::palette_source`].
    pub palette: [[u8; 3]; 4],
    /// EGA index `0..=15` used for palette entry 0 (the CGA "background
    /// / border" colour), read from `ega_palette` byte 16's high
    /// nibble. Round-trips straight back into the
    /// [`crate::encode_pcx_2bpp_cga`] `background_index` argument.
    pub background_index: u8,
    /// Origin of the [`Self::palette`] entries 1..=3 — the CGA palette
    /// family selected by `ega_palette` byte 19's bits 7/6.
    pub palette_source: Pcx2bppCgaPaletteSource,
}

impl PcxIndexed2x1Cga {
    /// Bytes per row (= `width`, one byte per pixel after unpacking).
    pub fn stride(&self) -> usize {
        self.width as usize
    }
}

/// Typed 1 bpp × 2 planes CGA paletted view returned by
/// [`crate::parse_pcx_indexed_1bpp_2planes_cga`].
///
/// The EGFF canonical PCX mode matrix lists 4-colour CGA as
/// `BitsPerPixel = 1, NumBitPlanes = 2` — the plane-oriented sibling of
/// the `2 bpp × 1 plane` packed-bits CGA layout that
/// [`PcxIndexed2x1Cga`] covers. Each on-disk scanline carries plane 0
/// then plane 1 one after another within the row; the bit at the same
/// x-position in each plane stacks into the 2-bit palette index
/// (`p0 | p1 << 1`). The 4-entry palette resolution is identical to the
/// packed mode (header byte 16 high nibble = background, byte 19 bits
/// 7/6 = palette family + intensity), so this view reuses the same
/// [`Pcx2bppCgaPaletteSource`] tag and `background_index`.
///
/// Useful for round-tripping a plane-oriented 4-colour CGA PCX through
/// [`crate::encode_pcx_1bpp_2planes_cga`] without re-quantising the
/// indices, or for applying palette-swap operations on the indices
/// directly.
#[derive(Debug, Clone)]
pub struct PcxIndexed1x2Cga {
    /// Picture width in pixels (spec §3 `x_max - x_min + 1`).
    pub width: u32,
    /// Picture height in pixels (spec §3 `y_max - y_min + 1`).
    pub height: u32,
    /// `width × height` palette indices, row-major top-down, one byte
    /// per pixel with the index in the low two bits (`0..=3`). The
    /// on-disk format stores two 1-bit planes per scanline; this
    /// accessor stacks the matching bit from each plane into one byte
    /// per pixel. Per-row padding bytes the encoder added to round
    /// `bytes_per_line` up to an even number per spec §1 are NOT
    /// included.
    pub indices: Vec<u8>,
    /// 4-entry RGB palette. Entry 0 is the resolved
    /// [`Self::background_index`] EGA colour; entries 1..=3 come from
    /// the CGA palette family selected by [`Self::palette_source`].
    pub palette: [[u8; 3]; 4],
    /// EGA index `0..=15` used for palette entry 0 (the CGA "background
    /// / border" colour), read from `ega_palette` byte 16's high
    /// nibble.
    pub background_index: u8,
    /// Origin of the [`Self::palette`] entries 1..=3 — the CGA palette
    /// family selected by `ega_palette` byte 19's bits 7/6.
    pub palette_source: Pcx2bppCgaPaletteSource,
}

impl PcxIndexed1x2Cga {
    /// Bytes per row (= `width`, one byte per pixel after unpacking).
    pub fn stride(&self) -> usize {
        self.width as usize
    }
}

/// The three significant bits of the CGA palette byte (header byte 19,
/// the colormap's fourth byte) decoded per the verbatim ZSoft PCX Technical
/// Reference Manual, Revision 5 ("CGA Color Map", Header Byte #19):
///
/// > Only upper 3 bits are used, lower 5 bits are ignored. The first
/// > three bits that are used are ordered C, P, I.
/// > * c: color burst enable — 0 = color; 1 = monochrome
/// > * p: palette — 0 = yellow; 1 = white
/// > * i: intensity — 0 = dim; 1 = bright
///
/// `C` is bit 7 (`0x80`), `P` is bit 6 (`0x40`), `I` is bit 5 (`0x20`).
///
/// This is the spec's authoritative three-bit decomposition surfaced by
/// [`crate::parse_pcx_indexed_2bpp_cga_cpi`]. It is the full
/// degree-of-freedom set the manual defines: the legacy
/// [`Pcx2bppCgaPaletteSource`] tag returned by the older
/// [`crate::parse_pcx_indexed_2bpp_cga`] accessor reads only bits 7 / 6
/// and never the intensity bit at position 5, so it cannot represent the
/// `color burst = monochrome` axis nor the dim/bright distinction the
/// manual places on bit 5. This typed view exists to carry all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pcx2bppCgaCpi {
    /// Color-burst bit (`C`, header byte 19 bit 7). `false` = color (the
    /// chroma palettes), `true` = monochrome (the composite-grey ramp).
    pub monochrome: bool,
    /// Palette bit (`P`, header byte 19 bit 6). In the spec's wording
    /// `false` = "yellow" family (green / red / brown), `true` = "white"
    /// family (cyan / magenta / white). Ignored when [`Self::monochrome`]
    /// is set (the monochrome ramp carries no chroma palette).
    pub palette_white: bool,
    /// Intensity bit (`I`, header byte 19 bit 5). `false` = dim, `true` =
    /// bright. Applies to both the chroma palettes and the monochrome
    /// ramp.
    pub intensity_bright: bool,
}

impl Pcx2bppCgaCpi {
    /// Decode the C / P / I bits from a raw header byte 19 value, masking
    /// off the lower five bits the manual says are ignored.
    pub fn from_byte19(byte19: u8) -> Self {
        Self {
            monochrome: byte19 & 0x80 != 0,
            palette_white: byte19 & 0x40 != 0,
            intensity_bright: byte19 & 0x20 != 0,
        }
    }

    /// Reconstruct the header byte 19 value (upper three C / P / I bits
    /// set, lower five zero) so a re-encode caller can hand the surfaced
    /// view straight back to [`crate::encode_pcx_2bpp_cga_cpi`] without
    /// re-deriving the bit positions.
    pub fn to_byte19(self) -> u8 {
        (u8::from(self.monochrome) << 7)
            | (u8::from(self.palette_white) << 6)
            | (u8::from(self.intensity_bright) << 5)
    }
}

/// Typed 2 bpp × 1 plane CGA paletted view returned by
/// [`crate::parse_pcx_indexed_2bpp_cga_cpi`] — the spec-faithful sibling
/// of [`PcxIndexed2x1Cga`] that decodes all three C / P / I bits of
/// header byte 19 per the verbatim ZSoft manual ("CGA Color Map").
///
/// The older [`crate::parse_pcx_indexed_2bpp_cga`] accessor reads only
/// header byte 19 bits 7 / 6, so it cannot represent the manual's
/// `color burst = monochrome` mode (bit 7 set) nor the intensity bit the
/// manual places at position 5. This view carries the full
/// [`Pcx2bppCgaCpi`] decomposition and resolves the matching palette,
/// including the four-level composite-grey ramp the monochrome mode
/// produces.
#[derive(Debug, Clone)]
pub struct PcxIndexed2x1CgaCpi {
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
    /// `width × height` palette indices, row-major top-down, one byte per
    /// pixel with the index in the low two bits (`0..=3`). Per-row padding
    /// bytes are stripped.
    pub indices: Vec<u8>,
    /// 4-entry resolved RGB palette. Entry 0 is the resolved
    /// [`Self::background_index`] EGA colour; entries 1..=3 come from the
    /// CGA palette family (or composite-grey ramp) the C / P / I bits
    /// select.
    pub palette: [[u8; 3]; 4],
    /// EGA index `0..=15` used for palette entry 0, read from header byte
    /// 16's high nibble.
    pub background_index: u8,
    /// The decoded C / P / I bits of header byte 19.
    pub cpi: Pcx2bppCgaCpi,
}

impl PcxIndexed2x1CgaCpi {
    /// Bytes per row (= `width`, one byte per pixel after unpacking).
    pub fn stride(&self) -> usize {
        self.width as usize
    }
}

/// Origin of the 8-entry palette resolved by
/// [`crate::parse_pcx_indexed_1bpp_3planes`] for a 1 bpp × 3 planes PCX
/// (the 8-colour EGA RGB bit-plane mode described in spec §4 — one 1-bit
/// plane per primary, plane order R, G, B).
///
/// Unlike the 16-colour EGA / 256-colour VGA / CGA modes — which read a
/// palette out of the header `ega_palette` field or a VGA tail block —
/// the 8-colour RGB mode carries *no* on-disk palette at all. Each of
/// the three plane bits directly toggles its channel between `0x00` and
/// `0xFF`, so the eight colours are the on/off primary combinations
/// enumerated by the plane bits themselves (per the spec §4 bit-plane
/// example). This enum therefore has a single arm; it exists to keep the
/// typed-view API symmetric with the other paletted accessors (each of
/// which carries a `*PaletteSource` tag) and to document the
/// no-header-palette property explicitly rather than leaving it implicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pcx1bpp3PlanesPaletteSource {
    /// The 8-entry palette is the fixed set of on/off RGB primaries
    /// (`bit 0 = R`, `bit 1 = G`, `bit 2 = B`, each channel either
    /// `0x00` or `0xFF`). No header `ega_palette` field or VGA tail
    /// block is consulted — the spec §4 8-colour RGB mode defines the
    /// colours intrinsically from the plane bits.
    FixedPrimaries,
}

/// Typed 1 bpp × 3 planes paletted view returned by
/// [`crate::parse_pcx_indexed_1bpp_3planes`].
///
/// Spec §4 describes the 8-colour EGA RGB mode where each scanline
/// carries three 1-bit planes laid out one after another within the row
/// (plane 0 = R, plane 1 = G, plane 2 = B — the same plane order
/// [`crate::encode_pcx_1bpp_3planes_ega_rgb`] writes). The three bits at
/// the same x-position across the three planes stack into a 3-bit colour
/// index (`r_bit | g_bit << 1 | b_bit << 2`), and each plane bit toggles
/// its channel between `0x00` and `0xFF`.
///
/// The standard [`crate::parse_pcx`] entry point always materialises an
/// `Rgba` buffer by toggling each channel per plane bit and dropping the
/// resolved index. This typed accessor preserves the index: the returned
/// [`PcxIndexed1x3`] carries one byte per pixel (low three bits = colour
/// index `0..=7`, top-down, padding stripped) alongside the fixed
/// 8-entry RGB palette and a [`Pcx1bpp3PlanesPaletteSource`] tag.
///
/// Useful for round-tripping an 8-colour EGA RGB PCX through
/// [`crate::encode_pcx_1bpp_3planes_ega_rgb`] without re-thresholding, or
/// for applying colour-swap operations on the indices directly. This is
/// the fifth paletted typed view, closing the EGA/CGA/VGA paletted-mode
/// series alongside [`PcxIndexed8`] (8 bpp), [`PcxIndexed4`] (4 bpp),
/// [`PcxIndexed1x4`] (1 bpp × 4 planes), and [`PcxIndexed2x1Cga`]
/// (2 bpp CGA).
#[derive(Debug, Clone)]
pub struct PcxIndexed1x3 {
    /// Picture width in pixels (derived from spec §3 `x_max - x_min +
    /// 1`, matching [`PcxImage::width`]).
    pub width: u32,
    /// Picture height in pixels (derived from spec §3 `y_max - y_min +
    /// 1`, matching [`PcxImage::height`]).
    pub height: u32,
    /// `width × height` colour indices, row-major top-down, one byte
    /// per pixel with the index in the low three bits (`0..=7`). The
    /// 1 bpp × 3 planes on-disk format stacks the same x-position bit
    /// from each of the three planes into a 3-bit value (`r | g << 1 |
    /// b << 2`); this accessor pre-resolves that stacking so the caller
    /// receives one index per pixel. Per-row padding bits beyond `width`
    /// (spec §1 rounds `bytes_per_line` up to an even number) are NOT
    /// included.
    pub indices: Vec<u8>,
    /// Fixed 8-entry RGB palette of on/off primaries. Entry `i` is
    /// `[0xFF if i & 1, 0xFF if i & 2, 0xFF if i & 4]` — the colour the
    /// matching 3-bit plane index resolves to. The source is always
    /// [`Pcx1bpp3PlanesPaletteSource::FixedPrimaries`].
    pub palette: [[u8; 3]; 8],
    /// Origin of the [`Self::palette`] entries — always
    /// [`Pcx1bpp3PlanesPaletteSource::FixedPrimaries`] for this mode.
    pub palette_source: Pcx1bpp3PlanesPaletteSource,
}

impl PcxIndexed1x3 {
    /// Bytes per row (= `width`, one byte per pixel after unpacking).
    pub fn stride(&self) -> usize {
        self.width as usize
    }
}

/// Typed 4 bpp × 4 planes paletted view returned by
/// [`crate::parse_pcx_indexed_4bpp_4planes`].
///
/// This is the one `(bpp, planes)` slot the EGFF canonical PCX video-mode
/// matrix
/// (`docs/image/pcx/pcx-egff-fileformat-info.html`, "PCX Image Data
/// Format" / mode table) does not list as a hardware video mode —
/// real-world files at this depth are vanishingly rare — but the format
/// is *structurally* reachable: the cross-reference summary defines the
/// maximum colour count of any PCX as
///
/// ```text
/// MaxNumberOfColors = (1 << (BitsPerPixel * NumBitPlanes));
/// ```
///
/// so a `4 bpp × 4 planes` file describes `1 << (4 * 4) = 65536` distinct
/// composite values. The on-disk layout is the same plane-oriented form
/// every other multi-plane PCX uses (spec §"Image File (.PCX) Format":
/// "each line of the image is stored by color plane"): each scanline
/// carries plane 0, plane 1, plane 2, plane 3 one after another, each a
/// `bytes_per_line`-byte slice holding `BitsPerPixel = 4` bits per pixel
/// (2 pixels/byte, high nibble first). The nibble at the same x-position
/// across the four planes stacks into a 16-bit composite index
/// (`p0 | p1 << 4 | p2 << 8 | p3 << 12`) — the same plane-`k`-supplies-
/// chunk-`k` ordering the [`PcxIndexed1x4`] EGA path uses, generalised
/// from 1-bit to 4-bit plane chunks.
///
/// Unlike the lower-depth paletted modes, **no palette is surfaced**: the
/// ZSoft rev-5 manual and the EGFF cross-reference define palette
/// geometries only for the ≤ 256-colour modes (the 16-entry header
/// `Colormap` for EGA/CGA, the 768-byte VGA tail for 256-colour) and
/// state outright that the 24-bit mode carries no palette at all. There
/// is no documented 65536-entry palette for this mode, so this accessor
/// surfaces the raw composite indices only and lets the caller decide how
/// to interpret them. That is why [`crate::parse_pcx`] (which must produce
/// packed `Rgba`) rejects `(4, 4)` with [`crate::PcxError::Unsupported`]
/// rather than inventing a colour mapping the spec does not define.
///
/// Useful for round-tripping a `4 bpp × 4 planes` PCX through
/// [`crate::encode_pcx_4bpp_4planes`] without re-quantising the indices.
#[derive(Debug, Clone)]
pub struct PcxIndexed4x4 {
    /// Picture width in pixels (derived from spec §3 `x_max - x_min +
    /// 1`, matching [`PcxImage::width`]).
    pub width: u32,
    /// Picture height in pixels (derived from spec §3 `y_max - y_min +
    /// 1`, matching [`PcxImage::height`]).
    pub height: u32,
    /// `width × height` composite indices, row-major top-down, one `u16`
    /// per pixel. The `4 bpp × 4 planes` on-disk format stacks the same
    /// x-position 4-bit nibble from each of the four planes into a 16-bit
    /// value (`p0 | p1 << 4 | p2 << 8 | p3 << 12`); this accessor
    /// pre-resolves that stacking so the caller receives one composite
    /// index per pixel. Per-row padding pixels beyond `width` (spec §1
    /// rounds `bytes_per_line` up to an even number) are NOT included.
    pub indices: Vec<u16>,
}

impl PcxIndexed4x4 {
    /// Pixels per row (= `width`, one `u16` per pixel after unpacking).
    pub fn stride(&self) -> usize {
        self.width as usize
    }
}
