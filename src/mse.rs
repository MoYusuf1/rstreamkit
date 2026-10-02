//! Browser side: fetch HLS playlists and segments (through a [`Fetch`]), transmux them, and feed a
//! `<video>` element through MediaSource. Live playlists are refreshed until stopped.
//!
//! Buffer updates await browser events; dropping a player cancels its pending work.

use std::{
    cell::{Cell, RefCell},
    ops::Range,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
    time::Duration,
};

use futures_core::Stream;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Headers, HtmlVideoElement, MediaSource, MediaSourceReadyState, ReadableStreamDefaultReader,
    Request, RequestInit, Response as WebResponse, SourceBuffer,
};

use crate::cancel::cancellable;

use crate::{
    Segment, Transmuxer, Unsupported,
    body::{self, Capped},
    hls,
};

/// A playlist is a few kilobytes. Anything bigger is a stream that isn't a playlist at all, and is
/// refused once it passes this.
const PLAYLIST_LIMIT: usize = 2 << 20;
/// A segment is a few seconds of video, tens of megabytes at the very most.
const SEGMENT_LIMIT: usize = 64 << 20;

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Status {
    Playing,
    Buffering,
    Reconnecting {
        attempt: u32,
    },
    /// Something the viewer should know but playback continues (e.g. audio codec unsupported).
    Note(String),
    /// The stream can't be played as it is, and why. What to do about it (say so, play without the
    /// sound, hand the stream to something that can convert it) is up to the app.
    Unsupported(Unsupported),
    Ended,
    /// Playback stopped for any other reason, in words for the viewer.
    Failed(String),
}

/// Why playback stopped, inside the player.
enum Failure {
    Unsupported(Unsupported),
    Other(String),
}

impl From<String> for Failure {
    fn from(why: String) -> Self {
        Failure::Other(why)
    }
}

impl From<&str> for Failure {
    fn from(why: &str) -> Self {
        Failure::Other(why.to_owned())
    }
}

impl From<Unsupported> for Failure {
    fn from(why: Unsupported) -> Self {
        Failure::Unsupported(why)
    }
}

impl From<crate::Error> for Failure {
    fn from(e: crate::Error) -> Self {
        match e.unsupported() {
            Some(why) => why.into(),
            None => Failure::Other(e.to_string()),
        }
    }
}

impl From<Failure> for Status {
    fn from(f: Failure) -> Self {
        match f {
            Failure::Unsupported(why) => Status::Unsupported(why),
            Failure::Other(why) => Status::Failed(why),
        }
    }
}

impl From<Failure> for String {
    fn from(f: Failure) -> Self {
        match f {
            Failure::Unsupported(why) => why.to_string(),
            Failure::Other(why) => why,
        }
    }
}

/// Stops playback when dropped.
pub struct Player {
    video: HtmlVideoElement,
    stats: Rc<RefCell<Stats>>,
    go_live: Rc<Cell<bool>>,
    variant: Rc<RefCell<Option<String>>>,
    audio: Rc<RefCell<Option<String>>>,
    stop: Rc<Cell<bool>>,
    wake: Rc<RefCell<Option<std::task::Waker>>>,
    paused_at: Cell<Option<f64>>,
}

/// Snapshot of playback measurements. Buffer memory is an estimate from appended bytes.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub buffered_seconds: f64,
    pub latency_seconds: f64,
    pub bitrate: f64,
    pub dropped_segments: u64,
    pub current_variant: Option<String>,
    pub variants: Vec<String>,
    /// Current server playlist span; zero for finite media.
    pub live_window_seconds: f64,
}

impl Player {
    pub fn stop(&self) {
        self.stop.set(true);
        if let Some(wake) = self.wake.borrow_mut().take() {
            wake.wake();
        }
    }
    pub fn pause(&self) {
        if !self.stop.get() {
            if self.paused_at.get().is_none() {
                self.paused_at.set(Some(js_sys::Date::now() / 1000.0));
            }
            let _ = self.video.pause();
        }
    }
    pub fn resume(&self) {
        if !self.stop.get() {
            if let Some(paused) = self.paused_at.take() {
                let window = self.stats.borrow().live_window_seconds;
                if window > 0.0 && js_sys::Date::now() / 1000.0 - paused > window {
                    self.go_live.set(true);
                }
            }
            let _ = self.video.play();
        }
    }
    pub fn seek(&self, seconds: f64) {
        if !self.stop.get() && seconds.is_finite() && seconds >= 0.0 {
            self.video.set_current_time(seconds);
        }
    }
    /// Ask the downloader to refresh before jumping to the newest buffered media.
    pub fn go_live(&self) {
        if !self.stop.get() {
            self.go_live.set(true);
        }
    }
    /// Lock a variant by its master-playlist URI; `None` restores adaptation.
    pub fn set_variant(&self, uri: Option<&str>) -> Result<(), String> {
        if self.stop.get() {
            return Err("player stopped".into());
        }
        if let Some(uri) = uri
            && !self.stats.borrow().variants.iter().any(|v| v == uri)
        {
            return Err("variant is not in the master playlist".into());
        }
        *self.variant.borrow_mut() = uri.map(str::to_owned);
        Ok(())
    }
    /// Select an ISO 639 PMT audio language on the next segment.
    pub fn set_audio_track(&self, language: &str) -> Result<(), String> {
        if language.len() != 3 || !language.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err("expected a three-letter ISO 639 language".into());
        }
        if self.stop.get() {
            return Err("player stopped".into());
        }
        *self.audio.borrow_mut() = Some(language.to_ascii_lowercase());
        Ok(())
    }
    pub fn stats(&self) -> Stats {
        let mut stats = self.stats.borrow().clone();
        stats.buffered_seconds = buffered_ahead(&self.video);
        stats
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `playlist` is the playlist's real address, and `http` gets every address the player needs
/// ([`Direct`] goes straight to the server; an app behind a proxy brings its own [`Fetch`]).
/// With `partial` a stream whose sound can't be played still plays, silently; without it that is
/// reported as `Status::Unsupported` so the app can decide what to do about it. `decode_sound`
/// decodes AC-3, E-AC-3 and MP2 sound here; without it that sound is such a problem too.
pub fn start(
    video: HtmlVideoElement,
    playlist: String,
    http: impl Fetch + 'static,
    partial: bool,
    decode_sound: bool,
    report: impl FnMut(Status) + 'static,
) -> Player {
    let player_video = video.clone();
    let stats = Rc::new(RefCell::new(Stats::default()));
    let shared_stats = stats.clone();
    let go_live = Rc::new(Cell::new(false));
    let live_request = go_live.clone();
    let variant = Rc::new(RefCell::new(None));
    let variant_request = variant.clone();
    let audio = Rc::new(RefCell::new(None));
    let audio_request = audio.clone();
    let stop = Rc::new(Cell::new(false));
    let stopped = stop.clone();
    let wake = Rc::new(RefCell::new(None));
    let waking = wake.clone();
    let mut report = report;
    wasm_bindgen_futures::spawn_local(async move {
        match cancellable(
            &stopped,
            &waking,
            run(
                &video,
                playlist,
                &http,
                partial,
                decode_sound,
                &stopped,
                &shared_stats,
                &live_request,
                &variant_request,
                &audio_request,
                &mut report,
            ),
        )
        .await
        {
            Some(Ok(())) if !stopped.get() => report(Status::Ended),
            Some(Err(e)) if !stopped.get() => report(e.into()),
            _ => {}
        }
    });
    Player {
        video: player_video,
        stats,
        go_live,
        variant,
        audio,
        stop,
        wake,
        paused_at: Cell::new(None),
    }
}

fn js_err(e: JsValue) -> String {
    js_sys::Reflect::get(&e, &"message".into())
        .ok()
        .and_then(|m| m.as_string())
        .unwrap_or_else(|| format!("{e:?}"))
}

/// Where `reference` points, seen from the address `base`: resolved by the browser's own `URL`,
/// so there is nothing of ours to get wrong, and nothing to ship.
fn resolve(base: &str, reference: &str) -> Result<String, String> {
    web_sys::Url::new_with_base(reference, base)
        .map(|u| u.href())
        .map_err(|_| format!("can't resolve \"{reference}\" against \"{base}\""))
}

/// The host of an address, for error messages (never the path or credentials).
fn host_of(address: &str) -> String {
    web_sys::Url::new(address).map_or_else(|_| "the server".into(), |u| u.hostname())
}

/// Ask the browser whether it can play a MIME type with codecs, e.g. `video/mp4; codecs="avc1.64001f"`.
/// Ask it first, and do the work only where it can't.
pub fn can_play(mime: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.create_element("video").ok())
        .and_then(|e| wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlMediaElement>(e).ok())
        .is_some_and(|v| !v.can_play_type(mime).is_empty())
}

/// Whether this browser plays HLS by itself (Safari always has, and recent Chrome does too). For a
/// stream that needs nothing from us (H.264 and AAC, honest timestamps) that is the leanest way to
/// play it: no wasm, no transmuxing, hardware decoding. What it can't do is what this crate is for:
/// sound the browser can't decode (AC-3, MP2), timelines that jump without a tag, and fetching
/// through your own proxy. Whether to try native first is the app's call.
pub fn plays_hls_natively() -> bool {
    can_play("application/vnd.apple.mpegurl")
}

/// Resolves after `d`, via the browser's timer.
pub async fn sleep(d: Duration) {
    let p = js_sys::Promise::new(&mut |resolve, reject| {
        let global = js_sys::global();
        let result = js_sys::Reflect::get(&global, &"setTimeout".into())
            .and_then(|f| f.dyn_into::<js_sys::Function>())
            .and_then(|f| {
                f.call2(
                    &global,
                    &resolve,
                    &(d.as_millis().min(i32::MAX as u128) as i32).into(),
                )
            });
        if let Err(e) = result {
            let _ = reject.call1(&JsValue::UNDEFINED, &e);
        }
    });
    let _ = JsFuture::from(p).await;
}

pub use crate::net::{Body, Fetch, FetchError, Response};

/// Fetches with the browser's own `fetch`, straight from the server, which therefore has to allow it
/// (CORS; and for byte ranges, `Access-Control-Expose-Headers: Content-Range`). Good for a server you
/// run yourself; behind a proxy, implement [`Fetch`] for that instead.
#[derive(Default, Clone, Copy)]
pub struct Direct;

impl Fetch for Direct {
    async fn get(&self, url: &str, range: Option<Range<u64>>) -> Result<Response, FetchError> {
        let failed = |e: JsValue| {
            FetchError::Temporary(format!("request to {} failed: {}", host_of(url), js_err(e)))
        };
        let headers = Headers::new().map_err(failed)?;
        if let Some(r) = range {
            headers
                .set("Range", &format!("bytes={}-{}", r.start, r.end - 1))
                .map_err(failed)?;
        }
        let abort = RequestAbort(web_sys::AbortController::new().map_err(failed)?);
        let init = RequestInit::new();
        init.set_signal(Some(&abort.0.signal()));
        init.set_headers(&headers);
        let request = Request::new_with_str_and_init(url, &init).map_err(failed)?;
        // `fetch` is on the global object, in a page or in a worker alike.
        let global = js_sys::global();
        let fetch: js_sys::Function = js_sys::Reflect::get(&global, &"fetch".into())
            .map_err(failed)?
            .unchecked_into();
        let promise: js_sys::Promise = fetch
            .call1(&global, &request)
            .map_err(failed)?
            .unchecked_into();
        let response: WebResponse = JsFuture::from(promise)
            .await
            .map_err(failed)?
            .unchecked_into();

        let header = |name: &str| response.headers().get(name).ok().flatten();
        let final_url = response.url();
        Ok(Response {
            status: response.status(),
            url: if final_url.is_empty() {
                url.to_owned()
            } else {
                final_url
            },
            content_type: header("content-type").unwrap_or_default(),
            content_length: header("content-length").and_then(|v| v.parse().ok()),
            // "bytes 0-262143/1234567"
            range_total: header("content-range")
                .and_then(|v| v.rsplit('/').next().and_then(|t| t.parse().ok())),
            body: Box::pin(Chunks {
                _abort: abort,
                reader: response.body().map(|b| {
                    b.get_reader()
                        .unchecked_into::<ReadableStreamDefaultReader>()
                }),
                pending: None,
            }),
        })
    }
}

// Aborts a fetch if its future is dropped before response headers arrive.
struct RequestAbort(web_sys::AbortController);
impl Drop for RequestAbort {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The body of a `fetch` response, read chunk by chunk. Dropping it cancels the download.
struct Chunks {
    _abort: RequestAbort,
    reader: Option<ReadableStreamDefaultReader>,
    pending: Option<JsFuture>,
}

impl Stream for Chunks {
    type Item = Result<Vec<u8>, String>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        let Some(reader) = &this.reader else {
            return Poll::Ready(None);
        };
        let read = this
            .pending
            .get_or_insert_with(|| JsFuture::from(reader.read()));
        let result = match Pin::new(read).poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(result) => result,
        };
        this.pending = None;
        Poll::Ready(match result {
            Err(e) => Some(Err(js_err(e))),
            Ok(chunk) => {
                let get = |key: &str| js_sys::Reflect::get(&chunk, &key.into()).ok();
                if get("done").and_then(|d| d.as_bool()).unwrap_or(true) {
                    None
                } else {
                    Some(match get("value") {
                        Some(bytes) => Ok(js_sys::Uint8Array::new(&bytes).to_vec()),
                        None => Err("the download returned a chunk with no data".into()),
                    })
                }
            }
        })
    }
}

impl Drop for Chunks {
    fn drop(&mut self) {
        if let Some(reader) = &self.reader {
            let _ = reader.cancel();
        }
    }
}

struct Fetched {
    body: Vec<u8>,
    content_type: String,
    /// Final URL after redirects; relative playlist entries resolve against it.
    url: String,
}

/// Why one download failed, and whether asking again could help.
enum Fail {
    Retry(String),
    Final(String),
    Unsupported(Unsupported),
}

impl From<FetchError> for Fail {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::Temporary(why) => Fail::Retry(why),
            FetchError::Permanent(why) => Fail::Final(why),
        }
    }
}

/// Retries transient failures with bounded exponential backoff.
async fn with_retries<T, F: Future<Output = Result<T, Fail>>>(
    stop: &Cell<bool>,
    mut once: impl FnMut() -> F,
    report: &mut dyn FnMut(Status),
) -> Result<T, Failure> {
    let mut last = String::new();
    for attempt in 0..7 {
        if attempt > 0 && !stop.get() {
            report(Status::Reconnecting { attempt });
        }
        if let Some(delay) = crate::live::retry_delay(attempt) {
            sleep(Duration::from_secs(delay)).await;
        }
        if stop.get() {
            break;
        }
        match once().await {
            Ok(v) => return Ok(v),
            // A refusal, or something that isn't what was asked for, won't get better by asking again.
            Err(Fail::Final(e)) => return Err(e.into()),
            Err(Fail::Unsupported(why)) => return Err(why.into()),
            Err(Fail::Retry(e)) => last = e,
        }
    }
    Err(last.into())
}

async fn fetch_once(http: &impl Fetch, url: &str, limit: usize) -> Result<Fetched, Fail> {
    fetch_resource(http, url, None, limit).await
}
async fn fetch_resource(
    http: &impl Fetch,
    url: &str,
    range: Option<Range<u64>>,
    limit: usize,
) -> Result<Fetched, Fail> {
    let host = host_of(url);
    let r = http.get(url, range.clone()).await?;
    if range.is_some() && r.status != 206 {
        return Err(Fail::Final("server did not honor HLS byte range".into()));
    }
    if matches!(r.status, 404 | 410) {
        return Err(Fail::Final(format!("{host} answered HTTP {}", r.status)));
    }
    if !(200..300).contains(&r.status) {
        return Err(Fail::Retry(format!("{host} answered HTTP {}", r.status)));
    }
    // Refuse a raw stream by what the server says it is, or how long it says it is, before reading
    // any of it.
    if limit == PLAYLIST_LIMIT {
        let kind = r.content_type.to_ascii_lowercase();
        if kind.starts_with("video/") && !kind.contains("mpegurl") {
            return Err(Fail::Unsupported(Unsupported::RawStream));
        }
    }
    if r.content_length.is_some_and(|n| n > limit as u64) {
        return Err(Fail::Final(too_big(&host, limit)));
    }
    // The length may be missing or wrong (chunked, or a stream dressed up as a file): count as it arrives.
    let expected = r.content_length.map_or(0, |n| n as usize);
    let body = body::read_capped(r.body, limit, expected)
        .await
        .map_err(|e| match e {
            // Reading it again would only read it all again.
            Capped::TooBig => Fail::Final(too_big(&host, limit)),
            Capped::Failed(why) => Fail::Retry(format!("download from {host} failed: {why}")),
        })?;
    Ok(Fetched {
        body,
        content_type: r.content_type,
        url: r.url,
    })
}

async fn fetch(
    http: &impl Fetch,
    url: &str,
    limit: usize,
    stop: &Cell<bool>,
    report: &mut dyn FnMut(Status),
) -> Result<Fetched, Failure> {
    with_retries(stop, || fetch_once(http, url, limit), report).await
}

#[derive(Default)]
struct Format {
    cmaf: crate::cmaf::Rebaser,
    map: Option<(String, hls::Map, Option<hls::Key>)>,
}

async fn decrypt(bytes: &[u8], key: &[u8], iv: &[u8; 16]) -> Result<Vec<u8>, Fail> {
    if key.len() != 16 {
        return Err(Fail::Final("AES-128 key must contain 16 bytes".into()));
    }
    let inner = async {
        let global = js_sys::global();
        let crypto = js_sys::Reflect::get(&global, &"crypto".into())?;
        let subtle = js_sys::Reflect::get(&crypto, &"subtle".into())?;
        let import =
            js_sys::Reflect::get(&subtle, &"importKey".into())?.dyn_into::<js_sys::Function>()?;
        let usages = js_sys::Array::new();
        usages.push(&"decrypt".into());
        let key = JsFuture::from(
            import
                .call5(
                    &subtle,
                    &"raw".into(),
                    &js_sys::Uint8Array::from(key),
                    &"AES-CBC".into(),
                    &false.into(),
                    &usages,
                )?
                .dyn_into::<js_sys::Promise>()?,
        )
        .await?;
        let algorithm = js_sys::Object::new();
        js_sys::Reflect::set(&algorithm, &"name".into(), &"AES-CBC".into())?;
        js_sys::Reflect::set(&algorithm, &"iv".into(), &js_sys::Uint8Array::from(&iv[..]))?;
        let decrypt =
            js_sys::Reflect::get(&subtle, &"decrypt".into())?.dyn_into::<js_sys::Function>()?;
        let plain = JsFuture::from(
            decrypt
                .call3(&subtle, &algorithm, &key, &js_sys::Uint8Array::from(bytes))?
                .dyn_into::<js_sys::Promise>()?,
        )
        .await?;
        Ok::<_, JsValue>(js_sys::Uint8Array::new(&plain).to_vec())
    };
    inner
        .await
        .map_err(|e| Fail::Final(format!("HLS AES-CBC decryption failed: {}", js_err(e))))
}

/// A segment, downloaded straight into the transmuxer as it arrives, so no copy of it is ever held
/// whole. A download that fails part way is tried again from the start, into a fresh segment.
#[allow(clippy::too_many_arguments)]
async fn fetch_segment(
    http: &impl Fetch,
    url: &str,
    limit: usize,
    stop: &Cell<bool>,
    tx: &RefCell<Transmuxer>,
    info: &hls::Segment,
    base: &str,
    format: &RefCell<Format>,
    report: &mut dyn FnMut(Status),
) -> Result<crate::Output, Failure> {
    with_retries(
        stop,
        || fetch_segment_once(http, url, limit, tx, info, base, format),
        report,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn fetch_segment_once(
    http: &impl Fetch,
    url: &str,
    limit: usize,
    tx: &RefCell<Transmuxer>,
    info: &hls::Segment,
    base: &str,
    format: &RefCell<Format>,
) -> Result<crate::Output, Fail> {
    if info.key.is_some() || info.byte_range.is_some() || info.map.is_some() {
        let mut fetched = fetch_resource(http, url, info.byte_range.clone(), limit).await?;
        if let Some(key) = &info.key {
            let key_url = resolve(base, &key.uri).map_err(Fail::Final)?;
            let key_bytes = fetch_resource(http, &key_url, None, 16).await?.body;
            fetched.body = decrypt(&fetched.body, &key_bytes, &key.iv_for(info.seq)).await?;
        }
        if let Some(map) = &info.map {
            let mut init = None;
            let map_url = resolve(base, &map.uri).map_err(Fail::Final)?;
            let identity = (map_url.clone(), map.clone(), info.key.clone());
            if format.borrow().map.as_ref() != Some(&identity) {
                let mut bytes = fetch_resource(http, &map_url, map.byte_range.clone(), 4 << 20)
                    .await?
                    .body;
                if let Some(key) = &info.key {
                    let iv = key.iv.ok_or_else(|| {
                        Fail::Final("encrypted initialization needs explicit IV".into())
                    })?;
                    let key_url = resolve(base, &key.uri).map_err(Fail::Final)?;
                    let key_bytes = fetch_resource(http, &key_url, None, 16).await?.body;
                    bytes = decrypt(&bytes, &key_bytes, &iv).await?;
                }
                let mime = format
                    .borrow_mut()
                    .cmaf
                    .init(&bytes)
                    .map_err(|e| Fail::Final(e.to_string()))?;
                format.borrow_mut().map = Some(identity);
                init = Some(crate::Init {
                    bytes,
                    mime,
                    interlaced: false,
                });
            }
            format
                .borrow_mut()
                .cmaf
                .fragment(&mut fetched.body, info.duration)
                .map_err(|e| Fail::Final(e.to_string()))?;
            return Ok(crate::Output {
                init,
                fragments: vec![crate::Fragment {
                    moof: vec![],
                    mdat: fetched.body,
                }],
                skipped_audio: None,
            });
        }
        let mut segment = tx.borrow().segment(fetched.body.len());
        segment.feed(&fetched.body);
        return finish_segment(tx, segment).await.map_err(|e| match e {
            Failure::Unsupported(why) => Fail::Unsupported(why),
            Failure::Other(why) => Fail::Final(why),
        });
    }
    let language = tx.borrow().audio_language.clone();
    let segment = download_segment(http, url, limit, language).await?;
    finish_segment(tx, segment).await.map_err(|e| match e {
        Failure::Unsupported(why) => Fail::Unsupported(why),
        Failure::Other(e) => Fail::Final(e),
    })
}

async fn finish_segment(
    tx: &RefCell<Transmuxer>,
    segment: Segment,
) -> Result<crate::Output, Failure> {
    let mut transmuxer = std::mem::take(&mut *tx.borrow_mut());
    let result = transmuxer
        .finish_yielded(segment)
        .await
        .map_err(Failure::from);
    *tx.borrow_mut() = transmuxer;
    result
}

async fn download_segment(
    http: &impl Fetch,
    url: &str,
    limit: usize,
    language: Option<String>,
) -> Result<Segment, Fail> {
    crate::net::download_segment(http, url, limit, language.as_deref())
        .await
        .map_err(Fail::from)
}

// One speculative download, polled alongside browser appends. No decoded state is changed
// until the caller consumes it, and dropping it cancels the reader/request.
type DownloadFuture<'a> = Pin<Box<dyn Future<Output = Result<Segment, Fail>> + 'a>>;
struct Prefetch<'a> {
    uri: String,
    future: Option<DownloadFuture<'a>>,
    ready: Option<Result<Segment, Fail>>,
    started: f64,
    seconds: Option<f64>,
}
async fn with_prefetch<T>(
    future: impl Future<Output = T>,
    prefetch: &mut Option<Prefetch<'_>>,
) -> T {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| {
        if let Some(p) = prefetch
            && let Some(f) = p.future.as_mut()
            && let Poll::Ready(value) = f.as_mut().poll(cx)
        {
            p.future = None;
            p.ready = Some(value);
            p.seconds = Some((js_sys::Date::now() / 1000.0 - p.started).max(0.001));
        }
        future.as_mut().poll(cx)
    })
    .await
}

struct BrowserSink<'a, 'b, 'c> {
    video: &'a HtmlVideoElement,
    buffer: &'a SourceBuffer,
    ms: &'a MediaSource,
    prefetch: &'b mut Option<Prefetch<'c>>,
}
impl crate::net::Sink for BrowserSink<'_, '_, '_> {
    async fn append(&mut self, bytes: &mut [u8]) -> Result<(), String> {
        with_prefetch(
            append(self.video, self.buffer, self.ms, bytes),
            self.prefetch,
        )
        .await
    }
}

fn too_big(host: &str, limit: usize) -> String {
    let what = if limit == PLAYLIST_LIMIT {
        "a playlist"
    } else {
        "a segment"
    };
    format!(
        "{host} sent more than {} MB where {what} was expected; that is a stream, not HLS",
        limit >> 20
    )
}

/// When a "playlist" isn't one, say what it was (`hls::describe_non_playlist`).
fn describe(f: &Fetched, why: impl ToString) -> Failure {
    let why = why.to_string();
    // Transport stream packets are unmistakable, whatever the server called the response (and
    // binary data fails as "not text" before anything looks for a playlist header).
    if hls::looks_like_ts(&f.body) {
        Unsupported::RawStream.into()
    } else if why.contains("EXTM3U") {
        Failure::Other(hls::describe_non_playlist(&f.body, &f.content_type))
    } else {
        Failure::Other(why)
    }
}

fn parse_media(f: &Fetched) -> Result<hls::Media, Failure> {
    match hls::parse(&f.body).map_err(|e| describe(f, e))? {
        hls::Parsed::Media(m) => Ok(m),
        hls::Parsed::Master(_) => {
            Err("expected a media playlist but got another master playlist".into())
        }
    }
}

/// Seconds of media buffered ahead of the playhead (0 if none).
fn buffered_ahead(video: &HtmlVideoElement) -> f64 {
    let b = video.buffered();
    match b.length().checked_sub(1).and_then(|last| b.end(last).ok()) {
        Some(end) => (end - video.current_time()).max(0.0),
        None => 0.0,
    }
}

fn cross_live_gap(video: &HtmlVideoElement, target: f64) {
    if video.paused() {
        return;
    }
    let b = video.buffered();
    let ranges: Vec<_> = (0..b.length())
        .filter_map(|i| Some((b.start(i).ok()?, b.end(i).ok()?)))
        .collect();
    if let Some(start) =
        crate::live::gap_target(&ranges, video.current_time(), target.max(1.0) * 2.0)
    {
        video.set_current_time(start);
    }
}

/// Resolves when the buffer has finished what it was doing: the browser says so (`updateend`, which
/// follows success, error and abort alike), so nothing has to ask every few milliseconds.
async fn wait_idle(sb: &SourceBuffer, ms: &MediaSource) -> Result<(), String> {
    loop {
        if ms.ready_state() == MediaSourceReadyState::Closed {
            return Err("the media source closed".into());
        }
        sb.buffered().map_err(js_err)?;
        if !sb.updating() {
            return Ok(());
        }
        let mut listener = None;
        let done = js_sys::Promise::new(&mut |resolve, _| {
            let callback = Closure::<dyn FnMut()>::new(move || {
                let _ = resolve.call0(&JsValue::UNDEFINED);
            });
            sb.set_onupdateend(Some(callback.as_ref().unchecked_ref()));
            ms.set_onsourceclose(Some(callback.as_ref().unchecked_ref()));
            ms.set_onsourceended(Some(callback.as_ref().unchecked_ref()));
            ms.source_buffers()
                .set_onremovesourcebuffer(Some(callback.as_ref().unchecked_ref()));
            listener = Some(callback);
        });
        // Clear handlers even when the player is dropped during the await.
        struct Listener<'a>(
            &'a SourceBuffer,
            &'a MediaSource,
            Option<Closure<dyn FnMut()>>,
        );
        impl Drop for Listener<'_> {
            fn drop(&mut self) {
                self.0.set_onupdateend(None);
                self.1.set_onsourceclose(None);
                self.1.set_onsourceended(None);
                self.1.source_buffers().set_onremovesourcebuffer(None);
                self.2.take();
            }
        }
        let _listener = Listener(sb, ms, listener);
        let _ = JsFuture::from(done).await;
        if ms.ready_state() != MediaSourceReadyState::Open {
            return Err("the media source closed".into());
        }
        sb.buffered().map_err(js_err)?; // Removed buffers throw even if the source is open.
        if !sb.updating() {
            return Ok(());
        }
    }
}

/// Drops buffered media far behind the playhead so a long live session doesn't fill memory.
async fn trim(
    video: &HtmlVideoElement,
    sb: &SourceBuffer,
    ms: &MediaSource,
    keep: f64,
) -> Result<(), String> {
    wait_idle(sb, ms).await?;
    let b = sb.buffered().map_err(js_err)?;
    if b.length() == 0 {
        return Ok(());
    }
    let start = b.start(0).map_err(js_err)?;
    let cut = video.current_time() - keep;
    if cut - start > 1.0 {
        let _ = sb.remove(start, cut);
        wait_idle(sb, ms).await?;
    }
    Ok(())
}

/// Hands `bytes` to the browser, which copies what it needs before returning: they are lent as a
/// view of wasm memory, not copied into a JS buffer of ours first.
async fn append(
    video: &HtmlVideoElement,
    sb: &SourceBuffer,
    ms: &MediaSource,
    bytes: &mut [u8],
) -> Result<(), String> {
    wait_idle(sb, ms).await?;
    if let Err(e) = sb.append_buffer_with_u8_array(bytes) {
        // Out of room: free everything old and try once more.
        trim(video, sb, ms, 5.0).await?;
        sb.append_buffer_with_u8_array(bytes)
            .map_err(|_| js_err(e))?;
    }
    wait_idle(sb, ms).await?;
    Ok(())
}

struct ObjectUrl(String);

impl Drop for ObjectUrl {
    fn drop(&mut self) {
        let _ = web_sys::Url::revoke_object_url(&self.0);
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    video: &HtmlVideoElement,
    playlist: String,
    http: &impl Fetch,
    partial: bool,
    decode_sound: bool,
    stop: &Cell<bool>,
    stats: &RefCell<Stats>,
    go_live: &Cell<bool>,
    variant_request: &RefCell<Option<String>>,
    audio_request: &RefCell<Option<String>>,
    report: &mut dyn FnMut(Status),
) -> Result<(), Failure> {
    // Never report after the viewer left: the UI state behind the callback may be gone.
    let mut last_state = None;
    let mut say = |s: Status| {
        if !stop.get() {
            if !matches!(s, Status::Note(_)) {
                if last_state.as_ref() == Some(&s) {
                    return;
                }
                last_state = Some(s.clone());
            }
            report(s)
        }
    };

    // A master playlist points at variants; pick one and follow it. `media_url` is what we refresh.
    say(Status::Buffering);
    let first = match fetch(http, &playlist, PLAYLIST_LIMIT, stop, &mut say).await {
        Err(Failure::Unsupported(Unsupported::RawStream)) => {
            return run_ts(video, &playlist, http, partial, decode_sound, stop, report).await;
        }
        result => result?,
    };
    let (mut media_url, mut latest, master_base, variants, mut variant_uri, mut audio_url) =
        match hls::parse(&first.body).map_err(|e| describe(&first, e))? {
            hls::Parsed::Media(_) => (playlist, first, None, vec![], String::new(), None),
            hls::Parsed::Master(variants) => {
                let v = hls::pick_variant(&variants).ok_or("the playlist lists no streams")?;
                let variant_uri = v.uri.clone();
                let audio_url = v
                    .renditions
                    .iter()
                    .filter(|r| r.uri.is_some())
                    .max_by_key(|r| r.default)
                    .and_then(|r| r.uri.as_deref())
                    .map(|uri| resolve(&first.url, uri))
                    .transpose()?;
                let url = resolve(&first.url, &v.uri)?;
                let f = fetch(http, &url, PLAYLIST_LIMIT, stop, &mut say).await?;
                (url, f, Some(first.url), variants, variant_uri, audio_url)
            }
        };
    stats.borrow_mut().variants = variants.iter().map(|v| v.uri.clone()).collect();
    let mut abr = crate::live::Abr::default();
    let mut media = parse_media(&latest)?;

    let mut ms = MediaSource::new().map_err(js_err)?;
    let mut object_url =
        ObjectUrl(web_sys::Url::create_object_url_with_source(&ms).map_err(js_err)?);
    video.set_src(&object_url.0);
    for _ in 0..500 {
        if ms.ready_state() == MediaSourceReadyState::Open {
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    if ms.ready_state() != MediaSourceReadyState::Open {
        return Err("the browser did not open the media source".into());
    }

    let tx = RefCell::new(Transmuxer::default().decode_sound(decode_sound));
    let format = RefCell::new(Format::default());
    let mut sb: Option<SourceBuffer> = None;
    let mut window = crate::live::Window::default();
    let mut bytes_per_second = 0.0;
    let mut started = false;
    let mut warned_audio = false;
    let mut frontier = 0.0;
    let mut jump_requested = false;
    let mut prefetch: Option<Prefetch<'_>> = None;
    let mut selected_audio = None;
    let mut audio_state = RenditionState::default();
    let mut video_init: Option<crate::Init> = None;

    loop {
        let wanted_audio = audio_request.borrow().clone();
        if selected_audio != wanted_audio {
            prefetch = None;
            selected_audio = wanted_audio;
            let mut current = tx.borrow_mut();
            current.audio_language = selected_audio.clone();
            if let (Some(base), Some(language)) = (&master_base, &selected_audio)
                && let Some(r) = variants
                    .iter()
                    .find(|v| v.uri == variant_uri)
                    .and_then(|v| {
                        v.renditions
                            .iter()
                            .find(|r| r.language.as_deref() == Some(language))
                    })
                && let Some(uri) = &r.uri
            {
                audio_url = Some(resolve(base, uri)?);
                audio_state = RenditionState::default();
            }
        }
        if go_live.replace(false) {
            latest = fetch(http, &media_url, PLAYLIST_LIMIT, stop, &mut say).await?;
            media = parse_media(&latest)?;
            window = crate::live::Window::default();
            jump_requested = true;
        }
        // Refresh after throttling: old playlist entries may have expired during a pause.
        let (ahead, _) = crate::live::buffer_limits(bytes_per_second, media.target_duration);
        let mut throttled = false;
        while buffered_ahead(video) > ahead {
            if go_live.get() {
                throttled = true;
                break;
            }
            cross_live_gap(video, media.target_duration);
            throttled = true;
            sleep(Duration::from_millis(250)).await;
        }
        if throttled && !media.ended {
            prefetch = None;
            latest = fetch(http, &media_url, PLAYLIST_LIMIT, stop, &mut say).await?;
            media = parse_media(&latest)?;
            if go_live.replace(false) {
                window = crate::live::Window::default();
                jump_requested = true;
            }
        }
        while let Some(seg) = window.next(&media) {
            let url = resolve(&latest.url, &seg.uri)?;
            let fetch_started = js_sys::Date::now() / 1000.0;
            if seg.discontinuity || window.discontinuous(seg) {
                tx.borrow_mut().discontinuity();
            }
            let mut download_seconds = None;
            let speculative = prefetch.as_ref().is_some_and(|p| p.uri == seg.uri);
            let fetched = if prefetch.as_ref().is_some_and(|p| p.uri == seg.uri) {
                let mut pending = prefetch.take().unwrap();
                let downloaded = match pending.ready.take() {
                    Some(value) => value,
                    None => pending.future.take().unwrap().await,
                };
                download_seconds = pending
                    .seconds
                    .or_else(|| Some((js_sys::Date::now() / 1000.0 - pending.started).max(0.001)));
                match downloaded {
                    Ok(segment) => finish_segment(&tx, segment).await,
                    Err(_) => {
                        fetch_segment(
                            http,
                            &url,
                            SEGMENT_LIMIT,
                            stop,
                            &tx,
                            seg,
                            &latest.url,
                            &format,
                            &mut say,
                        )
                        .await
                    }
                }
            } else {
                prefetch = None;
                fetch_segment(
                    http,
                    &url,
                    SEGMENT_LIMIT,
                    stop,
                    &tx,
                    seg,
                    &latest.url,
                    &format,
                    &mut say,
                )
                .await
            };
            let mut out = match fetched {
                Ok(out) => out,
                Err(Failure::Other(why)) if !media.ended => {
                    window.consumed(seg);
                    stats.borrow_mut().dropped_segments += 1;
                    tx.borrow_mut().discontinuity();
                    say(Status::Note(format!("skipping unavailable segment: {why}")));
                    continue;
                }
                Err(e) => return Err(e),
            };
            if stop.get() {
                return Ok(());
            }
            let appended: usize = out
                .fragments
                .iter()
                .map(|f| f.moof.len() + f.mdat.len())
                .sum();
            let seconds = if seg.duration > 0.0 {
                seg.duration
            } else {
                media.target_duration.max(1.0)
            };
            let measured = appended as f64 / seconds;
            if !speculative {
                abr.observe(
                    appended,
                    (js_sys::Date::now() / 1000.0 - fetch_started).max(0.001),
                );
            }
            if let Some(seconds) = download_seconds {
                abr.observe(appended, seconds);
            }
            bytes_per_second = if bytes_per_second == 0.0 {
                measured
            } else {
                0.8 * bytes_per_second + 0.2 * measured
            };
            if let Some(codec) = &out.skipped_audio {
                if !partial {
                    return Err(Unsupported::Sound(codec.clone()).into());
                }
                if !warned_audio {
                    warned_audio = true;
                    say(Status::Note(format!(
                        "{codec} audio can't be played in the browser; playing without sound"
                    )));
                }
            }
            if !video.paused()
                && prefetch.is_none()
                && let Some(next) = media
                    .segments
                    .iter()
                    .position(|s| {
                        s.seq == seg.seq && s.uri == seg.uri && s.byte_range == seg.byte_range
                    })
                    .and_then(|i| media.segments.get(i + 1))
                && next.key.is_none()
                && next.map.is_none()
                && next.byte_range.is_none()
            {
                let next_url = resolve(&latest.url, &next.uri)?;
                let language = selected_audio.clone();
                prefetch = Some(Prefetch {
                    uri: next.uri.clone(),
                    future: Some(Box::pin(async move {
                        download_segment(http, &next_url, SEGMENT_LIMIT, language).await
                    })),
                    ready: None,
                    started: js_sys::Date::now() / 1000.0,
                    seconds: None,
                });
            }
            if let Some(url) = &audio_url {
                let mut audio = get_rendition(
                    http,
                    url,
                    stop,
                    &mut audio_state,
                    frontier + seconds,
                    decode_sound,
                    partial,
                    &mut say,
                )
                .await?;
                let video_changed = out.init.is_some();
                if let Some(init) = out.init.take() {
                    video_init = Some(init);
                }
                if (video_changed || audio.init.is_some())
                    && let (Some(video_init), Some(audio_init)) = (&video_init, &audio_state.init)
                {
                    let (bytes, mime) =
                        crate::cmaf::merge_init(&video_init.bytes, &audio_init.bytes)?;
                    out.init = Some(crate::Init {
                        bytes,
                        mime,
                        interlaced: video_init.interlaced,
                    });
                }
                let offset = match (audio_state.tx.borrow().vbase, tx.borrow().vbase) {
                    (Some(a), Some(v)) => (a - v) * i64::from(audio_state.rate) / 90_000,
                    _ => 0,
                };
                for f in &mut audio.fragments {
                    if f.moof.is_empty() {
                        crate::cmaf::audio_fragment(&mut f.mdat, offset)?;
                    } else {
                        crate::cmaf::audio_fragment(&mut f.moof, offset)?;
                    }
                }
                out.fragments.extend(audio.fragments);
            }
            if let Some(mut init) = out.init {
                if init.interlaced && !partial {
                    return Err(Unsupported::Interlaced.into());
                }
                if let Some(old_buffer) = &sb {
                    // Track additions cannot be made by appending an init with a different
                    // track count. Let previously buffered pictures play before reopening,
                    // otherwise a fast download would discard the entire first segment.
                    wait_idle(old_buffer, &ms).await?;
                    ms.end_of_stream().map_err(js_err)?;
                    while !video.ended() && buffered_ahead(video) > 0.05 {
                        if ms.ready_state() == MediaSourceReadyState::Closed {
                            return Err("media source closed during a track change".into());
                        }
                        sleep(Duration::from_millis(20)).await;
                    }
                    ms = MediaSource::new().map_err(js_err)?;
                    object_url = ObjectUrl(
                        web_sys::Url::create_object_url_with_source(&ms).map_err(js_err)?,
                    );
                    video.set_src(&object_url.0);
                    for _ in 0..500 {
                        if ms.ready_state() == MediaSourceReadyState::Open {
                            break;
                        }
                        sleep(Duration::from_millis(10)).await;
                    }
                    if ms.ready_state() != MediaSourceReadyState::Open {
                        return Err("the browser did not reopen the media source".into());
                    }
                    started = false;
                }
                let buffer =
                    ms.add_source_buffer(&init.mime)
                        .map_err(|e| Unsupported::MediaType {
                            mime: init.mime.clone(),
                            why: js_err(e),
                        })?;
                with_prefetch(append(video, &buffer, &ms, &mut init.bytes), &mut prefetch).await?;
                sb = Some(buffer);
            }
            let buffer = sb.as_ref().ok_or("no media buffer")?;
            crate::net::append_fragments(
                &mut out.fragments,
                &mut BrowserSink {
                    video,
                    buffer,
                    ms: &ms,
                    prefetch: &mut prefetch,
                },
            )
            .await?;
            window.consumed(seg);
            frontier += seconds;
            let remaining: f64 = media
                .segments
                .iter()
                .skip_while(|s| {
                    s.seq != seg.seq || s.uri != seg.uri || s.byte_range != seg.byte_range
                })
                .skip(1)
                .map(|s| s.duration)
                .sum();
            let ranges = video.buffered();
            let end = ranges
                .length()
                .checked_sub(1)
                .and_then(|i| ranges.end(i).ok())
                .unwrap_or(frontier);
            let latency = end + remaining - video.current_time();
            {
                let mut state = stats.borrow_mut();
                state.bitrate = bytes_per_second * 8.0;
                state.buffered_seconds = buffered_ahead(video);
                state.latency_seconds = latency.max(0.0);
                state.current_variant = Some(variant_uri.clone());
                state.live_window_seconds = if media.ended { 0.0 } else { media.duration() };
            }
            if !media.ended {
                let target = (media.target_duration * 3.0).max(3.0);
                // A modest catch-up rate after stalls; don't change a paused viewer's position.
                if !video.paused() {
                    video.set_playback_rate(if latency > target + 2.0 { 1.05 } else { 1.0 });
                }
                let _ = ms.set_live_seekable_range((end - media.duration()).max(0.0), end);
            }
            if jump_requested && !media.ended {
                video.set_current_time((end - media.target_duration.max(1.0)).max(0.0));
                jump_requested = false;
            }
            if !started {
                started = true;
                if let Some(start) = buffer.buffered().ok().and_then(|b| b.start(0).ok())
                    && video.current_time() < start
                {
                    video.set_current_time(start);
                }
                let _ = video.play();
                say(Status::Playing);
            }
            if buffered_ahead(video) >= 0.5 {
                say(Status::Playing);
            }
            let (ahead, behind) =
                crate::live::buffer_limits(bytes_per_second, media.target_duration);
            trim(video, buffer, &ms, behind).await?;
            if let Some(base) = &master_base {
                let requested = variant_request.borrow().clone();
                let candidate = if let Some(uri) = requested {
                    variants
                        .iter()
                        .find(|v| v.uri == uri && v.uri != variant_uri)
                } else {
                    abr.choose(
                        &variants,
                        &variant_uri,
                        buffered_ahead(video),
                        media.target_duration,
                    )
                };
                if let Some(candidate) = candidate {
                    prefetch = None;
                    variant_uri = candidate.uri.clone();
                    let selected = candidate
                        .renditions
                        .iter()
                        .filter(|r| r.uri.is_some())
                        .max_by_key(|r| r.default)
                        .and_then(|r| r.uri.as_deref())
                        .map(|uri| resolve(base, uri))
                        .transpose()?;
                    if selected != audio_url {
                        audio_state = RenditionState::default();
                        audio_url = selected;
                    }
                    media_url = resolve(base, &variant_uri)?;
                    say(Status::Note(format!(
                        "switching variant to {}",
                        candidate.bandwidth
                    )));
                    latest = fetch(http, &media_url, PLAYLIST_LIMIT, stop, &mut say).await?;
                    media = parse_media(&latest)?;
                    break;
                }
            }
            if buffered_ahead(video) > ahead {
                break;
            }
        }

        if media.ended {
            if let Some(b) = &sb {
                wait_idle(b, &ms).await?;
            }
            ms.end_of_stream().map_err(js_err)?;
            // End-of-input is different from end-of-playback, especially after a source rebuild.
            // The element can publish its buffered ranges after the SourceBuffer update event.
            while !video.ended() && !stop.get() {
                if ms.ready_state() == MediaSourceReadyState::Closed {
                    return Err("the media source closed".into());
                }
                sleep(Duration::from_millis(250)).await;
            }
            return Ok(());
        }

        // Live: wait about half a segment, then look for new ones.
        sleep(Duration::from_secs_f64(
            (media.target_duration / 2.0).clamp(1.0, 5.0),
        ))
        .await;
        if stop.get() {
            return Ok(());
        }
        if window.next(&media).is_none()
            && window.frozen(js_sys::Date::now() / 1000.0, media.target_duration)
        {
            say(Status::Reconnecting { attempt: 1 });
        } else if buffered_ahead(video) < 0.5 {
            say(Status::Buffering);
        }
        latest = fetch(http, &media_url, PLAYLIST_LIMIT, stop, &mut say).await?;
        media = parse_media(&latest)?;
        cross_live_gap(video, media.target_duration);
    }
}

#[derive(Default)]
struct RenditionState {
    window: crate::live::Window,
    tx: RefCell<Transmuxer>,
    format: RefCell<Format>,
    init: Option<crate::Init>,
    frontier: f64,
    rate: u32,
}
#[allow(clippy::too_many_arguments)]
async fn get_rendition(
    http: &impl Fetch,
    url: &str,
    stop: &Cell<bool>,
    state: &mut RenditionState,
    horizon: f64,
    decode_sound: bool,
    partial: bool,
    report: &mut dyn FnMut(Status),
) -> Result<crate::Output, Failure> {
    let latest = fetch(http, url, PLAYLIST_LIMIT, stop, report).await?;
    let media = parse_media(&latest)?;
    state.tx.borrow_mut().skip_sound = !decode_sound;
    let mut result = crate::Output {
        init: None,
        fragments: vec![],
        skipped_audio: None,
    };
    while state.frontier < horizon {
        let Some(segment) = state.window.next(&media) else {
            break;
        };
        let source = resolve(&latest.url, &segment.uri)?;
        let mut out = fetch_segment(
            http,
            &source,
            SEGMENT_LIMIT,
            stop,
            &state.tx,
            segment,
            &latest.url,
            &state.format,
            report,
        )
        .await?;
        if let Some(codec) = out.skipped_audio
            && !partial
        {
            return Err(Unsupported::Sound(codec).into());
        }
        if let Some(init) = out.init.take() {
            result.init = Some(crate::Init {
                bytes: vec![],
                mime: init.mime.clone(),
                interlaced: false,
            });
            state.init = Some(init);
            let tx = state.tx.borrow();
            state.rate = match tx.audio {
                crate::Audio::Aac => tx.aac.as_ref().map_or(48000, |c| c.sample_rate()),
                crate::Audio::Sound(_, r) | crate::Audio::Dolby(r) => r,
                crate::Audio::None => 48000,
            };
        }
        result.fragments.extend(out.fragments);
        state.window.consumed(segment);
        state.frontier += if segment.duration > 0.0 {
            segment.duration
        } else {
            media.target_duration.max(1.0)
        };
    }
    Ok(result)
}

async fn run_ts(
    video: &HtmlVideoElement,
    url: &str,
    http: &impl Fetch,
    partial: bool,
    decode_sound: bool,
    stop: &Cell<bool>,
    report: &mut dyn FnMut(Status),
) -> Result<(), Failure> {
    let response = http.get(url, None).await.map_err(|e| match e {
        FetchError::Temporary(e) | FetchError::Permanent(e) => Failure::Other(e),
    })?;
    if !(200..300).contains(&response.status) {
        return Err(format!("TS source answered HTTP {}", response.status).into());
    }
    let ms = MediaSource::new().map_err(js_err)?;
    let object_url = ObjectUrl(web_sys::Url::create_object_url_with_source(&ms).map_err(js_err)?);
    video.set_src(&object_url.0);
    for _ in 0..500 {
        if ms.ready_state() == MediaSourceReadyState::Open {
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    if ms.ready_state() != MediaSourceReadyState::Open {
        return Err("the browser did not open the media source".into());
    }
    let mut continuous = crate::Continuous::default().decode_sound(decode_sound);
    let mut body = response.body;
    let mut sb = None;
    let mut started = false;
    while let Some(chunk) = std::future::poll_fn(|cx| body.as_mut().poll_next(cx)).await {
        let chunk = chunk?;
        for mut out in continuous.feed(&chunk)? {
            if let Some(codec) = out.skipped_audio
                && !partial
            {
                return Err(Unsupported::Sound(codec).into());
            }
            if let Some(mut init) = out.init {
                if init.interlaced && !partial {
                    return Err(Unsupported::Interlaced.into());
                }
                if let Some(old) = sb.take() {
                    ms.remove_source_buffer(&old).map_err(js_err)?;
                }
                let buffer =
                    ms.add_source_buffer(&init.mime)
                        .map_err(|e| Unsupported::MediaType {
                            mime: init.mime.clone(),
                            why: js_err(e),
                        })?;
                append(video, &buffer, &ms, &mut init.bytes).await?;
                sb = Some(buffer);
            }
            let buffer = sb.as_ref().ok_or("no TS media buffer")?;
            for f in &mut out.fragments {
                append(video, buffer, &ms, &mut f.moof).await?;
                append(video, buffer, &ms, &mut f.mdat).await?;
            }
            if !started {
                started = true;
                let _ = video.play();
                if !stop.get() {
                    report(Status::Playing);
                }
            }
            trim(video, buffer, &ms, 5.0).await?;
            while buffered_ahead(video) > 15.0 {
                sleep(Duration::from_millis(250)).await;
            }
        }
    }
    if let Some(buffer) = sb {
        let mut out = continuous.finish()?;
        for f in &mut out.fragments {
            append(video, &buffer, &ms, &mut f.moof).await?;
            append(video, &buffer, &ms, &mut f.mdat).await?;
        }
        wait_idle(&buffer, &ms).await?;
        let _ = ms.end_of_stream();
    }
    Ok(())
}

#[cfg(feature = "vod")]
pub use movie::{play_movie, probe};

/// Movies and episodes: the `vod` feature.
#[cfg(feature = "vod")]
mod movie {
    use super::*;
    use crate::{mkv, mp4, vod};

    // ---- Movies and episodes: plain files read by byte range ----

    struct Part {
        bytes: Vec<u8>,
        /// The whole file's length.
        total: u64,
    }

    /// `len` bytes of the file from `start`.
    async fn get_range(
        http: &impl Fetch,
        url: &str,
        start: u64,
        len: u64,
        stop: &Cell<bool>,
    ) -> Result<Part, Failure> {
        with_retries(stop, || get_range_once(http, url, start, len), &mut |_| {}).await
    }

    async fn get_range_once(
        http: &impl Fetch,
        url: &str,
        start: u64,
        len: u64,
    ) -> Result<Part, Fail> {
        let host = host_of(url);
        let no_ranges = || Fail::Final(format!("{host} does not serve byte ranges"));
        let r = http.get(url, Some(start..start + len.max(1))).await?;
        if r.status != 206 {
            // A 200 would be the whole file: dropping the response stops it.
            return Err(if r.status == 200 {
                no_ranges()
            } else {
                Fail::Retry(format!("{host} answered HTTP {}", r.status))
            });
        }
        let total = r.range_total.ok_or_else(no_ranges)?;
        // A range answers with at most what was asked for; anything more is not a range.
        let limit = usize::try_from(len).unwrap_or(usize::MAX >> 1) + 1024;
        let bytes = body::read_capped(r.body, limit, len as usize)
            .await
            .map_err(|e| match e {
                Capped::TooBig => Fail::Final(format!("{host} sent more than the range asked for")),
                Capped::Failed(why) => Fail::Retry(format!("download from {host} failed: {why}")),
            })?;
        Ok(Part { bytes, total })
    }

    /// Reads what kind of movie a file is: MP4 or Matroska, its tracks and its index, from a few range
    /// requests. Fails for anything else (or a server without ranges), in which case the browser
    /// should be left to play the file itself.
    pub async fn probe(http: &impl Fetch, url: &str) -> Result<Rc<vod::Movie>, String> {
        /// The index of a movie is megabytes at the very most.
        const INDEX_LIMIT: u64 = 64 << 20;
        let stop = Cell::new(false);
        let head = get_range(http, url, 0, 256 << 10, &stop).await?;
        let total = head.total;

        if head.bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
            let mut probe = mkv::Probe::new(total);
            let mut part = head;
            let mut at = 0;
            for _ in 0..16 {
                match probe.feed(at, &part.bytes).map_err(|e| e.to_string())? {
                    mkv::Step::Done(parsed) => {
                        return Ok(Rc::new(vod::Movie::from_mkv(*parsed, total)));
                    }
                    mkv::Step::Read(start, len) => {
                        if len > INDEX_LIMIT {
                            return Err("the movie's index is too big".into());
                        }
                        part = get_range(http, url, start, len, &stop).await?;
                        at = start;
                    }
                }
            }
            return Err("could not find the movie's index".into());
        }

        let (mut part, mut at) = (head, 0);
        for _ in 0..16 {
            match mp4::find_moov(&part.bytes, at) {
                mp4::Moov::At(off, len) => {
                    let len = if len == u64::MAX { total - off } else { len };
                    if len > INDEX_LIMIT {
                        return Err("the movie's index is too big".into());
                    }
                    let inside = off >= at && off + len <= at + part.bytes.len() as u64;
                    let moov = if inside {
                        part.bytes[(off - at) as usize..][..len as usize].to_vec()
                    } else {
                        get_range(http, url, off, len, &stop).await?.bytes
                    };
                    let (_, _, head) = mp4::box_header(&moov).ok_or("the MP4 index is damaged")?;
                    let movie =
                        vod::Movie::from_mp4(&moov[head..], total).map_err(|e| e.to_string())?;
                    return Ok(Rc::new(movie));
                }
                mp4::Moov::Next(to) if to < total => {
                    part = get_range(http, url, to, 64 << 10, &stop).await?;
                    at = to;
                }
                _ => return Err("this is not an MP4 file".into()),
            }
        }
        Err("could not find the movie's index".into())
    }

    /// Plays a movie or episode that `probe` found we can play, from `start` seconds in. Seeking in
    /// the `<video>` works as usual: the player reads from the new place. Stops when dropped.
    pub fn play_movie(
        video: HtmlVideoElement,
        movie: Rc<vod::Movie>,
        url: String,
        http: impl Fetch + 'static,
        start: f64,
        report: impl FnMut(Status) + 'static,
    ) -> Player {
        let player_video = video.clone();
        let stats = Rc::new(RefCell::new(Stats::default()));
        let go_live = Rc::new(Cell::new(false));
        let variant = Rc::new(RefCell::new(None));
        let audio = Rc::new(RefCell::new(None));
        let stop = Rc::new(Cell::new(false));
        let stopped = stop.clone();
        let wake = Rc::new(RefCell::new(None));
        let waking = wake.clone();
        let mut report = report;
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(Err(e)) = cancellable(
                &stopped,
                &waking,
                run_movie(&video, &movie, &url, &http, start, &stopped, &mut report),
            )
            .await
                && !stopped.get()
            {
                report(e.into());
            }
        });
        Player {
            video: player_video,
            stats,
            go_live,
            variant,
            audio,
            stop,
            wake,
            paused_at: Cell::new(None),
        }
    }

    /// How much of the movie to hold: seconds ahead of the playhead and behind it, and how much each
    /// piece read is worth. Sound we decode is FLAC without compression (1.5 Mbit/s), and Chrome keeps
    /// only about 12 MB of sound in a source buffer: a minute of it. So pieces are sized by time, not
    /// by bytes (a low-bitrate film makes a few megabytes a whole minute).
    const MOVIE_AHEAD: f64 = 25.0;
    const MOVIE_BEHIND: f64 = 10.0;
    const PIECE_SECONDS: f64 = 5.0;
    /// The first piece is small so the picture starts quickly.
    const FIRST_CHUNK: u64 = 768 << 10;

    fn buffered_at(video: &HtmlVideoElement, t: f64) -> bool {
        let b = video.buffered();
        (0..b.length()).any(
            |i| matches!((b.start(i), b.end(i)), (Ok(s), Ok(e)) if s <= t + 0.05 && t < e - 0.05),
        )
    }

    /// Frees what is buffered far behind the playhead, and far beyond where a seek left it.
    async fn evict(
        video: &HtmlVideoElement,
        sb: &SourceBuffer,
        ms: &MediaSource,
    ) -> Result<(), String> {
        wait_idle(sb, ms).await?;
        let b = sb.buffered().map_err(js_err)?;
        let t = video.current_time();
        let ranges: Vec<(f64, f64)> = (0..b.length())
            .filter_map(|i| Some((b.start(i).ok()?, b.end(i).ok()?)))
            .collect();
        for (s, e) in ranges {
            let (from, to) = if e < t - MOVIE_BEHIND {
                (s, e)
            } else if s < t - MOVIE_BEHIND - 1.0 {
                (s, t - MOVIE_BEHIND)
            } else if s > t + 3.0 * MOVIE_AHEAD {
                (s, e)
            } else {
                continue;
            };
            let _ = sb.remove(from, to);
            wait_idle(sb, ms).await?;
        }
        Ok(())
    }

    async fn run_movie(
        video: &HtmlVideoElement,
        movie: &Rc<vod::Movie>,
        url: &str,
        http: &impl Fetch,
        start: f64,
        stop: &Cell<bool>,
        report: &mut dyn FnMut(Status),
    ) -> Result<(), Failure> {
        let mut say = |s: Status| {
            if !stop.get() {
                report(s)
            }
        };
        let mut init = movie.init().map_err(|e| e.to_string())?;

        let ms = MediaSource::new().map_err(js_err)?;
        let object_url =
            ObjectUrl(web_sys::Url::create_object_url_with_source(&ms).map_err(js_err)?);
        video.set_src(&object_url.0);
        for _ in 0..500 {
            if ms.ready_state() == MediaSourceReadyState::Open {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        if ms.ready_state() != MediaSourceReadyState::Open {
            return Err("the browser did not open the media source".into());
        }
        let sb = ms
            .add_source_buffer(&init.mime)
            .map_err(|e| Unsupported::MediaType {
                mime: init.mime.clone(),
                why: js_err(e),
            })?;
        sb.set_timestamp_offset(movie.shift());
        ms.set_duration(movie.duration);
        append(video, &sb, &ms, &mut init.bytes).await?;
        if start > 0.0 {
            video.set_current_time(start);
        }

        let piece = ((movie.bytes_per_second() * PIECE_SECONDS) as u64).clamp(256 << 10, 6 << 20);
        let mut session = movie.session(start);
        // Where the next piece will begin, in seconds of the movie.
        let mut frontier = start;
        let mut started = false;
        let mut ended = false;
        let mut first_chunk = true;
        loop {
            if stop.get() {
                return Ok(());
            }
            let t = video.current_time();
            // The playhead is somewhere nothing is buffered, and not where the reading is up to: a
            // seek. Start again from there.
            if !buffered_at(video, t) && (t - frontier).abs() > 1.0 {
                session = movie.session(t);
                frontier = t;
                ended = false;
                first_chunk = true;
            }
            if session.done() {
                if !ended {
                    wait_idle(&sb, &ms).await?;
                    let _ = ms.end_of_stream();
                    ended = true;
                }
                sleep(Duration::from_millis(250)).await;
                continue;
            }
            if buffered_ahead(video) > MOVIE_AHEAD && buffered_at(video, t) {
                sleep(Duration::from_millis(250)).await;
                continue;
            }
            let Some((offset, len)) = session.range(if first_chunk {
                piece.min(FIRST_CHUNK)
            } else {
                piece
            }) else {
                continue;
            };
            first_chunk = false;
            let part = get_range(http, url, offset, len, stop).await?;
            if stop.get() {
                return Ok(());
            }
            // A seek during the download: what was fetched belongs to the old place.
            let t = video.current_time();
            if !buffered_at(video, t) && (t - frontier).abs() > 1.0 {
                continue;
            }
            let mut fragment = session.push(&part.bytes).map_err(|e| e.to_string())?;
            evict(video, &sb, &ms).await?;
            if !fragment.is_empty() {
                append(video, &sb, &ms, &mut fragment).await?;
            }
            frontier = session.reached();
            if !started && buffered_at(video, video.current_time().max(start)) {
                started = true;
                let _ = video.play();
                say(Status::Playing);
            }
        }
    }
}
