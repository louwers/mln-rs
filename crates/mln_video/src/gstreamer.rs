//! The video player of Linux and the BSDs: GStreamer 1.x, loaded at run
//! time, so that the library runs, without video, where it isn't installed.
//!
//! Each player runs a pipeline on a thread of its own:
//!
//! ```text
//! appsrc (random access, reading a ByteSource) ! decodebin3 ! [videoconvert !] appsink
//! ```
//!
//! `decodebin3` decodes only the first video stream: the player asks for it
//! when the decoder posts the streams it found, so audio never decodes. The
//! appsink syncs to the pipeline's clock, so frames arrive at the video's
//! rate, and it hands each to the source's [`FrameSink`]. While renderers
//! take GPU frames, the appsink accepts dmabufs first, which VA-API decoders
//! hand over without a copy, and has no `videoconvert`, which would refuse
//! them; once a renderer refuses one, or a decoder's frames don't share as
//! dmabufs, the player builds the pipeline again, for frames in memory,
//! where it was.

#![expect(unsafe_code, reason = "GStreamer's C API is the FFI here")]

mod ffi;
mod frames;

use std::ffi::{CStr, c_int, c_uint, c_void};
use std::io;
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use mln_storage::FileSource;
use mln_util::logging::{Event, Log};
use parking_lot::Mutex;

use self::ffi::{Api, MiniRef, Object};
use self::frames::Converted;
use crate::bytes::ByteSource;
use crate::player::{OpenRequest, VideoPlayer, VideoPlayers};
use crate::slot::FrameSink;

/// How long the driver waits for a message before it checks whether
/// renderers refused its GPU frames.
const POLL_NANOS: u64 = 100_000_000;

/// The bytes a read asks for when GStreamer asks for any.
const DEFAULT_READ: usize = 64 * 1024;

/// The players of GStreamer.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GstreamerPlayers;

impl VideoPlayers for GstreamerPlayers {
    fn open(&self, request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
        let api = ffi::api()?;
        let file_source =
            request.file_source.ok_or_else(|| "the map has no file source to read the video through".to_owned())?;
        let shared = Arc::new(Shared {
            frames: request.frames,
            requests: Mutex::default(),
            bus: Mutex::new(None),
            bytes: Mutex::new(None),
            stopped: AtomicBool::new(false),
        });
        let driver = Driver { api, shared: Arc::clone(&shared), file_source, playing: true, seek: None };
        let urls = request.urls;
        let thread = std::thread::Builder::new()
            .name("mln-video".to_owned())
            .spawn(move || driver.run(urls))
            .map_err(|error| format!("the video's thread didn't start: {error}"))?;
        Ok(Box::new(GstreamerPlayer { shared, thread: Some(thread) }))
    }
}

/// What a player and its driver share.
struct Shared {
    frames: FrameSink,
    requests: Mutex<Requests>,
    /// The bus of the pipeline that plays, which wakes its driver.
    bus: Mutex<Option<Object>>,
    /// The bytes that the pipeline reads, which the player cancels when it
    /// drops; `None` once it did.
    bytes: Mutex<Option<Arc<ByteSource>>>,
    stopped: AtomicBool,
}

/// What the player asked of its driver since the driver last looked.
#[derive(Default)]
struct Requests {
    playing: Option<bool>,
    seek: Option<f64>,
}

impl Shared {
    /// Wakes the driver, which waits for its pipeline's messages.
    fn wake(&self) {
        let bus = self.bus.lock();
        let Some(bus) = bus.as_ref() else { return };
        let Ok(api) = ffi::api() else { return };
        // SAFETY: the bus is live; the message takes the structure and the
        // bus takes the message.
        unsafe {
            let structure = (api.structure_new_empty)(c"mln-video-wake".as_ptr());
            let message = (api.message_new_application)(ptr::null_mut(), structure);
            if !message.is_null() {
                (api.bus_post)(bus.as_ptr(), message);
            }
        }
    }

    fn request(&self, change: impl FnOnce(&mut Requests)) {
        change(&mut self.requests.lock());
        self.wake();
    }

    /// Stops the driver, ending its reads.
    fn stop(&self) {
        {
            let mut bytes = self.bytes.lock();
            self.stopped.store(true, Ordering::Relaxed);
            if let Some(bytes) = bytes.take() {
                bytes.cancel();
            }
        }
        self.wake();
    }
}

/// A playing video: its driver's thread, which it stops and joins on drop.
struct GstreamerPlayer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl VideoPlayer for GstreamerPlayer {
    fn play(&mut self) {
        self.shared.request(|requests| requests.playing = Some(true));
    }

    fn pause(&mut self) {
        self.shared.request(|requests| requests.playing = Some(false));
    }

    fn seek(&mut self, seconds: f64) {
        if seconds.is_finite() {
            self.shared.request(|requests| requests.seek = Some(seconds.max(0.0)));
        }
    }
}

impl Drop for GstreamerPlayer {
    fn drop(&mut self) {
        self.shared.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// How playing one URL ended.
enum Ended {
    Stopped,
    Failed(String),
}

/// How driving one pipeline ended.
enum Drove {
    Stopped,
    Failed(String),
    /// Renderers refused the pipeline's GPU frames, or its frames don't
    /// share: the player goes on in memory from the position, if known.
    NeedsMemory(Option<f64>),
}

/// The thread that builds a player's pipelines and steers them.
struct Driver {
    api: &'static Api,
    shared: Arc<Shared>,
    file_source: Arc<dyn FileSource>,
    playing: bool,
    /// Where to seek once the pipeline can.
    seek: Option<f64>,
}

impl Driver {
    fn run(mut self, urls: Vec<String>) {
        let mut failures = Vec::new();
        for url in urls {
            let bytes = Arc::new(ByteSource::new(Arc::clone(&self.file_source), url.clone()));
            {
                let mut current = self.shared.bytes.lock();
                if self.shared.stopped.load(Ordering::Relaxed) {
                    return;
                }
                *current = Some(Arc::clone(&bytes));
            }
            match self.play(&bytes) {
                Ended::Stopped => return,
                Ended::Failed(why) => failures.push(format!("{url}: {why}")),
            }
            bytes.cancel();
        }
        if self.stopped() {
            return;
        }
        if failures.is_empty() {
            self.shared.frames.fail("it has no URLs");
        } else {
            self.shared.frames.fail(failures.join("; "));
        }
    }

    fn stopped(&self) -> bool {
        self.shared.stopped.load(Ordering::Relaxed)
    }

    /// Plays the video at `bytes` until the player drops or the video
    /// fails.
    fn play(&mut self, bytes: &Arc<ByteSource>) -> Ended {
        let size = match bytes.size() {
            Ok(size) => size,
            Err(_) if self.stopped() => return Ended::Stopped,
            Err(error) => return Ended::Failed(error.to_string()),
        };
        let mut gpu = !self.shared.frames.gpu_frames_refused();
        loop {
            let pipeline = match Pipeline::new(self.api, &self.shared, bytes, size, gpu) {
                Ok(pipeline) => pipeline,
                Err(why) => return Ended::Failed(why),
            };
            *self.shared.bus.lock() = Some(pipeline.bus());
            let drove = self.drive(&pipeline, gpu);
            *self.shared.bus.lock() = None;
            drop(pipeline);
            match drove {
                Drove::Stopped => return Ended::Stopped,
                // A decoder's frames may not negotiate without
                // `videoconvert`, which only the pipeline in memory has.
                Drove::Failed(_) if gpu && !self.stopped() => gpu = false,
                Drove::Failed(why) => return Ended::Failed(why),
                Drove::NeedsMemory(position) => {
                    gpu = false;
                    self.seek = self.seek.or(position);
                }
            }
        }
    }

    /// Steers `pipeline` until it ends, as the player asks.
    fn drive(&mut self, pipeline: &Pipeline, gpu: bool) -> Drove {
        if !pipeline.set_state(ffi::STATE_PAUSED) {
            return Drove::Failed("GStreamer's pipeline doesn't start".to_owned());
        }
        let mut prerolled = false;
        let mut missing_plugin = None;
        loop {
            if self.stopped() {
                return Drove::Stopped;
            }
            let requests = std::mem::take(&mut *self.shared.requests.lock());
            if let Some(playing) = requests.playing {
                self.playing = playing;
                if prerolled {
                    pipeline.set_state(if playing { ffi::STATE_PLAYING } else { ffi::STATE_PAUSED });
                }
            }
            if requests.seek.is_some() {
                self.seek = requests.seek;
            }
            if prerolled && let Some(seconds) = self.seek.take() {
                pipeline.seek(seconds, ffi::SEEK_FLAG_ACCURATE);
            }
            if gpu && (self.shared.frames.gpu_frames_refused() || pipeline.sink.unshareable.load(Ordering::Relaxed)) {
                return Drove::NeedsMemory(pipeline.position());
            }
            let Some(message) = pipeline.pop(POLL_NANOS) else { continue };
            // SAFETY: a message from the bus, whose type is its header's.
            let kind = unsafe { (*message.as_ptr().cast::<ffi::Message>()).type_ };
            match kind {
                ffi::MESSAGE_ASYNC_DONE if !prerolled => {
                    prerolled = true;
                    if let Some(seconds) = self.seek.take() {
                        pipeline.seek(seconds, ffi::SEEK_FLAG_ACCURATE);
                    }
                    if self.playing {
                        pipeline.set_state(ffi::STATE_PLAYING);
                    }
                }
                ffi::MESSAGE_EOS => {
                    if let Some(why) = pipeline.source.error.lock().take() {
                        return Drove::Failed(why);
                    }
                    pipeline.seek(0.0, ffi::SEEK_FLAG_KEY_UNIT);
                }
                ffi::MESSAGE_ERROR => {
                    let why = pipeline
                        .source
                        .error
                        .lock()
                        .take()
                        .or(missing_plugin)
                        .or_else(|| pipeline.decoding.why_not(self.api, self.not_negotiated(&message)))
                        .unwrap_or_else(|| self.error_message(&message));
                    return Drove::Failed(why);
                }
                ffi::MESSAGE_ELEMENT => {
                    if let Some(name) = self.missing_plugin(&message) {
                        missing_plugin = Some(format!("GStreamer has no {name} installed"));
                    }
                }
                _ => {}
            }
        }
    }

    /// The text of an error message.
    fn error_message(&self, message: &MiniRef) -> String {
        let api = self.api;
        let (mut error, mut debug) = (ptr::null_mut(), ptr::null_mut());
        // SAFETY: an error message, whose parse gives a new error and debug
        // string, which are freed here.
        unsafe {
            (api.message_parse_error)(message.as_ptr(), &raw mut error, &raw mut debug);
            (api.g_free)(debug.cast());
            api.take_error(error)
        }
    }

    /// Whether an error message is of a stream that stopped because its
    /// elements didn't agree on a format.
    fn not_negotiated(&self, message: &MiniRef) -> bool {
        let api = self.api;
        let mut details = ptr::null();
        // SAFETY: an error message, whose details are its own.
        unsafe {
            (api.message_parse_error_details)(message.as_ptr(), &raw mut details);
            !details.is_null() && api.structure_int(details, c"flow-return") == Some(ffi::FLOW_NOT_NEGOTIATED)
        }
    }

    /// What a missing-plugin message says is missing, such as "VP9
    /// decoder".
    fn missing_plugin(&self, message: &MiniRef) -> Option<String> {
        let api = self.api;
        // SAFETY: an element message, whose structure is its own.
        unsafe {
            let structure = (api.message_get_structure)(message.as_ptr());
            if structure.is_null() || (api.structure_has_name)(structure, c"missing-plugin".as_ptr()) == 0 {
                return None;
            }
            api.structure_string(structure, c"name").or_else(|| api.structure_string(structure, c"type"))
        }
    }
}

/// One pipeline that plays a video.
struct Pipeline {
    api: &'static Api,
    pipeline: Object,
    bus: Object,
    source: Arc<SourceState>,
    sink: Arc<SinkState>,
    decoding: Arc<Decoding>,
}

impl Pipeline {
    /// A pipeline that reads `bytes`, of `size` if known, and hands its
    /// frames to `shared`'s sink: dmabufs or frames in memory, if `gpu`,
    /// else only frames in memory.
    fn new(
        api: &'static Api,
        shared: &Arc<Shared>,
        bytes: &Arc<ByteSource>,
        size: Option<u64>,
        gpu: bool,
    ) -> Result<Pipeline, String> {
        let make = |factory: &CStr| {
            // SAFETY: a factory's new element, whose floating reference the
            // object takes.
            unsafe { Object::sink(api, (api.element_factory_make)(factory.as_ptr(), ptr::null())) }
                .ok_or_else(|| format!("GStreamer can't make a {}", factory.to_string_lossy()))
        };
        // SAFETY: a new pipeline, whose floating reference the object takes.
        let pipeline = unsafe { Object::sink(api, (api.pipeline_new)(ptr::null())) }
            .ok_or_else(|| "GStreamer can't make a pipeline".to_owned())?;
        let src = make(c"appsrc")?;
        let decode = make(api.decodebin)?;
        let convert = if gpu { None } else { Some(make(c"videoconvert")?) };
        let sink = make(c"appsink")?;
        // SAFETY: the pipeline's bus, a new reference.
        let bus = unsafe { Object::take(api, (api.element_get_bus)(pipeline.as_ptr())) }
            .ok_or_else(|| "GStreamer's pipeline has no bus".to_owned())?;
        let source = Arc::new(SourceState {
            bytes: Arc::clone(bytes),
            size,
            offset: AtomicU64::new(0),
            error: Mutex::new(None),
        });
        let sink_state = Arc::new(SinkState {
            api,
            shared: Arc::clone(shared),
            gpu,
            unshareable: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            last: Mutex::new(None),
        });
        let decoding = Arc::new(Decoding {
            decodebin: decode.as_ptr() as usize,
            no_video: AtomicBool::new(false),
            video_pad: Mutex::new(None),
            video_media_type: Mutex::new(None),
        });
        let built =
            Pipeline { api, pipeline, bus, source: Arc::clone(&source), sink: Arc::clone(&sink_state), decoding };

        let elements = [Some(&src), Some(&decode), convert.as_ref(), Some(&sink)];
        // SAFETY: live elements, which the pipeline adds a reference on.
        for element in elements.into_iter().flatten() {
            if unsafe { (api.bin_add)(built.pipeline.as_ptr(), element.as_ptr()) } == 0 {
                return Err("GStreamer can't build a pipeline".to_owned());
            }
        }
        let target = convert.as_ref().unwrap_or(&sink);

        // SAFETY: the elements are an appsrc and an appsink, as their
        // functions need; each callback's data stays alive until GStreamer
        // calls its destroy function.
        unsafe {
            (api.app_src_set_stream_type)(src.as_ptr(), ffi::APP_STREAM_TYPE_RANDOM_ACCESS);
            (api.app_src_set_size)(src.as_ptr(), size.and_then(|size| i64::try_from(size).ok()).unwrap_or(-1));
            let mut callbacks = ffi::AppSrcCallbacks {
                need_data: Some(need_data),
                enough_data: None,
                seek_data: Some(seek_data),
                reserved: [ptr::null_mut(); 4],
            };
            (api.app_src_set_callbacks)(
                src.as_ptr(),
                &raw mut callbacks,
                Arc::into_raw(source).cast_mut().cast(),
                Some(release_arc::<SourceState>),
            );

            let formats = if gpu {
                format!("video/x-raw(memory:DMABuf), format=(string)DMA_DRM; {}", frames::CPU_FORMATS)
            } else {
                frames::CPU_FORMATS.to_owned()
            };
            let formats = std::ffi::CString::new(formats).map_err(|_| "caps with a nul".to_owned())?;
            let caps = MiniRef::take(api, (api.caps_from_string)(formats.as_ptr()))
                .ok_or_else(|| "GStreamer can't parse the appsink's caps".to_owned())?;
            (api.app_sink_set_caps)(sink.as_ptr(), caps.as_ptr());
            api.set(&sink, c"sync", "true");
            api.set(&sink, c"max-buffers", "1");
            api.set(&sink, c"drop", "true");
            api.set(&sink, c"enable-last-sample", "false");
            let mut callbacks = ffi::AppSinkCallbacks {
                eos: None,
                new_preroll: Some(new_preroll),
                new_sample: Some(new_sample),
                new_event: None,
                propose_allocation: Some(propose_allocation),
                reserved: [ptr::null_mut(); 2],
            };
            (api.app_sink_set_callbacks)(
                sink.as_ptr(),
                &raw mut callbacks,
                Arc::into_raw(sink_state).cast_mut().cast(),
                Some(release_arc::<SinkState>),
            );

            let pad_added: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) = pad_added;
            // SAFETY: GObject calls a handler with the signal's arguments,
            // which are those of `pad_added`.
            let handler = std::mem::transmute::<
                unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void),
                ffi::Callback,
            >(pad_added);
            let target = Box::new(LinkTarget { element: target.clone(), decoding: Arc::clone(&built.decoding) });
            (api.g_signal_connect_data)(
                decode.as_ptr(),
                c"pad-added".as_ptr(),
                handler,
                Box::into_raw(target).cast(),
                Some(release_link_target),
                0,
            );

            (api.bus_set_sync_handler)(
                built.bus.as_ptr(),
                Some(on_bus_message),
                Arc::into_raw(Arc::clone(&built.decoding)).cast_mut().cast(),
                Some(release_arc::<Decoding>),
            );
        }
        // Last, since `decodebin3` reads the appsrc in pull mode only if it
        // is random access when they link.
        // SAFETY: elements of one pipeline.
        let linked = unsafe {
            (api.element_link)(src.as_ptr(), decode.as_ptr()) != 0
                && convert.as_ref().is_none_or(|convert| (api.element_link)(convert.as_ptr(), sink.as_ptr()) != 0)
        };
        if !linked {
            return Err("GStreamer can't link its pipeline".to_owned());
        }
        Ok(built)
    }

    fn bus(&self) -> Object {
        self.bus.clone()
    }

    /// Whether the pipeline took the state, or is taking it.
    fn set_state(&self, state: c_int) -> bool {
        // SAFETY: a live pipeline.
        unsafe { (self.api.element_set_state)(self.pipeline.as_ptr(), state) != ffi::STATE_CHANGE_FAILURE }
    }

    /// Seeks to `seconds`, flushing, at a key frame or `ACCURATE`ly.
    fn seek(&self, seconds: f64, flags: c_int) {
        let nanos = (seconds * 1e9) as i64;
        // SAFETY: a live pipeline.
        unsafe {
            (self.api.element_seek_simple)(
                self.pipeline.as_ptr(),
                ffi::FORMAT_TIME,
                ffi::SEEK_FLAG_FLUSH | flags,
                nanos,
            )
        };
    }

    /// The position in the video, in seconds, if the pipeline knows it.
    fn position(&self) -> Option<f64> {
        let mut nanos = 0;
        // SAFETY: a live pipeline.
        let known =
            unsafe { (self.api.element_query_position)(self.pipeline.as_ptr(), ffi::FORMAT_TIME, &raw mut nanos) } != 0;
        (known && nanos >= 0).then(|| nanos as f64 / 1e9)
    }

    /// The bus's next message, waiting up to `nanos` for one.
    fn pop(&self, nanos: u64) -> Option<MiniRef> {
        // SAFETY: a live bus; the message is a new reference.
        unsafe { MiniRef::take(self.api, (self.api.bus_timed_pop)(self.bus.as_ptr(), nanos)) }
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        // SAFETY: a live pipeline and its bus. Stopping the pipeline joins
        // its streaming threads, so no callback runs after it.
        unsafe {
            (self.api.element_set_state)(self.pipeline.as_ptr(), ffi::STATE_NULL);
            (self.api.bus_set_sync_handler)(self.bus.as_ptr(), None, ptr::null_mut(), None);
        }
        self.decoding.video_pad.lock().take();
    }
}

/// What the appsrc's callbacks read.
struct SourceState {
    bytes: Arc<ByteSource>,
    size: Option<u64>,
    offset: AtomicU64,
    /// Why a read failed, which the driver reports over GStreamer's error.
    error: Mutex<Option<String>>,
}

/// Feeds the appsrc the `length` bytes at its offset, on its streaming
/// thread, waiting for them to load.
unsafe extern "C" fn need_data(appsrc: *mut c_void, length: c_uint, data: *mut c_void) {
    let Ok(api) = ffi::api() else { return };
    // SAFETY: the data is the `SourceState` that the callbacks were set
    // with, alive until they are released.
    let state = unsafe { &*data.cast::<SourceState>() };
    let offset = state.offset.load(Ordering::Relaxed);
    let length = if length == 0 || length == c_uint::MAX { DEFAULT_READ } else { length as usize };
    // A demuxer may ask for more than the video has, as a broken file says.
    let length = state.size.map_or(length, |size| length.min(size.saturating_sub(offset) as usize)).max(1);
    // SAFETY: a new buffer of `length` bytes, mapped for writing while the
    // bytes are read into it; the appsrc takes it, or it is released.
    unsafe {
        let buffer = (api.buffer_new_allocate)(ptr::null_mut(), length, ptr::null_mut());
        let mut info = ffi::MapInfo::new();
        if buffer.is_null() || (api.buffer_map)(buffer, &raw mut info, ffi::MAP_WRITE) == 0 {
            if !buffer.is_null() {
                (api.mini_object_unref)(buffer);
            }
            (api.app_src_end_of_stream)(appsrc);
            return;
        }
        let read = state.bytes.read_full(offset, std::slice::from_raw_parts_mut(info.data, info.size));
        (api.buffer_unmap)(buffer, &raw mut info);
        match read {
            Ok(count) if count > 0 => {
                (api.buffer_set_size)(buffer, count as isize);
                state.offset.store(offset + count as u64, Ordering::Relaxed);
                (api.app_src_push_buffer)(appsrc, buffer);
            }
            result => {
                (api.mini_object_unref)(buffer);
                if let Err(error) = result
                    && error.kind() != io::ErrorKind::Interrupted
                {
                    *state.error.lock() = Some(error.to_string());
                }
                (api.app_src_end_of_stream)(appsrc);
            }
        }
    }
}

/// Moves the appsrc's reads to `offset`.
unsafe extern "C" fn seek_data(_appsrc: *mut c_void, offset: u64, data: *mut c_void) -> ffi::GBoolean {
    // SAFETY: as for `need_data`.
    let state = unsafe { &*data.cast::<SourceState>() };
    state.offset.store(offset, Ordering::Relaxed);
    1
}

/// What the appsink's callbacks share with the driver.
struct SinkState {
    api: &'static Api,
    shared: Arc<Shared>,
    /// Whether the appsink accepts dmabufs.
    gpu: bool,
    /// Whether a frame came in GPU memory that doesn't share as a dmabuf.
    unshareable: AtomicBool,
    /// Whether a frame failed to convert, which logs once.
    failed: AtomicBool,
    /// The buffer and time of the last frame handed over: a pipeline that
    /// starts playing renders the frame it prerolled with again.
    last: Mutex<Option<(usize, u64)>>,
}

impl SinkState {
    fn on_sample(&self, sample: MiniRef) {
        if self.gpu && self.shared.frames.gpu_frames_refused() {
            self.shared.wake();
            return;
        }
        // SAFETY: the sample's buffer, live while the sample is.
        let key = unsafe {
            let buffer = (self.api.sample_get_buffer)(sample.as_ptr());
            (!buffer.is_null()).then(|| (buffer as usize, (*buffer.cast::<ffi::Buffer>()).pts))
        };
        {
            let mut last = self.last.lock();
            if key.is_some() && *last == key {
                return;
            }
            *last = key;
        }
        match frames::convert(self.api, sample) {
            Ok(Converted::Frame(frame)) => self.shared.frames.push(frame),
            Ok(Converted::Unshareable(why)) => {
                Log::info(Event::Render, format!("Video frames go through memory: {why}"));
                self.unshareable.store(true, Ordering::Relaxed);
                self.shared.wake();
            }
            Err(why) => {
                if !self.failed.swap(true, Ordering::Relaxed) {
                    Log::warning(Event::Render, format!("A video frame was dropped: {why}"));
                }
            }
        }
    }
}

/// Hands over the frame that the appsink prerolled with.
unsafe extern "C" fn new_preroll(appsink: *mut c_void, data: *mut c_void) -> c_int {
    // SAFETY: the data is the `SinkState` that the callbacks were set with,
    // alive until they are released; the sample is a new reference.
    unsafe {
        let state = &*data.cast::<SinkState>();
        if let Some(sample) = MiniRef::take(state.api, (state.api.app_sink_pull_preroll)(appsink)) {
            state.on_sample(sample);
        }
    }
    ffi::FLOW_OK
}

/// Hands over the frame that the appsink renders now.
unsafe extern "C" fn new_sample(appsink: *mut c_void, data: *mut c_void) -> c_int {
    // SAFETY: as for `new_preroll`.
    unsafe {
        let state = &*data.cast::<SinkState>();
        if let Some(sample) = MiniRef::take(state.api, (state.api.app_sink_pull_sample)(appsink)) {
            state.on_sample(sample);
        }
    }
    ffi::FLOW_OK
}

/// Tells the decoder that the appsink reads video metas, so that it may
/// hand over frames with padded rows, and dmabufs, which VA-API decoders
/// hand over only with them. GStreamer calls this from 1.24.
unsafe extern "C" fn propose_allocation(_appsink: *mut c_void, query: *mut c_void, data: *mut c_void) -> ffi::GBoolean {
    // SAFETY: as for `new_preroll`; the query is an allocation query.
    unsafe {
        let api = (*data.cast::<SinkState>()).api;
        (api.query_add_allocation_meta)(query, (api.video_meta_api_get_type)(), ptr::null());
    }
    1
}

/// Links a decoder's video pad to the element that the signal's data holds,
/// once.
unsafe extern "C" fn pad_added(_element: *mut c_void, pad: *mut c_void, data: *mut c_void) {
    let Ok(api) = ffi::api() else { return };
    // SAFETY: the data is the boxed `LinkTarget` of `Pipeline::new`, alive
    // until the handler is released; the pad is the new pad.
    unsafe {
        let target = &*data.cast::<LinkTarget>();
        if !is_video_pad(api, pad) {
            return;
        }
        if let Some(pad) = NonNull::new(pad) {
            *target.decoding.video_pad.lock() = Some(Object::borrow(api, pad));
        }
        let sink_pad = (api.element_get_static_pad)(target.element.as_ptr(), c"sink".as_ptr());
        if sink_pad.is_null() {
            return;
        }
        if (api.pad_is_linked)(sink_pad) == 0 && (api.pad_link)(pad, sink_pad) != ffi::PAD_LINK_OK {
            Log::warning(Event::Render, "GStreamer's video decoder didn't link");
        }
        (api.object_unref)(sink_pad);
    }
}

/// The media type of a decoder's pad that carries video that it can't
/// decode, such as `video/x-vp9`: `decodebin3` hands over the encoded
/// stream when no decoder is installed, which fails to link downstream. The
/// pad keeps the caps that it sent.
///
/// # Safety
///
/// `pad` is a live pad.
unsafe fn encoded_media_type(api: &Api, pad: *mut c_void) -> Option<String> {
    // SAFETY: as the caller says; the caps are a new reference.
    unsafe {
        let caps = (api.pad_get_current_caps)(pad);
        if caps.is_null() {
            return None;
        }
        let structure = (api.caps_get_structure)(caps, 0);
        let name = if structure.is_null() { None } else { ffi::string((api.structure_get_name)(structure)) };
        (api.mini_object_unref)(caps);
        name.filter(|name| name != "video/x-raw")
    }
}

impl Decoding {
    /// What the decoder lacks, for a failure that is `not_negotiated` or
    /// not.
    fn why_not(&self, api: &Api, not_negotiated: bool) -> Option<String> {
        let pad = self.video_pad.lock();
        // SAFETY: the pad is live while the value holds it.
        let media_type = match pad.as_ref() {
            Some(pad) => unsafe { encoded_media_type(api, pad.as_ptr()) },
            // `decodebin3` doesn't hand over a stream that it can't decode,
            // which stops for want of a format downstream.
            None if not_negotiated => self.video_media_type.lock().clone(),
            None => None,
        };
        if let Some(media_type) = media_type {
            return Some(format!("GStreamer has no decoder for {media_type} installed"));
        }
        self.no_video.load(Ordering::Relaxed).then(|| "it has no video".to_owned())
    }
}

/// Whether a decoder's `pad` carries video, by its stream or its caps.
///
/// # Safety
///
/// `pad` is a live pad.
unsafe fn is_video_pad(api: &Api, pad: *mut c_void) -> bool {
    // SAFETY: as the caller says; the stream and caps are new references.
    unsafe {
        let stream = (api.pad_get_stream)(pad);
        if !stream.is_null() {
            let kind = (api.stream_get_stream_type)(stream);
            (api.object_unref)(stream);
            return kind & ffi::STREAM_TYPE_VIDEO != 0;
        }
        let caps = (api.pad_get_current_caps)(pad);
        if caps.is_null() {
            return false;
        }
        let video = frames::is_video_caps(api, caps);
        (api.mini_object_unref)(caps);
        video
    }
}

/// The decoder of a pipeline, whose streams the bus's sync handler selects,
/// and what it lacks.
struct Decoding {
    /// The `decodebin3`, which the pipeline holds while the handler is set.
    decodebin: usize,
    /// Whether the decoder found streams but no video.
    no_video: AtomicBool,
    /// The decoder's pad that the pipeline plays.
    video_pad: Mutex<Option<Object>>,
    /// The media type of the video stream that the decoder selected, such
    /// as `video/x-vp9`.
    video_media_type: Mutex<Option<String>>,
}

/// What a decoder's `pad-added` handler links its video pad to.
struct LinkTarget {
    element: Object,
    decoding: Arc<Decoding>,
}

/// Selects the first video stream of each collection that `decodebin3`
/// posts, as it posts it, so that it decodes nothing else.
unsafe extern "C" fn on_bus_message(_bus: *mut c_void, message: *mut c_void, data: *mut c_void) -> c_int {
    let Ok(api) = ffi::api() else { return ffi::BUS_PASS };
    // SAFETY: the data is the `Decoding` that the handler was set with,
    // alive until it is released; the message is live during the call, and
    // the collection that it parses to is a new reference.
    unsafe {
        let decoding = &*data.cast::<Decoding>();
        if (*message.cast::<ffi::Message>()).type_ != ffi::MESSAGE_STREAM_COLLECTION {
            return ffi::BUS_PASS;
        }
        let mut collection = ptr::null_mut();
        (api.message_parse_stream_collection)(message, &raw mut collection);
        if collection.is_null() {
            return ffi::BUS_PASS;
        }
        let video = (0..(api.stream_collection_get_size)(collection))
            .map(|index| (api.stream_collection_get_stream)(collection, index))
            .find(|&stream| !stream.is_null() && (api.stream_get_stream_type)(stream) & ffi::STREAM_TYPE_VIDEO != 0);
        let id = video.map(|stream| (api.stream_get_stream_id)(stream)).filter(|id| !id.is_null());
        match (video, id) {
            (Some(stream), Some(id)) => {
                decoding.no_video.store(false, Ordering::Relaxed);
                let caps = (api.stream_get_caps)(stream);
                if !caps.is_null() {
                    let structure = (api.caps_get_structure)(caps, 0);
                    if !structure.is_null() {
                        *decoding.video_media_type.lock() = ffi::string((api.structure_get_name)(structure));
                    }
                    (api.mini_object_unref)(caps);
                }
                // The event copies the IDs, which the collection holds.
                let list = (api.g_list_append)(ptr::null_mut(), id.cast_mut().cast());
                let event = (api.event_new_select_streams)(list);
                (api.g_list_free)(list);
                if !event.is_null() {
                    (api.element_send_event)(decoding.decodebin as *mut c_void, event);
                }
            }
            _ => decoding.no_video.store(true, Ordering::Relaxed),
        }
        (api.object_unref)(collection);
    }
    ffi::BUS_PASS
}

/// Releases the `Arc<T>` of a callback's data.
unsafe extern "C" fn release_arc<T>(data: *mut c_void) {
    // SAFETY: the data came from `Arc::into_raw` of an `Arc<T>`.
    drop(unsafe { Arc::from_raw(data.cast_const().cast::<T>()) });
}

/// Releases the boxed target of a `pad-added` handler.
unsafe extern "C" fn release_link_target(data: *mut c_void, _closure: *mut c_void) {
    // SAFETY: the data came from `Box::into_raw` of a `Box<LinkTarget>`.
    drop(unsafe { Box::from_raw(data.cast::<LinkTarget>()) });
}
