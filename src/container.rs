//! PCX container: one single-image file becomes one [`Packet`] on
//! stream `0`. Matches how other single-image codecs in the workspace
//! (`oxideav-bmp`, `oxideav-tga`, static `oxideav-webp`) plug into the
//! container pipeline.

use std::io::{Read, SeekFrom, Write};

use oxideav_core::{
    CodecId, CodecParameters, CodecResolver, Error, MediaType, Packet, PixelFormat, Result,
    StreamInfo, TimeBase,
};
use oxideav_core::{
    ContainerRegistry, Demuxer, Muxer, ProbeData, ProbeScore, ReadSeek, WriteSeek, MAX_PROBE_SCORE,
};

use crate::types::{parse_header, PCX_ENCODING_RLE, PCX_MANUFACTURER};

pub fn register(reg: &mut ContainerRegistry) {
    reg.register_demuxer("pcx", open_demuxer);
    reg.register_muxer("pcx", open_muxer);
    reg.register_extension("pcx", "pcx");
    reg.register_extension("pcc", "pcx");
    reg.register_probe("pcx", probe);
}

fn probe(data: &ProbeData) -> ProbeScore {
    if let Some(h) = parse_header(data.buf) {
        // Manufacturer + RLE encoding + a known version is the
        // strongest signal PCX has — the format's first byte is fixed
        // and the version byte takes a known small set.
        let plausible_version = matches!(h.version, 0 | 2 | 3 | 4 | 5);
        let plausible_planes = matches!(h.n_planes, 1 | 3 | 4);
        let plausible_depth = matches!(h.bits_per_pixel, 1 | 2 | 4 | 8);
        if h.manufacturer == PCX_MANUFACTURER
            && h.encoding == PCX_ENCODING_RLE
            && plausible_version
            && plausible_planes
            && plausible_depth
            && h.x_max >= h.x_min
            && h.y_max >= h.y_min
        {
            return MAX_PROBE_SCORE;
        }
    }
    if matches!(data.ext, Some("pcx") | Some("pcc")) {
        oxideav_core::PROBE_SCORE_EXTENSION
    } else {
        0
    }
}

pub fn open_demuxer(
    mut input: Box<dyn ReadSeek>,
    _codecs: &dyn CodecResolver,
) -> Result<Box<dyn Demuxer>> {
    input.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    input.read_to_end(&mut buf)?;
    let header = parse_header(&buf).ok_or_else(|| Error::invalid("PCX: header truncated"))?;
    let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    params.width = Some(header.width());
    params.height = Some(header.height());
    params.pixel_format = Some(PixelFormat::Rgba);
    let stream = StreamInfo {
        index: 0,
        params,
        time_base: TimeBase::new(1, 1),
        start_time: Some(0),
        duration: None,
    };
    Ok(Box::new(PcxDemuxer {
        streams: vec![stream],
        data: Some(buf),
    }))
}

struct PcxDemuxer {
    streams: Vec<StreamInfo>,
    /// `None` once the sole packet has been emitted.
    data: Option<Vec<u8>>,
}

impl Demuxer for PcxDemuxer {
    fn format_name(&self) -> &str {
        "pcx"
    }
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }
    fn next_packet(&mut self) -> Result<Packet> {
        match self.data.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.pts = Some(0);
                pkt.dts = Some(0);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => Err(Error::Eof),
        }
    }
}

pub fn open_muxer(output: Box<dyn WriteSeek>, streams: &[StreamInfo]) -> Result<Box<dyn Muxer>> {
    if streams.len() != 1 {
        return Err(Error::invalid(
            "PCX muxer: expected exactly one video stream",
        ));
    }
    if streams[0].params.media_type != MediaType::Video {
        return Err(Error::invalid("PCX muxer: stream must be video"));
    }
    Ok(Box::new(PcxMuxer { output }))
}

struct PcxMuxer {
    output: Box<dyn WriteSeek>,
}

impl Muxer for PcxMuxer {
    fn format_name(&self) -> &str {
        "pcx"
    }
    fn write_header(&mut self) -> Result<()> {
        Ok(())
    }
    fn write_packet(&mut self, packet: &Packet) -> Result<()> {
        self.output.write_all(&packet.data)?;
        Ok(())
    }
    fn write_trailer(&mut self) -> Result<()> {
        Ok(())
    }
}
