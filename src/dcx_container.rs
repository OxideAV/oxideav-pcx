//! DCX multi-page PCX container, wired into [`ContainerRegistry`].
//!
//! Each DCX file is a Microsoft FAX-style bundle of standalone PCX
//! pages (see [`crate::dcx`] for the bare bytestream API). The
//! [`Demuxer`] here emits one [`Packet`] per page where the packet
//! body is the full, standalone PCX 5.0 byte stream for that page —
//! the same payload the in-crate PCX decoder already consumes
//! one-PCX-file-per-packet through [`crate::container`].
//!
//! Why one packet per page (instead of one packet per scanline or a
//! single multi-page packet):
//!
//! * The crate's [`crate::decoder::PcxDecoder`] takes a whole PCX file
//!   in `send_packet` and produces one [`Frame`](oxideav_core::Frame) in
//!   `receive_frame`. Page-granular packets line up with that exactly.
//! * It lets each page carry its own pts (the page index) so a pipeline
//!   that wants to render multi-page documents page-by-page can do so
//!   trivially.
//!
//! The packet `stream_index` is always `0`; DCX bundles are a single
//! logical video-image stream regardless of page count.

use std::io::{Read, SeekFrom, Write};

use oxideav_core::{
    CodecId, CodecParameters, CodecResolver, Error, MediaType, Packet, PixelFormat, Result,
    StreamInfo, TimeBase,
};
use oxideav_core::{
    ContainerRegistry, Demuxer, Muxer, ProbeData, ProbeScore, ReadSeek, WriteSeek, MAX_PROBE_SCORE,
};

use crate::dcx::{parse_offset_table, DCX_MAGIC, DCX_MAX_PAGES};
use crate::types::parse_header;

/// Register the DCX container demuxer + muxer + extension + probe.
///
/// The registered format name is `"dcx"` so callers can request it
/// explicitly via the container registry. Probe + extension hits also
/// route to the DCX path automatically.
pub fn register(reg: &mut ContainerRegistry) {
    reg.register_demuxer("dcx", open_demuxer);
    reg.register_muxer("dcx", open_muxer);
    reg.register_extension("dcx", "dcx");
    reg.register_probe("dcx", probe);
}

/// Content probe: matches the 4-byte LE magic at offset 0 and falls
/// back to the file extension when the buffer is too short to carry the
/// magic. The magic is unambiguous (32 bits of fixed pattern), so a
/// magic match earns the maximum probe score.
fn probe(data: &ProbeData) -> ProbeScore {
    if data.buf.len() >= 4 {
        let magic = u32::from_le_bytes([data.buf[0], data.buf[1], data.buf[2], data.buf[3]]);
        if magic == DCX_MAGIC {
            return MAX_PROBE_SCORE;
        }
    }
    if matches!(data.ext, Some("dcx")) {
        oxideav_core::PROBE_SCORE_EXTENSION
    } else {
        0
    }
}

/// Open a DCX file as a multi-page packet stream.
pub fn open_demuxer(
    mut input: Box<dyn ReadSeek>,
    _codecs: &dyn CodecResolver,
) -> Result<Box<dyn Demuxer>> {
    input.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    input.read_to_end(&mut buf)?;
    // Walk the offset table once so we can size the page list and pull
    // out the first-page header for the StreamInfo box.
    let offsets =
        parse_offset_table(&buf).map_err(|e| Error::invalid(format!("DCX demuxer: {e}")))?;
    if offsets.is_empty() {
        return Err(Error::invalid("DCX demuxer: no pages in bundle"));
    }
    // Pre-slice each page into an owned `Vec<u8>` so packet emission is
    // trivial and the demuxer holds no references back into the parent
    // buffer. End of a page is start of the next, or EOF for the last.
    let mut pages: Vec<Vec<u8>> = Vec::with_capacity(offsets.len());
    for (i, &start) in offsets.iter().enumerate() {
        let end = offsets.get(i + 1).copied().unwrap_or(buf.len());
        if end < start || end > buf.len() {
            return Err(Error::invalid(format!(
                "DCX demuxer: page {i} range [{start}..{end}] out of bounds (file len {})",
                buf.len()
            )));
        }
        pages.push(buf[start..end].to_vec());
    }
    // Use the first page's PCX header to populate StreamInfo (width,
    // height). DCX doesn't constrain pages to share dimensions, but the
    // stream surface only carries one set of CodecParameters and the
    // pipeline can re-read per-frame dims at decode time.
    let first_header = parse_header(&pages[0])
        .ok_or_else(|| Error::invalid("DCX demuxer: first page has truncated PCX header"))?;
    let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    params.width = Some(first_header.width());
    params.height = Some(first_header.height());
    params.pixel_format = Some(PixelFormat::Rgba);
    let stream = StreamInfo {
        index: 0,
        params,
        time_base: TimeBase::new(1, 1),
        start_time: Some(0),
        duration: Some(pages.len() as i64),
    };
    Ok(Box::new(DcxDemuxer {
        streams: vec![stream],
        pages,
        cursor: 0,
    }))
}

struct DcxDemuxer {
    streams: Vec<StreamInfo>,
    /// One standalone PCX byte stream per page.
    pages: Vec<Vec<u8>>,
    /// Next page index to emit.
    cursor: usize,
}

impl Demuxer for DcxDemuxer {
    fn format_name(&self) -> &str {
        "dcx"
    }
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }
    fn next_packet(&mut self) -> Result<Packet> {
        if self.cursor >= self.pages.len() {
            return Err(Error::Eof);
        }
        // Move the page bytes out so the demuxer doesn't double-buffer.
        let bytes = std::mem::take(&mut self.pages[self.cursor]);
        let pts = self.cursor as i64;
        self.cursor += 1;
        let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
        pkt.pts = Some(pts);
        pkt.dts = Some(pts);
        pkt.flags.keyframe = true;
        Ok(pkt)
    }
}

/// Open a DCX writer for a single video stream.
///
/// Each subsequent [`Packet`] passed to `write_packet` is treated as a
/// fully-formed standalone PCX 5.0 byte stream (the same shape the PCX
/// muxer accepts) and stored as a DCX page. Up to [`DCX_MAX_PAGES`] are
/// permitted; further packets return [`Error::Unsupported`].
pub fn open_muxer(output: Box<dyn WriteSeek>, streams: &[StreamInfo]) -> Result<Box<dyn Muxer>> {
    if streams.len() != 1 {
        return Err(Error::invalid(
            "DCX muxer: expected exactly one video stream",
        ));
    }
    if streams[0].params.media_type != MediaType::Video {
        return Err(Error::invalid("DCX muxer: stream must be video"));
    }
    Ok(Box::new(DcxMuxer {
        output,
        pages: Vec::new(),
    }))
}

struct DcxMuxer {
    output: Box<dyn WriteSeek>,
    /// Pages buffered in memory until `write_trailer` finalises the
    /// container. DCX prefixes the file with a fixed-size offset table
    /// so we must know all page sizes before we can write the header.
    pages: Vec<Vec<u8>>,
}

impl Muxer for DcxMuxer {
    fn format_name(&self) -> &str {
        "dcx"
    }
    fn write_header(&mut self) -> Result<()> {
        // No-op: the magic + offset table are written in
        // `write_trailer` once page sizes are known.
        Ok(())
    }
    fn write_packet(&mut self, packet: &Packet) -> Result<()> {
        if self.pages.len() >= DCX_MAX_PAGES {
            return Err(Error::unsupported(format!(
                "DCX muxer: page cap {DCX_MAX_PAGES} reached"
            )));
        }
        // The packet body must already be a valid PCX 5.0 file. Sanity-
        // check the header so a bad upstream packet doesn't pollute the
        // bundle; the demuxer side rejects bad pages too.
        if parse_header(&packet.data).is_none() {
            return Err(Error::invalid(
                "DCX muxer: packet body too short for a PCX header",
            ));
        }
        self.pages.push(packet.data.clone());
        Ok(())
    }
    fn write_trailer(&mut self) -> Result<()> {
        if self.pages.is_empty() {
            return Err(Error::invalid("DCX muxer: no pages written"));
        }
        // Compute the header size: 4-byte magic + (n+1) u32 offsets
        // (one per page plus a trailing zero sentinel). Then emit the
        // magic, the offset table, and finally the page bodies. Layout
        // is identical to what [`crate::dcx::encode_dcx`] produces so
        // the existing parser round-trips.
        let header_bytes = 4 + (self.pages.len() + 1) * 4;
        self.output.write_all(&DCX_MAGIC.to_le_bytes())?;
        let mut cursor = header_bytes;
        for page in &self.pages {
            let off: u32 = cursor
                .try_into()
                .map_err(|_| Error::invalid("DCX muxer: page offset exceeds u32 (file > 4 GiB)"))?;
            self.output.write_all(&off.to_le_bytes())?;
            cursor += page.len();
        }
        self.output.write_all(&0u32.to_le_bytes())?;
        for page in &self.pages {
            self.output.write_all(page)?;
        }
        Ok(())
    }
}
