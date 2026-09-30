//! Fragmented MP4 writer: one init segment (moov) and one moof+mdat fragment per HLS segment,
//! with the video and audio tracks muxed into a single SourceBuffer.
//!
//! ponytail: video timescale is 90 kHz (same as TS) and audio uses its sample rate. avcC has no
//! High-profile extension bytes, and there are no edit lists (timelines start at ~0 instead).

use crate::ts::AacConfig;

pub const VIDEO_TRACK: u32 = 1;
pub const AUDIO_TRACK: u32 = 2;
pub const VIDEO_TIMESCALE: u32 = 90_000;

fn bx(kind: &[u8; 4], parts: &[&[u8]]) -> Vec<u8> {
    let len = 8 + parts.iter().map(|p| p.len()).sum::<usize>();
    let mut v = Vec::with_capacity(len);
    v.extend((len as u32).to_be_bytes());
    v.extend(kind);
    for p in parts {
        v.extend(*p);
    }
    v
}

fn full(kind: &[u8; 4], version: u8, flags: u32, parts: &[&[u8]]) -> Vec<u8> {
    let head = [
        version,
        (flags >> 16) as u8,
        (flags >> 8) as u8,
        flags as u8,
    ];
    let mut all: Vec<&[u8]> = vec![&head];
    all.extend(parts);
    bx(kind, &all)
}

fn u32s(vals: &[u32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_be_bytes()).collect()
}

const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

fn tkhd(track: u32, audio: bool, w: u32, h: u32) -> Vec<u8> {
    let volume: u16 = if audio { 0x0100 } else { 0 };
    full(
        b"tkhd",
        0,
        3, // enabled + in movie
        &[
            &u32s(&[0, 0, track, 0, 0, 0, 0]), // creation, modification, id, reserved, duration, reserved x2
            &[0, 0, 0, 0],                     // layer, alternate group
            &volume.to_be_bytes(),
            &[0, 0],
            &u32s(&MATRIX),
            &u32s(&[w << 16, h << 16]),
        ],
    )
}

fn mdia(timescale: u32, audio: bool, minf: Vec<u8>) -> Vec<u8> {
    let mdhd = full(
        b"mdhd",
        0,
        0,
        &[&u32s(&[0, 0, timescale, 0]), &[0x55, 0xC4, 0, 0]],
    );
    let (handler, name): (&[u8; 4], &[u8]) = if audio {
        (b"soun", b"SoundHandler\0")
    } else {
        (b"vide", b"VideoHandler\0")
    };
    let hdlr = full(b"hdlr", 0, 0, &[&u32s(&[0]), handler, &[0; 12], name]);
    bx(b"mdia", &[&mdhd, &hdlr, &minf])
}

fn minf(media_header: Vec<u8>, sample_entry: Vec<u8>) -> Vec<u8> {
    let url = full(b"url ", 0, 1, &[]);
    let dinf = bx(b"dinf", &[&full(b"dref", 0, 0, &[&u32s(&[1]), &url])]);
    let empty = |k: &[u8; 4]| full(k, 0, 0, &[&u32s(&[0])]);
    let stbl = bx(
        b"stbl",
        &[
            &full(b"stsd", 0, 0, &[&u32s(&[1]), &sample_entry]),
            &empty(b"stts"),
            &empty(b"stsc"),
            &full(b"stsz", 0, 0, &[&u32s(&[0, 0])]),
            &empty(b"stco"),
        ],
    );
    bx(b"minf", &[&media_header, &dinf, &stbl])
}

/// The `avcC` box for one SPS and one PPS (what a transport stream gives us).
fn avcc_box(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    bx(
        b"avcC",
        &[
            &[1, sps[1], sps[2], sps[3], 0xFF, 0xE1],
            &(sps.len() as u16).to_be_bytes(),
            sps,
            &[1],
            &(pps.len() as u16).to_be_bytes(),
            pps,
        ],
    )
}

fn avc1(avcc: &[u8], w: u16, h: u16, (hs, vs): (u32, u32)) -> Vec<u8> {
    let mut name = [0u8; 32];
    name[0] = 0; // empty compressor name
    let pasp = if hs == vs {
        vec![] // square pixels are the default
    } else {
        bx(b"pasp", &[&hs.to_be_bytes(), &vs.to_be_bytes()])
    };
    bx(
        b"avc1",
        &[
            &[0; 6],
            &1u16.to_be_bytes(), // data reference index
            &[0; 16],            // pre_defined / reserved
            &w.to_be_bytes(),
            &h.to_be_bytes(),
            &0x0048_0000u32.to_be_bytes(), // 72 dpi
            &0x0048_0000u32.to_be_bytes(),
            &[0; 4],
            &1u16.to_be_bytes(), // frame count
            &name,
            &0x0018u16.to_be_bytes(), // depth
            &0xFFFFu16.to_be_bytes(),
            avcc,
            &pasp,
        ],
    )
}

/// ES_Descriptor > DecoderConfigDescriptor > DecoderSpecificInfo (the AudioSpecificConfig), then
/// SLConfig. Every length here stays under 128, so each is one byte.
fn esds(asc: &[u8]) -> Vec<u8> {
    let mut config = vec![0x04, (13 + 2 + asc.len()) as u8, 0x40, 0x15];
    config.extend([0; 11]); // buffer size, maximum and average bit rate: unknown
    config.extend([0x05, asc.len() as u8]);
    config.extend(asc);
    let mut body = vec![0x03, (3 + config.len() + 3) as u8, 0, 0, 0]; // ES_ID 0, no flags
    body.extend(config);
    body.extend([0x06, 0x01, 0x02]);
    full(b"esds", 0, 0, &[&body])
}

fn mp4a(asc: &[u8], channels: u16, rate: u32) -> Vec<u8> {
    bx(
        b"mp4a",
        &[
            &[0; 6],
            &1u16.to_be_bytes(),
            &[0; 8],
            &channels.to_be_bytes(),
            &16u16.to_be_bytes(), // sample size
            &[0; 4],
            // 16.16 fixed point: rates above 65535 Hz don't fit (the real rate lives in the ASC).
            &(rate.min(65535) << 16).to_be_bytes(),
            &esds(asc),
        ],
    )
}

/// `fLaC` entry: FLAC in MP4 (what [`crate::sound`] writes decoded AC-3 and MP2 as).
fn flac_entry(rate: u32) -> Vec<u8> {
    // dfLa: version and flags, then one metadata block: "last" flag, type 0 (STREAMINFO), 34 bytes.
    let dfla = full(
        b"dfLa",
        0,
        0,
        &[&[0x80, 0, 0, 34], &crate::sound::flac_streaminfo(rate)],
    );
    bx(
        b"fLaC",
        &[
            &[0; 6],
            &1u16.to_be_bytes(),
            &[0; 8],
            &2u16.to_be_bytes(), // stereo
            &16u16.to_be_bytes(),
            &[0; 4],
            &(rate.min(65535) << 16).to_be_bytes(),
            &dfla,
        ],
    )
}

/// The audio of an fMP4 stream: AAC as it arrives, or FLAC made from sound we decoded.
pub enum AudioTrack<'a> {
    /// AAC from a transport stream, which only tells us its basic parameters.
    Aac(&'a AacConfig),
    /// AAC from a file, which brings its whole AudioSpecificConfig (HE-AAC needs the extra bytes).
    AacFile {
        asc: &'a [u8],
        channels: u16,
        rate: u32,
    },
    Flac {
        rate: u32,
    },
}

impl AudioTrack<'_> {
    fn timescale(&self) -> u32 {
        match self {
            AudioTrack::Aac(cfg) => cfg.sample_rate(),
            AudioTrack::AacFile { rate, .. } | AudioTrack::Flac { rate } => *rate,
        }
    }

    fn entry(&self) -> Vec<u8> {
        match self {
            AudioTrack::Aac(cfg) => mp4a(&cfg.asc(), u16::from(cfg.channels), cfg.sample_rate()),
            AudioTrack::AacFile {
                asc,
                channels,
                rate,
            } => mp4a(asc, *channels, *rate),
            AudioTrack::Flac { rate } => flac_entry(*rate),
        }
    }
}

pub struct VideoParams<'a> {
    pub sps: &'a [u8],
    pub pps: &'a [u8],
    /// The whole `avcC` payload when a file has one (more than one SPS, the High-profile bytes);
    /// without it a minimal one is built from `sps` and `pps`.
    pub avcc: Option<&'a [u8]>,
    pub width: u32,
    pub height: u32,
    /// Pixel shape (horizontal, vertical spacing); equal numbers mean square.
    pub pixel_aspect: (u32, u32),
}

/// `ftyp` + `moov` for an fMP4 stream. Audio is optional.
pub fn init_segment(video: &VideoParams, audio: Option<&AudioTrack>) -> Vec<u8> {
    let ftyp = bx(
        b"ftyp",
        &[
            b"isom",
            &512u32.to_be_bytes(),
            b"isom",
            b"iso6",
            b"avc1",
            b"mp41",
        ],
    );
    let mvhd = full(
        b"mvhd",
        0,
        0,
        &[
            &u32s(&[0, 0, 1000, 0, 0x0001_0000]), // creation, modification, timescale, duration, rate
            &0x0100u16.to_be_bytes(),             // volume
            &[0; 10],
            &u32s(&MATRIX),
            &[0; 24],
            &u32s(&[if audio.is_some() { 3 } else { 2 }]), // next track id
        ],
    );

    let vtrak = bx(
        b"trak",
        &[
            &tkhd(VIDEO_TRACK, false, video.width, video.height),
            &mdia(
                VIDEO_TIMESCALE,
                false,
                minf(
                    full(b"vmhd", 0, 1, &[&[0; 8]]),
                    avc1(
                        &video
                            .avcc
                            .map_or_else(|| avcc_box(video.sps, video.pps), |a| bx(b"avcC", &[a])),
                        video.width as u16,
                        video.height as u16,
                        video.pixel_aspect,
                    ),
                ),
            ),
        ],
    );
    let mut traks = vtrak;
    let mut trexes = full(b"trex", 0, 0, &[&u32s(&[VIDEO_TRACK, 1, 0, 0, 0])]);
    if let Some(a) = audio {
        traks.extend(bx(
            b"trak",
            &[
                &tkhd(AUDIO_TRACK, true, 0, 0),
                &mdia(
                    a.timescale(),
                    true,
                    minf(full(b"smhd", 0, 0, &[&[0; 4]]), a.entry()),
                ),
            ],
        ));
        trexes.extend(full(b"trex", 0, 0, &[&u32s(&[AUDIO_TRACK, 1, 0, 0, 0])]));
    }
    let moov = bx(b"moov", &[&mvhd, &traks, &bx(b"mvex", &[&trexes])]);
    [ftyp, moov].concat()
}

/// One sample as the muxer needs it. Times are in the track's timescale.
pub struct Sample<'a> {
    pub duration: u32,
    pub key: bool,
    /// Composition offset (pts - dts). Video only.
    pub cts: i32,
    pub data: &'a [u8],
}

pub struct TrackRun<'a> {
    pub track: u32,
    pub base_time: u64,
    pub samples: Vec<Sample<'a>>,
}

const SYNC: u32 = 0x0200_0000; // depends on nothing
const NON_SYNC: u32 = 0x0101_0000;

fn traf(run: &TrackRun, data_offset: i32) -> Vec<u8> {
    let video = run.track == VIDEO_TRACK;
    // data-offset | duration | size | flags [| composition offset]
    let flags = 0x1 | 0x100 | 0x200 | 0x400 | if video { 0x800 } else { 0 };
    let mut trun = vec![];
    trun.extend((run.samples.len() as u32).to_be_bytes());
    trun.extend(data_offset.to_be_bytes());
    for s in &run.samples {
        trun.extend(s.duration.to_be_bytes());
        trun.extend((s.data.len() as u32).to_be_bytes());
        trun.extend((if s.key { SYNC } else { NON_SYNC }).to_be_bytes());
        if video {
            trun.extend(s.cts.to_be_bytes());
        }
    }
    bx(
        b"traf",
        &[
            &full(b"tfhd", 0, 0x02_0000, &[&u32s(&[run.track])]), // offsets are relative to the moof
            &full(b"tfdt", 1, 0, &[&run.base_time.to_be_bytes()]),
            &full(b"trun", 1, flags, &[&trun]),
        ],
    )
}

/// `moof` + `mdat` for the given tracks (in order).
pub fn fragment(sequence: u32, runs: &[TrackRun]) -> Vec<u8> {
    let build = |offsets: &[i32]| {
        let mfhd = full(b"mfhd", 0, 0, &[&u32s(&[sequence])]);
        let trafs: Vec<Vec<u8>> = runs.iter().zip(offsets).map(|(r, &o)| traf(r, o)).collect();
        let mut parts: Vec<&[u8]> = vec![&mfhd];
        parts.extend(trafs.iter().map(|t| t.as_slice()));
        bx(b"moof", &parts)
    };
    // The moof's size doesn't depend on the offsets, so build once to measure, then for real.
    let moof_len = build(&vec![0; runs.len()]).len() as i32;
    let mut offsets = vec![];
    let mut at = moof_len + 8; // past the mdat header
    for r in runs {
        offsets.push(at);
        at += r.samples.iter().map(|s| s.data.len() as i32).sum::<i32>();
    }
    let moof = build(&offsets);
    let payload_len = (at - moof_len - 8) as u32;
    // The whole fragment is known by now: allocate it once, not by doubling.
    let mut out = Vec::with_capacity(moof.len() + 8 + payload_len as usize);
    out.extend(moof);
    out.extend((8 + payload_len).to_be_bytes());
    out.extend(b"mdat");
    for r in runs {
        for s in &r.samples {
            out.extend(s.data);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walks top-level boxes and returns (type, payload) pairs.
    pub fn boxes(mut b: &[u8]) -> Vec<(String, &[u8])> {
        let mut v = vec![];
        while b.len() >= 8 {
            let len = u32::from_be_bytes(b[..4].try_into().unwrap()) as usize;
            assert!(
                len >= 8 && len <= b.len(),
                "box length {len} exceeds remaining {}",
                b.len()
            );
            v.push((String::from_utf8_lossy(&b[4..8]).into_owned(), &b[8..len]));
            b = &b[len..];
        }
        v
    }

    #[test]
    fn fragment_offsets_point_at_sample_data() {
        let (a, b, c) = ([1u8, 2, 3], [4u8, 5], [9u8; 4]);
        let runs = [
            TrackRun {
                track: VIDEO_TRACK,
                base_time: 0,
                samples: vec![
                    Sample {
                        duration: 3000,
                        key: true,
                        cts: 0,
                        data: &a,
                    },
                    Sample {
                        duration: 3000,
                        key: false,
                        cts: 6000,
                        data: &b,
                    },
                ],
            },
            TrackRun {
                track: AUDIO_TRACK,
                base_time: 5,
                samples: vec![Sample {
                    duration: 1024,
                    key: true,
                    cts: 0,
                    data: &c,
                }],
            },
        ];
        let f = fragment(7, &runs);
        let top = boxes(&f);
        assert_eq!(
            top.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["moof", "mdat"]
        );
        assert_eq!(top[1].1, [1, 2, 3, 4, 5, 9, 9, 9, 9]);

        // data_offset in the first trun must land on the first byte of mdat's payload.
        let moof_len = 8 + top[0].1.len();
        let moof = &top[0].1;
        let pos = moof.windows(4).position(|w| w == b"trun").unwrap();
        let off = i32::from_be_bytes(moof[pos + 12..pos + 16].try_into().unwrap()) as usize;
        assert_eq!(off, moof_len + 8);
        assert_eq!(f[off], 1);
    }

    #[test]
    fn init_segment_is_well_formed() {
        let sps = [0x67, 0x64, 0x00, 0x1f, 0xac];
        let v = VideoParams {
            sps: &sps,
            pps: &[0x68, 0xee],
            avcc: None,
            width: 848,
            height: 480,
            pixel_aspect: (1, 1),
        };
        let init = init_segment(
            &v,
            Some(&AudioTrack::Aac(&AacConfig {
                object_type: 2,
                freq_index: 4,
                channels: 2,
            })),
        );
        let top = boxes(&init);
        assert_eq!(
            top.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["ftyp", "moov"]
        );
        let moov = boxes(top[1].1);
        assert_eq!(
            moov.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["mvhd", "trak", "trak", "mvex"]
        );
    }
}
