//! Plain MP4 files. The index in `moov` says where every frame is, so any moment of the movie can
//! be read with one range request and cut into frames without looking at the rest of the file.
//!
//! ponytail: fragmented files (no sample tables), edit lists beyond the first entry, and sample
//! entries other than AVC, HEVC, AAC, AC-3 and E-AC-3 are not understood; such a file is reported
//! as something else should play (see `vod::Verdict`).

use crate::{
    Error,
    vod::{Audio, AudioInfo, Frame, Track, VideoInfo},
};

fn damaged() -> Error {
    Error::Unsupported("the MP4 index is damaged".into())
}

type R<T> = Result<T, Error>;

fn be16(b: &[u8], at: usize) -> R<u16> {
    Ok(u16::from_be_bytes(
        b.get(at..at + 2).ok_or_else(damaged)?.try_into().unwrap(),
    ))
}

fn be32(b: &[u8], at: usize) -> R<u32> {
    Ok(u32::from_be_bytes(
        b.get(at..at + 4).ok_or_else(damaged)?.try_into().unwrap(),
    ))
}

fn be64(b: &[u8], at: usize) -> R<u64> {
    Ok(u64::from_be_bytes(
        b.get(at..at + 8).ok_or_else(damaged)?.try_into().unwrap(),
    ))
}

/// `(kind, size, header length)` of the box at the start of `b`. A size of 0 means "to the end of
/// the file" and comes back as `u64::MAX`.
pub fn box_header(b: &[u8]) -> Option<([u8; 4], u64, usize)> {
    let size = u64::from(u32::from_be_bytes(b.get(..4)?.try_into().ok()?));
    let kind: [u8; 4] = b.get(4..8)?.try_into().ok()?;
    match size {
        0 => Some((kind, u64::MAX, 8)),
        1 => Some((kind, u64::from_be_bytes(b.get(8..16)?.try_into().ok()?), 16)),
        n if n >= 8 => Some((kind, n, 8)),
        _ => None,
    }
}

/// The boxes inside `b`, as (kind, payload).
fn boxes(mut b: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    std::iter::from_fn(move || {
        let (kind, size, head) = box_header(b)?;
        let size = if size == u64::MAX {
            b.len()
        } else {
            usize::try_from(size).ok()?
        };
        if size < head || size > b.len() {
            return None;
        }
        let payload = &b[head..size];
        b = &b[size..];
        Some((kind, payload))
    })
}

fn child<'a>(b: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(b).find(|(k, _)| k == kind).map(|(_, p)| p)
}

/// Where the `moov` box is, given the bytes of the file from offset `at`.
#[derive(Debug, PartialEq)]
pub enum Moov {
    /// Its offset and its whole length (header included).
    At(u64, u64),
    /// Not in these bytes; look at the next box header at this offset.
    Next(u64),
    /// The file has none.
    Missing,
}

pub fn find_moov(bytes: &[u8], at: u64) -> Moov {
    let mut pos = 0usize;
    loop {
        let Some((kind, size, _)) = box_header(&bytes[pos.min(bytes.len())..]) else {
            return Moov::Next(at + pos as u64);
        };
        if &kind == b"moov" {
            return Moov::At(at + pos as u64, size);
        }
        if size == u64::MAX {
            return Moov::Missing;
        }
        match usize::try_from(size).ok().and_then(|s| pos.checked_add(s)) {
            Some(next) if next < bytes.len() => pos = next,
            _ => return Moov::Next((at + pos as u64).saturating_add(size)),
        }
    }
}

/// The top bit of a sample's `size_key`: it is a sync sample (a picture to start from).
const KEY: u32 = 1 << 31;

/// One frame of a track, straight from the sample tables: 24 bytes, because a long film has
/// hundreds of thousands of them (a 2-hour one, about 14 MB instead of 28).
struct Sample {
    offset: u64,
    /// Ticks of the track's timescale.
    dts: i64,
    /// The size in the low 31 bits, and the sync flag in the top one.
    size_key: u32,
    cts: i32,
}

impl Sample {
    fn size(&self) -> u32 {
        self.size_key & !KEY
    }

    fn key(&self) -> bool {
        self.size_key & KEY != 0
    }
}

struct Trak {
    samples: Vec<Sample>,
    /// How long the last sample lasts, in ticks.
    last_dur: u32,
    scale: u32,
    /// Where the first edit starts in the media, in ticks: what the player should skip.
    edit: i64,
}

impl Trak {
    /// Ticks sample `i` lasts: until the next one begins.
    fn dur(&self, i: u32) -> i64 {
        let s = &self.samples[i as usize];
        match self.samples.get(i as usize + 1) {
            Some(next) => next.dts - s.dts,
            None => i64::from(self.last_dur),
        }
    }
}

/// The sample tables of one track: for every frame its place in the file, time and kind, and how
/// long the last one lasts (each of the others lasts until the next begins).
fn samples(stbl: &[u8]) -> R<(Vec<Sample>, u32)> {
    let table = |kind: &[u8; 4]| child(stbl, kind).ok_or_else(damaged);

    let stsz = table(b"stsz")?;
    let (fixed, count) = (be32(stsz, 4)?, be32(stsz, 8)? as usize);
    // A corrupt count must not become a huge allocation: the table has to be as long as it says.
    if fixed == 0 && stsz.len() < 12 + 4 * count {
        return Err(damaged());
    }
    if count > 1 << 24 {
        return Err(damaged());
    }
    let size = |i: usize| -> R<u32> {
        if fixed != 0 {
            Ok(fixed)
        } else {
            be32(stsz, 12 + 4 * i)
        }
    };

    // Chunk offsets, then how many samples each chunk holds.
    let offsets: Vec<u64> = if let Some(co) = child(stbl, b"co64") {
        (0..be32(co, 4)? as usize)
            .map(|i| be64(co, 8 + 8 * i))
            .collect::<R<_>>()?
    } else {
        let co = table(b"stco")?;
        (0..be32(co, 4)? as usize)
            .map(|i| be32(co, 8 + 4 * i).map(u64::from))
            .collect::<R<_>>()?
    };
    let stsc = table(b"stsc")?;
    let runs: Vec<(u32, u32)> = (0..be32(stsc, 4)? as usize)
        .map(|i| Ok((be32(stsc, 8 + 12 * i)?, be32(stsc, 12 + 12 * i)?)))
        .collect::<R<_>>()?;

    let mut out: Vec<Sample> = Vec::with_capacity(count);
    let mut run = 0;
    for (chunk, &start) in offsets.iter().enumerate() {
        while runs.get(run + 1).is_some_and(|r| r.0 as usize <= chunk + 1) {
            run += 1;
        }
        let per = runs.get(run).ok_or_else(damaged)?.1;
        let mut at = start;
        for _ in 0..per {
            if out.len() == count {
                break;
            }
            let s = size(out.len())?;
            // The top bit of `size_key` is the sync flag, so a size has to leave it free.
            if s & KEY != 0 {
                return Err(damaged());
            }
            out.push(Sample {
                offset: at,
                dts: 0,
                size_key: s,
                cts: 0,
            });
            at += u64::from(s);
        }
    }
    if out.len() != count {
        return Err(damaged());
    }

    // Decode times and durations.
    let stts = table(b"stts")?;
    let (mut i, mut t, mut last_dur) = (0, 0i64, 0);
    for e in 0..be32(stts, 4)? as usize {
        let (n, delta) = (be32(stts, 8 + 8 * e)?, be32(stts, 12 + 8 * e)?);
        for _ in 0..n {
            let Some(s) = out.get_mut(i) else { break };
            s.dts = t;
            (t, last_dur) = (t + i64::from(delta), delta);
            i += 1;
        }
    }
    // Picture-order offsets (only video with B-frames has them).
    if let Some(ctts) = child(stbl, b"ctts") {
        let mut i = 0;
        for e in 0..be32(ctts, 4)? as usize {
            let (n, offset) = (be32(ctts, 8 + 8 * e)?, be32(ctts, 12 + 8 * e)? as i32);
            for _ in 0..n {
                if let Some(s) = out.get_mut(i) {
                    s.cts = offset;
                }
                i += 1;
            }
        }
    }
    // Sync samples; a track with no list (sound) is all sync.
    match child(stbl, b"stss") {
        Some(stss) => {
            for e in 0..be32(stss, 4)? as usize {
                let n = be32(stss, 8 + 4 * e)? as usize;
                if let Some(s) = n.checked_sub(1).and_then(|n| out.get_mut(n)) {
                    s.size_key |= KEY;
                }
            }
        }
        None => out.iter_mut().for_each(|s| s.size_key |= KEY),
    }
    // Reading a range at a time, in file order, needs each track's samples to be stored in order
    // (every muxer in use does).
    if out.windows(2).any(|w| w[1].offset < w[0].offset) {
        return Err(Error::Unsupported(
            "the movie's samples are not stored in file order".into(),
        ));
    }
    Ok((out, last_dur))
}

/// The first real edit's start in the media (`media_time`): a clip whose first picture is
/// shown late, or whose first sound is encoder delay, says so here.
fn edit(trak: &[u8]) -> i64 {
    let Some(elst) = child(trak, b"edts").and_then(|e| child(e, b"elst")) else {
        return 0;
    };
    let v1 = elst.first() == Some(&1);
    let entry = |i: usize| -> R<i64> {
        if v1 {
            Ok(be64(elst, 8 + 20 * i + 8)? as i64)
        } else {
            Ok(i64::from(be32(elst, 8 + 12 * i + 4)? as i32))
        }
    };
    (0..be32(elst, 4).unwrap_or(0) as usize)
        .filter_map(|i| entry(i).ok())
        .find(|&t| t >= 0)
        .unwrap_or(0)
}

fn us(ticks: i64, scale: u32) -> i64 {
    let (t, s) = (i128::from(ticks), i128::from(scale.max(1)));
    ((t * 1_000_000 + s / 2).div_euclid(s)) as i64
}

/// Reads an `esds`: the object type and the AudioSpecificConfig.
fn esds(b: &[u8]) -> Option<(u8, Vec<u8>)> {
    /// Walks MPEG-4 descriptors: a tag, then a length of 1-4 bytes of 7 bits each.
    fn walk(mut b: &[u8]) -> Vec<(u8, &[u8])> {
        let mut out = vec![];
        while let Some((&tag, rest)) = b.split_first() {
            let (mut len, mut used) = (0usize, 0);
            for &byte in rest.iter().take(4) {
                len = len << 7 | usize::from(byte & 0x7F);
                used += 1;
                if byte & 0x80 == 0 {
                    break;
                }
            }
            let Some(body) = rest.get(used..used + len) else {
                break;
            };
            out.push((tag, body));
            b = &rest[used + len..];
        }
        out
    }
    let es = walk(b.get(4..)?).into_iter().find(|d| d.0 == 3)?.1;
    // ES_ID, flags, then the optional fields the flags announce.
    let flags = *es.get(2)?;
    let skip = 3
        + if flags & 0x80 != 0 { 2 } else { 0 }
        + if flags & 0x40 != 0 {
            1 + usize::from(*es.get(3 + if flags & 0x80 != 0 { 2 } else { 0 })?)
        } else {
            0
        }
        + if flags & 0x20 != 0 { 2 } else { 0 };
    let config = walk(es.get(skip..)?).into_iter().find(|d| d.0 == 4)?.1;
    let asc = walk(config.get(13..)?).into_iter().find(|d| d.0 == 5)?.1;
    Some((*config.first()?, asc.to_vec()))
}

/// Everything about the two tracks a player needs, plus the index.
pub struct Parsed {
    pub duration: i64,
    pub video: VideoInfo,
    pub audio: Option<AudioInfo>,
    /// Microseconds the picture's first frame is late by (see `edit`): the player starts there.
    pub shift: i64,
    pub index: Index,
}

fn video_entry(kind: [u8; 4], e: &[u8]) -> R<VideoInfo> {
    let (width, height) = (be16(e, 24)?, be16(e, 26)?);
    let kids = e.get(78..).unwrap_or(&[]);
    let mut info = VideoInfo {
        name: String::new(),
        codec: String::new(),
        avcc: vec![],
        width: u32::from(width),
        height: u32::from(height),
        pixel_aspect: (1, 1),
        interlaced: false,
    };
    if let Some(p) = child(kids, b"pasp") {
        let (h, v) = (be32(p, 0)?, be32(p, 4)?);
        if h > 0 && v > 0 {
            info.pixel_aspect = (h, v);
        }
    }
    match &kind {
        b"avc1" | b"avc3" => {
            info.name = "H.264".into();
            if let (b"avc1", Some(avcc)) = (&kind, child(kids, b"avcC")) {
                info.set_h264(avcc);
            }
        }
        b"hvc1" | b"hev1" => {
            info.name = "HEVC".into();
            info.codec = "hvc1.1.6.L120.90".into();
        }
        b"vp09" => (info.name, info.codec) = ("VP9".into(), "vp09.00.10.08".into()),
        b"av01" => (info.name, info.codec) = ("AV1".into(), "av01.0.08M.08".into()),
        other => info.name = String::from_utf8_lossy(other).into_owned(),
    }
    Ok(info)
}

fn audio_entry(kind: [u8; 4], e: &[u8]) -> R<AudioInfo> {
    let version = be16(e, 8)?;
    let (channels, rate) = (be16(e, 16)?, be32(e, 24)? >> 16);
    let kids = e
        .get(28 + if version == 1 { 16 } else { 0 }..)
        .unwrap_or(&[]);
    let mut info = AudioInfo {
        name: String::from_utf8_lossy(&kind).into_owned(),
        codec: String::new(),
        audio: Audio::Other,
        channels,
        rate,
    };
    match &kind {
        b"mp4a" => {
            // Some writers wrap the `esds` in a QuickTime `wave` box.
            let esds_box = child(kids, b"esds")
                .or_else(|| child(kids, b"wave").and_then(|w| child(w, b"esds")));
            match esds_box.and_then(esds) {
                Some((0x40, asc)) => {
                    info.name = "AAC".into();
                    info.codec = "mp4a.40.2".into();
                    info.audio = Audio::Aac { asc };
                }
                Some((0x69 | 0x6B, _)) => {
                    (info.name, info.codec) = ("MP3".into(), "mp3".into());
                }
                _ => {}
            }
        }
        b"ac-3" => {
            (info.name, info.codec) = ("AC-3".into(), "ac-3".into());
            info.audio = Audio::Sound(crate::sound::Kind::Ac3);
        }
        b"ec-3" => {
            (info.name, info.codec) = ("E-AC-3".into(), "ec-3".into());
            info.audio = Audio::Sound(crate::sound::Kind::Ac3);
        }
        b"dtsc" | b"dtsh" | b"dtsl" | b"dtse" => info.name = "DTS".into(),
        b"mlpa" => info.name = "TrueHD".into(),
        b"Opus" => (info.name, info.codec) = ("Opus".into(), "opus".into()),
        b"fLaC" => (info.name, info.codec) = ("FLAC".into(), "flac".into()),
        _ => {}
    }
    Ok(info)
}

/// Reads the `moov` payload (without its header).
pub fn parse(moov: &[u8]) -> R<Parsed> {
    let mvhd = child(moov, b"mvhd").ok_or_else(damaged)?;
    let (scale, duration) = if mvhd.first() == Some(&1) {
        (be32(mvhd, 20)?, be64(mvhd, 24)?)
    } else {
        (be32(mvhd, 12)?, u64::from(be32(mvhd, 16)?))
    };
    if child(moov, b"mvex").is_some() {
        return Err(Error::Unsupported("a fragmented MP4".into()));
    }

    let (mut video, mut audio) = (None, None);
    for (kind, trak) in boxes(moov) {
        if &kind != b"trak" {
            continue;
        }
        let mdia = child(trak, b"mdia").ok_or_else(damaged)?;
        let mdhd = child(mdia, b"mdhd").ok_or_else(damaged)?;
        let media_scale = if mdhd.first() == Some(&1) {
            be32(mdhd, 20)?
        } else {
            be32(mdhd, 12)?
        };
        let handler = child(mdia, b"hdlr")
            .and_then(|h| h.get(8..12))
            .unwrap_or(&[]);
        let stbl = child(mdia, b"minf")
            .and_then(|m| child(m, b"stbl"))
            .ok_or_else(damaged)?;
        // The first sample entry; the payload after its header.
        let stsd = child(stbl, b"stsd").ok_or_else(damaged)?;
        let (entry_kind, entry) = boxes(stsd.get(8..).ok_or_else(damaged)?)
            .next()
            .ok_or_else(damaged)?;
        let make = || -> R<Trak> {
            let (samples, last_dur) = samples(stbl)?;
            Ok(Trak {
                samples,
                last_dur,
                scale: media_scale,
                edit: edit(trak),
            })
        };
        match handler {
            b"vide" if video.is_none() => video = Some((video_entry(entry_kind, entry)?, make()?)),
            // A file may list several languages; keep the first track we can do something with.
            b"soun" => {
                let info = audio_entry(entry_kind, entry)?;
                let usable = |i: &AudioInfo| !matches!(i.audio, Audio::Other);
                if audio
                    .as_ref()
                    .is_none_or(|a: &(AudioInfo, Trak)| !usable(&a.0) && usable(&info))
                {
                    audio = Some((info, make()?));
                }
            }
            _ => {}
        }
    }
    let (video_info, v) = video.ok_or_else(|| Error::Unsupported("no video track".into()))?;
    let shift = us(v.edit, v.scale).max(0);
    let (audio_info, a) = match audio {
        Some((info, trak)) => (Some(info), Some(trak)),
        None => (None, None),
    };
    Ok(Parsed {
        duration: us(duration as i64, scale),
        video: video_info,
        audio: audio_info,
        shift,
        index: Index::new(v, a, shift),
    })
}

/// Where every frame of both tracks is. The two tracks are each stored in order, so the order of
/// the file is their merge: there is no table of it to build, sort and keep.
pub struct Index {
    video: Trak,
    audio: Option<Trak>,
    /// Microseconds added to the sound's times so it lines up with the picture (see `edit`).
    audio_shift: i64,
    /// The pictures one can start from, by index.
    keys: Vec<u32>,
}

impl Index {
    fn new(video: Trak, audio: Option<Trak>, shift: i64) -> Index {
        let audio_shift = audio.as_ref().map_or(0, |a| shift - us(a.edit, a.scale));
        let keys = (0..video.samples.len() as u32)
            .filter(|&i| video.samples[i as usize].key())
            .collect();
        Index {
            video,
            audio,
            audio_shift,
            keys,
        }
    }

    fn video_pts(&self, i: u32) -> i64 {
        let s = &self.video.samples[i as usize];
        us(s.dts + i64::from(s.cts), self.video.scale)
    }

    /// Where to begin for a moment (microseconds on the output timeline): the picture at the
    /// keyframe before it, the sound from there on.
    pub fn cursor(&self, at: i64) -> Cursor {
        let k = self.keys.partition_point(|&i| self.video_pts(i) <= at);
        let v = self.keys.get(k.saturating_sub(1)).copied().unwrap_or(0);
        let start = self.video_pts(v);
        let a = self.audio.as_ref().map_or(0, |a| {
            a.samples
                .partition_point(|s| us(s.dts, a.scale) + self.audio_shift < start)
                as u32
        });
        Cursor { v, a }
    }

    /// The next sample in the order of the file: is it sound, and its index in its track.
    fn next(&self, v: u32, a: u32) -> Option<(bool, u32)> {
        let video = self.video.samples.get(v as usize).map(|s| s.offset);
        let audio = self
            .audio
            .as_ref()
            .and_then(|t| t.samples.get(a as usize))
            .map(|s| s.offset);
        match (video, audio) {
            (Some(x), Some(y)) if y < x => Some((true, a)),
            (Some(_), _) => Some((false, v)),
            (None, Some(_)) => Some((true, a)),
            (None, None) => None,
        }
    }

    fn sample(&self, sound: bool, i: u32) -> (&Sample, &Trak) {
        let trak = if sound {
            self.audio.as_ref().expect("entries of the sound track")
        } else {
            &self.video
        };
        (&trak.samples[i as usize], trak)
    }

    /// The next piece of the file to read: its offset and length (at most `max`, but always
    /// whole samples, and at least one).
    pub fn range(&self, c: &Cursor, max: u64) -> Option<(u64, u64)> {
        let (mut v, mut a) = (c.v, c.a);
        let advance = |sound: bool, v: &mut u32, a: &mut u32| *(if sound { a } else { v }) += 1;
        let (sound, i) = self.next(v, a)?;
        let first = self.sample(sound, i).0;
        let start = first.offset;
        let mut end = start + u64::from(first.size());
        advance(sound, &mut v, &mut a);
        while let Some((sound, i)) = self.next(v, a) {
            let s = self.sample(sound, i).0;
            let next = s.offset + u64::from(s.size());
            if next - start > max {
                break;
            }
            end = end.max(next);
            advance(sound, &mut v, &mut a);
        }
        Some((start, end - start))
    }

    /// Cuts the frames out of the bytes of a range (which begin at file offset `start`). Samples
    /// the bytes don't fully cover wait for the next range.
    pub fn frames<'a>(&self, c: &mut Cursor, start: u64, bytes: &'a [u8]) -> Vec<Frame<'a>> {
        let mut out = vec![];
        let end = start + bytes.len() as u64;
        while let Some((sound, i)) = self.next(c.v, c.a) {
            let (s, trak) = self.sample(sound, i);
            if s.offset < start || s.offset + u64::from(s.size()) > end {
                break;
            }
            let data = std::borrow::Cow::Borrowed(
                &bytes[(s.offset - start) as usize..][..s.size() as usize],
            );
            let dur = us(s.dts + trak.dur(i), trak.scale) - us(s.dts, trak.scale);
            if sound {
                c.a += 1;
                let t = us(s.dts, trak.scale) + self.audio_shift;
                out.push(Frame {
                    track: Track::Audio,
                    pts: t,
                    dts: t,
                    dur,
                    key: true,
                    data,
                });
            } else {
                c.v += 1;
                out.push(Frame {
                    track: Track::Video,
                    pts: us(s.dts + i64::from(s.cts), trak.scale),
                    dts: us(s.dts, trak.scale),
                    dur,
                    key: s.key(),
                    data,
                });
            }
        }
        out
    }

    pub fn done(&self, c: &Cursor) -> bool {
        c.v as usize >= self.video.samples.len()
            && c.a as usize >= self.audio.as_ref().map_or(0, |t| t.samples.len())
    }
}

/// How far through the file a session is: the next sample of each track.
#[derive(Clone)]
pub struct Cursor {
    v: u32,
    a: u32,
}
