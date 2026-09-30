//! Movies and episodes: plain files (MP4 or Matroska) read a piece at a time and played through
//! MediaSource, with AC-3, E-AC-3 and MP2 sound decoded here, so the picture never has to be
//! re-encoded and the sound never has to be fixed somewhere else while the viewer waits.
//!
//! The file's index says where everything is, so a [`Session`] can start anywhere: it asks for a
//! byte range, is handed those bytes, and gives back one fragmented-MP4 fragment to append. A seek
//! is just a new session at the new time.
//!
//! ponytail: one picture track and one sound track (the first usable one, stereo FLAC for decoded
//! sound), H.264 with 4-byte NAL lengths only, no subtitles, no edit lists beyond the first entry.

use std::rc::Rc;

use crate::{Error, Init, Unsupported, avc, fmp4, mkv, mp4, sound};

/// Every frame time on these types is microseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Track {
    Video,
    Audio,
}

/// One coded picture or block of sound. Times are on the output timeline: what the browser is
/// given, which is the movie's own clock moved forward by [`Movie::shift`].
#[derive(Debug)]
pub struct Frame {
    pub track: Track,
    pub pts: i64,
    /// When it is decoded; equals `pts` for sound.
    pub dts: i64,
    pub dur: i64,
    pub key: bool,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Container {
    Mp4,
    Matroska,
}

impl Container {
    fn mime(self) -> &'static str {
        match self {
            Container::Mp4 => "video/mp4",
            Container::Matroska => "video/x-matroska",
        }
    }
}

#[derive(Debug, Clone)]
pub struct VideoInfo {
    /// "H.264", "HEVC" and so on, for messages.
    pub name: String,
    /// What to ask a browser about it (`avc1.640028`); empty if unknown.
    pub codec: String,
    /// The `avcC` payload, only for H.264 that can be copied as it is.
    pub avcc: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pixel_aspect: (u32, u32),
    pub interlaced: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Audio {
    /// AAC, with its AudioSpecificConfig.
    Aac { asc: Vec<u8> },
    /// Sound we decode ourselves.
    Sound(sound::Kind),
    /// Anything else (DTS, TrueHD, Opus...).
    Other,
}

#[derive(Debug, Clone)]
pub struct AudioInfo {
    pub name: String,
    /// What to ask a browser about it (`ac-3`); empty if it can't play it by itself.
    pub codec: String,
    pub audio: Audio,
    pub channels: u16,
    pub rate: u32,
}

impl VideoInfo {
    /// Fills in H.264 from an `avcC` payload, when its pictures can be copied as they are (they
    /// can if their NAL units are prefixed with 4-byte lengths, which is what nearly all are).
    pub(crate) fn set_h264(&mut self, avcc: &[u8]) {
        self.name = "H.264".into();
        if avcc.len() < 9 || avcc[4] & 3 != 3 {
            return;
        }
        let sps_len = usize::from(u16::from_be_bytes([avcc[6], avcc[7]]));
        let Some(sps) = avcc.get(8..8 + sps_len) else {
            return;
        };
        self.codec = format!("avc1.{:02x}{:02x}{:02x}", avcc[1], avcc[2], avcc[3]);
        self.interlaced = avc::interlaced(sps).unwrap_or(false);
        if let Some(aspect) = avc::pixel_aspect(sps)
            && self.pixel_aspect == (1, 1)
        {
            self.pixel_aspect = aspect;
        }
        self.avcc = avcc.to_vec();
    }
}

/// What to do with a file, settled before playing any of it.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum Verdict {
    /// The browser plays it by itself.
    Native,
    /// We can play it (this module).
    Rust,
    /// Neither, and why. What to do about it (say so, re-encode it elsewhere) is up to the app.
    Unsupported(Unsupported),
}

enum Index {
    Mp4(mp4::Index),
    Mkv(mkv::Index),
}

enum Cursor {
    Mp4(mp4::Cursor),
    Mkv(mkv::Cursor),
}

/// What a file turned out to be: its tracks, its length, and its index.
pub struct Movie {
    pub container: Container,
    pub video: VideoInfo,
    pub audio: Option<AudioInfo>,
    /// Seconds.
    pub duration: f64,
    size: u64,
    /// Microseconds the movie's clock is moved forward by on the output timeline, so nothing the
    /// browser is given is negative or ahead of its decode time. The source buffer takes it back
    /// off again ([`Movie::shift`]).
    shift: i64,
    index: Index,
}

impl Movie {
    /// `moov` is the payload of the file's `moov` box, `size` the file's length.
    pub fn from_mp4(moov: &[u8], size: u64) -> Result<Movie, Error> {
        let p = mp4::parse(moov)?;
        Ok(Movie {
            container: Container::Mp4,
            video: p.video,
            audio: p.audio,
            duration: p.duration as f64 / 1e6,
            size,
            shift: p.shift,
            index: Index::Mp4(p.index),
        })
    }

    pub fn from_mkv(p: mkv::Parsed, size: u64) -> Movie {
        Movie {
            container: Container::Matroska,
            video: p.video,
            audio: p.audio,
            duration: p.duration as f64 / 1e6,
            size,
            shift: mkv::SHIFT,
            index: Index::Mkv(p.index),
        }
    }

    /// The file's average size per second of movie, to size pieces to read by time.
    pub fn bytes_per_second(&self) -> f64 {
        self.size as f64 / self.duration.max(1.0)
    }

    /// Seconds to subtract from the source buffer's timestamps (`timestampOffset`).
    pub fn shift(&self) -> f64 {
        -(self.shift as f64) / 1e6
    }

    fn codecs(&self) -> String {
        [
            self.video.codec.as_str(),
            self.audio.as_ref().map_or("", |a| &a.codec),
        ]
        .iter()
        .filter(|c| !c.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(",")
    }

    /// Whether to let the browser play the file, play it here, or neither.
    /// `can_play` answers a full MIME type with codecs (`canPlayType` in a browser).
    pub fn verdict(&self, can_play: &dyn Fn(&str) -> bool) -> Verdict {
        let known =
            !self.video.codec.is_empty() && self.audio.as_ref().is_none_or(|a| !a.codec.is_empty());
        if known
            && can_play(&format!(
                "{}; codecs=\"{}\"",
                self.container.mime(),
                self.codecs()
            ))
        {
            return Verdict::Native;
        }
        if self.video.avcc.is_empty() {
            return Verdict::Unsupported(Unsupported::Video(Some(self.video.name.clone())));
        }
        if self.video.interlaced {
            return Verdict::Unsupported(Unsupported::Interlaced);
        }
        match &self.audio {
            Some(a) if a.audio == Audio::Other => {
                Verdict::Unsupported(Unsupported::Sound(a.name.clone()))
            }
            _ => Verdict::Rust,
        }
    }

    /// `ftyp` + `moov` and the MIME type for `addSourceBuffer`.
    pub fn init(&self) -> Result<Init, Error> {
        let avcc = &self.video.avcc;
        let bad = || Error::Unsupported("the video's codec setup is unreadable".into());
        // avcC: 6 bytes, then 2-byte lengths before each parameter set.
        let sps_len = usize::from(u16::from_be_bytes(
            avcc.get(6..8).ok_or_else(bad)?.try_into().unwrap(),
        ));
        let sps = avcc.get(8..8 + sps_len).ok_or_else(bad)?;
        let at = 8 + sps_len;
        let pps_len = usize::from(u16::from_be_bytes(
            avcc.get(at + 1..at + 3)
                .ok_or_else(bad)?
                .try_into()
                .unwrap(),
        ));
        let pps = avcc.get(at + 3..at + 3 + pps_len).ok_or_else(bad)?;

        let (track, codec) = match self.audio.as_ref() {
            Some(AudioInfo {
                audio: Audio::Aac { asc },
                channels,
                rate,
                ..
            }) => (
                Some(fmp4::AudioTrack::AacFile {
                    asc,
                    channels: *channels,
                    rate: *rate,
                }),
                ",mp4a.40.2",
            ),
            Some(AudioInfo {
                audio: Audio::Sound(_),
                rate,
                ..
            }) => (Some(fmp4::AudioTrack::Flac { rate: *rate }), ",flac"),
            _ => (None, ""),
        };
        Ok(Init {
            bytes: fmp4::init_segment(
                &fmp4::VideoParams {
                    sps,
                    pps,
                    avcc: Some(avcc),
                    width: self.video.width,
                    height: self.video.height,
                    pixel_aspect: self.video.pixel_aspect,
                },
                track.as_ref(),
            ),
            mime: format!("video/mp4; codecs=\"{}{codec}\"", self.video.codec),
            interlaced: self.video.interlaced,
        })
    }

    /// Begins reading at `at` seconds (at the picture's keyframe just before it).
    pub fn session(self: &Rc<Self>, at: f64) -> Session {
        let at = (at.max(0.0) * 1e6) as i64 + self.shift;
        Session {
            movie: self.clone(),
            cursor: match &self.index {
                Index::Mp4(i) => Cursor::Mp4(i.cursor(at)),
                Index::Mkv(i) => Cursor::Mkv(i.cursor(at)),
            },
            frag: Fragmenter::new(self.audio.as_ref()),
            asked: (0, 0),
            done: false,
        }
    }
}

/// Reading a movie from one moment to its end.
pub struct Session {
    movie: Rc<Movie>,
    cursor: Cursor,
    frag: Fragmenter,
    asked: (u64, u64),
    done: bool,
}

impl Session {
    /// The next piece of the file to fetch (offset, length), about `max` bytes; `None` at the end.
    pub fn range(&mut self, max: u64) -> Option<(u64, u64)> {
        if self.done {
            return None;
        }
        let r = match (&self.movie.index, &self.cursor) {
            (Index::Mp4(i), Cursor::Mp4(c)) => i.range(c, max),
            (Index::Mkv(i), Cursor::Mkv(c)) => i.range(c, max, self.movie.size),
            _ => None,
        };
        self.asked = r.unwrap_or((0, 0));
        r
    }

    /// Hands over the bytes of the last range. Returns what to append (maybe nothing yet).
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<u8>, Error> {
        let (start, _) = self.asked;
        let (frames, done) = match (&self.movie.index, &mut self.cursor) {
            (Index::Mp4(i), Cursor::Mp4(c)) => {
                let frames = i.frames(c, start, bytes);
                (frames, i.done(c))
            }
            (Index::Mkv(i), Cursor::Mkv(c)) => {
                let last = start + bytes.len() as u64 >= self.movie.size;
                i.frames(c, start, bytes, last)
            }
            _ => unreachable!("a session reads the kind of file it was made for"),
        };
        if frames.is_empty() && !done && bytes.len() < self.asked.1 as usize {
            return Err(Error::Unsupported(
                "the server sent less than asked for".into(),
            ));
        }
        self.done = done;
        Ok(self.frag.push(frames, done))
    }

    pub fn done(&self) -> bool {
        self.done
    }

    /// Seconds of the movie produced so far.
    pub fn reached(&self) -> f64 {
        (self.frag.reach - self.movie.shift) as f64 / 1e6
    }
}

/// Puts frames into fragments: one picture run and one sound run each, sound decoded on the way.
struct Fragmenter {
    audio: Option<(Audio, u32)>,
    decoder: sound::Decoder,
    seq: u32,
    /// Where the picture starts (its first keyframe): nothing earlier is kept.
    floor: Option<i64>,
    /// How far the pictures handed out so far reach.
    reach: i64,
    /// Sound waiting for the picture to catch up with it.
    waiting: Vec<Frame>,
    audio_next: Option<u64>,
    flac_frames: u32,
}

/// The longest a frame of AC-3, E-AC-3, MP2 or AAC is, in microseconds (32 ms).
const SOUND_FRAME: i64 = 33_000;

/// 90 kHz ticks for a time in microseconds.
fn ticks(us: i64) -> i64 {
    (us * 9 + 50).div_euclid(100)
}

impl Fragmenter {
    fn new(audio: Option<&AudioInfo>) -> Self {
        Fragmenter {
            audio: audio.map(|a| (a.audio.clone(), a.rate)),
            decoder: sound::Decoder::new(),
            seq: 0,
            floor: None,
            reach: 0,
            waiting: vec![],
            audio_next: None,
            flac_frames: 0,
        }
    }

    fn push(&mut self, frames: Vec<Frame>, last: bool) -> Vec<u8> {
        let mut video = vec![];
        for f in frames {
            match f.track {
                Track::Video => match self.floor {
                    None if !f.key => {}
                    None => {
                        self.floor = Some(f.pts);
                        video.push(f);
                    }
                    // Pictures that belong before the start (they lean on the previous group).
                    Some(floor) if f.pts < floor => {}
                    Some(_) => video.push(f),
                },
                Track::Audio => self.waiting.push(f),
            }
        }
        if let Some(end) = video.iter().map(|f| f.pts + f.dur).max() {
            self.reach = self.reach.max(end);
        }

        // Sound goes out with the picture it belongs with, not a whole chunk ahead of it, and
        // nothing before the picture's start is kept.
        let horizon = if last { i64::MAX } else { self.reach };
        let floor = self.floor.unwrap_or(i64::MIN);
        let (ready, later): (Vec<Frame>, Vec<Frame>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|f| f.pts < horizon);
        self.waiting = later;
        // (A frame that begins a little early still holds the start of the first moment.)
        let sound: Vec<Frame> = ready
            .into_iter()
            .filter(|f| f.pts.saturating_add(SOUND_FRAME) >= floor)
            .collect();

        let mut runs = vec![];
        if !video.is_empty() {
            // A run's times come from its durations alone, so each lasts until the next one
            // that is kept begins (the last for as long as it says).
            let samples = video
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let (dts, pts) = (ticks(f.dts), ticks(f.pts));
                    let next = video
                        .get(i + 1)
                        .map_or(ticks(f.dts + f.dur), |n| ticks(n.dts));
                    fmp4::Sample {
                        duration: (next - dts).max(1) as u32,
                        key: f.key,
                        cts: (pts - dts) as i32,
                        data: &f.data,
                    }
                })
                .collect();
            runs.push(fmp4::TrackRun {
                track: fmp4::VIDEO_TRACK,
                base_time: ticks(video[0].dts).max(0) as u64,
                samples,
            });
        }
        let coded = self.coded_sound(&sound);
        if !coded.is_empty() {
            let rate = u64::from(self.audio.as_ref().map_or(48_000, |a| a.1));
            let derived = (sound[0].pts.max(0) as u64) * rate / 1_000_000;
            let tolerance = u64::from(coded[0].1);
            let start = match self.audio_next {
                Some(next) if derived.abs_diff(next) <= tolerance => next,
                _ => derived,
            };
            self.audio_next = Some(start + coded.iter().map(|c| u64::from(c.1)).sum::<u64>());
            runs.push(fmp4::TrackRun {
                track: fmp4::AUDIO_TRACK,
                base_time: start,
                samples: coded
                    .iter()
                    .map(|(data, duration)| fmp4::Sample {
                        duration: *duration,
                        key: true,
                        cts: 0,
                        data,
                    })
                    .collect(),
            });
        }
        if runs.is_empty() {
            return vec![];
        }
        self.seq += 1;
        fmp4::fragment(self.seq, &runs)
    }

    /// Sound as the samples of the audio track: (bytes, length in sample-rate ticks).
    fn coded_sound(&mut self, frames: &[Frame]) -> Vec<(Vec<u8>, u32)> {
        let mut out = vec![];
        match self.audio.as_ref().map(|a| &a.0) {
            Some(Audio::Aac { .. }) => {
                out.extend(frames.iter().map(|f| (f.data.clone(), 1024)));
            }
            Some(Audio::Sound(kind)) => {
                for f in frames {
                    for part in sound::split(*kind, &f.data) {
                        let pcm = self.decoder.decode(*kind, &part);
                        out.push((
                            sound::flac_frame(&pcm, self.flac_frames),
                            (pcm.len() / 2) as u32,
                        ));
                        self.flac_frames += 1;
                    }
                }
            }
            _ => {}
        }
        out
    }
}
