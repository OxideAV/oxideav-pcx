//! Crate-local error type used by `oxideav-pcx`'s standalone (no
//! `oxideav-core`) public API.
//!
//! When the `registry` feature is enabled, [`PcxError`] gains a
//! `From<PcxError> for oxideav_core::Error` impl (defined in
//! [`crate::registry`]) so the trait-side surface (`Decoder` /
//! `Encoder`) can keep returning `oxideav_core::Result<T>` while the
//! underlying decode/encode functions stay framework-free.

use core::fmt;

/// `Result` alias scoped to `oxideav-pcx`. Standalone (no
/// `oxideav-core`) callers see this; framework callers convert via the
/// gated `From<PcxError> for oxideav_core::Error` impl.
pub type Result<T> = core::result::Result<T, PcxError>;

/// Error variants returned by `oxideav-pcx`'s standalone API.
///
/// The variants mirror the subset of `oxideav_core::Error` the codec
/// can hit. The crate intentionally avoids surfacing transport (`Io`)
/// or framework-specific (`FormatNotFound`, `CodecNotFound`) errors —
/// those originate in callers that are already linking `oxideav-core`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcxError {
    /// The byte stream is malformed (truncated header, RLE run runs
    /// past the end of the scanline, palette marker missing where
    /// expected, version byte out of the {0,2,3,4,5} set, …).
    InvalidData(String),
    /// The byte stream uses a feature this codec doesn't implement
    /// (a (depth, planes) combination outside the round-1 set,
    /// encoding ≠ 1, …).
    Unsupported(String),
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
}

impl fmt::Display for PcxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(s) => write!(f, "invalid data: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

impl std::error::Error for PcxError {}
