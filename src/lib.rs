//! rffmpeg: the part of ffmpeg a live-TV player needs, in pure Rust. HLS playlists, MPEG-TS
//! demuxing, fMP4 muxing and AC-3, E-AC-3 and MP2 sound decoding (pure, testable natively), plus the
//! MediaSource glue that feeds a `<video>` element (wasm only, `mse`).

pub mod avc;
pub mod fmp4;
pub mod hls;
#[cfg(target_arch = "wasm32")]
pub mod mse;
pub mod sound;
pub mod ts;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Ts(#[from] ts::Error),
    #[error("bad playlist: {0}")]
    Playlist(String),
    #[error("{0}")]
    Unsupported(String),
}

/// Marks a playback error as "the browser can't play this, but something else could": HEVC video,
/// sound it can't decode, a raw stream. The page then asks the proxy to convert the stream.
pub const CONVERT: &str = "convert:";

/// Why this stream needs converting, if the error says it does.
pub fn needs_conversion(err: &str) -> Option<&str> {
    if let Some(reason) = err.strip_prefix(CONVERT) {
        Some(reason)
    } else if err.contains("no H.264 video") {
        Some("the video isn't H.264 (probably HEVC)")
    } else {
        None
    }
}

/// What to give `MediaSource.addSourceBuffer` and then append first.
pub struct Init {
    pub bytes: Vec<u8>,
    pub mime: String,
    /// Interlaced pictures play combed, at half the motion rate, in a browser.
    pub interlaced: bool,
}

pub struct Output {
    /// Present for the first segment only.
    pub init: Option<Init>,
    /// moof+mdat to append; empty if the segment held no samples.
    pub fragment: Vec<u8>,
    pub skipped_audio: Option<String>,
}

const WRAP: u64 = 1 << 33; // PES timestamps are 33 bits
/// A segment starting further than this (90 kHz ticks, 2 s) from where the last one ended is a
/// new timeline (stream restart, ad splice, missing tag) and gets glued on instead of leaving a gap.
const JUMP: u64 = 2 * 90_000;

/// What a stream's audio track is, settled by its first segment.
#[derive(Default, Clone, Copy, PartialEq)]
enum Audio {
    #[default]
    None,
    /// AAC, passed through as it is.
    Aac,
    /// Sound we decode ourselves (AC-3, E-AC-3, MP2) and write as FLAC: its kind and sample rate.
    Sound(sound::Kind, u32),
}

/// Turns consecutive HLS TS segments into a continuous fMP4 stream.
#[derive(Default)]
pub struct Transmuxer {
    base: Option<u64>,
    /// Last (unwrapped) timestamp seen, used to resolve the 33-bit rollover.
    last: u64,
    /// Where the previous fragment ended, relative to `base`, in 90 kHz ticks.
    end: u64,
    seq: u32,
    sent_init: bool,
    audio: Audio,
    audio_next: Option<u64>,
    decoder: sound::Decoder,
    /// FLAC frames written so far; each frame carries its number.
    flac_frames: u32,
}

impl Transmuxer {
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

    pub fn push(&mut self, segment: &[u8]) -> Result<Output, Error> {
        let d = ts::demux(segment)?;

        let init = if self.sent_init {
            None
        } else {
            let sps = d
                .sps
                .as_deref()
                .filter(|s| s.len() >= 4)
                .ok_or(ts::Error::NoVideoParams)?;
            let pps = d.pps.as_deref().ok_or(ts::Error::NoVideoParams)?;
            let (width, height) = avc::dimensions(sps)
                .ok_or_else(|| ts::Error::BadSps("cannot read the picture size".into()))?;
            self.audio = match (&d.aac, d.sound_kind, d.sound.first()) {
                (Some(_), _, _) => Audio::Aac,
                (None, Some(kind), Some(first)) => Audio::Sound(kind, first.rate),
                _ => Audio::None,
            };
            let (track, audio) = match self.audio {
                // Browsers only accept `mp4a.40.2` (AAC-LC) here, and decode Main and the rest of
                // the family fine when told so; declaring the real type (Main is `.1`) is refused.
                Audio::Aac => (d.aac.as_ref().map(fmp4::AudioTrack::Aac), ",mp4a.40.2"),
                Audio::Sound(_, rate) => (Some(fmp4::AudioTrack::Flac { rate }), ",flac"),
                Audio::None => (None, ""),
            };
            self.sent_init = true;
            Some(Init {
                bytes: fmp4::init_segment(
                    &fmp4::VideoParams {
                        sps,
                        pps,
                        width,
                        height,
                        pixel_aspect: avc::pixel_aspect(sps).unwrap_or((1, 1)),
                    },
                    track.as_ref(),
                ),
                interlaced: avc::interlaced(sps).unwrap_or(false),
                mime: format!(
                    "video/mp4; codecs=\"avc1.{:02x}{:02x}{:02x}{audio}\"",
                    sps[1], sps[2], sps[3]
                ),
            })
        };

        let video: Vec<(u64, u64)> = d
            .video
            .iter()
            .map(|s| (self.unwrap(s.dts), self.unwrap(s.pts)))
            .collect();
        let audio: Vec<u64> = d.audio.iter().map(|s| self.unwrap(s.pts)).collect();
        let sound_pts: Vec<u64> = d.sound.iter().map(|s| self.unwrap(s.pts)).collect();
        let first = video
            .first()
            .map(|v| v.0)
            .into_iter()
            .chain(audio.first().copied())
            .chain(sound_pts.first().copied())
            .min();
        let Some(first) = first else {
            return Ok(Output {
                init,
                fragment: vec![],
                skipped_audio: d.skipped_audio,
            });
        };

        // Timelines start at zero, and a jump in the source timeline is re-based so it plays on.
        let base = match self.base {
            Some(b) if first.abs_diff(b + self.end) <= JUMP => b,
            _ => {
                self.audio_next = None;
                first.saturating_sub(self.end)
            }
        };
        self.base = Some(base);

        let mut runs = vec![];
        let mut end = 0;
        if !video.is_empty() {
            let mut last_dur = 3003; // ~29.97 fps, only used if the segment has a single frame
            let samples: Vec<_> = d
                .video
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let dur = video
                        .get(i + 1)
                        .map_or(last_dur, |n| n.0.saturating_sub(video[i].0) as u32);
                    last_dur = dur;
                    fmp4::Sample {
                        duration: dur,
                        key: s.key,
                        cts: (video[i].1 as i64 - video[i].0 as i64) as i32,
                        data: &s.data,
                    }
                })
                .collect();
            let start = video[0].0.saturating_sub(base);
            end = end.max(start + samples.iter().map(|s| s.duration as u64).sum::<u64>());
            runs.push(fmp4::TrackRun {
                track: fmp4::VIDEO_TRACK,
                base_time: start,
                samples,
            });
        }
        if let (Some(cfg), Audio::Aac, false) = (d.aac, self.audio, audio.is_empty()) {
            let rate = cfg.sample_rate() as u64;
            let derived = audio[0].saturating_sub(base) * rate / 90_000;
            // Keep audio gapless: 90 kHz -> sample-rate rounding must not open 1-tick holes between fragments.
            let start = match self.audio_next {
                Some(next) if derived.abs_diff(next) <= 1024 => next,
                _ => derived,
            };
            let stop = start + 1024 * audio.len() as u64;
            self.audio_next = Some(stop);
            end = end.max(stop * 90_000 / rate);
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
        // Sound we decode: each frame becomes PCM, then one FLAC frame, one sample in the track.
        let mut flac = vec![];
        if let (Audio::Sound(kind, rate), false) = (self.audio, d.sound.is_empty()) {
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
            let rate = u64::from(rate);
            let derived = sound_pts[0].saturating_sub(base) * rate / 90_000;
            // Gapless, as for AAC: rounding 90 kHz ticks to samples must not open holes.
            let tolerance = u64::from(flac[0].1);
            let start = match self.audio_next {
                Some(next) if derived.abs_diff(next) <= tolerance => next,
                _ => derived,
            };
            let stop = start + flac.iter().map(|f| u64::from(f.1)).sum::<u64>();
            self.audio_next = Some(stop);
            end = end.max(stop * 90_000 / rate);
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
        self.end = end;

        self.seq += 1;
        Ok(Output {
            init,
            fragment: fmp4::fragment(self.seq, &runs),
            skipped_audio: d.skipped_audio,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn errors_that_a_converter_could_fix_are_recognised() {
        let hevc = ts::demux(include_bytes!("../tests/fixtures/hevc_ac3.ts")).unwrap_err();
        assert!(needs_conversion(&hevc.to_string()).is_some(), "{hevc}");
        assert_eq!(
            needs_conversion(&format!("{CONVERT}AC-3 sound")),
            Some("AC-3 sound")
        );
        assert_eq!(needs_conversion("bad playlist: nothing here"), None);
        assert_eq!(
            needs_conversion("the proxy refused x: address not allowed"),
            None
        );
    }
}
