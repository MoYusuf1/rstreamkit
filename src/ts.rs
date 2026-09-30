//! MPEG-TS demuxer for one HLS segment: H.264 video + AAC (ADTS) audio.
//!
//! ponytail: a segment is assumed to be self-contained (starts with PAT/PMT, PES packets don't
//! straddle segments), PSI sections fit in one packet, and one video PES carries one frame.
//! All true for normal HLS output; continuous `.ts` streams would need incremental input.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("not an MPEG-TS segment (no sync bytes)")]
    NoSync,
    #[error("segment has no program table")]
    NoPmt,
    #[error("no H.264 video in the segment{}", .0.as_ref().map(|c| format!(" (found {c}, which browsers can't play through MediaSource here)")).unwrap_or_default())]
    NoVideo(Option<String>),
    #[error("video has no SPS/PPS in this segment")]
    NoVideoParams,
    #[error("bad H.264 parameter set: {0}")]
    BadSps(String),
}

/// Timestamps are the raw 33-bit, 90 kHz values from the PES headers.
#[derive(Debug, Clone)]
pub struct VideoSample {
    pub dts: u64,
    pub pts: u64,
    pub key: bool,
    /// AVCC layout: each NAL prefixed by its 4-byte big-endian length.
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct AudioSample {
    pub pts: u64,
    /// One raw AAC frame (ADTS header removed), 1024 samples.
    pub data: Vec<u8>,
}

/// One frame of sound we decode ourselves (AC-3, E-AC-3, MP2); see [`crate::sound`].
#[derive(Debug, Clone)]
pub struct SoundSample {
    pub pts: u64,
    pub data: Vec<u8>,
    /// Samples per channel it decodes to.
    pub samples: u32,
    pub rate: u32,
    pub channels: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AacConfig {
    pub object_type: u8,
    pub freq_index: u8,
    pub channels: u8,
}

const RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

impl AacConfig {
    pub fn sample_rate(&self) -> u32 {
        RATES
            .get(self.freq_index as usize)
            .copied()
            .unwrap_or(48000)
    }

    /// The 2-byte AudioSpecificConfig that goes in `esds`.
    pub fn asc(&self) -> [u8; 2] {
        let v = (self.object_type as u16) << 11
            | (self.freq_index as u16) << 7
            | (self.channels as u16) << 3;
        v.to_be_bytes()
    }
}

#[derive(Debug, Default)]
pub struct Demuxed {
    pub video: Vec<VideoSample>,
    pub audio: Vec<AudioSample>,
    /// NAL units including their header byte, no start code.
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
    pub aac: Option<AacConfig>,
    /// Sound we have a decoder for, when there is no AAC.
    pub sound: Vec<SoundSample>,
    pub sound_kind: Option<crate::sound::Kind>,
    /// Set when the segment has audio a browser can't play itself (AC-3, MP2, ...). It is dropped
    /// unless `sound` holds its decoded frames, which the transmuxer then plays instead.
    pub skipped_audio: Option<String>,
}

fn codec_name(stream_type: u8) -> String {
    match stream_type {
        0x01 | 0x02 => "MPEG-2 video".into(),
        0x03 | 0x04 => "MPEG audio (MP2/MP3)".into(),
        0x06 | 0x81 => "AC-3".into(),
        0x87 => "E-AC-3".into(),
        0x24 => "HEVC".into(),
        t => format!("stream type 0x{t:02x}"),
    }
}

/// Splits a PES payload of Annex-B data into NAL units (without start codes).
fn nal_units(es: &[u8]) -> Vec<&[u8]> {
    let mut starts = vec![];
    let mut i = 0;
    while i + 3 <= es.len() {
        if es[i] == 0 && es[i + 1] == 0 && es[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = vec![];
    for (n, &s) in starts.iter().enumerate() {
        let mut e = starts.get(n + 1).map_or(es.len(), |&next| next - 3);
        // A 4-byte start code leaves one extra zero on the previous NAL; trailing zeros are never payload.
        while e > s && es[e - 1] == 0 {
            e -= 1;
        }
        if e > s {
            out.push(&es[s..e]);
        }
    }
    out
}

pub(crate) fn read_ts(b: &[u8]) -> u64 {
    (((b[0] as u64 >> 1) & 7) << 30)
        | ((b[1] as u64) << 22)
        | ((b[2] as u64 >> 1) << 15)
        | ((b[3] as u64) << 7)
        | (b[4] as u64 >> 1)
}

/// Returns (pts, dts, elementary-stream bytes) for one reassembled PES packet.
fn parse_pes(buf: &[u8]) -> Option<(u64, u64, &[u8])> {
    if buf.len() < 9 || buf[..3] != [0, 0, 1] {
        return None;
    }
    let start = 9 + buf[8] as usize;
    let flags = buf[7] >> 6;
    if flags & 2 == 0 || start > buf.len() || (flags == 3 && start < 19) || start < 14 {
        return None;
    }
    let pts = read_ts(&buf[9..14]);
    let dts = if flags == 3 {
        read_ts(&buf[14..19])
    } else {
        pts
    };
    Some((pts, dts, &buf[start..]))
}

fn adts_frames(es: &[u8], pts: u64, out: &mut Demuxed) {
    let (mut i, mut n) = (0, 0u64);
    while i + 7 <= es.len() {
        if es[i] != 0xFF || es[i + 1] & 0xF0 != 0xF0 {
            i += 1; // resync
            continue;
        }
        let hdr = if es[i + 1] & 1 == 1 { 7 } else { 9 };
        let len = ((es[i + 3] as usize & 3) << 11)
            | ((es[i + 4] as usize) << 3)
            | (es[i + 5] as usize >> 5);
        if len < hdr || i + len > es.len() {
            break;
        }
        let cfg = AacConfig {
            object_type: (es[i + 2] >> 6) + 1,
            freq_index: (es[i + 2] >> 2) & 0xF,
            channels: ((es[i + 2] & 1) << 2) | (es[i + 3] >> 6),
        };
        let out_cfg = *out.aac.get_or_insert(cfg);
        let rate = out_cfg.sample_rate() as u64;
        out.audio.push(AudioSample {
            pts: pts + n * 1024 * 90_000 / rate,
            data: es[i + hdr..i + len].to_vec(),
        });
        n += 1;
        i += len;
    }
}

fn sound_frames(es: &[u8], pts: u64, kind: crate::sound::Kind, out: &mut Demuxed) {
    let mut elapsed = 0u64; // samples since the PES packet's own timestamp
    for frame in crate::sound::split(kind, es) {
        out.sound.push(SoundSample {
            pts: pts + elapsed * 90_000 / u64::from(frame.rate),
            data: frame.data.to_vec(),
            samples: frame.samples,
            rate: frame.rate,
            channels: frame.channels,
        });
        elapsed += u64::from(frame.samples);
    }
}

fn video_sample(es: &[u8], pts: u64, dts: u64, out: &mut Demuxed) {
    // AVCC is the same size as Annex B give or take a byte per NAL unit: allocate it once.
    let (mut data, mut key) = (Vec::with_capacity(es.len() + 8), false);
    for nal in nal_units(es) {
        match nal[0] & 0x1F {
            7 => drop(out.sps.get_or_insert_with(|| nal.to_vec())),
            8 => drop(out.pps.get_or_insert_with(|| nal.to_vec())),
            9 => {} // access unit delimiter
            t => {
                key |= t == 5;
                data.extend((nal.len() as u32).to_be_bytes());
                data.extend(nal);
            }
        }
    }
    if !data.is_empty() {
        out.video.push(VideoSample {
            dts,
            pts,
            key,
            data,
        });
    }
}

/// The tags of a run of PMT descriptors.
fn descriptor_tags(mut d: Option<&[u8]>) -> Vec<u8> {
    let mut tags = vec![];
    while let Some([tag, len, rest @ ..]) = d {
        tags.push(*tag);
        d = rest.get(*len as usize..);
    }
    tags
}

fn find_sync(data: &[u8]) -> Option<usize> {
    (0..188.min(data.len())).find(|&o| {
        (0..3).all(|k| data.get(o + k * 188).is_none_or(|&b| b == 0x47))
            && data.get(o) == Some(&0x47)
    })
}

pub fn demux(data: &[u8]) -> Result<Demuxed, Error> {
    let start = find_sync(data).ok_or(Error::NoSync)?;
    let mut out = Demuxed::default();
    let (mut pmt_pid, mut video_pid, mut audio_pid) = (None, None, None);
    let mut sound = None::<(u16, crate::sound::Kind)>;
    let mut undecodable_audio = None;
    let mut foreign_video = None;
    let mut pes: HashMap<u16, Vec<u8>> = HashMap::new();

    // Emit a finished PES packet for the given PID.
    fn flush(
        pid: u16,
        buf: &[u8],
        video_pid: Option<u16>,
        sound: Option<(u16, crate::sound::Kind)>,
        out: &mut Demuxed,
    ) {
        let Some((pts, dts, es)) = parse_pes(buf) else {
            return;
        };
        if Some(pid) == video_pid {
            video_sample(es, pts, dts, out);
        } else if let Some((_, kind)) = sound.filter(|(p, _)| *p == pid) {
            sound_frames(es, pts, kind, out);
        } else {
            adts_frames(es, pts, out);
        }
    }

    for pkt in data[start..].as_chunks::<188>().0 {
        if pkt[0] != 0x47 {
            continue;
        }
        let pusi = pkt[1] & 0x40 != 0;
        let pid = (((pkt[1] & 0x1F) as u16) << 8) | pkt[2] as u16;
        let afc = (pkt[3] >> 4) & 3;
        let off = if afc & 2 != 0 { 5 + pkt[4] as usize } else { 4 };
        if afc & 1 == 0 || off >= 188 {
            continue;
        }
        let payload = &pkt[off..];

        if pid == 0 && pusi {
            // PAT: first real program's PMT PID.
            let s = &payload[1 + payload[0] as usize..];
            if s.len() >= 12 {
                let end = (3 + (((s[1] & 0x0F) as usize) << 8 | s[2] as usize))
                    .min(s.len())
                    .saturating_sub(4);
                pmt_pid = s
                    .get(8..end)
                    .unwrap_or(&[])
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .find(|e| e[0] != 0 || e[1] != 0)
                    .map(|e| ((e[2] & 0x1F) as u16) << 8 | e[3] as u16);
            }
        } else if Some(pid) == pmt_pid && pusi {
            let s = &payload[1 + payload[0] as usize..];
            if s.len() >= 12 {
                let end = (3 + (((s[1] & 0x0F) as usize) << 8 | s[2] as usize))
                    .min(s.len())
                    .saturating_sub(4);
                let mut i = 12 + (((s[10] & 0x0F) as usize) << 8 | s[11] as usize);
                while i + 5 <= end {
                    let (ty, es_pid) = (s[i], ((s[i + 1] & 0x1F) as u16) << 8 | s[i + 2] as u16);
                    let info_len = ((s[i + 3] & 0x0F) as usize) << 8 | s[i + 4] as usize;
                    // Descriptor tags of this stream: private data (0x06) is AC-3 only if one says so.
                    let tags = descriptor_tags(s.get(i + 5..(i + 5 + info_len).min(s.len())));
                    let ac3_tag = tags.iter().any(|t| matches!(t, 0x6A | 0x7A | 0x81));
                    match ty {
                        0x1B if video_pid.is_none() => video_pid = Some(es_pid),
                        0x0F if audio_pid.is_none() => audio_pid = Some(es_pid),
                        0x1B | 0x0F => {}
                        0x01 | 0x02 | 0x24 => foreign_video = Some(codec_name(ty)),
                        0x81 | 0x87 if sound.is_none() => {
                            sound = Some((es_pid, crate::sound::Kind::Ac3))
                        }
                        0x06 if sound.is_none() && ac3_tag => {
                            sound = Some((es_pid, crate::sound::Kind::Ac3))
                        }
                        0x03 | 0x04 if sound.is_none() => {
                            sound = Some((es_pid, crate::sound::Kind::Mpeg))
                        }
                        _ => {}
                    }
                    if matches!(ty, 0x03 | 0x04 | 0x81 | 0x87) || (ty == 0x06 && ac3_tag) {
                        undecodable_audio.get_or_insert_with(|| codec_name(ty));
                    }
                    i += 5 + info_len;
                }
            }
        } else if Some(pid) == video_pid
            || Some(pid) == audio_pid
            || sound.is_some_and(|(p, _)| p == pid)
        {
            if pusi {
                // One buffer per stream, reused for every packet of it: after the largest frame
                // there is nothing left to allocate.
                let buf = pes.entry(pid).or_default();
                flush(pid, buf, video_pid, sound, &mut out);
                buf.clear();
                buf.extend(payload);
            } else if let Some(buf) = pes.get_mut(&pid) {
                buf.extend(payload);
            }
        }
    }
    for (pid, buf) in &pes {
        flush(*pid, buf, video_pid, sound, &mut out);
    }

    if pmt_pid.is_none() {
        return Err(Error::NoPmt);
    }
    if video_pid.is_none() {
        return Err(Error::NoVideo(foreign_video));
    }
    // HashMap flush order is arbitrary and PES arrive interleaved; restore decode order.
    out.video.sort_by_key(|s| s.dts);
    out.audio.sort_by_key(|s| s.pts);
    out.sound.sort_by_key(|s| s.pts);
    if audio_pid.is_some() {
        // AAC plays as it is; it wins over anything else the stream carries.
        out.sound.clear();
    } else {
        // Audio a browser can't play itself: named, so it can be said what was left out, or
        // cleared by the caller when it decodes the sound instead.
        out.skipped_audio = undecodable_audio;
        if !out.sound.is_empty() {
            out.sound_kind = sound.map(|(_, kind)| kind);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_annexb_with_both_start_code_lengths() {
        let es = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9,
        ];
        let n: Vec<_> = nal_units(&es).into_iter().map(|n| n.to_vec()).collect();
        assert_eq!(n, vec![vec![0x67, 1, 2], vec![0x68, 3], vec![0x65, 9, 9]]);
    }

    #[test]
    fn timestamps_and_asc() {
        // PTS = 0x1_2345_6789 encoded in 5 bytes with marker bits.
        let pts: u64 = 0x1_2345_6789 & 0x1_FFFF_FFFF;
        let b = [
            0x21 | (((pts >> 30) & 7) as u8) << 1,
            (pts >> 22) as u8,
            (((pts >> 15) & 0x7F) as u8) << 1 | 1,
            (pts >> 7) as u8,
            ((pts & 0x7F) as u8) << 1 | 1,
        ];
        assert_eq!(read_ts(&b), pts);
        // AAC-LC, 44.1 kHz, stereo -> 0x1210.
        assert_eq!(
            AacConfig {
                object_type: 2,
                freq_index: 4,
                channels: 2
            }
            .asc(),
            [0x12, 0x10]
        );
    }

    #[test]
    fn rejects_non_ts() {
        assert_eq!(
            demux(b"definitely not transport stream data").unwrap_err(),
            Error::NoSync
        );
    }
}
