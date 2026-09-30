//! Browser side: fetch HLS playlists and segments (through a [`Fetch`]), transmux them, and feed a
//! `<video>` element through MediaSource. Live playlists are refreshed until stopped.
//!
//! ponytail: `SourceBuffer.updating` is polled every few ms instead of awaiting events, and
//! failed downloads are retried a fixed number of times. Both are simple and good enough.

use std::{cell::Cell, ops::Range, pin::Pin, rc::Rc, time::Duration};

use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::Url;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use web_sys::{HtmlVideoElement, MediaSource, MediaSourceReadyState, SourceBuffer};

use crate::{
    Transmuxer,
    body::{self, Capped},
    hls, mkv, mp4, vod,
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
pub enum Status {
    Playing,
    /// Something the viewer should know but playback continues (e.g. audio codec unsupported).
    Note(String),
    /// This browser can't play the stream as it is, but a converted copy might play: HEVC video,
    /// sound it can't decode, or a raw stream. The reason is for the viewer.
    NeedsConversion(String),
    Ended,
    Failed(String),
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

/// `playlist` is the playlist's real URL, and `http` gets every URL the player needs ([`Direct`]
/// goes straight to the server; an app behind a proxy brings its own [`Fetch`]).
/// With `partial` a stream whose sound can't be played still plays, silently; without it that is
/// reported as `NeedsConversion` so it can be fixed instead. `decode_sound` decodes AC-3, E-AC-3 and
/// MP2 sound here; without it that sound is such a problem too.
pub fn start(
    video: HtmlVideoElement,
    playlist: Url,
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
            Err(e) if !stopped.get() => report(match crate::needs_conversion(&e) {
                Some(reason) => Status::NeedsConversion(reason.to_string()),
                None => Status::Failed(e),
            }),
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

/// How the player reaches the network. The app supplies it, so rffmpeg knows nothing about proxies,
/// credentials or headers: [`Direct`] is the plain case, and an app that has to go through a proxy
/// implements this for it. Dropping the future or the [`Response::body`] cancels the request.
// Not `Send` on purpose: this only ever runs on the browser's one thread.
#[allow(async_fn_in_trait)]
pub trait Fetch {
    /// GETs `url`, which is always the real address; with a `range` (never empty), only those bytes
    /// of it, as an HTTP `Range` request. `Ok` means a response came back, whatever its status;
    /// `Err` is for when it didn't, or when the app itself refuses or can't make the request.
    async fn get(&self, url: &Url, range: Option<Range<u64>>) -> Result<Response, FetchError>;
}

pub struct Response {
    pub status: u16,
    /// Where the request ended up after redirects: relative playlist entries resolve against it.
    pub url: Url,
    pub content_type: String,
    pub content_length: Option<u64>,
    /// For a range request the server answered with 206: the whole file's length, from `Content-Range`.
    pub range_total: Option<u64>,
    pub body: Body,
}

/// A response body, chunk by chunk (the player stops reading one that grows past its limit).
pub type Body = Pin<Box<dyn Stream<Item = Result<Vec<u8>, String>>>>;

#[derive(Debug, Clone, PartialEq)]
pub enum FetchError {
    /// Might go away: a network error, a server that is down.
    Temporary(String),
    /// Won't get better by asking again: a refusal, or a proxy that doesn't answer as expected.
    Permanent(String),
}

/// Fetches with the browser's own `fetch`, straight from the server, which therefore has to allow it
/// (CORS). Good for a server you run yourself; behind a proxy, implement [`Fetch`] for that instead.
#[derive(Default, Clone)]
pub struct Direct(reqwest::Client);

impl Fetch for Direct {
    async fn get(&self, url: &Url, range: Option<Range<u64>>) -> Result<Response, FetchError> {
        let mut request = self.0.get(url.clone());
        if let Some(r) = range {
            request = request.header("Range", format!("bytes={}-{}", r.start, r.end - 1));
        }
        let r = request.send().await.map_err(|e| {
            FetchError::Temporary(format!(
                "request to {} failed: {}",
                url.host_str().unwrap_or("the server"),
                e.without_url()
            ))
        })?;
        Ok(r.into())
    }
}

/// For a [`Fetch`] that makes its own `reqwest` request (say, through a proxy) and only has to
/// adjust what comes back, such as the final URL.
impl From<reqwest::Response> for Response {
    fn from(r: reqwest::Response) -> Self {
        let content_type = r
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        // "bytes 0-262143/1234567"
        let range_total = r
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|t| t.parse().ok());
        Response {
            status: r.status().as_u16(),
            url: r.url().clone(),
            content_type,
            content_length: r.content_length(),
            range_total,
            body: Box::pin(r.bytes_stream().map(|chunk| {
                chunk
                    .map(|b| b.to_vec())
                    .map_err(|e| e.without_url().to_string())
            })),
        }
    }
}

struct Fetched {
    body: Vec<u8>,
    content_type: String,
    /// Final URL after redirects; relative playlist entries resolve against it.
    url: Url,
}

/// Why one download failed, and whether asking again could help.
enum Fail {
    Retry(String),
    Final(String),
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
) -> Result<T, String> {
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
            Err(Fail::Final(e)) => return Err(e),
            Err(Fail::Retry(e)) => last = e,
        }
    }
    Err(last)
}

async fn fetch_once(http: &impl Fetch, url: &Url, limit: usize) -> Result<Fetched, Fail> {
    let host = url.host_str().unwrap_or("the server");
    let r = http.get(url, None).await?;
    if !(200..300).contains(&r.status) {
        return Err(Fail::Retry(format!("{host} answered HTTP {}", r.status)));
    }
    // Refuse a raw stream by what the server says it is, or how long it says it is, before reading
    // any of it.
    if limit == PLAYLIST_LIMIT {
        let kind = r.content_type.to_ascii_lowercase();
        if kind.starts_with("video/") && !kind.contains("mpegurl") {
            return Err(Fail::Final(raw_stream()));
        }
    }
    if r.content_length.is_some_and(|n| n > limit as u64) {
        return Err(Fail::Final(too_big(host, limit)));
    }
    // The length may be missing or wrong (chunked, or a stream dressed up as a file): count as it arrives.
    let body = body::read_capped(r.body, limit)
        .await
        .map_err(|e| match e {
            // Reading it again would only read it all again.
            Capped::TooBig => Fail::Final(too_big(host, limit)),
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
    url: &Url,
    limit: usize,
    stop: &Cell<bool>,
) -> Result<Fetched, String> {
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

/// A raw stream where a playlist should be: something a converter can play.
fn raw_stream() -> String {
    format!("{}{}", crate::CONVERT, hls::RAW_STREAM)
}

/// When a "playlist" isn't one, say what it was (`hls::describe_non_playlist`).
fn describe(f: &Fetched, why: impl ToString) -> String {
    let why = why.to_string();
    if why.contains("EXTM3U") {
        if hls::looks_like_ts(&f.body) {
            raw_stream()
        } else {
            hls::describe_non_playlist(&f.body, &f.content_type)
        }
    } else {
        why
    }
}

fn parse_media(f: &Fetched) -> Result<hls::Media, String> {
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

async fn wait_idle(sb: &SourceBuffer) {
    while sb.updating() {
        sleep(Duration::from_millis(5)).await;
    }
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

async fn append(video: &HtmlVideoElement, sb: &SourceBuffer, bytes: &[u8]) -> Result<(), String> {
    wait_idle(sb).await;
    // Copy into a JS-side ArrayBuffer of exactly the right size (the wasm heap view overload is flaky).
    let buf = js_sys::Uint8Array::from(bytes).buffer();
    if let Err(e) = sb.append_buffer_with_array_buffer(&buf) {
        // Out of room: free everything old and try once more.
        trim(video, sb, 5.0).await;
        sb.append_buffer_with_array_buffer(&buf)
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
    playlist: Url,
    http: &impl Fetch,
    partial: bool,
    decode_sound: bool,
    stop: &Cell<bool>,
    report: &mut dyn FnMut(Status),
) -> Result<(), String> {
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
            let url = first.url.join(&v.uri).map_err(|e| e.to_string())?;
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
            let url = latest.url.join(&seg.uri).map_err(|e| e.to_string())?;
            let data = fetch(http, &url, SEGMENT_LIMIT, stop).await?;
            if stop.get() {
                return Ok(());
            }
            let out = tx.push(&data.body).map_err(|e| e.to_string())?;
            if let Some(codec) = &out.skipped_audio {
                if !partial {
                    return Err(format!(
                        "{}{codec} sound can't be played by this browser",
                        crate::CONVERT
                    ));
                }
                if !warned_audio {
                    warned_audio = true;
                    say(Status::Note(format!(
                        "{codec} audio can't be played in the browser; playing without sound"
                    )));
                }
            }
            if let Some(init) = out.init {
                if init.interlaced && !partial {
                    return Err(format!("{}interlaced video", crate::CONVERT));
                }
                let buffer = ms.add_source_buffer(&init.mime).map_err(|e| {
                    format!(
                        "{}this browser can't play {}: {}",
                        crate::CONVERT,
                        init.mime,
                        js_err(e)
                    )
                })?;
                append(video, &buffer, &init.bytes).await?;
                sb = Some(buffer);
            }
            let buffer = sb.as_ref().ok_or("no media buffer")?;
            if !out.fragment.is_empty() {
                append(video, buffer, &out.fragment).await?;
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

// ---- Movies and episodes: plain files read by byte range ----

/// Ask the browser whether it can play a MIME type with codecs, e.g. `video/mp4; codecs="avc1.64001f"`.
pub fn can_play(mime: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.create_element("video").ok())
        .and_then(|e| wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlMediaElement>(e).ok())
        .is_some_and(|v| !v.can_play_type(mime).is_empty())
}

struct Part {
    bytes: Vec<u8>,
    /// The whole file's length.
    total: u64,
}

/// `len` bytes of the file from `start`.
async fn get_range(
    http: &impl Fetch,
    url: &Url,
    start: u64,
    len: u64,
    stop: &Cell<bool>,
) -> Result<Part, String> {
    with_retries(stop, || get_range_once(http, url, start, len)).await
}

async fn get_range_once(http: &impl Fetch, url: &Url, start: u64, len: u64) -> Result<Part, Fail> {
    let host = url.host_str().unwrap_or("the server");
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
    let bytes = body::read_capped(r.body, limit)
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
pub async fn probe(http: &impl Fetch, url: &Url) -> Result<Rc<vod::Movie>, String> {
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
    url: Url,
    http: impl Fetch + 'static,
    start: f64,
    report: impl FnMut(Status) + 'static,
) -> Player {
    let stop = Rc::new(Cell::new(false));
    let stopped = stop.clone();
    let mut report = report;
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = run_movie(&video, &movie, &url, &http, start, &stopped, &mut report).await
            && !stopped.get()
        {
            report(match crate::needs_conversion(&e) {
                Some(reason) => Status::NeedsConversion(reason.to_string()),
                None => Status::Failed(e),
            });
        }
    });
    Player { stop }
}

/// Seconds buffered ahead of the playhead to keep, and behind it.
const MOVIE_AHEAD: f64 = 40.0;
/// The first piece is small so the picture starts quickly; the rest are this big.
const FIRST_CHUNK: u64 = 768 << 10;
const CHUNK: u64 = 3 << 20;

fn buffered_at(video: &HtmlVideoElement, t: f64) -> bool {
    let b = video.buffered();
    (0..b.length())
        .any(|i| matches!((b.start(i), b.end(i)), (Ok(s), Ok(e)) if s <= t + 0.05 && t < e - 0.05))
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
        let (from, to) = if e < t - KEEP_BEHIND {
            (s, e)
        } else if s < t - KEEP_BEHIND - 1.0 {
            (s, t - KEEP_BEHIND)
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
    url: &Url,
    http: &impl Fetch,
    start: f64,
    stop: &Cell<bool>,
    report: &mut dyn FnMut(Status),
) -> Result<(), String> {
    let mut say = |s: Status| {
        if !stop.get() {
            report(s)
        }
    };
    let init = movie.init().map_err(|e| e.to_string())?;

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
    let sb = ms.add_source_buffer(&init.mime).map_err(|e| {
        format!(
            "{}this browser can't play {}: {}",
            crate::CONVERT,
            init.mime,
            js_err(e)
        )
    })?;
    sb.set_timestamp_offset(movie.shift());
    ms.set_duration(movie.duration);
    append(video, &sb, &init.bytes).await?;
    if start > 0.0 {
        video.set_current_time(start);
    }

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
        let Some((offset, len)) = session.range(if first_chunk { FIRST_CHUNK } else { CHUNK })
        else {
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
        let fragment = session.push(&part.bytes).map_err(|e| e.to_string())?;
        evict(video, &sb).await;
        if !fragment.is_empty() {
            append(video, &sb, &fragment).await?;
        }
        frontier = session.reached();
        if !started && buffered_at(video, video.current_time().max(start)) {
            started = true;
            let _ = video.play();
            say(Status::Playing);
        }
    }
}
