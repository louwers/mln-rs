//! The parts of the NDK's media API (libmediandk) and hardware buffers
//! (libnativewindow) that the player uses, looked up at run time, and owners
//! of the objects they make, which delete them when dropped. Custom data
//! sources need API level 28, so the library still loads on older versions,
//! where each video fails to open.

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_void};
use std::io;
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use libloading::Library;
use parking_lot::{Condvar, Mutex};

use crate::bytes::ByteSource;

/// `media_status_t`: 0, or an error.
type Status = c_int;

/// `AMEDIA_OK`.
const OK: Status = 0;
/// `AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE`.
const NO_BUFFER_AVAILABLE: Status = -30001;
/// `AMEDIACODEC_INFO_TRY_AGAIN_LATER`.
const TRY_AGAIN_LATER: isize = -1;
/// `AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED`.
const OUTPUT_FORMAT_CHANGED: isize = -2;
/// `AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED`.
const OUTPUT_BUFFERS_CHANGED: isize = -3;
/// `AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM`.
pub(super) const END_OF_STREAM: u32 = 4;
/// `AMEDIAEXTRACTOR_SEEK_PREVIOUS_SYNC`.
const SEEK_PREVIOUS_SYNC: c_int = 0;
/// `AIMAGE_FORMAT_YUV_420_888`.
const YUV_420_888: i32 = 0x23;
/// `AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN`.
pub(super) const USAGE_CPU_READ_OFTEN: u64 = 3;
/// `AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE`.
pub(super) const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;

macro_rules! opaque {
    ($($name:ident),*) => {
        $(
            #[repr(C)]
            pub(super) struct $name {
                _private: [u8; 0],
            }
        )*
    };
}

opaque!(AMediaDataSource, AMediaExtractor, AMediaFormat, AMediaCodec, AImageReader, AImage, ANativeWindow);

/// `AHardwareBuffer`.
pub(super) type HardwareBuffer = c_void;

/// `AMediaCodecBufferInfo`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct BufferInfo {
    pub(super) offset: i32,
    pub(super) size: i32,
    pub(super) presentation_time_us: i64,
    pub(super) flags: u32,
}

/// `AImageCropRect`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct CropRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// `AImageReader_ImageListener`.
#[repr(C)]
struct ImageListener {
    context: *mut c_void,
    on_image_available: unsafe extern "C" fn(*mut c_void, *mut AImageReader),
}

type ReadAt = unsafe extern "C" fn(*mut c_void, i64, *mut c_void, usize) -> isize;
type GetSize = unsafe extern "C" fn(*mut c_void) -> isize;
type Close = unsafe extern "C" fn(*mut c_void);

/// Declares a table of functions that `load` looks up in a library, each
/// field by its symbol's name.
macro_rules! functions {
    ($table:ident from $library:literal { $($field:ident = $symbol:literal: fn($($arg:ty),*) $(-> $ret:ty)?;)* }) => {
        pub(super) struct $table {
            _library: Library,
            $($field: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl $table {
            fn load() -> Result<$table, String> {
                // SAFETY: a system library, whose initializers any process
                // may run, and the symbols have the NDK's signatures.
                unsafe {
                    let library = Library::new($library).map_err(|error| format!("{}: {error}", $library))?;
                    $(
                        let $field = *library
                            .get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>(concat!($symbol, "\0").as_bytes())
                            .map_err(|error| format!("{}: {error}", $symbol))?;
                    )*
                    Ok($table { _library: library, $($field),* })
                }
            }
        }
    };
}

functions!(Media from "libmediandk.so" {
    data_source_new = "AMediaDataSource_new": fn() -> *mut AMediaDataSource;
    data_source_delete = "AMediaDataSource_delete": fn(*mut AMediaDataSource);
    data_source_set_userdata = "AMediaDataSource_setUserdata": fn(*mut AMediaDataSource, *mut c_void);
    data_source_set_read_at = "AMediaDataSource_setReadAt": fn(*mut AMediaDataSource, ReadAt);
    data_source_set_get_size = "AMediaDataSource_setGetSize": fn(*mut AMediaDataSource, GetSize);
    data_source_set_close = "AMediaDataSource_setClose": fn(*mut AMediaDataSource, Close);

    extractor_new = "AMediaExtractor_new": fn() -> *mut AMediaExtractor;
    extractor_delete = "AMediaExtractor_delete": fn(*mut AMediaExtractor) -> Status;
    extractor_set_data_source = "AMediaExtractor_setDataSourceCustom":
        fn(*mut AMediaExtractor, *mut AMediaDataSource) -> Status;
    extractor_track_count = "AMediaExtractor_getTrackCount": fn(*mut AMediaExtractor) -> usize;
    extractor_track_format = "AMediaExtractor_getTrackFormat": fn(*mut AMediaExtractor, usize) -> *mut AMediaFormat;
    extractor_select_track = "AMediaExtractor_selectTrack": fn(*mut AMediaExtractor, usize) -> Status;
    extractor_read_sample = "AMediaExtractor_readSampleData": fn(*mut AMediaExtractor, *mut u8, usize) -> isize;
    extractor_sample_time = "AMediaExtractor_getSampleTime": fn(*mut AMediaExtractor) -> i64;
    extractor_advance = "AMediaExtractor_advance": fn(*mut AMediaExtractor) -> bool;
    extractor_seek_to = "AMediaExtractor_seekTo": fn(*mut AMediaExtractor, i64, c_int) -> Status;

    format_delete = "AMediaFormat_delete": fn(*mut AMediaFormat) -> Status;
    format_get_int32 = "AMediaFormat_getInt32": fn(*mut AMediaFormat, *const c_char, *mut i32) -> bool;
    format_get_string = "AMediaFormat_getString": fn(*mut AMediaFormat, *const c_char, *mut *const c_char) -> bool;

    codec_create_decoder = "AMediaCodec_createDecoderByType": fn(*const c_char) -> *mut AMediaCodec;
    codec_delete = "AMediaCodec_delete": fn(*mut AMediaCodec) -> Status;
    codec_configure = "AMediaCodec_configure":
        fn(*mut AMediaCodec, *const AMediaFormat, *mut ANativeWindow, *mut c_void, u32) -> Status;
    codec_start = "AMediaCodec_start": fn(*mut AMediaCodec) -> Status;
    codec_stop = "AMediaCodec_stop": fn(*mut AMediaCodec) -> Status;
    codec_flush = "AMediaCodec_flush": fn(*mut AMediaCodec) -> Status;
    codec_dequeue_input = "AMediaCodec_dequeueInputBuffer": fn(*mut AMediaCodec, i64) -> isize;
    codec_input_buffer = "AMediaCodec_getInputBuffer": fn(*mut AMediaCodec, usize, *mut usize) -> *mut u8;
    codec_queue_input = "AMediaCodec_queueInputBuffer":
        fn(*mut AMediaCodec, usize, c_long, usize, u64, u32) -> Status;
    codec_dequeue_output = "AMediaCodec_dequeueOutputBuffer": fn(*mut AMediaCodec, *mut BufferInfo, i64) -> isize;
    codec_output_format = "AMediaCodec_getOutputFormat": fn(*mut AMediaCodec) -> *mut AMediaFormat;
    codec_release_output = "AMediaCodec_releaseOutputBuffer": fn(*mut AMediaCodec, usize, bool) -> Status;

    reader_new = "AImageReader_newWithUsage": fn(i32, i32, i32, u64, i32, *mut *mut AImageReader) -> Status;
    reader_delete = "AImageReader_delete": fn(*mut AImageReader);
    reader_window = "AImageReader_getWindow": fn(*mut AImageReader, *mut *mut ANativeWindow) -> Status;
    reader_set_listener = "AImageReader_setImageListener": fn(*mut AImageReader, *mut ImageListener) -> Status;
    reader_acquire_latest = "AImageReader_acquireLatestImage": fn(*mut AImageReader, *mut *mut AImage) -> Status;

    image_delete = "AImage_delete": fn(*mut AImage);
    image_width = "AImage_getWidth": fn(*const AImage, *mut i32) -> Status;
    image_height = "AImage_getHeight": fn(*const AImage, *mut i32) -> Status;
    image_crop_rect = "AImage_getCropRect": fn(*const AImage, *mut CropRect) -> Status;
    image_plane_count = "AImage_getNumberOfPlanes": fn(*const AImage, *mut i32) -> Status;
    image_plane_pixel_stride = "AImage_getPlanePixelStride": fn(*const AImage, c_int, *mut i32) -> Status;
    image_plane_row_stride = "AImage_getPlaneRowStride": fn(*const AImage, c_int, *mut i32) -> Status;
    image_plane_data = "AImage_getPlaneData": fn(*const AImage, c_int, *mut *mut u8, *mut c_int) -> Status;
    image_hardware_buffer = "AImage_getHardwareBuffer": fn(*const AImage, *mut *mut HardwareBuffer) -> Status;
});

functions!(Window from "libnativewindow.so" {
    buffer_acquire = "AHardwareBuffer_acquire": fn(*mut HardwareBuffer);
});

functions!(Binder from "libbinder_ndk.so" {
    start_thread_pool = "ABinderProcess_startThreadPool": fn();
});

/// The NDK's functions.
pub(super) struct Ndk {
    media: Media,
    window: Window,
    /// Keeps the binder library loaded while its thread pool runs.
    _binder: Option<Binder>,
}

impl Ndk {
    /// The functions, looked up once.
    ///
    /// The media services read a video and hand its frames over through
    /// calls into the process, which a binder thread pool serves. An app's
    /// process has one; in another, such as a test or a command-line tool,
    /// this starts one (API level 29).
    ///
    /// # Errors
    ///
    /// Before API level 28, a function isn't there.
    pub(super) fn get() -> Result<&'static Ndk, String> {
        static NDK: OnceLock<Result<Ndk, String>> = OnceLock::new();
        NDK.get_or_init(|| {
            let binder = Binder::load().ok();
            if let Some(binder) = &binder {
                // SAFETY: no preconditions; it does nothing where the pool
                // runs.
                unsafe { (binder.start_thread_pool)() };
            }
            Ok(Ndk { media: Media::load()?, window: Window::load()?, _binder: binder })
        })
        .as_ref()
        .map_err(|error| format!("the NDK's media API isn't there (it needs API level 28): {error}"))
    }
}

/// Describes a failed call.
fn check(status: Status, what: &str) -> Result<(), String> {
    if status == OK { Ok(()) } else { Err(format!("{what} failed: {status}")) }
}

/// What a data source's callbacks read: the video's bytes, and why a read
/// failed last.
struct Reader {
    bytes: Arc<ByteSource>,
    error: Mutex<Option<io::Error>>,
}

unsafe extern "C" fn read_at(userdata: *mut c_void, offset: i64, buffer: *mut c_void, size: usize) -> isize {
    // SAFETY: the data source's reader, which outlives it.
    let reader = unsafe { &*userdata.cast::<Reader>() };
    if size == 0 {
        return 0;
    }
    let Ok(offset) = u64::try_from(offset) else { return -1 };
    // SAFETY: the extractor's buffer of `size` bytes, which it doesn't
    // touch until the call returns.
    let buffer = unsafe { std::slice::from_raw_parts_mut(buffer.cast::<u8>(), size) };
    // Extractors take a short read for an error, so a read ends at the end
    // of the video only. The framework reads 0 bytes as the end, and -1,
    // which the header calls the end, as an error.
    match reader.bytes.read_full(offset, buffer) {
        Ok(count) => isize::try_from(count).unwrap_or(-1),
        Err(error) => {
            *reader.error.lock() = Some(error);
            -1
        }
    }
}

unsafe extern "C" fn get_size(userdata: *mut c_void) -> isize {
    // SAFETY: the data source's reader, which outlives it.
    let reader = unsafe { &*userdata.cast::<Reader>() };
    match reader.bytes.size() {
        Ok(size) => size.and_then(|size| isize::try_from(size).ok()).unwrap_or(-1),
        Err(error) => {
            *reader.error.lock() = Some(error);
            -1
        }
    }
}

unsafe extern "C" fn close(userdata: *mut c_void) {
    // SAFETY: the data source's reader, which outlives it.
    let reader = unsafe { &*userdata.cast::<Reader>() };
    reader.bytes.cancel();
}

/// An `AMediaDataSource` that reads a [`ByteSource`].
pub(super) struct DataSource {
    ndk: &'static Ndk,
    raw: NonNull<AMediaDataSource>,
    /// The callbacks' userdata, which outlives the data source.
    reader: Box<Reader>,
}

impl DataSource {
    pub(super) fn new(ndk: &'static Ndk, bytes: Arc<ByteSource>) -> Result<DataSource, String> {
        let media = &ndk.media;
        // SAFETY: no preconditions.
        let raw = NonNull::new(unsafe { (media.data_source_new)() }).ok_or("AMediaDataSource_new failed")?;
        let reader = Box::new(Reader { bytes, error: Mutex::new(None) });
        // SAFETY: the data source is new, and the userdata is the reader,
        // which the callbacks take it for and which outlives it.
        unsafe {
            (media.data_source_set_userdata)(raw.as_ptr(), (&raw const *reader).cast_mut().cast());
            (media.data_source_set_read_at)(raw.as_ptr(), read_at);
            (media.data_source_set_get_size)(raw.as_ptr(), get_size);
            (media.data_source_set_close)(raw.as_ptr(), close);
        }
        Ok(DataSource { ndk, raw, reader })
    }

    /// Why a read failed last.
    pub(super) fn take_error(&self) -> Option<io::Error> {
        self.reader.error.lock().take()
    }
}

impl Drop for DataSource {
    fn drop(&mut self) {
        // SAFETY: the data source is this one's, and no extractor reads it
        // any more.
        unsafe { (self.ndk.media.data_source_delete)(self.raw.as_ptr()) };
    }
}

/// An `AMediaFormat`.
pub(super) struct Format {
    ndk: &'static Ndk,
    raw: NonNull<AMediaFormat>,
}

impl Format {
    fn key(key: &str) -> Option<CString> {
        CString::new(key).ok()
    }

    pub(super) fn int32(&self, key: &str) -> Option<i32> {
        let key = Self::key(key)?;
        let mut value = 0;
        // SAFETY: the format is alive, and the key a C string.
        unsafe { (self.ndk.media.format_get_int32)(self.raw.as_ptr(), key.as_ptr(), &raw mut value) }.then_some(value)
    }

    pub(super) fn string(&self, key: &str) -> Option<String> {
        let key = Self::key(key)?;
        let mut value = std::ptr::null();
        // SAFETY: as for `int32`; the string is the format's, and copied
        // before it goes.
        unsafe {
            if !(self.ndk.media.format_get_string)(self.raw.as_ptr(), key.as_ptr(), &raw mut value) || value.is_null() {
                return None;
            }
            Some(CStr::from_ptr(value).to_string_lossy().into_owned())
        }
    }
}

impl Drop for Format {
    fn drop(&mut self) {
        // SAFETY: the format is this one's.
        unsafe { (self.ndk.media.format_delete)(self.raw.as_ptr()) };
    }
}

/// An `AMediaExtractor`.
pub(super) struct Extractor {
    ndk: &'static Ndk,
    raw: NonNull<AMediaExtractor>,
}

// SAFETY: the NDK's media objects may be used from any thread, one at a
// time.
unsafe impl Send for Extractor {}

impl Extractor {
    /// An extractor of `source`'s video, which must outlive it. Blocks while
    /// it reads the container's header.
    pub(super) fn new(ndk: &'static Ndk, source: &DataSource) -> Result<Extractor, String> {
        // SAFETY: no preconditions.
        let raw = NonNull::new(unsafe { (ndk.media.extractor_new)() }).ok_or("AMediaExtractor_new failed")?;
        let extractor = Extractor { ndk, raw };
        // SAFETY: both are alive, and the caller keeps the source.
        let status = unsafe { (ndk.media.extractor_set_data_source)(raw.as_ptr(), source.raw.as_ptr()) };
        check(status, "reading the video's container")?;
        Ok(extractor)
    }

    pub(super) fn track_count(&self) -> usize {
        // SAFETY: the extractor is alive.
        unsafe { (self.ndk.media.extractor_track_count)(self.raw.as_ptr()) }
    }

    pub(super) fn track_format(&self, track: usize) -> Option<Format> {
        // SAFETY: the extractor is alive; the format is the caller's.
        let raw = NonNull::new(unsafe { (self.ndk.media.extractor_track_format)(self.raw.as_ptr(), track) })?;
        Some(Format { ndk: self.ndk, raw })
    }

    pub(super) fn select_track(&self, track: usize) -> Result<(), String> {
        // SAFETY: the extractor is alive.
        check(unsafe { (self.ndk.media.extractor_select_track)(self.raw.as_ptr(), track) }, "selecting a track")
    }

    /// Reads the current sample into `buffer`: its size, or `None` at the
    /// end of the track.
    pub(super) fn read_sample(&self, buffer: &mut [u8]) -> Option<usize> {
        // SAFETY: the extractor is alive, and the buffer has `len` bytes.
        let read =
            unsafe { (self.ndk.media.extractor_read_sample)(self.raw.as_ptr(), buffer.as_mut_ptr(), buffer.len()) };
        usize::try_from(read).ok()
    }

    /// The current sample's time in microseconds.
    pub(super) fn sample_time(&self) -> i64 {
        // SAFETY: the extractor is alive.
        unsafe { (self.ndk.media.extractor_sample_time)(self.raw.as_ptr()) }
    }

    pub(super) fn advance(&self) {
        // SAFETY: the extractor is alive.
        unsafe { (self.ndk.media.extractor_advance)(self.raw.as_ptr()) };
    }

    /// Moves to the sync sample at or before `time_us`.
    pub(super) fn seek_to(&self, time_us: i64) -> Result<(), String> {
        // SAFETY: the extractor is alive.
        let status = unsafe { (self.ndk.media.extractor_seek_to)(self.raw.as_ptr(), time_us, SEEK_PREVIOUS_SYNC) };
        check(status, "seeking")
    }
}

impl Drop for Extractor {
    fn drop(&mut self) {
        // SAFETY: the extractor is this one's.
        unsafe { (self.ndk.media.extractor_delete)(self.raw.as_ptr()) };
    }
}

/// What a codec gave from its output.
pub(super) enum Output {
    /// A decoded buffer, by its index, which goes back to the codec with
    /// [`Codec::release_output`].
    Buffer(usize, BufferInfo),
    /// The output's format changed.
    FormatChanged,
    /// Nothing yet.
    None,
}

/// A started `AMediaCodec` decoder.
pub(super) struct Codec {
    ndk: &'static Ndk,
    raw: NonNull<AMediaCodec>,
}

// SAFETY: as for `Extractor`.
unsafe impl Send for Codec {}

impl Codec {
    /// A decoder of `format` that renders into `window`, which must
    /// outlive it.
    pub(super) fn decoder(ndk: &'static Ndk, format: &Format, window: NonNull<ANativeWindow>) -> Result<Codec, String> {
        let media = &ndk.media;
        let mime = format.string("mime").ok_or("the video track has no MIME type")?;
        let mime_c = CString::new(mime.clone()).map_err(|error| error.to_string())?;
        // SAFETY: a C string.
        let raw = NonNull::new(unsafe { (media.codec_create_decoder)(mime_c.as_ptr()) })
            .ok_or_else(|| format!("no decoder of {mime}"))?;
        let codec = Codec { ndk, raw };
        // SAFETY: the codec is new, the format and the window are alive,
        // and the caller keeps the window.
        let configured = unsafe {
            (media.codec_configure)(raw.as_ptr(), format.raw.as_ptr(), window.as_ptr(), std::ptr::null_mut(), 0)
        };
        check(configured, &format!("configuring the {mime} decoder"))?;
        // SAFETY: the codec is configured.
        check(unsafe { (media.codec_start)(raw.as_ptr()) }, &format!("starting the {mime} decoder"))?;
        Ok(codec)
    }

    /// Takes back the buffers the codec holds, for a seek.
    pub(super) fn flush(&self) -> Result<(), String> {
        // SAFETY: the codec is alive.
        check(unsafe { (self.ndk.media.codec_flush)(self.raw.as_ptr()) }, "flushing the decoder")
    }

    /// Fills a free input buffer with `fill`, which writes into it and
    /// returns the size it wrote, the sample's time and flags; `false` when
    /// no buffer is free.
    pub(super) fn queue_input(&self, fill: impl FnOnce(&mut [u8]) -> (usize, i64, u32)) -> Result<bool, String> {
        let media = &self.ndk.media;
        // SAFETY: the codec is alive.
        let Ok(index) = usize::try_from(unsafe { (media.codec_dequeue_input)(self.raw.as_ptr(), 0) }) else {
            return Ok(false);
        };
        let mut capacity = 0;
        // SAFETY: an input buffer the codec gave, which is the caller's
        // until it is queued.
        let data = unsafe { (media.codec_input_buffer)(self.raw.as_ptr(), index, &raw mut capacity) };
        if data.is_null() {
            return Err("the decoder has no input buffer".to_owned());
        }
        // SAFETY: the buffer has `capacity` bytes.
        let buffer = unsafe { std::slice::from_raw_parts_mut(data, capacity) };
        let (size, time_us, flags) = fill(buffer);
        let time = u64::try_from(time_us).unwrap_or(0);
        // SAFETY: the buffer the codec gave, with `size` of its bytes.
        let status = unsafe { (media.codec_queue_input)(self.raw.as_ptr(), index, 0, size, time, flags) };
        check(status, "queueing a sample").map(|()| true)
    }

    /// The next output, waiting up to `timeout` for one.
    pub(super) fn dequeue_output(&self, timeout: Duration) -> Result<Output, String> {
        let mut info = BufferInfo::default();
        let timeout = i64::try_from(timeout.as_micros()).unwrap_or(i64::MAX);
        // SAFETY: the codec is alive.
        let index = unsafe { (self.ndk.media.codec_dequeue_output)(self.raw.as_ptr(), &raw mut info, timeout) };
        match index {
            TRY_AGAIN_LATER | OUTPUT_BUFFERS_CHANGED => Ok(Output::None),
            OUTPUT_FORMAT_CHANGED => Ok(Output::FormatChanged),
            index => usize::try_from(index)
                .map(|index| Output::Buffer(index, info))
                .map_err(|_| format!("decoding failed: {index}")),
        }
    }

    pub(super) fn output_format(&self) -> Option<Format> {
        // SAFETY: the codec is alive; the format is the caller's.
        let raw = NonNull::new(unsafe { (self.ndk.media.codec_output_format)(self.raw.as_ptr()) })?;
        Some(Format { ndk: self.ndk, raw })
    }

    /// Gives output buffer `index` back, and with `render`, sends it to the
    /// window first.
    pub(super) fn release_output(&self, index: usize, render: bool) -> Result<(), String> {
        // SAFETY: an output buffer the codec gave.
        let status = unsafe { (self.ndk.media.codec_release_output)(self.raw.as_ptr(), index, render) };
        check(status, "releasing a decoded buffer")
    }
}

impl Drop for Codec {
    fn drop(&mut self) {
        // SAFETY: the codec is this one's.
        unsafe {
            (self.ndk.media.codec_stop)(self.raw.as_ptr());
            (self.ndk.media.codec_delete)(self.raw.as_ptr());
        }
    }
}

/// Wakes a thread that waits for a reader's next image.
#[derive(Default)]
struct Signal {
    ready: Mutex<bool>,
    changed: Condvar,
}

unsafe extern "C" fn on_image_available(context: *mut c_void, _reader: *mut AImageReader) {
    // SAFETY: the reader's signal, which outlives the reader.
    let signal = unsafe { &*context.cast::<Signal>() };
    *signal.ready.lock() = true;
    signal.changed.notify_all();
}

/// An `AImageReader` of 4:2:0 YUV images, and the window it reads.
pub(super) struct ImageReader {
    ndk: &'static Ndk,
    raw: NonNull<AImageReader>,
    window: NonNull<ANativeWindow>,
    /// The listener's context, which outlives the reader.
    signal: Box<Signal>,
}

// SAFETY: as for `Extractor`.
unsafe impl Send for ImageReader {}

impl ImageReader {
    /// A reader of images of `width` by `height` pixels that holds up to
    /// `max_images` at once, of buffers with the `AHARDWAREBUFFER_USAGE_*`
    /// bits `usage`.
    pub(super) fn new(
        ndk: &'static Ndk,
        width: i32,
        height: i32,
        usage: u64,
        max_images: i32,
    ) -> Result<ImageReader, String> {
        let media = &ndk.media;
        let mut raw = std::ptr::null_mut();
        // SAFETY: a valid output.
        let status = unsafe { (media.reader_new)(width, height, YUV_420_888, usage, max_images, &raw mut raw) };
        check(status, &format!("making a {width}x{height} image reader of usage {usage:#x}"))?;
        let raw = NonNull::new(raw).ok_or("AImageReader_newWithUsage made no reader")?;
        let mut reader = ImageReader { ndk, raw, window: NonNull::dangling(), signal: Box::default() };
        let mut window = std::ptr::null_mut();
        // SAFETY: the reader is alive; the window is the reader's.
        check(unsafe { (media.reader_window)(raw.as_ptr(), &raw mut window) }, "getting the reader's window")?;
        reader.window = NonNull::new(window).ok_or("the image reader has no window")?;
        let mut listener = ImageListener { context: (&raw const *reader.signal).cast_mut().cast(), on_image_available };
        // SAFETY: the reader copies the listener, whose context outlives the
        // reader.
        check(unsafe { (media.reader_set_listener)(raw.as_ptr(), &raw mut listener) }, "listening for images")?;
        Ok(reader)
    }

    /// The window that a codec renders into.
    pub(super) fn window(&self) -> NonNull<ANativeWindow> {
        self.window
    }

    /// The newest image, waiting up to `timeout` for one.
    pub(super) fn acquire_latest(&self, timeout: Duration) -> Result<Option<Image>, String> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            *self.signal.ready.lock() = false;
            let mut image = std::ptr::null_mut();
            // SAFETY: the reader is alive.
            let status = unsafe { (self.ndk.media.reader_acquire_latest)(self.raw.as_ptr(), &raw mut image) };
            match status {
                OK => {
                    let raw = NonNull::new(image).ok_or("the image reader gave no image")?;
                    return Ok(Some(Image { ndk: self.ndk, raw }));
                }
                NO_BUFFER_AVAILABLE => {
                    let mut ready = self.signal.ready.lock();
                    if !*ready && self.signal.changed.wait_until(&mut ready, deadline).timed_out() {
                        return Ok(None);
                    }
                }
                status => return Err(format!("acquiring an image failed: {status}")),
            }
        }
    }
}

impl Drop for ImageReader {
    fn drop(&mut self) {
        // SAFETY: the reader is this one's; its images become unusable,
        // and deleting them later only frees them.
        unsafe { (self.ndk.media.reader_delete)(self.raw.as_ptr()) };
    }
}

/// One plane of an image in memory, valid while the image lives.
pub(super) struct Plane {
    pub(super) data: *const u8,
    pub(super) len: usize,
    pub(super) row_stride: usize,
    pub(super) pixel_stride: usize,
}

/// An image that a reader gave, which goes back to it when dropped.
pub(super) struct Image {
    ndk: &'static Ndk,
    raw: NonNull<AImage>,
}

// SAFETY: as for `Extractor`.
unsafe impl Send for Image {}

impl Image {
    /// The image's size: its crop's, where the codec crops it.
    pub(super) fn size(&self) -> Result<(u32, u32), String> {
        let (mut width, mut height) = (0, 0);
        // SAFETY: the image is alive.
        unsafe {
            check((self.ndk.media.image_width)(self.raw.as_ptr(), &raw mut width), "AImage_getWidth")?;
            check((self.ndk.media.image_height)(self.raw.as_ptr(), &raw mut height), "AImage_getHeight")?;
        }
        Ok((width.max(0) as u32, height.max(0) as u32))
    }

    /// The left and top edges of the image's crop in its planes.
    pub(super) fn crop_origin(&self) -> (usize, usize) {
        let mut rect = CropRect::default();
        // SAFETY: the image is alive.
        let status = unsafe { (self.ndk.media.image_crop_rect)(self.raw.as_ptr(), &raw mut rect) };
        if status == OK { (rect.left.max(0) as usize, rect.top.max(0) as usize) } else { (0, 0) }
    }

    /// Plane `index` in memory, of an image of a reader of CPU-readable
    /// buffers.
    pub(super) fn plane(&self, index: c_int) -> Result<Plane, String> {
        let media = &self.ndk.media;
        let mut count = 0;
        let (mut pixel_stride, mut row_stride) = (0, 0);
        let (mut data, mut len) = (std::ptr::null_mut(), 0);
        // SAFETY: the image is alive; the data is the image's while it
        // lives.
        unsafe {
            check((media.image_plane_count)(self.raw.as_ptr(), &raw mut count), "AImage_getNumberOfPlanes")?;
            if index >= count {
                return Err(format!("the image has {count} planes"));
            }
            let image = self.raw.as_ptr();
            check((media.image_plane_pixel_stride)(image, index, &raw mut pixel_stride), "AImage_getPlanePixelStride")?;
            check((media.image_plane_row_stride)(image, index, &raw mut row_stride), "AImage_getPlaneRowStride")?;
            check((media.image_plane_data)(image, index, &raw mut data, &raw mut len), "AImage_getPlaneData")?;
        }
        if data.is_null() {
            return Err(format!("the image's plane {index} has no data"));
        }
        Ok(Plane {
            data,
            len: len.max(0) as usize,
            row_stride: row_stride.max(0) as usize,
            pixel_stride: pixel_stride.max(0) as usize,
        })
    }

    /// The image's hardware buffer with a reference that the caller
    /// releases.
    pub(super) fn acquire_hardware_buffer(&self) -> Result<NonNull<HardwareBuffer>, String> {
        let mut buffer = std::ptr::null_mut();
        // SAFETY: the image is alive; the buffer is the image's until the
        // reference taken here.
        unsafe {
            check(
                (self.ndk.media.image_hardware_buffer)(self.raw.as_ptr(), &raw mut buffer),
                "AImage_getHardwareBuffer",
            )?;
            let buffer = NonNull::new(buffer).ok_or("the image has no hardware buffer")?;
            (self.ndk.window.buffer_acquire)(buffer.as_ptr());
            Ok(buffer)
        }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: the image is this one's.
        unsafe { (self.ndk.media.image_delete)(self.raw.as_ptr()) };
    }
}
