//! Test the browser's retry orchestration with fake time and real transport parsing.
use rstreamkit::net::{
    BODY_TIMEOUT, Clock, Fetch, FetchError, HEADERS_TIMEOUT, Response, RetryError, Timed,
    download_segment, retry,
};
use std::{
    cell::{Cell, RefCell},
    future::{Future, pending, ready},
    ops::Range,
    pin::Pin,
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
        Poll::Pending => panic!("fake time and transport must complete synchronously"),
    }
}
#[derive(Default)]
struct FakeClock {
    now: Cell<u64>,
    waits: RefCell<Vec<u64>>,
}
impl Clock for FakeClock {
    async fn sleep(&self, duration: Duration) {
        self.waits.borrow_mut().push(duration.as_secs());
        self.now.set(self.now.get() + duration.as_secs());
    }
}
struct Chunk(Option<Result<Vec<u8>, String>>);
impl futures_core::Stream for Chunk {
    type Item = Result<Vec<u8>, String>;
    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.take())
    }
}
struct Provider<'a> {
    clock: &'a FakeClock,
    requests: RefCell<Vec<u64>>,
    outage_until: u64,
    status: u16,
    drop_first_body: bool,
}
impl Fetch for Provider<'_> {
    async fn get(&self, url: &str, _: Option<Range<u64>>) -> Result<Response, FetchError> {
        self.requests.borrow_mut().push(self.clock.now.get());
        if self.clock.now.get() < self.outage_until {
            return Err(FetchError::Temporary("offline".into()));
        }
        let chunk = if self.drop_first_body && self.requests.borrow().len() == 1 {
            Err("connection dropped".into())
        } else {
            Ok(include_bytes!("fixtures/bbb_480p.ts").to_vec())
        };
        Ok(Response {
            status: self.status,
            url: url.into(),
            content_type: "video/mp2t".into(),
            content_length: None,
            range_total: None,
            body: Box::pin(Chunk(Some(chunk))),
        })
    }
}
fn provider(clock: &FakeClock) -> Provider<'_> {
    Provider {
        clock,
        requests: RefCell::default(),
        outage_until: 0,
        status: 200,
        drop_first_body: false,
    }
}
fn temporary(error: &FetchError) -> bool {
    matches!(error, FetchError::Temporary(_))
}

#[test]
fn ten_second_outage_recovers_real_transport_at_fifteen_seconds() {
    let clock = FakeClock::default();
    let host = Provider {
        outage_until: 10,
        ..provider(&clock)
    };
    let reports = RefCell::new(Vec::new());
    let segment = run(retry(
        &clock,
        || false,
        || download_segment(&host, "https://test/segment.ts", 8 * 1024 * 1024, None),
        temporary,
        |attempt| reports.borrow_mut().push(attempt),
    ))
    .unwrap();
    let output = rstreamkit::Transmuxer::default()
        .decode_sound(false)
        .finish(segment)
        .unwrap();
    assert!(!output.fragments.is_empty());
    assert_eq!(*host.requests.borrow(), [0, 1, 3, 7, 15]);
    assert_eq!(*reports.borrow(), [1, 2, 3, 4]);
}
#[test]
fn dropped_body_retries_and_recovers() {
    let clock = FakeClock::default();
    let host = Provider {
        drop_first_body: true,
        ..provider(&clock)
    };
    assert!(
        run(retry(
            &clock,
            || false,
            || download_segment(&host, "https://test/segment.ts", 8 * 1024 * 1024, None),
            temporary,
            |_| {}
        ))
        .is_ok()
    );
    assert_eq!(*host.requests.borrow(), [0, 1]);
}
#[test]
fn missing_segment_is_not_retried() {
    let clock = FakeClock::default();
    let host = Provider {
        status: 404,
        ..provider(&clock)
    };
    let result = run(retry(
        &clock,
        || false,
        || download_segment(&host, "https://test/missing.ts", 1024, None),
        temporary,
        |_| panic!("404 must not reconnect"),
    ));
    assert!(matches!(
        result,
        Err(RetryError::Failed(FetchError::Permanent(_)))
    ));
    assert_eq!(*host.requests.borrow(), [0]);
}
#[test]
fn repeated_failure_returns_last_error_after_seven_attempts() {
    let clock = FakeClock::default();
    let calls = Cell::new(0);
    let reports = RefCell::new(Vec::new());
    let result = run(retry(
        &clock,
        || false,
        || {
            calls.set(calls.get() + 1);
            ready(Err::<(), _>(calls.get()))
        },
        |_| true,
        |attempt| reports.borrow_mut().push(attempt),
    ));
    assert_eq!(result, Err(RetryError::Failed(7)));
    assert_eq!(clock.now.get(), 31);
    assert_eq!(*reports.borrow(), [1, 2, 3, 4, 5, 6]);
}
#[test]
fn cancellation_before_start_does_no_work() {
    let clock = FakeClock::default();
    let result = run(retry(
        &clock,
        || true,
        || -> std::future::Ready<Result<(), ()>> { panic!("no request") },
        |_| true,
        |_| panic!("no callback"),
    ));
    assert_eq!(result, Err(RetryError::Cancelled));
    assert!(clock.waits.borrow().is_empty());
}
#[test]
fn cancellation_in_callback_skips_next_timer_and_request() {
    let clock = FakeClock::default();
    let stop = Cell::new(false);
    let calls = Cell::new(0);
    let result = run(retry(
        &clock,
        || stop.get(),
        || {
            calls.set(calls.get() + 1);
            ready(Err::<(), _>(()))
        },
        |_| true,
        |_| stop.set(true),
    ));
    assert_eq!(result, Err(RetryError::Cancelled));
    assert_eq!(calls.get(), 1);
    assert_eq!(*clock.waits.borrow(), [0]);
}
#[test]
fn cancellation_during_wait_skips_next_request() {
    struct StopClock(Cell<bool>);
    impl Clock for StopClock {
        async fn sleep(&self, duration: Duration) {
            if !duration.is_zero() {
                self.0.set(true);
            }
        }
    }
    let clock = StopClock(Cell::new(false));
    let calls = Cell::new(0);
    let result = run(retry(
        &clock,
        || clock.0.get(),
        || {
            calls.set(calls.get() + 1);
            ready(Err::<(), _>(()))
        },
        |_| true,
        |_| {},
    ));
    assert_eq!(result, Err(RetryError::Cancelled));
    assert_eq!(calls.get(), 1);
}
#[test]
fn dropping_retry_releases_pending_timer() {
    struct Guard<'a>(&'a Cell<bool>);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    struct PendingClock(Cell<bool>);
    impl Clock for PendingClock {
        async fn sleep(&self, _: Duration) {
            let _guard = Guard(&self.0);
            pending::<()>().await;
        }
    }
    let clock = PendingClock(Cell::new(false));
    let mut task = Box::pin(retry(
        &clock,
        || false,
        || ready(Ok::<_, ()>(())),
        |_| true,
        |_| {},
    ));
    assert!(
        task.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    drop(task);
    assert!(clock.0.get());
}
#[test]
fn dropping_retry_releases_pending_request() {
    struct Guard<'a>(&'a Cell<bool>);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    let dropped = Cell::new(false);
    let clock = FakeClock::default();
    let mut task = Box::pin(retry(
        &clock,
        || false,
        || async {
            let _guard = Guard(&dropped);
            pending::<Result<(), ()>>().await
        },
        |_| true,
        |_| {},
    ));
    assert!(
        task.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    drop(task);
    assert!(dropped.get());
}

/// A provider that accepts the request and never answers.
struct Silent;
impl Fetch for Silent {
    async fn get(&self, _: &str, _: Option<Range<u64>>) -> Result<Response, FetchError> {
        pending().await
    }
}

#[test]
fn a_server_that_never_answers_times_out_and_is_retried() {
    let clock: &'static FakeClock = Box::leak(Box::default());
    let silent = Timed(clock, Silent);
    let reports = RefCell::new(Vec::new());
    let result = run(retry(
        &clock,
        || false,
        || silent.get("https://test/live.m3u8", None),
        temporary,
        |attempt| reports.borrow_mut().push(attempt),
    ));
    assert!(
        matches!(result, Err(RetryError::Failed(FetchError::Temporary(ref why))) if why.contains("no answer"))
    );
    assert_eq!(
        *reports.borrow(),
        [1, 2, 3, 4, 5, 6],
        "reported as reconnecting"
    );
    // Seven timeouts plus the 31 s of backoff between them.
    assert_eq!(clock.now.get(), 7 * HEADERS_TIMEOUT.as_secs() + 31);
}

#[test]
fn an_answer_ready_when_the_timer_fires_still_wins() {
    let clock: &'static FakeClock = Box::leak(Box::default());
    let host = provider(clock);
    let response = run(Timed(clock, host).get("https://test/segment.ts", None)).unwrap();
    assert_eq!(response.status, 200);
}

/// A provider that sends its headers, then never another byte.
struct Stalled;
struct Never;
impl futures_core::Stream for Never {
    type Item = Result<Vec<u8>, String>;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Pending
    }
}
impl Fetch for Stalled {
    async fn get(&self, url: &str, _: Option<Range<u64>>) -> Result<Response, FetchError> {
        Ok(Response {
            status: 200,
            url: url.into(),
            content_type: "video/mp2t".into(),
            content_length: None,
            range_total: None,
            body: Box::pin(Never),
        })
    }
}

#[test]
fn a_body_that_stops_after_its_headers_fails_and_is_retried() {
    let clock: &'static FakeClock = Box::leak(Box::default());
    let stalled = Timed(clock, Stalled);
    let result = run(retry(
        clock,
        || false,
        || download_segment(&stalled, "https://test/segment.ts", 1 << 20, None),
        temporary,
        |_| {},
    ));
    assert!(
        matches!(result, Err(RetryError::Failed(FetchError::Temporary(ref why))) if why.contains("no data"))
    );
    assert_eq!(clock.now.get(), 7 * BODY_TIMEOUT.as_secs() + 31);
}
