//! Sound browsers can't decode: AC-3, E-AC-3 and MPEG audio (MP2). It is decoded here, in Rust, to
//! PCM and written back as FLAC frames, which every browser plays inside the same fragmented MP4
//! as the picture, so the two stay in step with no extra machinery. (FLAC because it needs no
//! encoder: a frame can carry the samples as they are.)
//!
//! ponytail: the result is always stereo, 16-bit. Surround is folded down with the usual -3 dB
//! centre and surround mix and the LFE channel is dropped; E-AC-3 dependent substreams (the extra
//! channels of 7.1) are skipped.

use oxideav_core::{CodecId, CodecParameters, Decoder as CodecDecoder, Frame, Packet, TimeBase};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// AC-3 and E-AC-3; which one is told apart frame by frame.
    Ac3,
    /// MPEG audio layer II (what broadcasters send as "MP2").
    Mpeg,
}

/// One compressed frame and what it decodes to.
#[derive(Debug)]
pub struct Coded<'a> {
    pub data: &'a [u8],
    /// Samples per channel it decodes to.
    pub samples: u32,
    pub rate: u32,
    pub channels: u8,
}

/// Cuts a PES payload into frames. Anything that isn't a frame is stepped over.
pub fn split(kind: Kind, es: &[u8]) -> Vec<Coded<'_>> {
    match kind {
        Kind::Ac3 => split_ac3(es),
        Kind::Mpeg => split_mpeg(es),
    }
}

fn split_ac3(es: &[u8]) -> Vec<Coded<'_>> {
    let mut out = vec![];
    let mut i = 0;
    while i + 8 <= es.len() {
        if es[i] != 0x0B || es[i + 1] != 0x77 {
            i += 1;
            continue;
        }
        let bsid = es[i + 5] >> 3;
        // (frame bytes, samples per channel, rate, worth decoding)
        let frame = if bsid <= 10 {
            match oxideav_ac3::syncinfo::parse(&es[i..]) {
                Ok(si) => Some((si.frame_length as usize, 1536, si.sample_rate, true)),
                Err(_) => None,
            }
        } else {
            // E-AC-3: strmtyp(2) substreamid(3) frmsiz(11), then fscod(2) and numblkscod(2).
            let len = ((((es[i + 2] & 7) as usize) << 8 | es[i + 3] as usize) + 1) * 2;
            let (fscod, second) = (es[i + 4] >> 6, (es[i + 4] >> 4) & 3);
            let (rate, blocks) = if fscod == 3 {
                ([24_000, 22_050, 16_000, 0][second as usize], 6)
            } else {
                (
                    [48_000, 44_100, 32_000][fscod as usize],
                    [1, 2, 3, 6][second as usize],
                )
            };
            // Stream type 1 is a dependent substream: extra channels for a 7.1 decoder.
            Some((len, 256 * blocks, rate, es[i + 2] >> 6 != 1 && rate != 0))
        };
        let Some((len, samples, rate, decode)) = frame else {
            i += 1;
            continue;
        };
        if len < 8 || i + len > es.len() {
            break;
        }
        if decode {
            out.push(Coded {
                data: &es[i..i + len],
                samples,
                rate,
                channels: 2,
            });
        }
        i += len;
    }
    out
}

fn split_mpeg(es: &[u8]) -> Vec<Coded<'_>> {
    use oxideav_mp2::header::FrameHeader;
    let mut out = vec![];
    let mut i = 0;
    while i + 4 <= es.len() {
        match FrameHeader::parse(&es[i..]) {
            Ok(h) if i + h.frame_size_bytes() <= es.len() => {
                let len = h.frame_size_bytes();
                out.push(Coded {
                    data: &es[i..i + len],
                    samples: h.samples_per_channel() as u32,
                    rate: h.sample_rate,
                    channels: h.channels() as u8,
                });
                i += len;
            }
            Ok(_) => break,
            Err(_) => i += 1,
        }
    }
    out
}

pub struct Decoder {
    ac3: Option<Box<dyn CodecDecoder>>,
    mpeg: Option<Box<dyn CodecDecoder>>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            ac3: None,
            mpeg: None,
        }
    }

    /// Stereo samples (interleaved) for one frame. A frame that can't be decoded comes back as
    /// silence of the right length, so the timeline holds instead of the sound running early.
    pub fn decode(&mut self, kind: Kind, frame: &Coded) -> Vec<i16> {
        let silence = || vec![0; frame.samples as usize * 2];
        let (slot, name) = match kind {
            Kind::Ac3 => (&mut self.ac3, "ac3"),
            Kind::Mpeg => (&mut self.mpeg, "mp2"),
        };
        if slot.is_none() {
            let mut params = CodecParameters::audio(CodecId::new(name));
            params.channels = Some(u16::from(frame.channels.max(1)));
            let made = match kind {
                Kind::Ac3 => oxideav_ac3::decoder::make_decoder(&params),
                Kind::Mpeg => oxideav_mp2::codec_decoder::make_decoder(&params),
            };
            *slot = made.ok();
        }
        let Some(decoder) = slot.as_mut() else {
            return silence();
        };
        let packet = Packet::new(
            0,
            TimeBase::new(1, i64::from(frame.rate)),
            frame.data.to_vec(),
        );
        if decoder.send_packet(&packet).is_err() {
            return silence();
        }
        match decoder.receive_frame() {
            Ok(Frame::Audio(a)) => match a.data.as_slice() {
                // One buffer per channel (MP2 does it this way): weave the first two together.
                [left, right, ..] => weave(left, right, a.samples as usize),
                // One buffer with the channels interleaved (AC-3).
                [all] => stereo(all, a.samples as usize),
                [] => None,
            }
            .unwrap_or_else(silence),
            _ => silence(),
        }
    }
}

/// Two channel buffers of 16-bit samples, woven into one stereo buffer.
fn weave(left: &[u8], right: &[u8], samples: usize) -> Option<Vec<i16>> {
    if samples == 0 || left.len() < samples * 2 || right.len() < samples * 2 {
        return None;
    }
    let sample = |b: &[u8], i: usize| i16::from_le_bytes([b[i * 2], b[i * 2 + 1]]);
    Some(
        (0..samples)
            .flat_map(|i| [sample(left, i), sample(right, i)])
            .collect(),
    )
}

/// Interleaved 16-bit samples, however many channels, folded to stereo.
fn stereo(bytes: &[u8], samples: usize) -> Option<Vec<i16>> {
    if samples == 0 || bytes.len() < samples * 2 {
        return None;
    }
    let channels = bytes.len() / (samples * 2);
    let pcm: Vec<i16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| i16::from_le_bytes(b))
        .collect();
    let mut out = Vec::with_capacity(samples * 2);
    match channels {
        1 => pcm.iter().for_each(|&x| out.extend([x, x])),
        2 => out = pcm,
        // 5.1 in WAVE order: front left and right, centre, LFE, back left and right. The gain keeps
        // the loudest case (everything at full scale in one place) from clipping.
        6 => {
            const SIDE: f32 = std::f32::consts::FRAC_1_SQRT_2;
            const GAIN: f32 = 1.0 / (1.0 + 2.0 * SIDE);
            for f in pcm.as_chunks::<6>().0 {
                let (l, r, c, back_l, back_r) = (f[0], f[1], f[2], f[4], f[5]);
                let mix = |front: i16, back: i16| {
                    ((f32::from(front) + SIDE * f32::from(c) + SIDE * f32::from(back)) * GAIN)
                        as i16
                };
                out.extend([mix(l, back_l), mix(r, back_r)]);
            }
        }
        n => pcm.chunks_exact(n).for_each(|f| out.extend([f[0], f[1]])),
    }
    Some(out)
}

/// CRC-8 (polynomial 0x07) as FLAC frame headers use it.
fn crc8(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |mut crc, &b| {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
        crc
    })
}

/// CRC-16 (polynomial 0x8005) as FLAC frames use it.
fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0u16, |mut crc, &b| {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
        crc
    })
}

/// FLAC's "UTF-8" number: how a frame says which frame it is.
fn coded_number(n: u32) -> Vec<u8> {
    if n < 0x80 {
        return vec![n as u8];
    }
    // A lead byte (`110…`, `1110…`, … carrying the top bits), then 6 bits per continuation byte.
    let extra = if n < 0x800 {
        1
    } else if n < 0x1_0000 {
        2
    } else if n < 0x20_0000 {
        3
    } else if n < 0x400_0000 {
        4
    } else {
        5
    };
    let mut out = vec![((0xFF00u16 >> (extra + 1)) as u8) | (n >> (6 * extra)) as u8];
    for k in (0..extra).rev() {
        out.push(0x80 | ((n >> (6 * k)) & 0x3F) as u8);
    }
    out
}

/// One FLAC frame holding stereo 16-bit samples as they are ("verbatim" subframes). `number` counts
/// the frames of the stream.
pub fn flac_frame(pcm: &[i16], number: u32) -> Vec<u8> {
    let samples = pcm.len() / 2;
    let block_code: u8 = match samples {
        192 => 1,
        576 => 2,
        1152 => 3,
        2304 => 4,
        4608 => 5,
        256 => 8,
        512 => 9,
        1024 => 10,
        2048 => 11,
        4096 => 12,
        8192 => 13,
        16384 => 14,
        32768 => 15,
        n if n <= 256 => 6,
        _ => 7,
    };
    let mut frame = vec![
        0xFF,
        0xF8,            // sync, fixed block size
        block_code << 4, // sample rate: from the stream info
        0x18,            // two independent channels, 16 bits per sample
    ];
    frame.extend(coded_number(number));
    match block_code {
        6 => frame.push((samples - 1) as u8),
        7 => frame.extend(((samples - 1) as u16).to_be_bytes()),
        _ => {}
    }
    frame.push(crc8(&frame));
    for channel in 0..2 {
        frame.push(0x02); // subframe header: verbatim, no wasted bits
        for pair in pcm.as_chunks::<2>().0 {
            frame.extend(pair[channel].to_be_bytes());
        }
    }
    let crc = crc16(&frame);
    frame.extend(crc.to_be_bytes());
    frame
}

/// The FLAC STREAMINFO block (what `dfLa` carries): stereo, 16-bit, any block size.
pub fn flac_streaminfo(rate: u32) -> [u8; 34] {
    let mut info = [0u8; 34];
    info[..2].copy_from_slice(&16u16.to_be_bytes()); // smallest block
    info[2..4].copy_from_slice(&u16::MAX.to_be_bytes()); // largest block
    // Frame sizes unknown (0), then rate(20) channels-1(3) bits-1(5) total samples(36).
    let packed: u64 = u64::from(rate) << 44 | 1 << 41 | 15 << 36;
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flac_numbers_use_the_utf8_style_coding() {
        assert_eq!(coded_number(0), [0x00]);
        assert_eq!(coded_number(127), [0x7F]);
        assert_eq!(coded_number(128), [0xC2, 0x80]);
        assert_eq!(coded_number(0x7FF), [0xDF, 0xBF]);
        assert_eq!(coded_number(0x800), [0xE0, 0xA0, 0x80]);
        assert_eq!(coded_number(0x1_0000), [0xF0, 0x90, 0x80, 0x80]);
    }

    #[test]
    fn crcs_match_the_published_check_values() {
        // "123456789" is the standard check string: CRC-8/SMBUS is 0xF4, CRC-16/UMTS (0x8005) is 0xFEE8.
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
    }

    #[test]
    fn a_flac_frame_has_the_right_shape() {
        let pcm: Vec<i16> = (0..1152 * 2).map(|i| (i % 300) as i16 - 150).collect();
        let f = flac_frame(&pcm, 5);
        assert_eq!(
            &f[..4],
            [0xFF, 0xF8, 0x30, 0x18],
            "sync, 1152-sample block, stereo 16-bit"
        );
        assert_eq!(f[4], 5, "frame number");
        assert_eq!(f[5], crc8(&f[..5]), "header CRC");
        assert_eq!(f.len(), 6 + 2 * (1 + 1152 * 2) + 2);
        let body = f.len() - 2;
        assert_eq!(
            u16::from_be_bytes([f[body], f[body + 1]]),
            crc16(&f[..body]),
            "frame CRC"
        );
        // A block size with no code of its own is spelled out.
        let odd = flac_frame(&vec![0; 1536 * 2], 0);
        assert_eq!(odd[2] >> 4, 7);
        assert_eq!(u16::from_be_bytes([odd[5], odd[6]]), 1535);
    }

    #[test]
    fn streaminfo_packs_rate_channels_and_depth() {
        let s = flac_streaminfo(48_000);
        assert_eq!(u16::from_be_bytes([s[0], s[1]]), 16);
        assert_eq!(u16::from_be_bytes([s[2], s[3]]), 65535);
        let packed = u64::from_be_bytes(s[10..18].try_into().unwrap());
        assert_eq!(
            (packed >> 44, (packed >> 41) & 7, (packed >> 36) & 31),
            (48_000, 1, 15)
        );
    }

    #[test]
    fn separate_channel_buffers_are_woven_into_stereo() {
        let left: Vec<u8> = [1i16, 2, 3].iter().flat_map(|s| s.to_le_bytes()).collect();
        let right: Vec<u8> = [-1i16, -2, -3]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        assert_eq!(weave(&left, &right, 3).unwrap(), [1, -1, 2, -2, 3, -3]);
        assert_eq!(
            weave(&left, &right[..4], 3),
            None,
            "a short buffer is refused"
        );
    }

    #[test]
    fn surround_folds_to_stereo_without_clipping() {
        // Six channels at full scale everywhere: the mix must stay in range.
        let loud = [i16::MAX; 6 * 4];
        let bytes: Vec<u8> = loud.iter().flat_map(|s| s.to_le_bytes()).collect();
        let out = stereo(&bytes, 4).unwrap();
        assert_eq!(out.len(), 8);
        assert!(out.iter().all(|&s| s > 0));
        // Mono is doubled; stereo is untouched.
        assert_eq!(stereo(&[1, 0, 2, 0], 2).unwrap(), [1, 1, 2, 2]);
        assert_eq!(stereo(&[1, 0, 2, 0, 3, 0, 4, 0], 2).unwrap(), [1, 2, 3, 4]);
    }
}
