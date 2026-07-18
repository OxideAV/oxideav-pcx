//! `oxideav-core` integration layer for `oxideav-pcx`.
//!
//! Gated behind the default-on `registry` feature so image-library
//! consumers can depend on `oxideav-pcx` with `default-features = false`
//! and skip the `oxideav-core` dependency entirely.
//!
//! The module exposes:
//! * [`register`] / [`register_codecs`] / [`register_containers`] — the
//!   `CodecRegistry` / `ContainerRegistry` entry points the umbrella
//!   `oxideav` crate calls during framework initialisation.
//! * The `From<PcxError> for oxideav_core::Error` conversion that lets
//!   the trait-side `Decoder` / `Encoder` impls (in `decoder.rs` /
//!   `encoder.rs`) bubble bitstream errors up through the framework
//!   error type.

use oxideav_core::ContainerRegistry;
use oxideav_core::{CodecCapabilities, CodecId, PixelFormat};
use oxideav_core::{CodecInfo, CodecRegistry};

use crate::container;
use crate::dcx_container;
use crate::error::PcxError;

/// Convert a [`PcxError`] into the framework-shared `oxideav_core::Error`
/// so trait impls in this crate can use `?` on errors returned by the
/// framework-free decode/encode functions.
impl From<PcxError> for oxideav_core::Error {
    fn from(e: PcxError) -> Self {
        match e {
            PcxError::InvalidData(s) => oxideav_core::Error::InvalidData(s),
            PcxError::Unsupported(s) => oxideav_core::Error::Unsupported(s),
        }
    }
}

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
            .decoder(crate::decoder::make_decoder)
            .encoder(crate::encoder::make_encoder),
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

/// Combined registration for callers that just want everything wired up
/// in one call.
pub fn register(codecs: &mut CodecRegistry, containers: &mut ContainerRegistry) {
    register_codecs(codecs);
    register_containers(containers);
}
