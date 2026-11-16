//! Owners of the `OpenHarmony` `AVCodec` (source, demuxer and video decoder),
//! native image and native buffer objects that the player uses, which destroy
//! them when dropped.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ohos_media_sys::avbuffer::{OH_AVBuffer, OH_AVBuffer_GetAddr, OH_AVBuffer_GetBufferAttr, OH_AVBuffer_GetCapacity};
use ohos_media_sys::avbuffer_info::{OH_AVCodecBufferAttr, OH_AVCodecBufferFlags};
use ohos_media_sys::avcodec_base::{
    OH_AVCodec, OH_AVDataSourceExt, OH_AVSeekMode, OH_MD_KEY_CODEC_MIME, OH_MD_KEY_DURATION,
    OH_MD_KEY_ENABLE_SYNC_MODE, OH_MD_KEY_HEIGHT, OH_MD_KEY_MATRIX_COEFFICIENTS, OH_MD_KEY_PIXEL_FORMAT,
    OH_MD_KEY_RANGE_FLAG, OH_MD_KEY_TRACK_COUNT, OH_MD_KEY_TRACK_TYPE, OH_MD_KEY_VIDEO_CROP_LEFT,
    OH_MD_KEY_VIDEO_CROP_TOP, OH_MD_KEY_VIDEO_PIC_HEIGHT, OH_MD_KEY_VIDEO_PIC_WIDTH, OH_MD_KEY_VIDEO_SLICE_HEIGHT,
    OH_MD_KEY_VIDEO_STRIDE, OH_MD_KEY_WIDTH, OH_MediaType,
};
use ohos_media_sys::avcodec_videodecoder::{
    OH_VideoDecoder_Configure, OH_VideoDecoder_CreateByMime, OH_VideoDecoder_Destroy, OH_VideoDecoder_Flush,
    OH_VideoDecoder_FreeOutputBuffer, OH_VideoDecoder_GetInputBuffer, OH_VideoDecoder_GetOutputBuffer,
    OH_VideoDecoder_GetOutputDescription, OH_VideoDecoder_Prepare, OH_VideoDecoder_PushInputBuffer,
    OH_VideoDecoder_QueryInputBuffer, OH_VideoDecoder_QueryOutputBuffer, OH_VideoDecoder_RenderOutputBuffer,
    OH_VideoDecoder_SetSurface, OH_VideoDecoder_Start, OH_VideoDecoder_Stop,
};
use ohos_media_sys::avdemuxer::{
    OH_AVDemuxer, OH_AVDemuxer_CreateWithSource, OH_AVDemuxer_Destroy, OH_AVDemuxer_ReadSampleBuffer,
    OH_AVDemuxer_SeekToTime, OH_AVDemuxer_SelectTrackByID,
};
use ohos_media_sys::averrors::OH_AVErrCode;
use ohos_media_sys::avformat::{
    OH_AVFormat, OH_AVFormat_Copy, OH_AVFormat_Create, OH_AVFormat_Destroy, OH_AVFormat_GetIntValue,
    OH_AVFormat_GetLongValue, OH_AVFormat_GetStringValue, OH_AVFormat_SetIntValue, OH_AVPixelFormat,
};
use ohos_media_sys::avsource::{
    OH_AVSource, OH_AVSource_CreateWithDataSourceExt, OH_AVSource_Destroy, OH_AVSource_GetSourceFormat,
    OH_AVSource_GetTrackFormat,
};
use ohos_sys_opaque_types::{OH_NativeBuffer, OH_NativeImage, OHNativeWindow, OHNativeWindowBuffer};
use ohos_window_sys::OHNativeErrorCode;
use ohos_window_sys::native_buffer::native_buffer::{
    OH_NativeBuffer_Config, OH_NativeBuffer_FromNativeWindowBuffer, OH_NativeBuffer_GetConfig,
    OH_NativeBuffer_Reference, OH_NativeBuffer_Usage,
};
use ohos_window_sys::native_image::{
    OH_ConsumerSurface_Create, OH_ConsumerSurface_SetDefaultSize, OH_ConsumerSurface_SetDefaultUsage,
    OH_NativeImage_AcquireNativeWindow, OH_NativeImage_AcquireNativeWindowBuffer, OH_NativeImage_Destroy,
    OH_NativeImage_ReleaseNativeWindowBuffer, OH_NativeImage_SetOnFrameAvailableListener, OH_OnFrameAvailableListener,
};
use parking_lot::{Condvar, Mutex};

use crate::bytes::ByteSource;

pub(super) const END_OF_STREAM: u32 = OH_AVCodecBufferFlags::EOS.0;
const MEDIA_TYPE_VIDEO: i32 = OH_MediaType::MEDIA_TYPE_VID.0.cast_signed();
pub(super) const PIXEL_FORMAT_I420: i32 = OH_AVPixelFormat::AV_PIXEL_FORMAT_YUVI420.0.cast_signed();
pub(super) const PIXEL_FORMAT_NV12: i32 = OH_AVPixelFormat::AV_PIXEL_FORMAT_NV12.0.cast_signed();
pub(super) const PIXEL_FORMAT_NV21: i32 = OH_AVPixelFormat::AV_PIXEL_FORMAT_NV21.0.cast_signed();
pub(super) const PIXEL_FORMAT_RGBA: i32 = OH_AVPixelFormat::AV_PIXEL_FORMAT_RGBA.0.cast_signed();
const NATIVE_OK: i32 = OHNativeErrorCode::NATIVE_ERROR_OK.0.cast_signed();
const NATIVE_NO_BUFFER: i32 = OHNativeErrorCode::NATIVE_ERROR_NO_BUFFER.0.cast_signed();
/// What a readAt callback returns for the end of the data.
const READ_END: i32 = -1;
/// What a readAt callback returns for a read that failed.
const READ_FAILED: i32 = -2;

pub(super) type BufferAttr = OH_AVCodecBufferAttr;
pub(super) type NativeBuffer = OH_NativeBuffer;

/// A buffer's attributes before a call fills them.
fn empty_attr() -> BufferAttr {
    BufferAttr { pts: 0, size: 0, offset: 0, flags: 0 }
}

/// The format keys that the player reads.
#[derive(Clone, Copy, Debug)]
pub(super) enum Key {
    TrackCount,
    TrackType,
    Width,
    Height,
    PixelFormat,
    Stride,
    SliceHeight,
    PictureWidth,
    PictureHeight,
    CropLeft,
    CropTop,
    RangeFlag,
    MatrixCoefficients,
}

impl Key {
    fn name(self) -> *const c_char {
        // SAFETY: the codec base library's key names, which it sets before
        // any code runs and never changes.
        unsafe {
            match self {
                Key::TrackCount => OH_MD_KEY_TRACK_COUNT,
                Key::TrackType => OH_MD_KEY_TRACK_TYPE,
                Key::Width => OH_MD_KEY_WIDTH,
                Key::Height => OH_MD_KEY_HEIGHT,
                Key::PixelFormat => OH_MD_KEY_PIXEL_FORMAT,
                Key::Stride => OH_MD_KEY_VIDEO_STRIDE,
                Key::SliceHeight => OH_MD_KEY_VIDEO_SLICE_HEIGHT,
                Key::PictureWidth => OH_MD_KEY_VIDEO_PIC_WIDTH,
                Key::PictureHeight => OH_MD_KEY_VIDEO_PIC_HEIGHT,
                Key::CropLeft => OH_MD_KEY_VIDEO_CROP_LEFT,
                Key::CropTop => OH_MD_KEY_VIDEO_CROP_TOP,
                Key::RangeFlag => OH_MD_KEY_RANGE_FLAG,
                Key::MatrixCoefficients => OH_MD_KEY_MATRIX_COEFFICIENTS,
            }
        }
    }
}

/// Describes a failed call.
fn check(status: OH_AVErrCode, what: &str) -> Result<(), String> {
    if status == OH_AVErrCode::AV_ERR_OK { Ok(()) } else { Err(format!("{what} failed: {}", status.0)) }
}

/// What a data source's callback reads: the video's bytes, and why a read
/// failed last.
struct Reader {
    bytes: Arc<ByteSource>,
    error: Mutex<Option<io::Error>>,
}

unsafe extern "C" fn read_at(buffer: *mut OH_AVBuffer, length: i32, offset: i64, userdata: *mut c_void) -> i32 {
    // SAFETY: the source's reader, which outlives it.
    let reader = unsafe { &*userdata.cast::<Reader>() };
    let (Ok(offset), Ok(length)) = (u64::try_from(offset), usize::try_from(length)) else { return READ_FAILED };
    // SAFETY: the buffer the source reads into, which it doesn't touch
    // until the call returns, of at least `length` bytes.
    let data = unsafe {
        let address = OH_AVBuffer_GetAddr(buffer);
        let capacity = usize::try_from(OH_AVBuffer_GetCapacity(buffer)).unwrap_or(0);
        if address.is_null() {
            return READ_FAILED;
        }
        std::slice::from_raw_parts_mut(address, length.min(capacity))
    };
    // A short read ends the data, so a read fills the buffer but at the end
    // of the video.
    match reader.bytes.read_full(offset, data) {
        Ok(0) if !data.is_empty() => READ_END,
        Ok(count) => i32::try_from(count).unwrap_or(READ_FAILED),
        Err(error) => {
            *reader.error.lock() = Some(error);
            READ_FAILED
        }
    }
}

/// An `OH_AVSource` that reads a [`ByteSource`].
pub(super) struct AvSource {
    raw: NonNull<OH_AVSource>,
    /// What the source reads through, which outlives it.
    _desc: Box<OH_AVDataSourceExt>,
    reader: Box<Reader>,
}

// SAFETY: the AVCodec objects may be used from any thread, one at a time.
unsafe impl Send for AvSource {}

impl AvSource {
    /// A source of `bytes`' video. Blocks while it reads the container's
    /// header.
    pub(super) fn new(bytes: Arc<ByteSource>) -> Result<AvSource, String> {
        let size = bytes.size().map_err(|error| error.to_string())?;
        // An unknown size is a stream's.
        let size = size.and_then(|size| i64::try_from(size).ok()).unwrap_or(-1);
        let reader = Box::new(Reader { bytes, error: Mutex::new(None) });
        let mut desc = Box::new(OH_AVDataSourceExt { size, readAt: Some(read_at) });
        // SAFETY: the description and the reader outlive the source.
        let raw =
            unsafe { OH_AVSource_CreateWithDataSourceExt(&raw mut *desc, (&raw const *reader).cast_mut().cast()) };
        let Some(raw) = NonNull::new(raw) else {
            let error = reader.error.lock().take();
            return Err(match error {
                Some(error) => format!("reading the video's container failed: {error}"),
                None => "reading the video's container failed".to_owned(),
            });
        };
        Ok(AvSource { raw, _desc: desc, reader })
    }

    /// Why a read failed last.
    pub(super) fn take_error(&self) -> Option<io::Error> {
        self.reader.error.lock().take()
    }

    fn source_format(&self) -> Option<Format> {
        // SAFETY: the source is alive; the format is the caller's.
        let raw = NonNull::new(unsafe { OH_AVSource_GetSourceFormat(self.raw.as_ptr()) })?;
        Some(Format { raw })
    }

    fn track_count(&self) -> u32 {
        let count = self.source_format().and_then(|format| format.int(Key::TrackCount));
        count.and_then(|count| u32::try_from(count).ok()).unwrap_or(0)
    }

    /// The video's duration in microseconds, if the container says.
    fn duration_us(&self) -> Option<i64> {
        let format = self.source_format()?;
        let mut value = 0;
        // SAFETY: the format is alive, and the key a C string.
        let found = unsafe { OH_AVFormat_GetLongValue(format.raw.as_ptr(), OH_MD_KEY_DURATION, &raw mut value) };
        found.then_some(value).filter(|duration| *duration > 0)
    }

    /// The first video track, and its format.
    pub(super) fn video_track(&self) -> Option<(u32, Format)> {
        (0..self.track_count()).find_map(|track| {
            // SAFETY: the source is alive; the format is the caller's.
            let raw = NonNull::new(unsafe { OH_AVSource_GetTrackFormat(self.raw.as_ptr(), track) })?;
            let format = Format { raw };
            (format.int(Key::TrackType) == Some(MEDIA_TYPE_VIDEO)).then_some((track, format))
        })
    }
}

impl Drop for AvSource {
    fn drop(&mut self) {
        // SAFETY: the source is this one's, and no demuxer reads it any more.
        unsafe { OH_AVSource_Destroy(self.raw.as_ptr()) };
    }
}

/// An `OH_AVFormat`.
pub(super) struct Format {
    raw: NonNull<OH_AVFormat>,
}

// SAFETY: as for `AvSource`.
unsafe impl Send for Format {}

impl Format {
    /// `key`'s value, of a key whose name the codec base library holds.
    fn int_key(&self, key: *const c_char) -> Option<i32> {
        let mut value = 0;
        // SAFETY: the format is alive, and the key a C string.
        unsafe { OH_AVFormat_GetIntValue(self.raw.as_ptr(), key, &raw mut value) }.then_some(value)
    }

    pub(super) fn int(&self, key: Key) -> Option<i32> {
        self.int_key(key.name())
    }

    pub(super) fn mime(&self) -> Option<String> {
        let mut value = std::ptr::null();
        // SAFETY: as for `int`; the string is the format's, and copied
        // before it goes.
        unsafe {
            if !OH_AVFormat_GetStringValue(self.raw.as_ptr(), OH_MD_KEY_CODEC_MIME, &raw mut value) || value.is_null() {
                return None;
            }
            Some(CStr::from_ptr(value).to_string_lossy().into_owned())
        }
    }

    /// A copy of the format.
    fn copy(&self) -> Result<Format, String> {
        // SAFETY: no preconditions; then both formats are alive.
        unsafe {
            let raw = NonNull::new(OH_AVFormat_Create()).ok_or("OH_AVFormat_Create failed")?;
            let copy = Format { raw };
            if !OH_AVFormat_Copy(raw.as_ptr(), self.raw.as_ptr()) {
                return Err("copying a format failed".to_owned());
            }
            Ok(copy)
        }
    }

    /// Sets `key`, of a key whose name the codec base library holds.
    fn set_int(&self, key: *const c_char, value: i32) {
        // SAFETY: the format is alive, and the key a C string.
        unsafe { OH_AVFormat_SetIntValue(self.raw.as_ptr(), key, value) };
    }
}

impl Drop for Format {
    fn drop(&mut self) {
        // SAFETY: the format is this one's.
        unsafe { OH_AVFormat_Destroy(self.raw.as_ptr()) };
    }
}

/// An `OH_AVDemuxer` of one track.
pub(super) struct Demuxer {
    raw: NonNull<OH_AVDemuxer>,
    track: u32,
    /// The video's duration in microseconds, past which seeks fail.
    duration_us: Option<i64>,
}

// SAFETY: as for `AvSource`.
unsafe impl Send for Demuxer {}

impl Demuxer {
    /// A demuxer of `source`'s `track`; the source must outlive it.
    pub(super) fn new(source: &AvSource, track: u32) -> Result<Demuxer, String> {
        // SAFETY: the source is alive, and the caller keeps it.
        let raw = NonNull::new(unsafe { OH_AVDemuxer_CreateWithSource(source.raw.as_ptr()) })
            .ok_or("OH_AVDemuxer_CreateWithSource failed")?;
        let demuxer = Demuxer { raw, track, duration_us: source.duration_us() };
        // SAFETY: the demuxer is alive.
        check(unsafe { OH_AVDemuxer_SelectTrackByID(raw.as_ptr(), track) }, "selecting the video track")?;
        Ok(demuxer)
    }

    /// Reads the next sample into `buffer`, a decoder's input buffer, with
    /// its time and flags, the end of the track's among them.
    fn read_sample(&self, buffer: NonNull<OH_AVBuffer>) -> Result<(), String> {
        // SAFETY: the demuxer is alive, and the buffer is the caller's.
        let status = unsafe { OH_AVDemuxer_ReadSampleBuffer(self.raw.as_ptr(), self.track, buffer.as_ptr()) };
        check(status, "reading a sample")
    }

    /// Moves to the sync sample at or before `time_us`, or the last one
    /// for a time after the end.
    pub(super) fn seek_to(&self, time_us: i64) -> Result<(), String> {
        let time_us = self.duration_us.map_or(time_us, |duration| time_us.min(duration - 1));
        // SAFETY: the demuxer is alive.
        let mode = OH_AVSeekMode::SEEK_MODE_PREVIOUS_SYNC;
        let status = unsafe { OH_AVDemuxer_SeekToTime(self.raw.as_ptr(), time_us / 1000, mode) };
        check(status, "seeking")
    }
}

impl Drop for Demuxer {
    fn drop(&mut self) {
        // SAFETY: the demuxer is this one's.
        unsafe { OH_AVDemuxer_Destroy(self.raw.as_ptr()) };
    }
}

/// What a decoder gave from its output.
pub(super) enum Output {
    /// A decoded buffer, by its index, which goes back to the decoder with
    /// [`Decoder::render_output`] or [`Decoder::free_output`].
    Buffer(u32, BufferAttr),
    /// The output's format changed.
    FormatChanged,
    /// Nothing yet.
    None,
}

/// A started `OH_AVCodec` video decoder in synchronous mode.
pub(super) struct Decoder {
    raw: NonNull<OH_AVCodec>,
}

// SAFETY: as for `AvSource`.
unsafe impl Send for Decoder {}

impl Decoder {
    /// A decoder of `format`'s track into `pixel_format` (an
    /// `OH_AVPixelFormat`), or the decoder's own, that renders into
    /// `window`, which must outlive it, or, without one, decodes into
    /// memory.
    pub(super) fn new(
        format: &Format,
        pixel_format: Option<i32>,
        window: Option<NonNull<OHNativeWindow>>,
    ) -> Result<Decoder, String> {
        let mime = format.mime().ok_or("the video track has no MIME type")?;
        let mime_c = CString::new(mime.clone()).map_err(|error| error.to_string())?;
        // SAFETY: a C string.
        let raw = NonNull::new(unsafe { OH_VideoDecoder_CreateByMime(mime_c.as_ptr()) })
            .ok_or_else(|| format!("no decoder of {mime}"))?;
        let decoder = Decoder { raw };
        let config = format.copy()?;
        // SAFETY: as for `Key::name`.
        config.set_int(unsafe { OH_MD_KEY_ENABLE_SYNC_MODE }, 1);
        if let Some(pixel_format) = pixel_format {
            config.set_int(Key::PixelFormat.name(), pixel_format);
        }
        // SAFETY: the decoder is new, and the format alive.
        let configured = unsafe { OH_VideoDecoder_Configure(raw.as_ptr(), config.raw.as_ptr()) };
        check(configured, &format!("configuring the {mime} decoder"))?;
        if let Some(window) = window {
            // SAFETY: the decoder is configured, and the caller keeps the
            // window.
            let set = unsafe { OH_VideoDecoder_SetSurface(raw.as_ptr(), window.as_ptr()) };
            check(set, "setting the decoder's surface")?;
        }
        // SAFETY: the decoder is configured.
        unsafe {
            check(OH_VideoDecoder_Prepare(raw.as_ptr()), &format!("preparing the {mime} decoder"))?;
            check(OH_VideoDecoder_Start(raw.as_ptr()), &format!("starting the {mime} decoder"))?;
        }
        Ok(decoder)
    }

    /// Takes back the buffers the decoder holds, for a seek, and starts it
    /// again.
    pub(super) fn flush(&self) -> Result<(), String> {
        // SAFETY: the decoder is alive.
        unsafe {
            check(OH_VideoDecoder_Flush(self.raw.as_ptr()), "flushing the decoder")?;
            check(OH_VideoDecoder_Start(self.raw.as_ptr()), "starting the decoder again")
        }
    }

    /// Fills a free input buffer with `demuxer`'s next sample: whether one
    /// was free, and whether the sample ended the track.
    pub(super) fn queue_input(&self, demuxer: &Demuxer) -> Result<Option<bool>, String> {
        let mut index = 0;
        // SAFETY: the decoder is alive.
        let status = unsafe { OH_VideoDecoder_QueryInputBuffer(self.raw.as_ptr(), &raw mut index, 0) };
        if status == OH_AVErrCode::AV_ERR_TRY_AGAIN_LATER {
            return Ok(None);
        }
        check(status, "querying an input buffer")?;
        // SAFETY: an input buffer the decoder gave, which is the caller's
        // until it is pushed.
        let buffer = NonNull::new(unsafe { OH_VideoDecoder_GetInputBuffer(self.raw.as_ptr(), index) })
            .ok_or("the decoder has no input buffer")?;
        demuxer.read_sample(buffer)?;
        let mut attr = empty_attr();
        // SAFETY: as above.
        unsafe {
            check(OH_AVBuffer_GetBufferAttr(buffer.as_ptr(), &raw mut attr), "reading a sample's attributes")?;
            check(OH_VideoDecoder_PushInputBuffer(self.raw.as_ptr(), index), "queueing a sample")?;
        }
        Ok(Some(attr.flags & END_OF_STREAM != 0))
    }

    /// The next output, waiting up to `timeout` for one.
    pub(super) fn dequeue_output(&self, timeout: Duration) -> Result<Output, String> {
        let mut index = 0;
        let timeout = i64::try_from(timeout.as_micros()).unwrap_or(i64::MAX);
        // SAFETY: the decoder is alive.
        let status = unsafe { OH_VideoDecoder_QueryOutputBuffer(self.raw.as_ptr(), &raw mut index, timeout) };
        match status {
            OH_AVErrCode::AV_ERR_OK => {}
            OH_AVErrCode::AV_ERR_TRY_AGAIN_LATER => return Ok(Output::None),
            OH_AVErrCode::AV_ERR_STREAM_CHANGED => return Ok(Output::FormatChanged),
            status => return Err(format!("decoding failed: {}", status.0)),
        }
        let mut attr = empty_attr();
        // SAFETY: an output buffer the decoder gave.
        unsafe {
            let buffer = NonNull::new(OH_VideoDecoder_GetOutputBuffer(self.raw.as_ptr(), index))
                .ok_or("the decoder has no output buffer")?;
            check(OH_AVBuffer_GetBufferAttr(buffer.as_ptr(), &raw mut attr), "reading a frame's attributes")?;
        }
        Ok(Output::Buffer(index, attr))
    }

    /// The bytes of output buffer `index`, of a decoder into memory, valid
    /// until it goes back.
    pub(super) fn output_bytes(&self, index: u32, attr: BufferAttr) -> Result<&[u8], String> {
        // SAFETY: an output buffer the decoder gave, whose bytes are the
        // caller's until it goes back.
        unsafe {
            let buffer = NonNull::new(OH_VideoDecoder_GetOutputBuffer(self.raw.as_ptr(), index))
                .ok_or("the decoder has no output buffer")?;
            let address = OH_AVBuffer_GetAddr(buffer.as_ptr());
            if address.is_null() {
                return Err("a decoded frame has no memory".to_owned());
            }
            let capacity = usize::try_from(OH_AVBuffer_GetCapacity(buffer.as_ptr())).unwrap_or(0);
            let offset = usize::try_from(attr.offset).unwrap_or(0).min(capacity);
            let size = usize::try_from(attr.size).unwrap_or(0).min(capacity - offset);
            Ok(std::slice::from_raw_parts(address.add(offset), size))
        }
    }

    pub(super) fn output_format(&self) -> Option<Format> {
        // SAFETY: the decoder is alive; the format is the caller's.
        let raw = NonNull::new(unsafe { OH_VideoDecoder_GetOutputDescription(self.raw.as_ptr()) })?;
        Some(Format { raw })
    }

    /// Sends output buffer `index` to the decoder's surface.
    pub(super) fn render_output(&self, index: u32) -> Result<(), String> {
        // SAFETY: an output buffer the decoder gave.
        check(unsafe { OH_VideoDecoder_RenderOutputBuffer(self.raw.as_ptr(), index) }, "rendering a decoded frame")
    }

    /// Gives output buffer `index` back unseen.
    pub(super) fn free_output(&self, index: u32) -> Result<(), String> {
        // SAFETY: an output buffer the decoder gave.
        check(unsafe { OH_VideoDecoder_FreeOutputBuffer(self.raw.as_ptr(), index) }, "releasing a decoded frame")
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: the decoder is this one's.
        unsafe {
            OH_VideoDecoder_Stop(self.raw.as_ptr());
            OH_VideoDecoder_Destroy(self.raw.as_ptr());
        }
    }
}

/// Wakes a thread that waits for a surface's next frame.
#[derive(Default)]
struct Signal {
    ready: Mutex<bool>,
    changed: Condvar,
}

unsafe extern "C" fn on_frame_available(context: *mut c_void) {
    // SAFETY: the surface's signal, which outlives the surface.
    let signal = unsafe { &*context.cast::<Signal>() };
    *signal.ready.lock() = true;
    signal.changed.notify_all();
}

/// A consumer surface of native buffers that the GPU samples, and the window
/// a decoder renders into.
pub(super) struct Surface {
    raw: NonNull<OH_NativeImage>,
    window: NonNull<OHNativeWindow>,
    /// The listener's context, which outlives the surface.
    signal: Box<Signal>,
}

// SAFETY: as for `AvSource`.
unsafe impl Send for Surface {}
// SAFETY: the native image locks itself in each call, as its consumer's
// buffers go back from the threads that read them; the listener's signal is
// a mutex and a condition variable.
unsafe impl Sync for Surface {}

impl Surface {
    pub(super) fn new(width: i32, height: i32) -> Result<Surface, String> {
        // SAFETY: no preconditions.
        let raw = NonNull::new(unsafe { OH_ConsumerSurface_Create() }).ok_or("OH_ConsumerSurface_Create failed")?;
        let mut surface = Surface { raw, window: NonNull::dangling(), signal: Box::default() };
        let listener = OH_OnFrameAvailableListener {
            context: (&raw const *surface.signal).cast_mut().cast(),
            onFrameAvailable: Some(on_frame_available),
        };
        let usage = OH_NativeBuffer_Usage::NATIVEBUFFER_USAGE_HW_TEXTURE.0
            | OH_NativeBuffer_Usage::NATIVEBUFFER_USAGE_MEM_DMA.0;
        // SAFETY: the surface is alive, and the listener's context outlives
        // it; the window is the surface's.
        unsafe {
            let status = OH_ConsumerSurface_SetDefaultUsage(raw.as_ptr(), u64::from(usage));
            native_check(status, "setting the surface's usage")?;
            native_check(OH_ConsumerSurface_SetDefaultSize(raw.as_ptr(), width, height), "setting the surface's size")?;
            native_check(OH_NativeImage_SetOnFrameAvailableListener(raw.as_ptr(), listener), "listening for frames")?;
            let window = OH_NativeImage_AcquireNativeWindow(raw.as_ptr());
            surface.window = NonNull::new(window).ok_or("the surface has no window")?;
        }
        Ok(surface)
    }

    /// The window that a decoder renders into.
    pub(super) fn window(&self) -> NonNull<OHNativeWindow> {
        self.window
    }

    /// The next frame, once its writes finished, waiting up to `timeout`
    /// for one.
    pub(super) fn acquire(&self, timeout: Duration) -> Result<Option<SurfaceBuffer>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            *self.signal.ready.lock() = false;
            let (mut buffer, mut fence) = (std::ptr::null_mut(), -1);
            // SAFETY: the surface is alive.
            let status =
                unsafe { OH_NativeImage_AcquireNativeWindowBuffer(self.raw.as_ptr(), &raw mut buffer, &raw mut fence) };
            match status {
                NATIVE_OK => {
                    let window_buffer = NonNull::new(buffer).ok_or("the surface gave no buffer")?;
                    let acquired = SurfaceBuffer { surface: self.raw, window_buffer };
                    wait_for_fence(fence, deadline.saturating_duration_since(Instant::now()))?;
                    return Ok(Some(acquired));
                }
                NATIVE_NO_BUFFER => {
                    let mut ready = self.signal.ready.lock();
                    if !*ready && self.signal.changed.wait_until(&mut ready, deadline).timed_out() {
                        return Ok(None);
                    }
                }
                status => return Err(format!("acquiring a frame failed: {status}")),
            }
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        let mut raw = self.raw.as_ptr();
        // SAFETY: the surface is this one's, and its buffers went back.
        unsafe { OH_NativeImage_Destroy(&raw mut raw) };
    }
}

fn native_check(status: i32, what: &str) -> Result<(), String> {
    if status == NATIVE_OK { Ok(()) } else { Err(format!("{what} failed: {status}")) }
}

/// Waits up to `timeout` for `fence`, a sync file descriptor or -1, and
/// closes it.
fn wait_for_fence(fence: c_int, timeout: Duration) -> Result<(), String> {
    if fence < 0 {
        return Ok(());
    }
    let mut poll = libc::pollfd { fd: fence, events: libc::POLLIN, revents: 0 };
    let timeout = c_int::try_from(timeout.as_millis()).unwrap_or(c_int::MAX);
    // SAFETY: the descriptor is the caller's, and closed once.
    let ready = unsafe {
        let ready = libc::poll(&raw mut poll, 1, timeout);
        libc::close(fence);
        ready
    };
    if ready > 0 { Ok(()) } else { Err("a decoded frame's writes didn't finish".to_owned()) }
}

/// A frame that a surface gave, which goes back to it when dropped, from any
/// thread. It must drop before its surface.
pub(super) struct SurfaceBuffer {
    surface: NonNull<OH_NativeImage>,
    window_buffer: NonNull<OHNativeWindowBuffer>,
}

// SAFETY: as for `AvSource`.
unsafe impl Send for SurfaceBuffer {}

impl SurfaceBuffer {
    /// The frame's native buffer's size in pixels.
    pub(super) fn size(&self) -> Result<(u32, u32), String> {
        let mut buffer = std::ptr::null_mut();
        let mut config = OH_NativeBuffer_Config { width: 0, height: 0, format: 0, usage: 0, stride: 0 };
        // SAFETY: the window buffer is alive, and its native buffer with it.
        unsafe {
            let status = OH_NativeBuffer_FromNativeWindowBuffer(self.window_buffer.as_ptr(), &raw mut buffer);
            native_check(status, "OH_NativeBuffer_FromNativeWindowBuffer")?;
            let buffer = NonNull::new(buffer).ok_or("the frame has no native buffer")?;
            OH_NativeBuffer_GetConfig(buffer.as_ptr(), &raw mut config);
        }
        Ok((config.width.max(0) as u32, config.height.max(0) as u32))
    }

    /// The frame's native buffer, with a reference that the caller lets go
    /// of.
    pub(super) fn reference_native_buffer(&self) -> Result<NonNull<NativeBuffer>, String> {
        let mut buffer = std::ptr::null_mut();
        // SAFETY: the window buffer is alive; the native buffer is the
        // window buffer's until the reference taken here.
        unsafe {
            let status = OH_NativeBuffer_FromNativeWindowBuffer(self.window_buffer.as_ptr(), &raw mut buffer);
            native_check(status, "OH_NativeBuffer_FromNativeWindowBuffer")?;
            let buffer = NonNull::new(buffer).ok_or("the frame has no native buffer")?;
            native_check(OH_NativeBuffer_Reference(buffer.as_ptr()), "OH_NativeBuffer_Reference")?;
            Ok(buffer)
        }
    }
}

impl Drop for SurfaceBuffer {
    fn drop(&mut self) {
        // SAFETY: a buffer the surface gave, which goes back once.
        unsafe { OH_NativeImage_ReleaseNativeWindowBuffer(self.surface.as_ptr(), self.window_buffer.as_ptr(), -1) };
    }
}
