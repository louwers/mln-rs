//! The throttle's grid, and the pacer's requests and limits with a fake
//! source that the tests refresh.

use mln_util::time::Instant;

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

type Calls = Arc<Mutex<Vec<Vec<u32>>>>;

/// A pacer on a fake display, and the waiters of each of its callbacks.
fn fake_pacer(limit: Option<Limit>) -> (Arc<FramePacer<u32>>, Arc<FakeDisplay>, Calls) {
    let display = Arc::new(FakeDisplay::default());
    let calls = Calls::default();
    let called = Arc::clone(&calls);
    let source = |vsyncs: Vsyncs, _: Option<Limit>| {
        let _ = display.vsyncs.set(vsyncs);
        Ok::<_, Error>(FakeSource(Arc::clone(&display)))
    };
    let pacer = FramePacer::with_source(limit, move |_, waiters| called.lock().push(waiters), source).unwrap();
    (pacer, display, calls)
}

#[test]
fn requests_share_one_refresh_until_it_comes() {
    let (pacer, display, calls) = fake_pacer(None);
    pacer.request(1);
    pacer.request(2);
    pacer.request(1);
    assert_eq!(display.scheduled(), 1);
    assert_eq!(pacer.waiting(), [1, 2]);
    display.refresh(Duration::from_secs(1));
    assert_eq!(*calls.lock(), [vec![1, 2]]);
    // Nothing asked for another.
    assert_eq!(display.scheduled(), 0);
    pacer.request(3);
    assert_eq!(display.scheduled(), 1);
}

#[test]
fn a_request_from_the_callback_waits_for_the_next_refresh() {
    let display = Arc::new(FakeDisplay::default());
    let calls = Calls::default();
    let source = |vsyncs: Vsyncs, _: Option<Limit>| {
        let _ = display.vsyncs.set(vsyncs);
        Ok::<_, Error>(FakeSource(Arc::clone(&display)))
    };
    let pacer: Arc<OnceLock<Weak<FramePacer<u32>>>> = Arc::default();
    let (callback_pacer, called) = (Arc::clone(&pacer), Arc::clone(&calls));
    let on_refresh = move |_, waiters: Vec<u32>| {
        if waiters == [1]
            && let Some(pacer) = callback_pacer.get().and_then(Weak::upgrade)
        {
            pacer.request(2);
        }
        called.lock().push(waiters);
    };
    let frames = FramePacer::with_source(Limit::new(30), on_refresh, source).unwrap();
    let _ = pacer.set(Arc::downgrade(&frames));
    let at = Duration::from_millis;
    frames.request(1);
    display.refresh(at(1000));
    assert_eq!((display.scheduled(), frames.waiting()), (1, vec![2]));
    // The limit skips the next refresh, and takes the one after.
    display.refresh(at(1016));
    display.refresh(at(1033));
    assert_eq!(*calls.lock(), [vec![1], vec![2]]);
    assert_eq!(display.scheduled(), 0);
}

#[test]
fn a_callback_gets_when_the_display_shows_its_refreshs_frame() {
    let display = Arc::new(FakeDisplay::default());
    let shown = Arc::new(Mutex::new(Vec::new()));
    let reported = Arc::clone(&shown);
    let source = |vsyncs: Vsyncs, _: Option<Limit>| {
        let _ = display.vsyncs.set(vsyncs);
        Ok::<_, Error>(FakeSource(Arc::clone(&display)))
    };
    let pacer =
        FramePacer::with_source(None, move |shown_at, _: Vec<u32>| reported.lock().push(shown_at), source).unwrap();
    let target = Instant::now() + Duration::from_millis(25);
    pacer.request(1);
    *display.scheduled.lock() -= 1;
    display.vsyncs.get().unwrap().vsync_shown_at(Duration::from_secs(1), target);
    pacer.request(1);
    display.refresh(Duration::from_secs(2));
    assert_eq!(*shown.lock(), [Some(target), None]);
}

#[test]
fn a_refresh_that_the_limit_skips_schedules_the_next() {
    let (pacer, display, calls) = fake_pacer(Limit::new(30));
    let at = Duration::from_millis;
    pacer.request(1);
    display.refresh(at(1000));
    pacer.request(1);
    display.refresh(at(1016));
    assert_eq!(calls.lock().len(), 1);
    assert_eq!(display.scheduled(), 1);
    display.refresh(at(1033));
    assert_eq!(calls.lock().len(), 2);
    assert_eq!(display.scheduled(), 0);
}

#[test]
fn a_new_limit_reaches_the_source_and_the_refresh_on_its_way() {
    let (pacer, display, calls) = fake_pacer(None);
    let at = Duration::from_millis;
    pacer.request(1);
    display.refresh(at(1000));
    pacer.request(1);
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
    pacer.request(1);
    drop(pacer);
    assert!(*display.dropped.lock());
    display.refresh(Duration::from_secs(1));
    assert!(calls.lock().is_empty());
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "tvos")))]
#[test]
fn a_platform_without_a_display_source_makes_no_pacer() {
    let error = FramePacer::<()>::new(None, |_, _| {}).err().unwrap();
    assert_eq!(error.kind, mln_util::ErrorKind::Unsupported);
}
