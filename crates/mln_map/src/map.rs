// Ports include/mln/map/map.hpp and src/mln/map/map.cpp (the parts M1 runs).

use std::cell::Ref;
use std::rc::Rc;
use std::sync::Arc;

use mln_map_types::{
    AnimationOptions, BoundOptions, CameraOptions, ConstrainMode, MapDebugOptions, MapMode, NorthOrientation,
    ProjectionMode, TileLodMode, TransformState, VerticalPerspectiveProjection, ViewportMode,
};
use mln_renderer::{ExceptionPtr, RendererFrontend};
use mln_storage::{ClientOptions, FileSource, FileSourceManager, FileSourceType, ResourceOptions};
use mln_style::Style;
use mln_style_types::{ProjectionDefinition, ProjectionType};
use mln_util::exception::MisuseException;
use mln_util::geo::{EdgeInsets, LatLng, LatLngBounds, ScreenCoordinate, WrapMode};
use mln_util::mat4::Mat4;
use mln_util::size::Size;
use mln_util::vectors::{Vec3, Vec4};

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
    pub fn new<F>(frontend: &F, observer: Rc<dyn MapObserver>, map_options: MapOptions) -> Map
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
    ) -> Map
    where
        F: RendererFrontend + Clone + 'static,
    {
        let file_source =
            FileSourceManager::get().get_file_source(FileSourceType::ResourceLoader, resource_options, client_options);
        Map { impl_: MapImpl::new(Box::new(frontend.clone()), observer, file_source, &map_options) }
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
    ) -> Map
    where
        F: RendererFrontend + Clone + 'static,
    {
        Map { impl_: MapImpl::new(Box::new(frontend.clone()), observer, Some(file_source), &map_options) }
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

    /// On the globe, let drags and zooms around a point turn the bearing so
    /// the planet keeps its orientation, for maps whose rotation gestures are
    /// enabled. Off, the default, they hold the bearing as GL JS does.
    pub fn set_globe_free_bearing(&mut self, free_bearing: bool) {
        self.impl_.transform.borrow_mut().set_globe_free_bearing(free_bearing);
    }

    pub fn is_globe_free_bearing(&self) -> bool {
        self.impl_.transform.borrow().is_globe_free_bearing()
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
    pub fn lat_lng_bounds_for_camera(&self, camera: &CameraOptions) -> mln_util::geo::LatLngBounds {
        let mut shallow = crate::transform::Transform::from_state(self.impl_.transform.borrow().get_state().clone());
        let size = shallow.get_state().get_size();

        shallow.jump_to(camera);
        if shallow.get_state().is_globe_rendering() {
            return globe_bounds(&shallow);
        }
        mln_util::geo::LatLngBounds::hull(
            shallow.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(0.0, 0.0), mln_util::geo::WrapMode::Wrapped),
            shallow.screen_coordinate_to_lat_lng(
                &ScreenCoordinate::new(f64::from(size.width), f64::from(size.height)),
                mln_util::geo::WrapMode::Wrapped,
            ),
        )
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

    pub fn move_by(&mut self, point: &ScreenCoordinate, animation: &AnimationOptions) {
        self.impl_.camera_mutated.set(true);
        self.impl_.transform.borrow_mut().move_by(point, animation);
        self.impl_.on_update();
    }

    /// `moveBy(offset, anchor, animation)`: move by `offset` as a drag that
    /// holds `anchor`, the cursor: on the globe the location under it follows
    /// it.
    pub fn move_by_with_anchor(
        &mut self,
        offset: &ScreenCoordinate,
        anchor: Option<ScreenCoordinate>,
        animation: &AnimationOptions,
    ) {
        self.impl_.camera_mutated.set(true);
        self.impl_.transform.borrow_mut().move_by_with_anchor(offset, anchor, animation);
        self.impl_.on_update();
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
    /// `cameraForLatLngBounds(bounds, padding, bearing, pitch)`.
    pub fn camera_for_lat_lng_bounds(
        &self,
        bounds: &LatLngBounds,
        padding: &EdgeInsets,
        bearing: Option<f64>,
        pitch: Option<f64>,
    ) -> CameraOptions {
        self.camera_for_lat_lngs(
            &[bounds.northwest(), bounds.southwest(), bounds.southeast(), bounds.northeast()],
            padding,
            bearing,
            pitch,
        )
    }

    /// `cameraForLatLngs(latLngs, padding, bearing, pitch)`.
    pub fn camera_for_lat_lngs(
        &self,
        lat_lngs: &[LatLng],
        padding: &EdgeInsets,
        bearing: Option<f64>,
        pitch: Option<f64>,
    ) -> CameraOptions {
        if bearing.is_none() && pitch.is_none() {
            return camera_for_lat_lngs(lat_lngs, &self.impl_.transform.borrow(), padding);
        }

        let mut transform = crate::transform::Transform::from_state(self.impl_.transform.borrow().get_state().clone());

        transform.jump_to(&CameraOptions::default().with_bearing(bearing).with_pitch(pitch));

        camera_for_lat_lngs(lat_lngs, &transform, padding)
            .with_bearing(Some(mln_math::rad2deg(-transform.get_bearing())))
            .with_pitch(Some(mln_math::rad2deg(transform.get_pitch())))
    }

    // TODO(port): cameraForGeometry.

    /// `latLngBoundsForCameraUnwrapped(camera)`.
    pub fn lat_lng_bounds_for_camera_unwrapped(&self, camera: &CameraOptions) -> LatLngBounds {
        let mut shallow = crate::transform::Transform::from_state(self.impl_.transform.borrow().get_state().clone());
        let size = shallow.get_state().get_size();

        shallow.jump_to(camera);
        if shallow.get_state().is_globe_rendering() {
            return globe_bounds(&shallow);
        }
        let width = f64::from(size.width);
        let height = f64::from(size.height);
        let mut nw = shallow.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(0.0, 0.0), WrapMode::Wrapped);
        let mut se = shallow.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(width, height), WrapMode::Wrapped);
        let mut ne = shallow.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(width, 0.0), WrapMode::Wrapped);
        let mut sw = shallow.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(0.0, height), WrapMode::Wrapped);
        let center =
            shallow.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(width / 2.0, height / 2.0), WrapMode::Wrapped);
        nw.unwrap_for_shortest_path(&center);
        se.unwrap_for_shortest_path(&center);
        ne.unwrap_for_shortest_path(&center);
        sw.unwrap_for_shortest_path(&center);
        let mut bounds = LatLngBounds::hull(nw, se);
        bounds.extend(&ne);
        bounds.extend(&sw);
        bounds.extend(&center);
        bounds
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
                // On the globe the lowest zoom falls with latitude; on
                // Mercator it is the minimum.
                let state = transform.get_state();
                let latitude_term = state.get_min_zoom_at_latitude(state.get_lat_lng(WrapMode::Unwrapped).latitude())
                    - state.get_min_zoom();
                let min_zoom = min_zoom + latitude_term;
                if transform.get_zoom() < min_zoom {
                    change_camera = true;
                    camera_options = camera_options.with_zoom(Some(min_zoom));
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

    /// `latLngForPixel(pixel, wrapMode)`.
    pub fn lat_lng_for_pixel(&self, pixel: &ScreenCoordinate, wrap_mode: WrapMode) -> LatLng {
        self.impl_.transform.borrow().screen_coordinate_to_lat_lng(pixel, wrap_mode)
    }

    /// Whether the globe hides a location from the camera; a Mercator map
    /// hides nothing.
    pub fn is_location_occluded(&self, lat_lng: &LatLng) -> bool {
        self.impl_.transform.borrow().is_location_occluded(lat_lng)
    }

    /// Whether the map is drawn as a globe, alone or blending into Mercator.
    pub fn is_globe_rendering(&self) -> bool {
        self.impl_.transform.borrow().get_state().is_globe_rendering()
    }

    /// `pixelsForLatLngs(latLngs)`.
    pub fn pixels_for_lat_lngs(&self, lat_lngs: &[LatLng]) -> Vec<ScreenCoordinate> {
        lat_lngs.iter().map(|lat_lng| self.pixel_for_lat_lng(lat_lng)).collect()
    }

    /// `latLngsForPixels(screenCoords, wrapMode)`.
    pub fn lat_lngs_for_pixels(&self, screen_coords: &[ScreenCoordinate], wrap_mode: WrapMode) -> Vec<LatLng> {
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
) -> CameraOptions {
    if lat_lngs.is_empty() {
        return CameraOptions::default();
    }
    if transform.get_state().is_globe_rendering() {
        camera_for_lat_lngs_on_globe(lat_lngs, transform, padding)
    } else {
        camera_for_lat_lngs_on_screen(lat_lngs, transform, padding)
    }
}

fn camera_for_lat_lngs_on_screen(
    lat_lngs: &[LatLng],
    transform: &crate::transform::Transform,
    padding: &EdgeInsets,
) -> CameraOptions {
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

    CameraOptions::default()
        .with_center(Some(transform.screen_coordinate_to_lat_lng(&center_pixel, WrapMode::Wrapped)))
        .with_padding(Some(*padding))
        .with_zoom(Some(zoom))
}

/// `std::min`: `b < a ? b : a`.
fn std_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `std::max`: `a < b ? b : a`.
fn std_max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// `LatLng{lat, lon}`: C++ constructs it directly, which throws
/// std::domain_error; nothing catches it here.
fn lat_lng(lat: f64, lon: f64) -> LatLng {
    LatLng::new(lat, lon).unwrap_or_else(|e| panic!("{e}"))
}

/// GL JS `solveVectorScale`: how much to scale the globe for a surface point
/// to land on `target` along one clip axis, or nothing when the equation
/// degenerates. Zooming out also moves the camera along `to_center`, so the
/// point solved for is `vector * t + to_center * (1 - t)`.
fn globe_scale_for_clip_target(
    vector: &Vec3,
    to_center: &Vec3,
    projection: &Mat4,
    x_axis: bool,
    target: f64,
) -> Option<f64> {
    let column: [f64; 4] = if x_axis {
        [projection[0], projection[4], projection[8], projection[12]]
    } else {
        [projection[1], projection[5], projection[9], projection[13]]
    };
    let column_z: [f64; 4] = [projection[3], projection[7], projection[11], projection[15]];

    let vector_dot = vector[0] * column[0] + vector[1] * column[1] + vector[2] * column[2];
    let vector_dot_z = vector[0] * column_z[0] + vector[1] * column_z[1] + vector[2] * column_z[2];
    let to_center_dot = to_center[0] * column[0] + to_center[1] * column[1] + to_center[2] * column[2];
    let to_center_dot_z = to_center[0] * column_z[0] + to_center[1] * column_z[1] + to_center[2] * column_z[2];

    if to_center_dot + target * vector_dot_z == vector_dot + target * to_center_dot_z
        || column_z[3] * (vector_dot - to_center_dot)
            + column[3] * (to_center_dot_z - vector_dot_z)
            + vector_dot * to_center_dot_z
            == to_center_dot * vector_dot_z
    {
        return None;
    }
    Some(
        (to_center_dot + column[3] - target * to_center_dot_z - target * column_z[3])
            / (to_center_dot - vector_dot - target * to_center_dot_z + target * vector_dot_z),
    )
}

fn keep_smallest_scale(smallest: &mut f64, candidate: Option<f64>) {
    if let Some(candidate) = candidate
        && candidate >= 0.0
        && candidate < *smallest
    {
        *smallest = candidate;
    }
}

/// GL JS `VerticalPerspectiveCameraHelper.cameraForBoxAndBearing`: fit the
/// flat world first, then scale the globe until every corner and edge
/// midpoint of the bounds is inside the padded viewport, with no pitch.
fn camera_for_lat_lngs_on_globe(
    lat_lngs: &[LatLng],
    transform: &crate::transform::Transform,
    padding: &EdgeInsets,
) -> CameraOptions {
    let mut flat = crate::transform::Transform::from_state(transform.get_state().clone());
    flat.set_projection_definition(&ProjectionDefinition::new(ProjectionType::Mercator));
    flat.jump_to(&CameraOptions::default().with_pitch(Some(0.0)).with_roll(Some(0.0)));
    let fit = camera_for_lat_lngs_on_screen(lat_lngs, &flat, padding);
    let (Some(fit_center), Some(fit_zoom)) = (fit.center, fit.zoom) else {
        return fit;
    };

    // Solve with the padding the returned camera is shown with.
    let mut globe = crate::transform::Transform::from_state(transform.get_state().clone());
    globe.jump_to(
        &CameraOptions::default()
            .with_center(Some(fit_center))
            .with_zoom(Some(fit_zoom))
            .with_padding(Some(*padding))
            .with_pitch(Some(0.0))
            .with_roll(Some(0.0)),
    );
    let state = globe.get_state();
    let matrix = state.get_globe_view_projection_matrix();
    let size = state.get_size();
    let width = f64::from(size.width);
    let height = f64::from(size.height);
    let left = padding.left() / width * 2.0 - 1.0;
    let right = (width - padding.right()) / width * 2.0 - 1.0;
    let top = padding.top() / height * -2.0 + 1.0;
    let bottom = (height - padding.bottom()) / height * -2.0 + 1.0;

    let mut bounds = LatLngBounds::empty();
    for lat_lng in lat_lngs {
        bounds.extend(lat_lng);
    }
    let west = bounds.west();
    let east = bounds.east();
    let north = bounds.north();
    let south = bounds.south();
    let mid_longitude = west + VerticalPerspectiveProjection::difference_of_angles_degrees(west, east) * 0.5;
    let mid_latitude = north + VerticalPerspectiveProjection::difference_of_angles_degrees(north, south) * 0.5;
    let samples = [
        lat_lng(north, west),
        lat_lng(north, east),
        lat_lng(south, west),
        lat_lng(south, east),
        lat_lng(mid_latitude, east),
        lat_lng(mid_latitude, west),
        lat_lng(north, mid_longitude),
        lat_lng(south, mid_longitude),
    ];
    let to_center = VerticalPerspectiveProjection::surface_vector(&fit_center);

    let mut smallest = f64::INFINITY;
    for sample in &samples {
        let vector = VerticalPerspectiveProjection::surface_vector(sample);
        if left < 0.0 {
            keep_smallest_scale(&mut smallest, globe_scale_for_clip_target(&vector, &to_center, &matrix, true, left));
        }
        if right > 0.0 {
            keep_smallest_scale(&mut smallest, globe_scale_for_clip_target(&vector, &to_center, &matrix, true, right));
        }
        if top > 0.0 {
            keep_smallest_scale(&mut smallest, globe_scale_for_clip_target(&vector, &to_center, &matrix, false, top));
        }
        if bottom < 0.0 {
            keep_smallest_scale(
                &mut smallest,
                globe_scale_for_clip_target(&vector, &to_center, &matrix, false, bottom),
            );
        }
    }
    if !smallest.is_finite() || smallest == 0.0 {
        mln_util::logging::Log::warning(
            mln_util::logging::Event::General,
            "Unable to fit the bounds on the globe; keeping the flat fit.",
        );
        return fit;
    }
    fit.with_zoom(Some(std_min(globe.get_zoom() + state.scale_zoom(smallest), state.get_max_zoom())))
}

/// GL JS's globe bounds: the farthest of eight edge points on each side of
/// the center, where points off the planet fall on its horizon, widened to
/// every longitude when a pole is on screen.
fn globe_bounds(transform: &crate::transform::Transform) -> LatLngBounds {
    let width = f64::from(transform.get_state().get_size().width);
    let height = f64::from(transform.get_state().get_size().height);
    let center = transform.get_lat_lng(WrapMode::Wrapped);
    let mut west = 0.0;
    let mut east = 0.0;
    let mut south = 0.0;
    let mut north = 0.0;
    for point in [
        ScreenCoordinate::new(0.0, 0.0),
        ScreenCoordinate::new(width / 2.0, 0.0),
        ScreenCoordinate::new(width, 0.0),
        ScreenCoordinate::new(width, height / 2.0),
        ScreenCoordinate::new(width, height),
        ScreenCoordinate::new(width / 2.0, height),
        ScreenCoordinate::new(0.0, height),
        ScreenCoordinate::new(0.0, height / 2.0),
    ] {
        let lat_lng = transform.screen_coordinate_to_lat_lng(&point, WrapMode::Wrapped);
        let longitude =
            VerticalPerspectiveProjection::difference_of_angles_degrees(center.longitude(), lat_lng.longitude());
        let latitude =
            VerticalPerspectiveProjection::difference_of_angles_degrees(center.latitude(), lat_lng.latitude());
        west = std_min(west, longitude);
        east = std_max(east, longitude);
        south = std_min(south, latitude);
        north = std_max(north, latitude);
    }
    west += center.longitude();
    east += center.longitude();
    south += center.latitude();
    north += center.latitude();

    let on_screen = |pole: &LatLng| {
        // In front of the camera and inside its depth range, as GL JS's
        // isSurfacePointOnScreen asks.
        let mut clip: Vec4 = [0.0; 4];
        VerticalPerspectiveProjection::lat_lng_to_screen_coordinate(transform.get_state(), pole, &mut clip);
        if clip[3] <= 0.0 || (clip[2] / clip[3]).abs() >= 1.0 {
            return false;
        }
        let point = transform.lat_lng_to_screen_coordinate(pole);
        !transform.get_state().is_location_occluded(pole)
            && point.x > 0.0
            && point.x < width
            && point.y > 0.0
            && point.y < height
    };
    if on_screen(&lat_lng(90.0, 0.0)) {
        north = 90.0;
        west = -180.0;
        east = 180.0;
    }
    if on_screen(&lat_lng(-90.0, 0.0)) {
        south = -90.0;
        west = -180.0;
        east = 180.0;
    }
    LatLngBounds::hull(lat_lng(south, west), lat_lng(north, east))
}
