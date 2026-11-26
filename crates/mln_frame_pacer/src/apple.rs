//! The source on macOS, iOS and tvOS: a `CADisplayLink`.

/// Every pacer's display link runs on one thread that the library starts for
/// them, on a run loop that runs for the rest of the process, so a pacer
/// needs no run loop of the host's. A display link and its run loop aren't
/// thread-safe, so once the link is in that run loop, the pacer touches it
/// only on that thread: a call from another thread runs a task there. A link
/// that the main thread makes, as `AppKit` and `UIKit` ask, goes to that
/// thread before anything else uses it. A display link retains its target
/// until it is invalidated, so the target reaches the pacer through
/// [`Vsyncs`], which holds it weakly.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
pub(crate) mod display_link {
    use core::ffi::{c_int, c_void};
    use std::collections::VecDeque;
    use std::sync::mpsc;
    use std::thread::{self, ThreadId};
    use std::time::Duration;

    use mln_style::error::Error;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, sel};
    use objc2_core_foundation::{
        CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, kCFRunLoopDefaultMode,
    };
    use objc2_foundation::{NSDefaultRunLoopMode, NSRunLoop};
    use objc2_quartz_core::{CADisplayLink, CAFrameRateRange, CAFrameRateRangeDefault};
    use parking_lot::Mutex;

    use crate::{Limit, VsyncSource, Vsyncs};

    // A vsync that waits behind other work delays the frames it demands.
    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;

    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: c_int) -> c_int;
    }

    // NSView and NSScreen, which make the display links on macOS, are
    // AppKit's.
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
            let mut started = THREAD.lock();
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
            TASKS.lock().push_back(Box::new(task));
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
        let Some(source) = (unsafe { CFRunLoopSource::new(None, 0, &raw mut context) }) else {
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
            let Some(task) = TASKS.lock().pop_front() else {
                return;
            };
            // A panic stays on this side of the run loop.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| autoreleasepool(|_| task())));
        }
    }

    /// A display link, which only the link thread uses.
    #[derive(Clone)]
    struct Link(Retained<CADisplayLink>);

    // SAFETY: the link's methods run only on the thread that makes it, until
    // it hands the link to the link thread, and on the link thread after;
    // other threads only retain and release it, which is thread-safe.
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
        fn add_to_current_run_loop(&self) {
            // SAFETY: the link thread's own run loop.
            unsafe { self.0.addToRunLoop_forMode(&NSRunLoop::currentRunLoop(), NSDefaultRunLoopMode) };
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

    /// An `NSView` on macOS, or a `UIView` on iOS and tvOS, whose window's
    /// display a pacer follows.
    #[derive(Clone, Copy)]
    pub(crate) struct View(pub(crate) core::ptr::NonNull<c_void>);

    impl View {
        /// The view, retained. Runs on the main thread.
        fn retain(self) -> Result<Retained<AnyObject>, Error> {
            // SAFETY: the caller's view, which `for_view`'s contract makes a
            // view.
            unsafe { Retained::retain(self.0.as_ptr().cast::<AnyObject>()) }
                .ok_or_else(|| Error::invalid_argument("the frame pacer's view is null"))
        }
    }

    /// What makes the link: the view, whose link follows its window between
    /// displays, or the main `NSScreen`. Runs on the main thread, or without
    /// a view, on any.
    #[cfg(target_os = "macos")]
    fn link_owner(view: Option<View>) -> Result<Retained<AnyObject>, Error> {
        use objc2::runtime::AnyClass;

        if let Some(view) = view {
            return view.retain();
        }
        let no_display = || Error::native("the system has no display for the pacer to follow");
        let screens = AnyClass::get(c"NSScreen").ok_or_else(no_display)?;
        // SAFETY: `+[NSScreen screens]`, an array of screens, or nil without
        // a window server.
        let screens: Option<Retained<AnyObject>> = unsafe { msg_send![screens, screens] };
        // SAFETY: `-[NSArray firstObject]`, the main display, or nil.
        let screen: Option<Retained<AnyObject>> = unsafe { msg_send![&*screens.ok_or_else(no_display)?, firstObject] };
        screen.ok_or_else(no_display)
    }

    /// What makes the link: the view's window's `UIScreen`, or none for the
    /// main screen's. Runs on the main thread.
    #[cfg(not(target_os = "macos"))]
    fn link_owner(view: Option<View>) -> Result<Option<Retained<AnyObject>>, Error> {
        let Some(view) = view else {
            return Ok(None);
        };
        let view = view.retain()?;
        // SAFETY: `-[UIView window]`, or nil for a view in no window.
        let window: Option<Retained<AnyObject>> = unsafe { msg_send![&*view, window] };
        let window = window.ok_or_else(|| Error::invalid_state("the frame pacer's view is in no window"))?;
        // SAFETY: `-[UIWindow screen]`, which is never nil.
        let screen: Retained<AnyObject> = unsafe { msg_send![&*window, screen] };
        Ok(Some(screen))
    }

    /// A display link that calls `target`.
    fn display_link(target: &Target, view: Option<View>) -> Result<Retained<CADisplayLink>, Error> {
        let owner = link_owner(view)?;
        #[cfg(not(target_os = "macos"))]
        let Some(owner) = owner else {
            // SAFETY: a target that implements the selector.
            return Ok(unsafe { CADisplayLink::displayLinkWithTarget_selector(target, sel!(vsync:)) });
        };
        #[cfg(target_os = "macos")]
        {
            // SAFETY: `-[NSObject respondsToSelector:]`. macOS 14 added the
            // method to NSView and NSScreen.
            let available: bool =
                unsafe { msg_send![&*owner, respondsToSelector: sel!(displayLinkWithTarget:selector:)] };
            if !available {
                return Err(Error::unsupported("the frame pacer needs macOS 14 or later"));
            }
        }
        // SAFETY: `-[NSView displayLinkWithTarget:selector:]`,
        // `-[NSScreen displayLinkWithTarget:selector:]` or
        // `-[UIScreen displayLinkWithTarget:selector:]`, with a target that
        // implements the selector.
        let link: Option<Retained<CADisplayLink>> =
            unsafe { msg_send![&*owner, displayLinkWithTarget: target, selector: sel!(vsync:)] };
        link.ok_or_else(|| Error::native("the system has no display for the pacer to follow"))
    }

    /// A new display link, paused and at `limit`, that reports to `vsyncs`.
    fn new_link(vsyncs: Vsyncs, limit: Option<Limit>, view: Option<View>) -> Result<Link, Error> {
        let target = Target::new(vsyncs);
        let link = Link(display_link(&target, view)?);
        link.0.setPaused(true);
        if limit.is_some() {
            link.set_limit(limit);
        }
        Ok(link)
    }

    /// A pacer's display link.
    pub(crate) struct DisplayLinkSource {
        thread: &'static LinkThread,
        link: Link,
    }

    impl DisplayLinkSource {
        /// A source on a link of `view`'s window's display, or of the main
        /// display. A view's link is made on the main thread, which must run
        /// this, and on macOS so is the main display's when this runs there.
        pub(crate) fn new(vsyncs: Vsyncs, limit: Option<Limit>, view: Option<View>) -> Result<Self, Error> {
            let on_main = objc2::MainThreadMarker::new().is_some();
            if view.is_some() && !on_main {
                return Err(Error::invalid_state("a frame pacer for a view must be made on the main thread"));
            }
            let thread = LinkThread::get()?;
            let failed = || Error::native("the frame pacer's thread failed to take its display link");
            let link = if view.is_some() || (cfg!(target_os = "macos") && on_main) {
                let link = new_link(vsyncs, limit, view)?;
                let added = link.clone();
                // A link calls back on the thread whose run loop it is in.
                thread.call(move || added.add_to_current_run_loop()).ok_or_else(failed)?;
                link
            } else {
                thread
                    .call(move || {
                        let link = new_link(vsyncs, limit, None)?;
                        link.add_to_current_run_loop();
                        Ok(link)
                    })
                    .ok_or_else(failed)??
            };
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
