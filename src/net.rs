//! Host-provided transport, usable on native targets and in the browser.
use crate::{
    Segment,
    body::{self, Capped},
};
use futures_core::Stream;
use std::{future::Future, ops::Range, pin::Pin, time::Duration};

/// Host-provided timer. Native hosts may use their executor; tests can advance a fake clock.
/// Dropping the sleep future must release its pending work. No `Send` bound is required.
#[allow(async_fn_in_trait)]
pub trait Clock {
    async fn sleep(&self, duration: Duration);
}

/// A retry operation either failed with its original error or was cancelled between attempts.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum RetryError<E> {
    Failed(E),
    Cancelled,
}

/// Run at most seven attempts, with delays of 0, 1, 2, 4, 8, 8 and 8 seconds.
/// Only errors accepted by `retryable` are retried. Report reconnecting before each
/// retry delay, keeping callbacks and transport errors owned by the host.
///
/// Cancellation is checked before callbacks, timers and requests, and after timers.
/// To interrupt a request or timer already pending, drop this future; the host's
/// `Fetch` and `Clock` implementations must cancel their work on drop.
pub async fn retry<T, E, F: Future<Output = Result<T, E>>>(
    clock: &impl Clock,
    cancelled: impl Fn() -> bool,
    mut once: impl FnMut() -> F,
    retryable: impl Fn(&E) -> bool,
    mut reconnecting: impl FnMut(u32),
) -> Result<T, RetryError<E>> {
    let mut attempt = 0;
    loop {
        if cancelled() {
            return Err(RetryError::Cancelled);
        }
        if attempt > 0 {
            reconnecting(attempt);
        }
        if cancelled() {
            return Err(RetryError::Cancelled);
        }
        clock
            .sleep(Duration::from_secs(retry_delay(attempt).unwrap_or(0)))
            .await;
        if cancelled() {
            return Err(RetryError::Cancelled);
        }
        match once().await {
            Ok(value) => return Ok(value),
            Err(error) if attempt == 6 || !retryable(&error) => {
                return Err(RetryError::Failed(error));
            }
            Err(_) => attempt += 1,
        }
    }
}

/// Bounded backoff shared by native and browser playback.
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

/// How the player reaches the network. The app supplies it, so rstreamkit knows nothing about proxies,
/// credentials or headers: the browser’s `mse::Direct` adapter is the plain case, and an app that has to go through a proxy
/// implements this for it. Dropping the future or the [`Response::body`] cancels the request.
// Not `Send`: a browser implementation may own JS values. Native implementations are also usable.
#[allow(async_fn_in_trait)]
pub trait Fetch {
    /// GETs `url`, which is always the real address; with a `range` (never empty), only those bytes
    /// of it, as an HTTP `Range` request. `Ok` means a response came back, whatever its status;
    /// `Err` is for when it didn't, or when the app itself refuses or can't make the request.
    async fn get(&self, url: &str, range: Option<Range<u64>>) -> Result<Response, FetchError>;
}

pub struct Response {
    pub status: u16,
    /// Where the request ended up after redirects: relative playlist entries resolve against it.
    pub url: String,
    pub content_type: String,
    pub content_length: Option<u64>,
    /// For a range request the server answered with 206: the whole file's length, from `Content-Range`.
    pub range_total: Option<u64>,
    pub body: Body,
}

/// A response body, chunk by chunk (the player stops reading one that grows past its limit).
pub type Body = Pin<Box<dyn Stream<Item = Result<Vec<u8>, String>>>>;

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum FetchError {
    /// Might go away: a network error, a server that is down.
    Temporary(String),
    /// Won't get better by asking again: a refusal, or a proxy that doesn't answer as expected.
    Permanent(String),
}

/// An ordered output destination. Completion means the destination is ready for the next append.
/// A browser adapter waits for SourceBuffer update events; native tests can record the bytes.
#[allow(async_fn_in_trait)]
pub trait Sink {
    async fn append(&mut self, bytes: &mut [u8]) -> Result<(), String>;
}

/// Host playback buffer. Only I/O and playback-clock access belong here;
/// [`trim_buffer`] and [`wait_for_room`] own the shared removal/wait policy.
/// Pending operations must release their work when dropped. No `Send` bound is required.
#[allow(async_fn_in_trait)]
pub trait PlaybackBuffer {
    fn position(&self) -> f64;
    fn ahead(&self) -> f64;
    fn playback_rate(&self) -> f64;
    /// Cross an exhausted range's nearby gap, respecting the host's paused state.
    fn cross_gap(&mut self, target_seconds: f64);
    /// Wait for outstanding writes/removals, then return the oldest buffered time.
    async fn oldest(&mut self) -> Result<Option<f64>, String>;
    /// Remove this time range and wait for completion; report host errors.
    async fn remove(&mut self, range: Range<f64>) -> Result<(), String>;
}

/// Drop old media only when more than one second can be removed.
/// The host must preserve dependent frames according to its buffer semantics.
pub async fn trim_buffer(buffer: &mut impl PlaybackBuffer, keep: f64) -> Result<(), String> {
    if !keep.is_finite() || keep < 0.0 {
        return Err("invalid buffer retention".into());
    }
    if let Some(start) = buffer.oldest().await? {
        let cut = buffer.position() - keep;
        if start.is_finite() && cut.is_finite() && cut - start > 1.0 {
            buffer.remove(start..cut).await?;
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum BufferWait {
    Ready { waited: bool },
    Interrupted,
    Cancelled,
}

/// Apply backpressure before requesting more media. `limits` is (ahead, behind)
/// in seconds. A live host should refresh its playlist after any wait or interrupt;
/// finite input must keep its place and continue to the last segment.
/// Stop checks happen between awaits. Drop this future to cancel pending host work.
pub async fn wait_for_room(
    clock: &impl Clock,
    buffer: &mut impl PlaybackBuffer,
    limits: (f64, f64),
    target_seconds: f64,
    cancelled: impl Fn() -> bool,
    interrupted: impl Fn() -> bool,
) -> Result<BufferWait, String> {
    let (ahead, behind) = limits;
    if !ahead.is_finite() || ahead < 0.0 || !behind.is_finite() || behind < 0.0 {
        return Err("invalid buffer limits".into());
    }
    let mut waited = false;
    loop {
        if cancelled() {
            return Ok(BufferWait::Cancelled);
        }
        if interrupted() {
            return Ok(BufferWait::Interrupted);
        }
        let buffered = buffer.ahead();
        if !buffered.is_finite() {
            return Err("invalid buffered duration".into());
        }
        if buffered <= ahead {
            return Ok(BufferWait::Ready { waited });
        }
        buffer.cross_gap(target_seconds);
        trim_buffer(buffer, behind).await?;
        if cancelled() {
            return Ok(BufferWait::Cancelled);
        }
        if interrupted() {
            return Ok(BufferWait::Interrupted);
        }
        // Gap recovery can create room without a timer tick.
        let buffered = buffer.ahead();
        if !buffered.is_finite() {
            return Err("invalid buffered duration".into());
        }
        if buffered <= ahead {
            return Ok(BufferWait::Ready { waited: true });
        }
        clock
            .sleep(Duration::from_millis(buffer_delay_ms(
                buffered,
                ahead,
                buffer.playback_rate(),
            )))
            .await;
        waited = true;
    }
}

/// Wake near the threshold, bounded to 10–250 ms even with a paused/invalid clock.
pub(crate) fn buffer_delay_ms(buffered: f64, ahead: f64, rate: f64) -> u64 {
    if !buffered.is_finite() || !ahead.is_finite() || !rate.is_finite() || rate <= 0.0 {
        return 250;
    }
    ((buffered - ahead) * 1000.0 / rate).clamp(10.0, 250.0) as u64
}
/// Deliver fragments in order without concatenating or copying their media payloads.
pub async fn append_fragments(
    fragments: &mut [crate::Fragment],
    sink: &mut impl Sink,
) -> Result<(), String> {
    for fragment in fragments {
        if !fragment.moof.is_empty() {
            sink.append(&mut fragment.moof).await?;
        }
        sink.append(&mut fragment.mdat).await?;
    }
    Ok(())
}

/// Download TS incrementally, checking both advertised size and the actual body.
/// Dropping this future drops the response body, cancelling host transport work.
pub async fn download_segment(
    http: &impl Fetch,
    url: &str,
    limit: usize,
    language: Option<&str>,
) -> Result<Segment, FetchError> {
    let r = http.get(url, None).await?;
    if matches!(r.status, 404 | 410) {
        return Err(FetchError::Permanent(format!("HTTP {}", r.status)));
    }
    if !(200..300).contains(&r.status) {
        return Err(FetchError::Temporary(format!("HTTP {}", r.status)));
    }
    let too_big = || FetchError::Permanent(format!("segment exceeded {} byte limit", limit));
    if r.content_length.is_some_and(|n| n > limit as u64) {
        return Err(too_big());
    }
    let expected = r.content_length.map_or(0, |n| n.min(limit as u64) as usize);
    let mut segment = Segment::new(expected).audio_language(language);
    body::read_each(r.body, limit, |chunk| segment.feed(chunk))
        .await
        .map_err(|e| match e {
            Capped::TooBig => too_big(),
            Capped::Failed(why) => FetchError::Temporary(why),
        })?;
    Ok(segment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        future::Future,
        rc::Rc,
        task::{Context, Poll, Waker},
    };
    struct Chunks {
        data: Option<Vec<u8>>,
        pending: bool,
        drops: Rc<Cell<u32>>,
        polls: Rc<Cell<u32>>,
    }
    impl Stream for Chunks {
        type Item = Result<Vec<u8>, String>;
        fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.polls.set(self.polls.get() + 1);
            if self.pending {
                Poll::Pending
            } else {
                Poll::Ready(self.data.take().map(Ok))
            }
        }
    }
    impl Drop for Chunks {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1)
        }
    }
    struct Fake {
        status: u16,
        length: Option<u64>,
        data: Vec<u8>,
        pending: bool,
        drops: Rc<Cell<u32>>,
        polls: Rc<Cell<u32>>,
        requests: RefCell<Vec<String>>,
    }
    impl Fake {
        fn new(data: Vec<u8>) -> Self {
            Self {
                status: 200,
                length: None,
                data,
                pending: false,
                drops: Rc::default(),
                polls: Rc::default(),
                requests: RefCell::default(),
            }
        }
    }
    impl Fetch for Fake {
        async fn get(&self, url: &str, _: Option<Range<u64>>) -> Result<Response, FetchError> {
            self.requests.borrow_mut().push(url.into());
            Ok(Response {
                status: self.status,
                url: url.into(),
                content_type: "video/mp2t".into(),
                content_length: self.length,
                range_total: None,
                body: Box::pin(Chunks {
                    data: Some(self.data.clone()),
                    pending: self.pending,
                    drops: self.drops.clone(),
                    polls: self.polls.clone(),
                }),
            })
        }
    }
    fn ready<T>(f: impl Future<Output = T>) -> T {
        let mut f = std::pin::pin!(f);
        match f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(v) => v,
            Poll::Pending => panic!("fake unexpectedly suspended"),
        }
    }
    #[test]
    fn native_fetch_sink_loop_keeps_order_and_recovers_after_a_missing_segment() {
        struct Recording(Vec<Vec<u8>>);
        impl Sink for Recording {
            async fn append(&mut self, bytes: &mut [u8]) -> Result<(), String> {
                self.0.push(bytes.to_vec());
                Ok(())
            }
        }
        let playlist=b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nfirst.ts\n#EXTINF:2,\nmissing.ts\n#EXTINF:2,\nlast.ts\n#EXT-X-ENDLIST\n";
        let media = match crate::hls::parse(playlist).unwrap() {
            crate::hls::Parsed::Media(m) => m,
            _ => unreachable!(),
        };
        let mut provider = Fake::new(include_bytes!("../tests/fixtures/bbb_480p.ts").to_vec());
        let mut window = crate::live::Window::default();
        let mut tx = crate::Transmuxer::default();
        let mut sink = Recording(vec![]);
        let mut skipped = 0;
        while let Some(seg) = window.next(&media) {
            provider.status = if seg.uri == "missing.ts" { 404 } else { 200 };
            match ready(download_segment(&provider, &seg.uri, 1 << 20, None)) {
                Ok(segment) => {
                    let mut out = tx.finish(segment).unwrap();
                    ready(append_fragments(&mut out.fragments, &mut sink)).unwrap();
                }
                Err(FetchError::Permanent(_)) => {
                    skipped += 1;
                    tx.discontinuity();
                }
                Err(e) => panic!("unexpected provider error {e:?}"),
            }
            window.consumed(seg);
        }
        assert_eq!(skipped, 1);
        assert_eq!(
            &*provider.requests.borrow(),
            &["first.ts", "missing.ts", "last.ts"]
        );
        assert_eq!(sink.0.len(), 8);
        for pair in sink.0.as_chunks::<2>().0 {
            assert_eq!(&pair[0][4..8], b"moof");
            assert_eq!(&pair[1][4..8], b"mdat");
        }
        assert_eq!(provider.drops.get(), 3);
    }
    #[test]
    fn native_transport_feeds_the_real_pipeline_without_a_browser() {
        let fake = Fake::new(include_bytes!("../tests/fixtures/bbb_480p.ts").to_vec());
        let segment = ready(download_segment(
            &fake,
            "https://example.test/segment.ts",
            1 << 20,
            None,
        ))
        .unwrap();
        let out = crate::Transmuxer::default().finish(segment).unwrap();
        assert!(out.init.is_some());
        assert!(!out.fragments.is_empty());
        assert_eq!(fake.drops.get(), 1);
        assert_eq!(fake.requests.borrow().len(), 1);
    }
    #[test]
    fn cancellation_releases_a_suspended_body_and_limits_do_not_pull_it() {
        let mut fake = Fake::new(vec![0; 1024]);
        fake.pending = true;
        let mut f = Box::pin(download_segment(&fake, "segment", 1024, None));
        assert!(
            f.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        drop(f);
        assert_eq!(fake.drops.get(), 1);
        fake.length = Some(1025);
        assert!(matches!(
            ready(download_segment(&fake, "segment", 1024, None)),
            Err(FetchError::Permanent(_))
        ));
        assert_eq!(fake.polls.get(), 1);
        assert_eq!(fake.drops.get(), 2);
    }
    #[test]
    fn missing_segments_and_server_outages_have_distinct_retry_policy() {
        let mut fake = Fake::new(vec![]);
        fake.status = 404;
        assert!(matches!(
            ready(download_segment(&fake, "segment", 1024, None)),
            Err(FetchError::Permanent(_))
        ));
        fake.status = 503;
        assert!(matches!(
            ready(download_segment(&fake, "segment", 1024, None)),
            Err(FetchError::Temporary(_))
        ));
        assert_eq!(fake.polls.get(), 0);
    }
}
