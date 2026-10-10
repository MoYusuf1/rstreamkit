//! Script providers through the same selection/refresh controller used by playback.
use rstreamkit::{
    Transmuxer, body,
    hls::{self, Media, Parsed},
    net::{self, Clock, Fetch, FetchError, PlaybackBuffer, Response, Sink},
    session::{Controls, PlaylistSource, Preparation, Session},
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    future::Future,
    ops::Range,
    pin::Pin,
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
        Poll::Pending => panic!("scripted host must complete with fake time"),
    }
}
#[derive(Default)]
struct State {
    now: Cell<f64>,
    position: Cell<f64>,
    stop: Cell<bool>,
    jump: Cell<bool>,
    stop_at: Cell<Option<f64>>,
    jump_at: Cell<Option<f64>>,
    sleeps: RefCell<Vec<f64>>,
}
impl State {
    fn controls(&self) -> Controls<'_> {
        Controls::new(&self.stop, &self.jump)
    }
}
struct Timer(Rc<State>);
impl Clock for Timer {
    async fn sleep(&self, duration: Duration) {
        let seconds = duration.as_secs_f64();
        self.0.sleeps.borrow_mut().push(seconds);
        self.0.now.set(self.0.now.get() + seconds);
        self.0.position.set(self.0.position.get() + seconds);
        if self
            .0
            .stop_at
            .get()
            .is_some_and(|at| self.0.now.get() >= at)
        {
            self.0.stop.set(true);
        }
        if self
            .0
            .jump_at
            .get()
            .is_some_and(|at| self.0.now.get() >= at)
        {
            self.0.jump.set(true);
            self.0.jump_at.set(None);
        }
    }
}
struct Buffer {
    state: Rc<State>,
    end: f64,
    start: f64,
    removals: usize,
}
impl Buffer {
    fn new(state: &Rc<State>) -> Self {
        Self {
            state: state.clone(),
            end: 0.0,
            start: 0.0,
            removals: 0,
        }
    }
}
impl PlaybackBuffer for Buffer {
    fn position(&self) -> f64 {
        self.state.position.get()
    }
    fn ahead(&self) -> f64 {
        (self.end - self.position()).max(0.0)
    }
    fn playback_rate(&self) -> f64 {
        1.0
    }
    fn cross_gap(&mut self, _: f64) {}
    async fn oldest(&mut self) -> Result<Option<f64>, String> {
        Ok(Some(self.start))
    }
    async fn remove(&mut self, range: Range<f64>) -> Result<(), String> {
        self.start = range.end;
        self.removals += 1;
        Ok(())
    }
}
struct Chunks {
    once: Option<Result<Vec<u8>, String>>,
    endless: bool,
    polls: Rc<Cell<usize>>,
}
impl futures_core::Stream for Chunks {
    type Item = Result<Vec<u8>, String>;
    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.polls.set(self.polls.get() + 1);
        assert!(self.polls.get() < 64, "body cap must stop the provider");
        Poll::Ready(if self.endless {
            Some(Ok(vec![0; 256]))
        } else {
            self.once.take()
        })
    }
}
struct Provider {
    state: Rc<State>,
    snapshots: RefCell<VecDeque<String>>,
    requests: RefCell<Vec<(String, f64)>>,
    outage_until: f64,
    drop_body: Cell<bool>,
    segment_delay: f64,
    missing: Option<&'static str>,
    endless: Option<&'static str>,
    polls: Rc<Cell<usize>>,
    reports: Vec<bool>,
}
fn text(seq: u64, names: &[&str], ended: bool) -> String {
    format!(
        "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:{seq}\n{}{}",
        names
            .iter()
            .map(|n| format!("#EXTINF:4,\n{n}\n"))
            .collect::<String>(),
        if ended { "#EXT-X-ENDLIST\n" } else { "" }
    )
}
fn media(seq: u64, names: &[&str], ended: bool) -> Media {
    let Parsed::Media(media) = hls::parse(text(seq, names, ended).as_bytes()).unwrap() else {
        panic!()
    };
    media
}
impl Provider {
    fn new(state: &Rc<State>, snapshots: Vec<String>) -> Self {
        Self {
            state: state.clone(),
            snapshots: RefCell::new(snapshots.into()),
            requests: RefCell::default(),
            outage_until: 0.0,
            drop_body: Cell::new(false),
            segment_delay: 0.0,
            missing: None,
            endless: None,
            polls: Rc::new(Cell::new(0)),
            reports: Vec::new(),
        }
    }
    async fn playlist_once(&self) -> Result<Media, FetchError> {
        let response = self.get("playlist", None).await?;
        if !(200..300).contains(&response.status) {
            return Err(if response.status == 404 {
                FetchError::Permanent("HTTP 404".into())
            } else {
                FetchError::Temporary(format!("HTTP {}", response.status))
            });
        }
        let bytes = body::read_capped(response.body, 1024, 0)
            .await
            .map_err(|e| match e {
                body::Capped::TooBig => {
                    FetchError::Permanent("playlist body exceeded limit".into())
                }
                body::Capped::Failed(why) => FetchError::Temporary(why),
                other => FetchError::Permanent(format!("body failure: {other:?}")),
            })?;
        match hls::parse(&bytes).map_err(|e| FetchError::Permanent(e.to_string()))? {
            Parsed::Media(media) => Ok(media),
            _ => Err(FetchError::Permanent("expected media playlist".into())),
        }
    }
}
impl Fetch for Provider {
    async fn get(&self, url: &str, _: Option<Range<u64>>) -> Result<Response, FetchError> {
        self.requests
            .borrow_mut()
            .push((url.into(), self.state.now.get()));
        if self.state.now.get() < self.outage_until {
            return Err(FetchError::Temporary("offline".into()));
        }
        if url != "playlist" && self.segment_delay > 0.0 {
            Timer(self.state.clone())
                .sleep(Duration::from_secs_f64(self.segment_delay))
                .await;
        }
        let bytes = if url == "playlist" {
            let mut snapshots = self.snapshots.borrow_mut();
            if snapshots.len() > 1 {
                snapshots.pop_front().unwrap().into_bytes()
            } else {
                snapshots.front().unwrap().as_bytes().to_vec()
            }
        } else {
            include_bytes!("fixtures/bbb_480p.ts").to_vec()
        };
        self.polls.set(0);
        Ok(Response {
            status: if self.missing == Some(url) { 404 } else { 200 },
            url: url.into(),
            content_type: String::new(),
            content_length: None,
            range_total: None,
            body: Box::pin(Chunks {
                once: Some(if self.drop_body.replace(false) {
                    Err("connection dropped".into())
                } else {
                    Ok(bytes)
                }),
                endless: self.endless == Some(url),
                polls: self.polls.clone(),
            }),
        })
    }
}
impl PlaylistSource for Provider {
    type Error = String;
    async fn refresh(&mut self) -> Result<Media, String> {
        let clock = Timer(self.state.clone());
        net::retry(
            &clock,
            || self.state.stop.get(),
            || self.playlist_once(),
            |e| matches!(e, FetchError::Temporary(_)),
            |_| {},
        )
        .await
        .map_err(|e| format!("{e:?}"))
    }
    fn waiting(&mut self, stalled: bool) {
        self.reports.push(stalled);
    }
}
#[derive(Default)]
struct Output {
    appends: usize,
    bytes: usize,
}
impl Sink for Output {
    async fn append(&mut self, bytes: &mut [u8]) -> Result<(), String> {
        self.appends += 1;
        self.bytes += bytes.len();
        Ok(())
    }
}
async fn package_next(
    session: &mut Session,
    media: &Media,
    host: &Provider,
    tx: &mut Transmuxer,
    output: &mut Output,
) -> Result<u64, FetchError> {
    let segment = session.next(media).unwrap();
    if segment.discontinuity || session.discontinuous(segment) {
        tx.discontinuity();
    }
    let input = net::download_segment(host, &segment.uri, 8 << 20, None).await?;
    let mut packaged = tx.finish(input).unwrap();
    let moof = &packaged.fragments[0].moof;
    let tfdt = moof.windows(4).position(|w| w == b"tfdt").unwrap();
    let time = u64::from_be_bytes(moof[tfdt + 8..tfdt + 16].try_into().unwrap());
    net::append_fragments(&mut packaged.fragments, output)
        .await
        .unwrap();
    session.consumed(segment);
    Ok(time)
}

#[test]
fn sliding_windows_with_and_without_sequence_updates_feed_real_output() {
    for seq in [0, 1] {
        let state = Rc::new(State::default());
        let clock = Timer(state.clone());
        let mut provider = Provider::new(&state, vec![text(seq, &["b", "c", "d"], false)]);
        let mut current = media(0, &["a", "b", "c"], false);
        let mut session = Session::default();
        let mut tx = Transmuxer::default();
        let mut out = Output::default();
        for _ in 0..3 {
            run(package_next(
                &mut session,
                &current,
                &provider,
                &mut tx,
                &mut out,
            ))
            .unwrap();
        }
        run(session.wait_and_refresh(
            &clock,
            &mut provider,
            &mut current,
            state.controls(),
            || state.now.get(),
        ))
        .unwrap();
        assert_eq!(session.next(&current).unwrap().uri, "d");
        run(package_next(
            &mut session,
            &current,
            &provider,
            &mut tx,
            &mut out,
        ))
        .unwrap();
        assert_eq!(out.appends, 16);
        assert!(out.bytes > 0);
    }
}

#[test]
fn numbering_restart_and_unmarked_pts_reset_stay_monotonic() {
    let state = Rc::new(State::default());
    let clock = Timer(state.clone());
    let mut provider = Provider::new(&state, vec![text(0, &["new"], false)]);
    let mut current = media(100, &["first", "second"], false);
    let mut session = Session::default();
    let mut tx = Transmuxer::default();
    let mut out = Output::default();
    let first = run(package_next(
        &mut session,
        &current,
        &provider,
        &mut tx,
        &mut out,
    ))
    .unwrap();
    let second = run(package_next(
        &mut session,
        &current,
        &provider,
        &mut tx,
        &mut out,
    ))
    .unwrap();
    assert!(second > first, "untagged repeated source PTS must advance");
    run(session.wait_and_refresh(
        &clock,
        &mut provider,
        &mut current,
        state.controls(),
        || state.now.get(),
    ))
    .unwrap();
    assert!(session.discontinuous(session.next(&current).unwrap()));
    let restarted = run(package_next(
        &mut session,
        &current,
        &provider,
        &mut tx,
        &mut out,
    ))
    .unwrap();
    assert!(restarted > second);
}

#[test]
fn long_pause_refreshes_before_requesting_an_expired_segment() {
    let state = Rc::new(State::default());
    let clock = Timer(state.clone());
    let mut provider = Provider::new(&state, vec![text(200, &["fresh"], false)]);
    let mut current = media(100, &["expired"], false);
    let mut session = Session::default();
    let mut buffer = Buffer::new(&state);
    buffer.end = 40.0;
    state.now.set(60.0);
    assert_eq!(
        run(session.prepare(
            &clock,
            &mut buffer,
            &mut provider,
            &mut current,
            0.0,
            state.controls()
        ))
        .unwrap(),
        Preparation::Ready {
            refreshed: true,
            jump: false
        }
    );
    assert_eq!(session.next(&current).unwrap().uri, "fresh");
    assert!(buffer.removals > 0);
    assert_eq!(provider.requests.borrow().len(), 1);
}

#[test]
fn go_live_during_poll_wait_refreshes_once_and_resets_the_anchor() {
    let state = Rc::new(State::default());
    state.jump_at.set(Some(1.0));
    let clock = Timer(state.clone());
    let mut provider = Provider::new(&state, vec![text(200, &["x", "y", "z", "new"], false)]);
    let mut current = media(100, &["old"], false);
    let mut session = Session::default();
    session.consumed(&current.segments[0]);
    assert_eq!(
        run(session.wait_and_refresh(
            &clock,
            &mut provider,
            &mut current,
            state.controls(),
            || state.now.get()
        ))
        .unwrap(),
        Preparation::Ready {
            refreshed: true,
            jump: true
        }
    );
    run(session.prepare(
        &clock,
        &mut Buffer::new(&state),
        &mut provider,
        &mut current,
        0.0,
        state.controls(),
    ))
    .unwrap();
    assert_eq!(provider.requests.borrow().len(), 1);
    assert_eq!(session.next(&current).unwrap().uri, "y");
}

#[test]
fn ten_second_playlist_outage_recovers_through_shared_retry() {
    let state = Rc::new(State::default());
    state.jump.set(true);
    let clock = Timer(state.clone());
    let mut provider = Provider::new(&state, vec![text(200, &["new"], false)]);
    provider.outage_until = 10.0;
    let mut current = media(100, &["old"], false);
    let mut session = Session::default();
    run(session.prepare(
        &clock,
        &mut Buffer::new(&state),
        &mut provider,
        &mut current,
        0.0,
        state.controls(),
    ))
    .unwrap();
    assert_eq!(
        provider
            .requests
            .borrow()
            .iter()
            .map(|r| r.1)
            .collect::<Vec<_>>(),
        [0.0, 1.0, 3.0, 7.0, 15.0]
    );
    assert_eq!(session.next(&current).unwrap().uri, "new");
}

#[test]
fn missing_segment_does_not_move_the_cursor_until_host_skips_it() {
    let state = Rc::new(State::default());
    let mut provider = Provider::new(&state, vec![]);
    provider.missing = Some("missing");
    let current = media(0, &["missing", "good"], false);
    let mut session = Session::default();
    let mut tx = Transmuxer::default();
    let mut out = Output::default();
    assert!(matches!(
        run(package_next(
            &mut session,
            &current,
            &provider,
            &mut tx,
            &mut out
        )),
        Err(FetchError::Permanent(_))
    ));
    assert_eq!(session.next(&current).unwrap().uri, "missing");
    assert_eq!(out.appends, 0);
    session.consumed(session.next(&current).unwrap());
    run(package_next(
        &mut session,
        &current,
        &provider,
        &mut tx,
        &mut out,
    ))
    .unwrap();
    assert!(session.next(&current).is_none());
}

#[test]
fn endless_playlist_stops_at_the_body_cap_without_changing_current_media() {
    let state = Rc::new(State::default());
    let clock = Timer(state.clone());
    let mut provider = Provider::new(&state, vec![text(0, &["bad"], false)]);
    provider.endless = Some("playlist");
    let mut current = media(0, &["good"], false);
    let mut session = Session::default();
    assert!(
        run(session.wait_and_refresh(
            &clock,
            &mut provider,
            &mut current,
            state.controls(),
            || state.now.get()
        ))
        .is_err()
    );
    assert_eq!(provider.polls.get(), 5);
    assert_eq!(provider.requests.borrow().len(), 1);
    assert_eq!(session.next(&current).unwrap().uri, "good");
}

#[test]
fn finite_backpressure_keeps_all_segments_and_never_refreshes() {
    let state = Rc::new(State::default());
    let clock = Timer(state.clone());
    let mut provider = Provider::new(&state, vec![]);
    let mut current = media(0, &["a", "b", "c"], true);
    let mut session = Session::default();
    session.consumed(&current.segments[0]);
    let mut buffer = Buffer::new(&state);
    buffer.end = 40.0;
    run(session.prepare(
        &clock,
        &mut buffer,
        &mut provider,
        &mut current,
        0.0,
        state.controls(),
    ))
    .unwrap();
    assert_eq!(session.next(&current).unwrap().uri, "b");
    assert!(!session.finished(&current));
    run(session.wait_and_refresh(
        &clock,
        &mut provider,
        &mut current,
        state.controls(),
        || state.now.get(),
    ))
    .unwrap();
    assert!(provider.requests.borrow().is_empty());
}

#[test]
fn stop_during_poll_wait_makes_no_request_or_callback() {
    let state = Rc::new(State::default());
    state.stop_at.set(Some(1.0));
    let clock = Timer(state.clone());
    let mut provider = Provider::new(&state, vec![]);
    let mut current = media(0, &["a"], false);
    assert_eq!(
        run(Session::default().wait_and_refresh(
            &clock,
            &mut provider,
            &mut current,
            state.controls(),
            || state.now.get()
        ))
        .unwrap(),
        Preparation::Cancelled
    );
    assert!(provider.requests.borrow().is_empty());
    assert!(provider.reports.is_empty());
}

#[test]
fn frozen_playlist_reports_stall_after_three_target_durations() {
    let state = Rc::new(State::default());
    let clock = Timer(state.clone());
    let mut provider = Provider::new(&state, vec![text(0, &["a"], false)]);
    let mut current = media(0, &["a"], false);
    let mut session = Session::default();
    session.consumed(&current.segments[0]);
    for _ in 0..7 {
        run(session.wait_and_refresh(
            &clock,
            &mut provider,
            &mut current,
            state.controls(),
            || state.now.get(),
        ))
        .unwrap();
    }
    assert_eq!(
        provider.reports,
        [false, false, false, false, false, false, true]
    );
}

#[test]
fn dropped_playlist_body_retries_before_selecting_new_media() {
    let state = Rc::new(State::default());
    state.jump.set(true);
    let clock = Timer(state.clone());
    let mut provider = Provider::new(&state, vec![text(0, &["new"], false)]);
    provider.drop_body.set(true);
    let mut current = media(0, &["old"], false);
    let mut session = Session::default();
    run(session.prepare(
        &clock,
        &mut Buffer::new(&state),
        &mut provider,
        &mut current,
        0.0,
        state.controls(),
    ))
    .unwrap();
    assert_eq!(
        provider
            .requests
            .borrow()
            .iter()
            .map(|r| r.1)
            .collect::<Vec<_>>(),
        [0.0, 1.0]
    );
    assert_eq!(session.next(&current).unwrap().uri, "new");
}

#[test]
fn slow_segment_delivery_keeps_output_and_cursor_in_order() {
    let state = Rc::new(State::default());
    let mut provider = Provider::new(&state, vec![]);
    provider.segment_delay = 3.0;
    let current = media(0, &["first", "second"], true);
    let mut session = Session::default();
    let mut tx = Transmuxer::default();
    let mut out = Output::default();
    let first = run(package_next(
        &mut session,
        &current,
        &provider,
        &mut tx,
        &mut out,
    ))
    .unwrap();
    let second = run(package_next(
        &mut session,
        &current,
        &provider,
        &mut tx,
        &mut out,
    ))
    .unwrap();
    assert!(second > first);
    assert!(session.finished(&current));
    assert_eq!(state.now.get(), 6.0);
    assert_eq!(out.appends, 8);
    assert_eq!(
        provider
            .requests
            .borrow()
            .iter()
            .map(|r| r.0.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
}

#[test]
fn endless_segment_stops_without_consuming_or_appending() {
    let state = Rc::new(State::default());
    let mut provider = Provider::new(&state, vec![]);
    provider.endless = Some("endless");
    let current = media(0, &["endless"], false);
    let session = Session::default();
    let segment = session.next(&current).unwrap();
    assert!(matches!(
        run(net::download_segment(&provider, &segment.uri, 1024, None)),
        Err(FetchError::Permanent(_))
    ));
    assert_eq!(provider.polls.get(), 5);
    assert_eq!(session.next(&current).unwrap().uri, "endless");
}
