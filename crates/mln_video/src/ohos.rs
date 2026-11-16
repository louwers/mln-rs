//! `OpenHarmony`'s player: its `AVCodec` API, loaded at run time. A source
//! reads the video through a [`ByteSource`], a demuxer feeds its first video
//! track to a decoder in synchronous mode, and the decoder renders each frame
//! at its time into a consumer surface, whose native buffers go to the map,
//! or, once a renderer refuses those, decodes into memory, whose planes go to
//! the map. Each player runs on a thread of its own.

#![expect(unsafe_code, reason = "OpenHarmony's AVCodec API is the FFI here")]

mod ffi;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mln_gfx::video::{CpuPlane, FrameLease, FramePixels, GpuFrame, VideoFrame, YuvColor, YuvMatrix, YuvRange};
use mln_storage::FileSource;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use parking_lot::{Condvar, Mutex};

use self::ffi::{AvSource, BufferAttr, Demuxer, Format, Key, Ohos, Output, Surface, SurfaceBuffer};
use crate::bytes::ByteSource;
use crate::player::{OpenRequest, VideoPlayer, VideoPlayers};
use crate::slot::FrameSink;

/// How long the decoder waits for an output buffer before the player looks
/// at its controls again.
const OUTPUT_WAIT: Duration = Duration::from_millis(10);
/// How long a rendered frame may take to reach the surface.
const SURFACE_WAIT: Duration = Duration::from_secs(1);
/// How often a paused player checks whether the renderer refused its
/// frames, which it isn't told of.
const PAUSED_POLL: Duration = Duration::from_millis(100);

/// `OpenHarmony`'s players.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OhosPlayers;

impl VideoPlayers for OhosPlayers {
    fn open(&self, request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
        let shared = Arc::new(Shared {
            control: Mutex::new(Control { playing: true, seek: None, closed: false }),
            changed: Condvar::new(),
            bytes: Mutex::new(None),
        });
        let thread_shared = Arc::clone(&shared);
        let thread = mln_util::thread::spawn("video", move || run(request, &thread_shared))
            .map_err(|error| format!("starting the video's thread failed: {error}"))?;
        Ok(Box::new(OhosPlayer { shared, thread: Some(thread) }))
    }
}

/// What the player's handle tells its thread.
struct Control {
    playing: bool,
    /// A seek, in seconds, that the thread hasn't made yet.
    seek: Option<f64>,
    /// The handle dropped: the thread ends.
    closed: bool,
}

/// What the player's handle and its thread share.
struct Shared {
    control: Mutex<Control>,
    /// Signals a change of `control`.
    changed: Condvar,
    /// The bytes the thread reads, which closing cancels to end a blocked
    /// read.
    bytes: Mutex<Option<Arc<ByteSource>>>,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut Control)) {
        f(&mut self.control.lock());
        self.changed.notify_all();
    }

    fn closed(&self) -> bool {
        self.control.lock().closed
    }
}

/// A playing video, whose thread ends when it drops.
struct OhosPlayer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl VideoPlayer for OhosPlayer {
    fn play(&mut self) {
        self.shared.update(|control| control.playing = true);
    }

    fn pause(&mut self) {
        self.shared.update(|control| control.playing = false);
    }

    fn seek(&mut self, seconds: f64) {
        self.shared.update(|control| control.seek = Some(seconds.max(0.0)));
    }
}

impl Drop for OhosPlayer {
    fn drop(&mut self) {
        self.shared.update(|control| control.closed = true);
        if let Some(bytes) = self.shared.bytes.lock().as_ref() {
            bytes.cancel();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The player's thread: plays the first of the request's URLs that opens.
fn run(request: OpenRequest, shared: &Shared) {
    let OpenRequest { urls, file_source, frames, .. } = request;
    let Some(file_source) = file_source else {
        frames.fail("the map has no file source to read the video from");
        return;
    };
    let ohos = match Ohos::get() {
        Ok(ohos) => ohos,
        Err(error) => {
            frames.fail(error);
            return;
        }
    };
    let mut errors = Vec::new();
    for url in urls {
        let video = match Video::open(ohos, &file_source, &url, shared) {
            Ok(video) => video,
            Err(error) => {
                if shared.closed() {
                    return;
                }
                errors.push(format!("{url}: {error}"));
                continue;
            }
        };
        let mut playback = match Playback::new(video, frames.clone(), shared) {
            Ok(playback) => playback,
            Err(error) => {
                errors.push(format!("{url}: {error}"));
                continue;
            }
        };
        if let Err(error) = playback.run()
            && !shared.closed()
        {
            frames.fail(format!("{url}: {error}"));
        }
        return;
    }
    if errors.is_empty() {
        frames.fail("the video source has no URLs");
    } else {
        frames.fail(format!("none of the video's URLs plays: {}", errors.join("; ")));
    }
}

/// A video's first video track, open for reading.
struct Video {
    // Before the source, which it reads.
    demuxer: Demuxer,
    source: AvSource,
    ohos: &'static Ohos,
    format: Format,
    width: i32,
    height: i32,
}

impl Video {
    fn open(
        ohos: &'static Ohos,
        file_source: &Arc<dyn FileSource>,
        url: &str,
        shared: &Shared,
    ) -> Result<Video, String> {
        let bytes = Arc::new(ByteSource::new(Arc::clone(file_source), url));
        *shared.bytes.lock() = Some(Arc::clone(&bytes));
        // A close before the source was shared doesn't reach it.
        if shared.closed() {
            bytes.cancel();
        }
        let source = AvSource::new(ohos, bytes)?;
        let (track, format) = source.video_track().ok_or_else(|| match source.take_error() {
            Some(error) => format!("reading the video failed: {error}"),
            None => "the file has no video track".to_owned(),
        })?;
        let width = format.int(Key::Width).ok_or("the video track has no width")?;
        let height = format.int(Key::Height).ok_or("the video track has no height")?;
        let demuxer = Demuxer::new(ohos, &source, track)?;
        Ok(Video { demuxer, source, ohos, format, width, height })
    }

    /// Why the source failed to read last, after `error`.
    fn explain(&self, error: String) -> String {
        match self.source.take_error() {
            Some(read) => format!("{error}: {read}"),
            None => error,
        }
    }
}

/// How the player hands frames to the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// As native buffers.
    Gpu,
    /// Copied from the decoder's memory.
    Cpu,
}

/// Where a decoder into memory puts a frame's samples, as its output format
/// says.
#[derive(Clone, Copy, Debug)]
struct Layout {
    pixel_format: i32,
    /// The bytes from one luma row to the next.
    stride: usize,
    /// The rows of the luma plane, before the chroma.
    slice_height: usize,
    width: u32,
    height: u32,
    /// The picture's left and top edges in its planes.
    left: usize,
    top: usize,
}

impl Layout {
    fn of(format: Option<&Format>, video: &Video) -> Layout {
        let int = |key| format.and_then(|format| format.int(key)).filter(|value| *value > 0);
        let width = int(Key::PictureWidth).unwrap_or(video.width).max(1);
        let height = int(Key::PictureHeight).unwrap_or(video.height).max(1);
        let stride = int(Key::Stride).unwrap_or(width).max(width);
        let slice_height = int(Key::SliceHeight).unwrap_or(height).max(height);
        let edge = |key| format.and_then(|format| format.int(key)).unwrap_or(0).max(0) as usize;
        Layout {
            pixel_format: int(Key::PixelFormat).unwrap_or(ffi::PIXEL_FORMAT_NV12),
            stride: stride as usize,
            slice_height: slice_height as usize,
            width: width as u32,
            height: height as u32,
            left: edge(Key::CropLeft),
            top: edge(Key::CropTop),
        }
    }
}

/// A GPU frame's surface buffer, which goes back to its surface, for the
/// decoder to write again, when the frame drops; and the surface, which
/// outlives it.
struct SurfaceLease {
    // Dropped before the surface, which takes back its buffers.
    _buffer: SurfaceBuffer,
    _surface: Arc<Surface>,
}

// SAFETY: a shared lease does nothing; it is only moved and dropped.
unsafe impl Sync for SurfaceLease {}

/// A decoder that renders into a surface or decodes into memory.
struct Decoder {
    // Before the surface, whose window it renders into.
    codec: ffi::Decoder,
    /// Shared with the leases of the frames in flight, whose buffers go back
    /// to the surface before it goes.
    surface: Option<Arc<Surface>>,
    mode: Mode,
    layout: Layout,
}

impl Decoder {
    fn new(video: &Video, mode: Mode) -> Result<Decoder, String> {
        let (surface, codec) = match mode {
            Mode::Gpu => {
                // The decoder's own format, which hardware decoders render
                // in, or RGBA, where the system allocates no surface buffers
                // of it, such as the emulator's.
                let attempt = |pixel_format| {
                    let surface = Surface::new(video.ohos, video.width, video.height)?;
                    let codec = ffi::Decoder::new(video.ohos, &video.format, pixel_format, Some(surface.window()))?;
                    Ok::<_, String>((Some(surface), codec))
                };
                attempt(None).or_else(|error| {
                    attempt(Some(ffi::PIXEL_FORMAT_RGBA)).map_err(|rgba| format!("{error}; in RGBA: {rgba}"))
                })?
            }
            Mode::Cpu => {
                let nv12 = ffi::Decoder::new(video.ohos, &video.format, Some(ffi::PIXEL_FORMAT_NV12), None);
                let codec = nv12.or_else(|_| ffi::Decoder::new(video.ohos, &video.format, None, None))?;
                (None, codec)
            }
        };
        let layout = Layout::of(codec.output_format().as_ref(), video);
        Ok(Decoder { codec, surface: surface.map(Arc::new), mode, layout })
    }
}

/// When frames are due: the frame at `origin_us` at `start`, the others
/// after it by their times.
struct Clock {
    start: Instant,
    origin_us: i64,
}

impl Clock {
    fn due(&self, time_us: i64) -> Instant {
        let ahead = Duration::from_micros(time_us.saturating_sub(self.origin_us).max(0) as u64);
        self.start + ahead
    }
}

/// A video playing on the player's thread.
struct Playback<'a> {
    // Before the video, whose demuxer feeds it.
    decoder: Decoder,
    video: Video,
    frames: FrameSink,
    shared: &'a Shared,
    /// Whether the decoder has the end of the track.
    input_ended: bool,
    /// An output buffer that waits for its time.
    pending: Option<(u32, BufferAttr)>,
    /// The latest output buffer before a seek's time, which shows if the
    /// track ends before that time.
    skipped: Option<(u32, BufferAttr)>,
    /// When the frames are due, or `None` when the next one shows at once
    /// and starts the clock.
    clock: Option<Clock>,
    playing: bool,
    /// The time of the frame shown last, in microseconds.
    position_us: i64,
    /// The time between the last two frames shown, in microseconds.
    frame_interval_us: i64,
    /// Where a seek went: the frames before it decode unseen, and the first
    /// at or after it shows at once.
    seeking_to: Option<i64>,
    /// Whether a frame showed since the video last started over, so that a
    /// video without frames doesn't loop forever.
    shown_since_start: bool,
    /// Whether a frame's picture filled only part of its native buffer,
    /// which an external texture samples whole: frames go through memory,
    /// which `copy_planes` crops, from then on.
    cropped: bool,
    color: YuvColor,
}

impl<'a> Playback<'a> {
    fn new(video: Video, frames: FrameSink, shared: &'a Shared) -> Result<Playback<'a>, String> {
        let decoder = if frames.gpu_frames_refused() {
            Decoder::new(&video, Mode::Cpu)?
        } else {
            // A device that renders into no surface may still decode into
            // memory.
            Decoder::new(&video, Mode::Gpu).or_else(|error| {
                Log::info(Event::Render, format!("Video frames go through memory: {error}"));
                Decoder::new(&video, Mode::Cpu)
            })?
        };
        let color = color(Some(&video.format), default_color(video.height));
        Ok(Playback {
            decoder,
            video,
            frames,
            shared,
            input_ended: false,
            pending: None,
            skipped: None,
            clock: None,
            playing: false,
            position_us: 0,
            frame_interval_us: 0,
            // The first frame shows at once, and starts the clock.
            seeking_to: Some(0),
            shown_since_start: false,
            cropped: false,
            color,
        })
    }

    fn run(&mut self) -> Result<(), String> {
        loop {
            let (playing, seek) = {
                let mut control = self.shared.control.lock();
                if control.closed {
                    return Ok(());
                }
                (control.playing, control.seek.take())
            };
            if playing && !self.playing {
                // The clock starts again from the next frame.
                self.clock = None;
            }
            self.playing = playing;
            if self.decoder.mode == Mode::Gpu && (self.frames.gpu_frames_refused() || self.cropped) {
                self.decoder = Decoder::new(&self.video, Mode::Cpu)?;
                self.seek(seek.map_or(self.position_us, seconds_to_us))?;
            } else if let Some(seconds) = seek {
                self.seek(seconds_to_us(seconds))?;
            }
            if !self.playing && self.seeking_to.is_none() {
                let mut control = self.shared.control.lock();
                if !control.closed && control.playing == self.playing && control.seek.is_none() {
                    self.shared.changed.wait_for(&mut control, PAUSED_POLL);
                }
                continue;
            }
            self.feed().map_err(|error| self.video.explain(error))?;
            self.step()?;
        }
    }

    /// Moves the track and the decoder to the sync sample before `time_us`,
    /// to show the frame at `time_us` next.
    fn seek(&mut self, time_us: i64) -> Result<(), String> {
        self.video.demuxer.seek_to(time_us)?;
        // The flush takes back the buffers held.
        self.decoder.codec.flush()?;
        self.pending = None;
        self.skipped = None;
        self.input_ended = false;
        self.seeking_to = Some(time_us);
        Ok(())
    }

    /// Queues the track's samples into the decoder's free input buffers.
    fn feed(&mut self) -> Result<(), String> {
        while !self.input_ended {
            match self.decoder.codec.queue_input(&self.video.demuxer)? {
                None => break,
                Some(ended) => self.input_ended = ended,
            }
        }
        Ok(())
    }

    /// Takes the decoder's next output, and shows it once it is due.
    fn step(&mut self) -> Result<(), String> {
        let (index, attr) = match self.pending {
            Some(pending) => pending,
            None => match self.decoder.codec.dequeue_output(OUTPUT_WAIT)? {
                Output::Buffer(index, attr) => (index, attr),
                Output::FormatChanged => {
                    let format = self.decoder.codec.output_format();
                    self.color = color(format.as_ref(), self.color);
                    self.decoder.layout = Layout::of(format.as_ref(), &self.video);
                    return Ok(());
                }
                Output::None => return Ok(()),
            },
        };
        self.pending = None;
        if attr.flags & ffi::END_OF_STREAM != 0 {
            self.decoder.codec.free_output(index)?;
            if let Some((last, last_attr)) = self.skipped.take() {
                // A seek past the last frame shows the last frame.
                self.seeking_to = None;
                self.show(last, last_attr)?;
            }
            return self.start_over();
        }
        let time_us = attr.pts;
        if let Some(target) = self.seeking_to {
            let earlier = if time_us < target { self.skipped.replace((index, attr)) } else { self.skipped.take() };
            if let Some((earlier, _)) = earlier {
                self.decoder.codec.free_output(earlier)?;
            }
            if time_us < target {
                return Ok(());
            }
            self.seeking_to = None;
            self.clock = Some(Clock { start: Instant::now(), origin_us: time_us });
        } else {
            let due =
                self.clock.get_or_insert_with(|| Clock { start: Instant::now(), origin_us: time_us }).due(time_us);
            if Instant::now() < due {
                // Waits for the frame's time, or a change of the controls.
                self.pending = Some((index, attr));
                let mut control = self.shared.control.lock();
                if !control.closed && control.playing == self.playing && control.seek.is_none() {
                    self.shared.changed.wait_until(&mut control, due);
                }
                return Ok(());
            }
        }
        self.show(index, attr)
    }

    /// Hands output buffer `index`'s frame to the map: rendered into the
    /// surface and taken from it, or copied from memory.
    fn show(&mut self, index: u32, attr: BufferAttr) -> Result<(), String> {
        let time_us = attr.pts;
        let time = Some(time_us as f64 / 1e6);
        let decoder = &mut self.decoder;
        let frame = match decoder.mode {
            Mode::Gpu => {
                decoder.codec.render_output(index)?;
                let surface = decoder.surface.as_ref().ok_or("the decoder has no surface")?;
                let Some(buffer) = surface.acquire(SURFACE_WAIT)? else {
                    Log::warning(Event::Render, "A decoded video frame didn't reach the surface");
                    return Ok(());
                };
                let layout = decoder.layout;
                let (width, height) = buffer.size()?;
                if (layout.left, layout.top, layout.width, layout.height) != (0, 0, width, height) {
                    Log::info(
                        Event::Render,
                        format!(
                            "Video frames go through memory: the {}x{} picture is part of its {width}x{height} buffer",
                            layout.width, layout.height
                        ),
                    );
                    self.cropped = true;
                    self.position_us = time_us;
                    return Ok(());
                }
                let native = buffer.reference_native_buffer()?;
                // The frame keeps the buffer from the surface, so that the
                // decoder doesn't write into it while a renderer reads it.
                let lease: FrameLease = Box::new(SurfaceLease { _buffer: buffer, _surface: Arc::clone(surface) });
                let pixels = FramePixels::Gpu(GpuFrame::OhosNativeBuffer {
                    buffer: native.as_ptr(),
                    color: self.color,
                    lease: Some(lease),
                });
                VideoFrame { size: Size::new(layout.width, layout.height), pixels, time }
            }
            Mode::Cpu => {
                let layout = decoder.layout;
                let copied =
                    decoder.codec.output_bytes(index, attr).and_then(|bytes| copy_planes(bytes, &layout, self.color));
                decoder.codec.free_output(index)?;
                VideoFrame { size: Size::new(layout.width, layout.height), pixels: copied?, time }
            }
        };
        self.frames.push(frame);
        if time_us > self.position_us {
            self.frame_interval_us = time_us - self.position_us;
        }
        self.position_us = time_us;
        self.shown_since_start = true;
        Ok(())
    }

    /// Goes back to the start at the end of the track, as a looping video
    /// does.
    fn start_over(&mut self) -> Result<(), String> {
        if !self.shown_since_start {
            return Err("the video has no frames".to_owned());
        }
        self.shown_since_start = false;
        let seeking = self.seeking_to.is_some();
        self.seek(0)?;
        if !seeking {
            // The first frame follows the last on the clock, not at once.
            self.seeking_to = None;
            let next = self.position_us + self.frame_interval_us;
            self.clock = self.clock.take().map(|clock| Clock { start: clock.due(next), origin_us: 0 });
        }
        Ok(())
    }
}

fn seconds_to_us(seconds: f64) -> i64 {
    (seconds * 1e6) as i64
}

/// The colors of a video `height` pixels high that doesn't say: BT.601 for
/// standard definition, BT.709 from 720 rows up, in limited range.
fn default_color(height: i32) -> YuvColor {
    let matrix = if height >= 720 { YuvMatrix::Bt709 } else { YuvMatrix::Bt601 };
    YuvColor { matrix, range: YuvRange::Limited }
}

/// The colors that `format` says, H.273's matrix coefficients and a full
/// range flag, or `fallback`'s where it doesn't.
fn color(format: Option<&Format>, fallback: YuvColor) -> YuvColor {
    let Some(format) = format else { return fallback };
    let matrix = match format.int(Key::MatrixCoefficients) {
        Some(1) => YuvMatrix::Bt709,
        Some(5 | 6) => YuvMatrix::Bt601,
        Some(9 | 10) => YuvMatrix::Bt2020,
        _ => fallback.matrix,
    };
    let range = match format.int(Key::RangeFlag) {
        Some(0) => YuvRange::Limited,
        Some(_) => YuvRange::Full,
        None => fallback.range,
    };
    YuvColor { matrix, range }
}

/// Copies `rows` rows of `width` bytes from `data`, whose rows are `stride`
/// bytes apart, from byte `left` of row `top`.
fn copy_rows(
    data: &[u8],
    stride: usize,
    (left, top): (usize, usize),
    (width, rows): (usize, usize),
) -> Result<CpuPlane, String> {
    let mut out = Vec::with_capacity(width * rows);
    for row in 0..rows {
        let start = (top + row) * stride + left;
        out.extend_from_slice(data.get(start..start + width).ok_or("a decoded frame is shorter than its rows")?);
    }
    Ok(CpuPlane { data: out, stride: width })
}

/// The pixels of a decoded frame in `data`, a 4:2:0 YUV picture in
/// `layout`: NV12, I420, or NV21, whose chroma swaps into NV12's order.
fn copy_planes(data: &[u8], layout: &Layout, color: YuvColor) -> Result<FramePixels, String> {
    let (width, height) = (layout.width as usize, layout.height as usize);
    let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
    let (left, top) = (layout.left, layout.top);
    let y = copy_rows(data, layout.stride, (left, top), (width, height))?;
    let chroma = data.get(layout.stride * layout.slice_height..).ok_or("a decoded frame has no chroma")?;
    let chroma_origin = (left / 2, top / 2);
    match layout.pixel_format {
        ffi::PIXEL_FORMAT_NV12 | ffi::PIXEL_FORMAT_NV21 => {
            let mut uv = copy_rows(
                chroma,
                layout.stride,
                (chroma_origin.0 * 2, chroma_origin.1),
                (chroma_width * 2, chroma_height),
            )?;
            if layout.pixel_format == ffi::PIXEL_FORMAT_NV21 {
                for pair in uv.data.as_chunks_mut::<2>().0 {
                    pair.swap(0, 1);
                }
            }
            Ok(FramePixels::Nv12 { y, uv, color })
        }
        ffi::PIXEL_FORMAT_I420 => {
            let chroma_stride = layout.stride.div_ceil(2);
            let v_start = chroma_stride * layout.slice_height.div_ceil(2);
            let v_data = chroma.get(v_start..).ok_or("a decoded frame has no Cr plane")?;
            let size = (chroma_width, chroma_height);
            let u = copy_rows(chroma, chroma_stride, chroma_origin, size)?;
            let v = copy_rows(v_data, chroma_stride, chroma_origin, size)?;
            Ok(FramePixels::I420 { y, u, v, color })
        }
        other => Err(format!("the decoder decodes into pixel format {other}, which isn't 4:2:0 YUV")),
    }
}
