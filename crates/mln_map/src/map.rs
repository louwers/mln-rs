// Ports include/mln/map/map.hpp and src/mln/map/map.cpp (the parts M1 runs).

use std::cell::Ref;
use std::rc::Rc;
use std::sync::Arc;

use mln_map_types::{
    AnimationOptions, BoundOptions, CameraOptions, ConstrainMode, MapDebugOptions, MapMode, NorthOrientation,
    ProjectionMode, TileLodMode, TransformState, ViewportMode,
};
use mln_renderer::{ExceptionPtr, RendererFrontend};
use mln_storage::{ClientOptions, FileSource, FileSourceManager, FileSourceType, ResourceOptions};
use mln_style::Style;
use mln_util::exception::MisuseException;
use mln_util::geo::{EdgeInsets, LatLng, LatLngBounds, ScreenCoordinate, WrapMode};
use mln_util::size::Size;

use crate::map_impl::{MapImpl, StillImageCallback, StillImageRequest, StyleMut};
use crate::map_observer::MapObserver;
use crate::map_options::MapOptions;

pub struct Map {
    impl_: Rc<MapImpl>,
}

impl Map {
    /// `Map(RendererFrontend&, MapObserver&, const MapOptions&)` with the
    /// default `ResourceOptions()` and `ClientOptions()`.
    ///
    /// C++ keeps a reference to the frontend; frontends are handles here
    /// (`Clone` shares the same renderer), so the map keeps a clone.
    ///
    /// `Err` where C++ throws: "failed to resize: size is empty" for an
    /// empty `map_options.size()`.
    pub fn new<F>(frontend: &F, observer: Rc<dyn MapObserver>, map_options: MapOptions) -> Result<Map, String>
    where
        F: RendererFrontend + Clone + 'static,
    {
        Self::with_resource_options(
            frontend,
            observer,
            map_options,
            &ResourceOptions::default(),
            &ClientOptions::default(),
        )
    }

    /// `Map(RendererFrontend&, MapObserver&, const MapOptions&, const
    /// ResourceOptions&, const ClientOptions&)`: the file source is the
    /// FileSourceManager's `ResourceLoader` for these options.
    // TODO(port): ActionJournalOptions.
    pub fn with_resource_options<F>(
        frontend: &F,
        observer: Rc<dyn MapObserver>,
        map_options: MapOptions,
        resource_options: &ResourceOptions,
        client_options: &ClientOptions,
    ) -> Result<Map, String>
    where
        F: RendererFrontend + Clone + 'static,
    {
        let file_source =
            FileSourceManager::get().get_file_source(FileSourceType::ResourceLoader, resource_options, client_options);
        Ok(Map { impl_: MapImpl::new(Box::new(frontend.clone()), observer, file_source, &map_options)? })
    }

    // gtests-core: the C++ tests' `MapAdapter` (test/include/mln/test/
    // map_adapter.hpp), i.e. `Map(std::make_unique<Map::Impl>(frontend,
    // observer, fileSource, options))`: a map over a given file source.
    /// `Map(std::unique_ptr<Impl>)` with `Impl(frontend, observer, fileSource, mapOptions)`.
    pub fn with_file_source<F>(
        frontend: &F,
        observer: Rc<dyn MapObserver>,
        file_source: Arc<dyn FileSource>,
        map_options: MapOptions,
    ) -> Result<Map, String>
    where
        F: RendererFrontend + Clone + 'static,
    {
        Ok(Map { impl_: MapImpl::new(Box::new(frontend.clone()), observer, Some(file_source), &map_options)? })
    }

    /// `getClientOptions()`.
    pub fn get_client_options(&self) -> ClientOptions {
        match &self.impl_.file_source {
            Some(file_source) => file_source.get_client_options(),
            None => ClientOptions::default(),
        }
    }

    /// Register a callback that will get called (on the render thread) when
    /// all resources have been loaded and a complete render occurs.
    pub fn render_still(&mut self, callback: StillImageCallback) {
        let impl_ = &self.impl_;
        if impl_.mode != MapMode::Static && impl_.mode != MapMode::Tile {
            callback(Some(misuse("Map is not in static or tile image render modes")));
            return;
        }

        if impl_.still_image_request.borrow().is_some() {
            callback(Some(misuse("Map is currently rendering an image")));
            return;
        }

        let last_error = impl_.style.borrow().impl_.get_last_error();
        if let Some(error) = last_error {
            callback(Some(error));
            return;
        }

        *impl_.still_image_request.borrow_mut() = Some(StillImageRequest { callback });

        impl_.on_update();
    }

    /// `renderStill(const CameraOptions&, MapDebugOptions, StillImageCallback)`.
    pub fn render_still_with_camera(
        &mut self,
        camera: &CameraOptions,
        debug_options: MapDebugOptions,
        callback: StillImageCallback,
    ) {
        self.impl_.camera_mutated.set(true);
        self.impl_.debug_options.set(debug_options);
        self.impl_.transform.borrow_mut().jump_to(camera);
        self.render_still(callback);
    }

    /// Triggers a repaint.
    pub fn trigger_repaint(&mut self) {
        self.impl_.on_update();
    }

    // MARK: - Style

    /// `const style::Style& getStyle() const`.
    pub fn get_style(&self) -> Ref<'_, Style> {
        self.impl_.style.borrow()
    }

    /// `style::Style& getStyle()`: a guard that derefs to `&mut Style`; the
    /// style's observer calls made through it reach the map when it drops.
    pub fn get_style_mut(&mut self) -> StyleMut<'_> {
        StyleMut::new(&self.impl_)
    }

    // TODO(port): setStyle(std::unique_ptr<style::Style>).

    // MARK: - Transitions

    pub fn cancel_transitions(&mut self) {
        self.impl_.transform.borrow_mut().cancel_transitions();
        self.impl_.on_update();
    }

    /// `cancelTransitions(uint64_t transitionId)`: cancels the commands whose
    /// [`AnimationOptions::transition_id`] equals `transition_id`, and leaves
    /// the others running. Requests an update only when a command ended.
    pub fn cancel_transitions_with_id(&mut self, transition_id: u64) {
        let ended = self.impl_.transform.borrow_mut().cancel_transitions_with_id(transition_id);
        if ended {
            self.impl_.on_update();
        }
    }

    pub fn set_gesture_in_progress(&mut self, in_progress: bool) {
        self.impl_.transform.borrow_mut().set_gesture_in_progress(in_progress);
        self.impl_.on_update();
    }

    pub fn is_gesture_in_progress(&self) -> bool {
        self.impl_.transform.borrow().is_gesture_in_progress()
    }

    pub fn is_rotating(&self) -> bool {
        self.impl_.transform.borrow().is_rotating()
    }

    pub fn is_scaling(&self) -> bool {
        self.impl_.transform.borrow().is_scaling()
    }

    pub fn is_panning(&self) -> bool {
        self.impl_.transform.borrow().is_panning()
    }

    /// Whether any camera command is still animating. A command's
    /// `MapObserver::on_camera_did_change` ends that command only, so this
    /// tells whether the camera stopped once the last one ended.
    pub fn in_transition(&self) -> bool {
        self.impl_.transform.borrow().in_transition()
    }

    // MARK: - Camera

    pub fn get_camera_options(&self, padding: Option<EdgeInsets>) -> CameraOptions {
        self.impl_.transform.borrow().get_camera_options(padding)
    }

    pub fn jump_to(&mut self, camera: &CameraOptions) {
        self.impl_.jump_to(camera);
    }

    // query: the render-test runner's tile mode uses it.
    /// `latLngBoundsForCamera(camera)`.
    /// `Err`: the std::domain_error of the C++ LatLng constructor, for a
    /// camera whose corners have no LatLng.
    pub fn lat_lng_bounds_for_camera(&self, camera: &CameraOptions) -> Result<mln_util::geo::LatLngBounds, String> {
        let mut shallow = crate::transform::Transform::from_state(self.impl_.transform.borrow().get_state().clone());
        let size = shallow.get_state().get_size();

        shallow.jump_to(camera);
        Ok(mln_util::geo::LatLngBounds::hull(
            shallow.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(0.0, 0.0), mln_util::geo::WrapMode::Wrapped)?,
            shallow.screen_coordinate_to_lat_lng(
                &ScreenCoordinate::new(f64::from(size.width), f64::from(size.height)),
                mln_util::geo::WrapMode::Wrapped,
            )?,
        ))
    }

    pub fn ease_to(&mut self, camera: &CameraOptions, animation: &AnimationOptions) {
        self.impl_.camera_mutated.set(true);
        self.impl_.transform.borrow_mut().ease_to(camera, animation);
        self.impl_.on_update();
    }

    pub fn fly_to(&mut self, camera: &CameraOptions, animation: &AnimationOptions) {
        self.impl_.camera_mutated.set(true);
        self.impl_.transform.borrow_mut().fly_to(camera, animation, false);
        self.impl_.on_update();
    }

    /// `Err`: the std::domain_error of the C++ LatLng constructor (a NaN or
    /// infinite point).
    pub fn move_by(&mut self, point: &ScreenCoordinate, animation: &AnimationOptions) -> Result<(), String> {
        self.impl_.camera_mutated.set(true);
        self.impl_.transform.borrow_mut().move_by(point, animation)?;
        self.impl_.on_update();
        Ok(())
    }

    pub fn pitch_by(&mut self, pitch: f64, animation: &AnimationOptions) {
        let current = mln_math::rad2deg(self.impl_.transform.borrow().get_pitch());
        self.ease_to(&CameraOptions::default().with_pitch(Some(current - pitch)), animation);
    }

    pub fn scale_by(&mut self, scale: f64, anchor: Option<ScreenCoordinate>, animation: &AnimationOptions) {
        let zoom = {
            let transform = self.impl_.transform.borrow();
            transform.get_zoom() + transform.get_state().scale_zoom(scale)
        };
        self.ease_to(&CameraOptions::default().with_zoom(Some(zoom)).with_anchor(anchor), animation);
    }

    pub fn rotate_by(&mut self, first: &ScreenCoordinate, second: &ScreenCoordinate, animation: &AnimationOptions) {
        self.impl_.camera_mutated.set(true);
        self.impl_.transform.borrow_mut().rotate_by(first, second, animation);
        self.impl_.on_update();
    }

    // gtests-core: cameraForLatLngBounds, cameraForLatLngs,
    // latLngBoundsForCameraUnwrapped (test/map/map.test.cpp).
    /// `cameraForLatLngBounds(bounds, padding, bearing, pitch)`. `Err`: the
    /// std::domain_error of the C++ LatLng constructor, for a center that
    /// has none (padding that pushes it to infinity).
    pub fn camera_for_lat_lng_bounds(
        &self,
        bounds: &LatLngBounds,
        padding: &EdgeInsets,
        bearing: Option<f64>,
        pitch: Option<f64>,
    ) -> Result<CameraOptions, String> {
        self.camera_for_lat_lngs(
            &[bounds.northwest(), bounds.southwest(), bounds.southeast(), bounds.northeast()],
            padding,
            bearing,
            pitch,
        )
    }

    /// `cameraForLatLngs(latLngs, padding, bearing, pitch)`. `Err`: as
    /// [`camera_for_lat_lng_bounds`](Self::camera_for_lat_lng_bounds).
    pub fn camera_for_lat_lngs(
        &self,
        lat_lngs: &[LatLng],
        padding: &EdgeInsets,
        bearing: Option<f64>,
        pitch: Option<f64>,
    ) -> Result<CameraOptions, String> {
        if bearing.is_none() && pitch.is_none() {
            return camera_for_lat_lngs(lat_lngs, &self.impl_.transform.borrow(), padding);
        }

        let mut transform = crate::transform::Transform::from_state(self.impl_.transform.borrow().get_state().clone());

        transform.jump_to(&CameraOptions::default().with_bearing(bearing).with_pitch(pitch));

        Ok(camera_for_lat_lngs(lat_lngs, &transform, padding)?
            .with_bearing(Some(mln_math::rad2deg(-transform.get_bearing())))
            .with_pitch(Some(mln_math::rad2deg(transform.get_pitch()))))
    }

    // TODO(port): cameraForGeometry.

    /// `latLngBoundsForCameraUnwrapped(camera)`.
    /// `Err`: as [`lat_lng_bounds_for_camera`](Self::lat_lng_bounds_for_camera).
    pub fn lat_lng_bounds_for_camera_unwrapped(&self, camera: &CameraOptions) -> Result<LatLngBounds, String> {
        let mut shallow = crate::transform::Transform::from_state(self.impl_.transform.borrow().get_state().clone());
        let size = shallow.get_state().get_size();

        shallow.jump_to(camera);
        let width = f64::from(size.width);
        let height = f64::from(size.height);
        let mut nw = shallow.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(0.0, 0.0), WrapMode::Wrapped)?;
        let mut se = shallow.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(width, height), WrapMode::Wrapped)?;
        let mut ne = shallow.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(width, 0.0), WrapMode::Wrapped)?;
        let mut sw = shallow.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(0.0, height), WrapMode::Wrapped)?;
        let center = shallow
            .screen_coordinate_to_lat_lng(&ScreenCoordinate::new(width / 2.0, height / 2.0), WrapMode::Wrapped)?;
        nw.unwrap_for_shortest_path(&center);
        se.unwrap_for_shortest_path(&center);
        ne.unwrap_for_shortest_path(&center);
        sw.unwrap_for_shortest_path(&center);
        let mut bounds = LatLngBounds::hull(nw, se);
        bounds.extend(&ne);
        bounds.extend(&sw);
        bounds.extend(&center);
        Ok(bounds)
    }

    // MARK: - Bounds

    pub fn set_bounds(&mut self, options: &BoundOptions) -> Result<(), String> {
        let mut change_camera = false;
        let mut camera_options = CameraOptions::default();

        {
            let mut transform = self.impl_.transform.borrow_mut();

            if let Some(bounds) = options.bounds {
                change_camera = true;
                transform.set_lat_lng_bounds(bounds)?;
            }

            if let Some(min_zoom) = options.min_zoom {
                transform.set_min_zoom(min_zoom);
                if transform.get_zoom() < min_zoom {
                    change_camera = true;
                    camera_options = camera_options.with_zoom(options.min_zoom);
                }
            }

            if let Some(max_zoom) = options.max_zoom {
                transform.set_max_zoom(max_zoom);
                if transform.get_zoom() > max_zoom {
                    change_camera = true;
                    camera_options = camera_options.with_zoom(options.max_zoom);
                }
            }

            if let Some(max_pitch) = options.max_pitch {
                transform.set_max_pitch(max_pitch);
                if transform.get_pitch() > transform.get_state().get_max_pitch() {
                    change_camera = true;
                    camera_options = camera_options.with_pitch(options.max_pitch);
                }
            }

            if let Some(min_pitch) = options.min_pitch {
                transform.set_min_pitch(min_pitch);
                if transform.get_pitch() < transform.get_state().get_min_pitch() {
                    change_camera = true;
                    camera_options = camera_options.with_pitch(options.min_pitch);
                }
            }
        }

        if change_camera {
            self.jump_to(&camera_options);
        }
        Ok(())
    }

    /// Returns the current map bound options. All optional fields in
    /// BoundOptions are set.
    pub fn get_bounds(&self) -> BoundOptions {
        let transform = self.impl_.transform.borrow();
        let state = transform.get_state();
        BoundOptions::default()
            .with_lat_lng_bounds(state.get_lat_lng_bounds())
            .with_min_zoom(state.get_min_zoom())
            .with_max_zoom(state.get_max_zoom())
            .with_min_pitch(mln_math::rad2deg(state.get_min_pitch()))
            .with_max_pitch(mln_math::rad2deg(state.get_max_pitch()))
    }

    // MARK: - Map options

    pub fn set_north_orientation(&mut self, orientation: NorthOrientation) {
        self.impl_.transform.borrow_mut().set_north_orientation(orientation);
        self.impl_.on_update();
    }

    pub fn set_constrain_mode(&mut self, mode: ConstrainMode) {
        self.impl_.transform.borrow_mut().set_constrain_mode(mode);
        self.impl_.on_update();
    }

    pub fn set_viewport_mode(&mut self, mode: ViewportMode) {
        self.impl_.transform.borrow_mut().set_viewport_mode(mode);
        self.impl_.on_update();
    }

    /// C++ throws "failed to resize: size is empty"; the port returns it.
    pub fn set_size(&mut self, size: Size) -> Result<(), String> {
        self.impl_.transform.borrow_mut().resize(size)?;
        self.impl_.on_update();
        Ok(())
    }

    pub fn set_frustum_offset(&mut self, frustum_offset: &EdgeInsets) {
        self.impl_.transform.borrow_mut().set_frustum_offset(frustum_offset);
        self.impl_.on_update();
    }

    pub fn get_frustum_offset(&self) -> EdgeInsets {
        self.impl_.transform.borrow().get_frustum_offset()
    }

    pub fn get_map_options(&self) -> MapOptions {
        let transform = self.impl_.transform.borrow();
        let state = transform.get_state();
        MapOptions::new()
            .with_north_orientation(transform.get_north_orientation())
            .with_map_mode(self.impl_.mode)
            .with_constrain_mode(state.get_constrain_mode())
            .with_viewport_mode(state.get_viewport_mode())
            .with_cross_source_collisions(self.impl_.cross_source_collisions)
            .with_size(state.get_size())
            .with_pixel_ratio(self.impl_.pixel_ratio)
    }

    // MARK: - Projection mode

    pub fn set_projection_mode(&mut self, options: &ProjectionMode) {
        self.impl_.transform.borrow_mut().set_projection_mode(options);
        self.impl_.on_update();
    }

    pub fn get_projection_mode(&self) -> ProjectionMode {
        self.impl_.transform.borrow().get_projection_mode()
    }

    // MARK: - Transform

    pub fn get_transform_state(&self) -> TransformState {
        self.impl_.transform.borrow().get_state().clone()
    }

    // MARK: - Tile LOD

    pub fn set_tile_lod_min_radius(&mut self, radius: f64) {
        if self.impl_.tile_lod_min_radius.get() == radius {
            return;
        }
        self.impl_.tile_lod_min_radius.set(radius);
        self.impl_.on_update();
    }

    pub fn get_tile_lod_min_radius(&self) -> f64 {
        self.impl_.tile_lod_min_radius.get()
    }

    pub fn set_tile_lod_scale(&mut self, scale: f64) {
        if self.impl_.tile_lod_scale.get() == scale {
            return;
        }
        self.impl_.tile_lod_scale.set(scale);
        self.impl_.on_update();
    }

    pub fn get_tile_lod_scale(&self) -> f64 {
        self.impl_.tile_lod_scale.get()
    }

    pub fn set_tile_lod_pitch_threshold(&mut self, threshold: f64) {
        if self.impl_.tile_lod_pitch_threshold.get() == threshold {
            return;
        }
        self.impl_.tile_lod_pitch_threshold.set(threshold);
        self.impl_.on_update();
    }

    pub fn get_tile_lod_pitch_threshold(&self) -> f64 {
        self.impl_.tile_lod_pitch_threshold.get()
    }

    pub fn set_tile_lod_zoom_shift(&mut self, shift: f64) {
        if self.impl_.tile_lod_zoom_shift.get() == shift {
            return;
        }
        self.impl_.tile_lod_zoom_shift.set(shift);
        self.impl_.on_update();
    }

    pub fn get_tile_lod_zoom_shift(&self) -> f64 {
        self.impl_.tile_lod_zoom_shift.get()
    }

    pub fn set_tile_lod_mode(&mut self, mode: TileLodMode) {
        if self.impl_.tile_lod_mode.get() == mode {
            return;
        }
        self.impl_.tile_lod_mode.set(mode);
        self.impl_.on_update();
    }

    pub fn get_tile_lod_mode(&self) -> TileLodMode {
        self.impl_.tile_lod_mode.get()
    }

    // gtests-core: projection and prefetch zoom delta (test/map, test/api).
    // MARK: - Projection

    /// `pixelForLatLng(latLng)`.
    pub fn pixel_for_lat_lng(&self, lat_lng: &LatLng) -> ScreenCoordinate {
        let transform = self.impl_.transform.borrow();
        transform.lat_lng_to_screen_coordinate(&transform.get_state().lat_lng_near_center(lat_lng))
    }

    /// `latLngForPixel(pixel, wrapMode)`. `Err`: the std::domain_error of
    /// the C++ LatLng constructor (a NaN or infinite pixel).
    pub fn lat_lng_for_pixel(&self, pixel: &ScreenCoordinate, wrap_mode: WrapMode) -> Result<LatLng, String> {
        self.impl_.transform.borrow().screen_coordinate_to_lat_lng(pixel, wrap_mode)
    }

    /// `pixelsForLatLngs(latLngs)`.
    pub fn pixels_for_lat_lngs(&self, lat_lngs: &[LatLng]) -> Vec<ScreenCoordinate> {
        lat_lngs.iter().map(|lat_lng| self.pixel_for_lat_lng(lat_lng)).collect()
    }

    /// `latLngsForPixels(screenCoords, wrapMode)`.
    pub fn lat_lngs_for_pixels(
        &self,
        screen_coords: &[ScreenCoordinate],
        wrap_mode: WrapMode,
    ) -> Result<Vec<LatLng>, String> {
        screen_coords.iter().map(|point| self.lat_lng_for_pixel(point, wrap_mode)).collect()
    }

    /// `setPrefetchZoomDelta(delta)`.
    pub fn set_prefetch_zoom_delta(&mut self, delta: u8) {
        if self.impl_.prefetch_zoom_delta.get() == delta {
            return;
        }
        self.impl_.prefetch_zoom_delta.set(delta);
        self.impl_.on_update();
    }

    /// `getPrefetchZoomDelta()`.
    pub fn get_prefetch_zoom_delta(&self) -> u8 {
        self.impl_.prefetch_zoom_delta.get()
    }

    /// `getFreeCameraOptions()`.
    pub fn get_free_camera_options(&self) -> mln_map_types::FreeCameraOptions {
        self.impl_.transform.borrow().get_free_camera_options()
    }

    // TODO(port): annotations, rendering stats view, setFreeCameraOptions,
    // rendered features.

    // MARK: - Debug

    pub fn set_debug(&mut self, debug_options: MapDebugOptions) {
        self.impl_.debug_options.set(debug_options);
        self.impl_.on_update();
    }

    pub fn get_debug(&self) -> MapDebugOptions {
        self.impl_.debug_options.get()
    }

    pub fn is_fully_loaded(&self) -> bool {
        self.impl_.style.borrow().impl_.is_loaded() && self.impl_.renderer_fully_loaded.get()
    }

    /// `dumpDebugLogs()`: logs the style's sources and their state.
    pub fn dump_debug_logs(&self) {
        use mln_util::logging::{Event, Log};
        let rule = "-".repeat(80);
        Log::info(Event::General, &rule);
        self.impl_.style.borrow().impl_.dump_debug_logs();
        Log::info(Event::General, &rule);
    }
}

fn misuse(message: &str) -> ExceptionPtr {
    MisuseException::ptr(message)
}

// gtests-core: the free function `cameraForLatLngs(latLngs, transform,
// padding)` of map.cpp (Map::cameraForLatLngs and MapProjection use it).
pub(crate) fn camera_for_lat_lngs(
    lat_lngs: &[LatLng],
    transform: &crate::transform::Transform,
    padding: &EdgeInsets,
) -> Result<CameraOptions, String> {
    if lat_lngs.is_empty() {
        return Ok(CameraOptions::default());
    }
    let size = transform.get_state().get_size();
    // Calculate the bounds of the possibly rotated shape with respect to the viewport.
    let mut ne_pixel = ScreenCoordinate::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut sw_pixel = ScreenCoordinate::new(f64::INFINITY, f64::INFINITY);
    for lat_lng in lat_lngs {
        let pixel = transform.lat_lng_to_screen_coordinate(lat_lng);
        // std::min / std::max
        sw_pixel.x = if pixel.x < sw_pixel.x { pixel.x } else { sw_pixel.x };
        ne_pixel.x = if ne_pixel.x < pixel.x { pixel.x } else { ne_pixel.x };
        sw_pixel.y = if pixel.y < sw_pixel.y { pixel.y } else { sw_pixel.y };
        ne_pixel.y = if ne_pixel.y < pixel.y { pixel.y } else { ne_pixel.y };
    }
    let width = ne_pixel.x - sw_pixel.x;
    let height = ne_pixel.y - sw_pixel.y;

    // Calculate the zoom level.
    let mut min_scale = f64::INFINITY;
    if width > 0.0 || height > 0.0 {
        let mut scale_x = f64::from(size.width) / width;
        let mut scale_y = f64::from(size.height) / height;
        scale_x -= (padding.left() + padding.right()) / width;
        scale_y -= (padding.top() + padding.bottom()) / height;
        min_scale = mln_math::min_f64(scale_x, scale_y);
    }

    let mut zoom = transform.get_zoom();
    if min_scale > 0.0 {
        zoom = mln_math::clamp_f64(
            zoom + mln_math::log2(min_scale),
            transform.get_state().get_min_zoom(),
            transform.get_state().get_max_zoom(),
        );
    } else {
        mln_util::logging::Log::error(
            mln_util::logging::Event::General,
            "Unable to calculate appropriate zoom level for bounds. Vertical or horizontal padding is greater than \
             map's height or width.",
        );
    }

    // Calculate the center point of a virtual bounds that is extended in all directions by padding.
    let center_pixel = ScreenCoordinate::new((ne_pixel.x + sw_pixel.x) / 2.0, (ne_pixel.y + sw_pixel.y) / 2.0);

    Ok(CameraOptions::default()
        .with_center(Some(transform.screen_coordinate_to_lat_lng(&center_pixel, WrapMode::Wrapped)?))
        .with_padding(Some(*padding))
        .with_zoom(Some(zoom)))
}
