//! The parts of `OpenHarmony`'s `AVCodec` (its media core, demuxer, source and
//! video decoder libraries), native image and native buffer APIs that the
//! player uses, looked up at run time, and owners of the objects they make,
//! which destroy them when dropped. Data sources with user data and decoders
//! in synchronous mode need API level 20, so the library still loads on older
//! versions, where each video fails to open.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io;
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use libloading::Library;
use parking_lot::{Condvar, Mutex};

use crate::bytes::ByteSource;

/// `OH_AVErrCode`: `AV_ERR_OK`, or an error.
type AvError = c_int;

/// `AV_ERR_OK`.
const OK: AvError = 0;
/// `AV_ERR_STREAM_CHANGED`.
const STREAM_CHANGED: AvError = 5_410_005;
/// `AV_ERR_TRY_AGAIN_LATER`.
const TRY_AGAIN_LATER: AvError = 5_410_006;
/// `AVCODEC_BUFFER_FLAGS_EOS`.
pub(super) const END_OF_STREAM: u32 = 1;
/// `MEDIA_TYPE_VID`.
const MEDIA_TYPE_VIDEO: i32 = 1;
/// `SEEK_MODE_PREVIOUS_SYNC`.
const SEEK_PREVIOUS_SYNC: c_int = 1;
/// `AV_PIXEL_FORMAT_YUVI420`.
pub(super) const PIXEL_FORMAT_I420: i32 = 1;
/// `AV_PIXEL_FORMAT_NV12`.
pub(super) const PIXEL_FORMAT_NV12: i32 = 2;
/// `AV_PIXEL_FORMAT_NV21`.
pub(super) const PIXEL_FORMAT_NV21: i32 = 3;
/// `AV_PIXEL_FORMAT_RGBA`.
pub(super) const PIXEL_FORMAT_RGBA: i32 = 5;
/// `NATIVE_ERROR_OK`.
const NATIVE_OK: i32 = 0;
/// `NATIVE_ERROR_NO_BUFFER`.
const NATIVE_NO_BUFFER: i32 = 40_601_000;
/// `NATIVEBUFFER_USAGE_MEM_DMA`.
const USAGE_MEM_DMA: u64 = 1 << 3;
/// `NATIVEBUFFER_USAGE_HW_TEXTURE`.
const USAGE_HW_TEXTURE: u64 = 1 << 9;
/// What a readAt callback returns for the end of the data.
const READ_END: i32 = -1;
/// What a readAt callback returns for a read that failed.
const READ_FAILED: i32 = -2;

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

opaque!(
    OH_AVFormat,
    OH_AVBuffer,
    OH_AVSource,
    OH_AVDemuxer,
    OH_AVCodec,
    OH_NativeImage,
    OHNativeWindow,
    OHNativeWindowBuffer
);

/// `OH_NativeBuffer`.
pub(super) type NativeBuffer = c_void;

/// `OH_AVCodecBufferAttr`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct BufferAttr {
    pub(super) pts: i64,
    pub(super) size: i32,
    pub(super) offset: i32,
    pub(super) flags: u32,
}

type ReadAt = unsafe extern "C" fn(*mut OH_AVBuffer, i32, i64, *mut c_void) -> i32;

/// `OH_AVDataSourceExt`.
#[repr(C)]
struct DataSourceDesc {
    size: i64,
    read_at: ReadAt,
}

/// `OH_OnFrameAvailableListener`.
#[repr(C)]
struct FrameListener {
    context: *mut c_void,
    on_frame_available: unsafe extern "C" fn(*mut c_void),
}

/// Declares a table of functions that `load` looks up in a library, each
/// field by its symbol's name.
macro_rules! functions {
    ($table:ident from $library:literal { $($field:ident = $symbol:literal: fn($($arg:ty),*) $(-> $ret:ty)?;)* }) => {
        struct $table {
            _library: Library,
            $($field: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl $table {
            fn load() -> Result<$table, String> {
                // SAFETY: a system library, whose initializers any process
                // may run, and the symbols have the SDK's signatures.
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

functions!(Core from "libnative_media_core.so" {
    format_create = "OH_AVFormat_Create": fn() -> *mut OH_AVFormat;
    format_destroy = "OH_AVFormat_Destroy": fn(*mut OH_AVFormat);
    format_copy = "OH_AVFormat_Copy": fn(*mut OH_AVFormat, *mut OH_AVFormat) -> bool;
    format_get_int = "OH_AVFormat_GetIntValue": fn(*mut OH_AVFormat, *const c_char, *mut i32) -> bool;
    format_get_long = "OH_AVFormat_GetLongValue": fn(*mut OH_AVFormat, *const c_char, *mut i64) -> bool;
    format_get_string = "OH_AVFormat_GetStringValue": fn(*mut OH_AVFormat, *const c_char, *mut *const c_char) -> bool;
    format_set_int = "OH_AVFormat_SetIntValue": fn(*mut OH_AVFormat, *const c_char, i32) -> bool;

    buffer_addr = "OH_AVBuffer_GetAddr": fn(*mut OH_AVBuffer) -> *mut u8;
    buffer_capacity = "OH_AVBuffer_GetCapacity": fn(*mut OH_AVBuffer) -> i32;
    buffer_attr = "OH_AVBuffer_GetBufferAttr": fn(*mut OH_AVBuffer, *mut BufferAttr) -> AvError;
});

functions!(Source from "libnative_media_avsource.so" {
    create = "OH_AVSource_CreateWithDataSourceExt": fn(*mut DataSourceDesc, *mut c_void) -> *mut OH_AVSource;
    destroy = "OH_AVSource_Destroy": fn(*mut OH_AVSource) -> AvError;
    source_format = "OH_AVSource_GetSourceFormat": fn(*mut OH_AVSource) -> *mut OH_AVFormat;
    track_format = "OH_AVSource_GetTrackFormat": fn(*mut OH_AVSource, u32) -> *mut OH_AVFormat;
});

functions!(Demux from "libnative_media_avdemuxer.so" {
    create = "OH_AVDemuxer_CreateWithSource": fn(*mut OH_AVSource) -> *mut OH_AVDemuxer;
    destroy = "OH_AVDemuxer_Destroy": fn(*mut OH_AVDemuxer) -> AvError;
    select_track = "OH_AVDemuxer_SelectTrackByID": fn(*mut OH_AVDemuxer, u32) -> AvError;
    read_sample = "OH_AVDemuxer_ReadSampleBuffer": fn(*mut OH_AVDemuxer, u32, *mut OH_AVBuffer) -> AvError;
    seek = "OH_AVDemuxer_SeekToTime": fn(*mut OH_AVDemuxer, i64, c_int) -> AvError;
});

functions!(Vdec from "libnative_media_vdec.so" {
    create = "OH_VideoDecoder_CreateByMime": fn(*const c_char) -> *mut OH_AVCodec;
    destroy = "OH_VideoDecoder_Destroy": fn(*mut OH_AVCodec) -> AvError;
    configure = "OH_VideoDecoder_Configure": fn(*mut OH_AVCodec, *mut OH_AVFormat) -> AvError;
    set_surface = "OH_VideoDecoder_SetSurface": fn(*mut OH_AVCodec, *mut OHNativeWindow) -> AvError;
    prepare = "OH_VideoDecoder_Prepare": fn(*mut OH_AVCodec) -> AvError;
    start = "OH_VideoDecoder_Start": fn(*mut OH_AVCodec) -> AvError;
    stop = "OH_VideoDecoder_Stop": fn(*mut OH_AVCodec) -> AvError;
    flush = "OH_VideoDecoder_Flush": fn(*mut OH_AVCodec) -> AvError;
    output_description = "OH_VideoDecoder_GetOutputDescription": fn(*mut OH_AVCodec) -> *mut OH_AVFormat;
    query_input = "OH_VideoDecoder_QueryInputBuffer": fn(*mut OH_AVCodec, *mut u32, i64) -> AvError;
    input_buffer = "OH_VideoDecoder_GetInputBuffer": fn(*mut OH_AVCodec, u32) -> *mut OH_AVBuffer;
    push_input = "OH_VideoDecoder_PushInputBuffer": fn(*mut OH_AVCodec, u32) -> AvError;
    query_output = "OH_VideoDecoder_QueryOutputBuffer": fn(*mut OH_AVCodec, *mut u32, i64) -> AvError;
    output_buffer = "OH_VideoDecoder_GetOutputBuffer": fn(*mut OH_AVCodec, u32) -> *mut OH_AVBuffer;
    render_output = "OH_VideoDecoder_RenderOutputBuffer": fn(*mut OH_AVCodec, u32) -> AvError;
    free_output = "OH_VideoDecoder_FreeOutputBuffer": fn(*mut OH_AVCodec, u32) -> AvError;
});

functions!(Image from "libnative_image.so" {
    consumer_create = "OH_ConsumerSurface_Create": fn() -> *mut OH_NativeImage;
    consumer_set_usage = "OH_ConsumerSurface_SetDefaultUsage": fn(*mut OH_NativeImage, u64) -> i32;
    consumer_set_size = "OH_ConsumerSurface_SetDefaultSize": fn(*mut OH_NativeImage, i32, i32) -> i32;
    window = "OH_NativeImage_AcquireNativeWindow": fn(*mut OH_NativeImage) -> *mut OHNativeWindow;
    set_listener = "OH_NativeImage_SetOnFrameAvailableListener": fn(*mut OH_NativeImage, FrameListener) -> i32;
    acquire = "OH_NativeImage_AcquireNativeWindowBuffer":
        fn(*mut OH_NativeImage, *mut *mut OHNativeWindowBuffer, *mut c_int) -> i32;
    release = "OH_NativeImage_ReleaseNativeWindowBuffer": fn(*mut OH_NativeImage, *mut OHNativeWindowBuffer, c_int) -> i32;
    destroy = "OH_NativeImage_Destroy": fn(*mut *mut OH_NativeImage);
});

functions!(Buffer from "libnative_buffer.so" {
    from_window_buffer = "OH_NativeBuffer_FromNativeWindowBuffer":
        fn(*mut OHNativeWindowBuffer, *mut *mut NativeBuffer) -> i32;
    reference = "OH_NativeBuffer_Reference": fn(*mut NativeBuffer) -> i32;
    config = "OH_NativeBuffer_GetConfig": fn(*mut NativeBuffer, *mut BufferConfig);
});

/// `OH_NativeBuffer_Config`.
#[repr(C)]
#[derive(Default)]
pub(super) struct BufferConfig {
    width: i32,
    height: i32,
    format: i32,
    usage: i32,
    stride: i32,
}

/// The format keys, the values of the codec base library's `OH_MD_KEY_*`.
struct Keys {
    track_count: CString,
    track_type: CString,
    duration: CString,
    codec_mime: CString,
    width: CString,
    height: CString,
    pixel_format: CString,
    stride: CString,
    slice_height: CString,
    picture_width: CString,
    picture_height: CString,
    crop_left: CString,
    crop_top: CString,
    range_flag: CString,
    matrix_coefficients: CString,
    enable_sync_mode: CString,
}

impl Keys {
    fn load() -> Result<Keys, String> {
        const LIBRARY: &str = "libnative_media_codecbase.so";
        // SAFETY: a system library, as in `functions!`; each symbol is a
        // `const char *` that names a key, which is copied.
        unsafe {
            let library = Library::new(LIBRARY).map_err(|error| format!("{LIBRARY}: {error}"))?;
            let key = |name: &str| -> Result<CString, String> {
                let symbol = format!("OH_MD_KEY_{name}\0");
                let variable = *library
                    .get::<*const *const c_char>(symbol.as_bytes())
                    .map_err(|error| format!("OH_MD_KEY_{name}: {error}"))?;
                if variable.is_null() || (*variable).is_null() {
                    return Err(format!("OH_MD_KEY_{name} is null"));
                }
                Ok(CStr::from_ptr(*variable).to_owned())
            };
            Ok(Keys {
                track_count: key("TRACK_COUNT")?,
                track_type: key("TRACK_TYPE")?,
                duration: key("DURATION")?,
                codec_mime: key("CODEC_MIME")?,
                width: key("WIDTH")?,
                height: key("HEIGHT")?,
                pixel_format: key("PIXEL_FORMAT")?,
                stride: key("VIDEO_STRIDE")?,
                slice_height: key("VIDEO_SLICE_HEIGHT")?,
                picture_width: key("VIDEO_PIC_WIDTH")?,
                picture_height: key("VIDEO_PIC_HEIGHT")?,
                crop_left: key("VIDEO_CROP_LEFT")?,
                crop_top: key("VIDEO_CROP_TOP")?,
                range_flag: key("RANGE_FLAG")?,
                matrix_coefficients: key("MATRIX_COEFFICIENTS")?,
                enable_sync_mode: key("ENABLE_SYNC_MODE")?,
            })
        }
    }
}

/// The format keys that the player reads.
#[derive(Clone, Copy, Debug)]
pub(super) enum Key {
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

/// The functions and keys.
pub(super) struct Ohos {
    core: Core,
    source: Source,
    demux: Demux,
    vdec: Vdec,
    image: Image,
    buffer: Buffer,
    keys: Keys,
}

impl Ohos {
    /// The functions, looked up once.
    ///
    /// # Errors
    ///
    /// Before API level 20, a function isn't there.
    pub(super) fn get() -> Result<&'static Ohos, String> {
        static OHOS: OnceLock<Result<Ohos, String>> = OnceLock::new();
        OHOS.get_or_init(|| {
            Ok(Ohos {
                core: Core::load()?,
                source: Source::load()?,
                demux: Demux::load()?,
                vdec: Vdec::load()?,
                image: Image::load()?,
                buffer: Buffer::load()?,
                keys: Keys::load()?,
            })
        })
        .as_ref()
        .map_err(|error| format!("OpenHarmony's AVCodec API isn't there (it needs API level 20): {error}"))
    }

    fn key(&self, key: Key) -> &CStr {
        let keys = &self.keys;
        match key {
            Key::Width => &keys.width,
            Key::Height => &keys.height,
            Key::PixelFormat => &keys.pixel_format,
            Key::Stride => &keys.stride,
            Key::SliceHeight => &keys.slice_height,
            Key::PictureWidth => &keys.picture_width,
            Key::PictureHeight => &keys.picture_height,
            Key::CropLeft => &keys.crop_left,
            Key::CropTop => &keys.crop_top,
            Key::RangeFlag => &keys.range_flag,
            Key::MatrixCoefficients => &keys.matrix_coefficients,
        }
    }
}

/// Describes a failed call.
fn check(status: AvError, what: &str) -> Result<(), String> {
    if status == OK { Ok(()) } else { Err(format!("{what} failed: {status}")) }
}

/// What a data source's callback reads: the video's bytes, and why a read
/// failed last.
struct Reader {
    ohos: &'static Ohos,
    bytes: Arc<ByteSource>,
    error: Mutex<Option<io::Error>>,
}

unsafe extern "C" fn read_at(buffer: *mut OH_AVBuffer, length: i32, offset: i64, userdata: *mut c_void) -> i32 {
    // SAFETY: the source's reader, which outlives it.
    let reader = unsafe { &*userdata.cast::<Reader>() };
    let (Ok(offset), Ok(length)) = (u64::try_from(offset), usize::try_from(length)) else { return READ_FAILED };
    let core = &reader.ohos.core;
    // SAFETY: the buffer the source reads into, which it doesn't touch
    // until the call returns, of at least `length` bytes.
    let data = unsafe {
        let address = (core.buffer_addr)(buffer);
        let capacity = usize::try_from((core.buffer_capacity)(buffer)).unwrap_or(0);
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
    ohos: &'static Ohos,
    raw: NonNull<OH_AVSource>,
    /// What the source reads through, which outlives it.
    _desc: Box<DataSourceDesc>,
    reader: Box<Reader>,
}

// SAFETY: the AVCodec objects may be used from any thread, one at a time.
unsafe impl Send for AvSource {}

impl AvSource {
    /// A source of `bytes`' video. Blocks while it reads the container's
    /// header.
    pub(super) fn new(ohos: &'static Ohos, bytes: Arc<ByteSource>) -> Result<AvSource, String> {
        let size = bytes.size().map_err(|error| error.to_string())?;
        // An unknown size is a stream's.
        let size = size.and_then(|size| i64::try_from(size).ok()).unwrap_or(-1);
        let reader = Box::new(Reader { ohos, bytes, error: Mutex::new(None) });
        let mut desc = Box::new(DataSourceDesc { size, read_at });
        // SAFETY: the description and the reader outlive the source.
        let raw = unsafe { (ohos.source.create)(&raw mut *desc, (&raw const *reader).cast_mut().cast()) };
        let Some(raw) = NonNull::new(raw) else {
            let error = reader.error.lock().take();
            return Err(match error {
                Some(error) => format!("reading the video's container failed: {error}"),
                None => "reading the video's container failed".to_owned(),
            });
        };
        Ok(AvSource { ohos, raw, _desc: desc, reader })
    }

    /// Why a read failed last.
    pub(super) fn take_error(&self) -> Option<io::Error> {
        self.reader.error.lock().take()
    }

    fn source_format(&self) -> Option<Format> {
        // SAFETY: the source is alive; the format is the caller's.
        let raw = NonNull::new(unsafe { (self.ohos.source.source_format)(self.raw.as_ptr()) })?;
        Some(Format { ohos: self.ohos, raw })
    }

    fn track_count(&self) -> u32 {
        let count = self.source_format().and_then(|format| format.int_key(&self.ohos.keys.track_count));
        count.and_then(|count| u32::try_from(count).ok()).unwrap_or(0)
    }

    /// The video's duration in microseconds, if the container says.
    fn duration_us(&self) -> Option<i64> {
        let format = self.source_format()?;
        let mut value = 0;
        // SAFETY: the format is alive, and the key a C string.
        let found = unsafe {
            (self.ohos.core.format_get_long)(format.raw.as_ptr(), self.ohos.keys.duration.as_ptr(), &raw mut value)
        };
        found.then_some(value).filter(|duration| *duration > 0)
    }

    /// The first video track, and its format.
    pub(super) fn video_track(&self) -> Option<(u32, Format)> {
        (0..self.track_count()).find_map(|track| {
            // SAFETY: the source is alive; the format is the caller's.
            let raw = NonNull::new(unsafe { (self.ohos.source.track_format)(self.raw.as_ptr(), track) })?;
            let format = Format { ohos: self.ohos, raw };
            (format.int_key(&self.ohos.keys.track_type) == Some(MEDIA_TYPE_VIDEO)).then_some((track, format))
        })
    }
}

impl Drop for AvSource {
    fn drop(&mut self) {
        // SAFETY: the source is this one's, and no demuxer reads it any more.
        unsafe { (self.ohos.source.destroy)(self.raw.as_ptr()) };
    }
}

/// An `OH_AVFormat`.
pub(super) struct Format {
    ohos: &'static Ohos,
    raw: NonNull<OH_AVFormat>,
}

// SAFETY: as for `AvSource`.
unsafe impl Send for Format {}

impl Format {
    fn int_key(&self, key: &CStr) -> Option<i32> {
        let mut value = 0;
        // SAFETY: the format is alive, and the key a C string.
        unsafe { (self.ohos.core.format_get_int)(self.raw.as_ptr(), key.as_ptr(), &raw mut value) }.then_some(value)
    }

    pub(super) fn int(&self, key: Key) -> Option<i32> {
        self.int_key(self.ohos.key(key))
    }

    pub(super) fn mime(&self) -> Option<String> {
        let mut value = std::ptr::null();
        // SAFETY: as for `int`; the string is the format's, and copied
        // before it goes.
        unsafe {
            let key = self.ohos.keys.codec_mime.as_ptr();
            if !(self.ohos.core.format_get_string)(self.raw.as_ptr(), key, &raw mut value) || value.is_null() {
                return None;
            }
            Some(CStr::from_ptr(value).to_string_lossy().into_owned())
        }
    }

    /// A copy of the format.
    fn copy(&self) -> Result<Format, String> {
        // SAFETY: no preconditions; then both formats are alive.
        unsafe {
            let raw = NonNull::new((self.ohos.core.format_create)()).ok_or("OH_AVFormat_Create failed")?;
            let copy = Format { ohos: self.ohos, raw };
            if !(self.ohos.core.format_copy)(raw.as_ptr(), self.raw.as_ptr()) {
                return Err("copying a format failed".to_owned());
            }
            Ok(copy)
        }
    }

    fn set_int(&self, key: &CStr, value: i32) {
        // SAFETY: the format is alive, and the key a C string.
        unsafe { (self.ohos.core.format_set_int)(self.raw.as_ptr(), key.as_ptr(), value) };
    }
}

impl Drop for Format {
    fn drop(&mut self) {
        // SAFETY: the format is this one's.
        unsafe { (self.ohos.core.format_destroy)(self.raw.as_ptr()) };
    }
}

/// An `OH_AVDemuxer` of one track.
pub(super) struct Demuxer {
    ohos: &'static Ohos,
    raw: NonNull<OH_AVDemuxer>,
    track: u32,
    /// The video's duration in microseconds, past which seeks fail.
    duration_us: Option<i64>,
}

// SAFETY: as for `AvSource`.
unsafe impl Send for Demuxer {}

impl Demuxer {
    /// A demuxer of `source`'s `track`; the source must outlive it.
    pub(super) fn new(ohos: &'static Ohos, source: &AvSource, track: u32) -> Result<Demuxer, String> {
        // SAFETY: the source is alive, and the caller keeps it.
        let raw = NonNull::new(unsafe { (ohos.demux.create)(source.raw.as_ptr()) })
            .ok_or("OH_AVDemuxer_CreateWithSource failed")?;
        let demuxer = Demuxer { ohos, raw, track, duration_us: source.duration_us() };
        // SAFETY: the demuxer is alive.
        check(unsafe { (ohos.demux.select_track)(raw.as_ptr(), track) }, "selecting the video track")?;
        Ok(demuxer)
    }

    /// Reads the next sample into `buffer`, a decoder's input buffer, with
    /// its time and flags, the end of the track's among them.
    fn read_sample(&self, buffer: NonNull<OH_AVBuffer>) -> Result<(), String> {
        // SAFETY: the demuxer is alive, and the buffer is the caller's.
        let status = unsafe { (self.ohos.demux.read_sample)(self.raw.as_ptr(), self.track, buffer.as_ptr()) };
        check(status, "reading a sample")
    }

    /// Moves to the sync sample at or before `time_us`, or the last one
    /// for a time after the end.
    pub(super) fn seek_to(&self, time_us: i64) -> Result<(), String> {
        let time_us = self.duration_us.map_or(time_us, |duration| time_us.min(duration - 1));
        // SAFETY: the demuxer is alive.
        let status = unsafe { (self.ohos.demux.seek)(self.raw.as_ptr(), time_us / 1000, SEEK_PREVIOUS_SYNC) };
        check(status, "seeking")
    }
}

impl Drop for Demuxer {
    fn drop(&mut self) {
        // SAFETY: the demuxer is this one's.
        unsafe { (self.ohos.demux.destroy)(self.raw.as_ptr()) };
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
    ohos: &'static Ohos,
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
        ohos: &'static Ohos,
        format: &Format,
        pixel_format: Option<i32>,
        window: Option<NonNull<OHNativeWindow>>,
    ) -> Result<Decoder, String> {
        let vdec = &ohos.vdec;
        let mime = format.mime().ok_or("the video track has no MIME type")?;
        let mime_c = CString::new(mime.clone()).map_err(|error| error.to_string())?;
        // SAFETY: a C string.
        let raw =
            NonNull::new(unsafe { (vdec.create)(mime_c.as_ptr()) }).ok_or_else(|| format!("no decoder of {mime}"))?;
        let decoder = Decoder { ohos, raw };
        let config = format.copy()?;
        config.set_int(&ohos.keys.enable_sync_mode, 1);
        if let Some(pixel_format) = pixel_format {
            config.set_int(&ohos.keys.pixel_format, pixel_format);
        }
        // SAFETY: the decoder is new, and the format alive.
        let configured = unsafe { (vdec.configure)(raw.as_ptr(), config.raw.as_ptr()) };
        check(configured, &format!("configuring the {mime} decoder"))?;
        if let Some(window) = window {
            // SAFETY: the decoder is configured, and the caller keeps the
            // window.
            let set = unsafe { (vdec.set_surface)(raw.as_ptr(), window.as_ptr()) };
            check(set, "setting the decoder's surface")?;
        }
        // SAFETY: the decoder is configured.
        unsafe {
            check((vdec.prepare)(raw.as_ptr()), &format!("preparing the {mime} decoder"))?;
            check((vdec.start)(raw.as_ptr()), &format!("starting the {mime} decoder"))?;
        }
        Ok(decoder)
    }

    /// Takes back the buffers the decoder holds, for a seek, and starts it
    /// again.
    pub(super) fn flush(&self) -> Result<(), String> {
        // SAFETY: the decoder is alive.
        unsafe {
            check((self.ohos.vdec.flush)(self.raw.as_ptr()), "flushing the decoder")?;
            check((self.ohos.vdec.start)(self.raw.as_ptr()), "starting the decoder again")
        }
    }

    /// Fills a free input buffer with `demuxer`'s next sample: whether one
    /// was free, and whether the sample ended the track.
    pub(super) fn queue_input(&self, demuxer: &Demuxer) -> Result<Option<bool>, String> {
        let vdec = &self.ohos.vdec;
        let mut index = 0;
        // SAFETY: the decoder is alive.
        let status = unsafe { (vdec.query_input)(self.raw.as_ptr(), &raw mut index, 0) };
        if status == TRY_AGAIN_LATER {
            return Ok(None);
        }
        check(status, "querying an input buffer")?;
        // SAFETY: an input buffer the decoder gave, which is the caller's
        // until it is pushed.
        let buffer = NonNull::new(unsafe { (vdec.input_buffer)(self.raw.as_ptr(), index) })
            .ok_or("the decoder has no input buffer")?;
        demuxer.read_sample(buffer)?;
        let mut attr = BufferAttr::default();
        // SAFETY: as above.
        unsafe {
            check((self.ohos.core.buffer_attr)(buffer.as_ptr(), &raw mut attr), "reading a sample's attributes")?;
            check((vdec.push_input)(self.raw.as_ptr(), index), "queueing a sample")?;
        }
        Ok(Some(attr.flags & END_OF_STREAM != 0))
    }

    /// The next output, waiting up to `timeout` for one.
    pub(super) fn dequeue_output(&self, timeout: Duration) -> Result<Output, String> {
        let vdec = &self.ohos.vdec;
        let mut index = 0;
        let timeout = i64::try_from(timeout.as_micros()).unwrap_or(i64::MAX);
        // SAFETY: the decoder is alive.
        let status = unsafe { (vdec.query_output)(self.raw.as_ptr(), &raw mut index, timeout) };
        match status {
            OK => {}
            TRY_AGAIN_LATER => return Ok(Output::None),
            STREAM_CHANGED => return Ok(Output::FormatChanged),
            status => return Err(format!("decoding failed: {status}")),
        }
        let mut attr = BufferAttr::default();
        // SAFETY: an output buffer the decoder gave.
        unsafe {
            let buffer = NonNull::new((vdec.output_buffer)(self.raw.as_ptr(), index))
                .ok_or("the decoder has no output buffer")?;
            check((self.ohos.core.buffer_attr)(buffer.as_ptr(), &raw mut attr), "reading a frame's attributes")?;
        }
        Ok(Output::Buffer(index, attr))
    }

    /// The bytes of output buffer `index`, of a decoder into memory, valid
    /// until it goes back.
    pub(super) fn output_bytes(&self, index: u32, attr: BufferAttr) -> Result<&[u8], String> {
        // SAFETY: an output buffer the decoder gave, whose bytes are the
        // caller's until it goes back.
        unsafe {
            let buffer = NonNull::new((self.ohos.vdec.output_buffer)(self.raw.as_ptr(), index))
                .ok_or("the decoder has no output buffer")?;
            let address = (self.ohos.core.buffer_addr)(buffer.as_ptr());
            if address.is_null() {
                return Err("a decoded frame has no memory".to_owned());
            }
            let capacity = usize::try_from((self.ohos.core.buffer_capacity)(buffer.as_ptr())).unwrap_or(0);
            let offset = usize::try_from(attr.offset).unwrap_or(0).min(capacity);
            let size = usize::try_from(attr.size).unwrap_or(0).min(capacity - offset);
            Ok(std::slice::from_raw_parts(address.add(offset), size))
        }
    }

    pub(super) fn output_format(&self) -> Option<Format> {
        // SAFETY: the decoder is alive; the format is the caller's.
        let raw = NonNull::new(unsafe { (self.ohos.vdec.output_description)(self.raw.as_ptr()) })?;
        Some(Format { ohos: self.ohos, raw })
    }

    /// Sends output buffer `index` to the decoder's surface.
    pub(super) fn render_output(&self, index: u32) -> Result<(), String> {
        // SAFETY: an output buffer the decoder gave.
        check(unsafe { (self.ohos.vdec.render_output)(self.raw.as_ptr(), index) }, "rendering a decoded frame")
    }

    /// Gives output buffer `index` back unseen.
    pub(super) fn free_output(&self, index: u32) -> Result<(), String> {
        // SAFETY: an output buffer the decoder gave.
        check(unsafe { (self.ohos.vdec.free_output)(self.raw.as_ptr(), index) }, "releasing a decoded frame")
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: the decoder is this one's.
        unsafe {
            (self.ohos.vdec.stop)(self.raw.as_ptr());
            (self.ohos.vdec.destroy)(self.raw.as_ptr());
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
    ohos: &'static Ohos,
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
    pub(super) fn new(ohos: &'static Ohos, width: i32, height: i32) -> Result<Surface, String> {
        let image = &ohos.image;
        // SAFETY: no preconditions.
        let raw = NonNull::new(unsafe { (image.consumer_create)() }).ok_or("OH_ConsumerSurface_Create failed")?;
        let mut surface = Surface { ohos, raw, window: NonNull::dangling(), signal: Box::default() };
        let listener = FrameListener { context: (&raw const *surface.signal).cast_mut().cast(), on_frame_available };
        // SAFETY: the surface is alive, and the listener's context outlives
        // it; the window is the surface's.
        unsafe {
            let status = (image.consumer_set_usage)(raw.as_ptr(), USAGE_HW_TEXTURE | USAGE_MEM_DMA);
            native_check(status, "setting the surface's usage")?;
            native_check((image.consumer_set_size)(raw.as_ptr(), width, height), "setting the surface's size")?;
            native_check((image.set_listener)(raw.as_ptr(), listener), "listening for frames")?;
            surface.window = NonNull::new((image.window)(raw.as_ptr())).ok_or("the surface has no window")?;
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
            let status = unsafe { (self.ohos.image.acquire)(self.raw.as_ptr(), &raw mut buffer, &raw mut fence) };
            match status {
                NATIVE_OK => {
                    let window_buffer = NonNull::new(buffer).ok_or("the surface gave no buffer")?;
                    let acquired = SurfaceBuffer { surface: self.raw, ohos: self.ohos, window_buffer };
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
        unsafe { (self.ohos.image.destroy)(&raw mut raw) };
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
    ohos: &'static Ohos,
    surface: NonNull<OH_NativeImage>,
    window_buffer: NonNull<OHNativeWindowBuffer>,
}

// SAFETY: as for `AvSource`.
unsafe impl Send for SurfaceBuffer {}

impl SurfaceBuffer {
    /// The frame's native buffer's size in pixels.
    pub(super) fn size(&self) -> Result<(u32, u32), String> {
        let mut buffer = std::ptr::null_mut();
        let mut config = BufferConfig::default();
        // SAFETY: the window buffer is alive, and its native buffer with it.
        unsafe {
            let status = (self.ohos.buffer.from_window_buffer)(self.window_buffer.as_ptr(), &raw mut buffer);
            native_check(status, "OH_NativeBuffer_FromNativeWindowBuffer")?;
            let buffer = NonNull::new(buffer).ok_or("the frame has no native buffer")?;
            (self.ohos.buffer.config)(buffer.as_ptr(), &raw mut config);
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
            let status = (self.ohos.buffer.from_window_buffer)(self.window_buffer.as_ptr(), &raw mut buffer);
            native_check(status, "OH_NativeBuffer_FromNativeWindowBuffer")?;
            let buffer = NonNull::new(buffer).ok_or("the frame has no native buffer")?;
            native_check((self.ohos.buffer.reference)(buffer.as_ptr()), "OH_NativeBuffer_Reference")?;
            Ok(buffer)
        }
    }
}

impl Drop for SurfaceBuffer {
    fn drop(&mut self) {
        // SAFETY: a buffer the surface gave, which goes back once.
        unsafe { (self.ohos.image.release)(self.surface.as_ptr(), self.window_buffer.as_ptr(), -1) };
    }
}
