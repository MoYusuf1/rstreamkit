use rstreamkit::{mkv, mp4, mse, vod};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::{JsCast, prelude::*};

thread_local! { static GENERATION: Cell<u64> = const { Cell::new(0) }; static PLAYER: RefCell<Option<mse::Player>> = const { RefCell::new(None) }; }

fn video() -> web_sys::HtmlVideoElement {
    web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .get_element_by_id("video")
        .unwrap()
        .dyn_into()
        .unwrap()
}
fn report(status: mse::Status) {
    web_sys::console::log_1(&format!("{status:?}").into());
    let global = js_sys::global();
    if let Ok(f) = js_sys::Reflect::get(&global, &"onStatus".into()) {
        if let Some(f) = f.dyn_ref::<js_sys::Function>() {
            let _ = f.call1(&global, &format!("{status:?}").into());
        }
    }
}
#[wasm_bindgen]
pub fn stop() {
    GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    PLAYER.with(|p| {
        p.borrow_mut().take();
    });
}
#[wasm_bindgen]
pub fn live(url: String) {
    stop();
    PLAYER.with(|p| {
        *p.borrow_mut() = Some(mse::start(video(), url, mse::Direct, false, true, report));
    });
}
#[wasm_bindgen]
pub async fn movie(url: String) -> Result<(), JsValue> {
    stop();
    let generation = GENERATION.with(Cell::get);
    let m = mse::probe(&mse::Direct, &url)
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    if GENERATION.with(Cell::get) != generation {
        return Ok(());
    }
    PLAYER.with(|p| {
        *p.borrow_mut() = Some(mse::play_movie(video(), m, url, mse::Direct, 0.0, report));
    });
    Ok(())
}
#[wasm_bindgen]
pub fn transmux(bytes: &[u8], streamed: bool, repeats: u32) -> Result<usize, JsValue> {
    let mut t = rstreamkit::Transmuxer::default();
    let mut size = 0;
    for _ in 0..repeats {
        let out = if streamed {
            let mut segment = rstreamkit::Segment::new(bytes.len());
            for chunk in bytes.chunks(16 << 10) {
                segment.feed(chunk);
            }
            t.finish(segment)
        } else {
            t.push(bytes)
        }
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
        size = out
            .fragments
            .iter()
            .map(|f| f.moof.len() + f.mdat.len())
            .sum();
    }
    Ok(size)
}
#[wasm_bindgen]
pub fn demux(bytes: &[u8]) -> Result<usize, JsValue> {
    rstreamkit::ts::demux(bytes)
        .map(|d| d.video.len())
        .map_err(|e| JsValue::from_str(&e.to_string()))
}
fn load(bytes: &[u8], matroska: bool) -> Result<Rc<vod::Movie>, JsValue> {
    let err = |e: rstreamkit::Error| JsValue::from_str(&e.to_string());
    if matroska {
        let mut p = mkv::Probe::new(bytes.len() as u64);
        let (mut at, mut len) = (0, 256 << 10);
        loop {
            let end = (at + len).min(bytes.len() as u64);
            match p.feed(at, &bytes[at as usize..end as usize]).map_err(err)? {
                mkv::Step::Done(p) => {
                    return Ok(Rc::new(vod::Movie::from_mkv(*p, bytes.len() as u64)));
                }
                mkv::Step::Read(a, l) => {
                    at = a;
                    len = l;
                }
            }
        }
    }
    let mut at = 0;
    loop {
        match mp4::find_moov(&bytes[at..], at as u64) {
            mp4::Moov::At(off, len) => {
                let (_, _, head) = mp4::box_header(&bytes[off as usize..]).unwrap();
                return vod::Movie::from_mp4(
                    &bytes[off as usize + head..(off + len) as usize],
                    bytes.len() as u64,
                )
                .map(Rc::new)
                .map_err(err);
            }
            mp4::Moov::Next(to) => at = to as usize,
            mp4::Moov::Missing => return Err("missing moov".into()),
        }
    }
}
#[wasm_bindgen]
pub fn movie_profile(bytes: &[u8], matroska: bool, play: bool) -> Result<usize, JsValue> {
    let m = load(bytes, matroska)?;
    if !play {
        return Ok(m.duration as usize);
    }
    let mut session = m.session(0.0);
    let mut size = 0;
    let piece = (m.bytes_per_second() * 5.0) as u64;
    while let Some((at, len)) = session.range(piece.max(256 << 10)) {
        size += session
            .push(&bytes[at as usize..(at + len).min(bytes.len() as u64) as usize])
            .map_err(|e| JsValue::from_str(&e.to_string()))?
            .len();
    }
    Ok(size)
}
#[wasm_bindgen]
pub async fn timer() {
    mse::sleep(std::time::Duration::from_millis(10)).await;
}

#[wasm_bindgen]
pub fn memory_bytes() -> usize {
    wasm_bindgen::memory()
        .unchecked_into::<js_sys::WebAssembly::Memory>()
        .buffer()
        .unchecked_into::<js_sys::ArrayBuffer>()
        .byte_length() as usize
}

#[wasm_bindgen]
pub struct ProfileSession {
    movie: Rc<vod::Movie>,
    session: vod::Session,
}
#[wasm_bindgen]
impl ProfileSession {
    pub fn mp4(moov: &[u8], size: u64) -> Result<ProfileSession, JsValue> {
        let movie = Rc::new(
            vod::Movie::from_mp4(moov, size).map_err(|e| JsValue::from_str(&e.to_string()))?,
        );
        let session = movie.session(0.0);
        Ok(ProfileSession { movie, session })
    }
    pub fn range(&mut self) -> Option<Vec<u64>> {
        self.session
            .range((self.movie.bytes_per_second() * 5.0) as u64)
            .map(|(a, l)| vec![a, l])
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<usize, JsValue> {
        self.session
            .push(bytes)
            .map(|b| b.len())
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }
    pub fn duration(&self) -> f64 {
        self.movie.duration
    }
}
#[wasm_bindgen]
pub struct ProfileProbe {
    probe: mkv::Probe,
    size: u64,
    movie: Option<Rc<vod::Movie>>,
}
#[wasm_bindgen]
impl ProfileProbe {
    #[wasm_bindgen(constructor)]
    pub fn new(size: u64) -> ProfileProbe {
        ProfileProbe {
            probe: mkv::Probe::new(size),
            size,
            movie: None,
        }
    }
    pub fn feed(&mut self, at: u64, bytes: &[u8]) -> Result<Option<Vec<u64>>, JsValue> {
        match self
            .probe
            .feed(at, bytes)
            .map_err(|e| JsValue::from_str(&e.to_string()))?
        {
            mkv::Step::Read(a, l) => Ok(Some(vec![a, l])),
            mkv::Step::Done(p) => {
                self.movie = Some(Rc::new(vod::Movie::from_mkv(*p, self.size)));
                Ok(None)
            }
        }
    }
    pub fn session(&self) -> Result<ProfileSession, JsValue> {
        let movie = self
            .movie
            .as_ref()
            .ok_or_else(|| JsValue::from_str("probe incomplete"))?
            .clone();
        let session = movie.session(0.0);
        Ok(ProfileSession { movie, session })
    }
}

#[wasm_bindgen]
pub struct ProfileStream {
    tx: rstreamkit::Transmuxer,
    segment: Option<rstreamkit::Segment>,
}
#[wasm_bindgen]
impl ProfileStream {
    #[wasm_bindgen(constructor)]
    pub fn new() -> ProfileStream {
        ProfileStream {
            tx: rstreamkit::Transmuxer::default(),
            segment: None,
        }
    }
    pub fn begin(&mut self, expected: usize) {
        self.segment = Some(rstreamkit::Segment::new(expected));
    }
    pub fn feed(&mut self, bytes: &[u8]) {
        self.segment.as_mut().unwrap().feed(bytes);
    }
    pub fn finish(&mut self) -> Result<usize, JsValue> {
        self.tx
            .finish(self.segment.take().unwrap())
            .map(|out| {
                out.fragments
                    .iter()
                    .map(|f| f.moof.len() + f.mdat.len())
                    .sum()
            })
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }
}

#[wasm_bindgen]
pub struct ProfileAudio {
    pcm: Vec<Vec<i16>>,
}
#[wasm_bindgen]
impl ProfileAudio {
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8]) -> Result<ProfileAudio, JsValue> {
        let d = rstreamkit::ts::demux(bytes).map_err(|e| JsValue::from_str(&e.to_string()))?;
        let kind = d
            .sound_kind
            .ok_or_else(|| JsValue::from_str("no compressed audio"))?;
        let mut decoder = rstreamkit::sound::Decoder::new();
        let pcm = d
            .sound
            .iter()
            .map(|f| {
                decoder.decode(
                    kind,
                    &rstreamkit::sound::Coded {
                        data: &f.data,
                        samples: f.samples,
                        rate: f.rate,
                        channels: f.channels,
                    },
                )
            })
            .collect();
        Ok(ProfileAudio { pcm })
    }
    pub fn raw_size(&self) -> usize {
        self.pcm.iter().map(|p| p.len() * 2).sum()
    }
    pub fn encode(&self) -> usize {
        self.pcm
            .iter()
            .enumerate()
            .map(|(i, p)| rstreamkit::sound::flac_frame(p, i as u32).len())
            .sum()
    }
}

#[wasm_bindgen]
pub fn pause() {
    PLAYER.with(|p| {
        if let Some(p) = p.borrow().as_ref() {
            p.pause();
        }
    });
}
#[wasm_bindgen]
pub fn resume() {
    PLAYER.with(|p| {
        if let Some(p) = p.borrow().as_ref() {
            p.resume();
        }
    });
}
#[wasm_bindgen]
pub fn go_live() {
    PLAYER.with(|p| {
        if let Some(p) = p.borrow().as_ref() {
            p.go_live();
        }
    });
}
#[wasm_bindgen]
pub fn stats() -> String {
    PLAYER.with(|p| {
        p.borrow()
            .as_ref()
            .map(|p| format!("{:?}", p.stats()))
            .unwrap_or_default()
    })
}
