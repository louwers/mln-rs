//! The browser's players: the page's `<video>` elements, which a host opens
//! for the map through a JS object, a [`VideoHost`].
//!
//! The map may run in a Web Worker, which has no `<video>`, so the host
//! plays the videos where it can (the page's thread) and hands the map each
//! frame as an `ImageBitmap`, which the backends copy on the GPU. The
//! `<video>` loads the video itself, at the URLs after the map's resource
//! transform: the map's header rules, resource provider, offline packs and
//! cache don't apply to video in the browser.

use std::rc::Rc;
use std::sync::Arc;

use mln_actor::LocalObject;
use mln_gfx::video::{CpuPlane, FramePixels, GpuFrame, VideoFrame, WebImage};
use mln_util::size::Size;
use parking_lot::Mutex;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::player::{OpenRequest, VideoPlayer, VideoPlayers};
use crate::slot::FrameSink;
use crate::urls::{Transformed, transform_urls};

#[wasm_bindgen(typescript_custom_section)]
const VIDEO_HOST: &str = r#"
/**
 * Plays the videos of a map's video sources, for mln_video's WebPlayers:
 * where the page's `<video>` elements are, which may be another thread than
 * the map's.
 */
export interface VideoHost {
  /**
   * Opens the video at the first of `urls` that plays, and plays it from
   * its start, muted and looping. `onFrame` takes each new frame that the
   * video shows, and the frame there after a seek, even while paused: an
   * ImageBitmap that the callee closes, its size in pixels, and its time in
   * seconds from the video's start. `onError` takes why the video can't
   * play, such as none of its URLs playing; nothing comes after it. Neither
   * runs after `close()`.
   */
  open(
    urls: string[],
    onFrame: (frame: ImageBitmap, width: number, height: number, time: number) => void,
    onError: (message: string) => void,
  ): VideoHandle;
}

/** An open video of a VideoHost. */
export interface VideoHandle {
  play(): void;
  pause(): void;
  /** Moves to `seconds` from the start, and sends the frame there. */
  seek(seconds: number): void;
  /** Stops the video and lets go of it. */
  close(): void;
}
"#;

#[wasm_bindgen]
extern "C" {
    /// The JS object that opens a map's videos (web/pkg's `VideoHost`).
    #[wasm_bindgen(typescript_type = "VideoHost")]
    #[derive(Clone, Debug)]
    pub type VideoHost;

    #[wasm_bindgen(method, catch)]
    fn open(
        this: &VideoHost,
        urls: js_sys::Array,
        on_frame: &js_sys::Function,
        on_error: &js_sys::Function,
    ) -> Result<VideoHandle, JsValue>;

    /// An open video of a [`VideoHost`].
    #[wasm_bindgen(typescript_type = "VideoHandle")]
    #[derive(Debug)]
    type VideoHandle;

    #[wasm_bindgen(method, catch)]
    fn play(this: &VideoHandle) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch)]
    fn pause(this: &VideoHandle) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch)]
    fn seek(this: &VideoHandle, seconds: f64) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch)]
    fn close(this: &VideoHandle) -> Result<(), JsValue>;
}

/// The players of a browser host, which opens each video with its
/// [`VideoHost`]. They open videos on the thread they were made on, the
/// map's, and fail elsewhere.
pub struct WebPlayers {
    host: LocalObject<VideoHost>,
}

impl WebPlayers {
    #[must_use]
    pub fn new(host: VideoHost) -> WebPlayers {
        WebPlayers { host: LocalObject::new(host) }
    }
}

impl std::fmt::Debug for WebPlayers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebPlayers").finish_non_exhaustive()
    }
}

impl VideoPlayers for WebPlayers {
    /// Opens the video once the request's resource transform rewrote its
    /// URLs, or at once without one.
    fn open(&self, request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
        let host = self.host.get().ok_or("the browser's players open videos on the map's thread only")?;
        let player = Rc::new(Player {
            host: (*host).clone(),
            frames: request.frames,
            state: Mutex::new(State::Waiting { playing: true, seek: None }),
        });
        let waiting = LocalObject::new(Rc::downgrade(&player));
        transform_urls(request.urls, request.url_transform.as_ref(), move |urls| {
            if let Some(player) = waiting.get().and_then(|player| player.upgrade()) {
                player.start(urls);
            }
        });
        Ok(Box::new(WebPlayer(player)))
    }
}

/// A video of a [`VideoHost`], which closes when it drops.
struct WebPlayer(Rc<Player>);

struct Player {
    host: VideoHost,
    frames: FrameSink,
    state: Mutex<State>,
}

enum State {
    /// The URLs are on their way through the resource transform; the
    /// controls so far apply once the video opens.
    Waiting { playing: bool, seek: Option<f64> },
    Open {
        handle: VideoHandle,
        /// What the handle calls back, which lives while it's open.
        _on_frame: Closure<dyn FnMut(JsValue, u32, u32, f64)>,
        _on_error: Closure<dyn FnMut(String)>,
    },
    /// It failed to open, or it was closed.
    Closed,
}

impl Player {
    fn start(self: Rc<Self>, urls: Transformed) {
        let urls = match urls {
            Ok(urls) if urls.is_empty() => Err("the video source has no URLs".to_owned()),
            urls => urls,
        };
        let urls = match urls {
            Ok(urls) => urls,
            Err(message) => {
                *self.state.lock() = State::Closed;
                self.frames.fail(message);
                return;
            }
        };
        let frames = self.frames.clone();
        let on_frame = Closure::<dyn FnMut(JsValue, u32, u32, f64)>::new(move |image, width, height, time| {
            frames.push(frame(&frames, image, Size::new(width, height), time));
        });
        let frames = self.frames.clone();
        let on_error = Closure::<dyn FnMut(String)>::new(move |message: String| frames.fail(message));
        let array = urls.iter().map(|url| JsValue::from_str(url)).collect::<js_sys::Array>();
        let opened = self.host.open(array, on_frame.as_ref().unchecked_ref(), on_error.as_ref().unchecked_ref());
        let handle = match opened {
            Ok(handle) => handle,
            Err(error) => {
                *self.state.lock() = State::Closed;
                self.frames.fail(format!("the page couldn't open the video: {}", message(&error)));
                return;
            }
        };
        let mut state = self.state.lock();
        if let State::Waiting { playing, seek } = *state {
            if !playing {
                let _ = handle.pause();
            }
            if let Some(seconds) = seek {
                let _ = handle.seek(seconds);
            }
        }
        *state = State::Open { handle, _on_frame: on_frame, _on_error: on_error };
    }

    fn control(&self, play: Option<bool>, seek: Option<f64>) {
        match &mut *self.state.lock() {
            State::Waiting { playing, seek: waiting } => {
                *playing = play.unwrap_or(*playing);
                *waiting = seek.or(*waiting);
            }
            State::Open { handle, .. } => {
                let _ = match (play, seek) {
                    (Some(true), _) => handle.play(),
                    (Some(false), _) => handle.pause(),
                    (None, Some(seconds)) => handle.seek(seconds),
                    (None, None) => Ok(()),
                };
            }
            State::Closed => {}
        }
    }
}

impl VideoPlayer for WebPlayer {
    fn play(&mut self) {
        self.0.control(Some(true), None);
    }

    fn pause(&mut self) {
        self.0.control(Some(false), None);
    }

    fn seek(&mut self, seconds: f64) {
        self.0.control(None, Some(seconds));
    }
}

impl Drop for WebPlayer {
    fn drop(&mut self) {
        let state = std::mem::replace(&mut *self.0.state.lock(), State::Closed);
        if let State::Open { handle, .. } = state {
            let _ = handle.close();
        }
    }
}

/// The frame of `image`, a `size` `ImageBitmap` at `time` seconds: on the
/// GPU, or its pixels in memory once a renderer refused the GPU's.
fn frame(frames: &FrameSink, image: JsValue, size: Size, time: f64) -> VideoFrame {
    let time = time.is_finite().then_some(time);
    let image = WebImage(image);
    let pixels = if frames.gpu_frames_refused()
        && let Some(plane) = read_pixels(&image.0, size)
    {
        FramePixels::Rgba(plane)
    } else {
        FramePixels::Gpu(GpuFrame::Web(image))
    };
    VideoFrame { size, pixels, time }
}

/// The RGBA pixels of a `size` `ImageBitmap`, drawn on a canvas. A video's
/// frames are opaque, so they are premultiplied as they are.
fn read_pixels(image: &JsValue, size: Size) -> Option<CpuPlane> {
    let bitmap = image.dyn_ref::<web_sys::ImageBitmap>()?;
    let canvas = web_sys::OffscreenCanvas::new(size.width, size.height).ok()?;
    let context = canvas.get_context("2d").ok()??.dyn_into::<web_sys::OffscreenCanvasRenderingContext2d>().ok()?;
    context.draw_image_with_image_bitmap(bitmap, 0.0, 0.0).ok()?;
    let data = context.get_image_data(0.0, 0.0, f64::from(size.width), f64::from(size.height)).ok()?.data();
    Some(CpuPlane { data: data.0, stride: size.width as usize * 4 })
}

/// The message of a JS error, or the value as JS prints it.
fn message(error: &JsValue) -> String {
    error
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .or_else(|| error.as_string())
        .unwrap_or_else(|| format!("{error:?}"))
}

/// The players of a browser host that gave none: each video fails to open,
/// saying how to give them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Unsupplied;

impl VideoPlayers for Unsupplied {
    fn open(&self, _request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
        Err("in the browser, the host supplies the video players, through mln_video::WebPlayers".to_owned())
    }
}

/// [`crate::platform_players`] in the browser.
pub(crate) fn platform_players() -> Arc<dyn VideoPlayers> {
    Arc::new(Unsupplied)
}
