//! Browser side: fetch HLS playlists and segments through the proxy, transmux them, and feed a
//! `<video>` element through MediaSource. Live playlists are refreshed until stopped.
//!
//! ponytail: `SourceBuffer.updating` is polled every few ms instead of awaiting events, and
//! failed downloads are retried a fixed number of times. Both are simple and good enough.

use std::{cell::Cell, rc::Rc, time::Duration};

use reqwest::Url;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use web_sys::{HtmlVideoElement, MediaSource, MediaSourceReadyState, SourceBuffer};

use crate::{
    Transmuxer,
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

/// `playlist` is the (proxied) playlist URL; `wrap` turns any upstream URL into a proxied one.
/// With `partial` a stream whose sound can't be played still plays, silently; without it that is
/// reported as `NeedsConversion` so it can be fixed instead. `decode_sound` decodes AC-3, E-AC-3 and
/// MP2 sound here; without it that sound is such a problem too.
pub fn start(
    video: HtmlVideoElement,
    playlist: Url,
    wrap: impl Fn(Url) -> Url + 'static,
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
            &wrap,
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

/// Hostname behind a proxied URL, for error messages (never the path or credentials).
fn upstream_host(proxied: &Url) -> String {
    proxied
        .query_pairs()
        .find(|(k, _)| k == "url")
        .and_then(|(_, v)| Url::parse(&v).ok())
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| "the server".into())
}

struct Fetched {
    body: Vec<u8>,
    content_type: String,
    /// Final URL after redirects, as reported by the proxy; relative playlist entries resolve against it.
    upstream: Url,
}

async fn fetch_once(
    http: &reqwest::Client,
    proxied: &Url,
    limit: usize,
) -> Result<Fetched, String> {
    let host = upstream_host(proxied);
    let r = http
        .get(proxied.clone())
        .send()
        .await
        .map_err(|e| format!("request to the proxy failed: {}", e.without_url()))?;
    let status = r.status();
    if !status.is_success() {
        // The proxy explains its own refusals; anything else is the provider talking.
        if let Some(why) = r
            .headers()
            .get("x-riptv-error")
            .and_then(|v| v.to_str().ok())
        {
            // "refused" marks a permanent no, which the retry loop below gives up on.
            let what = if status == 403 {
                "refused"
            } else {
                "could not reach"
            };
            return Err(format!("the proxy {what} {host}: {why}"));
        }
        return Err(format!("{host} answered HTTP {status}"));
    }
    let upstream = r
        .headers()
        .get("x-upstream-url")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Url::parse(s).ok())
        .ok_or("the proxy did not report the upstream URL (is it an old build?)")?;
    let content_type = r
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    // Refuse a raw stream by what the server says it is, or how long it says it is, before reading
    // any of it.
    if limit == PLAYLIST_LIMIT {
        let kind = content_type.to_ascii_lowercase();
        if kind.starts_with("video/") && !kind.contains("mpegurl") {
            return Err(raw_stream());
        }
    }
    if r.content_length().is_some_and(|n| n > limit as u64) {
        return Err(too_big(&host, limit));
    }
    // The length may be missing or wrong (chunked, or a stream dressed up as a file): count as it arrives.
    let body = body::read_capped(std::pin::pin!(r.bytes_stream()), limit)
        .await
        .map_err(|e| match e {
            Capped::TooBig => too_big(&host, limit),
            Capped::Failed(e) => format!("download from {host} failed: {}", e.without_url()),
        })?;
    Ok(Fetched {
        body,
        content_type,
        upstream,
    })
}

async fn fetch(
    http: &reqwest::Client,
    proxied: &Url,
    limit: usize,
    stop: &Cell<bool>,
) -> Result<Fetched, String> {
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            sleep(Duration::from_secs(1)).await;
        }
        if stop.get() {
            break;
        }
        match fetch_once(http, proxied, limit).await {
            Ok(f) => return Ok(f),
            // A refusal or a bad build won't get better by asking again.
            Err(e)
                if e.contains("refused")
                    || e.contains("did not report")
                    || e.ends_with(TOO_BIG)
                    || crate::needs_conversion(&e).is_some() =>
            {
                return Err(e);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Ends the error for a body past its cap: reading it again would only read it all again.
const TOO_BIG: &str = "that is a stream, not HLS";

fn too_big(host: &str, limit: usize) -> String {
    let what = if limit == PLAYLIST_LIMIT {
        "a playlist"
    } else {
        "a segment"
    };
    format!(
        "{host} sent more than {} MB where {what} was expected; {TOO_BIG}",
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
    wrap: &dyn Fn(Url) -> Url,
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
    let http = reqwest::Client::new();

    // A master playlist points at variants; pick one and follow it. `media_url` is what we refresh.
    let first = fetch(&http, &playlist, PLAYLIST_LIMIT, stop).await?;
    let (media_url, mut latest) = match hls::parse(&first.body).map_err(|e| describe(&first, e))? {
        hls::Parsed::Media(_) => (playlist, first),
        hls::Parsed::Master(variants) => {
            let v = hls::pick_variant(&variants).ok_or("the playlist lists no streams")?;
            let url = wrap(first.upstream.join(&v.uri).map_err(|e| e.to_string())?);
            let f = fetch(&http, &url, PLAYLIST_LIMIT, stop).await?;
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
            let url = wrap(latest.upstream.join(&seg.uri).map_err(|e| e.to_string())?);
            let data = fetch(&http, &url, SEGMENT_LIMIT, stop).await?;
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
        latest = fetch(&http, &media_url, PLAYLIST_LIMIT, stop).await?;
        media = parse_media(&latest)?;
    }
}
