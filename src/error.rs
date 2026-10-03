//! Crate-local error type used by `oxideav-pcx`'s standalone (no
//! `oxideav-core`) public API.
//!
//! When the `registry` feature is enabled, [`PcxError`] gains a
//! `From<PcxError> for oxideav_core::Error` impl (defined in
//! `crate::registry`) so the trait-side surface (`Decoder` /
//! `Encoder`) can keep returning `oxideav_core::Result<T>` while the
//! underlying decode/encode functions stay framework-free.

use core::fmt;

/// `Result` alias scoped to `oxideav-pcx`. Standalone (no
/// `oxideav-core`) callers see this; framework callers convert via the
/// gated `From<PcxError> for oxideav_core::Error` impl.
pub type Result<T> = core::result::Result<T, PcxError>;

/// The contract name for [`PcxError`].
pub type Error = PcxError;

/// Error variants returned by `oxideav-pcx`'s standalone API.
///
/// The variants mirror the subset of `oxideav_core::Error` the codec
/// can hit plus the two the image-crate contract requires
/// (`LimitExceeded`, `Io`). Framework-specific errors
/// (`FormatNotFound`, `CodecNotFound`) originate in callers that are
/// already linking `oxideav-core`.
#[derive(Debug)]
#[non_exhaustive]
pub enum PcxError {
    /// The byte stream is malformed (truncated header, RLE run runs
    /// past the end of the image, palette marker missing where
    /// expected, version byte out of the {0,2,3,4,5} set, …), or a
    /// caller-assembled image is inconsistent (plane too short,
    /// palette index out of range, non-zero DPI sentinel broken).
    InvalidData(String),
    /// The byte stream uses a feature this codec doesn't implement
    /// (a (depth, planes) combination outside the spec's mode table,
    /// encoding ≠ 1, …), or the encoder was asked for something the
    /// format cannot represent (alpha without `drop_alpha`, more than
    /// 256 colours into an indexed geometry, …).
    Unsupported(String),
    /// A [`crate::DecodeOptions`] limit (dimensions / pixels / bytes)
    /// would be exceeded; nothing was allocated.
    LimitExceeded(String),
    /// A read / write on a caller-supplied stream failed
    /// ([`crate::decode_from`] / [`crate::encode_to`]).
    Io(std::io::Error),
}

impl PcxError {
    /// Construct a [`PcxError::InvalidData`] from a stringy message.
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidData(msg.into())
    }

    /// Construct a [`PcxError::Unsupported`] from a stringy message.
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }

    /// Construct a [`PcxError::LimitExceeded`] from a stringy message.
    pub fn limit(msg: impl Into<String>) -> Self {
        Self::LimitExceeded(msg.into())
    }
}

impl From<std::io::Error> for PcxError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl fmt::Display for PcxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(s) => write!(f, "invalid data: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::LimitExceeded(s) => write!(f, "limit exceeded: {s}"),
            Self::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for PcxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}
