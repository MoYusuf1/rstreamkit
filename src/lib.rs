//! rstreamkit: a pure-Rust streaming toolkit for the browser. HLS playlists, MPEG-TS
//! demuxing, fMP4 muxing and AC-3, E-AC-3 and MP2 sound decoding (pure, testable natively), movie
//! files (MP4 and Matroska, `vod`), plus the MediaSource glue that feeds a `<video>` element (wasm
//! only, `mse`).

//!
//! The media core runs on native Rust targets and WebAssembly. `mse` adds browser playback
//! on wasm; [`net::Fetch`] and [`net::Sink`] let other hosts supply their own I/O.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use std::io::Write;
//! let input = std::fs::read("stream.ts")?;
//! let out = rstreamkit::Transmuxer::default().push(&input)?;
//! let mut file = std::fs::File::create("output.mp4")?;
//! if let Some(init) = &out.init { file.write_all(&init.bytes)?; }
//! for fragment in &out.fragments {
//!     file.write_all(&fragment.moof)?;
//!     file.write_all(&fragment.mdat)?;
//! }
//! # Ok(())
//! # }
//! ```

pub mod avc;
pub mod body;
#[cfg(any(target_arch = "wasm32", test))]
mod cancel;
pub mod cmaf;
pub mod fmp4;
pub mod hls;
#[cfg(any(target_arch = "wasm32", test))]
mod live;
#[cfg(feature = "vod")]
pub mod mkv;
#[cfg(feature = "vod")]
pub mod mp4;
#[cfg(target_arch = "wasm32")]
pub mod mse;
pub mod net;
pub mod sound;
pub mod ts;
#[cfg(feature = "vod")]
pub mod vod;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Ts(#[from] ts::Error),
    #[error("bad playlist: {0}")]
    Playlist(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Capability(Unsupported),
}

impl Error {
    /// Whether the stream is fine but not something this browser can play, as opposed to broken.
    /// What to do about it (say so, play without the sound, hand it to something else) is up to the
    /// app.
    pub fn unsupported(&self) -> Option<Unsupported> {
        match self {
            Error::Capability(why) => Some(why.clone()),
            Error::Ts(ts::Error::NoVideo(codec)) => Some(Unsupported::Video(codec.clone())),
            _ => None,
        }
    }
}

/// Why a stream can't be played as it is. Its `Display` reads as a sentence for the viewer.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Unsupported {
    /// Video that isn't H.264 (HEVC, MPEG-2, ...): what it is, if it could be named.
    Video(Option<String>),
    /// Sound the browser can't play and that isn't being decoded here: the codec.
    Sound(String),
    /// Interlaced pictures, which play combed and at half the motion rate.
    Interlaced,
    /// A raw MPEG-TS stream where an HLS playlist should be.
    RawStream,
    /// A media type this browser's MediaSource refuses, and what the browser said.
    MediaType { mime: String, why: String },
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unsupported::Video(Some(codec)) => write!(f, "the video isn't H.264 (it is {codec})"),
            Unsupported::Video(None) => f.write_str("no supported audio or video was found"),
            Unsupported::Sound(codec) => write!(f, "{codec} sound can't be played by this browser"),
            Unsupported::Interlaced => f.write_str("interlaced video"),
            Unsupported::RawStream => f.write_str(hls::RAW_STREAM),
            Unsupported::MediaType { mime, why } => {
                write!(f, "this browser can't play {mime}: {why}")
            }
        }
    }
}

/// What to give `MediaSource.addSourceBuffer` and then append first.
#[derive(Debug)]
#[non_exhaustive]
pub struct Init {
    pub bytes: Vec<u8>,
    pub mime: String,
    /// Interlaced pictures play combed, at half the motion rate, in a browser.
    pub interlaced: bool,
}

#[derive(Debug)]
#[non_exhaustive]
pub struct Output {
    /// Present at startup and whenever codec parameters or track layout change.
    pub init: Option<Init>,
    /// What to append, in this order: a `moof` and `mdat` for each track that has samples in the
    /// segment, the pictures first. (Empty if it held none.) Tracks get their own, which a browser
    /// takes as it does any other, so the pictures, the bulk of it, go out in the buffer they were
    /// written in, with nothing copied around them.
    pub fragments: Vec<Fragment>,
    pub skipped_audio: Option<String>,
}

/// A `moof` box and the `mdat` box that follows it.
#[derive(Debug)]
#[non_exhaustive]
pub struct Fragment {
    pub moof: Vec<u8>,
    pub mdat: Vec<u8>,
}

impl Output {
    /// Every fragment in one buffer (a copy), for when one piece is what is wanted.
    pub fn fragment(&self) -> Vec<u8> {
        self.fragments
            .iter()
            .flat_map(|f| [f.moof.as_slice(), f.mdat.as_slice()])
            .collect::<Vec<_>>()
            .concat()
    }
}

/// Room at the front of a segment's buffer for the `mdat` header.
const MDAT_HEADER: usize = 8;

const WRAP: u64 = 1 << 33; // PES timestamps are 33 bits
/// A segment starting further than this (90 kHz ticks, 2 s) from where the last one ended is a
/// new timeline (stream restart, ad splice, missing tag) and gets glued on instead of leaving a gap.
const JUMP: u64 = 2 * 90_000;

/// The current representation and sample rate of a stream's audio track.
#[derive(Default, Clone, Copy, PartialEq)]
enum Audio {
    #[default]
    None,
    /// AAC, passed through as it is.
    Aac,
    /// Sound we decode ourselves (AC-3, E-AC-3, MP2) and write as FLAC: its kind and sample rate.
    Sound(sound::Kind, u32),
    Dolby(u32),
}

/// Turns consecutive HLS TS segments into a continuous fMP4 stream.
#[derive(Default)]
pub struct Transmuxer {
    /// Source time (90 kHz, unwrapped) that maps to zero in the output, for the picture (`None`
    /// until the first segment) and for the sound. Negative if the source clock restarted below the
    /// media already sent. They move apart at a discontinuity, where each track carries on exactly
    /// where it stopped.
    vbase: Option<i64>,
    abase: i64,
    /// Last (unwrapped) timestamp seen, used to resolve the 33-bit rollover.
    last: u64,
    /// Where the picture and the sound of the previous fragment ended, in 90 kHz ticks.
    vend: u64,
    aend: u64,
    seq: u32,
    sent_init: bool,
    reset_clock: bool,
    sps: Vec<u8>,
    pps: Vec<u8>,
    aac: Option<ts::AacConfig>,
    audio: Audio,
    audio_next: Option<u64>,
    decoder: sound::Decoder,
    /// FLAC frames written so far; each frame carries its number.
    flac_frames: u32,
    skip_sound: bool,
    passthrough_ac3: bool,
    audio_language: Option<String>,
    dolby: Option<sound::DolbyConfig>,
    prepared_flac: Option<Vec<(Vec<u8>, u32)>>,
}

impl Transmuxer {
    /// Whether to decode AC-3, E-AC-3 and MP2 sound ourselves (the default). Off, that sound is
    /// dropped and named in `Output::skipped_audio`, for the caller to get converted elsewhere.
    pub fn decode_sound(mut self, on: bool) -> Self {
        self.skip_sound = !on;
        self
    }

    pub fn audio_language(mut self, language: Option<&str>) -> Self {
        self.audio_language = language.map(str::to_owned);
        self
    }
    /// Begin a streamed segment using this transmuxer's track selection.
    pub fn segment(&self, expected: usize) -> Segment {
        Segment::new(expected).audio_language(self.audio_language.as_deref())
    }

    /// Opt in to AC-3 passthrough after verifying platform support. E-AC-3 remains decoded.
    pub fn passthrough_ac3(mut self, on: bool) -> Self {
        self.passthrough_ac3 = on;
        self
    }

    /// Glue the next segment onto the previous track ends (explicit discontinuity or loss).
    pub fn discontinuity(&mut self) {
        self.reset_clock = true;
    }

    /// Picks the representation of a 33-bit timestamp nearest to the last one seen.
    fn unwrap(&mut self, t: u64) -> u64 {
        let mut u = t + self.last / WRAP * WRAP;
        if u + WRAP / 2 < self.last {
            u += WRAP;
        } else if u > self.last + WRAP / 2 && u >= WRAP {
            u -= WRAP;
        }
        self.last = self.last.max(u);
        u
    }

    /// A whole segment at once. See [`segment`](Self::segment) for one that arrives in pieces.
    pub fn push(&mut self, segment: &[u8]) -> Result<Output, Error> {
        let mut s = self.segment(segment.len());
        s.feed(segment);
        self.finish(s)
    }

    /// Turns a segment that arrived in pieces (see [`Segment`]) into what to append.
    pub fn finish(&mut self, segment: Segment) -> Result<Output, Error> {
        let d = segment.demuxer.finish()?;
        self.assemble(d)
    }

    /// Browser-friendly decoding: yield between small batches of audio frames.
    #[cfg(target_arch = "wasm32")]
    pub async fn finish_yielded(&mut self, segment: Segment) -> Result<Output, Error> {
        let d = segment.demuxer.finish()?;
        if cfg!(feature = "sound")
            && !self.skip_sound
            && !d.sound.is_empty()
            && !(self.passthrough_ac3
                && sound::dolby_config(&d.sound[0].data)
                    .is_some_and(|c| !c.enhanced || d.simple_eac3))
            && let Some(kind) = d.sound_kind
        {
            let next = Audio::Sound(kind, d.sound[0].rate);
            if self.audio != next {
                self.decoder = sound::Decoder::new();
            }
            let mut frames = Vec::with_capacity(d.sound.len());
            for (i, f) in d.sound.iter().enumerate() {
                let coded = sound::Coded {
                    data: &f.data,
                    samples: f.samples,
                    rate: f.rate,
                    channels: f.channels,
                };
                let pcm = self.decoder.decode(kind, &coded);
                frames.push((
                    sound::flac_frame(&pcm, self.flac_frames),
                    (pcm.len() / 2) as u32,
                ));
                self.flac_frames += 1;
                if i % 8 == 7 {
                    crate::mse::sleep(std::time::Duration::ZERO).await;
                }
            }
            self.prepared_flac = Some(frames);
        }
        self.assemble(d)
    }

    /// `d`'s pictures must begin after room for the `mdat` header (see `segment`).
    pub(crate) fn assemble(&mut self, mut d: ts::Demuxed) -> Result<Output, Error> {
        let dolby = if self.passthrough_ac3 {
            d.sound
                .first()
                .and_then(|f| sound::dolby_config(&f.data))
                .filter(|c| !c.enhanced || d.simple_eac3)
        } else {
            None
        };
        if (self.skip_sound || !cfg!(feature = "sound")) && dolby.is_none() {
            d.sound.clear();
            d.sound_kind = None;
        } else if !d.sound.is_empty() {
            d.skipped_audio = None; // we decode it
        }

        if d.video.is_empty()
            && d.audio.is_empty()
            && d.sound.is_empty()
            && let Some(codec) = &d.skipped_audio
        {
            return Err(Error::Capability(Unsupported::Sound(codec.clone())));
        }
        let mut changed = false;
        if let Some(sps) = d.sps.as_ref()
            && *sps != self.sps
        {
            self.sps = sps.clone();
            changed = true;
        }
        if let Some(pps) = d.pps.as_ref()
            && *pps != self.pps
        {
            self.pps = pps.clone();
            changed = true;
        }
        if dolby != self.dolby && dolby.is_some() {
            changed = true;
        }
        if dolby.is_some() {
            self.dolby = dolby.clone();
        }
        let next_audio = match (&d.aac, d.sound_kind, d.sound.first()) {
            (Some(_), _, _) => Audio::Aac,
            (None, Some(_), Some(_)) if dolby.is_some() => {
                Audio::Dolby(dolby.as_ref().unwrap().rate)
            }
            (None, Some(kind), Some(first)) => Audio::Sound(kind, first.rate),
            _ => self.audio, // A video-only segment doesn't remove an established sound track.
        };
        if d.aac.is_some() && d.aac != self.aac {
            self.aac = d.aac;
            changed = true;
        }
        if next_audio != self.audio || changed && next_audio == Audio::Aac {
            let rate = match next_audio {
                Audio::Aac => self.aac.as_ref().map_or(48_000, |c| c.sample_rate()),
                Audio::Sound(_, rate) | Audio::Dolby(rate) => rate,
                Audio::None => 90_000,
            };
            self.audio_next = self
                .sent_init
                .then_some(self.aend * u64::from(rate) / 90_000);
            if next_audio != self.audio && self.prepared_flac.is_none() {
                self.decoder = sound::Decoder::new();
            }
            self.audio = next_audio;
            changed = true;
        }
        let init = if self.sent_init && !changed {
            None
        } else {
            let sps = self.sps.as_slice();
            if !d.video.is_empty() && (sps.len() < 4 || self.pps.is_empty()) {
                return Err(ts::Error::NoVideoParams.into());
            }
            let pps = self.pps.as_slice();
            let (width, height) = if sps.is_empty() {
                (0, 0)
            } else {
                avc::dimensions(sps)
                    .ok_or_else(|| ts::Error::BadSps("cannot read the picture size".into()))?
            };
            let (track, audio) = match self.audio {
                // Browsers only accept `mp4a.40.2` (AAC-LC) here, and decode Main and the rest of
                // the family fine when told so; declaring the real type (Main is `.1`) is refused.
                Audio::Aac => (self.aac.as_ref().map(fmp4::AudioTrack::Aac), ",mp4a.40.2"),
                Audio::Sound(_, rate) => (Some(fmp4::AudioTrack::Flac { rate }), ",flac"),
                Audio::Dolby(rate) => {
                    let cfg = self.dolby.as_ref().unwrap();
                    (
                        Some(fmp4::AudioTrack::Dolby {
                            rate,
                            channels: cfg.channels,
                            enhanced: cfg.enhanced,
                            config: &cfg.bytes,
                        }),
                        if cfg.enhanced { ",ec-3" } else { ",ac-3" },
                    )
                }
                Audio::None => (None, ""),
            };
            self.sent_init = true;
            Some(Init {
                bytes: if sps.is_empty() {
                    fmp4::audio_init_segment(track.as_ref().ok_or(ts::Error::NoVideo(None))?)
                } else {
                    fmp4::init_segment(
                        &fmp4::VideoParams {
                            sps,
                            pps,
                            avcc: None,
                            width,
                            height,
                            pixel_aspect: avc::pixel_aspect(sps).unwrap_or((1, 1)),
                        },
                        track.as_ref(),
                    )
                },
                interlaced: avc::interlaced(sps).unwrap_or(false),
                mime: if sps.is_empty() {
                    format!("audio/mp4; codecs=\"{}\"", audio.trim_start_matches(','))
                } else {
                    format!(
                        "video/mp4; codecs=\"avc1.{:02x}{:02x}{:02x}{audio}\"",
                        sps[1], sps[2], sps[3]
                    )
                },
            })
        };

        let video: Vec<(u64, u64)> = d
            .video
            .iter()
            .map(|s| (self.unwrap(s.dts), self.unwrap(s.pts)))
            .collect();
        let audio: Vec<u64> = d.audio.iter().map(|s| self.unwrap(s.pts)).collect();
        let sound_pts: Vec<u64> = d.sound.iter().map(|s| self.unwrap(s.pts)).collect();
        let video_first = video.first().map(|v| v.0);
        let sound_first = audio.first().or(sound_pts.first()).copied();
        let Some(first) = video_first.into_iter().chain(sound_first).min() else {
            return Ok(Output {
                init,
                fragments: vec![],
                skipped_audio: d.skipped_audio,
            });
        };

        // Timelines start at zero. A jump in the source timeline (a restart, an ad splice, a missing
        // tag) is re-based so that each track carries on exactly where it stopped: a hole of even a
        // few milliseconds in either one stalls playback in a browser. The price is that the tracks
        // keep the offset between them that they had before the jump.
        let reset_clock = std::mem::take(&mut self.reset_clock);
        let (vbase, abase, jumped) = match self.vbase {
            None => (first as i64, first as i64, false),
            Some(vbase) => {
                // Where this segment should start in source time, if the stream goes on as it was.
                let expected = [
                    video_first.map(|_| vbase + self.vend as i64),
                    sound_first.map(|_| self.abase + self.aend as i64),
                ]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or(0);
                if !reset_clock && (first as i64).abs_diff(expected) <= JUMP {
                    (vbase, self.abase, false)
                } else {
                    (
                        video_first.map_or(vbase, |t| t as i64 - self.vend as i64),
                        sound_first.map_or(self.abase, |t| t as i64 - self.aend as i64),
                        true,
                    )
                }
            }
        };
        (self.vbase, self.abase) = (Some(vbase), abase);
        let relv = |t: u64| (t as i64 - vbase).max(0) as u64;
        let rela = |t: u64| (t as i64 - abase).max(0) as u64;

        let mut runs = vec![];
        if !video.is_empty() {
            let mut last_dur = 3003; // ~29.97 fps, only used if the segment has a single frame
            let mut at = MDAT_HEADER;
            let samples: Vec<_> = d
                .video
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let dur = video
                        .get(i + 1)
                        .map_or(last_dur, |n| n.0.saturating_sub(video[i].0) as u32);
                    last_dur = dur;
                    let data = &d.video_data[at..at + s.len as usize];
                    at += s.len as usize;
                    fmp4::Sample {
                        duration: dur,
                        key: s.key,
                        cts: (video[i].1 as i64 - video[i].0 as i64) as i32,
                        data,
                    }
                })
                .collect();
            let derived = relv(video[0].0);
            // Snap sub-frame duration estimates at ordinary boundaries, keeping larger
            // source gaps visible to recovery rather than accumulating jitter.
            let start = if self.seq > 0 && derived.abs_diff(self.vend) <= u64::from(last_dur) {
                self.vend
            } else {
                derived
            };
            self.vend = start + samples.iter().map(|s| s.duration as u64).sum::<u64>();
            runs.push(fmp4::TrackRun {
                track: fmp4::VIDEO_TRACK,
                base_time: start,
                samples,
            });
        }
        if let (Some(cfg), Audio::Aac, false) = (d.aac, self.audio, audio.is_empty()) {
            let rate = cfg.sample_rate() as u64;
            let derived = rela(audio[0]) * rate / 90_000;
            // Keep audio gapless: 90 kHz -> sample-rate rounding must not open 1-tick holes between fragments.
            let start = match self.audio_next {
                Some(next) if jumped || derived.abs_diff(next) <= 1024 => next,
                _ => derived,
            };
            let stop = start + 1024 * audio.len() as u64;
            self.audio_next = Some(stop);
            self.aend = stop * 90_000 / rate;
            let samples = d
                .audio
                .iter()
                .map(|s| fmp4::Sample {
                    duration: 1024,
                    key: true,
                    cts: 0,
                    data: &s.data,
                })
                .collect();
            runs.push(fmp4::TrackRun {
                track: fmp4::AUDIO_TRACK,
                base_time: start,
                samples,
            });
        }
        if let Audio::Dolby(rate) = self.audio
            && !d.sound.is_empty()
        {
            let rate = u64::from(rate);
            let derived = rela(sound_pts[0]) * rate / 90_000;
            let start = match self.audio_next {
                Some(next) if jumped || derived.abs_diff(next) <= 1536 => next,
                _ => derived,
            };
            let duration: u64 = d.sound.iter().map(|f| u64::from(f.samples)).sum();
            self.audio_next = Some(start + duration);
            self.aend = (start + duration) * 90_000 / rate;
            runs.push(fmp4::TrackRun {
                track: fmp4::AUDIO_TRACK,
                base_time: start,
                samples: d
                    .sound
                    .iter()
                    .map(|f| fmp4::Sample {
                        duration: f.samples,
                        key: true,
                        cts: 0,
                        data: &f.data,
                    })
                    .collect(),
            });
        }
        // Sound we decode: each frame becomes PCM, then one FLAC frame, one sample in the track.
        let mut flac = vec![];
        if let (Audio::Sound(kind, rate), false) = (self.audio, d.sound.is_empty()) {
            if let Some(prepared) = self.prepared_flac.take() {
                flac = prepared;
            } else {
                for f in &d.sound {
                    let coded = sound::Coded {
                        data: &f.data,
                        samples: f.samples,
                        rate: f.rate,
                        channels: f.channels,
                    };
                    let pcm = self.decoder.decode(kind, &coded);
                    flac.push((
                        sound::flac_frame(&pcm, self.flac_frames),
                        (pcm.len() / 2) as u32,
                    ));
                    self.flac_frames += 1;
                }
            }
            let rate = u64::from(rate);
            let derived = rela(sound_pts[0]) * rate / 90_000;
            // Gapless, as for AAC: rounding 90 kHz ticks to samples must not open holes.
            let tolerance = u64::from(flac[0].1);
            let start = match self.audio_next {
                Some(next) if jumped || derived.abs_diff(next) <= tolerance => next,
                _ => derived,
            };
            let stop = start + flac.iter().map(|f| u64::from(f.1)).sum::<u64>();
            self.audio_next = Some(stop);
            self.aend = stop * 90_000 / rate;
            runs.push(fmp4::TrackRun {
                track: fmp4::AUDIO_TRACK,
                base_time: start,
                samples: flac
                    .iter()
                    .map(|(data, samples)| fmp4::Sample {
                        duration: *samples,
                        key: true,
                        cts: 0,
                        data,
                    })
                    .collect(),
            });
        }
        // Independent gapless splices can accumulate skew. Move the video clock toward
        // the audio clock by distributing a bounded (2%) timing correction across a run.
        // Track starts remain contiguous; changing the base keeps the following source
        // timestamps on the corrected clock instead of opening a hole at the next boundary.
        if runs.iter().any(|r| r.track == fmp4::AUDIO_TRACK)
            && let Some(run) = runs.iter_mut().find(|r| r.track == fmp4::VIDEO_TRACK)
        {
            let skew = vbase - self.abase;
            if skew.unsigned_abs() > 1800 {
                let total: i64 = run.samples.iter().map(|s| i64::from(s.duration)).sum();
                let correction = skew.clamp(-total / 50, total / 50);
                let count = run.samples.len() as i64;
                let mut applied = 0i64;
                for (i, sample) in run.samples.iter_mut().enumerate() {
                    let cumulative = correction * (i as i64 + 1) / count;
                    let delta = cumulative - applied;
                    let duration = (i64::from(sample.duration) + delta).clamp(1, u32::MAX as i64);
                    applied += duration - i64::from(sample.duration);
                    sample.duration = duration as u32;
                }
                self.vend = (self.vend as i64 + applied).max(0) as u64;
                self.vbase = Some(vbase - applied);
            }
        }
        // One `moof` and `mdat` for each track: nothing is copied into anything bigger.
        let moofs: Vec<Vec<u8>> = runs
            .iter()
            .map(|run| {
                self.seq += 1;
                fmp4::moof(self.seq, std::slice::from_ref(run))
            })
            .collect();
        let mut mdats: Vec<Vec<u8>> = runs
            .iter()
            .map(|run| {
                if run.track == fmp4::VIDEO_TRACK {
                    return vec![]; // the pictures are already in the buffer they were written in
                }
                let len: usize = run.samples.iter().map(|s| s.data.len()).sum();
                let mut mdat = Vec::with_capacity(MDAT_HEADER + len);
                mdat.extend(fmp4::mdat_header(len));
                run.samples
                    .iter()
                    .for_each(|s| mdat.extend_from_slice(s.data));
                mdat
            })
            .collect();
        let with_pictures = runs.first().is_some_and(|r| r.track == fmp4::VIDEO_TRACK);
        drop(runs);
        if with_pictures {
            let mut mdat = d.video_data;
            let header = fmp4::mdat_header(mdat.len() - MDAT_HEADER);
            mdat[..MDAT_HEADER].copy_from_slice(&header);
            mdats[0] = mdat;
        }
        Ok(Output {
            init,
            fragments: moofs
                .into_iter()
                .zip(mdats)
                .map(|(moof, mdat)| Fragment { moof, mdat })
                .collect(),
            skipped_audio: d.skipped_audio,
        })
    }
}

/// Bounded continuous MPEG-TS input, cut into output fragments at keyframes.
/// Unlike `Segment`, program tables and incomplete PES packets survive between feeds.
pub struct Continuous {
    demuxer: ts::Demuxer,
    transmuxer: Transmuxer,
    limit: usize,
}
impl Default for Continuous {
    fn default() -> Self {
        Self::new(64 << 20)
    }
}
impl Continuous {
    pub fn new(limit: usize) -> Self {
        Self {
            demuxer: ts::Demuxer::new(vec![0; MDAT_HEADER]),
            transmuxer: Transmuxer::default(),
            limit,
        }
    }
    pub fn decode_sound(mut self, on: bool) -> Self {
        self.transmuxer = self.transmuxer.decode_sound(on);
        self
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Output>, Error> {
        let mut out = vec![];
        // Even a caller handing over a giant chunk cannot allocate it all before checking the cap.
        for bytes in bytes.chunks(16 << 10) {
            self.demuxer.feed(bytes);
            while let Some(d) = self.demuxer.take_fragment()? {
                out.push(self.transmuxer.assemble(d)?);
            }
            if self.demuxer.buffered_bytes() > self.limit {
                return Err(Error::Unsupported(
                    "continuous TS exceeded its keyframe buffer limit".into(),
                ));
            }
        }
        Ok(out)
    }
    pub fn finish(mut self) -> Result<Output, Error> {
        self.transmuxer.finish(Segment {
            demuxer: self.demuxer,
        })
    }
}

/// One segment as it arrives, as a download delivers it: [`feed`](Self::feed) it what comes, then
/// hand it to [`Transmuxer::finish`]. Nothing holds the whole segment, and the pictures are written
/// straight into the buffer they leave in, so it takes about its own size in memory, not several
/// times that. It touches nothing else, so one dropped unfinished (a failed download) leaves no
/// trace.
pub struct Segment {
    demuxer: ts::Demuxer,
}

impl Segment {
    /// `expected` is how big the segment will be, if known (`Content-Length`): room for it is
    /// allocated once.
    pub fn new(expected: usize) -> Segment {
        let mut video = Vec::with_capacity(MDAT_HEADER + expected);
        video.resize(MDAT_HEADER, 0);
        Segment {
            demuxer: ts::Demuxer::new(video),
        }
    }

    /// The next piece of the segment; pieces can be cut anywhere.
    pub fn audio_language(mut self, language: Option<&str>) -> Self {
        self.demuxer = self.demuxer.audio_language(language);
        self
    }

    pub fn feed(&mut self, chunk: &[u8]) {
        self.demuxer.feed(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "sound")]
    #[test]
    fn sound_decoding_can_be_switched_off() {
        let ac3 = include_bytes!("../tests/fixtures/h264_ac3.ts");
        let on = Transmuxer::default().push(ac3).unwrap();
        assert!(on.init.unwrap().mime.ends_with(",flac\""));
        assert_eq!(on.skipped_audio, None);

        let off = Transmuxer::default().decode_sound(false).push(ac3).unwrap();
        let mime = off.init.unwrap().mime;
        assert!(!mime.contains("flac") && !mime.contains("mp4a"), "{mime}");
        assert_eq!(off.skipped_audio.as_deref(), Some("AC-3"));
        assert!(!off.fragments.is_empty(), "the picture still plays");
    }

    #[test]
    fn timestamp_unwrap_survives_the_33_bit_rollover() {
        let mut t = Transmuxer::default();
        let near_end = WRAP - 90_000;
        assert_eq!(t.unwrap(near_end), near_end);
        // 0.5 s later the counter wrapped to a small number; we must keep counting up.
        assert_eq!(t.unwrap(45_000 - 1), WRAP + 45_000 - 1);
        assert_eq!(
            t.unwrap(near_end + 1000),
            near_end + 1000,
            "a slightly older timestamp stays put"
        );
    }

    /// The base time of each track in a fragment (picture first, then sound).
    fn base_times(fragment: &[u8]) -> Vec<u64> {
        fragment
            .windows(4)
            .enumerate()
            .filter(|(_, w)| *w == b"tfdt")
            .map(|(at, _)| u64::from_be_bytes(fragment[at + 8..at + 16].try_into().unwrap()))
            .collect()
    }

    /// `segment` with every PES timestamp moved by `delta` ticks: the same media on another clock.
    fn shifted(segment: &[u8], delta: i64) -> Vec<u8> {
        let mut out = segment.to_vec();
        for packet in out.as_chunks_mut::<188>().0 {
            let payload = match (packet[3] >> 4) & 3 {
                1 => 4,
                3 => 5 + packet[4] as usize,
                _ => continue,
            };
            // A PES packet starts here, with a PTS (and maybe a DTS) in the header.
            if packet[1] & 0x40 == 0 || packet[payload..payload + 3] != [0, 0, 1] {
                continue;
            }
            let flags = packet[payload + 7] >> 6;
            for (at, present) in [(payload + 9, flags & 2 != 0), (payload + 14, flags == 3)] {
                if !present {
                    continue;
                }
                let t = ts::read_ts(&packet[at..at + 5]);
                let t = (t as i64 + delta).rem_euclid(WRAP as i64) as u64;
                packet[at] = (packet[at] & 0xF0) | (((t >> 30) & 7) as u8) << 1 | 1;
                packet[at + 1] = (t >> 22) as u8;
                packet[at + 2] = (((t >> 15) & 0x7F) as u8) << 1 | 1;
                packet[at + 3] = (t >> 7) as u8;
                packet[at + 4] = ((t & 0x7F) as u8) << 1 | 1;
            }
        }
        out
    }

    /// A browser stalls at any hole, however small, so after a discontinuity each track has to
    /// start exactly where it stopped, not merely near it. (This stalled in Chrome, at a 90 ms hole,
    /// while the segments were re-based together: the sound outlasted the picture, and the picture
    /// began after the later of the two.) The same media is moved onto other clocks: far ahead, a
    /// little ahead, back to an earlier time than what was already sent, and far ahead again.
    #[test]
    fn each_track_carries_on_exactly_where_it_stopped_after_a_jump() {
        let aac: &[u8] = include_bytes!("../tests/fixtures/bbb_480p.ts");
        let ac3: &[u8] = include_bytes!("../tests/fixtures/h264_ac3_st.ts");
        // Without the decoders the sound isn't there to be continuous.
        let cases = [("AAC", aac), ("AC-3 decoded", ac3)];
        for (name, segment) in cases
            .into_iter()
            .take(if cfg!(feature = "sound") { 2 } else { 1 })
        {
            let first = ts::demux(segment).unwrap().video[0].dts as i64;
            let mut t = Transmuxer::default();
            t.push(segment).unwrap();
            for (n, start) in [
                first + 9_000_000,
                first + 900_000,
                90_000,
                first + 27_000_000,
            ]
            .into_iter()
            .enumerate()
            {
                let (vend, aend) = (t.vend, t.audio_next.unwrap());
                let moved = shifted(segment, start - first);
                let starts = base_times(&t.push(&moved).unwrap().fragment());
                assert_eq!(starts, [vend, aend], "{name}, jump {n} (to {start})");
            }
        }
    }

    #[test]
    fn configuration_changes_and_audio_appearing_emit_new_init() {
        let clip = include_bytes!("../tests/fixtures/bbb_480p.ts");
        let mut silent = ts::demux(clip).unwrap();
        silent.audio.clear();
        silent.aac = None;
        silent.video_data.splice(0..0, [0; MDAT_HEADER]);
        let mut t = Transmuxer::default();
        let first = t.assemble(silent).unwrap();
        assert!(!first.init.unwrap().mime.contains("mp4a"));
        let with_audio = t.push(clip).unwrap();
        assert!(with_audio.init.unwrap().mime.contains("mp4a"));
        assert!(t.push(clip).unwrap().init.is_none());
        let changed = t
            .push(include_bytes!("../tests/fixtures/pal_anamorphic.ts"))
            .unwrap();
        assert!(changed.init.is_some());
    }

    #[test]
    fn radio_produces_an_audio_only_init_and_fragments() {
        let mut d = ts::demux(include_bytes!("../tests/fixtures/bbb_480p.ts")).unwrap();
        d.video.clear();
        d.video_data.clear();
        d.sps = None;
        d.pps = None;
        let out = Transmuxer::default().assemble(d).unwrap();
        assert!(out.init.unwrap().mime.starts_with("audio/mp4"));
        assert_eq!(out.fragments.len(), 1);
    }

    #[test]
    fn discontinuity_skew_measurement() {
        let clip = include_bytes!("../tests/fixtures/bbb_480p.ts");
        let mut t = Transmuxer::default();
        t.push(clip).unwrap();
        let initial = t.vend as i64 - t.aend as i64;
        let mut maximum = 0i64;
        for i in 0..20 {
            let mut d = ts::demux(&shifted(clip, (i + 1) * 9_000_000)).unwrap();
            for audio in &mut d.audio {
                audio.pts = (audio.pts as i64 + if i % 2 == 0 { 9000 } else { -9000 }) as u64;
            }
            d.video_data.splice(0..0, [0; MDAT_HEADER]);
            t.assemble(d).unwrap();
            maximum = maximum.max((t.vend as i64 - t.aend as i64).abs());
        }
        assert!(
            maximum < 18_000,
            "repeated splice skew exceeded 200 ms: {maximum}"
        );
        eprintln!(
            "splice skew initial={} ms max={} ms",
            initial as f64 / 90.0,
            maximum as f64 / 90.0
        );
    }

    #[test]
    fn thousands_of_restarts_stay_monotonic_with_bounded_output() {
        let clip = include_bytes!("../tests/fixtures/bbb_480p.ts");
        let mut t = Transmuxer::default();
        let mut previous = [0, 0];
        let mut max_bytes = 0;
        for i in 0..2000 {
            let out = t.push(clip).unwrap();
            let bases = base_times(&out.fragment());
            assert!(bases.iter().zip(previous).all(|(at, prev)| *at >= prev));
            previous.copy_from_slice(&bases);
            let size: usize = out
                .fragments
                .iter()
                .map(|f| f.moof.len() + f.mdat.len())
                .sum();
            max_bytes = max_bytes.max(size);
            assert!(size < clip.len() * 2);
            assert_eq!(out.init.is_some(), i == 0);
        }
        assert!(max_bytes > 0);
    }

    /// A segment that arrives in pieces comes out exactly as the same segment all at once.
    #[test]
    fn a_segment_fed_in_pieces_is_the_same_as_one_pushed_whole() {
        let clip: &[u8] = include_bytes!("../tests/fixtures/bbb_480p.ts");
        let whole = Transmuxer::default().push(clip).unwrap();
        let mut t = Transmuxer::default();
        let mut segment = Segment::new(clip.len());
        clip.chunks(1234).for_each(|c| segment.feed(c));
        let pieces = t.finish(segment).unwrap();
        assert!(!whole.fragments.is_empty());
        assert_eq!(pieces.fragments.len(), whole.fragments.len());
        for (p, w) in pieces.fragments.iter().zip(&whole.fragments) {
            assert_eq!((&p.moof, &p.mdat), (&w.moof, &w.mdat));
            // The mdat is one box, sized to what it holds.
            let size = u32::from_be_bytes(p.mdat[..4].try_into().unwrap()) as usize;
            assert_eq!((size, &p.mdat[4..8]), (p.mdat.len(), &b"mdat"[..]));
        }
        assert_eq!(pieces.init.unwrap().bytes, whole.init.unwrap().bytes);
    }

    /// A download that fails part way is dropped, and the transmuxer has not noticed: the next
    /// segment is still the first one, with its init segment and its place at zero.
    #[test]
    fn a_segment_dropped_unfinished_leaves_no_trace() {
        let clip: &[u8] = include_bytes!("../tests/fixtures/bbb_480p.ts");
        let mut t = Transmuxer::default();
        let mut half = Segment::new(clip.len());
        half.feed(&clip[..clip.len() / 2]);
        drop(half);
        let out = t.push(clip).unwrap();
        assert!(out.init.is_some());
        assert_eq!(base_times(&out.fragment())[0], 0);
    }

    #[test]
    fn what_a_stream_lacks_is_said_in_types_not_strings() {
        // HEVC: the demuxer names the codec, and that maps to `Unsupported::Video`.
        let hevc = Transmuxer::default()
            .push(include_bytes!("../tests/fixtures/hevc_ac3.ts"))
            .unwrap_err();
        assert_eq!(
            hevc.unsupported(),
            Some(Unsupported::Video(Some("HEVC".into())))
        );
        // A stream that is broken is an error, not "unsupported".
        let broken = Transmuxer::default()
            .push(b"definitely not transport stream data")
            .unwrap_err();
        assert_eq!(broken.unsupported(), None);
        assert_eq!(Error::Playlist("x".into()).unsupported(), None);
        // Every reason reads as a sentence for the viewer.
        assert_eq!(
            Unsupported::Sound("AC-3".into()).to_string(),
            "AC-3 sound can't be played by this browser"
        );
        assert_eq!(
            Unsupported::Video(Some("HEVC".into())).to_string(),
            "the video isn't H.264 (it is HEVC)"
        );
        assert!(
            Unsupported::Video(None)
                .to_string()
                .contains("no supported audio or video")
        );
        assert_eq!(Unsupported::RawStream.to_string(), hls::RAW_STREAM);
        let refused = Unsupported::MediaType {
            mime: "video/mp4".into(),
            why: "NotSupportedError".into(),
        };
        assert_eq!(
            refused.to_string(),
            "this browser can't play video/mp4: NotSupportedError"
        );
    }
}
