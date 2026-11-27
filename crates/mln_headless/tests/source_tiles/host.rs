//! A host of one map: a runtime whose resource provider is the test's
//! [`Network`], a map of `SIZE` with an offscreen session, and the map's
//! tile events.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mln_engine::RenderMode;
use mln_engine::camera::{CameraOptions, CameraUpdate};
use mln_engine::map::{MapEvent, MapMode};
use mln_engine::source::TileOperation;
use mln_engine::storage::{Resource, Response};
use mln_engine::tile::OverscaledTileID;
use mln_runtime::{
    Applied, AttachOptions, BackendType, Completion, DemandMode, Device, DeviceOptions, Error, EventMask, FrameDemand,
    FrameDisposition, FrameResult, Map, MapOptions, RenderSession, RenderTarget, ResourceRequest, Runtime,
    RuntimeEventKind, RuntimeOptions, SourceQueryOptions,
};
use mln_style::Edit;
use mln_style::geo::LatLng;
use mln_style::size::Size;
use parking_lot::{Condvar, Mutex};

const SIZE: Size = Size { width: 64, height: 64 };

/// How long a wait for something the test expects may take before the test
/// fails as hung.
const DEADLINE: Duration = Duration::from_secs(30);

/// Waits on `changed` until `ready` gives a value for what `state` holds.
pub(crate) fn wait_until<S, T>(
    state: &Mutex<S>,
    changed: &Condvar,
    what: &str,
    mut ready: impl FnMut(&mut S) -> Option<T>,
) -> T {
    let deadline = Instant::now() + DEADLINE;
    let mut state = state.lock();
    loop {
        if let Some(value) = ready(&mut state) {
            return value;
        }
        assert!(!changed.wait_until(&mut state, deadline).timed_out(), "waited in vain for {what}");
    }
}

pub(crate) fn wait<T: Send + 'static>(completion: Result<Completion<T>, Error>) -> T {
    completion.unwrap().wait().unwrap()
}

/// A request the provider saw.
struct Seen {
    resource: Resource,
    /// The request, until the test answers it.
    request: Option<ResourceRequest>,
}

/// What a [`Network`] answers each request with at once; `None` holds it
/// for the test.
type Answer = Box<dyn Fn(&Resource) -> Option<Response> + Send>;

/// The map's network: a resource provider that records every request, and
/// answers it as [`Network::answer_with`] says or holds it until the test
/// answers it.
#[derive(Default)]
pub(crate) struct Network {
    seen: Mutex<(Vec<Seen>, Option<Answer>)>,
    changed: Condvar,
}

impl Network {
    fn serve(&self, request: ResourceRequest) {
        let mut seen = self.seen.lock();
        let response = seen.1.as_ref().and_then(|answer| answer(request.resource()));
        let resource = request.resource().clone();
        let request = match response {
            Some(response) => {
                request.respond(response).unwrap();
                None
            }
            None => Some(request),
        };
        seen.0.push(Seen { resource, request });
        self.changed.notify_all();
    }

    /// Answers each request from now on with what `answer` gives it, or
    /// holds it when that's `None`.
    pub(crate) fn answer_with(&self, answer: impl Fn(&Resource) -> Option<Response> + Send + 'static) {
        self.seen.lock().1 = Some(Box::new(answer));
    }

    /// Holds each request from now on until the test answers it.
    pub(crate) fn hold(&self) {
        self.seen.lock().1 = None;
    }

    /// The requests for `url` so far, oldest first, each with whether the
    /// map cancelled it while it waited for an answer.
    pub(crate) fn requests(&self, url: &str) -> Vec<(Resource, bool)> {
        let seen = self.seen.lock();
        let of_url = seen.0.iter().filter(|seen| seen.resource.url == url);
        of_url
            .map(|seen| (seen.resource.clone(), seen.request.as_ref().is_some_and(ResourceRequest::is_cancelled)))
            .collect()
    }

    /// Waits until the map has requested `url` `count` times.
    pub(crate) fn wait_for(&self, url: &str, count: usize) {
        wait_until(&self.seen, &self.changed, &format!("request {count} of {url}"), |(seen, _)| {
            (seen.iter().filter(|seen| seen.resource.url == url).count() >= count).then_some(())
        });
    }

    /// Answers the oldest held request for `url` with `response`, once the
    /// map has made one. Returns what it asked for.
    pub(crate) fn answer(&self, url: &str, response: Response) -> Resource {
        let (resource, request) = wait_until(&self.seen, &self.changed, &format!("a request of {url}"), |(seen, _)| {
            let seen = seen.iter_mut().find(|seen| seen.resource.url == url && seen.request.is_some())?;
            Some((seen.resource.clone(), seen.request.take()?))
        });
        request.respond(response).unwrap();
        resource
    }
}

/// The map's events, as the runtime reported them.
#[derive(Default)]
struct Events {
    events: Mutex<Vec<MapEvent>>,
    changed: Condvar,
}

/// A host's map, its session and its network.
pub(crate) struct Host {
    pub(crate) map: Map,
    pub(crate) session: RenderSession,
    pub(crate) network: Arc<Network>,
    runtime: Runtime,
    events: Arc<Events>,
    /// How many of the events the test has read.
    read: Mutex<usize>,
    /// Whether frame results wait.
    frames: Arc<(Mutex<bool>, Condvar)>,
    /// The map update that shows the test's last command.
    update: AtomicU64,
}

impl Host {
    /// A map in `mode` over a runtime of the built-ins, whose style is
    /// `style`.
    pub(crate) fn new(mode: MapMode, style: &str) -> Host {
        let runtime = Runtime::new(&RuntimeOptions::default()).unwrap();
        let network = Arc::new(Network::default());
        let provider = Arc::clone(&network);
        wait(runtime.set_resource_provider(move |request| provider.serve(request)));
        let events = Arc::new(Events::default());
        let recorder = Arc::clone(&events);
        wait(runtime.set_event_handler(move |batch| {
            let mut events = recorder.events.lock();
            events.extend(batch.iter().filter_map(|event| match &event.kind {
                RuntimeEventKind::Map(event) => Some(event.clone()),
                _ => None,
            }));
            recorder.changed.notify_all();
        }));
        let map = mln_engine::map::MapOptions { map_mode: mode, size: SIZE, ..Default::default() };
        let options = MapOptions { map, event_mask: EventMask::ALL_MAP_EVENTS, ..Default::default() };
        let (map, created) = runtime.create_map(options).unwrap();
        created.wait().unwrap();
        let update = AtomicU64::new(wait(map.set_style_json(style)).render_update_generation);
        let device = Device::with_backend(backend(), DeviceOptions::default()).unwrap();
        let target = RenderTarget::Offscreen { size: SIZE, device: Some(device) };
        let frames: Arc<(Mutex<bool>, Condvar)> = Arc::default();
        let woken = Arc::clone(&frames);
        let frame_wake = Arc::new(move || {
            *woken.0.lock() = true;
            woken.1.notify_all();
        });
        let (session, attached) =
            map.attach(target, AttachOptions { frame_wake: Some(frame_wake), ..Default::default() }).unwrap();
        attached.wait().unwrap();
        Host { map, session, network, runtime, events, read: Mutex::new(0), frames, update }
    }

    /// Waits for a command to the map, whose change the next frame shows.
    pub(crate) fn command(&self, completion: Result<Completion<Applied>, Error>) {
        self.update.fetch_max(wait(completion).render_update_generation, Ordering::SeqCst);
    }

    /// Renders a frame of a continuous map that shows the test's commands,
    /// once the map has reported every event up to the frame's end.
    pub(crate) fn frame(&self) -> FrameResult {
        let frame = loop {
            self.session.demand_frame(FrameDemand::new(DemandMode::Force, 0)).unwrap();
            let frame = wait_until(&self.frames.0, &self.frames.1, "a frame", |woken| {
                std::mem::take(woken).then(|| self.session.drain_frame_results().unwrap().pop()).flatten()
            });
            assert_eq!(FrameDisposition::Rendered, frame.disposition);
            // A frame can come before the update of a command that returned.
            if frame.map_update_generation >= self.update.load(Ordering::SeqCst) {
                break frame;
            }
        };
        // Only the test's demands render, and each reports its end last.
        wait_until(&self.events.events, &self.events.changed, "the frame's end", |events| {
            let ends = events.iter().filter(|event| matches!(event, MapEvent::RenderFrameFinished(_))).count();
            (ends as u64 >= frame.frame_generation).then_some(())
        });
        frame
    }

    /// Renders a continuous map until a frame shows everything it needs
    /// loaded.
    pub(crate) fn settle(&self) {
        let deadline = Instant::now() + DEADLINE;
        while {
            let frame = self.frame();
            frame.mode != RenderMode::Full || frame.needs_repaint
        } {
            assert!(Instant::now() < deadline, "the map never loaded");
        }
    }

    /// Applies `edit` to the map's style, which the next frame or still
    /// image shows.
    pub(crate) fn edit(&self, edit: Edit) {
        wait(self.map.with_core(move |map| map.edit_style(|style| style.apply(edit).map(drop)))).unwrap();
        if self.map.mode() == MapMode::Continuous {
            self.command(self.map.request_repaint());
        }
    }

    /// Renders a still image of a static map.
    pub(crate) fn still(&self) -> Result<(), Error> {
        self.map.request_still_image().unwrap().wait()
    }

    /// Moves the map's center to `longitude` on the equator, at `zoom`.
    pub(crate) fn move_to(&self, longitude: f64, zoom: f64) {
        let camera = CameraOptions { center: LatLng::new(0.0, longitude).ok(), zoom: Some(zoom), ..Default::default() };
        self.command(self.map.update_camera(CameraUpdate { camera, ..Default::default() }));
    }

    /// What tile `id`'s loads went through, as the map reported since the
    /// last call.
    pub(crate) fn actions(&self, id: OverscaledTileID) -> Vec<TileOperation> {
        let events = self.events.events.lock();
        let mut read = self.read.lock();
        let actions = events[*read..].iter().filter_map(|event| match event {
            MapEvent::TileAction { op, id: tile, .. } if *tile == id => Some(*op),
            _ => None,
        });
        let actions = actions.collect();
        *read = events.len();
        actions
    }

    /// How many features of `source`, or of its source layer `layer`, the
    /// map loaded.
    pub(crate) fn features(&self, source: &str, layer: Option<&str>) -> usize {
        let options = SourceQueryOptions::new(layer.map(|layer| vec![layer.to_owned()]), None);
        wait(self.session.query_source_features(source, options)).len()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        wait(self.session.release());
        wait(self.map.release());
        if let Ok(released) = self.runtime.release() {
            let _ = released.wait();
        }
    }
}

/// The backend that `MLN_RENDER_BACKEND` names, or WebGPU.
fn backend() -> BackendType {
    match std::env::var("MLN_RENDER_BACKEND").map(|backend| backend.to_ascii_lowercase()).as_deref() {
        Ok("metal") => BackendType::Metal,
        Ok("vulkan") => BackendType::Vulkan,
        Ok("d3d12") => BackendType::D3D12,
        Ok("gl") => BackendType::OpenGL,
        Ok("webgpu") | Err(_) => BackendType::WebGPU,
        Ok(other) => panic!("MLN_RENDER_BACKEND names no backend: {other}"),
    }
}

/// One MVT tile with a layer "points" of one point feature at the tile's
/// center.
const POINT_TILE: &[u8] = &[
    0x1a, 0x1a, 0x78, 0x02, 0x0a, 0x06, b'p', b'o', b'i', b'n', b't', b's', 0x12, 0x0b, 0x08, 0x01, 0x18, 0x01, 0x22,
    0x05, 0x09, 0x80, 0x20, 0x80, 0x20, 0x28, 0x80, 0x20,
];

/// A response with [`POINT_TILE`], whose `ETag` is "a".
pub(crate) fn point_tile() -> Response {
    let mut response = Response::new();
    response.data = Some(Arc::new(POINT_TILE.to_vec()));
    response.etag = Some("a".to_owned());
    response
}
