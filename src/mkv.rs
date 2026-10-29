//! Matroska (MKV). The tracks and the cue index are near the start (or the end) of the file; a
//! seek goes to the cue before it and reads clusters from there, one block at a time.
//!
//! Matroska stores when a picture is shown, not when it is decoded, and a browser needs both. The
//! decode order is the file order, and its times are the same set of times in ascending order, so
//! each group of pictures gets them that way (see [`Cursor`]).
//!
//! Laced sound (several frames in one block, which mkvmerge writes by default) is split into its
//! frames, and header stripping (a prefix taken off every frame) is undone. ponytail: laced
//! pictures are skipped (no muxer writes them) and other content encodings (zlib, encryption) are
//! refused. A file with no cue index plays from its first cluster and can't seek.

use crate::{
    Error,
    vod::{Audio, AudioInfo, Frame, Track, VideoInfo, av1_codec, hevc_codec},
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
    /// Compressed or encrypted in a way not read here.
    encoded: bool,
    /// The bytes header stripping took off the front of every frame (empty without it).
    stripped: Vec<u8>,
}

/// The prefix a track's one content encoding strips from each frame, when that is all it does
/// (header stripping, mkvmerge's old default for AC-3 and H.264); `None` for anything else.
fn stripped_header(encodings: &[u8]) -> Option<Vec<u8>> {
    let mut all = elements(encodings).filter(|(id, _)| *id == 0x6240);
    let (_, encoding) = all.next()?;
    if all.next().is_some() {
        return None;
    }
    let (mut scope, mut kind, mut algo, mut prefix) = (1, 0, 0, vec![]);
    for (id, p) in elements(encoding) {
        match id {
            0x5032 => scope = uint(p),
            0x5033 => kind = uint(p),
            0x5034 => {
                for (id, p) in elements(p) {
                    match id {
                        0x4254 => algo = uint(p),
                        0x4255 => prefix = p.to_vec(),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    // Frames only (scope 1), compression (type 0), header stripping (algorithm 3).
    (scope == 1 && kind == 0 && algo == 3).then_some(prefix)
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
        stripped: vec![],
    };
    for (id, p) in elements(b) {
        match id {
            0xD7 => e.number = uint(p),
            0x83 => e.kind = uint(p),
            0x86 => e.codec = String::from_utf8_lossy(p).into_owned(),
            0x63A2 => e.private = p.to_vec(),
            0x23E383 => e.default_duration = (uint(p) / 1000) as i64,
            0x6D80 => match stripped_header(p) {
                Some(prefix) => e.stripped = prefix,
                None => e.encoded = true,
            },
            0xE0 => {
                for (id, p) in elements(p) {
                    match id {
                        0xB0 => e.video.0 = uint(p) as u32,
                        0xBA => e.video.1 = uint(p) as u32,
                        0x54B0 => e.video.2 = uint(p) as u32,
                        0x54BA => e.video.3 = uint(p) as u32,
                        0x9A => e.video.4 = uint(p) == 1, // FlagInterlaced: 0 unknown, 1 interlaced, 2 progressive
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
        "V_MPEGH/ISO/HEVC" => (info.name, info.codec) = ("HEVC".into(), hevc_codec(&e.private)),
        "V_VP9" => (info.name, info.codec) = ("VP9".into(), "vp09.00.10.08".into()),
        "V_VP8" => (info.name, info.codec) = ("VP8".into(), "vp8".into()),
        "V_AV1" => (info.name, info.codec) = ("AV1".into(), av1_codec(&e.private)),
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
    /// Where the first cluster begins: where to read from when there is no cue index.
    first_cluster: Option<u64>,
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
            first_cluster: None,
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
                return self.unindexed();
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
                    self.first_cluster.get_or_insert(abs);
                    let next = |id: u32| {
                        self.seeks
                            .iter()
                            .find(|s| s.0 == id && !self.visited.contains(&(seg + s.1)))
                            .map(|s| seg + s.1)
                    };
                    let Some(to) = next(CUES).or_else(|| next(SEEK_HEAD)) else {
                        return self.unindexed();
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

    /// No cue index: a movie that plays from its first cluster and can't seek.
    fn unindexed(&self) -> Result<Step, Error> {
        match (&self.info, &self.tracks, self.first_cluster) {
            (Some((scale, dur)), Some(tracks), Some(at)) => {
                let mut step = finish(*scale, *dur, tracks, vec![(0, at, 0)])?;
                if let Step::Done(parsed) = &mut step {
                    parsed.index.indexed = false;
                }
                Ok(step)
            }
            _ => Err(Error::Unsupported("the MKV has no cue index".into())),
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
            audio: audio.as_ref().map(|a| a.0.number),
            default_duration: if video.default_duration > 0 {
                video.default_duration
            } else {
                41_708
            },
            sound_frame: audio.as_ref().map_or(0, |(t, info)| sound_frame(t, info)),
            indexed: true,
            stripped: (
                video.stripped.clone(),
                audio.as_ref().map_or(vec![], |a| a.0.stripped.clone()),
            ),
            cues: points,
        },
    })))
}

/// Microseconds one sound frame lasts: the track's own default duration, or its codec's frame size.
fn sound_frame(e: &Entry, info: &AudioInfo) -> i64 {
    if e.default_duration > 0 {
        return e.default_duration;
    }
    let samples = match e.codec.as_str() {
        "A_AC3" | "A_EAC3" => 1536,
        "A_MPEG/L2" | "A_MPEG/L3" => 1152,
        c if c.starts_with("A_AAC") => 1024,
        _ => return 0,
    };
    samples * 1_000_000 / i64::from(info.rate.max(1))
}

/// The frames of a laced block's payload (after its flags byte), by the lacing in `flags`.
fn laces(flags: u8, data: &[u8]) -> Option<Vec<&[u8]>> {
    let (&count, mut rest) = data.split_first()?;
    let count = usize::from(count) + 1;
    let mut sizes = Vec::with_capacity(count);
    match flags & 0x06 {
        // Xiph: each size but the last as a run of 255s and a final byte.
        0x02 => {
            for _ in 1..count {
                let mut size = 0;
                loop {
                    let (&b, r) = rest.split_first()?;
                    rest = r;
                    size += usize::from(b);
                    if b != 255 {
                        break;
                    }
                }
                sizes.push(size);
            }
        }
        // EBML: the first size, then each next one as a signed difference.
        0x06 => {
            let (Some(first), n) = read_size(rest)? else {
                return None;
            };
            rest = &rest[n..];
            let mut size = i64::try_from(first).ok()?;
            sizes.push(usize::try_from(size).ok()?);
            for _ in 2..count {
                let (Some(raw), n) = read_size(rest)? else {
                    return None;
                };
                rest = &rest[n..];
                size += i64::try_from(raw).ok()? - ((1i64 << (7 * n - 1)) - 1);
                sizes.push(usize::try_from(size).ok()?);
            }
        }
        // Fixed: equal sizes.
        _ => {
            if rest.len() % count != 0 {
                return None;
            }
            sizes = vec![rest.len() / count; count - 1];
        }
    }
    let mut frames = Vec::with_capacity(count);
    for size in sizes {
        let (frame, r) = (rest.get(..size)?, &rest[size..]);
        frames.push(frame);
        rest = r;
    }
    frames.push(rest);
    Some(frames)
}

fn to_us(ticks: u64, scale: u64) -> i64 {
    (i128::from(ticks) * i128::from(scale) / 1000) as i64
}

/// Where to read and how to read it.
/// Bytes past the next cue a range also covers: the cluster header, any sound blocks before the
/// keyframe, and the keyframe's header.
const GROUP_MARGIN: u64 = 32 << 10;
/// The most a range is stretched to reach the next cue; a longer group is read in more pieces.
const GROUP_LIMIT: u64 = 16 << 20;

pub struct Index {
    /// Nanoseconds per timecode tick.
    scale: u64,
    video: u64,
    audio: Option<u64>,
    /// Microseconds, for a picture with no duration of its own.
    default_duration: i64,
    /// Microseconds one sound frame lasts, to time the frames of a laced block (0 if unknown).
    sound_frame: i64,
    /// What header stripping took off each (picture, sound) frame, put back when reading.
    stripped: (Vec<u8>, Vec<u8>),
    /// False for a file without a cue index: it is read from its first cluster and can't seek.
    indexed: bool,
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
    /// Whether the file has a cue index, so reading can start anywhere.
    pub fn indexed(&self) -> bool {
        self.indexed
    }

    pub fn cursor(&self, at: i64) -> Cursor {
        let movie = at.saturating_sub(SHIFT);
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
        // Reach the next cue's cluster (where the next group of pictures starts) and the header of
        // its first block: the pictures before it only go out once that keyframe is seen, so a
        // shorter range would cost another round trip before anything plays.
        let group = self
            .cues
            .iter()
            .map(|cue| cue.1)
            .find(|&o| o > c.offset)
            .map_or(0, |o| (o - c.offset + GROUP_MARGIN).min(GROUP_LIMIT));
        (c.offset < size).then(|| (c.offset, max.max(c.need).max(group).min(size - c.offset)))
    }

    /// Reads the blocks in `bytes` (the file from offset `start`, `size` bytes long). The last
    /// piece of the file also hands over the final group of pictures.
    pub fn frames(
        &self,
        c: &mut Cursor,
        start: u64,
        bytes: &[u8],
        size: u64,
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
                    TIMECODE | SIMPLE_BLOCK | BLOCK_GROUP => {
                        c.need = total;
                        // A keyframe's header is enough to know the group before it is complete.
                        let body = &bytes[i + h..];
                        if id == SIMPLE_BLOCK
                            && let Some((Some(track), n)) = read_size(body)
                            && track == self.video
                            && body.get(n + 2).is_some_and(|flags| flags & 0x80 != 0)
                        {
                            self.flush(c, &mut out);
                        }
                    }
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
        // The end of the file, read or stepped over (an index or tags last): hand over the last
        // group of pictures.
        let last = start + bytes.len() as u64 >= size || c.offset >= size;
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
        let rel = i64::from(i16::from_be_bytes([rel[0], rel[1]]));
        let pts = to_us((c.cluster + rel).max(0) as u64, self.scale) + SHIFT;
        if flags & 0x06 != 0 {
            // Laced: only sound (see the module comment); a malformed lace is dropped.
            if Some(track) == self.audio
                && let Some(frames) = laces(flags, data)
            {
                for (i, frame) in frames.into_iter().enumerate() {
                    out.push(Frame {
                        track: Track::Audio,
                        pts: pts + i as i64 * self.sound_frame,
                        dts: pts + i as i64 * self.sound_frame,
                        dur: 0,
                        key: true,
                        data: [self.stripped.1.as_slice(), frame].concat().into(),
                    });
                }
            }
            return;
        }
        if track == self.video {
            let key = group_key.unwrap_or(flags & 0x80 != 0);
            if key {
                self.flush(c, out);
            }
            c.gop.push(Held {
                pts,
                key,
                data: [self.stripped.0.as_slice(), data].concat(),
            });
        } else if Some(track) == self.audio {
            out.push(Frame {
                track: Track::Audio,
                pts,
                dts: pts,
                dur: 0,
                key: true,
                data: [self.stripped.1.as_slice(), data].concat().into(),
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

#[cfg(test)]
mod tests {
    use super::{Cursor, GROUP_MARGIN, Index, SHIFT, entry, laces};
    use crate::vod::Track;

    #[test]
    fn laced_frames_split_the_same_in_all_three_lacings() {
        let (a, b, c) = (vec![1u8; 300], vec![2u8; 3], vec![3u8; 290]);
        let joined = [a.as_slice(), &b, &c].concat();
        // Xiph: 300 = 255 + 45, then 3.
        let xiph = [&[2u8, 255, 45, 3][..], &joined].concat();
        assert_eq!(laces(0x02, &xiph).unwrap(), [&a[..], &b, &c]);
        // EBML: 300 as a 2-byte size, then 3 - 300 = -297 as a 2-byte signed difference.
        let delta = (-297i64 + 8191) as u16 | 0x4000;
        let ebml = [&[2u8, 0x41, 0x2C][..], &delta.to_be_bytes(), &joined].concat();
        assert_eq!(laces(0x06, &ebml).unwrap(), [&a[..], &b, &c]);
        // Fixed: equal sizes only.
        let fixed = [&[2u8][..], &[7u8; 12]].concat();
        assert_eq!(laces(0x04, &fixed).unwrap(), [&[7u8; 4][..]; 3]);
        assert!(laces(0x04, &[2, 1, 2, 3, 4]).is_none());
        assert!(laces(0x02, &[2, 255]).is_none());
    }

    /// A ContentEncodings element holding one compression with `algo` and, if given, settings.
    fn encodings(algo: u8, settings: &[u8]) -> Vec<u8> {
        let mut comp = vec![0x42, 0x54, 0x81, algo];
        if !settings.is_empty() {
            comp.extend([0x42, 0x55, 0x80 | settings.len() as u8]);
            comp.extend(settings);
        }
        let comp = [&[0x50, 0x34, 0x80 | comp.len() as u8][..], &comp].concat();
        let enc = [&[0x62, 0x40, 0x80 | comp.len() as u8][..], &comp].concat();
        [&[0x6D, 0x80, 0x80 | enc.len() as u8][..], &enc].concat()
    }

    #[test]
    fn hevc_codec_strings_come_from_the_hvcc_record() {
        use crate::vod::hevc_codec;
        // Main, level 4 (120), progressive and frame-only constraint flags.
        let main = [1, 0x01, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 120];
        assert_eq!(hevc_codec(&main), "hvc1.1.6.L120.90");
        // Main 10, high tier, level 5.1 (153).
        let main10 = [1, 0x22, 0x20, 0, 0, 0, 0xB0, 0, 0, 0, 0, 0, 153];
        assert_eq!(hevc_codec(&main10), "hvc1.2.4.H153.B0");
        assert_eq!(hevc_codec(&[1, 2]), "hvc1.1.6.L120.90");
    }

    #[test]
    fn av1_codec_strings_come_from_the_av1c_record() {
        use crate::vod::av1_codec;
        // Main profile, level 4.0 (8), main tier, 10-bit.
        assert_eq!(av1_codec(&[0x81, 0x08, 0x4C, 0]), "av01.0.08M.10");
        // Professional profile (2), level 5.1 (13), high tier, 12-bit.
        assert_eq!(av1_codec(&[0x81, 0x4D, 0xE0, 0]), "av01.2.13H.12");
        assert_eq!(av1_codec(&[0x81]), "av01.0.08M.08");
    }

    #[test]
    fn header_stripping_is_read_and_other_encodings_refused() {
        let stripped = entry(&encodings(3, &[0x0B, 0x77]));
        assert_eq!(
            (stripped.encoded, stripped.stripped),
            (false, vec![0x0B, 0x77])
        );
        // zlib (algorithm 0) is still refused.
        assert!(entry(&encodings(0, &[])).encoded);
    }

    #[test]
    fn stripped_bytes_are_put_back_on_every_frame() {
        let index = Index {
            scale: 1_000_000,
            video: 1,
            audio: Some(2),
            default_duration: 40_000,
            sound_frame: 32_000,
            stripped: (vec![0, 0, 0, 1], vec![0x0B, 0x77]),
            indexed: true,
            cues: vec![(0, 0)],
        };
        let mut c = Cursor {
            offset: 0,
            need: 0,
            cluster: 0,
            gop: vec![],
            last_dts: i64::MIN,
        };
        let mut out = vec![];
        // A laced pair of sound frames, then a keyframe picture and a flush.
        let sound = [&[0x82u8, 0, 0, 0x04, 1][..], &[5u8; 4]].concat();
        index.block(&mut c, &sound, None, &mut out);
        index.block(&mut c, &[0x81, 0, 0, 0x80, 0x65], None, &mut out);
        index.flush(&mut c, &mut out);
        let data: Vec<&[u8]> = out.iter().map(|f| f.data.as_ref()).collect();
        assert_eq!(
            data,
            [
                &[0x0B, 0x77, 5, 5][..],
                &[0x0B, 0x77, 5, 5],
                &[0, 0, 0, 1, 0x65]
            ]
        );
    }

    fn index() -> Index {
        Index {
            scale: 1_000_000,
            video: 1,
            audio: Some(2),
            default_duration: 40_000,
            sound_frame: 32_000,
            stripped: (vec![], vec![]),
            indexed: true,
            cues: vec![(0, 100), (2_000_000, 900_000)],
        }
    }

    fn cursor(offset: u64) -> Cursor {
        Cursor {
            offset,
            need: 0,
            cluster: 0,
            gop: vec![],
            last_dts: i64::MIN,
        }
    }

    #[test]
    fn a_range_reaches_the_next_keyframe_cluster() {
        let index = index();
        assert_eq!(
            index.range(&cursor(100), 1000, 10 << 20),
            Some((100, 900_000 - 100 + GROUP_MARGIN))
        );
        // Past the last cue, the asked size stands.
        assert_eq!(
            index.range(&cursor(950_000), 1000, 10 << 20),
            Some((950_000, 1000))
        );
    }

    #[test]
    fn a_keyframe_header_completes_the_group_before_it() {
        let index = index();
        let mut c = cursor(0);
        // A keyframe and a picture, then only the start of the next keyframe's block.
        let block = |flags: u8| [0xA3, 0x85, 0x81, 0, 0, flags, 0x65].to_vec();
        let bytes = [block(0x80), block(0), vec![0xA3, 0x88, 0x81, 0, 0, 0x80]].concat();
        let (frames, _) = index.frames(&mut c, 0, &bytes, 1 << 20);
        assert_eq!(frames.len(), 2);
        assert_eq!(c.offset, 14);
    }

    #[test]
    fn a_laced_sound_block_becomes_timed_frames() {
        let index = Index {
            scale: 1_000_000,
            video: 1,
            audio: Some(2),
            default_duration: 40_000,
            sound_frame: 32_000,
            stripped: (vec![], vec![]),
            indexed: true,
            cues: vec![(0, 0)],
        };
        let mut c = Cursor {
            offset: 0,
            need: 0,
            cluster: 1000,
            gop: vec![],
            last_dts: i64::MIN,
        };
        // Track 2, relative time +10 ms, fixed lacing, three 4-byte frames.
        let block = [&[0x82u8, 0, 10, 0x04, 2][..], &[9u8; 12]].concat();
        let mut out = vec![];
        index.block(&mut c, &block, None, &mut out);
        let times: Vec<i64> = out.iter().map(|f| f.pts - SHIFT).collect();
        assert_eq!(times, [1_010_000, 1_042_000, 1_074_000]);
        assert!(
            out.iter()
                .all(|f| f.track == Track::Audio && f.data.len() == 4)
        );
    }

    #[test]
    fn flag_interlaced_one_is_interlaced_and_two_is_progressive() {
        // A Video element (0xE0) holding only FlagInterlaced (0x9A) with a one-byte value.
        let flag = |v: u8| entry(&[0xE0, 0x83, 0x9A, 0x81, v]).video.4;
        assert!(flag(1));
        assert!(!flag(2));
        assert!(!flag(0));
    }
}
