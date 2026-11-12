//! The pacer's tests: its limit, its scheduling, and its sessions with a
//! fake display that the tests refresh, and helpers for each platform's tests
//! of its own source.

use core::ffi::c_void;
use std::sync::{Condvar, Mutex};
use std::time::Instant;

use super::*;
use crate::generated::types::{
    MLN_RENDER_RESULT_RENDERED, MLN_STATUS_INVALID_ARGUMENT, MLN_STATUS_INVALID_STATE, MLN_STATUS_OK,
    mln_completion_result, mln_diagnostic, mln_map, mln_render_frame_batch_view, mln_render_frame_result, mln_runtime,
    mln_status,
};
use crate::map::mln_map_set_style_json;
use crate::map::tests::{create_map, options, release_map, view};
use crate::render_session::tests::{
    attach_options, demand, offscreen_target, release_session, rendered_session, sized,
};
use crate::render_session::{
    mln_frame_pacer_release, mln_render_frame_batch_release, mln_render_session_barrier,
    mln_render_session_drain_frame_results, mln_render_session_set_frame_pacer,
};
use crate::render_target::mln_map_attach;
use crate::runtime::tests::{Waiter, channel_completion, create as create_runtime, release as release_runtime};

const NO_DIAGNOSTIC: *mut mln_diagnostic = core::ptr::null_mut();
const TIMEOUT: Duration = Duration::from_secs(30);

// MARK: - Limit

/// How many of the refreshes at `display_rate` over `seconds` a limit of
/// `limit` frames per second admits, with each refresh `jitter` early or late
/// in turn.
fn admitted(limit: Option<u32>, display_rate: u32, seconds: u32, jitter: Duration) -> u32 {
    let mut throttle = Throttle::new(limit.map(|limit| Limit::new(limit).unwrap()));
    let period = Duration::from_secs(1) / display_rate;
    let start = Duration::from_secs(1000);
    (0..display_rate * seconds)
        .filter(|&refresh| {
            let ideal = start + period * refresh;
            let time = if refresh % 2 == 0 { ideal + jitter } else { ideal.checked_sub(jitter).unwrap() };
            throttle.admit(time)
        })
        .count() as u32
}

#[test]
fn without_a_limit_every_refresh_demands() {
    assert_eq!(admitted(None, 60, 10, Duration::ZERO), 600);
    assert_eq!(admitted(None, 240, 10, Duration::from_millis(1)), 2400);
}

#[test]
fn a_limit_at_the_display_rate_takes_every_jittery_refresh() {
    assert_eq!(admitted(Some(60), 60, 10, Duration::from_micros(500)), 600);
    assert_eq!(admitted(Some(120), 120, 10, Duration::from_micros(300)), 1200);
}

#[test]
fn a_limit_below_the_display_rate_demands_up_to_its_rate() {
    for (limit, display_rate) in [(30, 60), (55, 60), (50, 120), (60, 144), (1, 60), (59, 60), (45, 120)] {
        let frames = admitted(Some(limit), display_rate, 10, Duration::from_micros(200));
        // At most one frame more than the limit, and no fewer than taking
        // every nth refresh, for the smallest n that stays within it.
        let floor = display_rate / display_rate.div_ceil(limit);
        assert!(frames <= limit * 10 + 1, "{frames} frames at {limit} on a {display_rate} Hz display");
        assert!(frames >= floor * 10, "{frames} frames at {limit} on a {display_rate} Hz display");
    }
}

#[test]
fn a_limit_above_the_display_rate_takes_every_refresh() {
    assert_eq!(admitted(Some(120), 60, 10, Duration::from_micros(500)), 600);
}

#[test]
fn an_early_refresh_waits_for_the_next() {
    let mut throttle = Throttle::new(Limit::new(30));
    let at = Duration::from_millis;
    assert!(throttle.admit(at(1000)));
    assert!(!throttle.admit(at(1016)));
    // Within the slack before the grid's next point, at 1033.3 ms.
    assert!(throttle.admit(at(1031)));
    assert!(!throttle.admit(at(1050)));
    assert!(throttle.admit(at(1066)));
}

#[test]
fn an_idle_spell_or_a_clock_that_moves_backward_starts_the_grid_again() {
    let mut throttle = Throttle::new(Limit::new(10));
    let at = Duration::from_millis;
    assert!(throttle.admit(at(1000)));
    // Over a period past the grid's next point.
    assert!(throttle.admit(at(5000)));
    assert!(!throttle.admit(at(5050)));
    assert!(throttle.admit(at(100)));
    assert!(!throttle.admit(at(150)));
    assert!(throttle.admit(at(195)));
}

#[test]
fn a_new_limit_counts_from_the_last_demand() {
    let at = Duration::from_millis;
    // A higher limit lets the next refresh come sooner.
    let mut throttle = Throttle::new(Limit::new(10));
    assert!(throttle.admit(at(1000)));
    throttle.set_limit(Limit::new(60));
    assert!(!throttle.admit(at(1008)));
    assert!(throttle.admit(at(1016)));
    assert!(!throttle.admit(at(1025)));
    assert!(throttle.admit(at(1033)));

    // A lower limit skips the refreshes that the old one took.
    throttle.set_limit(Limit::new(10));
    assert!(!throttle.admit(at(1050)));
    assert!(!throttle.admit(at(1116)));
    assert!(throttle.admit(at(1133)));

    // Without a limit, every refresh demands, and a limit set after them
    // counts from the last.
    throttle.set_limit(None);
    assert!(throttle.admit(at(1150)));
    assert!(throttle.admit(at(1166)));
    throttle.set_limit(Limit::new(30));
    assert!(!throttle.admit(at(1183)));
    assert!(throttle.admit(at(1200)));
}

#[test]
fn a_limit_set_before_any_demand_takes_the_next_refresh() {
    let mut throttle = Throttle::new(None);
    throttle.set_limit(Limit::new(30));
    assert!(throttle.admit(Duration::from_millis(1000)));
    assert!(!throttle.admit(Duration::from_millis(1016)));
}

// MARK: - Options

/// A source that the test refreshes.
#[derive(Default)]
pub(crate) struct FakeDisplay {
    vsyncs: OnceLock<Vsyncs>,
    /// How many refreshes the pacer scheduled that the test hasn't sent.
    scheduled: Mutex<u32>,
    changed: Condvar,
    /// Whether the pacer dropped its source.
    dropped: Mutex<bool>,
    /// The limits that the pacer told the source of after it created it.
    limits: Mutex<Vec<Option<Limit>>>,
}

struct FakeSource(Arc<FakeDisplay>);

impl VsyncSource for FakeSource {
    fn schedule(&self) {
        *lock(&self.0.scheduled) += 1;
        self.0.changed.notify_all();
    }

    fn set_limit(&self, limit: Option<Limit>) {
        lock(&self.0.limits).push(limit);
    }
}

impl Drop for FakeSource {
    fn drop(&mut self) {
        *lock(&self.0.dropped) = true;
    }
}

impl FakeDisplay {
    /// Sends a refresh at `time` when the pacer scheduled one, and says
    /// whether it did.
    fn refresh(&self, time: Duration) -> bool {
        {
            let mut scheduled = lock(&self.scheduled);
            if *scheduled == 0 {
                return false;
            }
            *scheduled -= 1;
        }
        self.vsyncs.get().unwrap().vsync(time);
        true
    }

    /// Waits until the pacer schedules a refresh, then sends it now.
    fn refresh_when_scheduled(&self) {
        self.wait_until_scheduled();
        assert!(self.refresh(now()));
    }

    /// Waits until the pacer schedules a refresh.
    fn wait_until_scheduled(&self) {
        let deadline = Instant::now() + TIMEOUT;
        let mut scheduled = lock(&self.scheduled);
        while *scheduled == 0 {
            let left = deadline.checked_duration_since(Instant::now()).expect("the pacer scheduled a refresh");
            scheduled = self.changed.wait_timeout(scheduled, left).unwrap().0;
        }
    }

    fn scheduled(&self) -> u32 {
        *lock(&self.scheduled)
    }
}

/// The time of a refresh now, on the monotonic clock that the tests' fake
/// refreshes use.
fn now() -> Duration {
    static START: OnceLock<Instant> = OnceLock::new();
    Duration::from_secs(1) + START.get_or_init(Instant::now).elapsed()
}

/// A pacer on a fake display, from `options`.
fn create_fake(options: *const mln_frame_pacer_options) -> Result<(mln_frame_pacer, Arc<FakeDisplay>), mln_status> {
    let display = Arc::new(FakeDisplay::default());
    let mut pacer = handles::NULL;
    let source = |vsyncs: Vsyncs, _: Option<Limit>| {
        let _ = display.vsyncs.set(vsyncs);
        Ok(FakeSource(Arc::clone(&display)))
    };
    // SAFETY: the caller's options, and a local out-handle.
    match unsafe { create(options, &raw mut pacer, source) } {
        Ok(()) => Ok((pacer, display)),
        Err(error) => Err(error.status()),
    }
}

fn fake_pacer() -> (mln_frame_pacer, Arc<FakeDisplay>) {
    create_fake(core::ptr::null()).unwrap()
}

fn pacer_options(fields: u32, maximum_frames_per_second: u32) -> mln_frame_pacer_options {
    mln_frame_pacer_options { fields, maximum_frames_per_second, ..sized() }
}

fn set_options(pacer: mln_frame_pacer, options: *const mln_frame_pacer_options) -> mln_status {
    // SAFETY: the caller's options, and a null diagnostic.
    unsafe { mln_frame_pacer_set_options(pacer, options, NO_DIAGNOSTIC) }
}

fn limit_of(pacer: mln_frame_pacer) -> Option<Limit> {
    lock(&resolve(pacer).unwrap().state).throttle.limit
}

#[test]
fn options_check_their_fields_and_limit() {
    let limited = pacer_options(MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, 30);
    let (pacer, _) = create_fake(&raw const limited).unwrap();
    assert_eq!(resolve(pacer).unwrap().state.lock().unwrap().throttle.limit, Limit::new(30));
    mln_frame_pacer_release(pacer);
    // A limit that its field doesn't set is no limit.
    let (pacer, _) = create_fake(&pacer_options(0, 0)).unwrap();
    assert_eq!(resolve(pacer).unwrap().state.lock().unwrap().throttle.limit, None);
    mln_frame_pacer_release(pacer);

    // The removal of a limit is the default.
    let (pacer, display) = create_fake(&pacer_options(MLN_FRAME_PACER_OPTION_UNLIMITED_FRAMES_PER_SECOND, 0)).unwrap();
    assert_eq!(limit_of(pacer), None);
    assert!(lock(&display.limits).is_empty());
    mln_frame_pacer_release(pacer);

    let zero = pacer_options(MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, 0);
    assert_eq!(create_fake(&raw const zero).err().unwrap(), MLN_STATUS_INVALID_ARGUMENT);
    let both = pacer_options(
        MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND | MLN_FRAME_PACER_OPTION_UNLIMITED_FRAMES_PER_SECOND,
        30,
    );
    assert_eq!(create_fake(&raw const both).err().unwrap(), MLN_STATUS_INVALID_ARGUMENT);
    assert_eq!(create_fake(&pacer_options(1 << 7, 30)).err().unwrap(), MLN_STATUS_INVALID_ARGUMENT);
    let small = mln_frame_pacer_options { size: 4, ..limited };
    assert_eq!(create_fake(&raw const small).err().unwrap(), MLN_STATUS_INVALID_ARGUMENT);

    let mut pacer = 7;
    // SAFETY: an out-handle that isn't null.
    let created = unsafe { create(core::ptr::null(), &raw mut pacer, |_, _| Ok(FakeSource(Arc::default()))) };
    assert_eq!(created.unwrap_err().status(), MLN_STATUS_INVALID_ARGUMENT);
    // SAFETY: a null out-handle.
    let created = unsafe { create(core::ptr::null(), core::ptr::null_mut(), |_, _| Ok(FakeSource(Arc::default()))) };
    assert_eq!(created.unwrap_err().status(), MLN_STATUS_INVALID_ARGUMENT);
}

#[test]
fn set_options_changes_only_what_it_sets() {
    let (pacer, display) = fake_pacer();
    let limited = pacer_options(MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, 30);
    assert_eq!(set_options(pacer, &raw const limited), MLN_STATUS_OK);
    assert_eq!(limit_of(pacer), Limit::new(30));
    // Options that set neither a limit nor its removal keep the limit.
    assert_eq!(set_options(pacer, &pacer_options(0, 0)), MLN_STATUS_OK);
    assert_eq!(limit_of(pacer), Limit::new(30));
    let unlimited = pacer_options(MLN_FRAME_PACER_OPTION_UNLIMITED_FRAMES_PER_SECOND, 0);
    assert_eq!(set_options(pacer, &raw const unlimited), MLN_STATUS_OK);
    assert_eq!(limit_of(pacer), None);
    // The source hears each change, in order.
    assert_eq!(*lock(&display.limits), [Limit::new(30), None]);
    mln_frame_pacer_release(pacer);
}

#[test]
fn set_options_checks_its_pacer_and_options_and_keeps_the_limit_it_refuses() {
    let limited = pacer_options(MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, 30);
    let (pacer, display) = create_fake(&raw const limited).unwrap();
    let zero = pacer_options(MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, 0);
    assert_eq!(set_options(pacer, &raw const zero), MLN_STATUS_INVALID_ARGUMENT);
    let both = pacer_options(
        MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND | MLN_FRAME_PACER_OPTION_UNLIMITED_FRAMES_PER_SECOND,
        60,
    );
    assert_eq!(set_options(pacer, &raw const both), MLN_STATUS_INVALID_ARGUMENT);
    assert_eq!(set_options(pacer, &pacer_options(1 << 7, 60)), MLN_STATUS_INVALID_ARGUMENT);
    let small = mln_frame_pacer_options { size: 4, ..limited };
    assert_eq!(set_options(pacer, &raw const small), MLN_STATUS_INVALID_ARGUMENT);
    assert_eq!(set_options(pacer, core::ptr::null()), MLN_STATUS_INVALID_ARGUMENT);
    assert_eq!(limit_of(pacer), Limit::new(30));
    assert!(lock(&display.limits).is_empty());

    let unlimited = pacer_options(MLN_FRAME_PACER_OPTION_UNLIMITED_FRAMES_PER_SECOND, 0);
    assert_eq!(set_options(handles::NULL, &raw const unlimited), MLN_STATUS_INVALID_ARGUMENT);
    mln_frame_pacer_release(pacer);
    assert_eq!(set_options(pacer, &raw const unlimited), MLN_STATUS_INVALID_STATE);
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "android",
    target_env = "ohos"
)))]
#[test]
fn a_platform_without_a_display_source_creates_no_pacer() {
    let mut pacer = handles::NULL;
    // SAFETY: null options, a local out-handle and a null diagnostic.
    let status = unsafe { mln_frame_pacer_create(core::ptr::null(), &raw mut pacer, NO_DIAGNOSTIC) };
    assert_eq!((status, pacer), (crate::generated::types::MLN_STATUS_UNSUPPORTED, handles::NULL));
}

// MARK: - Scheduling

#[test]
fn requests_share_one_refresh_until_it_comes() {
    let (handle, display) = fake_pacer();
    let pacer = resolve(handle).unwrap();
    // Handles of no session, which the demands skip.
    pacer.request(1);
    pacer.request(2);
    pacer.request(1);
    assert_eq!(display.scheduled(), 1);
    assert_eq!(lock(&pacer.state).waiting, [1, 2]);
    assert!(display.refresh(now()));
    assert_eq!(lock(&pacer.state).waiting, [] as [u64; 0]);
    assert_eq!(display.scheduled(), 0);

    pacer.request(3);
    assert_eq!(display.scheduled(), 1);
    mln_frame_pacer_release(handle);
}

#[test]
fn a_refresh_that_the_limit_skips_schedules_the_next() {
    let limited = pacer_options(MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, 30);
    let (handle, display) = create_fake(&raw const limited).unwrap();
    let pacer = resolve(handle).unwrap();
    let at = Duration::from_millis;
    pacer.request(1);
    assert!(display.refresh(at(1000)));
    pacer.request(1);
    assert!(display.refresh(at(1016)));
    assert_eq!((display.scheduled(), lock(&pacer.state).waiting.len()), (1, 1));
    assert!(display.refresh(at(1033)));
    assert_eq!((display.scheduled(), lock(&pacer.state).waiting.len()), (0, 0));
    mln_frame_pacer_release(handle);
}

#[test]
fn a_new_limit_applies_to_the_refresh_on_its_way() {
    let limited = pacer_options(MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, 10);
    let (handle, display) = create_fake(&raw const limited).unwrap();
    let pacer = resolve(handle).unwrap();
    let at = Duration::from_millis;
    pacer.request(1);
    assert!(display.refresh(at(1000)));
    pacer.request(1);
    assert!(display.refresh(at(1016)));
    assert_eq!((display.scheduled(), lock(&pacer.state).waiting.len()), (1, 1));

    // The higher limit takes the refresh that the old one would skip.
    let faster = pacer_options(MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, 30);
    assert_eq!(set_options(handle, &raw const faster), MLN_STATUS_OK);
    assert_eq!(display.scheduled(), 1, "the refresh on its way serves the new limit");
    assert!(display.refresh(at(1033)));
    assert_eq!((display.scheduled(), lock(&pacer.state).waiting.len()), (0, 0));

    // The lower limit skips the refresh that the old one would take.
    pacer.request(1);
    let slower = pacer_options(MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, 5);
    assert_eq!(set_options(handle, &raw const slower), MLN_STATUS_OK);
    assert!(display.refresh(at(1066)));
    assert_eq!((display.scheduled(), lock(&pacer.state).waiting.len()), (1, 1));
    assert!(display.refresh(at(1233)));
    assert_eq!((display.scheduled(), lock(&pacer.state).waiting.len()), (0, 0));
    mln_frame_pacer_release(handle);
}

#[test]
fn a_released_pacer_stops_once_no_session_holds_it() {
    let (pacer, display) = fake_pacer();
    mln_frame_pacer_release(pacer);
    assert!(*lock(&display.dropped));
    assert_eq!(resolve(pacer).err().unwrap().status(), MLN_STATUS_INVALID_STATE);
    // Releases of the null handle and of stale handles do nothing.
    mln_frame_pacer_release(pacer);
    mln_frame_pacer_release(handles::NULL);
}

// MARK: - Sessions

fn set_pacer(session: mln_render_session, pacer: mln_frame_pacer) -> mln_status {
    // SAFETY: a null diagnostic.
    unsafe { mln_render_session_set_frame_pacer(session, pacer, NO_DIAGNOSTIC) }
}

fn barrier(session: mln_render_session) {
    let done = Waiter::new();
    // SAFETY: a local completion.
    assert_eq!(
        unsafe { mln_render_session_barrier(session, &raw const done.descriptor, NO_DIAGNOSTIC) },
        MLN_STATUS_OK
    );
    assert_eq!(done.status(), MLN_STATUS_OK);
}

/// The frame results that wait.
fn drain(session: mln_render_session) -> Vec<mln_render_frame_result> {
    let mut batch = handles::NULL;
    let mut view = sized::<mln_render_frame_batch_view>();
    // SAFETY: local outputs.
    let status =
        unsafe { mln_render_session_drain_frame_results(session, &raw mut batch, &raw mut view, NO_DIAGNOSTIC) };
    assert_eq!(status, MLN_STATUS_OK);
    if batch == handles::NULL {
        return Vec::new();
    }
    // SAFETY: the batch's results, live until its release.
    let results = unsafe { core::slice::from_raw_parts(view.results, view.result_count) }.to_vec();
    mln_render_frame_batch_release(batch);
    results
}

unsafe extern "C" fn send_update_generation(user_data: *mut c_void, result: *const mln_completion_result) {
    // SAFETY: the sender that `channel_completion` boxed, and the library's
    // result.
    unsafe {
        let sender = &*user_data.cast::<std::sync::mpsc::Sender<(mln_status, u64)>>();
        let _ = sender.send(((*result).status, (*result).render_update_generation));
    }
}

/// Gives `map` a background of `color`, and returns the map update
/// generation of the frames that show it.
pub(crate) fn paint(map: mln_map, color: &str) -> u64 {
    let style = format!(
        r#"{{"version":8,"sources":{{}},"layers":[{{"id":"bg","type":"background","paint":{{"background-color":"{color}"}}}}]}}"#
    );
    let (completion, receiver) = channel_completion::<(mln_status, u64)>(send_update_generation);
    // SAFETY: local arguments, and a completion whose sender the library
    // releases after its call.
    let status = unsafe { mln_map_set_style_json(map, view(&style), &raw const completion, NO_DIAGNOSTIC) };
    assert_eq!(status, MLN_STATUS_OK);
    let (status, generation) = receiver.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(status, MLN_STATUS_OK);
    generation
}

/// Waits for a frame that the pacer demanded and that shows the map update
/// `generation`, skipping the pacer's frames before it.
pub(crate) fn wait_for_paced_frame(session: mln_render_session, generation: u64) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        for result in drain(session) {
            assert_eq!(result.token, 0, "only the pacer demands frames");
            if result.disposition == MLN_RENDER_RESULT_RENDERED && result.map_update_generation >= generation {
                return;
            }
        }
        assert!(Instant::now() < deadline, "no paced frame showed update {generation}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A session that rendered its map, with what the map had to draw answered,
/// and no frame result waiting.
fn settled_session() -> (mln_runtime, mln_map, mln_render_session) {
    let (runtime, map, session) = rendered_session();
    // The redraw wake of the new session ran before the barrier: after the
    // demand that drew the map or before it.
    barrier(session);
    assert_eq!(demand(session, 0, 9), MLN_STATUS_OK);
    barrier(session);
    assert_eq!(drain(session).iter().map(|result| result.token).collect::<Vec<_>>(), [9]);
    (runtime, map, session)
}

fn end(runtime: mln_runtime, map: mln_map, session: mln_render_session) {
    release_session(session);
    release_map(map);
    release_runtime(runtime);
}

/// Paces a new session with `pacer`, which it releases first when `release`
/// is set, and expects each map change to reach a frame that no one else
/// demands. Each platform's tests run it with a pacer of their own.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", target_os = "tvos", target_os = "android", target_env = "ohos")),
    allow(dead_code)
)]
pub(crate) fn expect_paced_frames(pacer: mln_frame_pacer, release: bool) {
    let (runtime, map, session) = settled_session();
    assert_eq!(set_pacer(session, pacer), MLN_STATUS_OK);
    if release {
        mln_frame_pacer_release(pacer);
    }
    wait_for_paced_frame(session, paint(map, "red"));
    wait_for_paced_frame(session, paint(map, "blue"));
    end(runtime, map, session);
    if !release {
        mln_frame_pacer_release(pacer);
    }
}

#[test]
fn a_paced_session_draws_each_change_at_the_next_refresh() {
    let (pacer, display) = fake_pacer();
    let (runtime, map, session) = settled_session();
    assert_eq!(set_pacer(session, pacer), MLN_STATUS_OK);
    barrier(session);
    assert_eq!(display.scheduled(), 0, "the session had nothing new to draw");

    for color in ["red", "blue"] {
        let generation = paint(map, color);
        display.refresh_when_scheduled();
        barrier(session);
        let results = drain(session);
        assert_eq!(results.len(), 1);
        let result = results[0];
        assert_eq!((result.token, result.disposition), (0, MLN_RENDER_RESULT_RENDERED));
        assert!(result.map_update_generation >= generation);
    }
    end(runtime, map, session);
    mln_frame_pacer_release(pacer);
}

#[test]
fn a_new_pacer_draws_what_the_session_had_to_draw() {
    let (pacer, display) = fake_pacer();
    let (runtime, map, session) = settled_session();
    let generation = paint(map, "red");
    barrier(session);
    assert_eq!(set_pacer(session, pacer), MLN_STATUS_OK);
    assert_eq!(display.scheduled(), 1);
    assert!(display.refresh(now()));
    wait_for_paced_frame(session, generation);
    end(runtime, map, session);
    mln_frame_pacer_release(pacer);
}

#[test]
fn a_released_pacer_keeps_pacing_its_sessions_until_they_end() {
    let (pacer, display) = fake_pacer();
    let (runtime, map, session) = settled_session();
    assert_eq!(set_pacer(session, pacer), MLN_STATUS_OK);
    mln_frame_pacer_release(pacer);
    assert!(!*lock(&display.dropped));

    let generation = paint(map, "red");
    display.refresh_when_scheduled();
    wait_for_paced_frame(session, generation);
    end(runtime, map, session);
    assert!(*lock(&display.dropped));
}

#[test]
fn a_paced_session_follows_a_new_limit_from_the_refresh_it_waits_for() {
    let limited = pacer_options(MLN_FRAME_PACER_OPTION_MAXIMUM_FRAMES_PER_SECOND, 1);
    let (pacer, display) = create_fake(&raw const limited).unwrap();
    let (runtime, map, session) = settled_session();
    assert_eq!(set_pacer(session, pacer), MLN_STATUS_OK);
    let at = Duration::from_millis;
    let generation = paint(map, "red");
    display.wait_until_scheduled();
    assert!(display.refresh(at(1000)));
    wait_for_paced_frame(session, generation);

    let generation = paint(map, "blue");
    barrier(session);
    display.wait_until_scheduled();
    // The limit skips this refresh, and the session waits for the next.
    assert!(display.refresh(at(1016)));
    barrier(session);
    assert!(drain(session).is_empty());
    let unlimited = pacer_options(MLN_FRAME_PACER_OPTION_UNLIMITED_FRAMES_PER_SECOND, 0);
    assert_eq!(set_options(pacer, &raw const unlimited), MLN_STATUS_OK);
    assert!(display.refresh(at(1033)));
    wait_for_paced_frame(session, generation);
    end(runtime, map, session);
    mln_frame_pacer_release(pacer);
}

#[test]
fn a_demand_answers_what_the_session_had_to_draw() {
    let (handle, display) = fake_pacer();
    let (other, _) = fake_pacer();
    let (pacer, other) = (resolve(handle).unwrap(), resolve(other).unwrap());
    let pacing = Arc::new(SessionPacing::default());
    let wake = pacing.wake();
    // A redraw before the session has a pacer waits for one.
    wake();
    assert_eq!(display.scheduled(), 0);
    pacing.set(77, Some(Arc::clone(&pacer)));
    assert_eq!((display.scheduled(), lock(&pacer.state).waiting.clone()), (1, vec![77]));
    // A host's demand answers it, so the refresh demands nothing.
    pacing.demanded();
    assert!(!pacing.take_redraw(&pacer));

    wake();
    assert!(!pacing.take_redraw(&other));
    assert!(pacing.take_redraw(&pacer));
    assert!(!pacing.take_redraw(&pacer));
    wake();
    pacing.set(77, None);
    assert!(!pacing.take_redraw(&pacer));
}

#[test]
fn one_pacer_serves_many_sessions_and_a_null_pacer_stops_one() {
    let (pacer, display) = fake_pacer();
    let sessions = [settled_session(), settled_session()];
    for &(_, _, session) in &sessions {
        assert_eq!(set_pacer(session, pacer), MLN_STATUS_OK);
    }
    let generations = sessions.map(|(_, map, _)| paint(map, "red"));
    for &(_, _, session) in &sessions {
        barrier(session);
    }
    assert_eq!(display.scheduled(), 1);
    assert!(display.refresh(now()));
    for (&(_, _, session), generation) in sessions.iter().zip(generations) {
        wait_for_paced_frame(session, generation);
    }

    let [(_, stopped_map, stopped), (_, paced_map, paced)] = sessions;
    assert_eq!(set_pacer(stopped, handles::NULL), MLN_STATUS_OK);
    barrier(stopped);
    drain(stopped);
    paint(stopped_map, "blue");
    let generation = paint(paced_map, "blue");
    display.refresh_when_scheduled();
    wait_for_paced_frame(paced, generation);
    barrier(stopped);
    assert!(drain(stopped).is_empty());
    for (runtime, map, session) in sessions {
        end(runtime, map, session);
    }
    mln_frame_pacer_release(pacer);
}

unsafe extern "C" fn ignore_redraw(_user_data: *mut c_void) {}

#[test]
fn set_frame_pacer_checks_its_session_and_pacer() {
    let (pacer, _) = fake_pacer();
    let runtime = create_runtime();
    let map = create_map(runtime, &options(1.0));
    let target = offscreen_target(16, 16);
    let mut options = attach_options();
    options.redraw_wake.callback = Some(ignore_redraw);
    let mut session = handles::NULL;
    let attached = Waiter::new();
    // SAFETY: local arguments.
    let status = unsafe {
        mln_map_attach(
            map,
            &raw const target,
            0,
            &raw const options,
            &raw mut session,
            &raw const attached.descriptor,
            NO_DIAGNOSTIC,
        )
    };
    assert_eq!((status, attached.status()), (MLN_STATUS_OK, MLN_STATUS_OK));
    assert_eq!(set_pacer(session, pacer), MLN_STATUS_INVALID_STATE);
    // A null pacer is no pacer, which a session with a redraw wake has.
    assert_eq!(set_pacer(session, handles::NULL), MLN_STATUS_INVALID_STATE);
    // A pacer handle that names no pacer fails first.
    assert_eq!(set_pacer(session, session), MLN_STATUS_INVALID_ARGUMENT);
    assert_eq!(set_pacer(handles::NULL, pacer), MLN_STATUS_INVALID_ARGUMENT);
    release_session(session);
    assert_eq!(set_pacer(session, pacer), MLN_STATUS_INVALID_STATE);
    mln_frame_pacer_release(pacer);
    assert_eq!(set_pacer(handles::NULL, pacer), MLN_STATUS_INVALID_STATE);
    release_map(map);
    release_runtime(runtime);
}
