// Render sessions that the host's thread drives: the driver work, its wake,
// and the session's lifecycle without a thread of its own. They follow
// maplibre-native-ffi's ABI tests (tests/native/abi/render/driver.c at
// eae96a42c) where those test the same rules for a caller driver.

mod support;

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use mln_map::MapMode;
use mln_runtime::{
    AttachOptions, BackendType, Completion, DemandMode, Device, DeviceOptions, Driver, DriverWork, ErrorKind,
    FrameDemand, FrameDisposition, FrameResult, LogicalExtent, Map, MapOptions, RenderSession, RenderSessionState,
    RenderTarget, Runtime, Wake,
};
use mln_util::size::Size;
use support::{TIMEOUT, eventually, runtime, wait};

const SIZE: Size = Size { width: 64, height: 64 };

fn style(color: &str) -> String {
    format!(
        r#"{{"version":8,"transition":{{"duration":0,"delay":0}},"sources":{{}},"layers":[{{"id":"bg","type":"background","paint":{{"background-color":"{color}"}}}}]}}"#
    )
}

/// A continuous map with a style of one color.
fn map_of(runtime: &Runtime, color: &str) -> Map {
    let options = MapOptions {
        map: mln_map::MapOptions { size: Size::new(256, 256), map_mode: MapMode::Continuous, ..Default::default() },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    created.wait().unwrap();
    wait(map.set_style_json(style(color)));
    map
}

fn offscreen() -> RenderTarget {
    RenderTarget::Offscreen { size: SIZE, device: None }
}

fn caller_options() -> AttachOptions {
    AttachOptions { driver: Driver::CallerThread, ..AttachOptions::default() }
}

/// A wake that counts its calls.
fn counter() -> (Wake, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let counted = count.clone();
    (
        Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }),
        count,
    )
}

/// Services `driver` until `done` holds.
fn service_until(driver: &DriverWork, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    while !done() {
        assert!(std::time::Instant::now() < deadline, "the driver never got there");
        driver.service().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Services `driver` until `completion` completes, and returns its result.
fn serviced<T: Send + 'static>(driver: &DriverWork, completion: Completion<T>) -> Result<T, mln_runtime::Error> {
    service_until(driver, || completion.is_complete());
    completion.wait()
}

/// A caller-driven offscreen session that has attached, and its driver work.
fn attach(map: &Map, options: AttachOptions) -> (RenderSession, DriverWork) {
    let (session, attached) = map.attach(offscreen(), options).unwrap();
    let driver = session.take_driver_work().unwrap();
    serviced(&driver, attached).unwrap();
    (session, driver)
}

/// Services `driver` until the session has a frame result, and takes it.
fn take_one(driver: &DriverWork, session: &RenderSession) -> FrameResult {
    let mut results = Vec::new();
    service_until(driver, || {
        results.extend(session.drain_frame_results().unwrap());
        !results.is_empty()
    });
    assert_eq!(1, results.len(), "{results:?}");
    results[0]
}

fn center_pixel(driver: &DriverWork, session: &RenderSession) -> [u8; 4] {
    let image = serviced(driver, session.read_texture().unwrap()).unwrap();
    let offset = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

#[test]
fn a_demand_made_while_attaching_fails_when_the_map_cant_bind_the_session() {
    let runtime = runtime();
    let options = MapOptions {
        map: mln_map::MapOptions { size: Size::new(256, 256), ..Default::default() },
        fallback_font_faces: Some("not font faces".to_owned()),
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    assert!(created.wait().is_err());
    let (session, attached) = map.attach(offscreen(), caller_options()).unwrap();
    let driver = session.take_driver_work().unwrap();
    // The runtime refused the bind before it ran this command.
    wait(runtime.clear_event_handler());
    session.demand_frame(FrameDemand::new(DemandMode::Force, 3)).unwrap();
    assert!(serviced(&driver, attached).is_err());
    let result = take_one(&driver, &session);
    assert_eq!((3, FrameDisposition::Failed), (result.token.unwrap(), result.disposition));
}

#[test]
fn a_caller_driven_session_works_only_when_serviced() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (wake, wakes) = counter();
    let (session, attached) =
        map.attach(offscreen(), AttachOptions { driver_work_wake: Some(wake), ..caller_options() }).unwrap();
    assert_eq!(Driver::CallerThread, session.capabilities().unwrap().driver);
    // The attach waits for the host's thread, and woke it before it returned.
    assert_eq!(1, wakes.load(Ordering::SeqCst));
    assert_eq!(RenderSessionState::Attaching, session.snapshot().unwrap().state);
    assert!(!attached.is_complete());

    let driver = session.take_driver_work().unwrap();
    serviced(&driver, attached).unwrap();
    assert_eq!(RenderSessionState::Attached, session.snapshot().unwrap().state);

    session.demand_frame(FrameDemand::new(DemandMode::Force, 3)).unwrap();
    let result = take_one(&driver, &session);
    assert_eq!((3, FrameDisposition::Rendered), (result.token.unwrap(), result.disposition));
    assert_eq!([255, 0, 0, 255], center_pixel(&driver, &session));

    wait_release(&driver, &session);
}

/// Releases a session, servicing it until the release completes.
fn wait_release(driver: &DriverWork, session: &RenderSession) {
    serviced(driver, session.release().unwrap()).unwrap();
}

#[test]
fn a_wait_on_the_drivers_thread_fails_instead_of_hanging() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (session, attached) = map.attach(offscreen(), caller_options()).unwrap();
    let driver = session.take_driver_work().unwrap();
    assert_eq!(ErrorKind::InvalidState, attached.wait().unwrap_err().kind);
    // The attach still completes in a service.
    service_until(&driver, || session.snapshot().unwrap().state == RenderSessionState::Attached);
    wait_release(&driver, &session);
}

#[test]
fn driver_work_is_taken_once_and_only_from_a_caller_driven_session() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (worker_session, attached) = map.attach(offscreen(), AttachOptions::default()).unwrap();
    attached.wait().unwrap();
    assert_eq!(ErrorKind::Unsupported, worker_session.take_driver_work().unwrap_err().kind);
    wait(worker_session.release());

    let (session, driver) = attach(&map, caller_options());
    assert_eq!(ErrorKind::InvalidState, session.take_driver_work().unwrap_err().kind);
    // Another thread can't take it either.
    let other = session.clone();
    let taken = std::thread::spawn(move || other.take_driver_work().map(drop).unwrap_err().kind);
    assert_eq!(ErrorKind::InvalidState, taken.join().unwrap());
    wait_release(&driver, &session);
}

/// Services until a service leaves no wake behind, since work that comes in
/// during a service wakes the host again, then clears the count.
fn settle(driver: &DriverWork, wakes: &AtomicUsize) {
    service_until(driver, || {
        wakes.store(0, Ordering::SeqCst);
        driver.service().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        wakes.load(Ordering::SeqCst) == 0
    });
}

#[test]
fn the_driver_wake_runs_once_until_a_service() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (wake, wakes) = counter();
    let (session, driver) = attach(&map, AttachOptions { driver_work_wake: Some(wake), ..caller_options() });
    settle(&driver, &wakes);

    for token in 0..3 {
        session.demand_frame(FrameDemand::new(DemandMode::Force, token)).unwrap();
    }
    assert_eq!(1, wakes.load(Ordering::SeqCst));
    let mut results = Vec::new();
    service_until(&driver, || {
        results.extend(session.drain_frame_results().unwrap());
        results.len() == 3
    });
    settle(&driver, &wakes);
    session.demand_frame(FrameDemand::new(DemandMode::Force, 4)).unwrap();
    assert_eq!(1, wakes.load(Ordering::SeqCst));
    assert_eq!(4, take_one(&driver, &session).token.unwrap());
    wait_release(&driver, &session);
}

thread_local! {
    /// The driver work of the session whose wakes service it again.
    static REENTRANT: RefCell<Option<DriverWork>> = const { RefCell::new(None) };
}

#[test]
fn a_service_inside_a_service_is_busy() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let busy = Arc::new(AtomicUsize::new(0));
    let counted = busy.clone();
    let host = std::thread::current().id();
    // The frame wake runs inside the service that publishes the result.
    let frame_wake: Wake = Arc::new(move || {
        if std::thread::current().id() == host {
            let status = REENTRANT.with(|driver| driver.borrow().as_ref().map(DriverWork::service));
            if let Some(Err(error)) = status {
                assert_eq!(ErrorKind::Busy, error.kind);
                counted.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    let (session, driver) = attach(&map, AttachOptions { frame_wake: Some(frame_wake), ..caller_options() });
    REENTRANT.with(|slot| *slot.borrow_mut() = Some(driver));
    session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while busy.load(Ordering::SeqCst) == 0 {
        assert!(std::time::Instant::now() < deadline);
        REENTRANT.with(|driver| driver.borrow().as_ref().map(DriverWork::service)).unwrap().unwrap();
    }
    assert_eq!(1, session.drain_frame_results().unwrap().len());
    let driver = REENTRANT.with(|slot| slot.borrow_mut().take()).unwrap();
    wait_release(&driver, &session);
}

#[test]
fn a_released_session_detaches_at_its_next_service() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (session, driver) = attach(&map, caller_options());
    session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
    take_one(&driver, &session);

    let released = session.release().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(!released.is_complete(), "the session retired before its detach was serviced");
    assert_eq!(ErrorKind::InvalidState, session.snapshot().unwrap_err().kind);
    serviced(&driver, released).unwrap();
    assert!(driver.is_finished());
    assert_eq!(ErrorKind::InvalidState, driver.service().unwrap_err().kind);

    let (other, attached) = map.attach(offscreen(), AttachOptions::default()).unwrap();
    attached.wait().unwrap();
    wait(other.release());
}

#[test]
fn a_session_released_while_attaching_lets_go_of_the_map_without_a_service() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (session, attached) = map.attach(offscreen(), caller_options()).unwrap();
    wait(session.release());
    assert_eq!(ErrorKind::TargetLost, attached.wait().unwrap_err().kind);

    let (other, attached) = map.attach(offscreen(), AttachOptions::default()).unwrap();
    attached.wait().unwrap();
    wait(other.release());
    // The driver work that no one took is still there to run the end of the
    // session, which fails once it ran.
    let driver = session.take_driver_work().unwrap();
    assert_eq!(ErrorKind::InvalidState, driver.service().unwrap_err().kind);
    assert!(driver.is_finished());
}

#[test]
fn an_abandon_completes_pending_work_at_once() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (session, driver) = attach(&map, caller_options());
    let pending = session.reduce_memory_use().unwrap();
    session.demand_frame(FrameDemand::new(DemandMode::Force, 6)).unwrap();
    // On the driver's thread, the session keeps none of its graphics objects.
    assert_eq!(0, session.abandon().unwrap());
    assert!(pending.is_complete());
    assert_eq!(ErrorKind::TargetLost, pending.wait().unwrap_err().kind);
    assert_eq!(RenderSessionState::Abandoned, session.snapshot().unwrap().state);

    // The map is free at once, and the next service lets go of the graphics
    // objects, then reports that the session ended.
    let (other, attached) = map.attach(offscreen(), AttachOptions::default()).unwrap();
    attached.wait().unwrap();
    assert_eq!(ErrorKind::InvalidState, driver.service().unwrap_err().kind);
    assert!(driver.is_finished());
    let results = session.drain_frame_results().unwrap();
    assert_eq!(1, results.len(), "{results:?}");
    assert_eq!((6, FrameDisposition::TargetNotReady), (results[0].token.unwrap(), results[0].disposition));
    wait(session.release());
    wait(other.release());
}

/// A release of a session that waits for its first service abandons it at
/// once, and completes the work on its way there, on the releasing thread,
/// inside the release's `send_then`: a `send_then` from one of those
/// callbacks runs at once instead of waiting for the release's.
#[test]
fn a_send_from_a_completion_that_a_release_settles_runs_at_once() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (session, attached) = map.attach(offscreen(), caller_options()).unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let inner = session.clone();
    session
        .send_then(RenderSession::barrier, move |result| {
            let resent = inner.send_then(RenderSession::barrier, |_| panic!("the callback ran"));
            sent.send((result, resent)).unwrap();
        })
        .unwrap();
    session.send_then(RenderSession::release, |result| result.unwrap()).unwrap();
    let (result, resent) = received.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(ErrorKind::TargetLost, result.unwrap_err().kind);
    assert_eq!(ErrorKind::InvalidState, resent.unwrap_err().kind);
    assert_eq!(ErrorKind::TargetLost, attached.wait().unwrap_err().kind);
}

#[test]
fn a_map_release_abandons_a_released_session_that_waits_for_a_service() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (session, driver) = attach(&map, caller_options());
    assert_eq!(ErrorKind::InvalidState, map.release().unwrap_err().kind);

    let released = session.release().unwrap();
    let map_released = map.release().unwrap();
    released.wait().unwrap();
    map_released.wait().unwrap();
    assert_eq!(ErrorKind::InvalidState, driver.service().unwrap_err().kind);
}

/// A library device of `backend`, or `None` where this build or machine has
/// none.
fn device_of(backend: BackendType) -> Option<Device> {
    match Device::with_backend(backend, DeviceOptions::default()) {
        Ok(device) => Some(device),
        Err(error) if error.kind == ErrorKind::Unsupported => {
            eprintln!("skipped: no {backend:?} device: {error}");
            None
        }
        Err(error) => panic!("a {backend:?} device: {error}"),
    }
}

#[test]
fn an_abandon_on_the_drivers_thread_keeps_only_open_gl_objects() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    // What an abandon keeps before the first frame and after it: an OpenGL
    // session's target, and its renderer's objects once it drew.
    for (backend, kept) in [(BackendType::Vulkan, [0, 0]), (BackendType::OpenGL, [1, 2])] {
        let Some(device) = device_of(backend) else { continue };
        for (rendered, kept) in kept.into_iter().enumerate() {
            let target = RenderTarget::Offscreen { size: SIZE, device: Some(device.clone()) };
            let (session, attached) = map.attach(target, caller_options()).unwrap();
            let driver = session.take_driver_work().unwrap();
            serviced(&driver, attached).unwrap();
            if rendered == 1 {
                session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
                take_one(&driver, &session);
            }
            assert_eq!(kept, session.abandon().unwrap(), "{backend:?}");
            assert_eq!(ErrorKind::InvalidState, driver.service().unwrap_err().kind);
            wait(session.release());
        }
    }
}

#[test]
fn a_session_released_after_its_map_is_abandoned_without_a_service() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (session, driver) = attach(&map, caller_options());
    // The disposed map waits for the session, which a release then
    // abandons rather than detach at a service that may never come.
    let map_released = map.dispose().unwrap();
    session.release().unwrap().wait().unwrap();
    map_released.wait().unwrap();
    assert_eq!(ErrorKind::InvalidState, driver.service().unwrap_err().kind);
}

#[test]
fn a_map_command_that_no_work_waits_for_wakes_no_driver() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (wake, wakes) = counter();
    let (session, driver) = attach(&map, AttachOptions { driver_work_wake: Some(wake), ..caller_options() });
    settle(&driver, &wakes);
    wait(map.style_url());
    assert_eq!(0, wakes.load(Ordering::SeqCst));
    // A demand waits for the commands sent before it, and wakes the driver.
    session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
    assert_eq!(1, wakes.load(Ordering::SeqCst));
    assert_eq!(FrameDisposition::Rendered, take_one(&driver, &session).disposition);
    wait_release(&driver, &session);
}

#[test]
fn a_caller_driven_session_asks_for_a_redraw_before_a_service() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (wake, redraws) = counter();
    let (session, driver) = attach(&map, AttachOptions { redraw_wake: Some(wake), ..caller_options() });
    session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
    take_one(&driver, &session);
    let before = redraws.load(Ordering::SeqCst);
    // The host hears of the change from the map's thread, and services the
    // session only once it demands the frame.
    wait(map.set_style_json(style("#0000ff")));
    assert!(eventually(|| redraws.load(Ordering::SeqCst) == before + 1));
    session.demand_frame(FrameDemand::new(DemandMode::IfChanged, 2)).unwrap();
    assert_eq!(FrameDisposition::Rendered, take_one(&driver, &session).disposition);
    wait_release(&driver, &session);
}

#[test]
fn dropping_the_driver_work_abandons_the_session() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (session, driver) = attach(&map, caller_options());
    let pending = session.reduce_memory_use().unwrap();
    drop(driver);
    assert_eq!(RenderSessionState::Abandoned, session.snapshot().unwrap().state);
    assert_eq!(ErrorKind::TargetLost, pending.wait().unwrap_err().kind);
    let (other, attached) = map.attach(offscreen(), AttachOptions::default()).unwrap();
    attached.wait().unwrap();
    wait(other.release());
    wait(session.release());
}

#[test]
fn one_thread_drives_the_sessions_of_two_maps() {
    let runtime = runtime();
    let red = map_of(&runtime, "#ff0000");
    let blue = map_of(&runtime, "#0000ff");
    let (red_session, red_driver) = attach(&red, caller_options());
    let (blue_session, blue_driver) = attach(&blue, caller_options());
    for (session, token) in [(&red_session, 1), (&blue_session, 2)] {
        session.demand_frame(FrameDemand::new(DemandMode::Force, token)).unwrap();
    }
    let mut red_results = Vec::new();
    let mut blue_results = Vec::new();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while red_results.is_empty() || blue_results.is_empty() {
        assert!(std::time::Instant::now() < deadline);
        red_driver.service().unwrap();
        blue_driver.service().unwrap();
        red_results.extend(red_session.drain_frame_results().unwrap());
        blue_results.extend(blue_session.drain_frame_results().unwrap());
    }
    assert_eq!([255, 0, 0, 255], center_pixel(&red_driver, &red_session));
    assert_eq!([0, 0, 255, 255], center_pixel(&blue_driver, &blue_session));
    wait_release(&red_driver, &red_session);
    wait_release(&blue_driver, &blue_session);
}

#[test]
fn a_caller_driven_session_resizes_in_order_with_frames() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (session, driver) = attach(&map, caller_options());
    let resized = session.resize(LogicalExtent::new(Size::new(32, 48), 1.0)).unwrap();
    session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
    let applied = serviced(&driver, resized).unwrap();
    let result = take_one(&driver, &session);
    assert_eq!(FrameDisposition::Rendered, result.disposition);
    assert!(result.map_update_generation >= applied.render_update_generation);
    assert_eq!(Size::new(32, 48), result.view.unwrap().extent.size);
    assert_eq!(2, session.snapshot().unwrap().generation);
    wait_release(&driver, &session);
}

#[test]
fn the_driver_wake_runs_with_the_session_unlocked() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let cell = Arc::new(OnceLock::<RenderSession>::new());
    let reads = Arc::new(AtomicUsize::new(0));
    let (reader, counted) = (cell.clone(), reads.clone());
    // The wake reads the session's state, which a lock held across the wake
    // would deadlock.
    let wake: Wake = Arc::new(move || {
        if let Some(session) = reader.get() {
            session.snapshot().unwrap();
            session.capabilities().unwrap();
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });
    let (session, driver) = attach(&map, AttachOptions { driver_work_wake: Some(wake), ..caller_options() });
    cell.set(session.clone()).unwrap();
    settle(&driver, &reads);
    session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
    assert_eq!(1, reads.load(Ordering::SeqCst));
    take_one(&driver, &session);
    settle(&driver, &reads);
    let detached = session.detach().unwrap();
    assert_eq!(1, reads.load(Ordering::SeqCst));
    serviced(&driver, detached).unwrap();
    wait_release(&driver, &session);
}

#[test]
fn work_sent_inside_a_service_leaves_the_wake_for_later_work() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (wake, wakes) = counter();
    let cell = Arc::new(OnceLock::<RenderSession>::new());
    let reader = cell.clone();
    let demanded = Arc::new(AtomicBool::new(false));
    let once = demanded.clone();
    // The frame wake runs inside the service, and demands another frame
    // there, once.
    let frame_wake: Wake = Arc::new(move || {
        if let Some(session) = reader.get()
            && !once.swap(true, Ordering::SeqCst)
        {
            session.demand_frame(FrameDemand::new(DemandMode::Force, 2)).unwrap();
        }
    });
    let options = AttachOptions { driver_work_wake: Some(wake), frame_wake: Some(frame_wake), ..caller_options() };
    let (session, driver) = attach(&map, options);
    cell.set(session.clone()).unwrap();
    settle(&driver, &wakes);
    session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
    wakes.store(0, Ordering::SeqCst);
    driver.service().unwrap();
    assert!(demanded.load(Ordering::SeqCst));
    let results = session.drain_frame_results().unwrap();
    let tokens: Vec<_> = results.iter().map(|result| result.token.unwrap()).collect();
    assert_eq!(vec![1, 2], tokens);
    // The demand that the service ran took the wake, which the host gets
    // again for work that comes after the service.
    session.demand_frame(FrameDemand::new(DemandMode::Force, 3)).unwrap();
    assert!(wakes.load(Ordering::SeqCst) >= 2, "no wake for work after the service");
    assert_eq!(3, take_one(&driver, &session).token.unwrap());
    wait_release(&driver, &session);
}

#[test]
fn an_abandon_from_a_completion_inside_a_service_is_busy() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (session, driver) = attach(&map, caller_options());
    let (sender, receiver) = std::sync::mpsc::channel();
    let abandoned = session.clone();
    session.barrier().unwrap().on_complete(move |result| {
        let _ = sender.send((result, abandoned.abandon().map_err(|error| error.kind)));
    });
    let mut received = None;
    service_until(&driver, || {
        received = received.take().or_else(|| receiver.try_recv().ok());
        received.is_some()
    });
    let (result, abandon) = received.unwrap();
    result.unwrap();
    assert_eq!(Err(ErrorKind::Busy), abandon);
    assert_eq!(RenderSessionState::Attached, session.snapshot().unwrap().state);
    wait_release(&driver, &session);
}

#[test]
fn an_abandon_while_another_thread_services_is_busy() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let cell = Arc::new(OnceLock::<RenderSession>::new());
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = cell.clone();
    let host = std::thread::current().id();
    // The frame wake runs inside the host's service, and has another thread
    // abandon the session meanwhile.
    let frame_wake: Wake = Arc::new(move || {
        if std::thread::current().id() == host
            && let Some(session) = reader.get().cloned()
        {
            let abandon = std::thread::spawn(move || session.abandon().map_err(|error| error.kind));
            let _ = sender.send(abandon.join().unwrap());
        }
    });
    let (session, driver) = attach(&map, AttachOptions { frame_wake: Some(frame_wake), ..caller_options() });
    cell.set(session.clone()).unwrap();
    session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
    take_one(&driver, &session);
    assert_eq!(Err(ErrorKind::Busy), receiver.try_recv().unwrap());
    assert_eq!(RenderSessionState::Attached, session.snapshot().unwrap().state);
    wait_release(&driver, &session);
}

#[test]
fn an_abandon_off_the_drivers_thread_keeps_the_graphics_objects() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let (session, driver) = attach(&map, caller_options());
    session.demand_frame(FrameDemand::new(DemandMode::Force, 1)).unwrap();
    take_one(&driver, &session);
    let other = session.clone();
    // Only the driver's thread may free them, and the host may destroy what
    // they were made on once the abandon returns.
    let kept = std::thread::spawn(move || other.abandon()).join().unwrap().unwrap();
    assert_eq!(2, kept);
    assert_eq!(RenderSessionState::Abandoned, session.snapshot().unwrap().state);
    assert_eq!(ErrorKind::InvalidState, driver.service().unwrap_err().kind);
    assert!(driver.is_finished());
    wait(session.release());
}

#[test]
fn a_caller_driven_session_renders_the_tiles_that_its_workers_parse() {
    let runtime = runtime();
    let map = map_of(&runtime, "#ff0000");
    let style = r##"{"version":8,"sources":{"points":{"type":"geojson","data":{"type":"Feature","properties":{},
        "geometry":{"type":"Point","coordinates":[0,0]}}}},"layers":[{"id":"bg","type":"background",
        "paint":{"background-color":"#ff0000"}},{"id":"circle","type":"circle","source":"points",
        "paint":{"circle-radius":20,"circle-color":"#0000ff"}}]}"##;
    wait(map.set_style_json(style));
    let (session, driver) = attach(&map, caller_options());
    // The first frame finds the tile still parsing, and a frame after it
    // has parsed draws the circle.
    let deadline = std::time::Instant::now() + TIMEOUT;
    let mut token = 0;
    while center_pixel_after(&driver, &session, &mut token) != [0, 0, 255, 255] {
        assert!(std::time::Instant::now() < deadline, "the tile never rendered");
    }
    wait_release(&driver, &session);
}

/// Renders a frame, forced the first time and then one that waits for the
/// map's next update, and reads its center pixel.
fn center_pixel_after(driver: &DriverWork, session: &RenderSession, token: &mut u64) -> [u8; 4] {
    *token += 1;
    let mode = if *token == 1 { DemandMode::Force } else { DemandMode::WaitForUpdate };
    session.demand_frame(FrameDemand::new(mode, *token)).unwrap();
    take_one(driver, session);
    center_pixel(driver, session)
}
