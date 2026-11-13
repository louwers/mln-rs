// Offscreen render sessions: frame demands and their results, the session's
// work, still images, queries and the session's lifecycle. They follow
// maplibre-native-ffi's ABI tests (tests/native/abi/render/{demands,driver,
// offscreen,queries}.c and map/still_image.c at eae96a42c) where those test
// the same rules.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;

use mln_map::{CameraOptions, CameraUpdate, MapEvent, MapMode, TileOperation};
use mln_runtime::{
    AttachOptions, DemandMode, Device, DeviceOptions, Driver, ErrorKind, FrameDemand, FrameDisposition, FrameResult,
    LogicalExtent, Map, MapOptions, RenderSession, RenderSessionState, RenderTarget, RenderedQueryGeometry,
    RenderedQueryOptions, ResourceRequest, Runtime, RuntimeEvent, RuntimeEventKind, SourceQueryOptions,
};
use mln_storage::resource::Kind;
use mln_storage::response::Reason;
use mln_storage::{ResourceError, Response};
use mln_util::chrono::Timestamp;
use mln_util::geo::{LatLng, ScreenCoordinate};
use mln_util::size::Size;
use parking_lot::Mutex;
use support::{EventLog, TIMEOUT, eventually, runtime, wait, wait_err};

const SIZE: Size = Size { width: 64, height: 64 };

fn style(color: &str) -> String {
    format!(
        r#"{{"version":8,"transition":{{"duration":0,"delay":0}},"sources":{{}},"layers":[{{"id":"bg","type":"background","paint":{{"background-color":"{color}"}}}}]}}"#
    )
}

fn map_in_mode(runtime: &Runtime, mode: MapMode) -> Map {
    let options = MapOptions {
        map: mln_map::MapOptions { size: Size::new(256, 256), map_mode: mode, ..Default::default() },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    created.wait().unwrap();
    map
}

/// A continuous map with a red style.
fn red_map(runtime: &Runtime) -> Map {
    let map = map_in_mode(runtime, MapMode::Continuous);
    wait(map.set_style_json(style("#ff0000")));
    map
}

fn offscreen(size: Size) -> RenderTarget {
    RenderTarget::Offscreen { size, device: None }
}

/// An offscreen session of `SIZE` that has attached.
fn attach(map: &Map) -> RenderSession {
    let (session, attached) = map.attach(offscreen(SIZE), AttachOptions::default()).unwrap();
    attached.wait().unwrap();
    session
}

fn demand(session: &RenderSession, mode: DemandMode, token: u64) {
    session.demand_frame(FrameDemand::new(mode, token)).unwrap();
}

/// Waits until the session has `count` results, and takes them.
fn take_results(session: &RenderSession, count: usize) -> Vec<FrameResult> {
    let mut results = Vec::new();
    let done = eventually(|| {
        results.extend(session.drain_frame_results().unwrap());
        results.len() >= count
    });
    assert!(done, "waited for {count} frame results, got {results:?}");
    results
}

fn take_one(session: &RenderSession) -> FrameResult {
    let results = take_results(session, 1);
    assert_eq!(1, results.len(), "{results:?}");
    results[0]
}

/// Renders until a frame shows the map settled: fully loaded, wanting no
/// further frame, and drawing the newest update. A later demand that doesn't
/// force a frame then finds nothing new.
fn render_until_idle(map: &Map, session: &RenderSession) {
    wait(map.camera());
    let mut mode = DemandMode::IfChanged;
    let deadline = std::time::Instant::now() + TIMEOUT;
    while std::time::Instant::now() < deadline {
        demand(session, mode, 0);
        let frame = take_one(session);
        match frame.disposition {
            FrameDisposition::Rendered => {
                let snapshot = map.snapshot().unwrap();
                if frame.mode == mln_map::RenderMode::Full
                    && !frame.needs_repaint
                    && !snapshot.transition_in_progress
                    && frame.map_update_generation >= snapshot.update_generation
                {
                    return;
                }
                mode = DemandMode::WaitForUpdate;
            }
            FrameDisposition::NoUpdate => mode = DemandMode::Force,
            disposition => panic!("a frame finished as {disposition:?}"),
        }
    }
    panic!("the map never settled");
}

/// Holds the runtime's thread until the returned sender sends or drops.
fn hold_runtime(map: &Map) -> std::sync::mpsc::Sender<()> {
    let (open, gate) = channel::<()>();
    let (entered, parked) = channel::<()>();
    drop(
        map.with_core(move |_| {
            let _ = entered.send(());
            let _ = gate.recv_timeout(TIMEOUT);
        })
        .unwrap(),
    );
    parked.recv_timeout(TIMEOUT).unwrap();
    open
}

fn zoom(map: &Map, zoom: f64) -> mln_runtime::Completion<mln_runtime::Applied> {
    map.update_camera(CameraUpdate {
        camera: CameraOptions { zoom: Some(zoom), ..Default::default() },
        ..Default::default()
    })
    .unwrap()
}

/// The premultiplied RGBA of the center pixel of the session's last frame.
fn center_pixel(session: &RenderSession, size: Size) -> [u8; 4] {
    let image = wait(session.read_texture());
    assert_eq!(size, image.size);
    let offset = ((size.height / 2 * size.width + size.width / 2) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

// MARK: - Frames

#[test]
fn a_forced_demand_renders_the_map() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    let capabilities = session.capabilities().unwrap();
    assert!(capabilities.readback);
    assert_eq!(Driver::CoreWorker, capabilities.driver);
    assert_eq!(RenderSessionState::Attached, session.snapshot().unwrap().state);

    demand(&session, DemandMode::Force, 7);
    let result = take_one(&session);
    assert_eq!(7, result.token.unwrap());
    assert_eq!(FrameDisposition::Rendered, result.disposition);
    assert_eq!(1, result.frame_generation);
    assert_eq!(SIZE, result.view.unwrap().extent.size);
    assert_eq!([255, 0, 0, 255], center_pixel(&session, SIZE));
    assert_eq!(1, session.snapshot().unwrap().frame_generation);
}

#[test]
fn frame_results_wake_the_host_once_until_it_drains() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = wakes.clone();
    let options = AttachOptions {
        frame_wake: Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })),
        ..AttachOptions::default()
    };
    let (session, attached) = map.attach(offscreen(SIZE), options).unwrap();
    attached.wait().unwrap();
    demand(&session, DemandMode::Force, 1);
    demand(&session, DemandMode::Force, 2);
    assert!(eventually(|| session.snapshot().unwrap().frame_generation == 2));
    assert_eq!(1, wakes.load(Ordering::SeqCst));
    let tokens: Vec<u64> = session.drain_frame_results().unwrap().iter().map(|result| result.token.unwrap()).collect();
    assert_eq!([1, 2], tokens.as_slice());
    assert_eq!(session.drain_frame_results().unwrap(), [] as [mln_runtime::FrameResult; 0]);
    demand(&session, DemandMode::Force, 3);
    assert_eq!(3, take_one(&session).token.unwrap());
    assert!(eventually(|| wakes.load(Ordering::SeqCst) == 2));
}

#[test]
fn a_demand_without_changes_renders_nothing() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    render_until_idle(&map, &session);
    demand(&session, DemandMode::IfChanged, 2);
    let result = take_one(&session);
    assert_eq!(FrameDisposition::NoUpdate, result.disposition);
    assert_eq!(0, result.frame_generation);
    assert_eq!(None, result.view);
}

// The runtime's thread is held while the first command is sent, so the
// session has an older update to draw when the demand reaches it, and the
// frame must show the command.
#[test]
fn a_demand_shows_the_commands_sent_before_it() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    render_until_idle(&map, &session);

    let open = hold_runtime(&map);
    let moved = zoom(&map, 3.0);
    demand(&session, DemandMode::IfChanged, 631);
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(session.drain_frame_results().unwrap(), [] as [mln_runtime::FrameResult; 0]);
    drop(open);

    let applied = moved.wait().unwrap();
    let result = take_one(&session);
    assert_eq!(631, result.token.unwrap());
    assert_eq!(FrameDisposition::Rendered, result.disposition);
    assert!(result.map_update_generation >= applied.render_update_generation);
    assert_eq!(3.0, result.view.unwrap().camera.zoom);
}

#[test]
fn a_demand_behind_a_command_without_an_update_finishes() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    render_until_idle(&map, &session);

    let open = hold_runtime(&map);
    let logged = map.dump_debug_logs().unwrap();
    demand(&session, DemandMode::IfChanged, 641);
    drop(open);
    logged.wait().unwrap();
    let result = take_one(&session);
    assert_eq!(641, result.token.unwrap());
    assert_eq!(FrameDisposition::NoUpdate, result.disposition);
}

#[test]
fn a_waiting_demand_renders_the_next_map_update() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    render_until_idle(&map, &session);
    let idle = map.snapshot().unwrap().update_generation;

    demand(&session, DemandMode::WaitForUpdate, 601);
    // Work that the session runs after the demand, and that publishes no
    // update.
    wait(session.dump_debug_logs());
    assert_eq!(session.drain_frame_results().unwrap(), [] as [mln_runtime::FrameResult; 0]);
    zoom(&map, 2.0).wait().unwrap();
    let result = take_one(&session);
    assert_eq!(601, result.token.unwrap());
    assert_eq!(FrameDisposition::Rendered, result.disposition);
    assert!(result.map_update_generation > idle);
}

#[test]
fn a_burst_of_queued_commands_renders_in_one_frame() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    render_until_idle(&map, &session);
    demand(&session, DemandMode::WaitForUpdate, 621);

    // The commands wait behind the held thread, and then run as one burst.
    let open = hold_runtime(&map);
    let zooms: Vec<_> = (1..=8).map(|step| zoom(&map, f64::from(step))).collect();
    drop(open);
    let applied: Vec<_> = zooms.into_iter().map(|zoom| zoom.wait().unwrap()).collect();
    let last = applied.last().unwrap();
    let result = take_one(&session);
    assert_eq!((621, FrameDisposition::Rendered), (result.token.unwrap(), result.disposition));
    assert_eq!(last.render_update_generation, result.map_update_generation);
    assert_eq!(map.snapshot().unwrap().update_generation, result.map_update_generation);
}

#[test]
fn a_waiting_demand_ends_by_supersession_barrier_or_detach() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    render_until_idle(&map, &session);

    demand(&session, DemandMode::WaitForUpdate, 611);
    demand(&session, DemandMode::WaitForUpdate, 612);
    wait(session.barrier());
    let results = take_results(&session, 2);
    assert_eq!((611, FrameDisposition::Superseded), (results[0].token.unwrap(), results[0].disposition));
    assert_eq!((612, FrameDisposition::NoUpdate), (results[1].token.unwrap(), results[1].disposition));

    demand(&session, DemandMode::WaitForUpdate, 613);
    wait(session.detach());
    let result = take_one(&session);
    assert_eq!((613, FrameDisposition::TargetNotReady), (result.token.unwrap(), result.disposition));
}

#[test]
fn a_detach_ends_a_demand_waiting_for_commands() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    render_until_idle(&map, &session);

    let open = hold_runtime(&map);
    let moved = zoom(&map, 3.0);
    demand(&session, DemandMode::Force, 661);
    let detached = session.detach().unwrap();
    // The detach completes while the map's thread stays held, before the
    // command sent ahead of it runs.
    assert!(eventually(|| detached.is_complete()));
    assert!(!moved.is_complete());
    detached.wait().unwrap();
    let result = take_one(&session);
    assert_eq!((661, FrameDisposition::TargetNotReady), (result.token.unwrap(), result.disposition));
    drop(open);
    moved.wait().unwrap();
}

#[test]
fn a_frame_leaves_out_what_an_open_command_group_holds() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    render_until_idle(&map, &session);

    let group = map.command_group().unwrap();
    wait(map.set_style_layer_property("bg", "background-color", "\"#0000ff\""));
    demand(&session, DemandMode::Force, 1);
    assert_eq!(FrameDisposition::Rendered, take_one(&session).disposition);
    assert_eq!([255, 0, 0, 255], center_pixel(&session, SIZE));

    wait(group.end());
    demand(&session, DemandMode::IfChanged, 2);
    assert_eq!(FrameDisposition::Rendered, take_one(&session).disposition);
    assert_eq!([0, 0, 255, 255], center_pixel(&session, SIZE));
}

#[test]
fn the_redraw_wake_runs_once_per_demand() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let redraws = Arc::new(AtomicUsize::new(0));
    let counter = redraws.clone();
    let options = AttachOptions {
        redraw_wake: Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })),
        ..AttachOptions::default()
    };
    let (session, attached) = map.attach(offscreen(SIZE), options).unwrap();
    attached.wait().unwrap();
    render_until_idle(&map, &session);
    let before = redraws.load(Ordering::SeqCst);

    zoom(&map, 2.0).wait().unwrap();
    assert!(eventually(|| redraws.load(Ordering::SeqCst) == before + 1));
    zoom(&map, 3.0).wait().unwrap();
    wait(session.dump_debug_logs());
    assert_eq!(before + 1, redraws.load(Ordering::SeqCst));
    demand(&session, DemandMode::IfChanged, 1);
    assert_eq!(FrameDisposition::Rendered, take_one(&session).disposition);
    zoom(&map, 4.0).wait().unwrap();
    assert!(eventually(|| redraws.load(Ordering::SeqCst) == before + 2));
}

#[test]
fn a_new_session_wakes_for_the_map_it_has_to_draw() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let redraws = Arc::new(AtomicUsize::new(0));
    let counter = redraws.clone();
    let options = AttachOptions {
        redraw_wake: Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })),
        ..AttachOptions::default()
    };
    let (session, attached) = map.attach(offscreen(SIZE), options).unwrap();
    attached.wait().unwrap();
    assert!(eventually(|| redraws.load(Ordering::SeqCst) == 1));
    demand(&session, DemandMode::IfChanged, 1);
    assert_eq!(FrameDisposition::Rendered, take_one(&session).disposition);
}

// MARK: - Resizing

#[test]
fn resizes_apply_in_order_and_publish_generations() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    demand(&session, DemandMode::Force, 301);
    let old = take_one(&session);

    assert_eq!(ErrorKind::InvalidArgument, session.resize(LogicalExtent::new(Size::new(0, 48), 1.0)).unwrap_err().kind);
    demand(&session, DemandMode::Force, 302);
    let size = Size::new(96, 48);
    let applied = wait(session.resize(LogicalExtent::new(size, 1.0)));
    assert_eq!(old.generation, take_one(&session).generation);
    assert_eq!(ErrorKind::InvalidState, wait_err(session.read_texture()).kind);

    demand(&session, DemandMode::Force, 303);
    let new = take_one(&session);
    assert!(new.generation > old.generation);
    assert!(new.frame_generation > old.frame_generation);
    assert!(new.map_update_generation >= applied.render_update_generation);
    assert_eq!(size, new.view.unwrap().extent.size);
    assert_eq!([255, 0, 0, 255], center_pixel(&session, size));
    let snapshot = session.snapshot().unwrap();
    assert_eq!(size, snapshot.extent.size);
    assert_eq!(1.0, snapshot.extent.scale_factor);
    assert_eq!(new.generation, snapshot.generation);
    assert_eq!(size, map.snapshot().unwrap().view.extent.size);
}

#[test]
fn a_session_renders_at_its_maps_pixel_ratio() {
    let runtime = runtime();
    let options = MapOptions {
        map: mln_map::MapOptions { size: SIZE, pixel_ratio: 2.0, ..Default::default() },
        ..Default::default()
    };
    let (map, created) = runtime.create_map(options).unwrap();
    created.wait().unwrap();
    wait(map.set_style_json(style("#ff0000")));
    let session = attach(&map);
    assert_eq!(2.0, session.snapshot().unwrap().extent.scale_factor);
    render_until_idle(&map, &session);
    assert_eq!([255, 0, 0, 255], center_pixel(&session, Size::new(128, 128)));
}

// A window that moves to a display of another density keeps its session.
#[test]
fn a_resize_changes_the_scale_factor_of_the_map_and_its_next_session() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    render_until_idle(&map, &session);
    let invalid = |scale_factor| session.resize(LogicalExtent::new(SIZE, scale_factor)).unwrap_err().kind;
    assert_eq!(ErrorKind::InvalidArgument, invalid(0.0));
    assert_eq!(ErrorKind::InvalidArgument, invalid(f64::NAN));

    wait(session.resize(LogicalExtent::new(Size::new(32, 16), 2.5)));
    assert_eq!(LogicalExtent::new(Size::new(32, 16), 2.5), session.snapshot().unwrap().extent);
    assert_eq!(2.5, map.scale_factor());
    demand(&session, DemandMode::Force, 1);
    assert_eq!(2.5, take_one(&session).view.unwrap().extent.scale_factor);
    assert_eq!([255, 0, 0, 255], center_pixel(&session, Size::new(80, 40)));

    // A resize that keeps the scale factor, and a new target, keep it too.
    wait(session.resize(LogicalExtent::new(Size::new(16, 16), 2.5)));
    demand(&session, DemandMode::Force, 2);
    assert_eq!(2.5, take_one(&session).view.unwrap().extent.scale_factor);
    wait(session.detach());
    let next = attach(&map);
    assert_eq!(2.5, next.snapshot().unwrap().extent.scale_factor);
    render_until_idle(&map, &next);
    assert_eq!([255, 0, 0, 255], center_pixel(&next, Size::new(160, 160)));
}

#[test]
fn a_resize_applies_while_a_command_group_holds_the_update() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    wait(map.begin_command_group());
    wait(session.resize(LogicalExtent::new(Size::new(96, 48), 1.0)));
    assert_eq!(Size::new(96, 48), map.snapshot().unwrap().view.extent.size);
    assert_eq!(Size::new(96, 48), session.snapshot().unwrap().extent.size);
    wait(session.detach());
    wait(map.end_command_group());
}

#[test]
fn work_sent_after_a_resize_waits_for_it() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    let before = session.snapshot().unwrap().generation;
    // The resize runs on the map's thread first, and the barrier waits for
    // it to reach the session.
    let open = hold_runtime(&map);
    drop(session.resize(LogicalExtent::new(Size::new(96, 80), 1.0)).unwrap());
    let barrier = session.barrier().unwrap();
    drop(open);
    barrier.wait().unwrap();
    let snapshot = session.snapshot().unwrap();
    assert_eq!(Size::new(96, 80), snapshot.extent.size);
    assert_eq!(before + 1, snapshot.generation);
}

/// A barrier on a core worker can finish before `barrier` returns, and a
/// callback attached after it would then run on the sending thread: the
/// callback of `send_then` runs on the session's thread every time.
#[test]
fn send_then_delivers_on_the_sessions_thread() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    let sender = std::thread::current().id();
    for _ in 0..50 {
        let (sent, received) = channel();
        session
            .send_then(RenderSession::barrier, move |result| sent.send((result, std::thread::current().id())).unwrap())
            .unwrap();
        let (result, thread) = received.recv_timeout(TIMEOUT).unwrap();
        result.unwrap();
        assert_ne!(sender, thread);
    }
    wait(session.detach());
    // A failed send runs no callback.
    let error = session.send_then(RenderSession::barrier, |_| panic!("the callback ran")).unwrap_err();
    assert_eq!(ErrorKind::InvalidState, error.kind);
}

/// An attach, resize, detach or release on a core worker can finish before
/// its call returns too, and a callback attached after it would then run on
/// the sending thread: the callbacks of `attach_then` and `send_then` run on
/// the session's thread every time, even when the sender is slow to attach
/// them, as a host's thread that is preempted is.
#[test]
fn attach_then_and_send_then_deliver_the_lifecycle_on_the_sessions_thread() {
    // Long enough for the session's thread to finish first.
    const SLOW: std::time::Duration = std::time::Duration::from_millis(50);
    let runtime = runtime();
    let map = red_map(&runtime);
    let sender = std::thread::current().id();
    let (sent, received) = channel();
    let delivered = move |name: &'static str| {
        let sent = sent.clone();
        move |result: Result<(), mln_runtime::Error>| sent.send((name, result, std::thread::current().id())).unwrap()
    };
    let check = || {
        let (name, result, thread) = received.recv_timeout(TIMEOUT).unwrap();
        result.unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_ne!(sender, thread, "{name} ran on the sending thread");
    };
    // Sends with `send`, then takes a while to attach the callback.
    let slowly = |send: fn(&RenderSession) -> Result<mln_runtime::Completion<()>, mln_runtime::Error>| {
        move |session: &RenderSession| {
            let completion = send(session)?;
            std::thread::sleep(SLOW);
            Ok(completion)
        }
    };
    for round in 0..4 {
        let session = map
            .attach_then(offscreen(SIZE), AttachOptions::default(), |session, attached| {
                std::thread::sleep(SLOW);
                attached.on_complete(delivered("attach"));
                session.clone()
            })
            .unwrap();
        check();
        for step in 1..=2 {
            let extent = LogicalExtent::new(Size::new(SIZE.width + step, SIZE.height), 1.0);
            let resize = move |session: &RenderSession| {
                let resized = session.resize(extent)?;
                std::thread::sleep(SLOW);
                Ok(resized)
            };
            let resized = delivered("resize");
            session.send_then(resize, move |result| resized(result.map(drop))).unwrap();
            check();
        }
        // Detached on even rounds, and released attached on odd ones.
        if round % 2 == 0 {
            session.send_then(slowly(RenderSession::detach), delivered("detach")).unwrap();
        } else {
            session.send_then(slowly(RenderSession::release), delivered("release")).unwrap();
        }
        check();
    }
}

// MARK: - Lifecycle

#[test]
fn attaching_sizes_the_map_and_holds_it() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    assert_eq!(SIZE, map.snapshot().unwrap().view.extent.size);

    let second = map.attach(offscreen(SIZE), AttachOptions::default()).unwrap_err();
    assert_eq!(ErrorKind::InvalidState, second.kind);
    assert_eq!(ErrorKind::InvalidState, map.release().unwrap_err().kind);
    assert_eq!(ErrorKind::InvalidState, map.resize(LogicalExtent::new(Size::new(32, 48), 1.0)).unwrap_err().kind);
    assert_eq!(SIZE, map.snapshot().unwrap().view.extent.size);

    wait(session.release());
    let other = attach(&map);
    wait(other.release());
    wait(map.release());
}

#[test]
fn a_disposed_map_lasts_until_its_session_ends() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    let disposed = map.dispose().unwrap();
    assert_eq!(ErrorKind::InvalidState, map.snapshot().unwrap_err().kind);
    assert_eq!(ErrorKind::InvalidState, map.dispose().unwrap_err().kind);
    // The runtime keeps the map, which the session still draws.
    assert_eq!(ErrorKind::InvalidState, runtime.release().unwrap_err().kind);
    demand(&session, DemandMode::Force, 1);
    assert_eq!(FrameDisposition::Rendered, take_one(&session).disposition);

    // A disposed runtime shuts down once the session lets go of the map.
    runtime.dispose().unwrap();
    wait(session.release());
    disposed.wait().unwrap();
}

#[test]
fn a_wait_for_session_work_on_the_runtimes_thread_fails_instead_of_hanging() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    let resizing = session.clone();
    // The resize needs the runtime's thread, which the wait would hold.
    let waited = wait(map.with_core(move |_| {
        resizing.resize(LogicalExtent::new(Size::new(32, 32), 1.0)).unwrap().wait().map_err(|e| e.kind)
    }));
    assert_eq!(Err(ErrorKind::InvalidState), waited.map(drop));
    wait(session.release());
}

#[test]
fn attach_rejects_an_empty_target() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let empty = map.attach(offscreen(Size::new(0, 64)), AttachOptions::default()).unwrap_err();
    assert_eq!(ErrorKind::InvalidArgument, empty.kind);
    // It didn't leave the map with a session.
    drop(attach(&map));
}

#[test]
fn a_demand_made_while_attaching_renders_after_the_attach() {
    let runtime = runtime();
    let map = red_map(&runtime);
    // The held thread binds the session only after the demand reached it.
    let open = hold_runtime(&map);
    let (session, attached) = map.attach(offscreen(SIZE), AttachOptions::default()).unwrap();
    demand(&session, DemandMode::Force, 5);
    let (sender, completed) = channel();
    let drained = session.clone();
    attached.on_complete(move |result| {
        let _ = sender.send(result.map(|()| drained.drain_frame_results().unwrap().len()));
    });
    drop(open);
    // The attach completes before the demand renders.
    assert_eq!(0, completed.recv_timeout(TIMEOUT).unwrap().unwrap());
    let result = take_one(&session);
    assert_eq!((5, FrameDisposition::Rendered), (result.token.unwrap(), result.disposition));
}

#[test]
fn a_detached_session_frees_its_map_and_refuses_work() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    wait(session.detach());
    assert_eq!(RenderSessionState::Detached, session.snapshot().unwrap().state);
    let other = attach(&map);
    drop(other);

    let refused = |error: mln_runtime::Error| assert_eq!(ErrorKind::InvalidState, error.kind);
    refused(session.demand_frame(FrameDemand::default()).unwrap_err());
    refused(session.barrier().unwrap_err());
    refused(session.resize(LogicalExtent::new(SIZE, 1.0)).unwrap_err());
    refused(session.reduce_memory_use().unwrap_err());
    refused(session.clear_data().unwrap_err());
    refused(session.dump_debug_logs().unwrap_err());
    refused(session.read_texture().unwrap_err());
    refused(session.detach().unwrap_err());
    refused(session.abandon().unwrap_err());
    assert_eq!(RenderSessionState::Detached, session.snapshot().unwrap().state);

    wait(session.release());
    refused(session.snapshot().unwrap_err());
    refused(session.dispose().unwrap_err());
}

#[test]
fn abandon_ends_pending_work_and_frees_the_map_at_once() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    render_until_idle(&map, &session);
    demand(&session, DemandMode::WaitForUpdate, 9);
    wait(session.dump_debug_logs());

    session.abandon().unwrap();
    assert_eq!(RenderSessionState::Abandoned, session.snapshot().unwrap().state);
    let other = attach(&map);
    let result = take_one(&session);
    assert_eq!((9, FrameDisposition::TargetNotReady), (result.token.unwrap(), result.disposition));
    assert_eq!(ErrorKind::InvalidState, session.demand_frame(FrameDemand::default()).unwrap_err().kind);
    assert_eq!(ErrorKind::InvalidState, session.abandon().unwrap_err().kind);
    wait(session.release());

    render_until_idle(&map, &other);
}

#[test]
fn an_abandon_from_a_completion_that_the_worker_delivers_is_busy() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    demand(&session, DemandMode::Force, 1);
    take_one(&session);
    let (sender, receiver) = channel();
    let abandoned = session.clone();
    // A plain `on_complete` would run on this thread once the work ran.
    session
        .send_then(RenderSession::reduce_memory_use, move |result| {
            let _ = sender.send((result, abandoned.abandon().map_err(|error| error.kind)));
        })
        .unwrap();
    let (result, abandon) = receiver.recv_timeout(TIMEOUT).unwrap();
    result.unwrap();
    assert_eq!(Err(ErrorKind::Busy), abandon);
    assert_eq!(RenderSessionState::Attached, session.snapshot().unwrap().state);
    // Off the worker's thread, the abandon waits for its call.
    assert_eq!(0, session.abandon().unwrap());
}

#[test]
fn an_abandon_that_keeps_graphics_counts_the_renderer_and_its_backend() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    demand(&session, DemandMode::Force, 1);
    take_one(&session);
    demand(&session, DemandMode::WaitForUpdate, 2);

    assert_eq!(2, session.abandon_keeping_graphics().unwrap());
    assert_eq!(RenderSessionState::Abandoned, session.snapshot().unwrap().state);
    let result = take_one(&session);
    assert_eq!((2, FrameDisposition::TargetNotReady), (result.token.unwrap(), result.disposition));
    assert_eq!(ErrorKind::InvalidState, session.abandon_keeping_graphics().unwrap_err().kind);
    wait(session.release());

    let other = attach(&map);
    render_until_idle(&map, &other);
}

#[test]
fn a_dispose_that_keeps_graphics_frees_the_map_at_once() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    demand(&session, DemandMode::Force, 1);
    take_one(&session);

    session.dispose_keeping_graphics().unwrap();
    assert_eq!(ErrorKind::InvalidState, session.snapshot().unwrap_err().kind);
    assert_eq!(ErrorKind::InvalidState, session.dispose_keeping_graphics().unwrap_err().kind);
    let other = attach(&map);
    render_until_idle(&map, &other);

    // A session that ended already is only released.
    wait(other.detach());
    other.dispose_keeping_graphics().unwrap();
    attach(&map);
}

#[test]
fn work_queued_before_an_abandon_completes_as_target_lost() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    let open = hold_runtime(&map);
    // The resize waits for the runtime's thread, and reaches the session
    // after the abandon.
    let resized = session.resize(LogicalExtent::new(Size::new(32, 32), 1.0)).unwrap();
    session.abandon().unwrap();
    drop(open);
    assert_eq!(ErrorKind::TargetLost, resized.wait().unwrap_err().kind);
}

#[test]
fn dropping_the_last_handle_detaches_the_session() {
    let runtime = runtime();
    let map = red_map(&runtime);
    drop(attach(&map));
    assert!(eventually(|| map.attach(offscreen(SIZE), AttachOptions::default()).is_ok()));
}

#[test]
fn maintenance_work_runs_in_order_with_frames() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    demand(&session, DemandMode::Force, 1);
    wait(session.reduce_memory_use());
    assert_eq!(1, session.drain_frame_results().unwrap().len());
    wait(session.clear_data());
    demand(&session, DemandMode::IfChanged, 2);
    wait(session.dump_debug_logs());
    let result = session.drain_frame_results().unwrap();
    assert_eq!(1, result.len());
    assert_eq!((2, FrameDisposition::Rendered), (result[0].token.unwrap(), result[0].disposition));
    assert_eq!(ErrorKind::Unsupported, session.release_target().unwrap_err().kind);
}

#[test]
fn maintenance_work_waits_behind_a_demand_for_map_commands() {
    let runtime = runtime();
    let map = red_map(&runtime);
    let session = attach(&map);
    render_until_idle(&map, &session);
    let open = hold_runtime(&map);
    let zoomed = zoom(&map, 2.0);
    demand(&session, DemandMode::Force, 1);
    // The work completes once the demand rendered, which waits for the zoom
    // that the held runtime hasn't applied yet.
    let (sender, receiver) = channel();
    let drained = session.clone();
    session.reduce_memory_use().unwrap().on_complete(move |result| {
        let _ = sender.send((result, drained.drain_frame_results().unwrap()));
    });
    drop(open);
    zoomed.wait().unwrap();
    let (result, results) = receiver.recv_timeout(TIMEOUT).unwrap();
    result.unwrap();
    assert_eq!(1, results.len(), "{results:?}");
    assert_eq!((1, FrameDisposition::Rendered), (results[0].token.unwrap(), results[0].disposition));
}

#[test]
fn sessions_on_two_maps_share_one_device() {
    let device = Device::new(DeviceOptions::default()).unwrap();
    let runtime = runtime();
    let (red, green) = (red_map(&runtime), map_in_mode(&runtime, MapMode::Continuous));
    wait(green.set_style_json(style("#00ff00")));
    let sessions: Vec<RenderSession> = [&red, &green]
        .into_iter()
        .map(|map| {
            let target = RenderTarget::Offscreen { size: SIZE, device: Some(device.clone()) };
            let (session, attached) = map.attach(target, AttachOptions::default()).unwrap();
            attached.wait().unwrap();
            session
        })
        .collect();
    // The device lasts while its sessions use it.
    drop(device);
    for (map, session) in [&red, &green].into_iter().zip(&sessions) {
        render_until_idle(map, session);
    }
    assert_eq!([255, 0, 0, 255], center_pixel(&sessions[0], SIZE));
    assert_eq!([0, 255, 0, 255], center_pixel(&sessions[1], SIZE));
}

#[test]
fn a_device_reports_a_missing_adapter() {
    let error = Device::new(DeviceOptions {
        adapter: Some(mln_runtime::Adapter::Index(usize::MAX)),
        ..DeviceOptions::default()
    })
    .unwrap_err();
    assert_eq!(ErrorKind::NotFound, error.kind);
}

// MARK: - Still images

#[test]
fn a_static_map_renders_still_images() {
    let runtime = runtime();
    let map = map_in_mode(&runtime, MapMode::Static);
    wait(map.set_style_json(style("#ff0000")));
    let session = attach(&map);

    wait(map.request_still_image());
    let results = session.drain_frame_results().unwrap();
    let last = results.last().unwrap();
    assert_eq!((None, FrameDisposition::Rendered), (last.token, last.disposition));
    assert_eq!([255, 0, 0, 255], center_pixel(&session, SIZE));

    wait(map.set_style_json(style("#00ff00")));
    wait(map.request_still_image());
    assert_eq!([0, 255, 0, 255], center_pixel(&session, SIZE));
}

#[test]
fn a_tile_map_session_resizes_between_still_images() {
    let runtime = runtime();
    let map = map_in_mode(&runtime, MapMode::Tile);
    wait(map.set_style_json(style("#ff0000")));
    let session = attach(&map);
    wait(map.request_still_image());
    let size = Size::new(96, 48);
    wait(session.resize(LogicalExtent::new(size, 1.0)));
    wait(map.request_still_image());
    assert_eq!([255, 0, 0, 255], center_pixel(&session, size));
}

#[test]
fn a_still_image_waits_for_a_session() {
    let runtime = runtime();
    let map = map_in_mode(&runtime, MapMode::Static);
    wait(map.set_style_json(style("#ff0000")));
    let still = map.request_still_image().unwrap();
    // A read behind the request.
    wait(map.camera());
    assert!(!still.is_complete());
    let session = attach(&map);
    still.wait().unwrap();
    assert_eq!([255, 0, 0, 255], center_pixel(&session, SIZE));
}

#[test]
fn a_continuous_map_refuses_a_still_image() {
    let runtime = runtime();
    let map = red_map(&runtime);
    assert_eq!(ErrorKind::InvalidState, map.request_still_image().unwrap_err().kind);
}

#[test]
fn a_still_image_inside_a_command_group_fails() {
    let runtime = runtime();
    let map = map_in_mode(&runtime, MapMode::Static);
    wait(map.begin_command_group());
    assert_eq!(ErrorKind::InvalidState, wait_err(map.request_still_image()).kind);
    wait(map.end_command_group());
}

#[test]
fn map_release_cancels_the_pending_still_image() {
    let runtime = runtime();
    let map = map_in_mode(&runtime, MapMode::Static);
    let pending = map.request_still_image().unwrap();
    assert_eq!(ErrorKind::InvalidState, wait_err(map.request_still_image()).kind);
    wait(map.release());
    assert_eq!(ErrorKind::Cancelled, pending.wait().unwrap_err().kind);
}

/// One vector source with one tile at zoom 0, which a still image of a
/// 64 by 64 map needs.
const TILED_STYLE: &str = r##"{"version":8,"sources":{"tiles":{"type":"vector","tiles":["custom://tiles/{z}/{x}/{y}.pbf"],"minzoom":0,"maxzoom":0}},"layers":[{"id":"bg","type":"background","paint":{"background-color":"#ff0000"}},{"id":"fill","type":"fill","source":"tiles","source-layer":"any"}]}"##;

/// A tile without content, which loads as an empty tile.
fn empty_tile() -> Response {
    Response { no_content: true, ..Response::default() }
}

/// A static map of `TILED_STYLE` whose provider holds the first tile
/// request, so that a still image waits for the test to answer it, and
/// answers every later one with an empty tile.
struct HeldTile {
    map: Map,
    held: Arc<Mutex<Option<ResourceRequest>>>,
    requests: Arc<AtomicUsize>,
}

impl HeldTile {
    fn new(runtime: &Runtime) -> HeldTile {
        let held = Arc::new(Mutex::new(None));
        let requests = Arc::new(AtomicUsize::new(0));
        let (holder, counter) = (held.clone(), requests.clone());
        wait(runtime.set_resource_provider(move |request: ResourceRequest| {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                *holder.lock() = Some(request);
            } else {
                // A request that the renderer cancelled meanwhile takes no
                // answer.
                let _ = request.respond(empty_tile());
            }
        }));
        let map = map_in_mode(runtime, MapMode::Static);
        wait(map.set_style_json(TILED_STYLE));
        HeldTile { map, held, requests }
    }

    /// Waits until the renderer has asked for the tile `count` times.
    fn wait_for_requests(&self, count: usize) {
        assert!(eventually(|| self.requests.load(Ordering::SeqCst) >= count), "the tile was never requested");
    }

    /// Answers the held request with an empty tile, unless the renderer
    /// cancelled it.
    fn answer(&self) {
        if let Some(request) = self.held.lock().take() {
            let _ = request.respond(empty_tile());
        }
    }
}

fn tokens_and_dispositions(results: &[FrameResult]) -> Vec<(Option<u64>, FrameDisposition)> {
    results.iter().map(|result| (result.token, result.disposition)).collect()
}

#[test]
fn a_still_image_waits_for_the_next_session() {
    let runtime = runtime();
    let held = HeldTile::new(&runtime);
    let still = held.map.request_still_image().unwrap();
    let first = attach(&held.map);
    held.wait_for_requests(1);
    wait(first.detach());
    let ended = first.drain_frame_results().unwrap();
    assert_eq!(vec![(None, FrameDisposition::TargetNotReady)], tokens_and_dispositions(&ended));
    assert!(!still.is_complete());

    // The detach cancelled the held request, so the next session's renderer
    // asks for the tile again, and gets an empty one.
    let second = attach(&held.map);
    still.wait().unwrap();
    let rendered = second.drain_frame_results().unwrap();
    assert_eq!(vec![(None, FrameDisposition::Rendered)], tokens_and_dispositions(&rendered));
    assert_eq!([255, 0, 0, 255], center_pixel(&second, SIZE));
    held.answer();
}

#[test]
fn a_barrier_ends_a_still_image_demand_until_the_next() {
    let runtime = runtime();
    let held = HeldTile::new(&runtime);
    let session = attach(&held.map);
    let still = held.map.request_still_image().unwrap();
    held.wait_for_requests(1);
    wait(session.barrier());
    let ended = take_one(&session);
    assert_eq!((None, FrameDisposition::NoUpdate), (ended.token, ended.disposition));
    assert!(!still.is_complete());

    // The demand that the barrier made again renders the image once the
    // tile loads.
    held.answer();
    still.wait().unwrap();
    let rendered = session.drain_frame_results().unwrap();
    assert_eq!(vec![(None, FrameDisposition::Rendered)], tokens_and_dispositions(&rendered));
}

#[test]
fn host_demands_and_a_still_image_demand_never_replace_each_other() {
    let runtime = runtime();
    let held = HeldTile::new(&runtime);
    let session = attach(&held.map);
    let still = held.map.request_still_image().unwrap();
    held.wait_for_requests(1);
    demand(&session, DemandMode::WaitForUpdate, 7);
    held.answer();
    still.wait().unwrap();
    // The host's demand ends with no update, before or after the image's
    // frame. Usually the image's frame shows the update that the demand waits
    // for, and a static map publishes another only for its next still image,
    // so the demand waits until a barrier ends it. But the demand may reach
    // the session after the map published an update and before the session
    // served the image with it: the demand then draws that update first,
    // while the tile is still held, and ends with nothing to show.
    wait(session.barrier());
    let mut results = tokens_and_dispositions(&take_results(&session, 2));
    results.sort_by_key(|(token, _)| *token);
    assert_eq!(vec![(None, FrameDisposition::Rendered), (Some(7), FrameDisposition::NoUpdate)], results);
}

#[test]
fn a_still_image_that_fails_ends_its_demand() {
    let runtime = runtime();
    wait(runtime.set_resource_provider(|request: ResourceRequest| {
        let error = ResourceError::new(Reason::Other, "no tile here");
        let _ = request.respond(Response { error: Some(Box::new(error)), ..Response::default() });
    }));
    let map = map_in_mode(&runtime, MapMode::Static);
    wait(map.set_style_json(TILED_STYLE));
    let session = attach(&map);
    assert_eq!(ErrorKind::Native, wait_err(map.request_still_image()).kind);
    let ended = take_one(&session);
    assert_eq!((None, FrameDisposition::NoUpdate), (ended.token, ended.disposition));
}

// MARK: - The tile cache

/// One vector source with tiles up to zoom 14.
const TILE_CACHE_STYLE: &str = r#"{"version":8,"sources":{"tiles":{"type":"vector","tiles":["custom://tiles/{z}/{x}/{y}.pbf"],"minzoom":0,"maxzoom":14}},"layers":[{"id":"lines","type":"line","source":"tiles","source-layer":"any"}]}"#;

/// Serves empty tiles that are fresh for years, so a request for a tile that
/// the map kept only revalidates it. It counts the other requests, the
/// loads, and holds them while the test says so.
#[derive(Clone, Default)]
struct TileServer {
    loads: Arc<AtomicUsize>,
    /// The loads that wait, or `None` while the server answers at once.
    held: Arc<Mutex<Option<Vec<ResourceRequest>>>>,
}

impl TileServer {
    fn answer(request: ResourceRequest, revalidation: bool) {
        let expires = Some(Timestamp::from_secs(4_102_444_800));
        let _ = request.respond(Response { expires, not_modified: revalidation, ..empty_tile() });
    }

    fn serve(&self, request: ResourceRequest) {
        let revalidation = request.resource().prior_expires.is_some();
        if request.resource().kind == Kind::Tile && !revalidation {
            self.loads.fetch_add(1, Ordering::SeqCst);
            if let Some(held) = self.held.lock().as_mut() {
                return held.push(request);
            }
        }
        TileServer::answer(request, revalidation);
    }

    fn hold(&self) {
        *self.held.lock() = Some(Vec::new());
    }

    fn release(&self) {
        for request in self.held.lock().take().unwrap_or_default() {
            TileServer::answer(request, false);
        }
    }
}

/// Whether `event` reports that the map looked for a tile of zoom 0 in the
/// cache, as it does once the tiles of a view fall back on their parents all
/// the way up.
fn looked_up_zoom_0(event: &RuntimeEvent) -> bool {
    matches!(
        &event.kind,
        RuntimeEventKind::Map(MapEvent::TileAction { op: TileOperation::RequestedFromCache, id, .. })
            if id.canonical.z == 0
    )
}

/// Renders still images of a view, of a view elsewhere, and of the first
/// view again, and returns how many tiles the first image loads and how many
/// the last one does. The ambient cache is off, so a tile that the map let
/// go loads again. The tiles of the first two views load only once the map
/// has looked for their parents in the cache, up to zoom 0: as with tiles
/// that take a while to load, the map then holds many tiles that load
/// nothing.
fn render_and_return(tile_cache_enabled: Option<bool>) -> (usize, usize) {
    let runtime = runtime();
    wait(runtime.set_maximum_ambient_cache_size(0));
    let server = TileServer::default();
    let serving = server.clone();
    wait(runtime.set_resource_provider(move |request: ResourceRequest| serving.serve(request)));
    let size = Size::new(256, 256);
    let mut options = MapOptions {
        map: mln_map::MapOptions { size, map_mode: MapMode::Static, ..Default::default() },
        ..Default::default()
    };
    if let Some(enabled) = tile_cache_enabled {
        options.tile_cache_enabled = enabled;
    }
    let (map, created) = runtime.create_map(options).unwrap();
    created.wait().unwrap();
    wait(map.set_style_json(TILE_CACHE_STYLE));
    let (_session, attached) = map.attach(offscreen(size), AttachOptions::default()).unwrap();
    attached.wait().unwrap();
    let render_at = |latitude: f64, longitude: f64, fall_back: bool| {
        let camera = CameraOptions {
            center: Some(LatLng::new(latitude, longitude).unwrap()),
            zoom: Some(10.0),
            ..CameraOptions::default()
        };
        wait(map.update_camera(CameraUpdate { camera, ..CameraUpdate::default() }));
        let log = fall_back.then(|| {
            server.hold();
            EventLog::install(&runtime)
        });
        let still = map.request_still_image().unwrap();
        if let Some(log) = log {
            assert!(log.wait_for(looked_up_zoom_0).is_some(), "the tiles never fell back on zoom 0");
            server.release();
        }
        still.wait().unwrap();
        server.loads.load(Ordering::SeqCst)
    };
    let first = render_at(0.0, 0.0, true);
    let elsewhere = render_at(40.0, 100.0, true);
    let returning = render_at(0.0, 0.0, false) - elsewhere;
    (first, returning)
}

#[test]
fn a_map_keeps_the_tiles_that_left_the_view_by_default() {
    let (first, returning) = render_and_return(None);
    assert!(first > 0);
    assert_eq!(0, returning);
}

#[test]
fn a_map_with_the_tile_cache_enabled_keeps_them() {
    let (first, returning) = render_and_return(Some(true));
    assert!(first > 0);
    assert_eq!(0, returning);
}

#[test]
fn a_map_with_the_tile_cache_disabled_loads_them_again() {
    let (first, returning) = render_and_return(Some(false));
    assert!(first > 0);
    assert_eq!(first, returning);
}

// MARK: - Queries

/// Two points on the center row, "west" at x 18 and "east" at x 46, which
/// the "circles" layer draws, and a clustered source of three points close
/// together, which no layer draws.
const QUERY_STYLE: &str = r#"{
  "version": 8,
  "sources": {
    "points": {"type": "geojson", "data": {"type": "FeatureCollection", "features": [
      {"type": "Feature", "id": 1, "properties": {"name": "west"}, "geometry": {"type": "Point", "coordinates": [-10, 0]}},
      {"type": "Feature", "id": 2, "properties": {"name": "east"}, "geometry": {"type": "Point", "coordinates": [10, 0]}}
    ]}},
    "clustered": {"type": "geojson", "cluster": true, "clusterMaxZoom": 5, "data": {"type": "FeatureCollection", "features": [
      {"type": "Feature", "properties": {"n": 1}, "geometry": {"type": "Point", "coordinates": [0, 0]}},
      {"type": "Feature", "properties": {"n": 2}, "geometry": {"type": "Point", "coordinates": [0.01, 0]}},
      {"type": "Feature", "properties": {"n": 3}, "geometry": {"type": "Point", "coordinates": [0, 0.01]}}
    ]}}
  },
  "layers": [
    {"id": "circles", "type": "circle", "source": "points", "paint": {"circle-radius": 4}},
    {"id": "clusters", "type": "circle", "source": "clustered", "paint": {"circle-radius": 2, "circle-opacity": 0}}
  ]
}"#;

/// A session of a map at zoom 0 around (0, 0) that has drawn `QUERY_STYLE`
/// with every source loaded.
fn query_session(runtime: &Runtime) -> (Map, RenderSession) {
    let map = map_in_mode(runtime, MapMode::Continuous);
    wait(map.set_style_json(QUERY_STYLE));
    wait(map.update_camera(CameraUpdate {
        camera: CameraOptions { zoom: Some(0.0), center: Some(mln_util::geo::LatLng::default()), ..Default::default() },
        ..Default::default()
    }));
    let session = attach(&map);
    let loaded = eventually(|| {
        demand(&session, DemandMode::Force, 0);
        let result = take_one(&session);
        result.mode == mln_map::RenderMode::Full
            && wait(session.query_source_features("points", SourceQueryOptions::default())).len() == 2
    });
    assert!(loaded, "the sources never loaded");
    (map, session)
}

#[test]
fn rendered_queries_select_by_geometry_and_layer() {
    let runtime = runtime();
    let (_map, session) = query_session(&runtime);
    let names = |features: Vec<mln_util::feature::Feature>| -> Vec<String> {
        let mut names: Vec<String> = features
            .iter()
            .filter_map(|feature| feature.properties.get("name").map(|name| format!("{name:?}")))
            .collect();
        names.sort();
        names
    };

    let east = RenderedQueryGeometry::Point(ScreenCoordinate::new(46.0, 32.0));
    let hits = wait(session.query_rendered_features(east.clone(), RenderedQueryOptions::default()));
    assert_eq!(1, hits.len(), "{hits:?}");
    assert_eq!("circles", hits[0].layer);
    assert_eq!("points", hits[0].source);

    let everything =
        RenderedQueryGeometry::Box { min: ScreenCoordinate::new(0.0, 0.0), max: ScreenCoordinate::new(64.0, 64.0) };
    let circles = RenderedQueryOptions::new(Some(vec!["circles".into()]), None);
    assert_eq!(2, names(wait(session.query_rendered_features(everything.clone(), circles))).len());
    let line =
        RenderedQueryGeometry::LineString(vec![ScreenCoordinate::new(10.0, 32.0), ScreenCoordinate::new(26.0, 32.0)]);
    assert_eq!(1, wait(session.query_rendered_features(line, RenderedQueryOptions::default())).len());
    let none = RenderedQueryOptions::new(Some(vec!["missing".into()]), None);
    assert_eq!(wait(session.query_rendered_features(east, none)), [] as [mln_util::feature::Feature; 0]);

    // A box past every edge covers the whole target, and one outside it
    // covers nothing.
    let beyond = RenderedQueryGeometry::Box {
        min: ScreenCoordinate::new(-4096.0, -4096.0),
        max: ScreenCoordinate::new(4096.0, 4096.0),
    };
    let circles = RenderedQueryOptions::new(Some(vec!["circles".into()]), None);
    assert_eq!(2, wait(session.query_rendered_features(beyond, circles)).len());
    let outside =
        RenderedQueryGeometry::Box { min: ScreenCoordinate::new(100.0, 0.0), max: ScreenCoordinate::new(200.0, 64.0) };
    assert_eq!(
        wait(session.query_rendered_features(outside, RenderedQueryOptions::default())),
        [] as [mln_util::feature::Feature; 0]
    );

    let invalid =
        |geometry| session.query_rendered_features(geometry, RenderedQueryOptions::default()).unwrap_err().kind;
    assert_eq!(ErrorKind::InvalidArgument, invalid(RenderedQueryGeometry::LineString(Vec::new())));
    assert_eq!(ErrorKind::InvalidArgument, invalid(RenderedQueryGeometry::Point(ScreenCoordinate::new(f64::NAN, 0.0))));
    let empty_source = session.query_source_features("", SourceQueryOptions::default()).unwrap_err();
    assert_eq!(ErrorKind::InvalidArgument, empty_source.kind);
}

#[test]
fn cluster_queries_read_a_clustered_source() {
    let runtime = runtime();
    let (_map, session) = query_session(&runtime);
    let clusters = wait(session.query_source_features("clustered", SourceQueryOptions::default()));
    let cluster_id = clusters
        .iter()
        .find_map(|feature| match feature.properties.get("cluster_id") {
            Some(feature::Value::Uint(id)) => Some(*id as u32),
            Some(feature::Value::Int(id)) => Some(*id as u32),
            Some(feature::Value::Double(id)) => Some(*id as u32),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no cluster in {clusters:?}"));

    assert_ne!(wait(session.cluster_children("clustered", cluster_id)), [] as [feature::Feature; 0]);
    assert_eq!(3, wait(session.cluster_leaves("clustered", cluster_id, 10, 0)).len());
    assert_eq!(1, wait(session.cluster_leaves("clustered", cluster_id, 1, 2)).len());
    assert!(wait(session.cluster_expansion_zoom("clustered", cluster_id)) > 0);

    let kind = |completion| wait_err(completion).kind;
    assert_eq!(ErrorKind::NotFound, kind(session.cluster_children("missing", cluster_id)));
    assert_eq!(ErrorKind::InvalidArgument, kind(session.cluster_children("points", cluster_id)));
    assert_eq!(ErrorKind::NotFound, kind(session.cluster_children("clustered", 0)));
}
