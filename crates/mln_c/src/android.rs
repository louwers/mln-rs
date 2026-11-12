//! `platform/android.h`: the library's setup in an Android app, and the
//! Android windows that window targets present to.

use core::ffi::c_void;

#[cfg(target_os = "android")]
pub(crate) use window::Window;

use crate::error::Error;
use crate::generated::types::{mln_diagnostic, mln_status};

/// `mln_android_init()`: what the library needs of the app is the Java VM
/// that HTTPS verifies certificates in, with Android's own verifier
/// (rustls-platform-verifier, whose Android component the app includes), and
/// the context's assets, which `asset://` URLs load when the runtime has no
/// asset path.
///
/// # Safety
///
/// Each pointer is null or valid for the header's use of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mln_android_init(
    jni_env: *mut c_void,
    context: *mut c_void,
    out_diagnostic: *mut mln_diagnostic,
) -> mln_status {
    export!(out_diagnostic, {
        if jni_env.is_null() || context.is_null() {
            return Err(Error::invalid_argument("jni_env and context must not be null"));
        }
        #[cfg(target_os = "android")]
        {
            // SAFETY: the caller's contract.
            unsafe { mln_storage::android::init(jni_env, context) }.map_err(Error::from)
        }
        #[cfg(not(target_os = "android"))]
        Err(Error::new(mln_util::ErrorKind::Unsupported, "Android initialization is not supported"))
    })
}

#[cfg(all(test, not(target_os = "android")))]
mod init_tests {
    use super::*;
    use crate::diagnostic::tests::{diagnostic, message};
    use crate::generated::types::{MLN_STATUS_INVALID_ARGUMENT, MLN_STATUS_UNSUPPORTED};

    #[test]
    fn android_initialization_needs_its_arguments_and_android() {
        let mut sentinel = 0u8;
        let sentinel: *mut c_void = (&raw mut sentinel).cast();
        for (jni_env, context) in [(core::ptr::null_mut(), sentinel), (sentinel, core::ptr::null_mut())] {
            let mut out = diagnostic();
            // SAFETY: null or local pointers, which the call checks before
            // it would use them, and a local diagnostic.
            assert_eq!(unsafe { mln_android_init(jni_env, context, &raw mut *out) }, MLN_STATUS_INVALID_ARGUMENT);
            assert_eq!(message(&out), "jni_env and context must not be null");
        }
        let mut out = diagnostic();
        // SAFETY: as above; no platform but Android reads them.
        assert_eq!(unsafe { mln_android_init(sentinel, sentinel, &raw mut *out) }, MLN_STATUS_UNSUPPORTED);
        assert_eq!(message(&out), "Android initialization is not supported");
    }
}

#[cfg(target_os = "android")]
mod window {
    use core::ffi::c_void;
    use core::ptr::NonNull;

    use raw_window_handle::{
        AndroidNdkWindowHandle, DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
        WindowHandle,
    };

    #[link(name = "android")]
    unsafe extern "C" {
        fn ANativeWindow_acquire(window: *mut c_void);
        fn ANativeWindow_release(window: *mut c_void);
    }

    /// A reference of the session's own to a host's `ANativeWindow`, which
    /// keeps the window alive until the session lets go of its target.
    pub(crate) struct Window(NonNull<c_void>);

    // SAFETY: an `ANativeWindow`'s references may be taken and released on
    // any thread, and the backends present to it from the session's.
    unsafe impl Send for Window {}
    // SAFETY: as for `Send`; nothing here changes the window.
    unsafe impl Sync for Window {}

    impl Window {
        /// Takes a reference to `window`.
        ///
        /// # Safety
        ///
        /// `window` is a live `ANativeWindow*`.
        pub(crate) unsafe fn acquire(window: NonNull<c_void>) -> Window {
            // SAFETY: the caller's contract.
            unsafe { ANativeWindow_acquire(window.as_ptr()) };
            Window(window)
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            // SAFETY: the reference is this one's.
            unsafe { ANativeWindow_release(self.0.as_ptr()) };
        }
    }

    impl HasWindowHandle for Window {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let handle = RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(self.0));
            // SAFETY: the reference keeps the window alive.
            Ok(unsafe { WindowHandle::borrow_raw(handle) })
        }
    }

    impl HasDisplayHandle for Window {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(DisplayHandle::android())
        }
    }
}

// Windows and hardware buffers made without Java: an `AImageReader`'s
// window, whose images the tests read back, and the hardware buffers of
// exported rings, which they lock.
#[cfg(all(test, target_os = "android"))]
mod tests {
    use core::ffi::{c_int, c_void};
    use std::time::{Duration, Instant};

    use crate::acquired_frame::{
        mln_acquired_frame_get_android_hardware_buffer, mln_acquired_frame_get_dma_buf,
        mln_acquired_frame_get_producer_sync, mln_acquired_frame_release, mln_render_session_acquire_frame,
    };
    use crate::generated::types::{
        MLN_FRAME_DEMAND_FORCE, MLN_GPU_DEVICE_OPTION_BACKEND, MLN_GPU_EXPORT_KIND_ANDROID_HARDWARE_BUFFER,
        MLN_GPU_EXPORT_KIND_FLAG_ANDROID_HARDWARE_BUFFER, MLN_GPU_SYNC_CPU_COMPLETE,
        MLN_GPU_SYNC_KIND_FLAG_CPU_COMPLETE, MLN_GPU_SYNC_KIND_FLAG_SYNC_FD, MLN_GPU_SYNC_SYNC_FD,
        MLN_RENDER_BACKEND_OPEN_GL, MLN_RENDER_BACKEND_VULKAN, MLN_RENDER_RESULT_RENDERED,
        MLN_RENDER_RESULT_TARGET_NOT_READY, MLN_RENDER_SESSION_CAPABILITY_FRAME_ACQUISITION,
        MLN_RENDER_SESSION_CAPABILITY_PRESENTATION, MLN_RENDER_TARGET_KIND_ANDROID_WINDOW,
        MLN_RENDER_TARGET_KIND_EXPORTED_RING, MLN_RENDER_TARGET_KIND_FLAG_ANDROID_WINDOW, MLN_STATUS_OK,
        MLN_STATUS_UNSUPPORTED, mln_android_hardware_buffer_frame, mln_android_window_target, mln_dma_buf_frame,
        mln_exported_ring_target, mln_gpu_device, mln_gpu_device_capabilities, mln_gpu_device_options, mln_gpu_sync,
        mln_logical_size, mln_map, mln_render_session, mln_render_session_capabilities, mln_render_target,
        mln_render_target_data,
    };
    use crate::gpu_device::{mln_gpu_device_create, mln_gpu_device_get_capabilities, mln_gpu_device_release};
    use crate::map::mln_map_set_style_json;
    use crate::map::tests::{create_map, options, release_map, view};
    use crate::render_session::mln_render_session_get_capabilities;
    use crate::render_session::tests::{attach_options, demand, release_session, sized, wait_for_result};
    use crate::render_target::{mln_map_attach, mln_render_session_set_target};
    use crate::runtime::tests::{Waiter, create, release};

    #[link(name = "mediandk")]
    unsafe extern "C" {
        fn AImageReader_newWithUsage(
            width: i32,
            height: i32,
            format: i32,
            usage: u64,
            max_images: i32,
            reader: *mut *mut c_void,
        ) -> c_int;
        fn AImageReader_getWindow(reader: *mut c_void, window: *mut *mut c_void) -> c_int;
        fn AImageReader_acquireLatestImage(reader: *mut c_void, image: *mut *mut c_void) -> c_int;
        fn AImageReader_delete(reader: *mut c_void);
        fn AImage_getWidth(image: *const c_void, width: *mut i32) -> c_int;
        fn AImage_getHeight(image: *const c_void, height: *mut i32) -> c_int;
        fn AImage_getPlaneData(image: *const c_void, plane: i32, data: *mut *mut u8, length: *mut c_int) -> c_int;
        fn AImage_getPlaneRowStride(image: *const c_void, plane: i32, stride: *mut i32) -> c_int;
        fn AImage_delete(image: *mut c_void);
    }

    /// `AHardwareBuffer_Desc`.
    #[repr(C)]
    #[derive(Default)]
    struct Desc {
        width: u32,
        height: u32,
        layers: u32,
        format: u32,
        usage: u64,
        stride: u32,
        rfu0: u32,
        rfu1: u64,
    }

    #[link(name = "nativewindow")]
    unsafe extern "C" {
        fn AHardwareBuffer_describe(buffer: *const c_void, desc: *mut Desc);
        fn AHardwareBuffer_lock(
            buffer: *mut c_void,
            usage: u64,
            fence: c_int,
            rect: *const c_void,
            address: *mut *mut c_void,
        ) -> c_int;
        fn AHardwareBuffer_unlock(buffer: *mut c_void, fence: *mut c_int) -> c_int;
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    /// `AIMAGE_FORMAT_RGBA_8888`.
    const RGBA_8888: i32 = 1;
    /// `AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN`.
    const CPU_READ_OFTEN: u64 = 3;
    const NO_DIAGNOSTIC: *mut crate::generated::types::mln_diagnostic = core::ptr::null_mut();

    /// An image reader, whose window a session presents to, and which reads
    /// back what it presented.
    struct Reader(*mut c_void);

    impl Reader {
        fn new(width: i32, height: i32) -> Reader {
            let mut reader = core::ptr::null_mut();
            // SAFETY: a local output.
            let status =
                unsafe { AImageReader_newWithUsage(width, height, RGBA_8888, CPU_READ_OFTEN, 4, &raw mut reader) };
            assert_eq!(status, 0, "an image reader");
            Reader(reader)
        }

        /// The reader's window, which it keeps.
        fn window(&self) -> *mut c_void {
            let mut window = core::ptr::null_mut();
            // SAFETY: the live reader and a local output.
            assert_eq!(unsafe { AImageReader_getWindow(self.0, &raw mut window) }, 0);
            window
        }

        /// The size and center pixel of the latest image that the window
        /// presented.
        fn center(&self) -> (i32, i32, [u8; 4]) {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut image = core::ptr::null_mut();
            // SAFETY: the live reader and a local output.
            while unsafe { AImageReader_acquireLatestImage(self.0, &raw mut image) } != 0 {
                assert!(Instant::now() < deadline, "the window presented no image");
                std::thread::sleep(Duration::from_millis(10));
            }
            // SAFETY: the reader's image, deleted below, whose one plane has
            // `stride` bytes a row.
            unsafe {
                let (mut width, mut height, mut stride, mut length) = (0, 0, 0, 0);
                let mut data = core::ptr::null_mut();
                AImage_getWidth(image, &raw mut width);
                AImage_getHeight(image, &raw mut height);
                AImage_getPlaneRowStride(image, 0, &raw mut stride);
                assert_eq!(AImage_getPlaneData(image, 0, &raw mut data, &raw mut length), 0);
                let offset = (height / 2 * stride + width / 2 * 4) as usize;
                let mut pixel = [0u8; 4];
                core::ptr::copy_nonoverlapping(data.add(offset), pixel.as_mut_ptr(), 4);
                AImage_delete(image);
                (width, height, pixel)
            }
        }
    }

    impl Drop for Reader {
        fn drop(&mut self) {
            // SAFETY: the reader is this one's.
            unsafe { AImageReader_delete(self.0) };
        }
    }

    fn window_target(window: *mut c_void, width: u32, height: u32) -> mln_render_target {
        let logical_size = mln_logical_size { width, height };
        let data = mln_render_target_data { android_window: mln_android_window_target { logical_size, window } };
        mln_render_target { kind: MLN_RENDER_TARGET_KIND_ANDROID_WINDOW, data, ..sized() }
    }

    fn red_map(runtime: crate::generated::types::mln_runtime) -> mln_map {
        let map = create_map(runtime, &options(1.0));
        let styled = Waiter::new();
        let style = r#"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background","paint":{"background-color":"red"}}]}"#;
        // SAFETY: local arguments.
        unsafe { mln_map_set_style_json(map, view(style), &raw const styled.descriptor, NO_DIAGNOSTIC) };
        assert_eq!(styled.status(), MLN_STATUS_OK);
        map
    }

    fn attach(map: mln_map, target: &mln_render_target, device: mln_gpu_device, producer: u32) -> mln_render_session {
        let options = crate::generated::types::mln_render_session_attach_options {
            producer_sync_kinds: producer,
            ..attach_options()
        };
        let (mut session, attached) = (0, Waiter::new());
        // SAFETY: local arguments.
        let status = unsafe {
            mln_map_attach(
                map,
                target,
                device,
                &raw const options,
                &raw mut session,
                &raw const attached.descriptor,
                NO_DIAGNOSTIC,
            )
        };
        assert_eq!((status, attached.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
        session
    }

    fn capabilities(session: mln_render_session) -> mln_render_session_capabilities {
        let mut capabilities = sized::<mln_render_session_capabilities>();
        // SAFETY: local capabilities.
        let status = unsafe { mln_render_session_get_capabilities(session, &raw mut capabilities, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_OK);
        capabilities
    }

    /// A library device of `backend` that presents to Android windows.
    fn device(backend: u32) -> mln_gpu_device {
        let options = mln_gpu_device_options { fields: MLN_GPU_DEVICE_OPTION_BACKEND, backend, ..sized() };
        let mut device = 0;
        // SAFETY: local arguments.
        assert_eq!(unsafe { mln_gpu_device_create(&raw const options, &raw mut device, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
        let mut capabilities = sized::<mln_gpu_device_capabilities>();
        // SAFETY: local arguments.
        unsafe { mln_gpu_device_get_capabilities(device, &raw mut capabilities, NO_DIAGNOSTIC) };
        assert_ne!(capabilities.target_kinds & MLN_RENDER_TARGET_KIND_FLAG_ANDROID_WINDOW, 0, "backend {backend}");
        device
    }

    fn rendered(session: mln_render_session, token: u64) {
        assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, token), MLN_STATUS_OK);
        assert_eq!(wait_for_result(session).disposition, MLN_RENDER_RESULT_RENDERED);
    }

    // A window target presents to an `ANativeWindow`, moves to another, and
    // outlives the windows that the platform destroys, on a Vulkan and an
    // OpenGL device and on the device that the session makes.
    #[test]
    fn an_android_window_target_presents_moves_and_outlives_its_window() {
        let runtime = create();
        let map = red_map(runtime);
        let devices = [device(MLN_RENDER_BACKEND_VULKAN), device(MLN_RENDER_BACKEND_OPEN_GL), 0];
        for device in devices {
            let first = Reader::new(64, 48);
            let session = attach(map, &window_target(first.window(), 64, 48), device, 0);
            assert_eq!(capabilities(session).flags, MLN_RENDER_SESSION_CAPABILITY_PRESENTATION);
            rendered(session, 1);
            assert_eq!(first.center(), (64, 48, RED), "device {device}");

            // The session holds its own reference, so the window that it
            // moves to may go away once the call returns.
            let second = Reader::new(80, 40);
            let moved = Waiter::new();
            // SAFETY: local arguments.
            let status = unsafe {
                mln_render_session_set_target(
                    session,
                    &window_target(second.window(), 80, 40),
                    &raw const moved.descriptor,
                    NO_DIAGNOSTIC,
                )
            };
            assert_eq!((status, moved.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
            rendered(session, 2);
            assert_eq!(second.center(), (80, 40, RED), "device {device}");

            // The platform destroys the window: its frames go nowhere, and
            // the session lets go of it.
            drop(second);
            assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, 3), MLN_STATUS_OK);
            let disposition = wait_for_result(session).disposition;
            assert!(
                matches!(disposition, MLN_RENDER_RESULT_RENDERED | MLN_RENDER_RESULT_TARGET_NOT_READY),
                "device {device}: {disposition}"
            );
            let released = Waiter::new();
            // SAFETY: a local completion.
            let status = unsafe {
                crate::render_session::mln_render_session_release_target(
                    session,
                    &raw const released.descriptor,
                    NO_DIAGNOSTIC,
                )
            };
            assert_eq!((status, released.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
            release_session(session);
        }
        for device in devices {
            mln_gpu_device_release(device);
        }
        release_map(map);
        release(runtime);
    }

    /// Waits for a frame's producer sync file, if it has one.
    fn wait_for_producer(sync: &mln_gpu_sync) {
        if sync.kind != MLN_GPU_SYNC_SYNC_FD || (sync.handle as i64) < 0 {
            return;
        }
        let mut waiting = libc::pollfd { fd: sync.handle as i32, events: libc::POLLIN, revents: 0 };
        // SAFETY: a poll of the frame's live sync file.
        assert_eq!(unsafe { libc::poll(&raw mut waiting, 1, 10_000) }, 1, "the frame's sync file signals");
    }

    /// Acquires the session's frame, checks that its hardware buffer is red
    /// and of the host's usage, and returns it and its slot.
    fn read_frame(
        session: mln_render_session,
        capabilities: &mln_render_session_capabilities,
    ) -> (crate::generated::types::mln_acquired_frame, u32) {
        let mut frame = 0;
        // SAFETY: a local output.
        assert_eq!(unsafe { mln_render_session_acquire_frame(session, &raw mut frame, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
        let mut buffer = sized::<mln_android_hardware_buffer_frame>();
        // SAFETY: a local output.
        let status = unsafe { mln_acquired_frame_get_android_hardware_buffer(frame, &raw mut buffer, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_OK);
        assert_eq!((buffer.width, buffer.height, buffer.scale_factor), (64, 48, 1.0));
        let mut dma_buf = sized::<mln_dma_buf_frame>();
        // SAFETY: a local output.
        let status = unsafe { mln_acquired_frame_get_dma_buf(frame, &raw mut dma_buf, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_UNSUPPORTED);
        let mut sync = sized::<mln_gpu_sync>();
        // SAFETY: a local output.
        assert_eq!(unsafe { mln_acquired_frame_get_producer_sync(frame, &raw mut sync, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
        assert_eq!(sync.kind, capabilities.producer_sync_kind);
        wait_for_producer(&sync);
        let mut desc = Desc::default();
        // SAFETY: the frame's live buffer, locked around the read.
        let center = unsafe {
            AHardwareBuffer_describe(buffer.buffer, &raw mut desc);
            assert_eq!((desc.width, desc.height, desc.layers, desc.format), (64, 48, 1, 1));
            assert_eq!(desc.usage & CPU_READ_OFTEN, CPU_READ_OFTEN);
            let mut data = core::ptr::null_mut();
            assert_eq!(AHardwareBuffer_lock(buffer.buffer, CPU_READ_OFTEN, -1, core::ptr::null(), &raw mut data), 0);
            let offset = (24 * desc.stride as usize + 32) * 4;
            let mut pixel = [0u8; 4];
            core::ptr::copy_nonoverlapping(data.cast::<u8>().add(offset), pixel.as_mut_ptr(), 4);
            assert_eq!(AHardwareBuffer_unlock(buffer.buffer, core::ptr::null_mut()), 0);
            pixel
        };
        assert_eq!(center, RED, "slot {}", buffer.slot);
        (frame, buffer.slot)
    }

    // An exported ring of a session that makes its own device hands out
    // hardware buffers with the host's usage bits, which the CPU reads.
    #[test]
    fn an_exported_ring_hands_out_hardware_buffers() {
        let runtime = create();
        let map = red_map(runtime);
        let ring = mln_exported_ring_target {
            logical_size: mln_logical_size { width: 64, height: 48 },
            texture_count: 3,
            export_kinds: MLN_GPU_EXPORT_KIND_FLAG_ANDROID_HARDWARE_BUFFER,
            dma_buf_formats: core::ptr::null(),
            dma_buf_format_count: 0,
            android_hardware_buffer_usage: CPU_READ_OFTEN,
        };
        let target = mln_render_target {
            kind: MLN_RENDER_TARGET_KIND_EXPORTED_RING,
            data: mln_render_target_data { exported_ring: ring },
            ..sized()
        };
        let producer = MLN_GPU_SYNC_KIND_FLAG_SYNC_FD | MLN_GPU_SYNC_KIND_FLAG_CPU_COMPLETE;
        let session = attach(map, &target, 0, producer);
        let capabilities = capabilities(session);
        assert_eq!(capabilities.export_kind, MLN_GPU_EXPORT_KIND_ANDROID_HARDWARE_BUFFER);
        assert_eq!(capabilities.flags, MLN_RENDER_SESSION_CAPABILITY_FRAME_ACQUISITION);
        // A sync file where the device has them.
        assert!(matches!(capabilities.producer_sync_kind, MLN_GPU_SYNC_SYNC_FD | MLN_GPU_SYNC_CPU_COMPLETE));
        // Two rounds of a frame in each buffer, which the host holds until
        // the round ends.
        for round in 0..2 {
            let mut frames = Vec::new();
            for token in 1..=3 {
                rendered(session, 10 * round + token);
                frames.push(read_frame(session, &capabilities));
            }
            let mut slots: Vec<u32> = frames.iter().map(|(_, slot)| *slot).collect();
            slots.sort_unstable();
            assert_eq!(slots, [0, 1, 2], "round {round}");
            for (frame, _) in frames {
                let done = mln_gpu_sync { kind: MLN_GPU_SYNC_CPU_COMPLETE, ..sized() };
                // SAFETY: a local sync.
                assert_eq!(unsafe { mln_acquired_frame_release(frame, &raw const done, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
            }
        }
        release_session(session);
        release_map(map);
        release(runtime);
    }
}
