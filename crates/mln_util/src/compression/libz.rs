//! `compress` and `decompress` on the system libz, through its C API, as
//! platform/default/src/mln/util/compression.cpp calls it. Error messages are
//! libz's.

#![allow(unsafe_code)]

use std::ffi::{CStr, c_char, c_int, c_uint, c_ulong, c_void};

/// zlib.h's `z_stream`.
#[repr(C)]
struct ZStream {
    next_in: *const u8,
    avail_in: c_uint,
    total_in: c_ulong,
    next_out: *mut u8,
    avail_out: c_uint,
    total_out: c_ulong,
    msg: *const c_char,
    state: *mut c_void,
    /// `alloc_func`; null for libz's own.
    zalloc: *const c_void,
    /// `free_func`; null for libz's own.
    zfree: *const c_void,
    opaque: *mut c_void,
    data_type: c_int,
    adler: c_ulong,
    reserved: c_ulong,
}

impl ZStream {
    fn new() -> Self {
        ZStream {
            next_in: std::ptr::null(),
            avail_in: 0,
            total_in: 0,
            next_out: std::ptr::null_mut(),
            avail_out: 0,
            total_out: 0,
            msg: std::ptr::null(),
            state: std::ptr::null_mut(),
            zalloc: std::ptr::null(),
            zfree: std::ptr::null(),
            opaque: std::ptr::null_mut(),
            data_type: 0,
            adler: 0,
            reserved: 0,
        }
    }

    /// The stream's error message, or `fallback` when libz gave none.
    fn message(&self, fallback: &str) -> String {
        if self.msg.is_null() {
            return fallback.to_string();
        }
        // SAFETY: libz sets `msg` to null or to a NUL-terminated static string.
        unsafe { CStr::from_ptr(self.msg) }.to_string_lossy().into_owned()
    }
}

const Z_OK: c_int = 0;
const Z_STREAM_END: c_int = 1;
const Z_NO_FLUSH: c_int = 0;
const Z_FINISH: c_int = 4;
const Z_DEFLATED: c_int = 8;
const Z_DEFAULT_COMPRESSION: c_int = -1;
const Z_DEFAULT_STRATEGY: c_int = 0;
const MAX_WBITS: c_int = 15;
/// `inflateInit2`'s window bits for zlib or gzip, detected from the header.
const DETECT_WBITS: c_int = MAX_WBITS + 32;

#[link(name = "z")]
unsafe extern "C" {
    fn zlibVersion() -> *const c_char;
    fn deflateInit2_(
        strm: *mut ZStream,
        level: c_int,
        method: c_int,
        window_bits: c_int,
        mem_level: c_int,
        strategy: c_int,
        version: *const c_char,
        stream_size: c_int,
    ) -> c_int;
    fn deflate(strm: *mut ZStream, flush: c_int) -> c_int;
    fn deflateEnd(strm: *mut ZStream) -> c_int;
    fn inflateInit2_(strm: *mut ZStream, window_bits: c_int, version: *const c_char, stream_size: c_int) -> c_int;
    fn inflate(strm: *mut ZStream, flush: c_int) -> c_int;
    fn inflateEnd(strm: *mut ZStream) -> c_int;
}

const STREAM_SIZE: c_int = std::mem::size_of::<ZStream>() as c_int;

/// Runs `step` over `raw` until it returns anything but `Z_OK`, collecting
/// the output. `step` gets whether all of `raw` has been handed to libz,
/// since `avail_in` holds at most `c_uint::MAX` bytes at a time.
fn run(strm: &mut ZStream, raw: &[u8], mut step: impl FnMut(&mut ZStream, bool) -> c_int) -> (c_int, Vec<u8>) {
    let mut result = Vec::new();
    let mut out = [0u8; 16384];
    let mut rest = raw;
    loop {
        if strm.avail_in == 0 {
            let n = rest.len().min(c_uint::MAX as usize);
            strm.next_in = rest.as_ptr();
            strm.avail_in = n as c_uint;
            rest = &rest[n..];
        }
        strm.next_out = out.as_mut_ptr();
        strm.avail_out = out.len() as c_uint;
        let code = step(strm, rest.is_empty());
        let produced = out.len() - strm.avail_out as usize;
        result.extend_from_slice(&out[..produced]);
        if code != Z_OK {
            return (code, result);
        }
    }
}

pub(super) fn compress(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut strm = ZStream::new();
    // SAFETY: `strm` is a zeroed z_stream that stays in place until
    // `deflateEnd`; `zlibVersion` is the linked libz's version string.
    let init = unsafe {
        deflateInit2_(
            &mut strm,
            Z_DEFAULT_COMPRESSION,
            Z_DEFLATED,
            MAX_WBITS,
            8,
            Z_DEFAULT_STRATEGY,
            zlibVersion(),
            STREAM_SIZE,
        )
    };
    if init != Z_OK {
        return Err("failed to initialize deflate".to_string());
    }
    // SAFETY: `run` points `next_in` and `next_out` at live buffers of
    // `avail_in` and `avail_out` bytes before each call.
    let (code, result) =
        run(&mut strm, raw, |strm, last| unsafe { deflate(strm, if last { Z_FINISH } else { Z_NO_FLUSH }) });
    let error = (code != Z_STREAM_END).then(|| strm.message("failed to deflate"));
    // SAFETY: `strm` was initialized by `deflateInit2_`.
    unsafe { deflateEnd(&mut strm) };
    match error {
        Some(error) => Err(error),
        None => Ok(result),
    }
}

pub(super) fn decompress(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut strm = ZStream::new();
    // SAFETY: as in `compress`.
    let init = unsafe { inflateInit2_(&mut strm, DETECT_WBITS, zlibVersion(), STREAM_SIZE) };
    if init != Z_OK {
        return Err("failed to initialize inflate".to_string());
    }
    // SAFETY: as in `compress`. inflate stops at the end of the stream, so
    // bytes after it are left unread; input that runs out first makes it
    // return Z_BUF_ERROR.
    let (code, result) = run(&mut strm, raw, |strm, _| unsafe { inflate(strm, Z_NO_FLUSH) });
    let error = (code != Z_STREAM_END).then(|| strm.message("decompression error"));
    // SAFETY: `strm` was initialized by `inflateInit2_`.
    unsafe { inflateEnd(&mut strm) };
    match error {
        Some(error) => Err(error),
        None => Ok(result),
    }
}
