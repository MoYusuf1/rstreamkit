//! Matroska (MKV). The tracks and the cue index are near the start (or the end) of the file; a
//! seek goes to the cue before it and reads clusters from there, one block at a time.
//!
//! Matroska stores when a picture is shown, not when it is decoded, and a browser needs both. The
//! decode order is the file order, and its times are the same set of times in ascending order, so
//! each group of pictures gets them that way (see [`Cursor`]).
//!
//! ponytail: no lacing (AC-3, AAC and video are never laced in practice), no content encodings
//! (compressed or encrypted tracks), and a file with no cue index is refused rather than scanned.

use crate::{
    Error,
    vod::{Audio, AudioInfo, Frame, Track, VideoInfo},
};

/// Microseconds the movie's clock is moved forward by on the output timeline. A picture can be
/// decoded well before it is shown (B-frames); this is room for that, so no decode time is
/// negative. A second is more than any encoder reorders by.
pub const SHIFT: i64 = 1_000_000;

const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114D_9B74;
const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const CUES: u32 = 0x1C53_BB6B;
const CLUSTER: u32 = 0x1F43_B675;
const TIMECODE: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;
const REFERENCE_BLOCK: u32 = 0xFB;

fn damaged() -> Error {
    Error::Unsupported("the MKV index is damaged".into())
}

/// An element ID (1-4 bytes, the length marker kept) and how many bytes it took.
fn read_id(b: &[u8]) -> Option<(u32, usize)> {
    let len = b.first()?.leading_zeros() as usize + 1;
    if len > 4 {
        return None;
    }
    let id = b
        .get(..len)?
        .iter()
        .fold(0, |id, &x| id << 8 | u32::from(x));
    Some((id, len))
}

/// A size (1-8 bytes, the length marker removed); `None` inside means "unknown".
fn read_size(b: &[u8]) -> Option<(Option<u64>, usize)> {
    let first = *b.first()?;
    let len = first.leading_zeros() as usize + 1;
    if len > 8 {
        return None;
    }
    let mut value = u64::from(first) & ((1 << (8 - len)) - 1);
    let mut ones = value == (1 << (8 - len)) - 1;
    for &x in b.get(1..len)? {
        value = value << 8 | u64::from(x);
        ones &= x == 0xFF;
    }
    Some((if ones { None } else { Some(value) }, len))
}

/// `(id, size, header length)` of the element at the start of `b`.
fn header(b: &[u8]) -> Option<(u32, Option<u64>, usize)> {
    let (id, a) = read_id(b)?;
    let (size, s) = read_size(&b[a..])?;
    Some((id, size, a + s))
}

/// The elements directly inside `b`, as (id, payload). Stops at the first one that doesn't fit.
fn elements(mut b: &[u8]) -> impl Iterator<Item = (u32, &[u8])> {
    std::iter::from_fn(move || {
        let (id, size, h) = header(b)?;
        let end = h.checked_add(usize::try_from(size?).ok()?)?;
        let payload = b.get(h..end)?;
        b = &b[end..];
        Some((id, payload))
    })
}

fn uint(b: &[u8]) -> u64 {
    b.iter().fold(0, |v, &x| v << 8 | u64::from(x))
}

fn float(b: &[u8]) -> f64 {
    match b.len() {
        4 => f64::from(f32::from_be_bytes(b.try_into().unwrap())),
        8 => f64::from_be_bytes(b.try_into().unwrap()),
        _ => 0.0,
    }
}

struct Entry {
    number: u64,
    kind: u64,
    codec: String,
    private: Vec<u8>,
    default_duration: i64,
    video: (u32, u32, u32, u32, bool),
    audio: (f64, u16),
    encoded: bool,
}

fn entry(b: &[u8]) -> Entry {
    let mut e = Entry {
        number: 0,
        kind: 0,
        codec: String::new(),
        private: vec![],
        default_duration: 0,
        video: (0, 0, 0, 0, false),
        audio: (8000.0, 1),
        encoded: false,
    };
    for (id, p) in elements(b) {
        match id {
            0xD7 => e.number = uint(p),
            0x83 => e.kind = uint(p),
            0x86 => e.codec = String::from_utf8_lossy(p).into_owned(),
            0x63A2 => e.private = p.to_vec(),
            0x23E383 => e.default_duration = (uint(p) / 1000) as i64,
            0x6D80 => e.encoded = true,
            0xE0 => {
                for (id, p) in elements(p) {
                    match id {
                        0xB0 => e.video.0 = uint(p) as u32,
                        0xBA => e.video.1 = uint(p) as u32,
                        0x54B0 => e.video.2 = uint(p) as u32,
                        0x54BA => e.video.3 = uint(p) as u32,
                        0x9A => e.video.4 = uint(p) > 1,
                        _ => {}
                    }
                }
            }
            0xE1 => {
                for (id, p) in elements(p) {
                    match id {
                        0xB5 => e.audio.0 = float(p),
                        0x9F => e.audio.1 = uint(p) as u16,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    e
}

fn video_info(e: &Entry) -> VideoInfo {
    let (w, h, dw, dh, interlaced) = e.video;
    let mut info = VideoInfo {
        name: String::new(),
        codec: String::new(),
        avcc: vec![],
        width: w,
        height: h,
        pixel_aspect: (1, 1),
        interlaced,
    };
    if dw > 0 && dh > 0 && w > 0 && h > 0 {
        let (num, den) = (u64::from(dw) * u64::from(h), u64::from(dh) * u64::from(w));
        let g = gcd(num, den).max(1);
        info.pixel_aspect = ((num / g) as u32, (den / g) as u32);
    }
    match e.codec.as_str() {
        "V_MPEG4/ISO/AVC" => info.set_h264(&e.private),
        "V_MPEGH/ISO/HEVC" => (info.name, info.codec) = ("HEVC".into(), "hvc1.1.6.L120.90".into()),
        "V_VP9" => (info.name, info.codec) = ("VP9".into(), "vp09.00.10.08".into()),
        "V_VP8" => (info.name, info.codec) = ("VP8".into(), "vp8".into()),
        "V_AV1" => (info.name, info.codec) = ("AV1".into(), "av01.0.08M.08".into()),
        other => info.name = other.trim_start_matches("V_").into(),
    }
    info
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn audio_info(e: &Entry) -> AudioInfo {
    let mut info = AudioInfo {
        name: e.codec.trim_start_matches("A_").into(),
        codec: String::new(),
        audio: Audio::Other,
        channels: e.audio.1,
        rate: e.audio.0 as u32,
    };
    let c = e.codec.as_str();
    if c.starts_with("A_AAC") {
        info.name = "AAC".into();
        info.codec = "mp4a.40.2".into();
        info.audio = Audio::Aac {
            asc: e.private.clone(),
        };
    } else if c == "A_AC3" {
        (info.name, info.codec) = ("AC-3".into(), "ac-3".into());
        info.audio = Audio::Sound(crate::sound::Kind::Ac3);
    } else if c == "A_EAC3" {
        (info.name, info.codec) = ("E-AC-3".into(), "ec-3".into());
        info.audio = Audio::Sound(crate::sound::Kind::Ac3);
    } else if c == "A_MPEG/L2" {
        info.name = "MP2".into();
        info.audio = Audio::Sound(crate::sound::Kind::Mpeg);
    } else if c == "A_MPEG/L3" {
        (info.name, info.codec) = ("MP3".into(), "mp3".into());
    } else if c.starts_with("A_DTS") {
        info.name = "DTS".into();
    } else if c == "A_TRUEHD" {
        info.name = "TrueHD".into();
    } else if c == "A_OPUS" {
        (info.name, info.codec) = ("Opus".into(), "opus".into());
    } else if c == "A_FLAC" {
        (info.name, info.codec) = ("FLAC".into(), "flac".into());
    } else if c == "A_VORBIS" {
        (info.name, info.codec) = ("Vorbis".into(), "vorbis".into());
    }
    info
}

/// What the start of the file told us, and what is needed to read the rest.
pub struct Parsed {
    pub duration: i64,
    pub video: VideoInfo,
    pub audio: Option<AudioInfo>,
    pub index: Index,
}

/// Finds the tracks and the cue index, asking for one piece of the file at a time.
pub struct Probe {
    size: u64,
    segment: Option<u64>,
    scan: u64,
    seeks: Vec<(u32, u64)>,
    info: Option<(u64, f64)>,
    tracks: Option<Vec<Entry>>,
    cues: Option<Vec<(u64, u64, u64)>>,
    visited: Vec<u64>,
}

pub enum Step {
    /// Fetch this many bytes from this offset and call `feed` again.
    Read(u64, u64),
    Done(Box<Parsed>),
}

const WINDOW: u64 = 256 << 10;

impl Probe {
    pub fn new(size: u64) -> Probe {
        Probe {
            size,
            segment: None,
            scan: 0,
            seeks: vec![],
            info: None,
            tracks: None,
            cues: None,
            visited: vec![],
        }
    }

    /// `bytes` are the file from offset `at`.
    pub fn feed(&mut self, at: u64, bytes: &[u8]) -> Result<Step, Error> {
        loop {
            if let (Some((scale, dur)), Some(tracks), Some(cues)) =
                (&self.info, &self.tracks, &self.cues)
            {
                return finish(*scale, *dur, tracks, cues.clone());
            }
            if self.scan >= self.size {
                return Err(Error::Unsupported("the MKV has no cue index".into()));
            }
            let off = match self.scan.checked_sub(at).map(usize::try_from) {
                Some(Ok(o)) if o < bytes.len() => o,
                _ => return Ok(Step::Read(self.scan, WINDOW.min(self.size - self.scan))),
            };
            let Some((id, size, h)) = header(&bytes[off..]) else {
                // A header is at most 12 bytes: with that many and still no header, it's not one.
                if bytes.len() - off >= 12 {
                    return Err(damaged());
                }
                return Ok(Step::Read(self.scan, WINDOW.min(self.size - self.scan)));
            };
            let abs = self.scan;
            match id {
                SEGMENT => {
                    self.segment = Some(abs + h as u64);
                    self.scan = abs + h as u64;
                }
                CLUSTER => {
                    // Media begins: the cues must be elsewhere, if the seek head says where.
                    let seg = self.segment.ok_or_else(damaged)?;
                    let next = |id: u32| {
                        self.seeks
                            .iter()
                            .find(|s| s.0 == id && !self.visited.contains(&(seg + s.1)))
                            .map(|s| seg + s.1)
                    };
                    let Some(to) = next(CUES).or_else(|| next(SEEK_HEAD)) else {
                        return Err(Error::Unsupported("the MKV has no cue index".into()));
                    };
                    self.visited.push(to);
                    self.scan = to;
                }
                SEEK_HEAD | INFO | TRACKS | CUES => {
                    let total = h as u64 + size.ok_or_else(damaged)?;
                    if abs + total > self.size {
                        return Err(damaged());
                    }
                    if abs + total > at + bytes.len() as u64 {
                        return Ok(Step::Read(abs, total));
                    }
                    let payload = &bytes[off + h..off + total as usize];
                    let seg = self.segment.ok_or_else(damaged)?;
                    match id {
                        SEEK_HEAD => self.seek_head(payload),
                        INFO => self.info = Some(info(payload)),
                        TRACKS => {
                            self.tracks = Some(
                                elements(payload)
                                    .filter(|e| e.0 == 0xAE)
                                    .map(|e| entry(e.1))
                                    .collect(),
                            )
                        }
                        _ => self.cues = Some(cues(payload, seg)),
                    }
                    self.scan = abs + total;
                }
                _ => match size {
                    Some(s) => self.scan = abs + h as u64 + s,
                    None => return Err(damaged()),
                },
            }
        }
    }

    fn seek_head(&mut self, payload: &[u8]) {
        for (id, seek) in elements(payload) {
            if id != 0x4DBB {
                continue;
            }
            let (mut what, mut at) = (0, 0);
            for (id, p) in elements(seek) {
                match id {
                    0x53AB => what = uint(p) as u32,
                    0x53AC => at = uint(p),
                    _ => {}
                }
            }
            self.seeks.push((what, at));
        }
    }
}

fn info(payload: &[u8]) -> (u64, f64) {
    let (mut scale, mut duration) = (1_000_000, 0.0);
    for (id, p) in elements(payload) {
        match id {
            0x2A_D7B1 => scale = uint(p),
            0x4489 => duration = float(p),
            _ => {}
        }
    }
    (scale, duration)
}

/// (time in timescale ticks, cluster offset in the file, track number) for each cue point.
fn cues(payload: &[u8], segment: u64) -> Vec<(u64, u64, u64)> {
    let mut out = vec![];
    for (id, point) in elements(payload) {
        if id != 0xBB {
            continue;
        }
        let mut time = 0;
        let mut at = vec![];
        for (id, p) in elements(point) {
            match id {
                0xB3 => time = uint(p),
                0xB7 => {
                    let (mut track, mut cluster) = (0, None);
                    for (id, p) in elements(p) {
                        match id {
                            0xF7 => track = uint(p),
                            0xF1 => cluster = Some(segment + uint(p)),
                            _ => {}
                        }
                    }
                    at.extend(cluster.map(|c| (track, c)));
                }
                _ => {}
            }
        }
        out.extend(
            at.into_iter()
                .map(|(track, cluster)| (time, cluster, track)),
        );
    }
    out
}

fn finish(
    scale: u64,
    duration: f64,
    tracks: &[Entry],
    cues: Vec<(u64, u64, u64)>,
) -> Result<Step, Error> {
    let video = tracks
        .iter()
        .find(|t| t.kind == 1)
        .ok_or_else(|| Error::Unsupported("no video track".into()))?;
    let mut audio: Option<(&Entry, AudioInfo)> = None;
    for t in tracks.iter().filter(|t| t.kind == 2) {
        let info = audio_info(t);
        let usable = |i: &AudioInfo| !matches!(i.audio, Audio::Other);
        if audio
            .as_ref()
            .is_none_or(|a| !usable(&a.1) && usable(&info))
        {
            audio = Some((t, info));
        }
    }
    if video.encoded || audio.as_ref().is_some_and(|a| a.0.encoded) {
        return Err(Error::Unsupported("compressed or encrypted tracks".into()));
    }
    // Cue points for the picture; a file that only lists the sound's will do as a guide too.
    let mut points: Vec<(i64, u64)> = cues
        .iter()
        .filter(|c| c.2 == video.number)
        .map(|c| (to_us(c.0, scale), c.1))
        .collect();
    if points.is_empty() {
        points = cues.iter().map(|c| (to_us(c.0, scale), c.1)).collect();
    }
    points.sort_unstable();
    points.dedup_by_key(|p| p.1);
    if points.is_empty() {
        return Err(Error::Unsupported("the MKV has no cue index".into()));
    }
    Ok(Step::Done(Box::new(Parsed {
        duration: (duration * scale as f64 / 1000.0) as i64,
        video: video_info(video),
        audio: audio.as_ref().map(|a| a.1.clone()),
        index: Index {
            scale,
            video: video.number,
            audio: audio.map(|a| a.0.number),
            default_duration: if video.default_duration > 0 {
                video.default_duration
            } else {
                41_708
            },
            cues: points,
        },
    })))
}

fn to_us(ticks: u64, scale: u64) -> i64 {
    (i128::from(ticks) * i128::from(scale) / 1000) as i64
}

/// Where to read and how to read it.
pub struct Index {
    /// Nanoseconds per timecode tick.
    scale: u64,
    video: u64,
    audio: Option<u64>,
    /// Microseconds, for a picture with no duration of its own.
    default_duration: i64,
    /// (time in microseconds, offset of its cluster) of each place a picture can be started from.
    cues: Vec<(i64, u64)>,
}

/// A picture waiting for the rest of its group, so its decode time can be worked out.
struct Held {
    pts: i64,
    key: bool,
    data: Vec<u8>,
}

/// How far through the file a session is.
pub struct Cursor {
    offset: u64,
    /// At least this many bytes from `offset` are needed (a block bigger than a chunk).
    need: u64,
    /// The cluster being read, in timecode ticks.
    cluster: i64,
    gop: Vec<Held>,
    last_dts: i64,
}

impl Index {
    pub fn cursor(&self, at: i64) -> Cursor {
        let movie = at - SHIFT;
        let i = self.cues.partition_point(|c| c.0 <= movie);
        Cursor {
            offset: self.cues[i.saturating_sub(1)].1,
            need: 0,
            cluster: 0,
            gop: vec![],
            last_dts: i64::MIN,
        }
    }

    pub fn range(&self, c: &Cursor, max: u64, size: u64) -> Option<(u64, u64)> {
        (c.offset < size).then(|| (c.offset, max.max(c.need).min(size - c.offset)))
    }

    /// Reads the blocks in `bytes` (the file from offset `start`). The last piece of the file
    /// also hands over the final group of pictures.
    pub fn frames(
        &self,
        c: &mut Cursor,
        start: u64,
        bytes: &[u8],
        last: bool,
    ) -> (Vec<Frame<'static>>, bool) {
        let mut out = vec![];
        let mut i = 0;
        let mut jump = None;
        while let Some((id, size, h)) = header(&bytes[i..]) {
            // Containers are entered, never skipped: a cluster may have no size at all.
            if matches!(id, CLUSTER) || (id == BLOCK_GROUP && size.is_none()) {
                i += h;
                continue;
            }
            let Some(len) = size else { break };
            let total = h as u64 + len;
            if i as u64 + total > bytes.len() as u64 {
                match id {
                    TIMECODE | SIMPLE_BLOCK | BLOCK_GROUP => c.need = total,
                    // Something we don't read (cues, tags, attachments): step over it unread.
                    _ => jump = Some(start + i as u64 + total),
                }
                break;
            }
            let payload = &bytes[i + h..i + total as usize];
            i += total as usize;
            match id {
                TIMECODE => c.cluster = uint(payload) as i64,
                SIMPLE_BLOCK => self.block(c, payload, None, &mut out),
                BLOCK_GROUP => {
                    let mut key = true;
                    let mut block = None;
                    for (id, p) in elements(payload) {
                        match id {
                            BLOCK => block = Some(p),
                            REFERENCE_BLOCK => key = false,
                            _ => {}
                        }
                    }
                    if let Some(p) = block {
                        self.block(c, p, Some(key), &mut out);
                    }
                }
                _ => {}
            }
        }
        c.offset = jump.unwrap_or(start + i as u64);
        // The end of the file: hand over the last group of pictures.
        if last {
            self.flush(c, &mut out);
        }
        (out, last)
    }

    fn block(
        &self,
        c: &mut Cursor,
        p: &[u8],
        group_key: Option<bool>,
        out: &mut Vec<Frame<'static>>,
    ) {
        let Some((Some(track), n)) = read_size(p) else {
            return;
        };
        let (Some(rel), Some(&flags)) = (p.get(n..n + 2), p.get(n + 2)) else {
            return;
        };
        let data = &p[n + 3..];
        // ponytail: laced blocks are skipped (see the module comment).
        if flags & 0x06 != 0 {
            return;
        }
        let rel = i64::from(i16::from_be_bytes([rel[0], rel[1]]));
        let pts = to_us((c.cluster + rel).max(0) as u64, self.scale) + SHIFT;
        if track == self.video {
            let key = group_key.unwrap_or(flags & 0x80 != 0);
            if key {
                self.flush(c, out);
            }
            c.gop.push(Held {
                pts,
                key,
                data: data.to_vec(),
            });
        } else if Some(track) == self.audio {
            out.push(Frame {
                track: Track::Audio,
                pts,
                dts: pts,
                dur: 0,
                key: true,
                data: data.to_vec().into(),
            });
        }
    }

    /// Gives the waiting group of pictures its decode times: the same times as the pictures'
    /// own, in ascending order (decode order is file order), never going backwards.
    fn flush(&self, c: &mut Cursor, out: &mut Vec<Frame<'static>>) {
        let gop = std::mem::take(&mut c.gop);
        let mut order: Vec<i64> = gop.iter().map(|g| g.pts - SHIFT).collect();
        order.sort_unstable();
        let mut dts: Vec<i64> = vec![];
        for &t in &order {
            dts.push(t.max(dts.last().copied().unwrap_or(c.last_dts).saturating_add(1)));
        }
        // The last picture lasts as long as the one before it did.
        let tail = match order.as_slice() {
            [.., a, b] => b - a,
            _ => self.default_duration,
        };
        for (n, held) in gop.into_iter().enumerate() {
            let dur = dts.get(n + 1).map_or(tail, |next| next - dts[n]);
            c.last_dts = dts[n];
            out.push(Frame {
                track: Track::Video,
                pts: held.pts,
                dts: dts[n],
                dur,
                key: held.key,
                data: held.data.into(),
            });
        }
    }
}
