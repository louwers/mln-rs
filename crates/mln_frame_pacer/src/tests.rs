//! The throttle's grid, and the pacer's requests and limits with a fake
//! source that the tests refresh.

use super::*;

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
fn without_a_limit_every_refresh_calls_back() {
    assert_eq!(admitted(None, 60, 10, Duration::ZERO), 600);
    assert_eq!(admitted(None, 240, 10, Duration::from_millis(1)), 2400);
}

#[test]
fn a_limit_at_the_display_rate_takes_every_jittery_refresh() {
    assert_eq!(admitted(Some(60), 60, 10, Duration::from_micros(500)), 600);
    assert_eq!(admitted(Some(120), 120, 10, Duration::from_micros(300)), 1200);
}

#[test]
fn a_limit_below_the_display_rate_calls_back_up_to_its_rate() {
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
fn a_new_limit_counts_from_the_last_callback() {
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

    // Without a limit, every refresh calls back, and a limit set after them
    // counts from the last.
    throttle.set_limit(None);
    assert!(throttle.admit(at(1150)));
    assert!(throttle.admit(at(1166)));
    throttle.set_limit(Limit::new(30));
    assert!(!throttle.admit(at(1183)));
    assert!(throttle.admit(at(1200)));
}

#[test]
fn a_limit_set_before_any_callback_takes_the_next_refresh() {
    let mut throttle = Throttle::new(None);
    throttle.set_limit(Limit::new(30));
    assert!(throttle.admit(Duration::from_millis(1000)));
    assert!(!throttle.admit(Duration::from_millis(1016)));
}

// MARK: - Pacer

/// A source that the test refreshes.
#[derive(Default)]
struct FakeDisplay {
    vsyncs: OnceLock<Vsyncs>,
    /// How many refreshes the pacer scheduled that the test hasn't sent.
    scheduled: Mutex<u32>,
    /// The limits that the pacer told the source of after it created it.
    limits: Mutex<Vec<Option<Limit>>>,
    dropped: Mutex<bool>,
}

struct FakeSource(Arc<FakeDisplay>);

impl VsyncSource for FakeSource {
    fn schedule(&self) {
        *self.0.scheduled.lock() += 1;
    }

    fn set_limit(&self, limit: Option<Limit>) {
        self.0.limits.lock().push(limit);
    }
}

impl Drop for FakeSource {
    fn drop(&mut self) {
        *self.0.dropped.lock() = true;
    }
}

impl FakeDisplay {
    /// Sends the refresh that the pacer scheduled at `time`.
    fn refresh(&self, time: Duration) {
        let mut scheduled = self.scheduled.lock();
        assert!(*scheduled > 0, "the pacer scheduled no refresh");
        *scheduled -= 1;
        drop(scheduled);
        self.vsyncs.get().unwrap().vsync(time);
    }

    fn scheduled(&self) -> u32 {
        *self.scheduled.lock()
    }
}

type Calls = Arc<Mutex<Vec<(u32, Duration)>>>;

/// A pacer on a fake display, which records each call to its waiters.
fn fake_pacer(limit: Option<Limit>) -> (Arc<FramePacer>, Arc<FakeDisplay>, Calls) {
    let display = Arc::new(FakeDisplay::default());
    let source = |vsyncs: Vsyncs, _: Option<Limit>| {
        let _ = display.vsyncs.set(vsyncs);
        Ok::<_, Error>(FakeSource(Arc::clone(&display)))
    };
    let pacer = FramePacer::with_source(limit, source).unwrap();
    (pacer, display, Calls::default())
}

/// A waiter, `id`, that records its calls in `calls`.
fn waiter(id: u32, calls: &Calls) -> Arc<dyn RefreshWaiter> {
    let calls = Arc::clone(calls);
    Arc::new(move |time| calls.lock().push((id, time)))
}

fn waiting(pacer: &FramePacer) -> usize {
    pacer.state.lock().waiting.len()
}

fn ids(calls: &Calls) -> Vec<u32> {
    calls.lock().iter().map(|&(id, _)| id).collect()
}

#[test]
fn requests_share_one_refresh_until_it_comes() {
    let (pacer, display, calls) = fake_pacer(None);
    let (one, two) = (waiter(1, &calls), waiter(2, &calls));
    pacer.request(&one);
    pacer.request(&two);
    pacer.request(&one);
    assert_eq!((display.scheduled(), waiting(&pacer)), (1, 2));
    display.refresh(Duration::from_secs(1));
    assert_eq!(*calls.lock(), [(1, Duration::from_secs(1)), (2, Duration::from_secs(1))]);
    // Nothing asked for another.
    assert_eq!(display.scheduled(), 0);
    pacer.request(&waiter(3, &calls));
    assert_eq!(display.scheduled(), 1);
}

#[test]
fn a_request_from_the_callback_waits_for_the_next_refresh() {
    let (pacer, display, calls) = fake_pacer(Limit::new(30));
    let second = waiter(2, &calls);
    let first: Arc<dyn RefreshWaiter> = Arc::new({
        let (pacer, second, calls) = (Arc::downgrade(&pacer), Arc::clone(&second), Arc::clone(&calls));
        move |time| {
            calls.lock().push((1, time));
            if let Some(pacer) = pacer.upgrade() {
                pacer.request(&second);
            }
        }
    });
    let at = Duration::from_millis;
    pacer.request(&first);
    display.refresh(at(1000));
    assert_eq!((display.scheduled(), waiting(&pacer)), (1, 1));
    // The limit skips the next refresh, and takes the one after.
    display.refresh(at(1016));
    display.refresh(at(1033));
    assert_eq!(*calls.lock(), [(1, at(1000)), (2, at(1033))]);
    assert_eq!(display.scheduled(), 0);
}

#[test]
fn a_refresh_that_the_limit_skips_schedules_the_next() {
    let (pacer, display, calls) = fake_pacer(Limit::new(30));
    let one = waiter(1, &calls);
    let at = Duration::from_millis;
    pacer.request(&one);
    display.refresh(at(1000));
    pacer.request(&one);
    display.refresh(at(1016));
    assert_eq!(calls.lock().len(), 1);
    assert_eq!(display.scheduled(), 1);
    display.refresh(at(1033));
    assert_eq!(ids(&calls), [1, 1]);
    assert_eq!(display.scheduled(), 0);
}

#[test]
fn a_new_limit_reaches_the_source_and_the_refresh_on_its_way() {
    let (pacer, display, calls) = fake_pacer(None);
    let one = waiter(1, &calls);
    let at = Duration::from_millis;
    pacer.request(&one);
    display.refresh(at(1000));
    pacer.request(&one);
    pacer.set_limit(Limit::new(10));
    assert_eq!(pacer.limit(), Limit::new(10));
    assert_eq!(*display.limits.lock(), [Limit::new(10)]);
    display.refresh(at(1016));
    assert_eq!(calls.lock().len(), 1);
    display.refresh(at(1100));
    assert_eq!(calls.lock().len(), 2);
}

#[test]
fn a_dropped_pacer_drops_its_source_and_ignores_its_refreshes() {
    let (pacer, display, calls) = fake_pacer(None);
    pacer.request(&waiter(1, &calls));
    drop(pacer);
    assert!(*display.dropped.lock());
    display.refresh(Duration::from_secs(1));
    assert!(calls.lock().is_empty());
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "android",
    target_env = "ohos"
)))]
#[test]
fn a_platform_without_a_display_source_makes_no_pacer() {
    let error = FramePacer::new(None).err().unwrap();
    assert_eq!(error.kind, mln_style::error::ErrorKind::Unsupported);
}

/// The platform's display link calls back at its refreshes, on macOS from a
/// pacer made off the main thread, as the tests run.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
#[test]
fn a_display_link_calls_back_at_refreshes() {
    for limit in [None, Limit::new(30)] {
        let pacer = FramePacer::new(limit).unwrap();
        let (sender, refreshes) = std::sync::mpsc::channel();
        let waiter: Arc<dyn RefreshWaiter> = Arc::new(move |time| {
            let _ = sender.send(time);
        });
        let mut last = Duration::ZERO;
        for _ in 0..3 {
            pacer.request(&waiter);
            let time = refreshes.recv_timeout(Duration::from_secs(10)).expect("the display link called back");
            assert!(time > last);
            last = time;
        }
    }
}

/// A pacer for a view must be made on the main thread, which the tests
/// don't run on.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
#[test]
#[expect(unsafe_code, reason = "the view is a pointer to an Objective-C object")]
fn a_pacer_for_a_view_needs_the_main_thread() {
    let view = core::ptr::NonNull::<u8>::dangling().cast();
    // SAFETY: the pacer checks the thread before it uses the view.
    let error = unsafe { FramePacer::for_view(view, None) }.err().unwrap();
    assert_eq!(error.kind, mln_style::error::ErrorKind::InvalidState);
}
