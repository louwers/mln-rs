// Ports platform/default/include/mln/map/map_snapshotter.hpp and
// platform/default/src/mln/map/map_snapshotter.cpp.
//
// Deviation: C++ renders on a renderer thread (`util::Thread<SnapshotterRenderer>`)
// and brings the renderer's observer calls back to the snapshotter's thread
// through an actor (`ForwardingRendererObserver`). The port's renderer and
// HeadlessFrontend are single-threaded (`Rc`), so they run on the
// snapshotter's thread, and the map's events reach the snapshotter's
// observer from a task on its run loop. The still image is read in the
// map's still image callback, at the end of the frame that completes it.
// The snapshot callback arrives later, from the run loop, as C++'s actor
// delivers it; the port gathers the attributions and transforms there
// (C++: in the still image callback, on the same thread just before), since
// `Map::render_still` calls its callback while the map is borrowed.
// TODO(port): annotations (`addAnnotationImage`, `addAnnotation`), with
// Map::addAnnotation*; `localFontFamily`.

use std::cell::{RefCell, RefMut};
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::AsyncTask;
use mln_map::transform::Transform;
use mln_map::{CameraOptions, Map, MapEvent, MapMode, MapOptions};
use mln_storage::{FileSourceManager, ResourceOptions};
use mln_util::error::Error;
use mln_util::geo::{EdgeInsets, LatLng, LatLngBounds, ScreenCoordinate, WrapMode};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::HeadlessFrontend;

/// `MapSnapshotterObserver`.
pub trait MapSnapshotterObserver {
    fn on_did_fail_loading_style(&self, _error: &str) {}
    fn on_did_finish_loading_style(&self) {}
    fn on_style_image_missing(&self, _id: &str) {}
}

/// `MapSnapshotterObserver::nullObserver()`.
pub struct NullMapSnapshotterObserver;

impl MapSnapshotterObserver for NullMapSnapshotterObserver {}

/// `MapSnapshotter::PointForFn`.
pub type PointForFn = Box<dyn Fn(&LatLng) -> ScreenCoordinate>;
/// `MapSnapshotter::LatLngForFn`.
/// `Err`: the `std::domain_error` of the C++ `LatLng` constructor.
pub type LatLngForFn = Box<dyn Fn(&ScreenCoordinate) -> Result<LatLng, Error>>;
/// `MapSnapshotter::Attributions`.
pub type Attributions = Vec<String>;
/// `MapSnapshotter::Callback` (a `std::function`: `snapshot` may call it
/// twice, see there).
pub type Callback = Box<dyn FnMut(Option<Error>, PremultipliedImage, Attributions, PointForFn, LatLngForFn)>;

/// The outcome of a still image: its error, or its image.
type StillImageResult = Result<PremultipliedImage, Error>;

/// Hands the map's events to the snapshotter's observer.
fn forward_events(map: &RefCell<Map>, observer: &dyn MapSnapshotterObserver) {
    // A caller that holds the map gets its events with the next ones.
    let Ok(mut map) = map.try_borrow_mut() else {
        return;
    };
    let events = map.drain_events();
    drop(map);
    for event in events {
        match event {
            MapEvent::LoadingFailed(error) => observer.on_did_fail_loading_style(error.message()),
            MapEvent::StyleLoaded => observer.on_did_finish_loading_style(),
            MapEvent::StyleImageMissing { id } => observer.on_style_image_missing(&id),
            _ => {}
        }
    }
}

/// `MapSnapshotter`.
pub struct MapSnapshotter {
    /// `renderStillCallback`: the callback of the snapshot being rendered.
    render_still_callback: Rc<RefCell<Option<Callback>>>,
    /// The outcome of the still image the delivery task hands over.
    still_image_result: Rc<RefCell<Option<StillImageResult>>>,
    deliver: AsyncTask,
    /// Hands the map's events to the observer.
    _forward_events: AsyncTask,
    frontend: HeadlessFrontend,
    map: Rc<RefCell<Map>>,
    region: LatLngBounds,
    region_insets: EdgeInsets,
}

impl MapSnapshotter {
    /// `MapSnapshotter(size, pixelRatio, resourceOptions, observer)`, with
    /// file sources of its own.
    pub fn with_observer(
        size: Size,
        pixel_ratio: f32,
        resource_options: &ResourceOptions,
        observer: Rc<dyn MapSnapshotterObserver>,
    ) -> Result<Self, Error> {
        let frontend = HeadlessFrontend::with_options(size, pixel_ratio, true)?;
        let map = Map::with_resource_options(
            MapOptions::new().with_map_mode(MapMode::Static).with_size(size).with_pixel_ratio(pixel_ratio),
            &FileSourceManager::new(),
            resource_options,
        )?;
        frontend.attach(&map);
        let map = Rc::new(RefCell::new(map));
        let forward = {
            let map = Rc::downgrade(&map);
            AsyncTask::new(move || {
                if let Some(map) = map.upgrade() {
                    forward_events(&map, &*observer);
                }
            })
        };
        let sender = forward.sender();
        map.borrow_mut().set_waker(Some(Arc::new(move || sender.send())));

        let render_still_callback: Rc<RefCell<Option<Callback>>> = Rc::new(RefCell::new(None));
        let still_image_result: Rc<RefCell<Option<StillImageResult>>> = Rc::new(RefCell::new(None));
        let deliver = {
            let render_still_callback = render_still_callback.clone();
            let still_image_result = still_image_result.clone();
            let map = Rc::downgrade(&map);
            let frontend = frontend.clone();
            AsyncTask::new(move || {
                let Some(result) = still_image_result.borrow_mut().take() else {
                    return;
                };
                let Some(map) = map.upgrade() else {
                    return;
                };
                let (point_for_fn, lat_lng_for_fn, attributions) = {
                    let map = map.borrow();
                    let center = map.get_camera_options(None).center.expect("the camera's center");

                    // Create lambda that captures the current transform state
                    // and can be used to translate for geographic to screen
                    // coordinates
                    let transform_state = frontend.get_transform_state().unwrap_or_default();
                    let point_for_fn: PointForFn = Box::new(move |lat_lng| {
                        let mut unwrapped_lat_lng = lat_lng.wrapped();
                        unwrapped_lat_lng.unwrap_for_shortest_path(&center);
                        Transform::from_state(transform_state.clone()).lat_lng_to_screen_coordinate(&unwrapped_lat_lng)
                    });

                    // Create lambda that captures the current transform state
                    // and can be used to translate for geographic to screen
                    // coordinates
                    let transform_state = frontend.get_transform_state().unwrap_or_default();
                    let lat_lng_for_fn: LatLngForFn = Box::new(move |screen_coordinate| {
                        Transform::from_state(transform_state.clone())
                            .screen_coordinate_to_lat_lng(screen_coordinate, WrapMode::Wrapped)
                    });

                    // Collect all source attributions
                    let attributions: Attributions =
                        map.style().get_sources().iter().filter_map(|source| source.get_attribution()).collect();
                    (point_for_fn, lat_lng_for_fn, attributions)
                };

                let (error, image) = match result {
                    Ok(image) => (None, image),
                    Err(error) => (Some(error), PremultipliedImage::new(Size::default())),
                };

                // A canceled snapshot has no callback left.
                let callback = render_still_callback.borrow_mut().take();
                if let Some(mut callback) = callback {
                    callback(error, image, attributions, point_for_fn, lat_lng_for_fn);
                }
            })
        };

        Ok(MapSnapshotter {
            render_still_callback,
            still_image_result,
            deliver,
            _forward_events: forward,
            frontend,
            map,
            region: LatLngBounds::empty(),
            region_insets: EdgeInsets::default(),
        })
    }

    /// `MapSnapshotter(size, pixelRatio, resourceOptions)`. `Err` where the
    /// C++ Map constructor throws (an empty size).
    pub fn new(size: Size, pixel_ratio: f32, resource_options: &ResourceOptions) -> Result<Self, Error> {
        Self::with_observer(size, pixel_ratio, resource_options, Rc::new(NullMapSnapshotterObserver))
    }

    /// The map, for `getStyle()`: `snapshotter.map().style_mut()`.
    #[must_use]
    pub fn map(&self) -> RefMut<'_, Map> {
        self.map.borrow_mut()
    }

    pub fn set_style_url(&self, style_url: &str) {
        self.map().style_mut().load_url(style_url);
    }

    #[must_use]
    pub fn get_style_url(&self) -> String {
        self.map.borrow().style().get_url()
    }

    pub fn set_style_json(&self, style_json: &str) {
        self.map().style_mut().load_json(style_json);
    }

    #[must_use]
    pub fn get_style_json(&self) -> String {
        self.map.borrow().style().get_json()
    }

    pub fn set_size(&self, size: Size) {
        // C++ ignores Map::setSize's result here too.
        let _ = self.map().set_size(size);
        self.frontend.clone().set_size(size);
    }

    #[must_use]
    pub fn get_size(&self) -> Size {
        self.map.borrow().get_map_options().size()
    }

    /// `setCameraOptions(options)`: the region padding applies unless the
    /// options set their own.
    pub fn set_camera_options(&self, options: &CameraOptions) {
        let mut options_with_padding = *options;
        if options.padding.is_none() {
            // Apply region padding if no padding is explicitly set in camera options
            options_with_padding = options_with_padding.with_padding(Some(self.region_insets));
        }
        self.map().jump_to(&options_with_padding);
    }

    #[must_use]
    pub fn get_camera_options(&self) -> CameraOptions {
        self.map.borrow().get_camera_options(None)
    }

    /// Fails as [`Map::camera_for_lat_lngs`] does.
    pub fn set_region(&mut self, region: &LatLngBounds) -> Result<(), Error> {
        self.region = *region;
        self.jump_to_region()
    }

    /// Fails as [`Map::lat_lng_bounds_for_camera`] does.
    pub fn get_region(&self) -> Result<LatLngBounds, Error> {
        let map = self.map.borrow();
        map.lat_lng_bounds_for_camera(&map.get_camera_options(Some(self.get_padding())))
    }

    /// Fails as [`Map::camera_for_lat_lngs`] does.
    pub fn set_padding(&mut self, insets: &EdgeInsets) -> Result<(), Error> {
        self.region_insets = *insets;
        if !self.region.is_empty() {
            self.jump_to_region()?;
        }
        Ok(())
    }

    #[must_use]
    pub fn get_padding(&self) -> EdgeInsets {
        self.region_insets
    }

    fn jump_to_region(&self) -> Result<(), Error> {
        let lat_lngs = [self.region.southwest(), self.region.northeast()];
        let camera = self.map.borrow().camera_for_lat_lngs(&lat_lngs, &self.region_insets, None, None)?;
        self.map().jump_to(&camera);
        Ok(())
    }

    /// `snapshot(callback)`: renders a still image of the map and calls
    /// `callback` with it from the run loop.
    pub fn snapshot(&self, mut callback: Callback) {
        if self.render_still_callback.borrow().is_some() {
            callback(
                Some(Error::invalid_state("MapSnapshotter is currently rendering an image")),
                PremultipliedImage::new(Size::default()),
                Vec::new(),
                Box::new(|_| ScreenCoordinate::default()),
                Box::new(|_| Ok(LatLng::default())),
            );
            // C++ goes on to replace the current snapshot's callback with
            // this one, which is then called again with the result.
        }
        *self.render_still_callback.borrow_mut() = Some(callback);

        let still_image_result = self.still_image_result.clone();
        let deliver = self.deliver.sender();
        let frontend = self.frontend.clone();
        self.map().render_still(Box::new(move |result| {
            *still_image_result.borrow_mut() = Some(result.map(|()| frontend.read_still_image()));
            deliver.send();
        }));
    }

    /// `cancel()`: the current snapshot's callback is not called.
    pub fn cancel(&self) {
        self.render_still_callback.borrow_mut().take();
    }
}
