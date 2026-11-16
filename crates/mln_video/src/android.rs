//! Android's player: the NDK's media API, loaded at run time. An extractor
//! reads the video through a [`ByteSource`], a decoder of its first video
//! track renders each frame at its time into an image reader's window, and
//! the reader's images go to the map as hardware buffers, or, once a
//! renderer refuses those, copied into memory. Each player runs on a thread
//! of its own.

#![expect(unsafe_code, reason = "the NDK's media API is the FFI here")]

mod ndk;
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

use self::ndk::{Codec, DataSource, Extractor, Format, Image, ImageReader, Ndk, Output};
use crate::bytes::ByteSource;
use crate::player::{OpenRequest, VideoPlayer, VideoPlayers};
use crate::slot::FrameSink;

/// The hardware buffer frames that the map may hold at once: the newest
/// frame, and those that renderers still draw into their textures. Each
/// frame keeps its image from the reader, so that the decoder doesn't write
/// into a buffer that a renderer draws.
const FRAMES_IN_FLIGHT: usize = 3;
/// How long the decoder waits for an output buffer before the player looks
/// at its controls again.
const OUTPUT_WAIT: Duration = Duration::from_millis(10);
/// How long a rendered frame may take to reach the image reader.
const IMAGE_WAIT: Duration = Duration::from_secs(1);
/// How often a paused player checks whether the renderer refused its
/// frames, which it isn't told of.
const PAUSED_POLL: Duration = Duration::from_millis(100);

/// Android's players.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AndroidPlayers;

impl VideoPlayers for AndroidPlayers {
    fn open(&self, request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
        let shared = Arc::new(Shared {
            control: Mutex::new(Control { playing: true, seek: None, closed: false }),
            changed: Condvar::new(),
            bytes: Mutex::new(None),
        });
        let thread_shared = Arc::clone(&shared);
        let thread = mln_util::thread::spawn("video", move || run(request, &thread_shared))
            .map_err(|error| format!("starting the video's thread failed: {error}"))?;
        Ok(Box::new(AndroidPlayer { shared, thread: Some(thread) }))
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
struct AndroidPlayer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl VideoPlayer for AndroidPlayer {
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

impl Drop for AndroidPlayer {
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
    let ndk = match Ndk::get() {
        Ok(ndk) => ndk,
        Err(error) => {
            frames.fail(error);
            return;
        }
    };
    let mut errors = Vec::new();
    for url in urls {
        let video = match Video::open(ndk, &file_source, &url, shared) {
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
        if let Err(error) = playback.run() {
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
    // Before the data source, which it reads.
    extractor: Extractor,
    _source: DataSource,
    ndk: &'static Ndk,
    format: Format,
    width: i32,
    height: i32,
}

impl Video {
    fn open(ndk: &'static Ndk, file_source: &Arc<dyn FileSource>, url: &str, shared: &Shared) -> Result<Video, String> {
        let bytes = Arc::new(ByteSource::new(Arc::clone(file_source), url));
        *shared.bytes.lock() = Some(Arc::clone(&bytes));
        // A close before the source was shared doesn't reach it.
        if shared.closed() {
            bytes.cancel();
        }
        let source = DataSource::new(ndk, bytes)?;
        let extractor = Extractor::new(ndk, &source).map_err(|error| match source.take_error() {
            Some(read) => format!("{error}: {read}"),
            None => error,
        })?;
        for track in 0..extractor.track_count() {
            let Some(format) = extractor.track_format(track) else { continue };
            if !format.string("mime").is_some_and(|mime| mime.starts_with("video/")) {
                continue;
            }
            extractor.select_track(track)?;
            let width = format.int32("width").ok_or("the video track has no width")?;
            let height = format.int32("height").ok_or("the video track has no height")?;
            return Ok(Video { extractor, _source: source, ndk, format, width, height });
        }
        Err("the file has no video track".to_owned())
    }
}

/// How the player hands frames to the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// As hardware buffers.
    Gpu,
    /// Copied into memory.
    Cpu,
}

/// A GPU frame's image, which goes back to its reader, for the decoder to
/// write again, when the frame drops; and the reader, which outlives it.
struct ImageLease {
    // Dropped before the reader: deleting a reader takes back its images.
    _image: Image,
    _reader: Arc<ImageReader>,
}

// SAFETY: a shared lease does nothing; it is only moved and dropped.
unsafe impl Sync for ImageLease {}

/// A decoder that renders into an image reader.
struct Decoder {
    // Before the reader, whose window it renders into.
    codec: Codec,
    /// Shared with the leases of the frames in flight, whose images go back
    /// to the reader before it goes.
    reader: Arc<ImageReader>,
    mode: Mode,
}

impl Decoder {
    fn new(video: &Video, mode: Mode) -> Result<Decoder, String> {
        let (usage, max_images) = match mode {
            // The frames in flight, the newest image, and one more that
            // acquiring the latest takes.
            Mode::Gpu => (ndk::USAGE_GPU_SAMPLED_IMAGE, FRAMES_IN_FLIGHT + 2),
            Mode::Cpu => (ndk::USAGE_CPU_READ_OFTEN, 2),
        };
        let reader = ImageReader::new(video.ndk, video.width, video.height, usage, max_images as i32)?;
        let codec = Codec::decoder(video.ndk, &video.format, reader.window())?;
        Ok(Decoder { codec, reader: Arc::new(reader), mode })
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
    // Before the video, whose extractor feeds it.
    decoder: Decoder,
    video: Video,
    frames: FrameSink,
    shared: &'a Shared,
    /// Whether the decoder has the end of the track.
    input_ended: bool,
    /// An output buffer that waits for its time.
    pending: Option<(usize, ndk::BufferInfo)>,
    /// The latest output buffer before a seek's time, which shows if the
    /// track ends before that time.
    skipped: Option<(usize, ndk::BufferInfo)>,
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
    color: YuvColor,
}

impl<'a> Playback<'a> {
    fn new(video: Video, frames: FrameSink, shared: &'a Shared) -> Result<Playback<'a>, String> {
        let decoder = if frames.gpu_frames_refused() {
            Decoder::new(&video, Mode::Cpu)?
        } else {
            // A device that decodes into no GPU-only buffers may still
            // decode into readable ones.
            Decoder::new(&video, Mode::Gpu).or_else(|error| {
                Log::info(Event::Android, format!("Video frames go through memory: {error}"));
                Decoder::new(&video, Mode::Cpu)
            })?
        };
        let color = default_color(video.height);
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
            if self.decoder.mode == Mode::Gpu && self.frames.gpu_frames_refused() {
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
            self.feed()?;
            self.step()?;
        }
    }

    /// Moves the track and the decoder to the sync sample before `time_us`,
    /// to show the frame at `time_us` next.
    fn seek(&mut self, time_us: i64) -> Result<(), String> {
        self.video.extractor.seek_to(time_us)?;
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
        let extractor = &self.video.extractor;
        while !self.input_ended {
            let mut ended = false;
            let queued = self.decoder.codec.queue_input(|buffer| {
                if let Some(size) = extractor.read_sample(buffer) {
                    (size, extractor.sample_time(), 0)
                } else {
                    ended = true;
                    (0, 0, ndk::END_OF_STREAM)
                }
            })?;
            if !queued {
                break;
            }
            if ended {
                self.input_ended = true;
            } else {
                extractor.advance();
            }
        }
        Ok(())
    }

    /// Takes the decoder's next output, and shows it once it is due.
    fn step(&mut self) -> Result<(), String> {
        let (index, info) = match self.pending {
            Some(pending) => pending,
            None => match self.decoder.codec.dequeue_output(OUTPUT_WAIT)? {
                Output::Buffer(index, info) => (index, info),
                Output::FormatChanged => {
                    self.color = self.output_color();
                    return Ok(());
                }
                Output::None => return Ok(()),
            },
        };
        self.pending = None;
        if info.flags & ndk::END_OF_STREAM != 0 {
            self.decoder.codec.release_output(index, false)?;
            if let Some((last, last_info)) = self.skipped.take() {
                // A seek past the last frame shows the last frame.
                self.seeking_to = None;
                self.show(last, last_info.presentation_time_us)?;
            }
            return self.start_over();
        }
        let time_us = info.presentation_time_us;
        if let Some(target) = self.seeking_to {
            let earlier = if time_us < target { self.skipped.replace((index, info)) } else { self.skipped.take() };
            if let Some((earlier, _)) = earlier {
                self.decoder.codec.release_output(earlier, false)?;
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
                self.pending = Some((index, info));
                let mut control = self.shared.control.lock();
                if !control.closed && control.playing == self.playing && control.seek.is_none() {
                    self.shared.changed.wait_until(&mut control, due);
                }
                return Ok(());
            }
        }
        self.show(index, time_us)
    }

    /// Renders output buffer `index`, of the frame at `time_us`, and hands
    /// the image it becomes to the map.
    fn show(&mut self, index: usize, time_us: i64) -> Result<(), String> {
        self.decoder.codec.release_output(index, true)?;
        let Some(image) = self.decoder.reader.acquire_latest(IMAGE_WAIT)? else {
            Log::warning(Event::Android, "A decoded video frame didn't reach the image reader");
            return Ok(());
        };
        let (width, height) = image.size()?;
        let size = Size::new(width, height);
        let time = Some(time_us as f64 / 1e6);
        let frame = match self.decoder.mode {
            Mode::Gpu => {
                let buffer = image.acquire_hardware_buffer()?;
                let lease: FrameLease =
                    Box::new(ImageLease { _image: image, _reader: Arc::clone(&self.decoder.reader) });
                let pixels = FramePixels::Gpu(GpuFrame::AndroidHardwareBuffer {
                    buffer: buffer.as_ptr(),
                    color: self.color,
                    lease: Some(lease),
                });
                VideoFrame { size, pixels, time }
            }
            Mode::Cpu => VideoFrame { size, pixels: copy_planes(&image, size, self.color)?, time },
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

    /// The colors of the decoder's output, as its format says, or as the
    /// video's size suggests.
    fn output_color(&self) -> YuvColor {
        let Some(format) = self.decoder.codec.output_format() else { return self.color };
        let default = default_color(format.int32("height").unwrap_or(self.video.height));
        // MediaFormat's COLOR_STANDARD_* and COLOR_RANGE_*.
        let matrix = match format.int32("color-standard") {
            Some(1) => YuvMatrix::Bt709,
            Some(2 | 4) => YuvMatrix::Bt601,
            Some(6) => YuvMatrix::Bt2020,
            _ => default.matrix,
        };
        let range = match format.int32("color-range") {
            Some(1) => YuvRange::Full,
            Some(2) => YuvRange::Limited,
            _ => default.range,
        };
        YuvColor { matrix, range }
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

/// The bytes of `plane`, valid while its image lives.
///
/// # Safety
///
/// The plane's image is alive for `'a`.
unsafe fn plane_bytes<'a>(plane: &ndk::Plane) -> &'a [u8] {
    // SAFETY: the plane's data has `len` bytes while its image lives.
    unsafe { std::slice::from_raw_parts(plane.data, plane.len) }
}

/// Copies `rows` rows of `width` samples, `pixel_stride` bytes apart, from
/// `data`, whose rows are `row_stride` bytes apart, from sample `left` of
/// row `top`.
fn copy_samples(
    data: &[u8],
    (row_stride, pixel_stride): (usize, usize),
    (left, top): (usize, usize),
    (width, rows): (usize, usize),
) -> Result<CpuPlane, String> {
    let mut out = Vec::with_capacity(width * rows);
    for row in 0..rows {
        let start = (top + row) * row_stride + left * pixel_stride;
        let end = start + (width.max(1) - 1) * pixel_stride + 1;
        let samples = data.get(start..end).ok_or("an image plane is shorter than its rows")?;
        out.extend(samples.iter().step_by(pixel_stride.max(1)));
    }
    Ok(CpuPlane { data: out, stride: width })
}

/// The pixels of `image`, a 4:2:0 YUV image of `size` in readable memory:
/// three planes, or a plane of interleaved chroma where its chroma planes
/// are one.
fn copy_planes(image: &Image, size: Size, color: YuvColor) -> Result<FramePixels, String> {
    let (left, top) = image.crop_origin();
    let (width, height) = (size.width as usize, size.height as usize);
    let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
    let chroma_origin = (left / 2, top / 2);
    let (y, u, v) = (image.plane(0)?, image.plane(1)?, image.plane(2)?);
    // SAFETY: the planes' image lives until the copies are made.
    let (y_data, u_data, v_data) = unsafe { (plane_bytes(&y), plane_bytes(&u), plane_bytes(&v)) };
    let y = copy_samples(y_data, (y.row_stride, y.pixel_stride), (left, top), (width, height))?;
    let interleaved = u.pixel_stride == 2
        && v.pixel_stride == 2
        && u.row_stride == v.row_stride
        && v.data as usize == u.data as usize + 1;
    if interleaved {
        // The interleaved samples run from the first Cb to the last Cr.
        let len = v.len + 1;
        // SAFETY: the Cb and Cr planes are one, of the image's.
        let uv_data = unsafe { std::slice::from_raw_parts(u.data, len) };
        let uv = copy_samples(
            uv_data,
            (u.row_stride, 1),
            (chroma_origin.0 * 2, chroma_origin.1),
            (chroma_width * 2, chroma_height),
        )?;
        return Ok(FramePixels::Nv12 { y, uv, color });
    }
    let chroma = (chroma_width, chroma_height);
    let u = copy_samples(u_data, (u.row_stride, u.pixel_stride), chroma_origin, chroma)?;
    let v = copy_samples(v_data, (v.row_stride, v.pixel_stride), chroma_origin, chroma)?;
    Ok(FramePixels::I420 { y, u, v, color })
}
