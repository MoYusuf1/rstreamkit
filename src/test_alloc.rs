//! Thread-local test-only allocation counts; never included in shipped builds.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Stats {
    pub(crate) allocations: usize,
    pub(crate) reallocations: usize,
    pub(crate) requested_bytes: usize,
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
pub(crate) fn measure(f: impl FnOnce()) -> Stats {
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
