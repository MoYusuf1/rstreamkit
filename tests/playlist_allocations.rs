//! Observe allocations on this test thread; fixture construction is excluded.
use rstreamkit::{
    hls::{self, Media, Parsed},
    session::{Batch, Session},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    hint::black_box,
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Stats {
    allocations: usize,
    reallocations: usize,
    requested_bytes: usize,
}
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static STATS: Cell<Stats> = const { Cell::new(Stats { allocations: 0, reallocations: 0, requested_bytes: 0 }) };
}
struct Counting;
// SAFETY: all allocation and deallocation operations are forwarded unchanged to
// System. Only constant-initialized, non-allocating thread-local counters are touched.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size(), false);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size, true);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn record(bytes: usize, realloc: bool) {
    if TRACK.try_with(Cell::get).unwrap_or(false) {
        let _ = STATS.try_with(|cell| {
            let mut stats = cell.get();
            if realloc {
                stats.reallocations += 1;
            } else {
                stats.allocations += 1;
            }
            stats.requested_bytes += bytes;
            cell.set(stats);
        });
    }
}
fn fixture(count: usize) -> Media {
    let text = format!(
        "#EXTM3U\n#EXT-X-TARGETDURATION:6\n{}#EXT-X-ENDLIST\n",
        (0..count)
            .map(|i| format!("#EXTINF:6,\nsegment{i:08}.ts\n"))
            .collect::<String>()
    );
    let Parsed::Media(media) = hls::parse(text.as_bytes()).unwrap() else {
        panic!()
    };
    media
}
fn measure(f: impl FnOnce()) -> Stats {
    STATS.with(|stats| stats.set(Stats::default()));
    TRACK.with(|track| track.set(true));
    struct Tracking;
    impl Drop for Tracking {
        fn drop(&mut self) {
            TRACK.with(|track| track.set(false));
        }
    }
    let tracking = Tracking;
    f();
    drop(tracking);
    STATS.with(Cell::get)
}
fn batch_allocations(media: &Media) -> Stats {
    let session = Session::default();
    measure(|| {
        for item in session.batch(black_box(media)) {
            black_box((item.remaining_seconds, item.segment.seq));
        }
    })
}
fn cursor_allocations(media: &Media) -> Stats {
    let mut session = Session::default();
    measure(|| {
        for segment in &media.segments {
            session.consumed(segment);
            black_box(&session);
        }
    })
}
#[test]
fn preparing_and_walking_a_batch_needs_no_heap_allocation() {
    for count in [0, 3, 128, 20_000] {
        assert_eq!(batch_allocations(&fixture(count)), Stats::default());
    }
}
#[test]
fn cursor_reuses_its_uri_storage_for_equal_length_names() {
    let stats = cursor_allocations(&fixture(20_000));
    assert_eq!(stats.allocations, 1);
    assert_eq!(stats.reallocations, 0);
}

#[test]
fn extreme_duration_fallback_allocation_is_bounded_independently_of_count() {
    let mut previous = None;
    for count in [3, 20_000] {
        let mut media = fixture(count);
        media.segments[0].duration = 1e308;
        media.segments[count - 1].duration = 0.25;
        let stats = batch_allocations(&media);
        assert_eq!(stats.allocations, 1);
        assert_eq!(stats.reallocations, 0);
        assert!(stats.requested_bytes <= 512);
        if let Some(previous) = previous {
            assert_eq!(stats, previous);
        }
        previous = Some(stats);
    }
}
#[test]
#[ignore = "manual allocator measurement; run in release mode"]
fn measure_playlist_allocations() {
    for count in [3, 128, 20_000] {
        let media = fixture(count);
        println!(
            "segments={count} batch={:?} cursor={:?} batch_struct_bytes={}",
            batch_allocations(&media),
            cursor_allocations(&media),
            std::mem::size_of::<Batch<'_>>()
        );
        let mut extreme = media.clone();
        extreme.segments[0].duration = 1e308;
        extreme.segments[count - 1].duration = 0.25;
        println!(
            "segments={count} extreme_batch={:?}",
            batch_allocations(&extreme)
        );
    }
}
