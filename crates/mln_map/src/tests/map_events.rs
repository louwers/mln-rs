//! The map's event queue: what it reports to its owner, what it does with
//! the events of its style and renderer, and its waker.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mln_engine::RendererEvent;
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Response};
use mln_style::Edit;
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::{Error, ErrorKind};
use mln_style::geo::LatLng;
use mln_style::size::Size;
use mln_style::time::Instant;
use mln_util::queue::Queue;

use crate::map_options::UnusedStyleImagePolicy;
use crate::{
    AnimationOptions, CameraChange, CameraChangeMode, CameraChangeSource, CameraOptions, CameraUpdate,
    CameraUpdateMode, Map, MapEvent, MapMode, MapOptions,
};

fn map(options: MapOptions, run_loop: &Queue<()>) -> Map {
    Map::with_file_source(
        StubFileSource::on_queue(run_loop, ResponseType::Asynchronous),
        options,
        &crate::tests::pool(),
    )
    .expect("map")
}

/// Reports `event` to the map as its renderer would.
fn report(map: &Map, event: RendererEvent) {
    map.renderer_events().push(event);
}

/// The `update_id` of the map's latest update, which a frame of it names.
fn latest_update(map: &Map) -> u64 {
    map.update_reader().latest().map_or(0, |update| update.parameters.update_id)
}

/// The report of a `Full` frame of `map`'s latest update.
fn full_frame(map: &Map) -> RendererEvent {
    RendererEvent::RenderFrameFinished {
        mode: mln_engine::RenderMode::Full,
        needs_repaint: false,
        repaint_at: None,
        placement_changed: false,
        still_image_request_id: 0,
        update_id: latest_update(map),
        stats: Arc::default(),
        error: None,
    }
}

const STYLE: &str = r#"{"version": 8, "center": [10, 20], "zoom": 3, "sources": {}, "layers": []}"#;

#[test]
fn camera_commands_report_their_changes_in_order() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions::default(), &run_loop);
    map.drain_events();

    map.jump_to(&CameraOptions { zoom: Some(2.0), ..Default::default() });
    let change = CameraChange { mode: CameraChangeMode::Immediate, source: CameraChangeSource::Command };
    let events = map.drain_events();
    assert!(
        matches!(events.as_slice(), [MapEvent::CameraWillChange(will), MapEvent::CameraDidChange(did)] if *will == change && *did == change),
        "{events:?}"
    );
    assert!(map.drain_events().is_empty());
}

#[test]
fn a_style_load_applies_in_the_call_that_loads_it() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions::default(), &run_loop);
    let updates = map.update_generation();
    map.load_style_json(STYLE);
    // The map applies the style's camera as the style loads.
    assert_eq!(CameraOptions::from(map.snapshot().view.camera).zoom, Some(3.0));
    assert!(map.update_generation() > updates);

    let events = map.drain_events();
    let loading = events.iter().position(|event| matches!(event, MapEvent::LoadingStarted));
    let loaded = events.iter().position(|event| matches!(event, MapEvent::StyleLoaded));
    assert!(loading.is_some() && loading < loaded, "{events:?}");
}

#[test]
fn a_camera_command_after_a_style_load_wins_over_the_style_camera() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions::default(), &run_loop);
    map.load_style_json(STYLE);
    // The owner has the map handle the load before the command, so the
    // style's center stays and the command's zoom applies.
    map.process_events();
    map.jump_to(&CameraOptions { zoom: Some(5.0), ..Default::default() });
    let camera = CameraOptions::from(map.snapshot().view.camera);
    assert_eq!(camera.zoom, Some(5.0));
    let center = camera.center.expect("center");
    assert!((center.latitude() - 20.0).abs() < 1e-9 && (center.longitude() - 10.0).abs() < 1e-9, "{center:?}");
}

/// A style that draws the globe below zoom 3 and Mercator above.
const GLOBE_BELOW_3: &str = r#"{"version": 8, "sources": {}, "layers": [],
    "projection": {"type": ["step", ["zoom"], "vertical-perspective", 3, "mercator"]}}"#;

/// The zoom that a jump from zoom 5 to -0.5 at 60° N lands on. A viewport
/// taller than the world keeps Mercator above zoom 1, which the globe
/// doesn't, so the jump lands on -0.5 only when the transform has the
/// projection that the style gives at -0.5.
fn lowest_jump(map: &mut Map) -> f64 {
    let center = LatLng::new(60.0, 0.0).expect("LatLng");
    map.jump_to(&CameraOptions { center: Some(center), zoom: Some(5.0), ..Default::default() });
    map.jump_to(&CameraOptions { zoom: Some(-0.5), ..Default::default() });
    CameraOptions::from(map.snapshot().view.camera).zoom.expect("zoom")
}

#[test]
fn each_change_to_the_style_projection_reaches_the_transform() {
    let run_loop = Queue::<()>::new();
    let file_source = StubFileSource::on_queue(&run_loop, ResponseType::Asynchronous);
    file_source.set_response(Kind::Style, |_| {
        let mut response = Response::new();
        response.data = Some(Arc::new(GLOBE_BELOW_3.as_bytes().to_vec()));
        Some(response)
    });
    let options = MapOptions { size: Size::new(512, 1024), ..Default::default() };
    let mut map = Map::with_file_source(file_source, options, &crate::tests::pool()).expect("map");
    assert!(lowest_jump(&mut map) >= 1.0);

    // A style load.
    map.load_style_url("http://style");
    map.process_events();
    run_loop.run_once(&mut ());
    map.process_events();
    assert!(map.style_document_loaded());
    assert!((lowest_jump(&mut map) + 0.5).abs() < 1e-9);
    let globe_below_3 = (**map.style().get_projection()).clone();

    // A style's JSON that replaces it.
    map.load_style_json(STYLE);
    assert!(lowest_jump(&mut map) >= 1.0);

    // An edit.
    map.edit_style(|style| style.apply(Edit::SetProjection(globe_below_3))).unwrap();
    assert!((lowest_jump(&mut map) + 0.5).abs() < 1e-9);

    // A JSON edit.
    map.edit_style(|style| style.set_root_property_json("projection", r#"{"type":"mercator"}"#)).unwrap();
    assert!(lowest_jump(&mut map) >= 1.0);
}

#[test]
fn the_waker_runs_when_the_map_has_events() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions::default(), &run_loop);
    map.drain_events();
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    map.set_waker(Some(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })));
    assert_eq!(wakes.load(Ordering::SeqCst), 0);

    report(&map, RendererEvent::Invalidate);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    let updates = map.update_generation();
    map.process_events();
    assert_eq!(map.update_generation(), updates + 1);
}

/// A frame that only waits for a symbol placement asks for no update, and
/// the map isn't settled or idle, until its owner repaints it then. The
/// repaint shows the map at that time, though the host's last frame time
/// still counts and comes before it.
#[test]
fn a_frame_waiting_for_a_placement_asks_for_no_update_until_then() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions { map_mode: MapMode::Continuous, ..Default::default() }, &run_loop);
    map.load_style_json(STYLE);
    map.process_events();
    map.drain_events();
    let frame = |map: &Map, repaint_at| RendererEvent::RenderFrameFinished {
        mode: mln_engine::RenderMode::Full,
        needs_repaint: false,
        repaint_at,
        placement_changed: false,
        still_image_request_id: 0,
        update_id: latest_update(map),
        stats: Arc::default(),
        error: None,
    };
    let finished = |map: &mut Map| {
        let events = map.drain_events();
        let settled = events.iter().find_map(|event| match event {
            MapEvent::RenderFrameFinished(status) => Some(!status.needs_repaint),
            _ => None,
        });
        (settled, events.iter().any(|event| matches!(event, MapEvent::Idle)))
    };

    let now = Instant::now();
    map.set_frame_time(Some(now.checked_sub(Duration::from_millis(20)).unwrap()));
    let at = now.checked_sub(Duration::from_millis(10)).unwrap();
    let updates = map.update_generation();
    report(&map, frame(&map, Some(at)));
    map.process_events();
    assert_eq!(map.update_generation(), updates, "no update until the owner repaints");
    assert_eq!(map.repaint_at(), Some(at));
    assert_eq!(finished(&mut map), (Some(false), false));

    map.trigger_repaint();
    assert_eq!(map.repaint_at(), None);
    let update = map.update_reader().latest().expect("an update");
    assert!(update.parameters.time_point >= Some(at), "the repaint shows the map at its time");
    report(&map, frame(&map, None));
    map.process_events();
    assert_eq!(finished(&mut map), (Some(true), true));
}

/// While a host's frame clock samples a running animation, each frame
/// already showed the animation at its time: the frame's report adds no
/// update, which would show the frame's camera again and report it as
/// moving again. Without the clock, the report's update is what moves it.
#[test]
fn a_frame_report_adds_no_update_while_the_frame_clock_samples_an_animation() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions { map_mode: MapMode::Continuous, ..Default::default() }, &run_loop);
    map.load_style_json(STYLE);
    map.update_camera(&CameraUpdate {
        mode: CameraUpdateMode::Ease,
        camera: CameraOptions { zoom: Some(10.0), ..Default::default() },
        animation: AnimationOptions { duration: Some(Duration::from_secs(60)), ..Default::default() },
        ..Default::default()
    });
    let moving =
        |events: &[MapEvent]| events.iter().filter(|event| matches!(event, MapEvent::CameraIsChanging)).count();

    // The frame's sample updates the map once.
    let updates = map.update_generation();
    map.set_frame_time(Some(Instant::now()));
    assert_eq!(map.update_generation(), updates + 1);
    map.process_events();
    map.drain_events();
    report(&map, full_frame(&map));
    map.process_events();
    let events = map.drain_events();
    assert_eq!(map.update_generation(), updates + 1);
    assert_eq!(moving(&events), 0, "{events:?}");

    map.set_frame_time(None);
    map.drain_events();
    let updates = map.update_generation();
    report(&map, full_frame(&map));
    map.process_events();
    let events = map.drain_events();
    assert_eq!(map.update_generation(), updates + 1);
    assert_eq!(moving(&events), 1, "{events:?}");
}

#[test]
fn the_reports_waker_runs_for_reports_only() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions::default(), &run_loop);
    map.drain_events();
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    map.set_reports_waker(Some(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })));

    // The map's own call reports its events, which its owner drains.
    map.jump_to(&CameraOptions { zoom: Some(2.0), ..Default::default() });
    assert_eq!(wakes.load(Ordering::SeqCst), 0);
    assert!(!map.drain_events().is_empty());

    report(&map, RendererEvent::Invalidate);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
}

#[test]
fn missing_images_reach_the_owner() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions::default(), &run_loop);
    map.load_style_json(STYLE);
    map.edit_style(|style| style.apply(Edit::AddImage(image("present")))).unwrap();
    map.drain_events();

    report(&map, RendererEvent::StyleImageMissing("present".to_owned()));
    report(&map, RendererEvent::StyleImageMissing("absent".to_owned()));
    let missing: Vec<String> = map
        .drain_events()
        .into_iter()
        .filter_map(|event| match event {
            MapEvent::StyleImageMissing { id } => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(missing, ["absent"]);
}

fn image(id: &str) -> Arc<mln_style::image::Impl> {
    let pixels = PremultipliedImage::new(Size::new(1, 1));
    Arc::new(
        mln_style::image::Impl::new(id.to_owned(), pixels, 1.0, false, Vec::new(), Vec::new(), None, None, None)
            .expect("image"),
    )
}

#[test]
fn the_unused_image_policy_decides_what_stays() {
    let run_loop = Queue::<()>::new();
    for (policy, kept) in [(UnusedStyleImagePolicy::Remove, false), (UnusedStyleImagePolicy::Keep, true)] {
        let mut map = map(MapOptions { unused_style_image_policy: policy, ..Default::default() }, &run_loop);
        map.load_style_json(STYLE);
        map.edit_style(|style| style.apply(Edit::AddImage(image("marker")))).unwrap();
        report(&map, RendererEvent::UnusedStyleImages(vec!["marker".to_owned()]));
        map.process_events();
        assert_eq!(map.style().get_image("marker").is_some(), kept, "{policy:?}");
    }
}

#[test]
fn an_image_that_keeps_itself_outlasts_the_remove_policy() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions::default(), &run_loop);
    map.load_style_json(STYLE);
    let mut kept = (*image("kept")).clone();
    kept.keep_unused = true;
    map.edit_style(|style| style.apply(Edit::AddImage(Arc::new(kept)))).unwrap();
    map.edit_style(|style| style.apply(Edit::AddImage(image("dropped")))).unwrap();
    report(&map, RendererEvent::UnusedStyleImages(vec!["kept".to_owned(), "dropped".to_owned()]));
    map.process_events();
    assert!(map.style().get_image("kept").is_some());
    assert!(map.style().get_image("dropped").is_none());
}

#[test]
fn a_still_image_completes_with_a_full_frame() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions { map_mode: MapMode::Static, ..Default::default() }, &run_loop);
    map.load_style_json(STYLE);
    map.process_events();
    map.render_still().unwrap();

    let stats = Arc::default();
    let frame = |mode, still_image_request_id| RendererEvent::RenderFrameFinished {
        mode,
        needs_repaint: false,
        repaint_at: None,
        placement_changed: false,
        still_image_request_id,
        update_id: 0,
        stats: Arc::clone(&stats),
        error: None,
    };
    report(&map, frame(mln_engine::RenderMode::Partial, 1));
    map.process_events();
    assert_eq!(map.take_still_image(), None);
    report(&map, frame(mln_engine::RenderMode::Full, 1));
    map.process_events();
    assert_eq!(map.take_still_image(), Some(Ok(())));
    assert_eq!(map.take_still_image(), None);
    assert!(map.snapshot().fully_loaded);

    // A full frame of the first image's update, drawn late, doesn't complete
    // the next image, nor does a request while one is on its way.
    map.render_still().unwrap();
    assert!(map.render_still().is_err());
    report(&map, frame(mln_engine::RenderMode::Full, 1));
    map.process_events();
    assert_eq!(map.take_still_image(), None);
    report(&map, frame(mln_engine::RenderMode::Full, 2));
    map.process_events();
    assert_eq!(map.take_still_image(), Some(Ok(())));
}

/// A frame whose tile's wake reaches the map after the frame took the tile
/// in, but before its report, leaves the map idle only once: the frame of
/// the map's answer to the wake does, and the earlier frame doesn't.
#[test]
fn a_continuous_map_goes_idle_once_on_the_frame_of_its_last_update() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions { map_mode: MapMode::Continuous, ..Default::default() }, &run_loop);
    map.load_style_json(STYLE);
    map.process_events();
    map.drain_events();
    let idles = |map: &mut Map| map.drain_events().iter().filter(|event| matches!(event, MapEvent::Idle)).count();
    let frame = full_frame(&map);
    let updates = map.update_generation();
    report(&map, RendererEvent::Invalidate);
    report(&map, frame);
    map.process_events();
    assert_eq!(map.update_generation(), updates + 1, "the wake makes an update");
    assert_eq!(0, idles(&mut map), "a frame of an earlier update left the map idle");
    report(&map, full_frame(&map));
    map.process_events();
    assert_eq!(1, idles(&mut map));
}

/// The report of a frame of `map`'s latest update for the still image
/// `still_image_request_id`, or for none with 0, with everything loaded,
/// that failed on the GPU with `message`, if it did.
fn failed_frame(map: &Map, still_image_request_id: u64, message: Option<&str>) -> RendererEvent {
    RendererEvent::RenderFrameFinished {
        mode: mln_engine::RenderMode::Full,
        needs_repaint: false,
        repaint_at: None,
        placement_changed: false,
        still_image_request_id,
        update_id: latest_update(map),
        stats: Arc::default(),
        error: message.map(|message| Box::new(Error::native(message))),
    }
}

/// The render errors in `events`.
fn render_errors(events: &[MapEvent]) -> Vec<String> {
    let errors = events.iter().filter_map(|event| match event {
        MapEvent::RenderError(error) => Some(error.message.clone()),
        _ => None,
    });
    errors.collect()
}

/// A frame that failed fails the still image it was drawn for, with its
/// error, and no other: not one asked for after that image ended, which a
/// late frame of the first would otherwise fail before it drew, nor one
/// that waits while a frame of the host's own fails. Each failed frame
/// reports its error.
#[test]
fn a_failed_frame_fails_only_the_still_image_it_was_drawn_for() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions { map_mode: MapMode::Static, ..Default::default() }, &run_loop);
    map.load_style_json(STYLE);
    map.process_events();
    map.render_still().unwrap();
    report(&map, failed_frame(&map, 1, Some("the GPU failed")));
    map.process_events();
    let ended = map.take_still_image().expect("the image ended").expect_err("the image succeeded");
    assert_eq!(("the GPU failed", ErrorKind::Native), (ended.message.as_str(), ended.kind));
    assert_eq!(vec!["the GPU failed".to_owned()], render_errors(&map.drain_events()));

    // The first image ends, by a resource that failed, before its frame
    // does; that frame, failed, doesn't fail the next image.
    map.render_still().unwrap();
    report(&map, RendererEvent::ResourceError(Error::native("a tile failed")));
    map.process_events();
    assert!(map.take_still_image().expect("the image ended").is_err());
    map.render_still().unwrap();
    report(&map, failed_frame(&map, 2, Some("late")));
    report(&map, failed_frame(&map, 0, Some("the host's frame")));
    map.process_events();
    assert_eq!(map.take_still_image(), None, "a frame of another image failed this one");
    assert_eq!(vec!["late".to_owned(), "the host's frame".to_owned()], render_errors(&map.drain_events()));
    report(&map, failed_frame(&map, 3, None));
    map.process_events();
    assert_eq!(map.take_still_image(), Some(Ok(())));
}

/// A continuous map reports the first of a run of failed frames only. It
/// draws the first again a little later, and goes idle on the next frame,
/// failed or not, as on a frame that drew whole. A frame that draws without
/// failing ends the run.
#[test]
fn a_continuous_map_reports_a_run_of_failures_once_and_retries_once() {
    let run_loop = Queue::<()>::new();
    let mut map = map(MapOptions { map_mode: MapMode::Continuous, ..Default::default() }, &run_loop);
    map.load_style_json(STYLE);
    map.process_events();
    map.drain_events();
    let idle = |events: &[MapEvent]| events.iter().any(|event| matches!(event, MapEvent::Idle));
    let fail = |map: &mut Map, message| {
        let before = Instant::now();
        report(map, failed_frame(map, 0, Some(message)));
        map.process_events();
        let events = map.drain_events();
        (render_errors(&events), idle(&events), map.repaint_at().map(|at| at - before))
    };

    let (errors, went_idle, retry) = fail(&mut map, "the GPU failed");
    assert_eq!((vec!["the GPU failed".to_owned()], false), (errors, went_idle));
    assert!(retry.is_some_and(|retry| retry > Duration::ZERO && retry < Duration::from_secs(1)), "{retry:?}");
    map.trigger_repaint();
    assert_eq!((Vec::new(), true, None), fail(&mut map, "the GPU failed"));

    report(&map, failed_frame(&map, 0, None));
    map.process_events();
    assert!(idle(&map.drain_events()));
    let (errors, went_idle, retry) = fail(&mut map, "again");
    assert_eq!((vec!["again".to_owned()], false, true), (errors, went_idle, retry.is_some()));
}
