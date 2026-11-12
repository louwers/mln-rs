//! The pacer's source on Android: the `AChoreographer` of the thread that
//! creates it.

/// `AChoreographer` works only on its looper's thread, so a request from
/// another thread signals an eventfd that the looper watches, and the watch's
/// callback posts the frame callback.
///
/// `AChoreographer` can stop while the phone's display is off, even while a
/// Surface on another display, such as a car's, still shows the map. So each
/// request also arms a timerfd that the looper watches, which sends the frame
/// when the vsync is late. Once the timer sent a frame, it sends one every
/// [`TIMER_FRAME_INTERVAL`] that a frame is requested, until a vsync comes
/// first again. The pacer's throttle keeps the timer's frames within its
/// limit, as it does the vsyncs, so the source follows no limit of its own,
/// and a new one needs nothing from it.
///
/// A posted frame callback can't be cancelled, and the looper keeps calling
/// a watch's callback until the callback ends the watch, so each of them
/// holds the [`LooperSide`] until it runs for the last time.
#[cfg(target_os = "android")]
pub(crate) mod choreographer {
    use core::ffi::{c_int, c_long, c_void};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crate::error::Error;
    use crate::frame_pacer::{Limit, VsyncSource, Vsyncs};
    use crate::lock;

    /// Longer than a vsync period at the lowest refresh rate that a phone
    /// display runs at while it is on.
    const VSYNC_TIMEOUT: Duration = Duration::from_millis(50);
    const TIMER_FRAME_INTERVAL: Duration = Duration::from_millis(16);

    #[repr(C)]
    pub(crate) struct ALooper {
        _opaque: [u8; 0],
    }

    #[repr(C)]
    struct AChoreographer {
        _opaque: [u8; 0],
    }

    type ALooperCallback = unsafe extern "C" fn(fd: c_int, events: c_int, data: *mut c_void) -> c_int;
    type FrameCallback = unsafe extern "C" fn(frame_time_nanos: c_long, data: *mut c_void);
    type FrameCallback64 = unsafe extern "C" fn(frame_time_nanos: i64, data: *mut c_void);
    type PostFrameCallback64 = unsafe extern "C" fn(*mut AChoreographer, FrameCallback64, *mut c_void);

    const ALOOPER_POLL_CALLBACK: c_int = -2;
    const ALOOPER_EVENT_INPUT: c_int = 1;

    #[link(name = "android")]
    unsafe extern "C" {
        fn ALooper_forThread() -> *mut ALooper;
        fn ALooper_addFd(
            looper: *mut ALooper,
            fd: c_int,
            ident: c_int,
            events: c_int,
            callback: Option<ALooperCallback>,
            data: *mut c_void,
        ) -> c_int;
        fn ALooper_removeFd(looper: *mut ALooper, fd: c_int) -> c_int;
        fn AChoreographer_getInstance() -> *mut AChoreographer;
        fn AChoreographer_postFrameCallback(
            choreographer: *mut AChoreographer,
            callback: FrameCallback,
            data: *mut c_void,
        );
    }

    /// API level 29 added `AChoreographer_postFrameCallback64()`. The older
    /// `AChoreographer_postFrameCallback()` passes the frame time as a
    /// `long`, which is 32 bits in a 32-bit process.
    fn post_frame_callback64() -> Option<PostFrameCallback64> {
        // SAFETY: a lookup of a NUL-terminated name.
        let function = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"AChoreographer_postFrameCallback64".as_ptr()) };
        // SAFETY: the NDK's function of that name has this signature.
        (!function.is_null()).then(|| unsafe { core::mem::transmute::<*mut c_void, PostFrameCallback64>(function) })
    }

    /// The clock of a frame callback's frame time.
    fn monotonic_now() -> Duration {
        let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: a local timespec.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut now) };
        Duration::new(now.tv_sec as u64, now.tv_nsec as u32)
    }

    /// Arms `timer_fd` to expire once after `delay`, or disarms it for a zero
    /// `delay`.
    #[allow(clippy::cast_lossless, reason = "`c_long` takes the nanoseconds without loss only where it has 64 bits")]
    fn arm_timer(timer_fd: c_int, delay: Duration) {
        let spec = libc::itimerspec {
            it_interval: libc::timespec { tv_sec: 0, tv_nsec: 0 },
            it_value: libc::timespec { tv_sec: delay.as_secs() as _, tv_nsec: delay.subsec_nanos() as _ },
        };
        // SAFETY: a local spec, and no old value to write.
        unsafe { libc::timerfd_settime(timer_fd, 0, &raw const spec, core::ptr::null_mut()) };
    }

    /// Reads the 8-byte counter of an eventfd or a timerfd, and whether there
    /// was one to read.
    fn read_counter(fd: c_int) -> bool {
        let mut count = 0_u64;
        // SAFETY: reads at most the 8 bytes of a local.
        let read = unsafe { libc::read(fd, (&raw mut count).cast(), size_of::<u64>()) };
        read == size_of::<u64>() as isize
    }

    /// Whether the choreographer reports no vsyncs, as a test can make it.
    fn choreographer_stopped() -> bool {
        #[cfg(test)]
        return tests::choreographer_stopped();
        #[cfg(not(test))]
        false
    }

    /// What the looper thread's callbacks share with the pacer. The watches
    /// of the eventfd and the timerfd and a posted frame callback each hold
    /// it until they end, so it can outlive the pacer.
    struct LooperSide {
        looper: *mut ALooper,
        choreographer: *mut AChoreographer,
        event_fd: c_int,
        timer_fd: c_int,
        thread: libc::pid_t,
        vsyncs: Vsyncs,
        stopping: AtomicBool,
        /// Used only on the looper's thread, and never locked while the
        /// pacer runs.
        state: Mutex<LooperState>,
    }

    // SAFETY: the looper and the choreographer are used only on their
    // thread, except `ALooper_*` calls, which are thread-safe.
    unsafe impl Send for LooperSide {}
    // SAFETY: as for `Send`.
    unsafe impl Sync for LooperSide {}

    #[derive(Default)]
    struct LooperState {
        watch_hold: Option<Arc<LooperSide>>,
        timer_hold: Option<Arc<LooperSide>>,
        frame_hold: Option<Arc<LooperSide>>,
        /// Whether the pacer waits for a frame that neither a vsync nor the
        /// timer has sent.
        requested: bool,
        /// Whether the timer sent the last frame.
        vsync_stopped: bool,
        last_frame_time: Option<Duration>,
    }

    impl Drop for LooperSide {
        fn drop(&mut self) {
            // SAFETY: the side's own descriptors, which no watch uses now.
            unsafe {
                libc::close(self.event_fd);
                libc::close(self.timer_fd);
            }
        }
    }

    impl LooperSide {
        fn data(self: &Arc<Self>) -> *mut c_void {
            Arc::as_ptr(self).cast_mut().cast()
        }

        /// The side that a callback's `data` names, which a hold keeps.
        ///
        /// # Safety
        ///
        /// `data` is a [`data`](Self::data) whose hold the callback hasn't
        /// dropped.
        unsafe fn from_data<'a>(data: *mut c_void) -> &'a Self {
            // SAFETY: the caller's contract.
            unsafe { &*data.cast::<Self>() }
        }

        fn signal(&self) {
            let one = 1_u64;
            // A write that fails because the counter is full still leaves
            // the looper a signal to read.
            // SAFETY: writes the 8 bytes of a local.
            while unsafe { libc::write(self.event_fd, (&raw const one).cast(), size_of::<u64>()) } < 0
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
            {}
        }

        /// Sends a frame at `time` to the pacer. Runs on the looper's thread.
        fn send_frame(&self, time: Duration, from_vsync: bool) {
            let time = {
                let mut state = lock(&self.state);
                state.requested = false;
                state.vsync_stopped = !from_vsync;
                // A vsync's time can come before the time of a timer frame
                // sent just before it.
                let time = state.last_frame_time.map_or(time, |last| time.max(last + Duration::from_nanos(1)));
                state.last_frame_time = Some(time);
                time
            };
            arm_timer(self.timer_fd, Duration::ZERO);
            self.vsyncs.vsync(time);
        }

        /// Asks for a frame. Runs on the looper's thread.
        fn request_frame(self: &Arc<Self>) {
            let mut state = lock(&self.state);
            if std::mem::replace(&mut state.requested, true) {
                return;
            }
            // A frame callback that is still posted serves this request.
            if state.frame_hold.is_none() && !choreographer_stopped() {
                state.frame_hold = Some(Arc::clone(self));
                // SAFETY: the choreographer of this thread, and a callback
                // whose data the hold keeps.
                unsafe {
                    match post_frame_callback64() {
                        Some(post) => post(self.choreographer, on_frame64, self.data()),
                        None => AChoreographer_postFrameCallback(self.choreographer, on_frame, self.data()),
                    }
                }
            }
            let delay = if state.vsync_stopped { TIMER_FRAME_INTERVAL } else { VSYNC_TIMEOUT };
            drop(state);
            arm_timer(self.timer_fd, delay);
        }

        /// Runs the frame callback that `data` names, with its frame time.
        ///
        /// # Safety
        ///
        /// `data` is a [`data`](Self::data) that a frame callback was posted
        /// with, and whose callback this is.
        unsafe fn run_frame_callback(data: *mut c_void, time: Duration) {
            // SAFETY: the frame hold keeps the side until this takes it.
            let side = unsafe { Self::from_data(data) };
            // The hold lets go of the side once the frame has run.
            let hold = lock(&side.state).frame_hold.take();
            if let Some(side) = hold {
                side.run_frame(time);
            }
        }

        fn run_frame(&self, time: Duration) {
            // Leaves armed the timer that ends the timer's watch.
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            let requested = {
                let mut state = lock(&self.state);
                if !state.requested {
                    // A frame callback that was posted before the timer took
                    // over shows that vsyncs arrive again.
                    state.vsync_stopped = false;
                }
                state.requested
            };
            if requested {
                self.send_frame(time, true);
            }
        }
    }

    unsafe extern "C" fn on_frame64(frame_time_nanos: i64, data: *mut c_void) {
        // SAFETY: the data of a frame callback that the side posted.
        unsafe { LooperSide::run_frame_callback(data, Duration::from_nanos(frame_time_nanos.max(0) as u64)) };
    }

    /// A 32-bit `long` carries only the low bits of the frame time, so the
    /// high bits come from `CLOCK_MONOTONIC`, the frame time's clock, as the
    /// NDK recommends.
    unsafe extern "C" fn on_frame(frame_time_nanos: c_long, data: *mut c_void) {
        // A `long` is an `i64` in a 64-bit process.
        #[allow(clippy::useless_conversion)]
        let mut time = i64::from(frame_time_nanos);
        if size_of::<c_long>() < size_of::<i64>() {
            const LOW_BITS: i64 = 0xffff_ffff;
            let now = monotonic_now().as_nanos() as i64;
            time = (now & !LOW_BITS) | (time & LOW_BITS);
            if time > now {
                time -= LOW_BITS + 1;
            }
        }
        // SAFETY: the data of a frame callback that the side posted.
        unsafe { LooperSide::run_frame_callback(data, Duration::from_nanos(time.max(0) as u64)) };
    }

    unsafe extern "C" fn on_signal(fd: c_int, _events: c_int, data: *mut c_void) -> c_int {
        // SAFETY: the watch hold keeps the side until the stop below drops
        // it.
        let side = unsafe { LooperSide::from_data(data) };
        read_counter(fd);
        if !side.stopping.load(Ordering::SeqCst) {
            let hold = lock(&side.state).watch_hold.clone();
            if let Some(side) = hold {
                side.request_frame();
            }
            return 1;
        }
        // Wakes the timer's watch, so that it ends too.
        arm_timer(side.timer_fd, Duration::from_nanos(1));
        // Ends the watch before the side can close the eventfd, which the
        // looper would otherwise still watch when this returns.
        // SAFETY: this thread's looper, which watches `fd`.
        unsafe { ALooper_removeFd(side.looper, fd) };
        let hold = lock(&side.state).watch_hold.take();
        drop(hold);
        0
    }

    unsafe extern "C" fn on_timer(fd: c_int, _events: c_int, data: *mut c_void) -> c_int {
        // SAFETY: the timer hold keeps the side until the stop below drops
        // it.
        let side = unsafe { LooperSide::from_data(data) };
        // A timer that a frame disarmed after it expired has nothing to
        // read.
        let expired = read_counter(fd);
        if !side.stopping.load(Ordering::SeqCst) {
            let requested = lock(&side.state).requested;
            if expired && requested {
                side.send_frame(monotonic_now(), false);
            }
            return 1;
        }
        // SAFETY: this thread's looper, which watches `fd`.
        unsafe { ALooper_removeFd(side.looper, fd) };
        let hold = lock(&side.state).timer_hold.take();
        drop(hold);
        0
    }

    /// A pacer's choreographer and its looper thread.
    pub(crate) struct ChoreographerSource {
        side: Arc<LooperSide>,
    }

    impl ChoreographerSource {
        pub(crate) fn new(vsyncs: Vsyncs, _limit: Option<Limit>) -> Result<Self, Error> {
            // SAFETY: no arguments.
            let looper = unsafe { ALooper_forThread() };
            if looper.is_null() {
                return Err(Error::invalid_state("the calling thread has no ALooper"));
            }
            // SAFETY: no arguments; this thread has a looper.
            let choreographer = unsafe { AChoreographer_getInstance() };
            if choreographer.is_null() {
                return Err(Error::native("the calling thread has no AChoreographer"));
            }
            // SAFETY: flags only.
            let event_fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
            if event_fd < 0 {
                return Err(Error::native("the frame pacer could not create an eventfd"));
            }
            // SAFETY: flags only.
            let timer_fd =
                unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC | libc::TFD_NONBLOCK) };
            if timer_fd < 0 {
                // SAFETY: the eventfd above.
                unsafe { libc::close(event_fd) };
                return Err(Error::native("the frame pacer could not create a timerfd"));
            }
            let side = Arc::new(LooperSide {
                looper,
                choreographer,
                event_fd,
                timer_fd,
                // SAFETY: no arguments.
                thread: unsafe { libc::gettid() },
                vsyncs,
                stopping: AtomicBool::new(false),
                state: Mutex::new(LooperState::default()),
            });
            let source = Self { side: Arc::clone(&side) };
            lock(&side.state).watch_hold = Some(Arc::clone(&side));
            // SAFETY: this thread's looper, and a callback whose data the
            // watch hold keeps.
            let added = unsafe {
                ALooper_addFd(
                    looper,
                    event_fd,
                    ALOOPER_POLL_CALLBACK,
                    ALOOPER_EVENT_INPUT,
                    Some(on_signal),
                    side.data(),
                )
            };
            if added != 1 {
                lock(&side.state).watch_hold = None;
                // Dropping the source signals a stop that no watch reads.
                return Err(Error::native("the frame pacer could not watch its eventfd"));
            }
            lock(&side.state).timer_hold = Some(Arc::clone(&side));
            // SAFETY: as above.
            let added = unsafe {
                ALooper_addFd(looper, timer_fd, ALOOPER_POLL_CALLBACK, ALOOPER_EVENT_INPUT, Some(on_timer), side.data())
            };
            if added != 1 {
                lock(&side.state).timer_hold = None;
                // Dropping the source ends the eventfd's watch.
                return Err(Error::native("the frame pacer could not watch its timerfd"));
            }
            Ok(source)
        }
    }

    impl VsyncSource for ChoreographerSource {
        fn schedule(&self) {
            // SAFETY: no arguments.
            if unsafe { libc::gettid() } == self.side.thread {
                self.side.request_frame();
            } else {
                self.side.signal();
            }
        }
    }

    impl Drop for ChoreographerSource {
        /// Ends the watches, which let go of the side once they see the
        /// signal.
        fn drop(&mut self) {
            self.side.stopping.store(true, Ordering::SeqCst);
            self.side.signal();
        }
    }
    #[cfg(test)]
    mod tests {
        use std::cell::Cell;
        use std::sync::mpsc;

        use super::*;
        use crate::frame_pacer::mln_frame_pacer_create;
        use crate::frame_pacer::tests::expect_paced_frames;
        use crate::generated::types::{MLN_STATUS_INVALID_STATE, MLN_STATUS_OK, mln_frame_pacer, mln_status};
        use crate::handles;

        unsafe extern "C" {
            fn ALooper_prepare(options: c_int) -> *mut ALooper;
            fn ALooper_pollOnce(
                timeout_millis: c_int,
                out_fd: *mut c_int,
                out_events: *mut c_int,
                out_data: *mut *mut c_void,
            ) -> c_int;
            fn ALooper_wake(looper: *mut ALooper);
        }

        thread_local! {
            /// Whether this thread's choreographer reports no vsyncs, as it
            /// can while the display is off.
            static CHOREOGRAPHER_STOPPED: Cell<bool> = const { Cell::new(false) };
        }

        pub(super) fn choreographer_stopped() -> bool {
            CHOREOGRAPHER_STOPPED.get()
        }

        struct Looper(*mut ALooper);

        // SAFETY: other threads only wake the looper, which is thread-safe.
        unsafe impl Send for Looper {}

        fn create_pacer() -> (mln_status, mln_frame_pacer) {
            let mut pacer = handles::NULL;
            // SAFETY: null options, a local out-handle and a null diagnostic.
            let status = unsafe { mln_frame_pacer_create(core::ptr::null(), &raw mut pacer, core::ptr::null_mut()) };
            (status, pacer)
        }

        /// Runs `body` with a pacer on a thread that polls its looper, as an
        /// app's main thread does, and whose choreographer reports no vsyncs
        /// when `choreographer_stopped` says so. Stops the thread after, even
        /// when `body` panics.
        fn with_looper_pacer(choreographer_stopped: bool, body: impl FnOnce(mln_frame_pacer)) {
            let stop = Arc::new(AtomicBool::new(false));
            let (ready, readied) = mpsc::channel();
            let thread = std::thread::spawn({
                let stop = Arc::clone(&stop);
                move || {
                    CHOREOGRAPHER_STOPPED.set(choreographer_stopped);
                    // SAFETY: prepares this thread's own looper.
                    let looper = unsafe { ALooper_prepare(0) };
                    let _ = ready.send((Looper(looper), create_pacer()));
                    while !stop.load(Ordering::SeqCst) {
                        // SAFETY: this thread's looper, and no outputs.
                        unsafe {
                            ALooper_pollOnce(-1, core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut())
                        };
                    }
                }
            });
            let (looper, (status, pacer)) = readied.recv().unwrap();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                assert_eq!(status, MLN_STATUS_OK);
                body(pacer);
            }));
            stop.store(true, Ordering::SeqCst);
            // SAFETY: the thread's looper, which lives until the thread ends.
            unsafe { ALooper_wake(looper.0) };
            thread.join().unwrap();
            if let Err(panic) = outcome {
                std::panic::resume_unwind(panic);
            }
        }

        #[test]
        fn a_thread_without_a_looper_creates_no_pacer() {
            let (status, pacer) = std::thread::spawn(create_pacer).join().unwrap();
            assert_eq!((status, pacer), (MLN_STATUS_INVALID_STATE, handles::NULL));
        }

        #[test]
        fn a_choreographer_paces_a_session() {
            with_looper_pacer(false, |pacer| expect_paced_frames(pacer, false));
        }

        #[test]
        fn the_timer_paces_a_session_while_the_choreographer_stops() {
            with_looper_pacer(true, |pacer| expect_paced_frames(pacer, false));
        }

        #[test]
        fn a_released_choreographer_pacer_keeps_pacing_its_sessions() {
            with_looper_pacer(false, |pacer| expect_paced_frames(pacer, true));
        }
    }
}
