// Render sessions that a frame pacer demands frames of: a fake display that
// the tests refresh, and the platform's display where it has a pacer.

mod support;

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use mln_frame_pacer::{FramePacer, Limit, VsyncSource, Vsyncs};
use mln_runtime::{
    AttachOptions, DemandMode, ErrorKind, FrameDemand, FrameDisposition, FrameResult, Map, RenderSession, RenderTarget,
};
use mln_style::size::Size;
use parking_lot::{Condvar, Mutex};
use support::{TIMEOUT, map, result_of, runtime, wait};

const SIZE: Size = Size { width: 16, height: 16 };

fn style(color: &str) -> String {
    format!(
        r#"{{"version":8,"transition":{{"duration":0,"delay":0}},"sources":{{}},"layers":[{{"id":"bg","type":"background","paint":{{"background-color":"{color}"}}}}]}}"#
    )
}

/// Gives the map a background of `color`, and returns the map update
/// generation of the frames that show it.
fn paint(map: &Map, color: &str) -> u64 {
    wait(map.set_style_json(style(color))).render_update_generation
}

// MARK: - A fake display

/// A display that the test refreshes.
#[derive(Default)]
struct FakeDisplay {
    vsyncs: OnceLock<Vsyncs>,
    /// How many refreshes the pacer scheduled that the test hasn't sent.
    scheduled: Mutex<u32>,
    changed: Condvar,
    dropped: Mutex<bool>,
}

struct FakeSource(Arc<FakeDisplay>);

impl VsyncSource for FakeSource {
    fn schedule(&self) {
        *self.0.scheduled.lock() += 1;
        self.0.changed.notify_all();
    }
}

impl Drop for FakeSource {
    fn drop(&mut self) {
        *self.0.dropped.lock() = true;
    }
}

impl FakeDisplay {
    /// A pacer on the display.
    fn pacer(self: &Arc<Self>, limit: Option<Limit>) -> Arc<FramePacer> {
        FramePacer::with_source(limit, |vsyncs, _| {
            let _ = self.vsyncs.set(vsyncs);
            Ok::<_, mln_style::error::Error>(FakeSource(Arc::clone(self)))
        })
        .unwrap()
    }

    /// Sends a refresh now when the pacer scheduled one, and says whether it
    /// did.
    fn refresh(&self) -> bool {
        {
            let mut scheduled = self.scheduled.lock();
            if *scheduled == 0 {
                return false;
            }
            *scheduled -= 1;
        }
        self.vsyncs.get().unwrap().vsync(now());
        true
    }

    /// Waits until the pacer schedules a refresh.
    fn wait_until_scheduled(&self) {
        let deadline = Instant::now() + TIMEOUT;
        let mut scheduled = self.scheduled.lock();
        while *scheduled == 0 {
            let left = deadline.checked_duration_since(Instant::now()).expect("the pacer scheduled a refresh");
            self.changed.wait_for(&mut scheduled, left);
        }
    }

    /// Waits until the pacer schedules a refresh, then sends it.
    fn refresh_when_scheduled(&self) {
        self.wait_until_scheduled();
        assert!(self.refresh());
    }

    fn scheduled(&self) -> u32 {
        *self.scheduled.lock()
    }
}

/// The time of a refresh now, on the fake refreshes' monotonic clock.
fn now() -> Duration {
    static START: OnceLock<Instant> = OnceLock::new();
    Duration::from_secs(1) + START.get_or_init(Instant::now).elapsed()
}

// MARK: - Sessions

/// An offscreen session without a redraw wake that has drawn its map, with
/// no frame result waiting, so that it has nothing new to draw.
fn settled_session(map: &Map) -> RenderSession {
    let (session, attached) =
        map.attach(RenderTarget::Offscreen { size: SIZE, device: None }, AttachOptions::default()).unwrap();
    result_of(attached).unwrap();
    // The redraw that the new session's map asks for comes before the
    // barrier, so the demand after answers it.
    wait(session.barrier());
    session.demand_frame(FrameDemand::new(DemandMode::Force, 9)).unwrap();
    wait(session.barrier());
    assert_eq!(tokens(&drain(&session)), [Some(9)]);
    session
}

fn drain(session: &RenderSession) -> Vec<FrameResult> {
    session.drain_frame_results().unwrap()
}

fn tokens(results: &[FrameResult]) -> Vec<Option<u64>> {
    results.iter().map(|result| result.token).collect()
}

/// Runs `poll` until `session` has a frame that the pacer demanded and that
/// shows the map update `generation`, skipping the pacer's frames before it.
fn poll_for_paced_frame(session: &RenderSession, generation: u64, mut poll: impl FnMut()) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        poll();
        for result in drain(session) {
            assert_eq!(result.token, Some(0), "only the pacer demands frames");
            if result.disposition == FrameDisposition::Rendered && result.map_update_generation >= generation {
                return;
            }
        }
        assert!(Instant::now() < deadline, "no paced frame showed update {generation}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_paced_session_draws_each_change_at_the_next_refresh() {
    let runtime = runtime();
    let map = map(&runtime);
    let display = Arc::new(FakeDisplay::default());
    let session = settled_session(&map);
    session.set_frame_pacer(Some(display.pacer(None))).unwrap();
    wait(session.barrier());
    assert_eq!(display.scheduled(), 0, "the session had nothing new to draw");

    for color in ["red", "blue"] {
        let generation = paint(&map, color);
        display.refresh_when_scheduled();
        wait(session.barrier());
        let results = drain(&session);
        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!((results[0].token, results[0].disposition), (Some(0), FrameDisposition::Rendered));
        assert!(results[0].map_update_generation >= generation);
    }
}

#[test]
fn a_new_pacer_draws_what_the_session_had_to_draw() {
    let runtime = runtime();
    let map = map(&runtime);
    let display = Arc::new(FakeDisplay::default());
    let session = settled_session(&map);
    let generation = paint(&map, "red");
    wait(session.barrier());
    session.set_frame_pacer(Some(display.pacer(None))).unwrap();
    assert_eq!(display.scheduled(), 1);
    assert!(display.refresh());
    poll_for_paced_frame(&session, generation, || {});
}

#[test]
fn a_demand_answers_what_the_session_had_to_draw() {
    let runtime = runtime();
    let map = map(&runtime);
    let display = Arc::new(FakeDisplay::default());
    let session = settled_session(&map);
    session.set_frame_pacer(Some(display.pacer(None))).unwrap();
    paint(&map, "red");
    display.wait_until_scheduled();
    // The host's demand answers the redraw, so the refresh demands nothing.
    session.demand_frame(FrameDemand::new(DemandMode::IfChanged, 5)).unwrap();
    assert!(display.refresh());
    wait(session.barrier());
    assert_eq!(tokens(&drain(&session)), [Some(5)]);
}

#[test]
fn a_refresh_that_an_earlier_pacer_asked_for_demands_nothing() {
    let runtime = runtime();
    let map = map(&runtime);
    let (first, second) = (Arc::new(FakeDisplay::default()), Arc::new(FakeDisplay::default()));
    let first_pacer = first.pacer(None);
    let session = settled_session(&map);
    session.set_frame_pacer(Some(Arc::clone(&first_pacer))).unwrap();
    let generation = paint(&map, "red");
    first.wait_until_scheduled();
    // The new pacer asks at once for what the session has to draw.
    session.set_frame_pacer(Some(second.pacer(None))).unwrap();
    assert_eq!(second.scheduled(), 1);
    assert!(first.refresh());
    wait(session.barrier());
    assert_eq!(tokens(&drain(&session)), []);
    assert!(second.refresh());
    poll_for_paced_frame(&session, generation, || {});
}

#[test]
fn one_pacer_serves_many_sessions_and_none_stops_one() {
    let runtime = runtime();
    let (stopped_map, paced_map) = (map(&runtime), map(&runtime));
    let display = Arc::new(FakeDisplay::default());
    let pacer = display.pacer(None);
    let (stopped, paced) = (settled_session(&stopped_map), settled_session(&paced_map));
    stopped.set_frame_pacer(Some(Arc::clone(&pacer))).unwrap();
    paced.set_frame_pacer(Some(Arc::clone(&pacer))).unwrap();
    let generations = [paint(&stopped_map, "red"), paint(&paced_map, "red")];
    wait(stopped.barrier());
    wait(paced.barrier());
    assert_eq!(display.scheduled(), 1);
    assert!(display.refresh());
    poll_for_paced_frame(&stopped, generations[0], || {});
    poll_for_paced_frame(&paced, generations[1], || {});

    stopped.set_frame_pacer(None).unwrap();
    wait(stopped.barrier());
    drain(&stopped);
    paint(&stopped_map, "blue");
    let generation = paint(&paced_map, "blue");
    // The first frames can leave a refresh scheduled, which goes out before
    // the paced session's next redraw reaches the pacer. That redraw then
    // schedules one of its own.
    poll_for_paced_frame(&paced, generation, || {
        display.refresh();
    });
    wait(stopped.barrier());
    assert_eq!(tokens(&drain(&stopped)), []);
}

#[test]
fn a_released_session_lets_go_of_its_pacer() {
    let runtime = runtime();
    let map = map(&runtime);
    let display = Arc::new(FakeDisplay::default());
    let session = settled_session(&map);
    session.set_frame_pacer(Some(display.pacer(None))).unwrap();
    assert!(!*display.dropped.lock());
    let copy = session.clone();
    wait(session.release());
    assert!(*display.dropped.lock());
    assert_eq!(copy.set_frame_pacer(None).unwrap_err().kind, ErrorKind::InvalidState);
}

#[test]
fn a_session_with_a_redraw_wake_takes_no_pacer() {
    let runtime = runtime();
    let map = map(&runtime);
    let options = AttachOptions { redraw_wake: Some(Arc::new(|| {})), ..AttachOptions::default() };
    let (session, attached) = map.attach(RenderTarget::Offscreen { size: SIZE, device: None }, options).unwrap();
    result_of(attached).unwrap();
    let pacer = Arc::new(FakeDisplay::default()).pacer(None);
    assert_eq!(session.set_frame_pacer(Some(pacer)).unwrap_err().kind, ErrorKind::InvalidState);
    assert_eq!(session.set_frame_pacer(None).unwrap_err().kind, ErrorKind::InvalidState);
}

/// The platform's display paces a session, where it has a pacer.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
#[test]
fn the_platforms_display_paces_a_session() {
    let runtime = runtime();
    let map = map(&runtime);
    let session = settled_session(&map);
    session.set_frame_pacer(Some(FramePacer::new(Limit::new(30)).unwrap())).unwrap();
    poll_for_paced_frame(&session, paint(&map, "red"), || {});
    poll_for_paced_frame(&session, paint(&map, "blue"), || {});
}
