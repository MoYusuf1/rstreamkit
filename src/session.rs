//! Shared HLS selection, playlist refresh and playback backpressure.
//! Hosts own transport, URL resolution and media output; dropping pending work cancels it.
use crate::{
    hls::{Media, Segment},
    net::{BufferWait, Clock, PlaybackBuffer, wait_for_room},
};
use std::{cell::Cell, time::Duration};

/// Playlist I/O. The host applies body limits, resolves redirects and classifies errors.
/// Pending refresh work must release its resources when dropped.
#[allow(async_fn_in_trait)]
pub trait PlaylistSource {
    type Error: From<String>;
    async fn refresh(&mut self) -> Result<Media, Self::Error>;
    /// Called before a periodic refresh; `stalled` means no progress for three target durations.
    fn waiting(&mut self, _stalled: bool) {}
}

/// Shared stop/go-live flags. Stop takes priority; pending I/O is cancelled by dropping its future.
#[derive(Clone, Copy)]
pub struct Controls<'a> {
    stop: &'a Cell<bool>,
    go_live: &'a Cell<bool>,
}
impl<'a> Controls<'a> {
    pub fn new(stop: &'a Cell<bool>, go_live: &'a Cell<bool>) -> Self {
        Self { stop, go_live }
    }
}

/// Whether the host should continue, replace speculative input or move to the live edge.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum Preparation {
    /// Discard speculative input after a refresh; `jump` requests a new live-edge anchor.
    Ready {
        refreshed: bool,
        jump: bool,
    },
    Cancelled,
}

impl Session {
    /// Refresh after a long pause/backpressure or go-live, then select from current media.
    /// Finite input keeps its place after backpressure. No segment bytes are cloned.
    pub async fn prepare<P: PlaylistSource>(
        &mut self,
        clock: &impl Clock,
        buffer: &mut impl PlaybackBuffer,
        source: &mut P,
        media: &mut Media,
        bytes_per_second: f64,
        controls: Controls<'_>,
    ) -> Result<Preparation, P::Error> {
        if controls.stop.get() {
            return Ok(Preparation::Cancelled);
        }
        let mut refreshed = false;
        let mut jump = controls.go_live.replace(false);
        if jump {
            *media = source.refresh().await?;
            *self = Self::default();
            refreshed = true;
        }
        let waited = wait_for_room(
            clock,
            buffer,
            buffer_limits(bytes_per_second, media.target_duration),
            media.target_duration,
            || controls.stop.get(),
            || controls.go_live.get(),
        )
        .await?;
        if waited == BufferWait::Cancelled {
            return Ok(Preparation::Cancelled);
        }
        if !media.ended && waited != (BufferWait::Ready { waited: false }) {
            *media = source.refresh().await?;
            refreshed = true;
            if controls.go_live.replace(false) {
                *self = Self::default();
                jump = true;
            }
        }
        if controls.stop.get() {
            return Ok(Preparation::Cancelled);
        }
        Ok(Preparation::Ready { refreshed, jump })
    }

    /// Wait about half a segment and refresh live media. `now` supplies host time in seconds.
    /// A go-live request during this wait uses this refresh instead of fetching twice.
    pub async fn wait_and_refresh<P: PlaylistSource>(
        &mut self,
        clock: &impl Clock,
        source: &mut P,
        media: &mut Media,
        controls: Controls<'_>,
        now: impl Fn() -> f64,
    ) -> Result<Preparation, P::Error> {
        if controls.stop.get() {
            return Ok(Preparation::Cancelled);
        }
        if media.ended {
            return Ok(Preparation::Ready {
                refreshed: false,
                jump: false,
            });
        }
        if !media.target_duration.is_finite() || media.target_duration <= 0.0 {
            return Err("invalid playlist target duration".to_owned().into());
        }
        clock
            .sleep(Duration::from_secs_f64(
                (media.target_duration / 2.0).clamp(1.0, 5.0),
            ))
            .await;
        if controls.stop.get() {
            return Ok(Preparation::Cancelled);
        }
        let stalled = self.next(media).is_none() && self.frozen(now(), media.target_duration);
        source.waiting(stalled);
        if controls.stop.get() {
            return Ok(Preparation::Cancelled);
        }
        *media = source.refresh().await?;
        if controls.stop.get() {
            return Ok(Preparation::Cancelled);
        }
        let jump = controls.go_live.replace(false);
        if jump {
            *self = Self::default();
        }
        Ok(Preparation::Ready {
            refreshed: true,
            jump,
        })
    }
}

/// Approximate compressed media budget; browsers account buffers differently.
pub const BUFFER_BUDGET: usize = 32 << 20;

/// Segment cursor and stall history, shared by native hosts and browser playback.
/// Media bytes and playlist ownership stay with the host.
#[derive(Default)]
pub struct Session {
    last: Option<(u64, String, Option<std::ops::Range<u64>>)>,
    stalled_since: Option<f64>,
}

/// A borrowed segment and constant-time metadata from a prepared playlist walk.
#[non_exhaustive]
pub struct ScheduledSegment<'a> {
    pub segment: &'a Segment,
    pub next: Option<&'a Segment>,
    pub remaining_seconds: f64,
    pub window_seconds: f64,
}

/// One immutable snapshot walk. The duration cache costs eight bytes per pending
/// segment plus a sentinel. It is freed with the batch; it holds no media copies.
pub struct Batch<'a> {
    media: &'a Media,
    index: usize,
    start: usize,
    suffix: Vec<f64>,
    window_seconds: f64,
}
impl<'a> Iterator for Batch<'a> {
    type Item = ScheduledSegment<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let segment = self.media.segments.get(self.index)?;
        self.index += 1;
        Some(ScheduledSegment {
            segment,
            next: self.media.segments.get(self.index),
            remaining_seconds: self.suffix[self.index - self.start],
            window_seconds: self.window_seconds,
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.media.segments.len() - self.index;
        (remaining, Some(remaining))
    }
}
impl ExactSizeIterator for Batch<'_> {}
impl std::iter::FusedIterator for Batch<'_> {}

impl Session {
    pub fn finished(&self, media: &Media) -> bool {
        media.ended && self.next(media).is_none()
    }

    pub fn next<'a>(&self, media: &'a Media) -> Option<&'a Segment> {
        media.segments.get(self.next_index(media))
    }

    fn next_index(&self, media: &Media) -> usize {
        match &self.last {
            None => {
                if media.ended {
                    0
                } else {
                    media.segments.len().saturating_sub(3)
                }
            }
            Some((seq, uri, range)) => {
                // URI anchors survive broken providers that slide without advancing sequence.
                if let Some(at) = media
                    .segments
                    .iter()
                    .position(|s| s.seq == *seq && s.uri == *uri && s.byte_range == *range)
                {
                    at + 1
                } else if let Some(at) = media
                    .segments
                    .iter()
                    .rposition(|s| s.uri == *uri && s.byte_range == *range)
                {
                    if media.segments[at].seq == *seq
                        || media.segments.iter().any(|s| s.seq <= *seq)
                    {
                        at + 1
                    } else {
                        0
                    }
                } else if media.segments.first().is_some_and(|s| s.seq > *seq) {
                    0
                } else if media.segments.last().is_some_and(|s| s.seq > *seq) {
                    media
                        .segments
                        .iter()
                        .position(|s| s.seq > *seq)
                        .unwrap_or(media.segments.len())
                } else {
                    media.segments.len().saturating_sub(3)
                }
            }
        }
    }

    /// Prepare one immutable playlist walk. Keep it across finite-media buffer waits;
    /// rebuild after a refresh or selection/control change. No segment bytes are copied.
    pub fn batch<'a>(&self, media: &'a Media) -> Batch<'a> {
        let start = self.next_index(media).min(media.segments.len());
        let pending = &media.segments[start..];
        let mut suffix = vec![0.0; pending.len() + 1];
        for (index, segment) in pending.iter().enumerate().rev() {
            suffix[index] = segment.duration + suffix[index + 1];
        }
        Batch {
            media,
            index: start,
            start,
            suffix,
            window_seconds: media.duration(),
        }
    }

    /// Advance only after successful output, or an explicit host decision to skip a failed segment.
    pub fn consumed(&mut self, segment: &Segment) {
        self.last = Some((segment.seq, segment.uri.clone(), segment.byte_range.clone()));
        self.stalled_since = None;
    }

    pub fn discontinuous(&self, segment: &Segment) -> bool {
        self.last
            .as_ref()
            .is_some_and(|(seq, _, _)| segment.seq < *seq || segment.seq > seq.saturating_add(1))
    }

    pub fn frozen(&mut self, now: f64, target: f64) -> bool {
        now - *self.stalled_since.get_or_insert(now) >= 3.0 * target.max(1.0)
    }
}

/// Duration limits derived from bytes actually appended, including decoded sound.
/// Reserve the next segment before assigning rewind/ahead space. Keep at least
/// one segment behind: removing part of the current GOP can also remove its future
/// dependent frames. Long GOPs can exceed the budget; this is not a process-memory cap.
pub fn buffer_limits(bytes_per_second: f64, segment_seconds: f64) -> (f64, f64) {
    let total = if bytes_per_second > 0.0 && bytes_per_second.is_finite() {
        BUFFER_BUDGET as f64 / bytes_per_second
    } else {
        40.0
    };
    let segment = if segment_seconds.is_finite() {
        segment_seconds.max(1.0)
    } else {
        1.0
    };
    let available = (total - segment).max(0.0);
    let behind = (available * 0.2).min(10.0).max(segment);
    // Keep a little time for fetch, append and decoder startup at a segment boundary.
    // A positive threshold also avoids waiting forever on the final frame of a range.
    ((available - behind).clamp(0.5, 30.0), behind)
}
