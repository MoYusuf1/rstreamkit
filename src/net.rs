//! Host-provided transport, usable on native targets and in the browser.
use crate::{
    Segment,
    body::{self, Capped},
};
use futures_core::Stream;
use std::{ops::Range, pin::Pin};

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
