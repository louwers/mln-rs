//! The pacer's source on macOS, iOS and tvOS: a `CADisplayLink`.

/// Every pacer's display link runs on one thread that the library starts for
/// them, on a run loop that runs for the rest of the process, so a pacer
/// needs no run loop of the host's. A display link and its run loop aren't
/// thread-safe, so the pacer touches its link only on that thread: a call
/// from another thread runs a task there. A display link retains its target
/// until it is invalidated, so the target reaches the pacer through
/// [`Vsyncs`], which holds it weakly.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
pub(super) mod display_link {
    use core::ffi::{c_int, c_void};
    use std::collections::VecDeque;
    use std::sync::{Mutex, mpsc};
    use std::thread::{self, ThreadId};
    use std::time::Duration;

    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::NSObject;
    use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, sel};
    use objc2_core_foundation::{
        CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, kCFRunLoopDefaultMode,
    };
    use objc2_foundation::{NSDefaultRunLoopMode, NSRunLoop};
    use objc2_quartz_core::{CADisplayLink, CAFrameRateRange, CAFrameRateRangeDefault};

    use crate::error::Error;
    use crate::frame_pacer::{Limit, VsyncSource, Vsyncs};
    use crate::lock;

    // A vsync that waits behind other work delays the frames it demands.
    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;

    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: c_int) -> c_int;
    }

    // NSScreen, whose display links follow a screen, is AppKit's.
    #[cfg(target_os = "macos")]
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}

    type Task = Box<dyn FnOnce() + Send>;

    /// The tasks that wait for the link thread.
    static TASKS: Mutex<VecDeque<Task>> = Mutex::new(VecDeque::new());

    /// The link thread, once it started.
    static THREAD: Mutex<Option<&'static LinkThread>> = Mutex::new(None);

    /// A Core Foundation object that the link thread and its callers share.
    struct Shared<T: ?Sized>(CFRetained<T>);

    // SAFETY: the run loop and its source are used from other threads only
    // through CFRunLoopSourceSignal() and CFRunLoopWakeUp(), which are
    // thread-safe.
    unsafe impl<T: ?Sized> Send for Shared<T> {}
    // SAFETY: as for `Send`.
    unsafe impl<T: ?Sized> Sync for Shared<T> {}

    struct LinkThread {
        id: ThreadId,
        run_loop: Shared<CFRunLoop>,
        /// Signalled to run the tasks.
        source: Shared<CFRunLoopSource>,
    }

    impl LinkThread {
        /// The thread, which starts on first use.
        fn get() -> Result<&'static Self, Error> {
            let mut started = lock(&THREAD);
            if let Some(thread) = *started {
                return Ok(thread);
            }
            let (ready, readied) = mpsc::channel();
            thread::Builder::new()
                .name("MLN frame pacer".into())
                .spawn(move || run(&ready))
                .map_err(|error| Error::native(format!("the frame pacer could not start its thread: {error}")))?;
            let thread =
                readied.recv().map_err(|_| Error::native("the frame pacer's thread could not start its run loop"))?;
            let thread: &'static Self = Box::leak(Box::new(thread));
            *started = Some(thread);
            Ok(thread)
        }

        fn is_current(&self) -> bool {
            thread::current().id() == self.id
        }

        /// Runs `task` on the thread, after the tasks sent before it.
        fn send(&self, task: impl FnOnce() + Send + 'static) {
            lock(&TASKS).push_back(Box::new(task));
            self.source.0.signal();
            self.run_loop.0.wake_up();
        }

        /// Runs `task` on the thread and waits for its result, which is
        /// `None` when the task panicked.
        fn call<R: Send + 'static>(&self, task: impl FnOnce() -> R + Send + 'static) -> Option<R> {
            if self.is_current() {
                return Some(task());
            }
            let (done, result) = mpsc::sync_channel(1);
            self.send(move || {
                let _ = done.send(task());
            });
            result.recv().ok()
        }
    }

    /// The link thread's body.
    fn run(ready: &mpsc::Sender<LinkThread>) {
        // SAFETY: sets the calling thread's own class.
        let _ = unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
        let Some(run_loop) = CFRunLoop::current() else {
            return;
        };
        let mut context = CFRunLoopSourceContext {
            version: 0,
            info: core::ptr::null_mut(),
            retain: None,
            release: None,
            copyDescription: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform: Some(perform),
        };
        // SAFETY: a version 0 context whose callbacks take no `info`.
        let Some(source) = (unsafe { CFRunLoopSource::new(None, 0, &mut context) }) else {
            return;
        };
        // SAFETY: a constant of Core Foundation's.
        let mode = unsafe { kCFRunLoopDefaultMode };
        // The source also keeps the run loop running: a run loop returns at
        // once while its mode has no source, as it has while no pacer has a
        // display link.
        run_loop.add_source(Some(&source), mode);
        let thread = LinkThread { id: thread::current().id(), run_loop: Shared(run_loop), source: Shared(source) };
        if ready.send(thread).is_err() {
            return;
        }
        loop {
            CFRunLoop::run();
        }
    }

    /// The source's callback, on the link thread: runs the tasks that wait.
    unsafe extern "C-unwind" fn perform(_info: *mut c_void) {
        loop {
            let Some(task) = lock(&TASKS).pop_front() else {
                return;
            };
            // A panic stays on this side of the run loop.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| autoreleasepool(|_| task())));
        }
    }

    /// A display link, which only the link thread uses.
    #[derive(Clone)]
    struct Link(Retained<CADisplayLink>);

    // SAFETY: the link's methods run only on the link thread; other threads
    // only retain and release it, which is thread-safe.
    unsafe impl Send for Link {}
    // SAFETY: as for `Send`.
    unsafe impl Sync for Link {}

    impl Link {
        /// Resumes the link, which calls back at the next refresh. Runs on
        /// the link thread.
        fn resume(&self) {
            self.0.setPaused(false);
        }

        /// Lets a display with a variable refresh rate slow down to `limit`,
        /// or, without one, run at its own rate again. Runs on the link
        /// thread.
        fn set_limit(&self, limit: Option<Limit>) {
            let range = match limit {
                Some(limit) => {
                    let rate = limit.frames_per_second() as f32;
                    CAFrameRateRange { minimum: rate, maximum: rate, preferred: rate }
                }
                // SAFETY: a constant of Core Animation's.
                None => unsafe { CAFrameRateRangeDefault },
            };
            self.0.setPreferredFrameRateRange(range);
        }

        /// Runs on the link thread.
        fn invalidate(&self) {
            self.0.invalidate();
        }
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and the class
        // doesn't implement `Drop`.
        #[unsafe(super(NSObject))]
        #[name = "MLNRustFramePacerTarget"]
        #[ivars = Vsyncs]
        struct Target;

        impl Target {
            /// The display link's callback, on the link thread.
            #[unsafe(method(vsync:))]
            fn vsync(&self, link: &CADisplayLink) {
                // A paused link calls back no more until the pacer schedules
                // another refresh.
                link.setPaused(true);
                // The timestamp is in seconds on the host's monotonic clock.
                let time = Duration::try_from_secs_f64(link.timestamp()).unwrap_or_default();
                self.ivars().vsync(time);
            }
        }
    );

    impl Target {
        fn new(vsyncs: Vsyncs) -> Retained<Self> {
            let this = Self::alloc().set_ivars(vsyncs);
            // SAFETY: NSObject's initializer.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// A display link that calls `target`, on the link thread.
    #[cfg(target_os = "macos")]
    fn display_link(target: &Target) -> Option<Retained<CADisplayLink>> {
        use objc2::runtime::{AnyClass, AnyObject};

        let screens = AnyClass::get(c"NSScreen")?;
        // SAFETY: `+[NSScreen screens]`, an array of screens, or nil without
        // a window server.
        let screens: Option<Retained<AnyObject>> = unsafe { msg_send![screens, screens] };
        // SAFETY: `-[NSArray firstObject]`, the main display, the one with
        // the menu bar, or nil.
        let screen: Option<Retained<AnyObject>> = unsafe { msg_send![&*screens?, firstObject] };
        let screen = screen?;
        let selector = sel!(displayLinkWithTarget:selector:);
        // SAFETY: `-[NSObject respondsToSelector:]`. macOS 14 added the
        // method.
        let available: bool = unsafe { msg_send![&*screen, respondsToSelector: selector] };
        if !available {
            return None;
        }
        // SAFETY: `-[NSScreen displayLinkWithTarget:selector:]`, with a
        // target that implements the selector.
        unsafe { msg_send![&*screen, displayLinkWithTarget: target, selector: sel!(vsync:)] }
    }

    /// A display link that calls `target`, on the link thread.
    #[cfg(not(target_os = "macos"))]
    fn display_link(target: &Target) -> Option<Retained<CADisplayLink>> {
        // SAFETY: a target that implements the selector.
        Some(unsafe { CADisplayLink::displayLinkWithTarget_selector(target, sel!(vsync:)) })
    }

    /// A pacer's display link.
    pub(crate) struct DisplayLinkSource {
        thread: &'static LinkThread,
        link: Link,
    }

    impl DisplayLinkSource {
        pub(crate) fn new(vsyncs: Vsyncs, limit: Option<Limit>) -> Result<Self, Error> {
            let thread = LinkThread::get()?;
            let link = thread.call(move || {
                let target = Target::new(vsyncs);
                let link = Link(display_link(&target)?);
                link.0.setPaused(true);
                if limit.is_some() {
                    link.set_limit(limit);
                }
                // SAFETY: the link thread's own run loop.
                unsafe { link.0.addToRunLoop_forMode(&NSRunLoop::currentRunLoop(), NSDefaultRunLoopMode) };
                Some(link)
            });
            let link =
                link.flatten().ok_or_else(|| Error::native("the system has no display for the pacer to follow"))?;
            Ok(Self { thread, link })
        }
    }

    impl VsyncSource for DisplayLinkSource {
        fn schedule(&self) {
            if self.thread.is_current() {
                return self.link.resume();
            }
            let link = self.link.clone();
            self.thread.send(move || link.resume());
        }

        /// Runs later even on the link thread, after the changes sent before
        /// it, so that the link ends with the last limit.
        fn set_limit(&self, limit: Option<Limit>) {
            let link = self.link.clone();
            self.thread.send(move || link.set_limit(limit));
        }
    }

    impl Drop for DisplayLinkSource {
        /// Invalidates the link, which lets go of its target. It runs later
        /// even on the link thread, which can drop the pacer from its own
        /// callback.
        fn drop(&mut self) {
            let link = self.link.clone();
            self.thread.send(move || link.invalidate());
        }
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "ios", target_os = "tvos")))]
mod tests {
    use crate::frame_pacer::mln_frame_pacer_create;
    use crate::frame_pacer::tests::expect_paced_frames;
    use crate::generated::types::{
        MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, MLN_STATUS_OK, mln_frame_pacer, mln_frame_pacer_options,
    };
    use crate::render_session::tests::sized;

    fn create(options: Option<&mln_frame_pacer_options>) -> mln_frame_pacer {
        let options = options.map_or(core::ptr::null(), core::ptr::from_ref);
        let mut pacer = crate::handles::NULL;
        // SAFETY: the caller's options, a local out-handle and a null
        // diagnostic.
        let status = unsafe { mln_frame_pacer_create(options, &raw mut pacer, core::ptr::null_mut()) };
        assert_eq!(status, MLN_STATUS_OK);
        pacer
    }

    #[test]
    fn a_display_link_paces_a_session() {
        expect_paced_frames(create(None), false);
    }

    #[test]
    fn a_released_display_link_pacer_keeps_pacing_its_sessions() {
        expect_paced_frames(create(None), true);
    }

    /// The limit also sets the display link's preferred frame rate range.
    #[test]
    fn a_display_link_with_a_frame_rate_limit_paces_a_session() {
        let options = mln_frame_pacer_options {
            fields: MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND,
            maximum_frames_per_second: 30,
            ..sized()
        };
        expect_paced_frames(create(Some(&options)), false);
    }
}
