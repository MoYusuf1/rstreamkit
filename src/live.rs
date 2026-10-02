//! Platform-neutral live scheduling and memory policy, shared by the browser and tests.
use crate::hls::{Media, Segment};

/// Approximate compressed media budget; browsers account buffers differently.
pub const BUFFER_BUDGET: usize = 32 << 20;

#[derive(Default)]
pub(crate) struct Window {
    last: Option<(u64, String, Option<std::ops::Range<u64>>)>,
    stalled_since: Option<f64>,
}

impl Window {
    pub fn next<'a>(&self, media: &'a Media) -> Option<&'a Segment> {
        let start = match &self.last {
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
        };
        media.segments.get(start)
    }

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

/// Cross a nearby gap only after the current range is exhausted, never over playable media.
pub(crate) fn gap_target(ranges: &[(f64, f64)], time: f64, limit: f64) -> Option<f64> {
    for &(start, end) in ranges {
        if time >= start && time < end - 0.15 {
            return None;
        }
        if start > time && start - time <= limit {
            return Some(start);
        }
    }
    None
}

/// Duration limits derived from bytes actually appended, including decoded sound.
pub(crate) fn buffer_limits(bytes_per_second: f64, segment_seconds: f64) -> (f64, f64) {
    let total = if bytes_per_second > 0.0 {
        BUFFER_BUDGET as f64 / bytes_per_second
    } else {
        40.0
    };
    let floor = (2.0 * segment_seconds.max(1.0)).min(30.0);
    let behind = (total * 0.2).clamp(0.0, 10.0);
    // Reserve one segment: the append completing a throttle can overshoot its threshold.
    (
        (total - behind - segment_seconds.max(1.0)).clamp(floor, 30.0),
        behind,
    )
}

#[derive(Default)]
pub(crate) struct Abr {
    throughput: f64,
    headroom: u32,
}
impl Abr {
    pub fn observe(&mut self, bytes: usize, seconds: f64) {
        if seconds > 0.0 && seconds.is_finite() {
            let sample = bytes as f64 * 8.0 / seconds;
            self.throughput = if self.throughput == 0.0 {
                sample
            } else {
                self.throughput * 0.7 + sample * 0.3
            };
        }
    }
    pub fn choose<'a>(
        &mut self,
        variants: &'a [crate::hls::Variant],
        current: &str,
        buffered: f64,
        target: f64,
    ) -> Option<&'a crate::hls::Variant> {
        let candidate = crate::hls::pick_variant_for(variants, (self.throughput * 0.75) as u64)?;
        let now = variants.iter().find(|v| v.uri == current)?;
        if candidate.bandwidth < now.bandwidth {
            self.headroom = 0;
            return Some(candidate);
        }
        if candidate.bandwidth > now.bandwidth && buffered >= target * 2.0 {
            self.headroom += 1;
            if self.headroom >= 3 {
                self.headroom = 0;
                return Some(candidate);
            }
        } else {
            self.headroom = 0;
        }
        None
    }
}

/// Bounded retries tolerate a ten-second outage without an unbounded retry loop.
pub(crate) fn retry_delay(attempt: u32) -> Option<u64> {
    match attempt {
        0 => Some(0),
        1 => Some(1),
        2 => Some(2),
        3 => Some(4),
        4..=6 => Some(8),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hls::{self, Parsed};
    #[test]
    fn recovered_live_gaps_do_not_block_a_full_buffer_or_skip_playable_media() {
        let ranges = [(0.0, 18.026), (20.0, 48.021)];
        assert_eq!(gap_target(&ranges, 17.944, 4.0), Some(20.0));
        assert_eq!(gap_target(&ranges, 12.0, 10.0), None);
        assert_eq!(gap_target(&ranges, 17.944, 0.5), None);
        assert_eq!(gap_target(&ranges, 48.021, 4.0), None);
        let mut window = Window::default();
        window.consumed(&playlist(100, &["old"]).segments[0]);
        assert!(window.discontinuous(&playlist(103, &["new"]).segments[0]));
        assert!(window.discontinuous(&playlist(0, &["restart"]).segments[0]));
        assert!(!window.discontinuous(&playlist(101, &["next"]).segments[0]));
    }
    fn playlist(seq: u64, names: &[&str]) -> Media {
        let text = format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:{seq}\n{}",
            names
                .iter()
                .map(|n| format!("#EXTINF:2,\n{n}\n"))
                .collect::<String>()
        );
        let Parsed::Media(m) = hls::parse(text.as_bytes()).unwrap() else {
            panic!()
        };
        m
    }
    #[test]
    fn repeated_uri_byte_ranges_are_distinct_segments() {
        let media=match crate::hls::parse(b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\n#EXT-X-BYTERANGE:100@20\nsame.mp4\n#EXTINF:2,\n#EXT-X-BYTERANGE:100\nsame.mp4\n#EXT-X-ENDLIST\n").unwrap() {crate::hls::Parsed::Media(m)=>m,_=>unreachable!()};
        let mut window = Window::default();
        let first = window.next(&media).unwrap();
        assert_eq!(first.seq, 0);
        window.consumed(first);
        let second = window.next(&media).unwrap();
        assert_eq!(second.seq, 1);
        window.consumed(second);
        assert!(window.next(&media).is_none());
    }
    #[test]
    fn a_repeated_uri_does_not_skip_the_segments_between_its_occurrences() {
        let media = playlist(0, &["same", "changed", "same"]);
        let mut window = Window::default();
        for segment in &media.segments {
            assert_eq!(window.next(&media).unwrap().seq, segment.seq);
            window.consumed(segment);
        }
        assert!(window.next(&media).is_none());
    }
    #[test]
    fn a_long_pause_with_broken_sequence_restarts_from_the_current_window() {
        let old = playlist(0, &["a", "b", "c", "d", "e", "f"]);
        let new = playlist(0, &["g", "h", "i", "j", "k", "l"]);
        let mut window = Window::default();
        window.consumed(old.segments.last().unwrap());
        assert_eq!(window.next(&new).unwrap().uri, "j");
        assert!(window.discontinuous(window.next(&new).unwrap()));
    }
    #[test]
    fn sliding_windows_with_and_without_advancing_sequence() {
        for seq in [0, 1] {
            let mut w = Window::default();
            let old = playlist(0, &["a", "b", "c"]);
            for s in &old.segments {
                w.consumed(s);
            }
            let new = playlist(seq, &["b", "c", "d"]);
            assert_eq!(w.next(&new).unwrap().uri, "d");
            w.consumed(w.next(&new).unwrap());
            assert!(w.next(&new).is_none());
        }
    }
    #[test]
    fn long_pause_and_numbering_restart_use_current_window() {
        let mut w = Window::default();
        w.consumed(&playlist(100, &["old"]).segments[0]);
        assert_eq!(
            w.next(&playlist(200, &["new1", "new2"])).unwrap().uri,
            "new1"
        );
        assert_eq!(
            w.next(&playlist(0, &["reset1", "reset2", "reset3", "reset4"]))
                .unwrap()
                .uri,
            "reset2"
        );
    }
    #[test]
    fn failed_segment_is_skipped_and_freeze_is_bounded() {
        let mut w = Window::default();
        let m = playlist(0, &["404", "garbage", "good"]);
        for name in ["404", "garbage", "good"] {
            let s = w.next(&m).unwrap();
            assert_eq!(s.uri, name);
            w.consumed(s);
        }
        assert!(!w.frozen(10.0, 2.0));
        assert!(w.frozen(16.0, 2.0));
        let recovery = playlist(3, &["recovered"]);
        w.consumed(w.next(&recovery).unwrap());
        assert!(!w.frozen(20.0, 2.0));
    }
    #[test]
    fn adaptive_selection_steps_down_then_recovers_with_headroom() {
        let variants = vec![
            crate::hls::Variant {
                uri: "low".into(),
                bandwidth: 200_000,
                ..Default::default()
            },
            crate::hls::Variant {
                uri: "high".into(),
                bandwidth: 2_000_000,
                ..Default::default()
            },
        ];
        let mut abr = Abr::default();
        abr.observe(100_000, 2.0);
        assert_eq!(abr.choose(&variants, "high", 0.0, 2.0).unwrap().uri, "low");
        for _ in 0..8 {
            abr.observe(2_000_000, 1.0);
        }
        assert!(abr.choose(&variants, "low", 5.0, 2.0).is_none());
        assert!(abr.choose(&variants, "low", 5.0, 2.0).is_none());
        assert_eq!(abr.choose(&variants, "low", 5.0, 2.0).unwrap().uri, "high");
    }

    #[test]
    fn bitrate_limits_and_retry_budget() {
        let (ahead, behind) = buffer_limits(2_000_000.0, 6.0);
        assert!((12.0..30.0).contains(&ahead));
        assert!((ahead + behind) * 2_000_000.0 <= BUFFER_BUDGET as f64);
        assert_eq!(retry_delay(7), None);
        assert!((0..7).map(|a| retry_delay(a).unwrap()).sum::<u64>() > 10);
    }
}
