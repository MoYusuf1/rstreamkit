use rstreamkit::{
    hls::{self, Media, Parsed},
    session::Session,
};

fn playlist(count: usize, ended: bool) -> Media {
    let text = format!(
        "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:100\n{}{}",
        (0..count)
            .map(|i| format!("#EXTINF:6,\nsegment{i}.ts\n"))
            .collect::<String>(),
        if ended { "#EXT-X-ENDLIST\n" } else { "" }
    );
    let Parsed::Media(media) = hls::parse(text.as_bytes()).unwrap() else {
        panic!()
    };
    media
}

#[test]
fn prepared_walk_matches_selection_next_segment_and_variable_durations() {
    let mut media = playlist(57, true);
    for (i, segment) in media.segments.iter_mut().enumerate() {
        segment.duration = 0.125 + (i % 7) as f64 / 10.0;
    }
    let mut session = Session::default();
    for (index, scheduled) in session.batch(&media).enumerate() {
        assert_eq!(Some(scheduled.segment), session.next(&media));
        assert_eq!(scheduled.next, media.segments.get(index + 1));
        let expected: f64 = media.segments[index + 1..].iter().map(|s| s.duration).sum();
        assert!((scheduled.remaining_seconds - expected).abs() < 1e-10);
        assert_eq!(scheduled.window_seconds, media.duration());
        session.consumed(scheduled.segment);
    }
    assert!(session.finished(&media));
    assert_eq!(session.batch(&media).count(), 0);
}

#[test]
fn empty_live_tail_and_resumed_finite_input_keep_their_place() {
    assert_eq!(Session::default().batch(&playlist(0, true)).len(), 0);
    let live = playlist(8, false);
    assert_eq!(
        Session::default()
            .batch(&live)
            .map(|s| s.segment.seq)
            .collect::<Vec<_>>(),
        [105, 106, 107]
    );
    let finite = playlist(8, true);
    let mut session = Session::default();
    session.consumed(&finite.segments[3]);
    let mut batch = session.batch(&finite);
    assert_eq!(batch.len(), 4);
    assert_eq!(batch.next().unwrap().segment.seq, 104);
    assert_eq!(batch.len(), 3);
    assert_eq!(batch.last().unwrap().remaining_seconds, 0.0);
}

#[test]
fn refreshed_broken_sequences_and_byte_ranges_keep_existing_anchor_policy() {
    let mut old = playlist(4, false);
    for (i, segment) in old.segments.iter_mut().enumerate() {
        segment.uri = "same.mp4".into();
        segment.byte_range = Some(i as u64 * 10..(i as u64 + 1) * 10);
    }
    let mut session = Session::default();
    session.consumed(&old.segments[1]);
    assert_eq!(
        session.batch(&old).next().unwrap().segment.byte_range,
        Some(20..30)
    );
    let mut slid = old.clone();
    slid.segments.remove(0);
    for (i, segment) in slid.segments.iter_mut().enumerate() {
        segment.seq = 100 + i as u64;
    }
    assert_eq!(
        session.batch(&slid).next().unwrap().segment,
        session.next(&slid).unwrap()
    );
    let restarted = playlist(7, false);
    assert_eq!(
        session.batch(&restarted).next().unwrap().segment,
        session.next(&restarted).unwrap()
    );
}

#[test]
fn suffix_cache_does_not_lose_small_tails_after_huge_or_overflowing_sums() {
    let mut media = playlist(3, true);
    media.segments[0].duration = 1e308;
    media.segments[1].duration = 1e308;
    media.segments[2].duration = 0.25;
    let values: Vec<_> = Session::default()
        .batch(&media)
        .map(|s| s.remaining_seconds)
        .collect();
    assert_eq!(values, [1e308, 0.25, 0.0]);
    assert!(media.duration().is_infinite());
}

#[test]
#[ignore = "manual native playlist bookkeeping benchmark; run in release mode"]
fn measure_playlist_bookkeeping() {
    use std::{hint::black_box, time::Instant};
    fn old(media: &Media) {
        let mut cursor = Session::default();
        while let Some(segment) = cursor.next(media) {
            let next = media
                .segments
                .iter()
                .position(|s| {
                    s.seq == segment.seq
                        && s.uri == segment.uri
                        && s.byte_range == segment.byte_range
                })
                .and_then(|i| media.segments.get(i + 1));
            let remaining: f64 = media
                .segments
                .iter()
                .skip_while(|s| {
                    s.seq != segment.seq
                        || s.uri != segment.uri
                        || s.byte_range != segment.byte_range
                })
                .skip(1)
                .map(|s| s.duration)
                .sum();
            black_box((
                next.map(|s| s.seq),
                remaining,
                if media.ended { 0.0 } else { media.duration() },
                if media.ended { 0.0 } else { media.duration() },
            ));
            cursor.consumed(segment);
        }
    }
    fn prepared(media: &Media) {
        let mut cursor = Session::default();
        for item in cursor.batch(media) {
            black_box((
                item.next.map(|s| s.seq),
                item.remaining_seconds,
                if media.ended {
                    0.0
                } else {
                    item.window_seconds
                },
                if media.ended {
                    0.0
                } else {
                    item.window_seconds
                },
            ));
            cursor.consumed(item.segment);
        }
    }
    let counts = std::env::var("RSTREAMKIT_BENCH_SEGMENTS")
        .map(|count| vec![count.parse::<usize>().expect("segment count")])
        .unwrap_or_else(|_| vec![128, 2048, 20_000]);
    for count in counts {
        let media = playlist(count, true);
        old(&media);
        prepared(&media);
        let mut before = Vec::new();
        let mut after = Vec::new();
        for round in 0..9 {
            let mut measure = |is_old| {
                let start = Instant::now();
                if is_old {
                    old(black_box(&media));
                } else {
                    prepared(black_box(&media));
                }
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                if is_old {
                    before.push(ms);
                } else {
                    after.push(ms);
                }
            };
            if round % 2 == 0 {
                measure(true);
                measure(false);
            } else {
                measure(false);
                measure(true);
            }
        }
        before.sort_by(f64::total_cmp);
        after.sort_by(f64::total_cmp);
        println!(
            "segments={count} before_median_ms={:.6} after_median_ms={:.6} before_p95_ms={:.6} after_p95_ms={:.6} cache_bytes={}",
            before[4],
            after[4],
            before[8],
            after[8],
            (count + 1) * 8
        );
    }
}
