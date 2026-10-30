//! Small browser entry point: playback controls, without benchmark exports.
use rstreamkit::mse;
use std::cell::{Cell, RefCell};
use wasm_bindgen::{JsCast, prelude::*};

thread_local! {
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    static PLAYER: RefCell<Option<mse::Player>> = const { RefCell::new(None) };
}

fn video() -> Result<web_sys::HtmlVideoElement, JsValue> {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("video"))
        .and_then(|v| v.dyn_into().ok())
        .ok_or_else(|| JsValue::from_str("Expected a <video id=\"video\"> element"))
}

fn report(status: mse::Status) {
    let global = js_sys::global();
    if let Ok(callback) = js_sys::Reflect::get(&global, &"onStatus".into())
        && let Some(callback) = callback.dyn_ref::<js_sys::Function>()
    {
        let _ = callback.call1(&global, &format!("{status:?}").into());
    }
}

#[wasm_bindgen]
pub fn stop() {
    GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    PLAYER.with(|p| p.borrow_mut().take());
}

#[wasm_bindgen]
pub fn live(url: String) -> Result<(), JsValue> {
    let video = video()?;
    stop();
    PLAYER.with(|p| {
        *p.borrow_mut() = Some(mse::start(video, url, mse::Direct, false, true, report));
    });
    Ok(())
}

#[cfg(feature = "vod")]
#[wasm_bindgen]
/// `start` is where to begin, in seconds (resuming); 0 when left out.
pub async fn movie(url: String, start: Option<f64>) -> Result<(), JsValue> {
    let video = video()?;
    stop();
    let generation = GENERATION.with(Cell::get);
    let movie = mse::probe(&mse::Direct, &url)
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    if GENERATION.with(Cell::get) == generation {
        PLAYER.with(|p| {
            *p.borrow_mut() = Some(mse::play_movie(
                video,
                movie,
                url,
                mse::Direct,
                start.unwrap_or(0.0),
                report,
            ));
        });
    }
    Ok(())
}

#[wasm_bindgen]
pub fn pause() {
    PLAYER.with(|p| {
        if let Some(player) = p.borrow().as_ref() {
            player.pause();
        }
    });
}

#[wasm_bindgen]
pub fn resume() {
    PLAYER.with(|p| {
        if let Some(player) = p.borrow().as_ref() {
            player.resume();
        }
    });
}

#[wasm_bindgen]
pub fn go_live() {
    PLAYER.with(|p| {
        if let Some(player) = p.borrow().as_ref() {
            player.go_live();
        }
    });
}

#[wasm_bindgen]
pub fn stats() -> String {
    PLAYER.with(|p| {
        p.borrow()
            .as_ref()
            .map(|p| {
                let s = p.stats();
                format!("Stats {{ buffered_seconds: {:.3}, latency_seconds: {:.3}, bitrate: {:.0}, dropped_segments: {}, current_variant: {:?}, variants: {:?}, live_window_seconds: {:.3} }}",
                    s.buffered_seconds, s.latency_seconds, s.bitrate, s.dropped_segments,
                    s.current_variant, s.variants, s.live_window_seconds)
            })
            .unwrap_or_default()
    })
}

#[wasm_bindgen]
pub fn memory_bytes() -> usize {
    wasm_bindgen::memory()
        .unchecked_into::<js_sys::WebAssembly::Memory>()
        .buffer()
        .unchecked_into::<js_sys::ArrayBuffer>()
        .byte_length() as usize
}
