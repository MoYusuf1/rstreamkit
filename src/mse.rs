//! Browser side: fetch HLS playlists and segments (through a [`Fetch`]), transmux them, and feed a
//! `<video>` element through MediaSource. Live playlists are refreshed until stopped.
//!
//! ponytail: `SourceBuffer.updating` is polled every few ms instead of awaiting events, and
//! failed downloads are retried a fixed number of times. Both are simple and good enough.

use std::{
    cell::Cell,
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

use crate::{
    Transmuxer, Unsupported,
    body::{self, Capped},
    hls,
};

/// Start a live stream this many segments back from the newest one.
const LIVE_BACKLOG: usize = 3;
/// Don't download more than this many seconds ahead of the playhead.
const AHEAD: f64 = 30.0;
/// Free buffered media older than this many seconds behind the playhead.
const KEEP_BEHIND: f64 = 30.0;
/// A playlist is a few kilobytes. Anything bigger is a stream that isn't a playlist at all, and is
/// refused once it passes this.
const PLAYLIST_LIMIT: usize = 2 << 20;
/// A segment is a few seconds of video, tens of megabytes at the very most.
const SEGMENT_LIMIT: usize = 64 << 20;

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Status {
    Playing,
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
    stop: Rc<Cell<bool>>,
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.set(true);
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
    let stop = Rc::new(Cell::new(false));
    let stopped = stop.clone();
    let mut report = report;
    wasm_bindgen_futures::spawn_local(async move {
        match run(
            &video,
            playlist,
            &http,
            partial,
            decode_sound,
            &stopped,
            &mut report,
        )
        .await
        {
            Ok(()) if !stopped.get() => report(Status::Ended),
            Err(e) if !stopped.get() => report(e.into()),
            _ => {}
        }
    });
    Player { stop }
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
    let p = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(
                &resolve,
                d.as_millis() as i32,
            );
        }
    });
    let _ = JsFuture::from(p).await;
}

/// How the player reaches the network. The app supplies it, so rstreamkit knows nothing about proxies,
/// credentials or headers: [`Direct`] is the plain case, and an app that has to go through a proxy
/// implements this for it. Dropping the future or the [`Response::body`] cancels the request.
// Not `Send` on purpose: this only ever runs on the browser's one thread.
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
        let init = RequestInit::new();
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
                reader: response.body().map(|b| {
                    b.get_reader()
                        .unchecked_into::<ReadableStreamDefaultReader>()
                }),
                pending: None,
            }),
        })
    }
}

/// The body of a `fetch` response, read chunk by chunk. Dropping it cancels the download.
struct Chunks {
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

/// Runs `once` up to three times, a second apart, until it works or fails for good.
async fn with_retries<T, F: Future<Output = Result<T, Fail>>>(
    stop: &Cell<bool>,
    mut once: impl FnMut() -> F,
) -> Result<T, Failure> {
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            sleep(Duration::from_secs(1)).await;
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
    let host = host_of(url);
    let r = http.get(url, None).await?;
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
) -> Result<Fetched, Failure> {
    with_retries(stop, || fetch_once(http, url, limit)).await
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

/// Resolves when the buffer has finished what it was doing: the browser says so (`updateend`, which
/// follows success, error and abort alike), so nothing has to ask every few milliseconds.
async fn wait_idle(sb: &SourceBuffer) {
    if !sb.updating() {
        return;
    }
    let done = js_sys::Promise::new(&mut |resolve, _| {
        let resolve_once = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::UNDEFINED);
        });
        sb.set_onupdateend(Some(resolve_once.unchecked_ref()));
    });
    let _ = JsFuture::from(done).await;
}

/// Drops buffered media far behind the playhead so a long live session doesn't fill memory.
async fn trim(video: &HtmlVideoElement, sb: &SourceBuffer, keep: f64) {
    wait_idle(sb).await;
    let Ok(b) = sb.buffered() else { return };
    if b.length() == 0 {
        return;
    }
    let Ok(start) = b.start(0) else { return };
    let cut = video.current_time() - keep;
    if cut - start > 1.0 {
        let _ = sb.remove(start, cut);
        wait_idle(sb).await;
    }
}

/// Hands `bytes` to the browser, which copies what it needs before returning: they are lent as a
/// view of wasm memory, not copied into a JS buffer of ours first.
async fn append(
    video: &HtmlVideoElement,
    sb: &SourceBuffer,
    bytes: &mut [u8],
) -> Result<(), String> {
    wait_idle(sb).await;
    if let Err(e) = sb.append_buffer_with_u8_array(bytes) {
        // Out of room: free everything old and try once more.
        trim(video, sb, 5.0).await;
        sb.append_buffer_with_u8_array(bytes)
            .map_err(|_| js_err(e))?;
    }
    wait_idle(sb).await;
    Ok(())
}

struct ObjectUrl(String);

impl Drop for ObjectUrl {
    fn drop(&mut self) {
        let _ = web_sys::Url::revoke_object_url(&self.0);
    }
}

async fn run(
    video: &HtmlVideoElement,
    playlist: String,
    http: &impl Fetch,
    partial: bool,
    decode_sound: bool,
    stop: &Cell<bool>,
    report: &mut dyn FnMut(Status),
) -> Result<(), Failure> {
    // Never report after the viewer left: the UI state behind the callback may be gone.
    let mut say = |s: Status| {
        if !stop.get() {
            report(s)
        }
    };

    // A master playlist points at variants; pick one and follow it. `media_url` is what we refresh.
    let first = fetch(http, &playlist, PLAYLIST_LIMIT, stop).await?;
    let (media_url, mut latest) = match hls::parse(&first.body).map_err(|e| describe(&first, e))? {
        hls::Parsed::Media(_) => (playlist, first),
        hls::Parsed::Master(variants) => {
            let v = hls::pick_variant(&variants).ok_or("the playlist lists no streams")?;
            let url = resolve(&first.url, &v.uri)?;
            let f = fetch(http, &url, PLAYLIST_LIMIT, stop).await?;
            (url, f)
        }
    };
    let mut media = parse_media(&latest)?;

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

    let mut tx = Transmuxer::default().decode_sound(decode_sound);
    let mut sb: Option<SourceBuffer> = None;
    let mut next_seq: Option<u64> = None;
    let mut started = false;
    let mut warned_audio = false;

    loop {
        if next_seq.is_none_or(|n| media.segments.last().is_some_and(|l| l.seq + 1 < n)) {
            // First pass, or the server restarted its numbering: (re)start near the live edge.
            let from = if media.ended {
                0
            } else {
                media.segments.len().saturating_sub(LIVE_BACKLOG)
            };
            next_seq = media.segments.get(from).map(|s| s.seq);
        }

        let from_seq = next_seq.unwrap_or(0);
        for seg in media.segments.iter().filter(|s| s.seq >= from_seq) {
            while buffered_ahead(video) > AHEAD && !stop.get() {
                sleep(Duration::from_millis(500)).await;
            }
            if stop.get() {
                return Ok(());
            }
            let url = resolve(&latest.url, &seg.uri)?;
            let data = fetch(http, &url, SEGMENT_LIMIT, stop).await?;
            if stop.get() {
                return Ok(());
            }
            let mut out = tx.push_owned(data.body)?;
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
            if let Some(mut init) = out.init {
                if init.interlaced && !partial {
                    return Err(Unsupported::Interlaced.into());
                }
                let buffer =
                    ms.add_source_buffer(&init.mime)
                        .map_err(|e| Unsupported::MediaType {
                            mime: init.mime.clone(),
                            why: js_err(e),
                        })?;
                append(video, &buffer, &mut init.bytes).await?;
                sb = Some(buffer);
            }
            let buffer = sb.as_ref().ok_or("no media buffer")?;
            if !out.fragment.is_empty() {
                append(video, buffer, &mut out.fragment).await?;
            }
            next_seq = Some(seg.seq + 1);

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
            trim(video, buffer, KEEP_BEHIND).await;
        }

        if media.ended {
            while buffered_ahead(video) > 0.5 && !stop.get() {
                sleep(Duration::from_millis(250)).await;
            }
            if let Some(b) = &sb {
                wait_idle(b).await;
            }
            let _ = ms.end_of_stream();
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
        latest = fetch(http, &media_url, PLAYLIST_LIMIT, stop).await?;
        media = parse_media(&latest)?;
    }
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
        with_retries(stop, || get_range_once(http, url, start, len)).await
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
        let stop = Rc::new(Cell::new(false));
        let stopped = stop.clone();
        let mut report = report;
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) =
                run_movie(&video, &movie, &url, &http, start, &stopped, &mut report).await
                && !stopped.get()
            {
                report(e.into());
            }
        });
        Player { stop }
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
    async fn evict(video: &HtmlVideoElement, sb: &SourceBuffer) {
        wait_idle(sb).await;
        let Ok(b) = sb.buffered() else { return };
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
            wait_idle(sb).await;
        }
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
        append(video, &sb, &mut init.bytes).await?;
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
                    wait_idle(&sb).await;
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
            evict(video, &sb).await;
            if !fragment.is_empty() {
                append(video, &sb, &mut fragment).await?;
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
