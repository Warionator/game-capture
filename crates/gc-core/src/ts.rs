//! MPEG-TS chunker.
//!
//! FFmpeg writes the encoded stream as MPEG-TS to stdout. This module splits
//! that byte stream into *chunks* that each start at a video keyframe and are
//! independently decodable: FFmpeg's TS muxer re-emits PAT/PMT right before
//! every video keyframe, so each chunk begins at the PAT that precedes the
//! keyframe's first packet. Chunks can therefore be dropped from the front of
//! the replay ring and the remaining bytes concatenated into a valid stream.

pub const PACKET: usize = 188;
const SYNC: u8 = 0x47;
const PTS_WRAP: u64 = 1 << 33;

/// A keyframe-aligned slice of the transport stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub data: Vec<u8>,
    /// PTS (90 kHz) of the keyframe that opens the chunk.
    pub pts: Option<u64>,
}

#[derive(Debug, Default)]
pub struct TsChunker {
    /// Bytes of an incomplete trailing packet (or unsynchronised input).
    carry: Vec<u8>,
    /// Complete packets of the chunk being assembled.
    current: Vec<u8>,
    current_pts: Option<u64>,
    /// Offset in `current` of the last PAT packet.
    last_pat: Option<usize>,
    pmt_pid: Option<u16>,
    video_pid: Option<u16>,
    /// Seen the first keyframe; bytes before it are discarded.
    started: bool,
    pub stats: ChunkerStats,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ChunkerStats {
    pub packets: u64,
    pub resyncs: u64,
    pub keyframes: u64,
}

impl TsChunker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The video PID discovered from the PMT, if any.
    pub fn video_pid(&self) -> Option<u16> {
        self.video_pid
    }

    /// Bytes of the chunk currently being assembled (starts at a keyframe once
    /// the first one has been seen). Useful to include the freshest second
    /// of footage when saving a replay.
    pub fn in_progress(&self) -> Option<Chunk> {
        (self.started && !self.current.is_empty()).then(|| Chunk {
            data: self.current.clone(),
            pts: self.current_pts,
        })
    }

    /// Feeds raw bytes and returns every chunk completed by them.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Chunk> {
        let mut out = Vec::new();
        self.carry.extend_from_slice(bytes);
        let buf = std::mem::take(&mut self.carry);
        let mut pos = 0;
        while buf.len() - pos >= PACKET {
            if buf[pos] != SYNC || (buf.len() - pos > PACKET && buf[pos + PACKET] != SYNC) {
                // Lost sync: skip to the next plausible packet start.
                self.stats.resyncs += 1;
                pos += 1;
                while pos < buf.len()
                    && !(buf[pos] == SYNC
                        && (pos + PACKET >= buf.len() || buf[pos + PACKET] == SYNC))
                {
                    pos += 1;
                }
                continue;
            }
            let pkt = &buf[pos..pos + PACKET];
            if let Some(chunk) = self.packet(pkt) {
                out.push(chunk);
            }
            pos += PACKET;
        }
        self.carry = buf[pos..].to_vec();
        out
    }

    /// Ends the stream and returns the trailing partial chunk.
    pub fn finish(&mut self) -> Option<Chunk> {
        let c = self.in_progress();
        self.current.clear();
        self.last_pat = None;
        c
    }

    fn packet(&mut self, pkt: &[u8]) -> Option<Chunk> {
        self.stats.packets += 1;
        let pid = (((pkt[1] & 0x1F) as u16) << 8) | pkt[2] as u16;
        let pusi = pkt[1] & 0x40 != 0;
        let afc = (pkt[3] >> 4) & 0x3;
        let mut payload_off = 4;
        let mut rai = false;
        if afc & 0x2 != 0 {
            let af_len = pkt[4] as usize;
            if af_len > 0 {
                rai = pkt[5] & 0x40 != 0;
            }
            payload_off = 5 + af_len;
        }
        let payload = if afc & 0x1 != 0 && payload_off < PACKET {
            &pkt[payload_off..]
        } else {
            &[][..]
        };

        if pid == 0 && pusi {
            if let Some(p) = parse_pat(payload) {
                self.pmt_pid = Some(p);
            }
        } else if Some(pid) == self.pmt_pid && pusi && self.video_pid.is_none() {
            self.video_pid = parse_pmt_video_pid(payload);
        }

        let mut emitted = None;
        let is_keyframe_start = Some(pid) == self.video_pid && pusi && rai;
        if is_keyframe_start {
            self.stats.keyframes += 1;
            let boundary = self.last_pat.unwrap_or(self.current.len());
            if self.started {
                let tail = self.current.split_off(boundary);
                let head = std::mem::replace(&mut self.current, tail);
                if !head.is_empty() {
                    emitted = Some(Chunk {
                        data: head,
                        pts: self.current_pts,
                    });
                }
            } else {
                self.current.drain(..boundary);
                self.started = true;
            }
            self.current_pts = parse_pes_pts(payload);
            self.last_pat = None;
        }
        if pid == 0 {
            self.last_pat = Some(self.current.len());
        }
        self.current.extend_from_slice(pkt);
        emitted
    }
}

/// Section payload after the pointer field.
fn section(payload: &[u8]) -> Option<&[u8]> {
    let ptr = *payload.first()? as usize;
    payload.get(1 + ptr..)
}

fn parse_pat(payload: &[u8]) -> Option<u16> {
    let s = section(payload)?;
    if *s.first()? != 0x00 {
        return None;
    }
    let len = ((*s.get(1)? as usize & 0x0F) << 8) | *s.get(2)? as usize;
    let end = (3 + len).checked_sub(4)?.min(s.len()); // minus CRC
    let mut i = 8;
    while i + 4 <= end {
        let program = ((s[i] as u16) << 8) | s[i + 1] as u16;
        let pid = (((s[i + 2] & 0x1F) as u16) << 8) | s[i + 3] as u16;
        if program != 0 {
            return Some(pid);
        }
        i += 4;
    }
    None
}

fn parse_pmt_video_pid(payload: &[u8]) -> Option<u16> {
    const VIDEO_TYPES: [u8; 7] = [0x01, 0x02, 0x10, 0x1B, 0x24, 0x33, 0x42];
    let s = section(payload)?;
    if *s.first()? != 0x02 {
        return None;
    }
    let len = ((*s.get(1)? as usize & 0x0F) << 8) | *s.get(2)? as usize;
    let end = (3 + len).checked_sub(4)?.min(s.len());
    let prog_info_len = ((*s.get(10)? as usize & 0x0F) << 8) | *s.get(11)? as usize;
    let mut i = 12 + prog_info_len;
    let mut first = None;
    let mut av1 = None;
    while i + 5 <= end {
        let stream_type = s[i];
        let pid = (((s[i + 1] & 0x1F) as u16) << 8) | s[i + 2] as u16;
        let es_len = ((s[i + 3] as usize & 0x0F) << 8) | s[i + 4] as usize;
        let desc = s.get(i + 5..(i + 5 + es_len).min(end)).unwrap_or(&[]);
        if VIDEO_TYPES.contains(&stream_type) {
            return Some(pid);
        }
        // AV1 is carried as private data with an "AV01" registration descriptor.
        if stream_type == 0x06 && desc.windows(4).any(|w| w == b"AV01") {
            av1 = av1.or(Some(pid));
        }
        first = first.or(Some(pid));
        i += 5 + es_len;
    }
    av1.or(first)
}

fn parse_pes_pts(payload: &[u8]) -> Option<u64> {
    if payload.len() < 14 || payload[..3] != [0, 0, 1] {
        return None;
    }
    let flags = payload[7];
    if flags & 0x80 == 0 {
        return None;
    }
    let p = &payload[9..14];
    let pts = (((p[0] as u64 >> 1) & 0x07) << 30)
        | ((p[1] as u64) << 22)
        | (((p[2] as u64) >> 1) << 15)
        | ((p[3] as u64) << 7)
        | ((p[4] as u64) >> 1);
    Some(pts)
}

/// Difference `b - a` of two 33-bit PTS values, accounting for wrap-around.
pub fn pts_delta(a: u64, b: u64) -> i64 {
    let d = (b.wrapping_sub(a)) % PTS_WRAP;
    if d > PTS_WRAP / 2 {
        d as i64 - PTS_WRAP as i64
    } else {
        d as i64
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a TS packet with optional adaptation field.
    pub fn packet(pid: u16, pusi: bool, rai: bool, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![
            SYNC,
            ((pusi as u8) << 6) | ((pid >> 8) as u8 & 0x1F),
            pid as u8,
        ];
        let mut body = Vec::new();
        if rai {
            p.push(0x30); // adaptation + payload
            body.push(1); // af length
            body.push(0x40); // random access indicator
        } else {
            p.push(0x10);
        }
        body.extend_from_slice(payload);
        body.resize(PACKET - 4, 0xFF);
        p.extend(body);
        p
    }

    fn pat(pmt_pid: u16) -> Vec<u8> {
        // pointer, table 0, len=13, tsid, ver, sec, last, program 1 → pmt, crc
        let mut s = vec![0u8, 0x00, 0xB0, 13, 0, 1, 0xC1, 0, 0, 0, 1];
        s.push(0xE0 | (pmt_pid >> 8) as u8);
        s.push(pmt_pid as u8);
        s.extend([0, 0, 0, 0]);
        packet(0, true, false, &s)
    }

    fn pmt(video: u16, audio: u16) -> Vec<u8> {
        let mut s = vec![0u8, 0x02, 0xB0, 23, 0, 1, 0xC1, 0, 0, 0xE1, 0x00, 0xF0, 0];
        s.extend([0x0F, 0xE0 | (audio >> 8) as u8, audio as u8, 0xF0, 0]); // AAC first
        s.extend([0x1B, 0xE0 | (video >> 8) as u8, video as u8, 0xF0, 0]);
        s.extend([0, 0, 0, 0]);
        packet(0x1000, true, false, &s)
    }

    fn pes(pts: u64) -> Vec<u8> {
        vec![
            0,
            0,
            1,
            0xE0,
            0,
            0,
            0x80,
            0x80,
            5,
            (0x21 | ((pts >> 29) & 0x0E)) as u8,
            (pts >> 22) as u8,
            (((pts >> 14) & 0xFE) | 1) as u8,
            (pts >> 7) as u8,
            (((pts << 1) & 0xFE) | 1) as u8,
        ]
    }

    /// One "GOP": PAT, PMT, keyframe, some video and audio packets.
    pub fn gop(pts: u64, filler: usize) -> Vec<u8> {
        let mut v = pat(0x1000);
        v.extend(pmt(0x100, 0x101));
        v.extend(packet(0x100, true, true, &pes(pts)));
        for i in 0..filler {
            v.extend(packet(
                if i % 3 == 0 { 0x101 } else { 0x100 },
                i % 3 == 0,
                i % 3 == 0,
                &[],
            ));
        }
        v
    }

    #[test]
    fn splits_on_keyframes_and_keeps_pat_pmt() {
        let mut stream = vec![0xAAu8; 50]; // garbage before sync
        stream.extend(packet(0x100, false, false, &[])); // junk before first keyframe
        for k in 0..4 {
            stream.extend(gop(k * 90_000, 10));
        }
        let mut c = TsChunker::new();
        let mut chunks = Vec::new();
        // Feed in awkward slice sizes.
        for part in stream.chunks(77) {
            chunks.extend(c.push(part));
        }
        assert_eq!(c.video_pid(), Some(0x100));
        assert_eq!(chunks.len(), 3);
        for (i, ch) in chunks.iter().enumerate() {
            assert_eq!(ch.data.len(), PACKET * 13);
            assert_eq!(ch.data[0], SYNC);
            assert_eq!(ch.data[1] & 0x1F, 0, "chunk starts with PAT");
            assert_eq!(ch.pts, Some(i as u64 * 90_000));
        }
        let last = c.finish().unwrap();
        assert_eq!(last.pts, Some(270_000));
        assert_eq!(c.stats.keyframes, 4);
        assert!(c.stats.resyncs >= 1);
    }

    #[test]
    fn pts_delta_wraps() {
        assert_eq!(pts_delta(100, 190), 90);
        assert_eq!(pts_delta(PTS_WRAP - 10, 20), 30);
        assert_eq!(pts_delta(20, PTS_WRAP - 10), -30);
    }
}
