//! The parts of GStreamer's, GObject's and GLib's C APIs that the player
//! uses, loaded at run time so that the library runs, without video, where
//! GStreamer isn't installed. The types and constants are those of the
//! GStreamer 1.x headers, whose ABI stays stable across the 1.x releases.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::ptr::{self, NonNull};
use std::sync::OnceLock;

use libloading::Library;

pub(super) type GBoolean = c_int;
pub(super) type GType = usize;
pub(super) type Callback = unsafe extern "C" fn();
pub(super) type DestroyNotify = unsafe extern "C" fn(*mut c_void);
pub(super) type BusSyncHandler = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> c_int;

pub(super) const STATE_NULL: c_int = 1;
pub(super) const STATE_PAUSED: c_int = 3;
pub(super) const STATE_PLAYING: c_int = 4;
pub(super) const STATE_CHANGE_FAILURE: c_int = 0;

pub(super) const FORMAT_TIME: c_int = 3;
pub(super) const SEEK_FLAG_FLUSH: c_int = 1 << 0;
pub(super) const SEEK_FLAG_ACCURATE: c_int = 1 << 1;
pub(super) const SEEK_FLAG_KEY_UNIT: c_int = 1 << 2;
pub(super) const CLOCK_TIME_NONE: u64 = u64::MAX;

pub(super) const MESSAGE_EOS: c_uint = 1 << 0;
pub(super) const MESSAGE_ERROR: c_uint = 1 << 1;
pub(super) const MESSAGE_ELEMENT: c_uint = 1 << 15;
pub(super) const MESSAGE_ASYNC_DONE: c_uint = 1 << 21;
pub(super) const MESSAGE_STREAM_COLLECTION: c_uint = (1 << 31) + 4;
pub(super) const BUS_PASS: c_int = 1;

pub(super) const STREAM_TYPE_VIDEO: c_int = 1 << 2;
pub(super) const APP_STREAM_TYPE_RANDOM_ACCESS: c_int = 2;
pub(super) const FLOW_OK: c_int = 0;
pub(super) const FLOW_NOT_NEGOTIATED: c_int = -4;
pub(super) const PAD_LINK_OK: c_int = 0;
pub(super) const MAP_READ: c_int = 1;
pub(super) const MAP_WRITE: c_int = 2;

pub(super) const COLOR_RANGE_0_255: c_int = 1;
pub(super) const COLOR_MATRIX_FCC: c_int = 2;
pub(super) const COLOR_MATRIX_BT709: c_int = 3;
pub(super) const COLOR_MATRIX_BT601: c_int = 4;
pub(super) const COLOR_MATRIX_SMPTE240M: c_int = 5;
pub(super) const COLOR_MATRIX_BT2020: c_int = 6;

/// `GstMiniObject`, the header of buffers, messages, caps and samples.
#[repr(C)]
pub(super) struct MiniObject {
    pub(super) type_: GType,
    pub(super) refcount: c_int,
    pub(super) lockstate: c_int,
    pub(super) flags: c_uint,
    pub(super) copy: *mut c_void,
    pub(super) dispose: *mut c_void,
    pub(super) free: *mut c_void,
    pub(super) priv_uint: c_uint,
    pub(super) priv_pointer: *mut c_void,
}

/// The start of `GstMessage`.
#[repr(C)]
pub(super) struct Message {
    pub(super) mini_object: MiniObject,
    pub(super) type_: c_uint,
}

/// The start of `GstBuffer`.
#[repr(C)]
pub(super) struct Buffer {
    pub(super) mini_object: MiniObject,
    pub(super) pool: *mut c_void,
    pub(super) pts: u64,
}

/// `GstMapInfo`.
#[repr(C)]
pub(super) struct MapInfo {
    pub(super) memory: *mut c_void,
    pub(super) flags: c_int,
    pub(super) data: *mut u8,
    pub(super) size: usize,
    pub(super) maxsize: usize,
    pub(super) user_data: [*mut c_void; 4],
    pub(super) reserved: [*mut c_void; 4],
}

impl MapInfo {
    pub(super) fn new() -> MapInfo {
        MapInfo {
            memory: ptr::null_mut(),
            flags: 0,
            data: ptr::null_mut(),
            size: 0,
            maxsize: 0,
            user_data: [ptr::null_mut(); 4],
            reserved: [ptr::null_mut(); 4],
        }
    }
}

/// The start of `GstVideoMeta`.
#[repr(C)]
pub(super) struct VideoMeta {
    pub(super) meta_flags: c_uint,
    pub(super) meta_info: *const c_void,
    pub(super) buffer: *mut c_void,
    pub(super) flags: c_uint,
    pub(super) format: c_int,
    pub(super) id: c_int,
    pub(super) width: c_uint,
    pub(super) height: c_uint,
    pub(super) n_planes: c_uint,
    pub(super) offset: [usize; 4],
    pub(super) stride: [c_int; 4],
}

/// `GstVideoColorimetry`.
#[repr(C)]
#[derive(Default)]
pub(super) struct VideoColorimetry {
    pub(super) range: c_int,
    pub(super) matrix: c_int,
    pub(super) transfer: c_int,
    pub(super) primaries: c_int,
}

/// `GError`.
#[repr(C)]
pub(super) struct GError {
    pub(super) domain: u32,
    pub(super) code: c_int,
    pub(super) message: *mut c_char,
}

/// `GList`.
#[repr(C)]
pub(super) struct GList {
    pub(super) data: *mut c_void,
    pub(super) next: *mut GList,
    pub(super) prev: *mut GList,
}

/// `GstAppSrcCallbacks`.
#[repr(C)]
pub(super) struct AppSrcCallbacks {
    pub(super) need_data: Option<unsafe extern "C" fn(*mut c_void, c_uint, *mut c_void)>,
    pub(super) enough_data: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    pub(super) seek_data: Option<unsafe extern "C" fn(*mut c_void, u64, *mut c_void) -> GBoolean>,
    pub(super) reserved: [*mut c_void; 4],
}

/// `GstAppSinkCallbacks`.
#[repr(C)]
pub(super) struct AppSinkCallbacks {
    pub(super) eos: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    pub(super) new_preroll: Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int>,
    pub(super) new_sample: Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int>,
    pub(super) new_event: Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> GBoolean>,
    pub(super) propose_allocation: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> GBoolean>,
    pub(super) reserved: [*mut c_void; 2],
}

/// The functions of the libraries, resolved.
pub(super) struct Api {
    pub(super) g_free: unsafe extern "C" fn(*mut c_void),
    pub(super) g_error_free: unsafe extern "C" fn(*mut GError),
    pub(super) g_list_append: unsafe extern "C" fn(*mut GList, *mut c_void) -> *mut GList,
    pub(super) g_list_free: unsafe extern "C" fn(*mut GList),
    pub(super) g_signal_connect_data: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        Callback,
        *mut c_void,
        Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
        c_int,
    ) -> std::ffi::c_ulong,

    pub(super) init_check: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char, *mut *mut GError) -> GBoolean,
    pub(super) object_ref: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(super) object_ref_sink: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(super) object_unref: unsafe extern "C" fn(*mut c_void),
    pub(super) mini_object_unref: unsafe extern "C" fn(*mut c_void),
    pub(super) element_factory_find: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    pub(super) element_factory_make: unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void,
    pub(super) pipeline_new: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    pub(super) bin_add: unsafe extern "C" fn(*mut c_void, *mut c_void) -> GBoolean,
    pub(super) element_link: unsafe extern "C" fn(*mut c_void, *mut c_void) -> GBoolean,
    pub(super) element_set_state: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    pub(super) element_get_bus: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(super) element_get_static_pad: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    pub(super) element_seek_simple: unsafe extern "C" fn(*mut c_void, c_int, c_int, i64) -> GBoolean,
    pub(super) element_query_position: unsafe extern "C" fn(*mut c_void, c_int, *mut i64) -> GBoolean,
    pub(super) element_send_event: unsafe extern "C" fn(*mut c_void, *mut c_void) -> GBoolean,
    pub(super) util_set_object_arg: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char),
    pub(super) bus_timed_pop: unsafe extern "C" fn(*mut c_void, u64) -> *mut c_void,
    pub(super) bus_post: unsafe extern "C" fn(*mut c_void, *mut c_void) -> GBoolean,
    pub(super) bus_set_sync_handler:
        unsafe extern "C" fn(*mut c_void, Option<BusSyncHandler>, *mut c_void, Option<DestroyNotify>),
    pub(super) message_new_application: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void,
    pub(super) message_parse_error: unsafe extern "C" fn(*mut c_void, *mut *mut GError, *mut *mut c_char),
    pub(super) message_parse_error_details: unsafe extern "C" fn(*mut c_void, *mut *const c_void),
    pub(super) message_get_structure: unsafe extern "C" fn(*mut c_void) -> *const c_void,
    pub(super) message_parse_stream_collection: unsafe extern "C" fn(*mut c_void, *mut *mut c_void),
    pub(super) stream_collection_get_size: unsafe extern "C" fn(*mut c_void) -> c_uint,
    pub(super) stream_collection_get_stream: unsafe extern "C" fn(*mut c_void, c_uint) -> *mut c_void,
    pub(super) stream_get_stream_type: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub(super) stream_get_stream_id: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    pub(super) stream_get_caps: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(super) event_new_select_streams: unsafe extern "C" fn(*mut GList) -> *mut c_void,
    pub(super) pad_get_stream: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(super) pad_get_current_caps: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(super) pad_is_linked: unsafe extern "C" fn(*mut c_void) -> GBoolean,
    pub(super) pad_link: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int,
    pub(super) caps_from_string: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    pub(super) caps_get_structure: unsafe extern "C" fn(*const c_void, c_uint) -> *const c_void,
    pub(super) caps_get_features: unsafe extern "C" fn(*const c_void, c_uint) -> *const c_void,
    pub(super) caps_features_contains: unsafe extern "C" fn(*const c_void, *const c_char) -> GBoolean,
    pub(super) structure_new_empty: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    pub(super) structure_get_name: unsafe extern "C" fn(*const c_void) -> *const c_char,
    pub(super) structure_has_name: unsafe extern "C" fn(*const c_void, *const c_char) -> GBoolean,
    pub(super) structure_get_string: unsafe extern "C" fn(*const c_void, *const c_char) -> *const c_char,
    pub(super) structure_get_int: unsafe extern "C" fn(*const c_void, *const c_char, *mut c_int) -> GBoolean,
    pub(super) sample_get_buffer: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(super) sample_get_caps: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(super) sample_get_segment: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(super) segment_to_stream_time: unsafe extern "C" fn(*const c_void, c_int, u64) -> u64,
    pub(super) buffer_new_allocate: unsafe extern "C" fn(*mut c_void, usize, *mut c_void) -> *mut c_void,
    pub(super) buffer_set_size: unsafe extern "C" fn(*mut c_void, isize),
    pub(super) buffer_map: unsafe extern "C" fn(*mut c_void, *mut MapInfo, c_int) -> GBoolean,
    pub(super) buffer_unmap: unsafe extern "C" fn(*mut c_void, *mut MapInfo),
    pub(super) buffer_find_memory:
        unsafe extern "C" fn(*mut c_void, usize, usize, *mut c_uint, *mut c_uint, *mut usize) -> GBoolean,
    pub(super) buffer_peek_memory: unsafe extern "C" fn(*mut c_void, c_uint) -> *mut c_void,
    pub(super) memory_get_sizes: unsafe extern "C" fn(*mut c_void, *mut usize, *mut usize) -> usize,
    pub(super) query_add_allocation_meta: unsafe extern "C" fn(*mut c_void, GType, *const c_void),

    pub(super) app_src_set_callbacks:
        unsafe extern "C" fn(*mut c_void, *mut AppSrcCallbacks, *mut c_void, Option<DestroyNotify>),
    pub(super) app_src_set_size: unsafe extern "C" fn(*mut c_void, i64),
    pub(super) app_src_set_stream_type: unsafe extern "C" fn(*mut c_void, c_int),
    pub(super) app_src_push_buffer: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int,
    pub(super) app_src_end_of_stream: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub(super) app_sink_set_callbacks:
        unsafe extern "C" fn(*mut c_void, *mut AppSinkCallbacks, *mut c_void, Option<DestroyNotify>),
    pub(super) app_sink_set_caps: unsafe extern "C" fn(*mut c_void, *const c_void),
    pub(super) app_sink_pull_sample: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub(super) app_sink_pull_preroll: unsafe extern "C" fn(*mut c_void) -> *mut c_void,

    pub(super) buffer_get_video_meta: unsafe extern "C" fn(*mut c_void) -> *mut VideoMeta,
    pub(super) video_meta_api_get_type: unsafe extern "C" fn() -> GType,
    pub(super) video_colorimetry_from_string: unsafe extern "C" fn(*mut VideoColorimetry, *const c_char) -> GBoolean,

    pub(super) is_dmabuf_memory: unsafe extern "C" fn(*mut c_void) -> GBoolean,
    pub(super) dmabuf_memory_get_fd: unsafe extern "C" fn(*mut c_void) -> c_int,

    /// The decoding bin to use: `decodebin3`, or `decodebin` before it.
    pub(super) decodebin: &'static CStr,
    _libraries: [Library; 6],
}

/// The elements that every pipeline needs.
const ELEMENTS: [&CStr; 3] = [c"appsrc", c"appsink", c"videoconvert"];

/// GStreamer, loaded and initialized once for the process, or why it can't
/// be.
pub(super) fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(Api::load).as_ref().map_err(Clone::clone)
}

impl Api {
    fn load() -> Result<Api, String> {
        let open = |name: &str| {
            // SAFETY: these libraries' initializers need nothing of the
            // caller.
            unsafe { Library::new(name) }.map_err(|error| format!("GStreamer 1.x isn't installed: {error}"))
        };
        let glib = open("libglib-2.0.so.0")?;
        let gobject = open("libgobject-2.0.so.0")?;
        let gst = open("libgstreamer-1.0.so.0")?;
        let app = open("libgstapp-1.0.so.0")?;
        let video = open("libgstvideo-1.0.so.0")?;
        let allocators = open("libgstallocators-1.0.so.0")?;
        macro_rules! symbol {
            ($library:ident, $name:literal) => {
                // SAFETY: the field's type is the signature that the
                // function has in its library's headers.
                *unsafe { $library.get(concat!($name, "\0").as_bytes()) }
                    .map_err(|error| format!("GStreamer 1.x isn't installed: {error}"))?
            };
        }
        let api = Api {
            g_free: symbol!(glib, "g_free"),
            g_error_free: symbol!(glib, "g_error_free"),
            g_list_append: symbol!(glib, "g_list_append"),
            g_list_free: symbol!(glib, "g_list_free"),
            g_signal_connect_data: symbol!(gobject, "g_signal_connect_data"),

            init_check: symbol!(gst, "gst_init_check"),
            object_ref: symbol!(gst, "gst_object_ref"),
            object_ref_sink: symbol!(gst, "gst_object_ref_sink"),
            object_unref: symbol!(gst, "gst_object_unref"),
            mini_object_unref: symbol!(gst, "gst_mini_object_unref"),
            element_factory_find: symbol!(gst, "gst_element_factory_find"),
            element_factory_make: symbol!(gst, "gst_element_factory_make"),
            pipeline_new: symbol!(gst, "gst_pipeline_new"),
            bin_add: symbol!(gst, "gst_bin_add"),
            element_link: symbol!(gst, "gst_element_link"),
            element_set_state: symbol!(gst, "gst_element_set_state"),
            element_get_bus: symbol!(gst, "gst_element_get_bus"),
            element_get_static_pad: symbol!(gst, "gst_element_get_static_pad"),
            element_seek_simple: symbol!(gst, "gst_element_seek_simple"),
            element_query_position: symbol!(gst, "gst_element_query_position"),
            element_send_event: symbol!(gst, "gst_element_send_event"),
            util_set_object_arg: symbol!(gst, "gst_util_set_object_arg"),
            bus_timed_pop: symbol!(gst, "gst_bus_timed_pop"),
            bus_post: symbol!(gst, "gst_bus_post"),
            bus_set_sync_handler: symbol!(gst, "gst_bus_set_sync_handler"),
            message_new_application: symbol!(gst, "gst_message_new_application"),
            message_parse_error: symbol!(gst, "gst_message_parse_error"),
            message_parse_error_details: symbol!(gst, "gst_message_parse_error_details"),
            message_get_structure: symbol!(gst, "gst_message_get_structure"),
            message_parse_stream_collection: symbol!(gst, "gst_message_parse_stream_collection"),
            stream_collection_get_size: symbol!(gst, "gst_stream_collection_get_size"),
            stream_collection_get_stream: symbol!(gst, "gst_stream_collection_get_stream"),
            stream_get_stream_type: symbol!(gst, "gst_stream_get_stream_type"),
            stream_get_stream_id: symbol!(gst, "gst_stream_get_stream_id"),
            stream_get_caps: symbol!(gst, "gst_stream_get_caps"),
            event_new_select_streams: symbol!(gst, "gst_event_new_select_streams"),
            pad_get_stream: symbol!(gst, "gst_pad_get_stream"),
            pad_get_current_caps: symbol!(gst, "gst_pad_get_current_caps"),
            pad_is_linked: symbol!(gst, "gst_pad_is_linked"),
            pad_link: symbol!(gst, "gst_pad_link"),
            caps_from_string: symbol!(gst, "gst_caps_from_string"),
            caps_get_structure: symbol!(gst, "gst_caps_get_structure"),
            caps_get_features: symbol!(gst, "gst_caps_get_features"),
            caps_features_contains: symbol!(gst, "gst_caps_features_contains"),
            structure_new_empty: symbol!(gst, "gst_structure_new_empty"),
            structure_get_name: symbol!(gst, "gst_structure_get_name"),
            structure_has_name: symbol!(gst, "gst_structure_has_name"),
            structure_get_string: symbol!(gst, "gst_structure_get_string"),
            structure_get_int: symbol!(gst, "gst_structure_get_int"),
            sample_get_buffer: symbol!(gst, "gst_sample_get_buffer"),
            sample_get_caps: symbol!(gst, "gst_sample_get_caps"),
            sample_get_segment: symbol!(gst, "gst_sample_get_segment"),
            segment_to_stream_time: symbol!(gst, "gst_segment_to_stream_time"),
            buffer_new_allocate: symbol!(gst, "gst_buffer_new_allocate"),
            buffer_set_size: symbol!(gst, "gst_buffer_set_size"),
            buffer_map: symbol!(gst, "gst_buffer_map"),
            buffer_unmap: symbol!(gst, "gst_buffer_unmap"),
            buffer_find_memory: symbol!(gst, "gst_buffer_find_memory"),
            buffer_peek_memory: symbol!(gst, "gst_buffer_peek_memory"),
            memory_get_sizes: symbol!(gst, "gst_memory_get_sizes"),
            query_add_allocation_meta: symbol!(gst, "gst_query_add_allocation_meta"),

            app_src_set_callbacks: symbol!(app, "gst_app_src_set_callbacks"),
            app_src_set_size: symbol!(app, "gst_app_src_set_size"),
            app_src_set_stream_type: symbol!(app, "gst_app_src_set_stream_type"),
            app_src_push_buffer: symbol!(app, "gst_app_src_push_buffer"),
            app_src_end_of_stream: symbol!(app, "gst_app_src_end_of_stream"),
            app_sink_set_callbacks: symbol!(app, "gst_app_sink_set_callbacks"),
            app_sink_set_caps: symbol!(app, "gst_app_sink_set_caps"),
            app_sink_pull_sample: symbol!(app, "gst_app_sink_pull_sample"),
            app_sink_pull_preroll: symbol!(app, "gst_app_sink_pull_preroll"),

            buffer_get_video_meta: symbol!(video, "gst_buffer_get_video_meta"),
            video_meta_api_get_type: symbol!(video, "gst_video_meta_api_get_type"),
            video_colorimetry_from_string: symbol!(video, "gst_video_colorimetry_from_string"),

            is_dmabuf_memory: symbol!(allocators, "gst_is_dmabuf_memory"),
            dmabuf_memory_get_fd: symbol!(allocators, "gst_dmabuf_memory_get_fd"),

            decodebin: c"decodebin3",
            _libraries: [glib, gobject, gst, app, video, allocators],
        };
        let mut error = ptr::null_mut();
        // SAFETY: GStreamer reads no arguments, and it reports a failure in
        // a new error, which is freed here.
        if unsafe { (api.init_check)(ptr::null_mut(), ptr::null_mut(), &raw mut error) } == 0 {
            // SAFETY: a failed init sets the error, or leaves it null.
            let message = unsafe { api.take_error(error) };
            return Err(format!("GStreamer failed to start: {message}"));
        }
        let missing: Vec<&str> = ELEMENTS
            .into_iter()
            .filter(|name| !api.has_element(name))
            .map(|name| name.to_str().unwrap_or_default())
            .collect();
        if !missing.is_empty() {
            return Err(format!("GStreamer's base plugins aren't installed: it lacks {}", missing.join(", ")));
        }
        let decodebin = if api.has_element(c"decodebin3") {
            c"decodebin3"
        } else if api.has_element(c"decodebin") {
            c"decodebin"
        } else {
            return Err("GStreamer's base plugins aren't installed: it lacks decodebin".to_owned());
        };
        Ok(Api { decodebin, ..api })
    }

    fn has_element(&self, name: &CStr) -> bool {
        // SAFETY: a lookup by name, whose factory reference is released.
        unsafe {
            let factory = (self.element_factory_find)(name.as_ptr());
            if factory.is_null() {
                return false;
            }
            (self.object_unref)(factory);
            true
        }
    }

    /// The message of `error`, which this frees.
    ///
    /// # Safety
    ///
    /// `error` is null or a `GError` the caller owns.
    pub(super) unsafe fn take_error(&self, error: *mut GError) -> String {
        if error.is_null() {
            return "an unknown error".to_owned();
        }
        // SAFETY: the caller's error, whose message is a C string or null.
        unsafe {
            let message = string((*error).message).unwrap_or_else(|| "an unknown error".to_owned());
            (self.g_error_free)(error);
            message
        }
    }

    /// Sets `object`'s property `name` from its string form, as
    /// `gst-launch-1.0` would.
    pub(super) fn set(&self, object: &Object, name: &CStr, value: &str) {
        let Ok(value) = CString::new(value) else { return };
        // SAFETY: a live object, and C strings that outlive the call.
        unsafe { (self.util_set_object_arg)(object.as_ptr(), name.as_ptr(), value.as_ptr()) };
    }

    /// A string field of `structure`.
    ///
    /// # Safety
    ///
    /// `structure` is a live `GstStructure`.
    pub(super) unsafe fn structure_string(&self, structure: *const c_void, field: &CStr) -> Option<String> {
        // SAFETY: as the caller says; the string is the structure's.
        unsafe { string((self.structure_get_string)(structure, field.as_ptr())) }
    }

    /// An integer field of `structure`.
    ///
    /// # Safety
    ///
    /// `structure` is a live `GstStructure`.
    pub(super) unsafe fn structure_int(&self, structure: *const c_void, field: &CStr) -> Option<i32> {
        let mut value = 0;
        // SAFETY: as the caller says.
        (unsafe { (self.structure_get_int)(structure, field.as_ptr(), &raw mut value) } != 0).then_some(value)
    }
}

/// A copy of the C string at `chars`, if it isn't null.
///
/// # Safety
///
/// `chars` is null or a C string.
pub(super) unsafe fn string(chars: *const c_char) -> Option<String> {
    // SAFETY: as the caller says.
    (!chars.is_null()).then(|| unsafe { CStr::from_ptr(chars) }.to_string_lossy().into_owned())
}

/// A reference on a `GstObject`, released on drop.
pub(super) struct Object {
    api: &'static Api,
    ptr: NonNull<c_void>,
}

// SAFETY: GStreamer's objects are thread-safe and reference-counted
// atomically.
unsafe impl Send for Object {}
// SAFETY: as for `Send`.
unsafe impl Sync for Object {}

impl Object {
    /// Takes over the full reference at `ptr`; `None` for null.
    ///
    /// # Safety
    ///
    /// `ptr` is null or a `GstObject` with a full reference that the caller
    /// gives up.
    pub(super) unsafe fn take(api: &'static Api, ptr: *mut c_void) -> Option<Object> {
        Some(Object { api, ptr: NonNull::new(ptr)? })
    }

    /// Takes over the floating reference of the new object at `ptr`, such
    /// as an element that a factory made; `None` for null.
    ///
    /// # Safety
    ///
    /// `ptr` is null or a new `GstObject` whose floating reference the
    /// caller gives up.
    pub(super) unsafe fn sink(api: &'static Api, ptr: *mut c_void) -> Option<Object> {
        let ptr = NonNull::new(ptr)?;
        // SAFETY: as the caller says: the floating reference becomes this
        // value's.
        unsafe { (api.object_ref_sink)(ptr.as_ptr()) };
        Some(Object { api, ptr })
    }

    /// A new reference on `ptr`.
    ///
    /// # Safety
    ///
    /// `ptr` is a live `GstObject`.
    pub(super) unsafe fn borrow(api: &'static Api, ptr: NonNull<c_void>) -> Object {
        // SAFETY: as the caller says.
        unsafe { (api.object_ref)(ptr.as_ptr()) };
        Object { api, ptr }
    }

    pub(super) fn as_ptr(&self) -> *mut c_void {
        self.ptr.as_ptr()
    }
}

impl Clone for Object {
    fn clone(&self) -> Object {
        // SAFETY: a live object, which gains a reference.
        unsafe { (self.api.object_ref)(self.ptr.as_ptr()) };
        Object { api: self.api, ptr: self.ptr }
    }
}

impl Drop for Object {
    fn drop(&mut self) {
        // SAFETY: the reference is this value's.
        unsafe { (self.api.object_unref)(self.ptr.as_ptr()) };
    }
}

/// A reference on a `GstMiniObject` (a sample, caps or message), released
/// on drop.
pub(super) struct MiniRef {
    api: &'static Api,
    ptr: NonNull<c_void>,
}

// SAFETY: mini objects are reference-counted atomically, and the player only
// reads the ones it shares.
unsafe impl Send for MiniRef {}
// SAFETY: as for `Send`.
unsafe impl Sync for MiniRef {}

impl MiniRef {
    /// Takes over the reference at `ptr`; `None` for null.
    ///
    /// # Safety
    ///
    /// `ptr` is null or a mini object with a reference the caller gives up.
    pub(super) unsafe fn take(api: &'static Api, ptr: *mut c_void) -> Option<MiniRef> {
        Some(MiniRef { api, ptr: NonNull::new(ptr)? })
    }

    pub(super) fn as_ptr(&self) -> *mut c_void {
        self.ptr.as_ptr()
    }
}

impl Drop for MiniRef {
    fn drop(&mut self) {
        // SAFETY: the reference is this value's.
        unsafe { (self.api.mini_object_unref)(self.ptr.as_ptr()) };
    }
}
