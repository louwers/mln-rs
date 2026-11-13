//! The pacer's source on `OpenHarmony`: an `OH_NativeVSync` of its own.

/// The `OH_NativeVSync` calls back on a thread that it creates, so a request
/// can come from any thread and needs no event loop of the host's. A request
/// can't be cancelled, and destroying the `OH_NativeVSync` while its callback
/// runs would free what the callback uses, so each request holds the
/// [`VsyncSide`] until its callback has run. When that callback lets go of it
/// last, the `OH_NativeVSync` is destroyed on its own thread, which its event
/// runner allows.
#[cfg(target_env = "ohos")]
pub(super) mod native_vsync {
    use core::ffi::{c_char, c_int, c_longlong, c_uint, c_void};
    use std::sync::Arc;
    use std::time::Duration;

    use parking_lot::Mutex;

    use crate::error::Error;
    use crate::frame_pacer::{Limit, VsyncSource, Vsyncs};

    #[repr(C)]
    struct OH_NativeVSync {
        _opaque: [u8; 0],
    }

    type FrameCallback = unsafe extern "C" fn(timestamp: c_longlong, data: *mut c_void);

    #[link(name = "native_vsync")]
    unsafe extern "C" {
        fn OH_NativeVSync_Create(name: *const c_char, length: c_uint) -> *mut OH_NativeVSync;
        fn OH_NativeVSync_Destroy(vsync: *mut OH_NativeVSync);
        fn OH_NativeVSync_RequestFrame(vsync: *mut OH_NativeVSync, callback: FrameCallback, data: *mut c_void)
        -> c_int;
    }

    /// What the `OH_NativeVSync`'s callback shares with the pacer. A waiting
    /// request holds it until its callback runs, so it can outlive the pacer.
    struct VsyncSide {
        vsync: *mut OH_NativeVSync,
        vsyncs: Vsyncs,
        /// Set while a request waits for its callback.
        request_hold: Mutex<Option<Arc<VsyncSide>>>,
    }

    // SAFETY: `OH_NativeVSync_RequestFrame()` may be called on any thread,
    // and the side destroys the native VSync once, when nothing else uses it.
    unsafe impl Send for VsyncSide {}
    // SAFETY: as for `Send`.
    unsafe impl Sync for VsyncSide {}

    impl Drop for VsyncSide {
        fn drop(&mut self) {
            // SAFETY: the side's own native VSync, which no request uses now.
            unsafe { OH_NativeVSync_Destroy(self.vsync) };
        }
    }

    /// The `OH_NativeVSync`'s callback. The timestamp is in nanoseconds on
    /// `CLOCK_MONOTONIC`.
    unsafe extern "C" fn on_frame(timestamp: c_longlong, data: *mut c_void) {
        // SAFETY: the request hold keeps the side until this takes it.
        let side = unsafe { &*data.cast::<VsyncSide>() };
        // Declared first, so that it lets go of the side last.
        let _hold = side.request_hold.lock().take();
        side.vsyncs.vsync(Duration::from_nanos(timestamp.max(0) as u64));
    }

    /// A pacer's `OH_NativeVSync`.
    pub(crate) struct NativeVsyncSource {
        side: Arc<VsyncSide>,
    }

    impl NativeVsyncSource {
        pub(crate) fn new(vsyncs: Vsyncs, _limit: Option<Limit>) -> Result<Self, Error> {
            const NAME: &str = "mln-rs";
            // SAFETY: a name of `NAME.len()` bytes.
            let vsync = unsafe { OH_NativeVSync_Create(NAME.as_ptr().cast(), NAME.len() as c_uint) };
            if vsync.is_null() {
                return Err(Error::native("the frame pacer could not create an OH_NativeVSync"));
            }
            Ok(Self { side: Arc::new(VsyncSide { vsync, vsyncs, request_hold: Mutex::new(None) }) })
        }
    }

    impl VsyncSource for NativeVsyncSource {
        fn schedule(&self) {
            *self.side.request_hold.lock() = Some(Arc::clone(&self.side));
            let data = Arc::as_ptr(&self.side).cast_mut().cast();
            // The callback can run before this returns. A request fails only
            // when the process loses its connection to the render service,
            // which leaves no vsyncs to pace with.
            // SAFETY: the side's native VSync, and a callback whose data the
            // request hold keeps.
            if unsafe { OH_NativeVSync_RequestFrame(self.side.vsync, on_frame, data) } != 0 {
                self.side.request_hold.lock().take();
            }
        }
    }
}

#[cfg(all(test, target_env = "ohos"))]
mod tests {
    use crate::frame_pacer::mln_frame_pacer_create;
    use crate::frame_pacer::tests::expect_paced_frames;
    use crate::generated::types::{MLN_STATUS_OK, mln_frame_pacer};

    fn create() -> mln_frame_pacer {
        let mut pacer = crate::handles::NULL;
        // SAFETY: null options, a local out-handle and a null diagnostic.
        let status = unsafe { mln_frame_pacer_create(core::ptr::null(), &raw mut pacer, core::ptr::null_mut()) };
        assert_eq!(status, MLN_STATUS_OK);
        pacer
    }

    #[test]
    fn a_native_vsync_paces_a_session() {
        expect_paced_frames(create(), false);
    }

    #[test]
    fn a_released_native_vsync_pacer_keeps_pacing_its_sessions() {
        expect_paced_frames(create(), true);
    }
}
