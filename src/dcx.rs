//! DCX multi-page PCX bundle (Microsoft FAX format).
//!
//! DCX is a thin wrapper that concatenates multiple PCX files into one
//! container. It was originally used for fax pages and document scans
//! where a single logical document spans multiple bitmap pages.
//!
//! On-disk layout:
//!
//! ```text
//!   [4-byte LE magic]                — 0x3ADE_68B1
//!   [u32 LE × up to 1024 entries]    — byte offsets of each PCX page
//!     - Entries are non-zero offsets into the file.
//!     - The list is terminated by a 0x00000000 sentinel entry, OR by
//!       the natural cap of 1024 entries (1023 pages + sentinel).
//!     - Empty slots after the sentinel may be padded with zeros so the
//!       offset table always occupies a fixed 4 KiB region in the
//!       writer's view; readers MUST stop at the first zero.
//!   [PCX file 1]                     — at offset[0]
//!   [PCX file 2]                     — at offset[1]
//!   ...
//! ```
//!
//! Each embedded PCX file is a stand-alone, valid PCX 5.0 stream
//! (header + RLE pixel data + optional VGA tail palette). The container
//! does not interpret PCX page contents — it only locates them.

use crate::decoder::parse_pcx;
use crate::error::{PcxError as Error, Result};
use crate::image::PcxImage;

/// 4-byte little-endian magic that prefixes every DCX file.
pub const DCX_MAGIC: u32 = 0x3ADE_68B1;

/// Maximum number of PCX pages in a single DCX bundle. The on-disk
/// offset table holds 1024 u32 LEs and one of those slots is reserved
/// for the terminating zero, so up to 1023 pages can be carried.
pub const DCX_MAX_PAGES: usize = 1023;

/// One decoded DCX bundle.
#[derive(Debug, Clone)]
pub struct DcxImage {
    /// One [`PcxImage`] per page in the bundle. `pages.len() ≤
    /// `DCX_MAX_PAGES`].
    pub pages: Vec<PcxImage>,
}

/// Parse a DCX file: magic check, walk the offset table, then decode
/// each embedded PCX page with [`parse_pcx`].
///
/// Returns an error if the magic doesn't match, the offset table is
/// truncated, an offset is out of range, or any embedded PCX page
/// fails to decode.
pub fn parse_dcx(input: &[u8]) -> Result<DcxImage> {
    let offsets = parse_offset_table(input)?;
    let mut pages = Vec::with_capacity(offsets.len());
    for (i, &start) in offsets.iter().enumerate() {
        // The end of this page is the start of the next page, OR EOF
        // for the last page. PCX has no length field in its own header
        // so we slice the bundle accordingly before handing the bytes
        // to `parse_pcx`.
        let end = offsets.get(i + 1).copied().unwrap_or(input.len());
        if end > input.len() || end < start {
            return Err(Error::invalid(format!(
                "DCX: page {i} offset range [{start}..{end}] out of file bounds (len {})",
                input.len()
            )));
        }
        let page_bytes = &input[start..end];
        let img = parse_pcx(page_bytes)
            .map_err(|e| Error::invalid(format!("DCX: page {i} parse failed: {e}")))?;
        pages.push(img);
    }
    Ok(DcxImage { pages })
}

/// Parse the DCX offset table and return the in-bundle byte offsets of
/// each PCX page. Used internally by [`parse_dcx`] but also exposed
/// for callers that want to demux pages lazily.
pub fn parse_offset_table(input: &[u8]) -> Result<Vec<usize>> {
    if input.len() < 4 {
        return Err(Error::invalid("DCX: file shorter than magic (4 bytes)"));
    }
    let magic = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);
    if magic != DCX_MAGIC {
        return Err(Error::invalid(format!(
            "DCX: bad magic 0x{:08X} (expected 0x{:08X})",
            magic, DCX_MAGIC
        )));
    }
    // Walk the offset table: every 4-byte LE u32 starting at offset 4.
    // Stop at the first zero (sentinel) OR the cap of 1024 slots.
    let mut offsets = Vec::new();
    let mut cursor = 4usize;
    for _ in 0..1024 {
        if cursor + 4 > input.len() {
            return Err(Error::invalid(
                "DCX: offset table truncated (no terminating zero)",
            ));
        }
        let off = u32::from_le_bytes([
            input[cursor],
            input[cursor + 1],
            input[cursor + 2],
            input[cursor + 3],
        ]) as usize;
        cursor += 4;
        if off == 0 {
            // Sentinel reached.
            return Ok(offsets);
        }
        if off >= input.len() {
            return Err(Error::invalid(format!(
                "DCX: page offset {off} out of file bounds (len {})",
                input.len()
            )));
        }
        if offsets.len() >= DCX_MAX_PAGES {
            return Err(Error::invalid(format!(
                "DCX: too many pages (cap is {DCX_MAX_PAGES})"
            )));
        }
        // Offsets MUST be monotonically increasing per the spec — pages
        // appear in file order, and the reader needs increasing offsets
        // to compute per-page byte ranges.
        if let Some(&prev) = offsets.last() {
            if off <= prev {
                return Err(Error::invalid(format!(
                    "DCX: non-monotonic offsets ({prev} then {off})"
                )));
            }
        }
        offsets.push(off);
    }
    // Cap reached without seeing a sentinel — accept it; some writers
    // pack the table to the brim with no sentinel.
    Ok(offsets)
}

/// Build a DCX bundle from a list of stand-alone PCX byte streams.
///
/// The bundle layout matches what [`parse_dcx`] consumes: 4-byte magic,
/// `pages.len() + 1` u32 LE offsets (with a trailing zero sentinel),
/// then the concatenated pages. Returns an error if more than
/// `DCX_MAX_PAGES` pages are supplied.
pub fn encode_dcx(pages: &[Vec<u8>]) -> Result<Vec<u8>> {
    if pages.is_empty() {
        return Err(Error::invalid("DCX: no pages supplied"));
    }
    if pages.len() > DCX_MAX_PAGES {
        return Err(Error::invalid(format!(
            "DCX: {} pages exceeds cap {DCX_MAX_PAGES}",
            pages.len()
        )));
    }
    // Header = 4-byte magic + (pages.len() + 1) × 4 bytes (one slot per
    // page plus the sentinel zero). We use the minimum required size so
    // the file is compact; readers handle both the minimum and the
    // 4 KiB-padded layout.
    let header_bytes = 4 + (pages.len() + 1) * 4;
    let total: usize = header_bytes + pages.iter().map(|p| p.len()).sum::<usize>();
    let mut out = Vec::with_capacity(total);
    // Magic.
    out.extend_from_slice(&DCX_MAGIC.to_le_bytes());
    // Offsets — compute as we go.
    let mut cursor = header_bytes;
    for page in pages {
        let off: u32 = cursor
            .try_into()
            .map_err(|_| Error::invalid("DCX: page offset exceeds u32"))?;
        out.extend_from_slice(&off.to_le_bytes());
        cursor += page.len();
    }
    // Sentinel.
    out.extend_from_slice(&0u32.to_le_bytes());
    debug_assert_eq!(out.len(), header_bytes);
    // Concatenated PCX pages.
    for page in pages {
        out.extend_from_slice(page);
    }
    Ok(out)
}
