//! The `AVFoundation` player of Apple platforms.
//!
//! Each video plays in an `AVPlayer`, which `AVFoundation` steps on the main
//! thread: the player, its items and their status live on the main queue,
//! which applies the [`VideoPlayer`]'s commands and loops the video at its
//! end. A thread of the video's own polls the current item's
//! `AVPlayerItemVideoOutput`, which any thread may read, for new frames. An
//! `AVAssetResourceLoader` delegate serves the video's bytes from a
//! [`ByteSource`] on a serial queue, so they go through the map's storage.
//!
//! A process whose main thread doesn't run its run loop, such as a test or a
//! command-line render, never plays a video: the player reports that
//! through the source's [`FrameSink`] once the main queue has been idle for
//! [`MAIN_QUEUE_TIMEOUT`].

#![expect(unsafe_code, reason = "AVFoundation through objc2 is the FFI here")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use mln_gfx::video::{CpuPlane, FramePixels, GpuFrame, VideoFrame, YuvColor, YuvMatrix, YuvRange};
use mln_storage::FileSource;
use mln_util::size::Size;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, MainThreadMarker, define_class, msg_send, sel};
use objc2_av_foundation::{
    AVAssetResourceLoader, AVAssetResourceLoaderDelegate, AVAssetResourceLoadingRequest, AVPlayer, AVPlayerItem,
    AVPlayerItemStatus, AVPlayerItemVideoOutput, AVURLAsset,
};
use objc2_core_foundation::CFString;
use objc2_core_media::{CMTime, CMTimeFlags, kCMTimeZero};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight,
    CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVImageBufferYCbCrMatrix_ITU_R_2020, kCVImageBufferYCbCrMatrixKey,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelFormatType_32BGRA, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, kCVReturnSuccess,
};
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSLocalizedDescriptionKey, NSNumber, NSString, NSURL, ns_string,
};
use parking_lot::Mutex;

use crate::bytes::ByteSource;
use crate::player::{OpenRequest, VideoPlayer, VideoPlayers};
use crate::slot::FrameSink;

/// The scheme of the URLs that the resource loader serves: `AVFoundation`
/// asks the delegate for the bytes of URLs of schemes it doesn't load.
const SCHEME: &str = "mln-video";
/// How often the frame thread polls before it knows the video's frame
/// rate, and how often it steps the player on the main queue.
const DEFAULT_POLL: Duration = Duration::from_millis(16);
/// The bounds of the poll interval, half a frame of the video.
const MIN_POLL: Duration = Duration::from_millis(4);
const MAX_POLL: Duration = Duration::from_millis(50);
/// How long the main queue may take to open the video before the player
/// gives up on it.
pub(crate) const MAIN_QUEUE_TIMEOUT: Duration = Duration::from_secs(5);
/// The most bytes that one response to a data request holds.
const RESPONSE_BYTES: usize = 256 * 1024;

unsafe extern "C" {
    /// The host clock that `itemTimeForMachAbsoluteTime:` maps, from
    /// libSystem.
    fn mach_absolute_time() -> u64;
}

/// The players of Apple platforms, on `AVFoundation`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AvFoundationPlayers;

impl VideoPlayers for AvFoundationPlayers {
    fn open(&self, request: OpenRequest) -> Result<Box<dyn VideoPlayer>, String> {
        let OpenRequest { urls, file_source, frames, .. } = request;
        if urls.is_empty() {
            return Err("a video source without URLs".to_owned());
        }
        let shared = Arc::new(Shared {
            frames,
            closed: AtomicBool::new(false),
            opened: AtomicBool::new(false),
            stepping: AtomicBool::new(false),
            sources: Mutex::default(),
            output: Mutex::default(),
            video: Mutex::default(),
        });
        let opening = Arc::clone(&shared);
        on_main(move |mtm| {
            if !opening.closed.load(Ordering::Relaxed) {
                let mut video = Video::new(mtm, urls, file_source);
                video.open_next(&opening);
                *opening.video.lock() = Some(Confined(video));
            }
            opening.opened.store(true, Ordering::Relaxed);
        });
        let thread = std::thread::Builder::new()
            .name("mln-video".to_owned())
            .spawn({
                let shared = Arc::clone(&shared);
                move || poll_frames(&shared)
            })
            .map_err(|error| format!("no thread for the video player: {error}"))?;
        Ok(Box::new(AvFoundationPlayer { shared, thread: Some(thread) }))
    }
}

/// Runs `work` on the main queue, after the work queued there before.
fn on_main(work: impl FnOnce(MainThreadMarker) + Send + 'static) {
    DispatchQueue::main().exec_async(move || {
        // SAFETY: the main queue runs on the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        autoreleasepool(|_| work(mtm));
    });
}

/// A value that blocks of one serial queue alone touch and drop: the main
/// queue for the player (`Shared::video`), a loader's queue for the loader
/// that a closed item lets go of.
struct Confined<T>(T);

// SAFETY: as above: one queue's blocks reach the value, one at a time.
unsafe impl<T> Send for Confined<T> {}
// SAFETY: as above.
unsafe impl<T> Sync for Confined<T> {}

/// What the player's handle, its frame thread and the main queue share.
struct Shared {
    frames: FrameSink,
    closed: AtomicBool,
    /// Whether the main queue opened the video.
    opened: AtomicBool,
    /// Whether a step of the player waits on the main queue.
    stepping: AtomicBool,
    /// The byte sources of the URLs opened so far, which closing cancels.
    sources: Mutex<Vec<Arc<ByteSource>>>,
    /// The current item's output, which the frame thread polls.
    output: Mutex<Option<Output>>,
    /// The player, on the main queue.
    video: Mutex<Option<Confined<Video>>>,
}

/// An item's video output, which any thread may poll for frames.
struct Output(Retained<AVPlayerItemVideoOutput>);

// SAFETY: AVPlayerItemVideoOutput's frame methods are for any thread, such
// as a display link's; the frame thread calls nothing else.
unsafe impl Send for Output {}

impl Shared {
    fn closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Runs `step` on the video on the main queue, unless it closed.
    fn on_video(self: &Arc<Shared>, step: impl FnOnce(&mut Video, &Arc<Shared>) + Send + 'static) {
        let shared = Arc::clone(self);
        on_main(move |_| {
            if shared.closed() {
                return;
            }
            let mut video = shared.video.lock().take();
            if let Some(Confined(video)) = &mut video {
                step(video, &shared);
            }
            *shared.video.lock() = video;
        });
    }
}

/// A playing video, until the handle drops.
struct AvFoundationPlayer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl VideoPlayer for AvFoundationPlayer {
    fn play(&mut self) {
        self.shared.on_video(|video, _| video.play(true));
    }

    fn pause(&mut self) {
        self.shared.on_video(|video, _| video.play(false));
    }

    fn seek(&mut self, seconds: f64) {
        self.shared.on_video(move |video, _| video.seek_to(seconds));
    }
}

impl Drop for AvFoundationPlayer {
    /// Stops the video: ends its reads, joins its frame thread, and has the
    /// main queue let go of the player, without waiting for it.
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Relaxed);
        for source in self.shared.sources.lock().iter() {
            source.cancel();
        }
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
        let shared = Arc::clone(&self.shared);
        on_main(move |_| {
            *shared.output.lock() = None;
            if let Some(Confined(video)) = shared.video.lock().take() {
                video.close();
            }
        });
    }
}

/// The frame thread: steps the player on the main queue and sends each new
/// frame of its output, until the player closes.
fn poll_frames(shared: &Arc<Shared>) {
    let started = Instant::now();
    let mut last_time: Option<f64> = None;
    let mut poll = DEFAULT_POLL;
    while !shared.closed() {
        if !shared.opened.load(Ordering::Relaxed) && started.elapsed() > MAIN_QUEUE_TIMEOUT {
            shared.frames.fail(
                "AVFoundation plays video only in a process whose main thread runs its run loop, and the main \
                 queue didn't open the video",
            );
            return;
        }
        // One step at a time waits on the main queue, once it opened the
        // video.
        if shared.opened.load(Ordering::Relaxed) && !shared.stepping.swap(true, Ordering::Relaxed) {
            shared.on_video(|video, shared| {
                video.step(shared);
                shared.stepping.store(false, Ordering::Relaxed);
            });
        }
        autoreleasepool(|_| {
            let Some(frame) = new_frame(shared) else {
                return;
            };
            if let (Some(time), Some(last)) = (frame.time, last_time)
                && time > last
            {
                poll = Duration::from_secs_f64((time - last) / 2.0).clamp(MIN_POLL, MAX_POLL);
            }
            last_time = frame.time;
            if !shared.closed() {
                shared.frames.push(frame);
            }
        });
        std::thread::park_timeout(poll);
    }
}

/// The output's frame for now, if it's one not sent yet.
fn new_frame(shared: &Shared) -> Option<VideoFrame> {
    let (buffer, shown) = {
        let output = shared.output.lock();
        let Output(output) = output.as_ref()?;
        // SAFETY: the output's frame methods, for any thread, with a valid
        // pointer for the time of the image.
        unsafe {
            let now = output.itemTimeForMachAbsoluteTime(mach_absolute_time().cast_signed());
            if !output.hasNewPixelBufferForItemTime(now) {
                return None;
            }
            let mut shown = now;
            output.copyPixelBufferForItemTime_itemTimeForDisplay(now, &raw mut shown).map(|buffer| (buffer, shown))
        }?
    };
    match frame(buffer, seconds(shown), !shared.frames.gpu_frames_refused()) {
        Ok(frame) => Some(frame),
        Err(message) => {
            shared.frames.fail(message);
            None
        }
    }
}

/// The URL being opened or played.
struct Item {
    url: String,
    item: Retained<AVPlayerItem>,
    /// The loader of a URL read through the map's storage, and its queue.
    loader: Option<(Retained<Loader>, DispatchRetained<DispatchQueue>)>,
    ready: bool,
}

impl Item {
    /// Lets go of the item. Its loader goes once its queue finishes what
    /// it serves.
    fn close(self) {
        if let Some((loader, queue)) = self.loader {
            let loader = Confined(loader);
            queue.exec_async(move || drop(loader));
        }
    }
}

/// A video's player and its state, on the main queue.
struct Video {
    mtm: MainThreadMarker,
    player: Retained<AVPlayer>,
    urls: std::vec::IntoIter<String>,
    file_source: Option<Arc<dyn FileSource>>,
    item: Option<Item>,
    /// Why each URL so far failed.
    failures: Vec<String>,
    playing: bool,
    /// A seek asked for before the item was ready.
    pending_seek: Option<f64>,
}

impl Video {
    fn new(mtm: MainThreadMarker, urls: Vec<String>, file_source: Option<Arc<dyn FileSource>>) -> Video {
        // SAFETY: an empty player, which plays nothing yet.
        let player = unsafe { AVPlayer::playerWithPlayerItem(None, mtm) };
        // SAFETY: plain property setters, the second one where the OS has
        // it.
        unsafe {
            player.setMuted(true);
            // A map's video is no reason to keep the display awake.
            if player.respondsToSelector(sel!(setPreventsDisplaySleepDuringVideoPlayback:)) {
                player.setPreventsDisplaySleepDuringVideoPlayback(false);
            }
        }
        Video {
            mtm,
            player,
            urls: urls.into_iter(),
            file_source,
            item: None,
            failures: Vec::new(),
            playing: true,
            pending_seek: None,
        }
    }

    /// Opens the next URL, or reports that none plays.
    fn open_next(&mut self, shared: &Shared) {
        if let Some(item) = self.item.take() {
            item.close();
        }
        *shared.output.lock() = None;
        // SAFETY: the player drops its item.
        unsafe { self.player.replaceCurrentItemWithPlayerItem(None) };
        let Some(url) = self.urls.next() else {
            shared.frames.fail(format!("no URL of the video plays: {}", self.failures.join("; ")));
            return;
        };
        match self.open(&url, shared) {
            Ok((item, output)) => {
                // SAFETY: the player takes the item, and plays it once it's
                // ready.
                unsafe {
                    self.player.replaceCurrentItemWithPlayerItem(Some(&item.item));
                    if self.playing {
                        self.player.play();
                    }
                }
                *shared.output.lock() = Some(Output(output));
                self.item = Some(item);
            }
            Err(message) => {
                self.failures.push(format!("{url}: {message}"));
                self.open_next(shared);
            }
        }
    }

    fn open(&self, url: &str, shared: &Shared) -> Result<(Item, Retained<AVPlayerItemVideoOutput>), String> {
        let (asset, loader) = if let Some(file_source) = &self.file_source {
            let source = Arc::new(ByteSource::new(Arc::clone(file_source), url));
            shared.sources.lock().push(Arc::clone(&source));
            if shared.closed() {
                source.cancel();
            }
            let extension = extension(url);
            let dot = if extension.is_empty() { "" } else { "." };
            let loader_url = format!("{SCHEME}://video/video{dot}{extension}");
            let ns_url = NSURL::URLWithString(&NSString::from_str(&loader_url))
                .ok_or_else(|| format!("no URL of {loader_url}"))?;
            // SAFETY: a URL asset of a valid URL, without options.
            let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&ns_url, None) };
            let loader = Loader::new(source, content_type(extension));
            let queue = DispatchQueue::new("mln-video-loader", DispatchQueueAttr::SERIAL);
            // SAFETY: the item keeps the delegate, which the loader holds
            // weakly, and the queue, alive.
            unsafe {
                asset.resourceLoader().setDelegate_queue(Some(ProtocolObject::from_ref(&*loader)), Some(&queue));
            }
            (asset, Some((loader, queue)))
        } else {
            let ns_url = NSURL::URLWithString(&NSString::from_str(url)).ok_or("not a URL")?;
            // SAFETY: as above.
            (unsafe { AVURLAsset::URLAssetWithURL_options(&ns_url, None) }, None)
        };
        // SAFETY: an item of the asset, on the main thread.
        let item = unsafe { AVPlayerItem::playerItemWithAsset(&asset, self.mtm) };
        let output = video_output();
        // SAFETY: a new output, of no item yet.
        unsafe { item.addOutput(&output) };
        Ok((Item { url: url.to_owned(), item, loader, ready: false }, output))
    }

    fn play(&mut self, playing: bool) {
        self.playing = playing;
        // SAFETY: plays or pauses the current item, or the next one once
        // ready.
        unsafe {
            if playing {
                self.player.play();
            } else {
                self.player.pause();
            }
        }
    }

    fn seek_to(&mut self, seconds: f64) {
        if self.item.as_ref().is_some_and(|item| item.ready) {
            self.seek(seconds);
        } else {
            self.pending_seek = Some(seconds);
        }
    }

    /// Seeks to the frame at `seconds`, exactly, so that a paused video
    /// shows that frame.
    fn seek(&self, seconds: f64) {
        // SAFETY: CoreMedia's zero time, and a time of a finite number.
        unsafe {
            let time = if seconds.is_finite() { CMTime::with_seconds(seconds.max(0.0), 600) } else { kCMTimeZero };
            self.player.seekToTime_toleranceBefore_toleranceAfter(time, kCMTimeZero, kCMTimeZero);
        }
    }

    /// Follows the item's status: tries the next URL once it fails, seeks
    /// where asked once it's ready, and loops it at its end.
    fn step(&mut self, shared: &Shared) {
        let Some(item) = &mut self.item else {
            return;
        };
        // SAFETY: property reads of the item.
        let status = unsafe { item.item.status() };
        if status == AVPlayerItemStatus::Failed {
            // SAFETY: as above.
            let message = unsafe { item.item.error() }
                .map_or_else(|| "the video failed".to_owned(), |error| error.localizedDescription().to_string());
            let url = item.url.clone();
            self.failures.push(format!("{url}: {message}"));
            self.open_next(shared);
            return;
        }
        if status != AVPlayerItemStatus::ReadyToPlay {
            return;
        }
        if !item.ready {
            item.ready = true;
            if let Some(seconds) = self.pending_seek.take() {
                self.seek(seconds);
            }
        }
        let Some(item) = &self.item else {
            return;
        };
        // SAFETY: property reads of the item and the player.
        let (now, duration, rate) = unsafe { (item.item.currentTime(), item.item.duration(), self.player.rate()) };
        let ended = seconds(duration).is_some_and(|end| seconds(now).is_some_and(|time| time >= end));
        if self.playing && rate == 0.0 && ended {
            self.seek(0.0);
            // SAFETY: plays the item again from its start.
            unsafe { self.player.play() };
        }
    }

    /// Stops the video and lets go of its item.
    fn close(mut self) {
        // SAFETY: the player stops and drops its item.
        unsafe {
            self.player.pause();
            self.player.replaceCurrentItemWithPlayerItem(None);
        }
        if let Some(item) = self.item.take() {
            item.close();
        }
    }
}

/// An output of IOSurface-backed, Metal-compatible NV12 frames.
fn video_output() -> Retained<AVPlayerItemVideoOutput> {
    // SAFETY: CoreVideo's constant keys, which are NSStrings too.
    let keys: [&NSString; 3] = unsafe {
        [
            cf_as_ns(kCVPixelBufferPixelFormatTypeKey),
            cf_as_ns(kCVPixelBufferIOSurfacePropertiesKey),
            cf_as_ns(kCVPixelBufferMetalCompatibilityKey),
        ]
    };
    let format = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
    let surface = NSDictionary::<NSString, AnyObject>::new();
    let metal = NSNumber::new_bool(true);
    let values: [&AnyObject; 3] = [&format, &surface, &metal];
    let attributes = NSDictionary::from_slices(&keys, &values);
    // SAFETY: pixel buffer attributes of CoreVideo's keys and values.
    unsafe {
        AVPlayerItemVideoOutput::initWithPixelBufferAttributes(AVPlayerItemVideoOutput::alloc(), Some(&attributes))
    }
}

/// `string`, a CoreFoundation string, as the `NSString` it bridges to.
fn cf_as_ns(string: &CFString) -> &NSString {
    // SAFETY: CFString and NSString are toll-free bridged.
    unsafe { &*std::ptr::from_ref(string).cast::<NSString>() }
}

/// The time in seconds, if it's a number.
fn seconds(time: CMTime) -> Option<f64> {
    let valid = time.flags.contains(CMTimeFlags::Valid)
        && !time
            .flags
            .intersects(CMTimeFlags::PositiveInfinity | CMTimeFlags::NegativeInfinity | CMTimeFlags::Indefinite);
    // SAFETY: a plain conversion of a valid time.
    (valid && time.timescale > 0).then(|| unsafe { time.seconds() })
}

/// The frame of `buffer`: the buffer itself, `gpu`, or its pixels copied.
fn frame(buffer: Retained<CVPixelBuffer>, time: Option<f64>, gpu: bool) -> Result<VideoFrame, String> {
    let size = Size::new(CVPixelBufferGetWidth(&buffer) as u32, CVPixelBufferGetHeight(&buffer) as u32);
    let format = CVPixelBufferGetPixelFormatType(&buffer);
    let yuv = format == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        || format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;
    if !yuv && format != kCVPixelFormatType_32BGRA {
        return Err(format!("frames of pixel format {:?}", String::from_utf8_lossy(&format.to_be_bytes())));
    }
    let color = YuvColor {
        matrix: yuv_matrix(&buffer),
        range: if format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange {
            YuvRange::Full
        } else {
            YuvRange::Limited
        },
    };
    let pixels = if gpu {
        // The frame owns the buffer's retain, which its drop releases.
        FramePixels::Gpu(GpuFrame::CvPixelBuffer { buffer: Retained::into_raw(buffer).cast(), color })
    } else {
        let locked = Locked::new(&buffer)?;
        if yuv {
            FramePixels::Nv12 { y: locked.plane(0), uv: locked.plane(1), color }
        } else {
            let plane = locked.plane(0);
            let row_bytes = size.width as usize * 4;
            let mut data = Vec::with_capacity(row_bytes * size.height as usize);
            for row in plane.data.chunks(plane.stride).take(size.height as usize) {
                data.extend(
                    row[..row_bytes].as_chunks::<4>().0.iter().flat_map(|bgra| [bgra[2], bgra[1], bgra[0], bgra[3]]),
                );
            }
            FramePixels::Rgba(CpuPlane { data, stride: row_bytes })
        }
    };
    Ok(VideoFrame { size, pixels, time })
}

/// The matrix that `buffer`'s attachment names, BT.709 by default.
fn yuv_matrix(buffer: &CVPixelBuffer) -> YuvMatrix {
    // SAFETY: CoreVideo's constant strings, and no mode pointer.
    let value = unsafe { buffer.attachment(kCVImageBufferYCbCrMatrixKey, std::ptr::null_mut()) };
    let Some(name) = value.as_deref().and_then(|value| value.downcast_ref::<CFString>()) else {
        return YuvMatrix::Bt709;
    };
    // SAFETY: as above.
    let (bt601, bt2020) = unsafe { (kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVImageBufferYCbCrMatrix_ITU_R_2020) };
    if name == bt601 {
        YuvMatrix::Bt601
    } else if name == bt2020 {
        YuvMatrix::Bt2020
    } else {
        YuvMatrix::Bt709
    }
}

/// A pixel buffer locked for reading, which unlocks as it drops.
struct Locked<'a>(&'a CVPixelBuffer);

impl<'a> Locked<'a> {
    fn new(buffer: &'a CVPixelBuffer) -> Result<Locked<'a>, String> {
        // SAFETY: a valid buffer, which the drop unlocks.
        let status = unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
        if status == kCVReturnSuccess { Ok(Locked(buffer)) } else { Err(format!("locking a frame failed ({status})")) }
    }

    /// A copy of plane `index`'s rows.
    fn plane(&self, index: usize) -> CpuPlane {
        let stride = CVPixelBufferGetBytesPerRowOfPlane(self.0, index);
        let rows = CVPixelBufferGetHeightOfPlane(self.0, index);
        let base = CVPixelBufferGetBaseAddressOfPlane(self.0, index).cast::<u8>();
        if base.is_null() {
            return CpuPlane { data: Vec::new(), stride };
        }
        // SAFETY: the locked plane has `rows` rows of `stride` bytes.
        let data = unsafe { std::slice::from_raw_parts(base, stride * rows) }.to_vec();
        CpuPlane { data, stride }
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        // SAFETY: locked in `new` with the same flags.
        unsafe { CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags::ReadOnly) };
    }
}

/// The extension of `url`'s path, lowercase.
fn extension(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let name = path.rsplit('/').next().unwrap_or_default();
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && extension.chars().all(|c| c.is_ascii_alphanumeric()) => {
            extension
        }
        _ => "",
    }
}

/// The uniform type identifier of a video of `extension`, MPEG-4 when it's
/// none `AVFoundation` knows by name here.
fn content_type(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        "mov" | "qt" => "com.apple.quicktime-movie",
        "m4v" => "com.apple.m4v-video",
        "3gp" | "3gpp" => "public.3gpp",
        "3g2" => "public.3gpp2",
        "webm" => "org.webmproject.webm",
        "ts" | "mts" | "m2ts" => "public.mpeg-2-transport-stream",
        _ => "public.mpeg-4",
    }
}

/// What the loader serves: the video's bytes and its type.
struct LoaderIvars {
    source: Arc<ByteSource>,
    content_type: &'static str,
}

define_class!(
    /// Serves an asset's loading requests from a [`ByteSource`], each in
    /// full, on the loader's serial queue.
    #[unsafe(super(NSObject))]
    #[name = "MlnRsVideoResourceLoader"]
    #[ivars = LoaderIvars]
    struct Loader;

    unsafe impl NSObjectProtocol for Loader {}

    unsafe impl AVAssetResourceLoaderDelegate for Loader {
        #[unsafe(method(resourceLoader:shouldWaitForLoadingOfRequestedResource:))]
        fn should_wait(&self, _loader: &AVAssetResourceLoader, request: &AVAssetResourceLoadingRequest) -> bool {
            autoreleasepool(|_| match self.serve(request) {
                // SAFETY: the request's own completion, once.
                Ok(()) => unsafe { request.finishLoading() },
                Err(message) => {
                    let error = loading_error(&message);
                    // SAFETY: as above.
                    unsafe { request.finishLoadingWithError(Some(&error)) };
                }
            });
            true
        }
    }
);

impl Loader {
    fn new(source: Arc<ByteSource>, content_type: &'static str) -> Retained<Loader> {
        let this = Loader::alloc().set_ivars(LoaderIvars { source, content_type });
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }

    /// Answers `request`'s content information and data requests.
    fn serve(&self, request: &AVAssetResourceLoadingRequest) -> Result<(), String> {
        let LoaderIvars { source, content_type } = self.ivars();
        let size = source.size().map_err(|error| error.to_string())?;
        // SAFETY: property reads and writes of the request, on the queue
        // its loader calls the delegate on.
        unsafe {
            if let Some(info) = request.contentInformationRequest() {
                let size = size.ok_or("the video's size isn't known")?;
                info.setContentType(Some(&NSString::from_str(content_type)));
                info.setContentLength(i64::try_from(size).map_err(|_| "a video too large")?);
                info.setByteRangeAccessSupported(true);
            }
            let Some(data) = request.dataRequest() else {
                return Ok(());
            };
            let mut offset = u64::try_from(data.currentOffset()).unwrap_or(0);
            let end = if data.requestsAllDataToEndOfResource() {
                size.unwrap_or(u64::MAX)
            } else {
                let requested = u64::try_from(data.requestedOffset()).unwrap_or(0);
                requested.saturating_add(u64::try_from(data.requestedLength()).unwrap_or(0))
            };
            let mut buffer = vec![0; RESPONSE_BYTES];
            while offset < end {
                if request.isCancelled() {
                    return Ok(());
                }
                let want = usize::try_from(end - offset).map_or(buffer.len(), |left| left.min(buffer.len()));
                let read = source.read_at(offset, &mut buffer[..want]).map_err(|error| error.to_string())?;
                if read == 0 {
                    break;
                }
                data.respondWithData(&NSData::with_bytes(&buffer[..read]));
                offset += read as u64;
            }
        }
        Ok(())
    }
}

/// An error of a loading request, which `AVFoundation` shows as the item's.
fn loading_error(message: &str) -> Retained<NSError> {
    let description = NSString::from_str(message);
    // SAFETY: Foundation's constant key.
    let key = unsafe { NSLocalizedDescriptionKey };
    let value: &AnyObject = &description;
    let info = NSDictionary::<NSString, AnyObject>::from_slices(&[key], &[value]);
    // SAFETY: an error of a domain, a code and a user info of strings.
    unsafe { NSError::errorWithDomain_code_userInfo(ns_string!("MlnRsVideo"), -1, Some(&info)) }
}
