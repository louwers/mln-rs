//! `platform/android.h`: the library's setup in an Android app, and the
//! Android windows that window targets present to.

#[cfg(target_os = "android")]
pub(crate) use window::Window;

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

// Windows made without Java: an `AImageReader`'s window, whose images the
// tests read back.
#[cfg(all(test, target_os = "android"))]
mod tests {
    use core::ffi::{c_int, c_void};
    use std::time::{Duration, Instant};

    use crate::generated::types::{
        MLN_FRAME_DEMAND_FORCE, MLN_GPU_DEVICE_OPTION_BACKEND, MLN_RENDER_BACKEND_VULKAN, MLN_RENDER_RESULT_RENDERED,
        MLN_RENDER_RESULT_TARGET_NOT_READY, MLN_RENDER_SESSION_CAPABILITY_PRESENTATION,
        MLN_RENDER_TARGET_KIND_ANDROID_WINDOW, MLN_RENDER_TARGET_KIND_FLAG_ANDROID_WINDOW, MLN_STATUS_OK,
        mln_android_window_target, mln_gpu_device, mln_gpu_device_capabilities, mln_gpu_device_options,
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
            let status = unsafe { AImageReader_newWithUsage(width, height, RGBA_8888, CPU_READ_OFTEN, 4, &mut reader) };
            assert_eq!(status, 0, "an image reader");
            Reader(reader)
        }

        /// The reader's window, which it keeps.
        fn window(&self) -> *mut c_void {
            let mut window = core::ptr::null_mut();
            // SAFETY: the live reader and a local output.
            assert_eq!(unsafe { AImageReader_getWindow(self.0, &mut window) }, 0);
            window
        }

        /// The size and center pixel of the latest image that the window
        /// presented.
        fn center(&self) -> (i32, i32, [u8; 4]) {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut image = core::ptr::null_mut();
            // SAFETY: the live reader and a local output.
            while unsafe { AImageReader_acquireLatestImage(self.0, &mut image) } != 0 {
                assert!(Instant::now() < deadline, "the window presented no image");
                std::thread::sleep(Duration::from_millis(10));
            }
            // SAFETY: the reader's image, deleted below, whose one plane has
            // `stride` bytes a row.
            unsafe {
                let (mut width, mut height, mut stride, mut length) = (0, 0, 0, 0);
                let mut data = core::ptr::null_mut();
                AImage_getWidth(image, &mut width);
                AImage_getHeight(image, &mut height);
                AImage_getPlaneRowStride(image, 0, &mut stride);
                assert_eq!(AImage_getPlaneData(image, 0, &mut data, &mut length), 0);
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
        unsafe { mln_map_set_style_json(map, view(style), &styled.descriptor, NO_DIAGNOSTIC) };
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
        let status =
            unsafe { mln_map_attach(map, target, device, &options, &mut session, &attached.descriptor, NO_DIAGNOSTIC) };
        assert_eq!((status, attached.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
        session
    }

    fn capabilities(session: mln_render_session) -> mln_render_session_capabilities {
        let mut capabilities = sized::<mln_render_session_capabilities>();
        // SAFETY: local capabilities.
        let status = unsafe { mln_render_session_get_capabilities(session, &mut capabilities, NO_DIAGNOSTIC) };
        assert_eq!(status, MLN_STATUS_OK);
        capabilities
    }

    fn vulkan_device() -> mln_gpu_device {
        let options = mln_gpu_device_options {
            fields: MLN_GPU_DEVICE_OPTION_BACKEND,
            backend: MLN_RENDER_BACKEND_VULKAN,
            ..sized()
        };
        let mut device = 0;
        // SAFETY: local arguments.
        assert_eq!(unsafe { mln_gpu_device_create(&options, &mut device, NO_DIAGNOSTIC) }, MLN_STATUS_OK);
        device
    }

    fn rendered(session: mln_render_session, token: u64) {
        assert_eq!(demand(session, MLN_FRAME_DEMAND_FORCE, token), MLN_STATUS_OK);
        assert_eq!(wait_for_result(session).disposition, MLN_RENDER_RESULT_RENDERED);
    }

    // A window target presents to an `ANativeWindow`, moves to another, and
    // outlives the windows that the platform destroys, on the Vulkan device
    // and on the device that the session makes.
    #[test]
    fn an_android_window_target_presents_moves_and_outlives_its_window() {
        let runtime = create();
        let map = red_map(runtime);
        let vulkan = vulkan_device();
        let mut device_capabilities = sized::<mln_gpu_device_capabilities>();
        // SAFETY: local arguments.
        unsafe { mln_gpu_device_get_capabilities(vulkan, &mut device_capabilities, NO_DIAGNOSTIC) };
        assert_ne!(device_capabilities.target_kinds & MLN_RENDER_TARGET_KIND_FLAG_ANDROID_WINDOW, 0);
        for device in [vulkan, 0] {
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
                    &moved.descriptor,
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
                crate::render_session::mln_render_session_release_target(session, &released.descriptor, NO_DIAGNOSTIC)
            };
            assert_eq!((status, released.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
            release_session(session);
        }
        mln_gpu_device_release(vulkan);
        release_map(map);
        release(runtime);
    }
}
