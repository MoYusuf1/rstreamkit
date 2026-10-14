//! Run playback's real trim/backpressure orchestration without a browser or wall time.
use rstreamkit::net::{BufferWait, Clock, PlaybackBuffer, trim_buffer, wait_for_room};
use std::{
    cell::{Cell, RefCell},
    future::{Future, pending},
    ops::Range,
    rc::Rc,
    task::{Context, Poll, Waker},
    time::Duration,
};

fn run<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("expected fake time to complete"),
    }
}
#[derive(Default)]
struct State {
    position: Cell<f64>,
    rate: Cell<f64>,
    stop: Cell<bool>,
    jump: Cell<bool>,
    waits: RefCell<Vec<Duration>>,
}
struct FakeClock(Rc<State>);
impl Clock for FakeClock {
    async fn sleep(&self, duration: Duration) {
        self.0.waits.borrow_mut().push(duration);
        self.0
            .position
            .set(self.0.position.get() + duration.as_secs_f64() * self.0.rate.get());
    }
}
struct Buffer {
    state: Rc<State>,
    start: Option<f64>,
    end: f64,
    gaps: u32,
    removed: Vec<Range<f64>>,
    failure: bool,
    stop_on_remove: bool,
    gap_to: Option<f64>,
    oldest_calls: u32,
}
impl Buffer {
    fn new() -> Self {
        Self {
            state: Rc::new(State {
                position: Cell::new(10.0),
                rate: Cell::new(2.0),
                ..State::default()
            }),
            start: Some(0.0),
            end: 13.0,
            gaps: 0,
            removed: Vec::new(),
            failure: false,
            stop_on_remove: false,
            gap_to: None,
            oldest_calls: 0,
        }
    }
}
impl PlaybackBuffer for Buffer {
    fn position(&self) -> f64 {
        self.state.position.get()
    }
    fn ahead(&self) -> f64 {
        self.end - self.position()
    }
    fn playback_rate(&self) -> f64 {
        self.state.rate.get()
    }
    fn cross_gap(&mut self, _: f64) {
        self.gaps += 1;
        if let Some(position) = self.gap_to.take() {
            self.state.position.set(position);
        }
    }
    async fn oldest(&mut self) -> Result<Option<f64>, String> {
        self.oldest_calls += 1;
        Ok(self.start)
    }
    async fn remove(&mut self, range: Range<f64>) -> Result<(), String> {
        if self.failure {
            return Err("buffer removed during update".into());
        }
        self.start = Some(range.end);
        self.removed.push(range);
        if self.stop_on_remove {
            self.state.stop.set(true);
        }
        Ok(())
    }
}

#[test]
fn full_buffer_trims_and_waits_before_more_media_can_be_requested() {
    let mut buffer = Buffer::new();
    let state = buffer.state.clone();
    let result = run(wait_for_room(
        &FakeClock(state.clone()),
        &mut buffer,
        (1.0, 2.0),
        2.0,
        || state.stop.get(),
        || state.jump.get(),
    ))
    .unwrap();
    assert_eq!(result, BufferWait::Ready { waited: true });
    assert_eq!(*state.waits.borrow(), [Duration::from_millis(250); 4]);
    assert_eq!(buffer.removed, [0.0..8.0, 8.0..9.5]);
    assert_eq!(buffer.ahead(), 1.0);
}

#[test]
fn room_available_does_not_touch_host_or_timer() {
    let mut buffer = Buffer::new();
    buffer.end = 10.5;
    let state = buffer.state.clone();
    assert_eq!(
        run(wait_for_room(
            &FakeClock(state.clone()),
            &mut buffer,
            (1.0, 2.0),
            2.0,
            || false,
            || false
        ))
        .unwrap(),
        BufferWait::Ready { waited: false }
    );
    assert_eq!(buffer.oldest_calls, 0);
    assert!(state.waits.borrow().is_empty());
}

#[test]
fn gap_recovery_avoids_an_unneeded_timer_tick() {
    let mut buffer = Buffer::new();
    buffer.gap_to = Some(12.5);
    let state = buffer.state.clone();
    assert_eq!(
        run(wait_for_room(
            &FakeClock(state.clone()),
            &mut buffer,
            (1.0, 2.0),
            2.0,
            || false,
            || false
        ))
        .unwrap(),
        BufferWait::Ready { waited: true }
    );
    assert!(state.waits.borrow().is_empty());
    assert_eq!(buffer.removed, [0.0..10.5]);
}

#[test]
fn go_live_interrupts_without_consuming_or_waiting() {
    let mut buffer = Buffer::new();
    let state = buffer.state.clone();
    state.jump.set(true);
    assert_eq!(
        run(wait_for_room(
            &FakeClock(state.clone()),
            &mut buffer,
            (1.0, 2.0),
            2.0,
            || false,
            || state.jump.get()
        ))
        .unwrap(),
        BufferWait::Interrupted
    );
    assert_eq!(buffer.oldest_calls, 0);
    assert!(state.waits.borrow().is_empty());
}

#[test]
fn stop_before_start_touches_no_buffer() {
    let mut buffer = Buffer::new();
    let state = buffer.state.clone();
    assert_eq!(
        run(wait_for_room(
            &FakeClock(state),
            &mut buffer,
            (1.0, 2.0),
            2.0,
            || true,
            || panic!("stop takes priority")
        ))
        .unwrap(),
        BufferWait::Cancelled
    );
    assert_eq!(buffer.gaps, 0);
    assert_eq!(buffer.oldest_calls, 0);
}

#[test]
fn stop_during_removal_does_not_start_a_timer() {
    let mut buffer = Buffer::new();
    buffer.stop_on_remove = true;
    let state = buffer.state.clone();
    assert_eq!(
        run(wait_for_room(
            &FakeClock(state.clone()),
            &mut buffer,
            (1.0, 2.0),
            2.0,
            || state.stop.get(),
            || false
        ))
        .unwrap(),
        BufferWait::Cancelled
    );
    assert!(state.waits.borrow().is_empty());
}

#[test]
fn removal_error_is_returned_without_waiting_or_adding_media() {
    let mut buffer = Buffer::new();
    buffer.failure = true;
    let state = buffer.state.clone();
    assert_eq!(
        run(wait_for_room(
            &FakeClock(state.clone()),
            &mut buffer,
            (1.0, 2.0),
            2.0,
            || false,
            || false
        )),
        Err("buffer removed during update".into())
    );
    assert!(state.waits.borrow().is_empty());
}

#[test]
fn trim_keeps_small_tails_and_handles_empty_buffers() {
    let mut buffer = Buffer::new();
    buffer.start = Some(7.0);
    run(trim_buffer(&mut buffer, 2.0)).unwrap();
    assert!(buffer.removed.is_empty()); // exactly one second is retained
    buffer.start = None;
    run(trim_buffer(&mut buffer, 2.0)).unwrap();
    assert!(buffer.removed.is_empty());
}

#[test]
fn invalid_limits_cannot_remove_future_media() {
    let mut buffer = Buffer::new();
    let state = buffer.state.clone();
    for keep in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(run(trim_buffer(&mut buffer, keep)).is_err());
        assert!(
            run(wait_for_room(
                &FakeClock(state.clone()),
                &mut buffer,
                (1.0, keep),
                2.0,
                || false,
                || false
            ))
            .is_err()
        );
    }
    assert_eq!(buffer.oldest_calls, 0);
}

#[test]
fn invalid_host_duration_fails_instead_of_waiting_forever() {
    let mut buffer = Buffer::new();
    buffer.end = f64::NAN;
    let state = buffer.state.clone();
    assert!(
        run(wait_for_room(
            &FakeClock(state.clone()),
            &mut buffer,
            (1.0, 2.0),
            2.0,
            || false,
            || false
        ))
        .is_err()
    );
    assert!(state.waits.borrow().is_empty());
    assert_eq!(buffer.oldest_calls, 0);
}

#[test]
fn go_live_during_wait_interrupts_before_more_buffer_work() {
    struct JumpClock(Rc<State>);
    impl Clock for JumpClock {
        async fn sleep(&self, duration: Duration) {
            self.0.waits.borrow_mut().push(duration);
            self.0.jump.set(true);
        }
    }
    let mut buffer = Buffer::new();
    let state = buffer.state.clone();
    assert_eq!(
        run(wait_for_room(
            &JumpClock(state.clone()),
            &mut buffer,
            (1.0, 2.0),
            2.0,
            || false,
            || state.jump.get()
        ))
        .unwrap(),
        BufferWait::Interrupted
    );
    assert_eq!(buffer.oldest_calls, 1);
    assert_eq!(state.waits.borrow().len(), 1);
}

#[test]
fn dropping_a_paused_wait_releases_timer_and_preserves_the_queue() {
    struct Guard<'a>(&'a Cell<bool>);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    struct PausedClock(Cell<bool>);
    impl Clock for PausedClock {
        async fn sleep(&self, _: Duration) {
            let _guard = Guard(&self.0);
            pending::<()>().await;
        }
    }
    let mut buffer = Buffer::new();
    buffer.state.rate.set(0.0);
    let clock = PausedClock(Cell::new(false));
    let mut task = Box::pin(wait_for_room(
        &clock,
        &mut buffer,
        (1.0, 2.0),
        2.0,
        || false,
        || false,
    ));
    assert!(
        task.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    drop(task);
    assert!(clock.0.get());
    assert_eq!(buffer.position(), 10.0);
    assert_eq!(buffer.ahead(), 3.0);
}
