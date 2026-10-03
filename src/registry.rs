//! `oxideav-core` integration layer for `oxideav-pcx`.
//!
//! Gated behind the default-on `registry` feature so image-library
//! consumers can depend on `oxideav-pcx` with `default-features = false`
//! and skip the `oxideav-core` dependency entirely.
//!
//! The module exposes:
//! * [`register`] (the fleet `RuntimeContext` entry point, also what
//!   the `oxideav_core::register!` macro dispatches),
//!   [`register_codecs`] / [`register_containers`] /
//!   [`register_registries`] for callers holding the sub-registries.
//! * [`make_decoder`] / [`make_encoder`] — the codec factories; the
//!   framework `Decoder` / `Encoder` are thin adapters over
//!   [`crate::decode`] / [`crate::encode`] (one implementation).
//! * The frame bridge: `From<PcxImage> for VideoFrame`,
//!   [`PcxImage::from_video_frame`] and
//!   `TryFrom<(&VideoFrame, &CodecParameters)>`, plus the 1:1
//!   [`PcxPixelFormat`] ↔ `oxideav_core::PixelFormat` name mapping.
//! * The `From<PcxError> for oxideav_core::Error` conversion.

use oxideav_core::{
    CodecCapabilities, CodecId, CodecInfo, CodecParameters, CodecRegistry, ColorPrimaries,
    ColorSignal, ContainerRegistry, Decoder, Encoder, Frame, MatrixCoefficients, Packet,
    PixelFormat, RuntimeContext, TimeBase, TransferCharacteristics, VideoFrame, VideoPlane,
};

use crate::container;
use crate::dcx_container;
use crate::error::PcxError;
use crate::image::{ColorInfo, ColorRange, Palette, PcxImage, PcxPixelFormat, Plane};
use crate::options::EncodeOptions;

/// Convert a [`PcxError`] into the framework-shared `oxideav_core::Error`
/// so trait impls in this crate can use `?` on errors returned by the
/// framework-free decode/encode functions.
impl From<PcxError> for oxideav_core::Error {
    fn from(e: PcxError) -> Self {
        match e {
            PcxError::InvalidData(s) => oxideav_core::Error::InvalidData(s),
            PcxError::Unsupported(s) => oxideav_core::Error::Unsupported(s),
            PcxError::LimitExceeded(s) => oxideav_core::Error::InvalidData(s),
            PcxError::Io(e) => oxideav_core::Error::Io(e),
        }
    }
}

// ---- Pixel-format and colour mapping (1:1 by name) ----

/// The 1:1 name mapping from the framework enum to [`PcxPixelFormat`].
pub fn from_core_pixel_format(pf: PixelFormat) -> oxideav_core::Result<PcxPixelFormat> {
    Ok(match pf {
        PixelFormat::Rgba => PcxPixelFormat::Rgba,
        PixelFormat::Rgb24 => PcxPixelFormat::Rgb24,
        PixelFormat::Gray8 => PcxPixelFormat::Gray8,
        PixelFormat::Pal8 => PcxPixelFormat::Pal8,
        other => {
            return Err(oxideav_core::Error::unsupported(format!(
                "PCX: pixel format {other:?} not supported"
            )))
        }
    })
}

/// The 1:1 name mapping from [`PcxPixelFormat`] to the framework enum.
pub fn to_core_pixel_format(pf: PcxPixelFormat) -> PixelFormat {
    match pf {
        PcxPixelFormat::Rgba => PixelFormat::Rgba,
        PcxPixelFormat::Rgb24 => PixelFormat::Rgb24,
        PcxPixelFormat::Gray8 => PixelFormat::Gray8,
        PcxPixelFormat::Pal8 => PixelFormat::Pal8,
    }
}

impl From<PcxPixelFormat> for PixelFormat {
    fn from(pf: PcxPixelFormat) -> Self {
        to_core_pixel_format(pf)
    }
}

impl TryFrom<PixelFormat> for PcxPixelFormat {
    type Error = oxideav_core::Error;
    fn try_from(pf: PixelFormat) -> oxideav_core::Result<Self> {
        from_core_pixel_format(pf)
    }
}

/// [`ColorInfo`] as the framework's [`ColorSignal`] (code points map
/// 1:1; `Unspecified` range stays unspecified).
pub fn to_color_signal(c: &ColorInfo) -> ColorSignal {
    let range = match c.range {
        ColorRange::Unspecified => oxideav_core::ColorRange::Unspecified,
        ColorRange::Limited => oxideav_core::ColorRange::Limited,
        ColorRange::Full => oxideav_core::ColorRange::Full,
    };
    ColorSignal::new(
        range,
        ColorPrimaries(c.primaries),
        TransferCharacteristics(c.transfer),
        MatrixCoefficients(c.matrix),
    )
}

/// The inverse of [`to_color_signal`].
pub fn from_color_signal(s: &ColorSignal) -> ColorInfo {
    let range = match s.range {
        oxideav_core::ColorRange::Limited => ColorRange::Limited,
        oxideav_core::ColorRange::Full => ColorRange::Full,
        _ => ColorRange::Unspecified,
    };
    ColorInfo::new(range, s.primaries.0, s.transfer.0, s.matrix.0)
}

// ---- Frame bridge ----

fn stamp_frame_side_channels(frame: &mut VideoFrame, image: &PcxImage) {
    if let (PcxPixelFormat::Pal8, Some(p)) = (image.format, &image.palette) {
        frame.set_palette(p.to_rgb());
    }
    // PCX never signals colour; only a caller-supplied signal beyond
    // the documented default is worth stamping.
    let c = image.color;
    if c.primaries != ColorInfo::UNSPECIFIED
        || c.transfer != ColorInfo::UNSPECIFIED
        || c.range == ColorRange::Limited
    {
        frame.set_color_signal(to_color_signal(&c));
    }
}

/// [`PcxImage`] → `VideoFrame` with `pts` stamped: the single packed
/// plane, the palette side-channel for `Pal8` (RGB only — the
/// framework's palette record has no alpha, and PCX has none to lose),
/// and the colour-signal side-channel when the image signals more than
/// PCX's default.
pub fn image_into_video_frame(mut image: PcxImage, pts: Option<i64>) -> VideoFrame {
    let stride = image.stride();
    let data = if image.planes.is_empty() {
        Vec::new()
    } else {
        std::mem::take(&mut image.planes[0].data)
    };
    let mut frame = VideoFrame {
        pts,
        planes: vec![VideoPlane { stride, data }],
    };
    stamp_frame_side_channels(&mut frame, &image);
    frame
}

impl From<PcxImage> for VideoFrame {
    /// The pixel plane (`pts` `None`) plus the side-channels; see
    /// [`image_into_video_frame`].
    fn from(image: PcxImage) -> Self {
        image_into_video_frame(image, None)
    }
}

impl From<&PcxImage> for VideoFrame {
    fn from(image: &PcxImage) -> Self {
        image_into_video_frame(image.clone(), None)
    }
}

impl PcxImage {
    /// Rebuild an image from a framework frame and the stream
    /// parameters that describe it (`width`, `height` required;
    /// `pixel_format` defaults to `Rgb24`). For `Pal8` the palette comes
    /// from the frame's palette side-channel (opaque entries); the
    /// frame's colour-signal side-channel, when attached, becomes
    /// `color`. The geometry is validated.
    pub fn from_video_frame(frame: &VideoFrame, params: &CodecParameters) -> crate::Result<Self> {
        let width = params
            .width
            .ok_or_else(|| PcxError::invalid("PCX: missing width"))?;
        let height = params
            .height
            .ok_or_else(|| PcxError::invalid("PCX: missing height"))?;
        let pix = from_core_pixel_format(params.pixel_format.unwrap_or(PixelFormat::Rgb24))
            .map_err(|e| PcxError::unsupported(e.to_string()))?;
        let plane = frame
            .image_planes()
            .first()
            .ok_or_else(|| PcxError::invalid("PCX: frame has no planes"))?;
        let mut img = PcxImage::new(
            width,
            height,
            pix,
            vec![Plane::new(plane.stride, plane.data.clone())],
        )?;
        if pix == PcxPixelFormat::Pal8 {
            let rgb = frame.palette().ok_or_else(|| {
                PcxError::invalid(
                    "PCX: Pal8 frame carries no palette side-channel \
                     (attach one via VideoFrame::set_palette)",
                )
            })?;
            if rgb.is_empty() || rgb.len() % 3 != 0 {
                return Err(PcxError::invalid(format!(
                    "PCX: palette side-channel must be packed RGB triplets, got {} bytes",
                    rgb.len()
                )));
            }
            img.palette = Some(Palette::from_rgb(rgb));
            img.validate()?;
        }
        if let Some(sig) = frame.color_signal() {
            img.color = from_color_signal(&sig);
        }
        Ok(img)
    }
}

impl TryFrom<(&VideoFrame, &CodecParameters)> for PcxImage {
    type Error = PcxError;
    fn try_from((frame, params): (&VideoFrame, &CodecParameters)) -> crate::Result<Self> {
        PcxImage::from_video_frame(frame, params)
    }
}

// ---- Decoder trait impl + factory ----

/// Factory registered with the codec registry. Consumes one packet per
/// whole PCX file (or DCX page) and produces one frame in the **native
/// layout**: `Pal8` with the file's palette on the frame's palette
/// side-channel for every sub-24-bit geometry, `Gray8` for grayscale
/// files, `Rgb24` for 24-bit files — what the container's stream
/// parameters declare. `params.pixel_format` is not consulted; a
/// consumer that wants packed RGBA converts downstream (or calls
/// [`crate::decode_rgba8`] on the packet bytes).
pub fn make_decoder(_params: &CodecParameters) -> oxideav_core::Result<Box<dyn Decoder>> {
    Ok(Box::new(PcxDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
    }))
}

struct PcxDecoder {
    codec_id: CodecId,
    pending: Option<VideoFrame>,
    eof: bool,
}

impl Decoder for PcxDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> oxideav_core::Result<()> {
        let image = crate::decode(&packet.data)?;
        self.pending = Some(image_into_video_frame(image, packet.pts));
        Ok(())
    }
    fn receive_frame(&mut self) -> oxideav_core::Result<Frame> {
        match self.pending.take() {
            Some(f) => Ok(Frame::Video(f)),
            None => {
                if self.eof {
                    Err(oxideav_core::Error::Eof)
                } else {
                    Err(oxideav_core::Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> oxideav_core::Result<()> {
        self.eof = true;
        Ok(())
    }
}

// ---- Encoder trait impl + factory ----

/// Factory registered with the codec registry: one frame in, one
/// complete PCX file out through [`crate::encode`] with
/// [`EncodeOptions::default`] (alpha dropped for the alpha-bearing
/// framework layouts, which the pre-contract encoder also did).
///
/// Accepted `pixel_format`s: `Rgb24`, `Gray8`, `Pal8` (palette on the
/// frame's side-channel) map 1:1 onto the [`PcxImage`] layouts;
/// `Rgba` / `Bgra` drop alpha and `Bgr24` / `Bgra` swap to RGB;
/// `MonoBlack` / `MonoWhite` unpack the MSB-first 1-bit rows into a
/// two-colour `Pal8` (black / white, `MonoWhite` inverted so bit 1 =
/// white on disk per spec §4.1) that writes as 1 bpp × 1 plane.
pub fn make_encoder(params: &CodecParameters) -> oxideav_core::Result<Box<dyn Encoder>> {
    let mut out_params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    out_params.width = params.width;
    out_params.height = params.height;
    out_params.pixel_format = params.pixel_format;
    Ok(Box::new(PcxEncoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        out_params,
        opts: EncodeOptions::default().with_drop_alpha(true),
        pending: None,
        eof: false,
    }))
}

struct PcxEncoder {
    codec_id: CodecId,
    out_params: CodecParameters,
    opts: EncodeOptions,
    pending: Option<Vec<u8>>,
    eof: bool,
}

/// Bring a framework frame in a layout outside the 1:1 set (`Bgr24`,
/// `Bgra`, `MonoBlack`, `MonoWhite`) into a [`PcxImage`]; the 1:1
/// layouts go through [`PcxImage::from_video_frame`] unchanged.
fn frame_to_image(vf: &VideoFrame, params: &CodecParameters) -> oxideav_core::Result<PcxImage> {
    let format = params.pixel_format.ok_or_else(|| {
        oxideav_core::Error::invalid("PCX encoder: pixel_format missing in CodecParameters")
    })?;
    let width = params.width.ok_or_else(|| {
        oxideav_core::Error::invalid("PCX encoder: width missing in CodecParameters")
    })?;
    let height = params.height.ok_or_else(|| {
        oxideav_core::Error::invalid("PCX encoder: height missing in CodecParameters")
    })?;
    let plane = vf
        .image_planes()
        .first()
        .ok_or_else(|| oxideav_core::Error::invalid("PCX encoder: empty frame plane"))?;
    let (w, h) = (width as usize, height as usize);
    match format {
        PixelFormat::Rgba | PixelFormat::Rgb24 | PixelFormat::Gray8 | PixelFormat::Pal8 => {
            Ok(PcxImage::from_video_frame(vf, params)?)
        }
        PixelFormat::Bgr24 | PixelFormat::Bgra => {
            let bpp = if format == PixelFormat::Bgr24 { 3 } else { 4 };
            let tight = tighten_packed(plane, w, h, bpp)?;
            let rgb: Vec<u8> = tight
                .chunks_exact(bpp)
                .flat_map(|c| [c[2], c[1], c[0]])
                .collect();
            Ok(PcxImage::from_rgb8(width, height, rgb)?)
        }
        PixelFormat::MonoBlack | PixelFormat::MonoWhite => {
            let pixels = unpack_msb_mono(plane, w, h, format == PixelFormat::MonoWhite)?;
            Ok(PcxImage::new_indexed(
                width,
                height,
                pixels,
                Palette::from_rgb_triples(&[[0, 0, 0], [0xFF, 0xFF, 0xFF]]),
            )?)
        }
        other => Err(oxideav_core::Error::invalid(format!(
            "PCX encoder: unsupported pixel format {other:?}"
        ))),
    }
}

impl Encoder for PcxEncoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn output_params(&self) -> &CodecParameters {
        &self.out_params
    }
    fn send_frame(&mut self, frame: &Frame) -> oxideav_core::Result<()> {
        let vf = match frame {
            Frame::Video(v) => v,
            _ => {
                return Err(oxideav_core::Error::invalid(
                    "PCX encoder: expected video frame",
                ))
            }
        };
        let image = frame_to_image(vf, &self.out_params)?;
        self.pending = Some(crate::encoder::encode_image(&image, &self.opts)?);
        Ok(())
    }
    fn receive_packet(&mut self) -> oxideav_core::Result<Packet> {
        match self.pending.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => {
                if self.eof {
                    Err(oxideav_core::Error::Eof)
                } else {
                    Err(oxideav_core::Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> oxideav_core::Result<()> {
        self.eof = true;
        Ok(())
    }
}

fn tighten_packed(
    plane: &VideoPlane,
    width: usize,
    height: usize,
    bytes_per_pixel: usize,
) -> oxideav_core::Result<Vec<u8>> {
    let want = width * bytes_per_pixel;
    if plane.stride < want {
        return Err(oxideav_core::Error::invalid(format!(
            "PCX encoder: plane stride {} smaller than width × bytes-per-pixel {}",
            plane.stride, want
        )));
    }
    if plane.data.len() < plane.stride * height {
        return Err(oxideav_core::Error::invalid(
            "PCX encoder: plane data shorter than stride × height",
        ));
    }
    let mut tight = Vec::with_capacity(want * height);
    for y in 0..height {
        let off = y * plane.stride;
        tight.extend_from_slice(&plane.data[off..off + want]);
    }
    Ok(tight)
}

fn unpack_msb_mono(
    plane: &VideoPlane,
    width: usize,
    height: usize,
    invert: bool,
) -> oxideav_core::Result<Vec<u8>> {
    // Per `oxideav_core::PixelFormat::MonoBlack` / `MonoWhite`: 1 bit
    // per pixel packed MSB-first; rows are padded to a byte boundary
    // (and stride may be larger than `ceil(width / 8)`).
    let row_bytes = width.div_ceil(8);
    if plane.stride < row_bytes {
        return Err(oxideav_core::Error::invalid(format!(
            "PCX encoder: mono plane stride {} smaller than width's row-byte count {}",
            plane.stride, row_bytes
        )));
    }
    if plane.data.len() < plane.stride * height {
        return Err(oxideav_core::Error::invalid(
            "PCX encoder: mono plane data shorter than stride × height",
        ));
    }
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        let row = &plane.data[y * plane.stride..y * plane.stride + row_bytes];
        for x in 0..width {
            // `MonoBlack`: 1 = white, a direct map onto the spec §4.1
            // bit-1 = white convention; `MonoWhite` inverts.
            let bit = (row[x / 8] >> (7 - (x % 8))) & 1;
            out.push(if invert { 1 - bit } else { bit });
        }
    }
    Ok(out)
}

// ---- Registration ----

/// Register the PCX codec into the supplied [`CodecRegistry`].
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("pcx_sw")
        .with_intra_only(true)
        .with_lossless(true)
        .with_max_size(65535, 65535)
        .with_pixel_formats(vec![
            PixelFormat::Rgba,
            PixelFormat::Rgb24,
            PixelFormat::Bgr24,
            PixelFormat::Bgra,
            PixelFormat::Gray8,
            PixelFormat::MonoBlack,
            PixelFormat::MonoWhite,
            PixelFormat::Pal8,
        ]);
    reg.register(
        CodecInfo::new(CodecId::new(crate::CODEC_ID_STR))
            .capabilities(caps)
            .decoder(make_decoder)
            .encoder(make_encoder),
    );
}

/// Register the PCX container demuxer + muxer + extension + probe
/// into the supplied [`ContainerRegistry`].
///
/// Also registers the DCX multi-page bundle (Microsoft FAX container)
/// alongside, since both formats share the PCX codec on the codec side.
pub fn register_containers(reg: &mut ContainerRegistry) {
    container::register(reg);
    dcx_container::register(reg);
}

/// Combined registration for callers holding the two sub-registries
/// rather than a [`RuntimeContext`] (the pre-contract two-argument
/// `register`).
pub fn register_registries(codecs: &mut CodecRegistry, containers: &mut ContainerRegistry) {
    register_codecs(codecs);
    register_containers(containers);
}

/// Unified registration entry point — installs the PCX codec into the
/// codec sub-registry and the PCX + DCX containers into the container
/// sub-registry of the supplied [`RuntimeContext`]. This is the form
/// `oxideav_meta::register_all` dispatches via the
/// [`oxideav_core::register!`] macro.
pub fn register(ctx: &mut RuntimeContext) {
    register_registries(&mut ctx.codecs, &mut ctx.containers);
}

/// The pre-contract name of [`register`].
#[deprecated(note = "use oxideav_pcx::register(&mut RuntimeContext) (IMAGE_CRATE_API)")]
pub fn register_runtime(ctx: &mut RuntimeContext) {
    register(ctx);
}

oxideav_core::register!("pcx", register);

#[cfg(test)]
mod runtime_entry_tests {
    use super::*;

    #[test]
    fn oxideav_entry_installs_codec_and_container() {
        let mut ctx = oxideav_core::RuntimeContext::new();
        __oxideav_entry(&mut ctx);
        assert!(
            ctx.codecs.decoder_ids().next().is_some(),
            "__oxideav_entry should install codec decoder factories"
        );
        assert_eq!(
            ctx.containers.container_for_extension("pcx"),
            Some("pcx"),
            "__oxideav_entry should install the .pcx extension hint"
        );
        assert_eq!(ctx.containers.container_for_extension("dcx"), Some("dcx"));
    }

    #[test]
    fn frame_bridge_round_trips_every_layout() {
        let pal = Palette::new(vec![[1, 2, 3, 255], [4, 5, 6, 255]]);
        let imgs = vec![
            PcxImage::from_rgba8(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap(),
            PcxImage::from_rgb8(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap(),
            PcxImage::from_gray8(2, 1, vec![9, 8]).unwrap(),
            PcxImage::new_indexed(2, 1, vec![0, 1], pal).unwrap(),
        ];
        for img in imgs {
            let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
            params.width = Some(img.width);
            params.height = Some(img.height);
            params.pixel_format = Some(img.format.into());
            let frame: VideoFrame = img.clone().into();
            let back = PcxImage::try_from((&frame, &params)).unwrap();
            assert_eq!(back.format, img.format);
            assert_eq!(back.data(), img.data());
            assert_eq!(back.to_rgba8(), img.to_rgba8());
        }
    }

    #[test]
    fn unsupported_core_format_is_rejected() {
        assert!(from_core_pixel_format(PixelFormat::Yuv420P).is_err());
        assert!(PcxPixelFormat::try_from(PixelFormat::Nv12).is_err());
        assert_eq!(PixelFormat::from(PcxPixelFormat::Pal8), PixelFormat::Pal8);
    }
}
