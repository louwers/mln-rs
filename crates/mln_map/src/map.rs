// C++ `Map::Impl` observes the transform, the style and the renderer, and
// reacts inside their callbacks. Here they push events into queues that the
// map drains (`process_events`), so the map owns its style and transform and
// lends them out as `&mut`.

use std::rc::Rc;
use std::sync::Arc;

use mln_actor::{TaggedScheduler, scheduler};
use mln_map_types::{
    AnimationOptions, BoundOptions, CameraDelta, CameraError, CameraOptions, CameraUpdate, ConstrainMode,
    MapDebugOptions, MapMode, MapView, NorthOrientation, ProjectionMode, TileLodMode, TransformState,
    VerticalPerspectiveProjection, ViewportMode,
};
use mln_renderer::{
    FeatureInfo, LayerFeaturesMap, RendererEvent, RendererEvents, RenderingStats, SourceLayerID, UpdateParameters,
    UpdateReader, UpdateSlot,
};
use mln_storage::{FileSource, FileSourceManager, FileSourceType, ResourceOptions};
use mln_style::observer::{Observer as StyleObserver, SourceObserver};
use mln_style::source::Source;
use mln_style::sprite::Sprite;
use mln_style::{Style, StyleLoadError};
use mln_style_types::{ProjectionDefinition, ProjectionType};
use mln_util::error::Error;
use mln_util::event_queue::{EventQueue, Waker};
use mln_util::geo::{EdgeInsets, LatLng, LatLngBounds, ScreenCoordinate, WrapMode};
use mln_util::geometry::{Geometry, for_each_point};
use mln_util::identity::SimpleIdentity;
use mln_util::mat4::Mat4;
use mln_util::size::Size;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::time::Instant;
use mln_util::vectors::Vec3;

use crate::feature_state::FeatureStateStore;
use crate::map_event::{CameraChange, MapEvent, RenderFrameStatus, RenderMode};
use crate::map_options::{MapOptions, UnusedStyleImagePolicy};
use crate::map_snapshot::{CameraBounds, FreeCamera, MapSnapshot, TileSettings};
use crate::rendering_stats_view::{RenderingStatsView, RenderingStatsViewOptions};
use crate::transform::{Transform, TransformObserver};

/// Receives the outcome of [`Map::render_still`]: `Ok` once a complete frame
/// rendered, or why the image failed.
pub(crate) type StillImageCallback = Box<dyn FnOnce(Result<(), Error>)>;

/// A call of the style's observer.
enum StyleEvent {
    SourceChanged(String),
    Update,
    StyleLoading,
    StyleLoaded,
    StyleError(StyleLoadError),
    ResourceError(Error),
    SpriteLoaded(Option<Sprite>),
    SpriteError(Option<Sprite>, Error),
    SpriteRequested(Option<Sprite>),
    /// A response to the style's URL is waiting (see
    /// `Observer::on_style_response`).
    StyleResponse,
}

/// The style's observer: queues the calls for the map.
struct StyleEvents(EventQueue<StyleEvent>);

impl SourceObserver for StyleEvents {
    fn on_source_changed(&self, source: &dyn Source) {
        self.0.push(StyleEvent::SourceChanged(source.get_id()));
    }
}

impl StyleObserver for StyleEvents {
    fn on_style_loading(&self) {
        self.0.push(StyleEvent::StyleLoading);
    }
    fn on_style_loaded(&self) {
        self.0.push(StyleEvent::StyleLoaded);
    }
    fn on_update(&self) {
        self.0.push(StyleEvent::Update);
    }
    fn on_style_error(&self, error: StyleLoadError) {
        self.0.push(StyleEvent::StyleError(error));
    }
    fn on_resource_error(&self, error: Error) {
        self.0.push(StyleEvent::ResourceError(error));
    }
    fn on_sprite_loaded(&self, sprite: Option<&Sprite>) {
        self.0.push(StyleEvent::SpriteLoaded(sprite.cloned()));
    }
    fn on_sprite_error(&self, sprite: Option<&Sprite>, error: Error) {
        self.0.push(StyleEvent::SpriteError(sprite.cloned(), error));
    }
    fn on_sprite_requested(&self, sprite: Option<&Sprite>) {
        self.0.push(StyleEvent::SpriteRequested(sprite.cloned()));
    }
    fn on_style_response(&self) {
        self.0.push(StyleEvent::StyleResponse);
    }
}

/// The transform's observer: queues the camera events for the map's owner.
struct CameraEvents(EventQueue<MapEvent>);

impl TransformObserver for CameraEvents {
    fn on_camera_will_change(&self, change: CameraChange) {
        self.0.push(MapEvent::CameraWillChange(change));
    }
    fn on_camera_is_changing(&self) {
        self.0.push(MapEvent::CameraIsChanging);
    }
    fn on_camera_did_change(&self, change: CameraChange) {
        self.0.push(MapEvent::CameraDidChange(change));
    }
}

/// A map: its style, its camera, and the updates it publishes for its
/// renderer.
///
/// The map publishes its state for a renderer in an update slot, which a
/// renderer reads through [`update_reader`](Self::update_reader) and
/// reports back to through [`renderer_events`](Self::renderer_events).
/// The style and the renderer report to the map through queues, which the
/// map handles in [`process_events`](Self::process_events) and at the start
/// of each of its calls that take `&mut self`. What the map reports to its
/// owner waits in another queue, for [`drain_events`](Self::drain_events).
/// An owner that runs a `RunLoop` sets a waker
/// ([`set_waker`](Self::set_waker)) that schedules those calls.
pub struct Map {
    updates: UpdateSlot,
    transform: Transform,

    mode: MapMode,
    pixel_ratio: f32,
    cross_source_collisions: bool,
    fast_pfor_enabled: bool,
    capture_rendered_features: bool,
    unused_style_image_policy: UnusedStyleImagePolicy,

    debug_options: MapDebugOptions,
    rendering_stats_view: Option<RenderingStatsView>,
    /// The stats of the last frame the renderer finished.
    last_frame_stats: Option<Arc<RenderingStats>>,
    style: Style,
    /// The state of the sources' features, which each update carries.
    feature_state: FeatureStateStore,

    camera_mutated: bool,
    prefetch_zoom_delta: u8,
    loading: bool,
    renderer_fully_loaded: bool,
    still_image_request: Option<StillImageCallback>,
    /// Counts the still image requests, so the renderer can tell a new one.
    still_image_request_id: u64,

    tile_lod_min_radius: f64,
    tile_lod_scale: f64,
    tile_lod_pitch_threshold: f64,
    tile_lod_zoom_shift: f64,
    tile_lod_mode: TileLodMode,

    /// Grows with each change to the map's state.
    generation: u64,

    style_events: EventQueue<StyleEvent>,
    renderer_events: RendererEvents,
    /// What the map reports to its owner.
    events: EventQueue<MapEvent>,

    /// Declared after `style`, so the style and its requests go first.
    file_source: Option<Arc<dyn FileSource>>,
}

impl Map {
    /// A map with a `ResourceLoader` of its own for the default
    /// `ResourceOptions`.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an empty `map_options.size`.
    pub fn new(map_options: MapOptions) -> Result<Map, Error> {
        Self::with_resource_options(map_options, &FileSourceManager::new(), &ResourceOptions::default())
    }

    /// A map whose file source is `file_sources`' `ResourceLoader` for
    /// `resource_options`, or none when `file_sources` has no
    /// `ResourceLoader` factory.
    pub fn with_resource_options(
        map_options: MapOptions,
        file_sources: &FileSourceManager,
        resource_options: &ResourceOptions,
    ) -> Result<Map, Error> {
        let file_source = file_sources.get_file_source(FileSourceType::ResourceLoader, resource_options);
        Self::create(file_source, &map_options)
    }

    // gtests-core: the C++ tests' `MapAdapter` (test/include/mln/test/
    // map_adapter.hpp): a map over a given file source.
    /// A map whose resources come from `file_source`.
    pub fn with_file_source(file_source: Arc<dyn FileSource>, map_options: MapOptions) -> Result<Map, Error> {
        Self::create(Some(file_source), &map_options)
    }

    fn create(file_source: Option<Arc<dyn FileSource>>, map_options: &MapOptions) -> Result<Map, Error> {
        let events = EventQueue::new();
        let style_events = EventQueue::new();
        let renderer_events = RendererEvents::new();
        let pixel_ratio = map_options.pixel_ratio;
        // The style's sources work on the background pool, under a tag of
        // their own.
        let thread_pool = TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new());
        let mut style = Style::with_file_source(file_source.clone(), pixel_ratio, thread_pool);
        style.impl_.set_observer(Some(Rc::new(StyleEvents(style_events.clone()))));
        let mut transform = Transform::new(
            Rc::new(CameraEvents(events.clone())),
            map_options.constrain_mode,
            map_options.viewport_mode,
        );
        transform.set_north_orientation(map_options.north_orientation);
        let mut map = Map {
            updates: UpdateSlot::new(),
            transform,
            mode: map_options.map_mode,
            pixel_ratio,
            cross_source_collisions: map_options.cross_source_collisions,
            fast_pfor_enabled: map_options.fast_pfor_enabled,
            capture_rendered_features: map_options.capture_rendered_features,
            unused_style_image_policy: map_options.unused_style_image_policy,
            debug_options: MapDebugOptions::NO_DEBUG,
            rendering_stats_view: None,
            last_frame_stats: None,
            style,
            feature_state: FeatureStateStore::default(),
            camera_mutated: false,
            // util::DEFAULT_PREFETCH_ZOOM_DELTA
            prefetch_zoom_delta: 4,
            loading: false,
            renderer_fully_loaded: false,
            still_image_request: None,
            still_image_request_id: 0,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            generation: 1,
            style_events,
            renderer_events,
            events,
            file_source,
        };
        map.transform.resize(map_options.size)?;
        Ok(map)
    }

    // MARK: - Renderer

    /// A reader of the updates that the map publishes for its renderer.
    pub fn update_reader(&self) -> UpdateReader {
        self.updates.reader()
    }

    /// The queue that the map's renderer reports its events to.
    pub fn renderer_events(&self) -> RendererEvents {
        self.renderer_events.clone()
    }

    /// Holds the map's updates back from its renderer until the matching
    /// [`end_command_group`](Self::end_command_group), so the changes in
    /// between show in the same frame. Groups nest, and the changes show
    /// when the outermost group ends.
    pub fn begin_command_group(&mut self) {
        self.process_events();
        self.updates.begin_group();
    }

    /// Ends the group that [`begin_command_group`](Self::begin_command_group)
    /// began. Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind)
    /// when no group is open.
    pub fn end_command_group(&mut self) -> Result<(), Error> {
        self.process_events();
        self.updates.end_group()
    }

    /// How many command groups are open.
    pub fn command_group_depth(&self) -> u32 {
        self.updates.group_depth()
    }

    /// Holds the map's updates back from its renderer, as a command group
    /// does, until [`release_updates`](Self::release_updates). The hold is
    /// apart from the groups: it doesn't nest, and
    /// [`command_group_depth`](Self::command_group_depth) doesn't count it.
    pub fn hold_updates(&mut self) {
        self.updates.hold();
    }

    /// Ends the hold that [`hold_updates`](Self::hold_updates) began. The
    /// changes show unless a command group still holds them.
    pub fn release_updates(&mut self) {
        self.process_events();
        self.updates.release();
    }

    // MARK: - Events

    /// Sets the waker that runs, on any thread, when the map has events to
    /// handle or to drain. It must not call the map: it schedules a call of
    /// [`drain_events`](Self::drain_events) or
    /// [`process_events`](Self::process_events) on the map's thread, as by
    /// sending an `AsyncTask`.
    pub fn set_waker(&mut self, waker: Option<Waker>) {
        self.style_events.set_waker(waker.clone());
        self.renderer_events.set_waker(waker.clone());
        self.events.set_waker(waker);
    }

    /// Handles what the style and the renderer reported since the last
    /// call: loads, frames, errors and missing images. The map's reactions,
    /// such as an update for the renderer or a still image's callback, run
    /// here, and its events wait for [`drain_events`](Self::drain_events).
    pub fn process_events(&mut self) {
        loop {
            let style_events = self.style_events.take();
            let renderer_events = self.renderer_events.take();
            if style_events.is_empty() && renderer_events.is_empty() {
                break;
            }
            // A style that loads takes every source's feature state with it.
            // The updates that its parse asked for are in the same batch, and
            // go out with the new style's sources, so the state goes first.
            let state_cleared =
                style_events.iter().any(|event| matches!(event, StyleEvent::StyleLoaded)) && self.feature_state.clear();
            for event in style_events {
                self.handle_style_event(event);
            }
            if state_cleared {
                self.on_update();
            }
            for event in renderer_events {
                self.handle_renderer_event(event);
            }
        }
        self.share_projection_with_transform();
    }

    /// Handles what the style and the renderer reported, as
    /// [`process_events`](Self::process_events) does, and takes the map's
    /// events, oldest first.
    pub fn drain_events(&mut self) -> Vec<MapEvent> {
        self.process_events();
        self.events.take()
    }

    fn emit(&self, event: MapEvent) {
        self.events.push(event);
    }

    fn handle_style_event(&mut self, event: StyleEvent) {
        match event {
            StyleEvent::SourceChanged(source_id) => self.emit(MapEvent::SourceChanged { source_id }),
            StyleEvent::Update => self.on_update(),
            StyleEvent::StyleLoading => {
                self.changed();
                self.loading = true;
                self.renderer_fully_loaded = false;
                self.emit(MapEvent::LoadingStarted);
            }
            StyleEvent::StyleLoaded => {
                self.changed();
                if !self.camera_mutated {
                    let camera = self.style.get_default_camera();
                    self.jump(&camera);
                }
                self.emit(MapEvent::StyleLoaded);
            }
            StyleEvent::StyleError(error) => self.emit(MapEvent::LoadingFailed(error)),
            StyleEvent::ResourceError(error) => self.on_resource_error(error),
            StyleEvent::SpriteLoaded(sprite) => self.emit(MapEvent::SpriteLoaded(sprite)),
            StyleEvent::SpriteError(sprite, error) => self.emit(MapEvent::SpriteError(sprite, error)),
            StyleEvent::SpriteRequested(sprite) => self.emit(MapEvent::SpriteRequested(sprite)),
            // C++ parses inside the file source callback (the lambda in
            // Style::Impl::loadURL).
            StyleEvent::StyleResponse => self.style.impl_.process_style_responses(),
        }
    }

    fn handle_renderer_event(&mut self, event: RendererEvent) {
        let continuous = self.mode == MapMode::Continuous;
        match event {
            RendererEvent::Invalidate => self.on_update(),
            RendererEvent::ResourceError(error) => self.on_resource_error(error),
            RendererEvent::RenderFrameStarted => {
                if continuous {
                    self.emit(MapEvent::RenderFrameStarted);
                }
            }
            RendererEvent::RenderFrameFinished {
                mode,
                needs_repaint,
                placement_changed,
                still_image_request_id,
                stats,
            } => {
                self.on_render_frame_finished(mode, needs_repaint, placement_changed, still_image_request_id, stats);
            }
            RendererEvent::RenderMapStarted => {
                if continuous {
                    self.emit(MapEvent::RenderMapStarted);
                }
            }
            RendererEvent::RenderMapFinished => {
                if continuous && self.loading {
                    self.emit(MapEvent::RenderMapFinished(RenderMode::Full));
                    self.loading = false;
                    self.emit(MapEvent::LoadingFinished);
                }
            }
            RendererEvent::StyleImageMissing(id) => {
                if self.style.get_image(&id).is_none() {
                    self.emit(MapEvent::StyleImageMissing { id });
                }
                self.on_update();
            }
            RendererEvent::UnusedStyleImages(ids) => {
                if self.unused_style_image_policy == UnusedStyleImagePolicy::Remove {
                    for id in ids {
                        if self.style.get_image(&id).is_some_and(|image| !image.keep_unused) {
                            self.style.remove_image(&id);
                        }
                    }
                }
            }
            RendererEvent::GlyphsRequested { font_stack, range } => {
                self.emit(MapEvent::GlyphsRequested { font_stack, range });
            }
            RendererEvent::GlyphsLoaded { font_stack, range } => {
                self.emit(MapEvent::GlyphsLoaded { font_stack, range });
            }
            RendererEvent::GlyphsError { font_stack, range, error } => {
                self.emit(MapEvent::GlyphsError { font_stack, range, error });
            }
            RendererEvent::TileAction { op, id, source_id } => self.emit(MapEvent::TileAction { op, id, source_id }),
            RendererEvent::RenderError(error) => self.emit(MapEvent::RenderError(error)),
        }
    }

    /// Fails the still image being rendered.
    fn on_resource_error(&mut self, error: Error) {
        if self.mode != MapMode::Continuous
            && let Some(callback) = self.still_image_request.take()
        {
            callback(Err(error));
        }
    }

    fn on_render_frame_finished(
        &mut self,
        mode: mln_renderer::RenderMode,
        needs_repaint: bool,
        placement_changed: bool,
        still_image_request_id: u64,
        stats: Arc<RenderingStats>,
    ) {
        let fully_loaded = mode == mln_renderer::RenderMode::Full;
        if std::mem::replace(&mut self.renderer_fully_loaded, fully_loaded) != fully_loaded {
            self.changed();
        }

        if let Some(view) = self.rendering_stats_view.as_mut() {
            view.update(&mut self.style, &stats);
        }

        if self.mode == MapMode::Continuous {
            self.emit(MapEvent::RenderFrameFinished(RenderFrameStatus {
                mode: if fully_loaded { RenderMode::Full } else { RenderMode::Partial },
                needs_repaint,
                placement_changed,
                rendering_stats: Arc::clone(&stats),
            }));

            if needs_repaint || self.transform.in_transition() {
                self.on_update();
            } else if self.renderer_fully_loaded {
                self.emit(MapEvent::Idle);
            }
        } else if self.renderer_fully_loaded
            && still_image_request_id == self.still_image_request_id
            && let Some(callback) = self.still_image_request.take()
        {
            callback(Ok(()));
        }

        self.last_frame_stats = Some(stats);
    }

    /// Records a change to the map's state.
    fn changed(&mut self) {
        self.generation += 1;
    }

    /// Counts a change to state that the map's owner keeps for the map, such
    /// as which of its events a runtime reports, in the map's
    /// [`generation`](Self::generation).
    pub fn record_change(&mut self) {
        self.changed();
    }

    /// Publishes an update of the map's state for the renderer.
    fn on_update(&mut self) {
        // Each update request comes from a change, so the generation grows
        // even in still mode, where no update goes out.
        self.changed();

        // Don't load/render anything in still mode until explicitly requested.
        if self.mode != MapMode::Continuous && self.still_image_request.is_none() {
            return;
        }

        // Still modes have no clock: they show every transition as ended.
        let time_point =
            (self.mode == MapMode::Continuous).then(|| self.transform.frame_time().unwrap_or_else(Instant::now));

        self.transform.update_transitions(time_point);
        let projection = self.projection_at(self.transform.get_zoom());
        self.transform.set_projection_definition(&projection);

        let style_impl = &self.style.impl_;
        let params = UpdateParameters {
            style_loaded: style_impl.is_loaded(),
            mode: self.mode,
            pixel_ratio: self.pixel_ratio,
            debug_options: self.debug_options,
            time_point,
            transform_state: self.transform.get_state().clone(),
            glyph_url: style_impl.get_glyph_url().to_owned(),
            font_faces: style_impl.get_font_faces(),
            sprite_loaded: style_impl.are_sprites_loaded(),
            transition_options: style_impl.get_transition_options(),
            light: style_impl.get_light_impl(),
            sky: style_impl.get_sky_impl(),
            images: style_impl.get_image_impls(),
            sources: style_impl.get_source_impls(),
            layers: style_impl.get_layer_impls(),
            global_state: Some(style_impl.get_global_state_shared()),
            feature_states: self.feature_state.shared(),
            file_source: self.file_source.clone(),
            prefetch_zoom_delta: self.prefetch_zoom_delta,
            still_image_request: self.still_image_request.is_some(),
            still_image_request_id: self.still_image_request_id,
            cross_source_collisions: self.cross_source_collisions,
            fast_pfor_enabled: self.fast_pfor_enabled,
            capture_rendered_features: self.capture_rendered_features,
            tile_lod_min_radius: self.tile_lod_min_radius,
            tile_lod_scale: self.tile_lod_scale,
            tile_lod_pitch_threshold: self.tile_lod_pitch_threshold,
            tile_lod_zoom_shift: self.tile_lod_zoom_shift,
            tile_lod_mode: self.tile_lod_mode,
            subdivision_granularity: if self.mode == MapMode::Tile {
                SubdivisionGranularitySetting::none()
            } else {
                style_impl.get_projection().impl_.get_subdivision_granularity()
            },
        };

        self.updates.publish(Arc::new(params));
    }

    /// Jumps the camera without handling the queued events first.
    fn jump(&mut self, camera: &CameraOptions) {
        self.camera_mutated = true;
        // The style's projection decides how the camera is constrained, so it
        // has to be in place first.
        let zoom = camera.zoom.unwrap_or_else(|| self.transform.get_zoom());
        let projection = self.projection_at(zoom);
        self.transform.set_projection_definition(&projection);
        self.transform.jump_to(camera);
        self.on_update();
    }

    /// The style's projection at the zoom; always Mercator in tile mode,
    /// whose tiles are Mercator tiles.
    fn projection_at(&self, zoom: f64) -> ProjectionDefinition {
        if self.mode == MapMode::Tile {
            return ProjectionDefinition::new(ProjectionType::Mercator);
        }
        self.style.impl_.get_projection().impl_.evaluate(zoom as f32)
    }

    /// Gives the transform the style's current projection, which constrains
    /// the target of an animation it starts.
    fn share_projection_with_transform(&mut self) {
        let projection = (self.mode != MapMode::Tile).then(|| Arc::clone(&self.style.impl_.get_projection().impl_));
        self.transform.set_projection_for_zoom(Some(Rc::new(move |zoom: f64| match &projection {
            Some(projection) => projection.evaluate(zoom as f32),
            None => ProjectionDefinition::new(ProjectionType::Mercator),
        })));
    }

    // MARK: - Still images

    /// Register a callback that will get called (on the render thread) when
    /// all resources have been loaded and a complete render occurs.
    pub fn render_still(&mut self, callback: StillImageCallback) {
        self.process_events();
        if self.mode != MapMode::Static && self.mode != MapMode::Tile {
            callback(Err(Error::invalid_state("Map is not in static or tile image render modes")));
            return;
        }

        if self.still_image_request.is_some() {
            callback(Err(Error::invalid_state("Map is currently rendering an image")));
            return;
        }

        if let Some(error) = self.style.impl_.get_last_error() {
            callback(Err(error));
            return;
        }

        self.still_image_request = Some(callback);
        self.still_image_request_id += 1;

        self.on_update();
    }

    /// The still image request that waits for a frame, as the
    /// `still_image_request_id` of the updates that ask for it, or `None`
    /// once it completed or failed.
    #[must_use]
    pub fn pending_still_image(&self) -> Option<u64> {
        self.still_image_request.as_ref().map(|_| self.still_image_request_id)
    }

    /// Triggers a repaint.
    pub fn trigger_repaint(&mut self) {
        self.process_events();
        self.on_update();
    }

    // MARK: - Feature state

    /// Merges `state` into the state of the feature `feature_id` of the
    /// source `source_id`, in its source layer `source_layer_id`: its keys
    /// replace the same keys of the feature's state, and the other keys
    /// stay. `None`, like the empty string, names no source layer.
    ///
    /// The map keeps the state until it is removed or a style loads, which
    /// removes the state of every source, and sends it to the renderer with
    /// its updates. Publishes an update when the state changes. Returns
    /// whether it changed.
    pub fn set_feature_state(
        &mut self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
        state: mln_style::expression::expression::FeatureState,
    ) -> bool {
        self.process_events();
        let changed = self.feature_state.set(source_id, source_layer_id.unwrap_or(""), feature_id, state);
        if changed {
            self.on_update();
        }
        changed
    }

    /// The state of the feature `feature_id` of the source `source_id`, in
    /// its source layer `source_layer_id`, empty when it has none.
    pub fn get_feature_state(
        &self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
    ) -> mln_style::expression::expression::FeatureState {
        self.feature_state.get(source_id, source_layer_id.unwrap_or(""), feature_id)
    }

    /// Removes the state of the feature `feature_id` of the source
    /// `source_id`, in its source layer `source_layer_id`: only its key
    /// `state_key` when there is one, and that of every feature in the
    /// source layer without `feature_id`. Publishes an update when the state
    /// changes. Returns whether it changed.
    pub fn remove_feature_state(
        &mut self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: Option<&str>,
        state_key: Option<&str>,
    ) -> bool {
        self.process_events();
        let changed = self.feature_state.remove(source_id, source_layer_id.unwrap_or(""), feature_id, state_key);
        if changed {
            self.on_update();
        }
        changed
    }

    /// Removes the state of every feature of the source `source_id`, as when
    /// the source leaves the style. Publishes an update when the state
    /// changes. Returns whether it changed.
    pub fn remove_source_feature_state(&mut self, source_id: &str) -> bool {
        self.process_events();
        let changed = self.feature_state.remove_source(source_id);
        if changed {
            self.on_update();
        }
        changed
    }

    // MARK: - Style

    /// The map's style.
    pub fn style(&self) -> &Style {
        &self.style
    }

    /// The map's style, to change. The map handles what the changes report
    /// at its next call that takes `&mut self`.
    pub fn style_mut(&mut self) -> &mut Style {
        self.process_events();
        self.changed();
        &mut self.style
    }

    // MARK: - Transitions

    pub fn cancel_transitions(&mut self) {
        self.process_events();
        self.transform.cancel_transitions();
        self.on_update();
    }

    /// Cancels the commands whose [`AnimationOptions::transition_id`] equals
    /// `transition_id`, and leaves the others running. Requests an update only
    /// when a command ended.
    pub fn cancel_transitions_with_id(&mut self, transition_id: u64) {
        self.process_events();
        let ended = self.transform.cancel_transitions_with_id(transition_id);
        if ended {
            self.on_update();
        }
    }

    /// Moves the camera relative to where it is, as a gesture does
    /// (maplibre-native-ffi's `mln_map_apply_camera_delta`): a pan, a zoom,
    /// turn and tilt around an anchor, a gesture phase, and the momentum a
    /// gesture's end continues with. See [`CameraDelta`].
    pub fn apply_camera_delta(&mut self, delta: &CameraDelta) -> Result<(), CameraError> {
        self.process_events();
        self.transform.apply_camera_delta(delta)?;
        self.camera_mutated = true;
        self.on_update();
        Ok(())
    }

    /// Moves the camera to absolute values, by a jump, an ease or a flight,
    /// as a command or as part of a gesture (maplibre-native-ffi's
    /// `mln_map_update_camera`). See [`CameraUpdate`].
    pub fn update_camera(&mut self, update: &CameraUpdate) {
        self.process_events();
        self.camera_mutated = true;
        self.transform.update_camera(update);
        self.on_update();
    }

    /// On the globe, let drags and zooms around a point turn the bearing so
    /// the planet keeps its orientation, for maps whose rotation gestures are
    /// enabled. Off, the default, they hold the bearing as GL JS does.
    pub fn set_globe_free_bearing(&mut self, free_bearing: bool) {
        self.process_events();
        if free_bearing != self.is_globe_free_bearing() {
            self.transform.set_globe_free_bearing(free_bearing);
            self.changed();
        }
    }

    pub fn is_globe_free_bearing(&self) -> bool {
        self.transform.is_globe_free_bearing()
    }

    /// Samples the camera's animations at the host's frame time, from now on,
    /// and starts new ones from it: call it at the start of each frame with
    /// `Instant::now()`, before rendering. One older than
    /// `STALE_FRAME_TIME` counts as
    /// unset, so commands made while the host isn't drawing start now. If an
    /// animation runs, the map updates for this frame at once. `None` goes
    /// back to sampling at `Instant::now()` whenever the map updates. Still
    /// images ignore it. See
    /// [`Transform::set_frame_time`](crate::transform::Transform::set_frame_time).
    pub fn set_frame_time(&mut self, time: Option<Instant>) {
        self.process_events();
        self.transform.set_frame_time(time);
        if self.transform.in_transition() {
            self.on_update();
        }
    }

    /// Whether any camera command is still animating. A command's
    /// [`MapEvent::CameraDidChange`] ends that command only, so this
    /// tells whether the camera stopped once the last one ended.
    pub fn in_transition(&self) -> bool {
        self.transform.in_transition()
    }

    // MARK: - Camera

    pub fn get_camera_options(&self, padding: Option<EdgeInsets>) -> CameraOptions {
        self.transform.get_camera_options(padding)
    }

    pub fn jump_to(&mut self, camera: &CameraOptions) {
        self.process_events();
        self.jump(camera);
    }

    // query: the render-test runner's tile mode uses it.
    /// The bounds of the area the map shows from `camera`.
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a camera whose
    /// corners have no `LatLng`.
    pub fn lat_lng_bounds_for_camera(&self, camera: &CameraOptions) -> Result<LatLngBounds, Error> {
        let mut shallow = Transform::from_state(self.transform.get_state().clone());
        let size = shallow.get_state().get_size();

        shallow.jump_to(camera);
        if shallow.get_state().is_globe_rendering() {
            return globe_bounds(&shallow);
        }
        Ok(LatLngBounds::hull(
            shallow.screen_coordinate_to_lat_lng(&ScreenCoordinate::new(0.0, 0.0), WrapMode::Wrapped)?,
            shallow.screen_coordinate_to_lat_lng(
                &ScreenCoordinate::new(f64::from(size.width), f64::from(size.height)),
                WrapMode::Wrapped,
            )?,
        ))
    }

    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a NaN or infinite
    /// point.
    pub fn move_by(&mut self, point: &ScreenCoordinate, animation: &AnimationOptions) -> Result<(), Error> {
        self.process_events();
        self.camera_mutated = true;
        self.transform.move_by(point, animation)?;
        self.on_update();
        Ok(())
    }

    // gtests-core: cameraForLatLngBounds, cameraForLatLngs,
    // latLngBoundsForCameraUnwrapped (test/map/map.test.cpp).
    /// The camera that fits `bounds` in the view, inset by `padding`, at
    /// `bearing` and `pitch` (the current ones for `None`). Fails with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a center that has no
    /// `LatLng` (padding that pushes it to infinity).
    pub fn camera_for_lat_lng_bounds(
        &self,
        bounds: &LatLngBounds,
        padding: &EdgeInsets,
        bearing: Option<f64>,
        pitch: Option<f64>,
    ) -> Result<CameraOptions, Error> {
        self.camera_for_lat_lngs(
            &[bounds.northwest(), bounds.southwest(), bounds.southeast(), bounds.northeast()],
            padding,
            bearing,
            pitch,
        )
    }

    /// The camera that fits every point of `lat_lngs` in the view, inset by
    /// `padding`, at `bearing` and `pitch` (the current ones for `None`).
    /// Fails as [`camera_for_lat_lng_bounds`](Self::camera_for_lat_lng_bounds)
    /// does.
    pub fn camera_for_lat_lngs(
        &self,
        lat_lngs: &[LatLng],
        padding: &EdgeInsets,
        bearing: Option<f64>,
        pitch: Option<f64>,
    ) -> Result<CameraOptions, Error> {
        if bearing.is_none() && pitch.is_none() {
            return camera_for_lat_lngs(lat_lngs, &self.transform, padding);
        }

        let mut transform = Transform::from_state(self.transform.get_state().clone());

        transform.jump_to(&CameraOptions { bearing, pitch, ..Default::default() });

        Ok(CameraOptions {
            bearing: Some(mln_math::rad2deg(-transform.get_bearing())),
            pitch: Some(mln_math::rad2deg(transform.get_pitch())),
            ..camera_for_lat_lngs(lat_lngs, &transform, padding)?
        })
    }

    /// The camera that fits every point of `geometry` (x longitude, y
    /// latitude), as [`camera_for_lat_lngs`](Self::camera_for_lat_lngs) fits
    /// them. Fails as that does, and with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a point with no
    /// `LatLng` (an infinite or NaN coordinate, or a latitude beyond ±90°).
    pub fn camera_for_geometry(
        &self,
        geometry: &Geometry<f64>,
        padding: &EdgeInsets,
        bearing: Option<f64>,
        pitch: Option<f64>,
    ) -> Result<CameraOptions, Error> {
        let mut points = Vec::new();
        for_each_point(geometry, &mut |point| points.push(*point));
        let lat_lngs = points.iter().map(|point| LatLng::new(point.y, point.x)).collect::<Result<Vec<_>, _>>()?;
        self.camera_for_lat_lngs(&lat_lngs, padding, bearing, pitch)
    }

    /// The bounds of the area the map shows from `camera`, with the corners
    /// unwrapped toward the center, so that the bounds can cross the
    /// antimeridian. Fails as
    /// [`lat_lng_bounds_for_camera`](Self::lat_lng_bounds_for_camera) does.
    pub fn lat_lng_bounds_for_camera_unwrapped(&self, camera: &CameraOptions) -> Result<LatLngBounds, Error> {
        let mut shallow = Transform::from_state(self.transform.get_state().clone());
        let size = shallow.get_state().get_size();

        shallow.jump_to(camera);
        if shallow.get_state().is_globe_rendering() {
            return globe_bounds(&shallow);
        }
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

    pub fn set_bounds(&mut self, options: &BoundOptions) -> Result<(), Error> {
        self.process_events();
        if *options != BoundOptions::default() {
            self.changed();
        }
        let mut change_camera = false;
        let mut camera_options = CameraOptions::default();

        {
            let transform = &mut self.transform;

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
                    camera_options = CameraOptions { zoom: Some(min_zoom), ..camera_options };
                }
            }

            if let Some(max_zoom) = options.max_zoom {
                transform.set_max_zoom(max_zoom);
                if transform.get_zoom() > max_zoom {
                    change_camera = true;
                    camera_options = CameraOptions { zoom: options.max_zoom, ..camera_options };
                }
            }

            if let Some(max_pitch) = options.max_pitch {
                transform.set_max_pitch(max_pitch);
                if transform.get_pitch() > transform.get_state().get_max_pitch() {
                    change_camera = true;
                    camera_options = CameraOptions { pitch: options.max_pitch, ..camera_options };
                }
            }

            if let Some(min_pitch) = options.min_pitch {
                transform.set_min_pitch(min_pitch);
                if transform.get_pitch() < transform.get_state().get_min_pitch() {
                    change_camera = true;
                    camera_options = CameraOptions { pitch: options.min_pitch, ..camera_options };
                }
            }
        }

        if change_camera {
            self.jump_to(&camera_options);
        }
        Ok(())
    }

    // MARK: - Map options

    pub fn set_north_orientation(&mut self, orientation: NorthOrientation) {
        self.process_events();
        self.transform.set_north_orientation(orientation);
        self.on_update();
    }

    pub fn set_constrain_mode(&mut self, mode: ConstrainMode) {
        self.process_events();
        self.transform.set_constrain_mode(mode);
        self.on_update();
    }

    pub fn set_viewport_mode(&mut self, mode: ViewportMode) {
        self.process_events();
        self.transform.set_viewport_mode(mode);
        self.on_update();
    }

    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an empty size.
    pub fn set_size(&mut self, size: Size) -> Result<(), Error> {
        self.process_events();
        self.transform.resize(size)?;
        self.on_update();
        Ok(())
    }

    /// Sets the map's pixel ratio: device pixels per logical pixel. The
    /// renderer draws the next frame at the new ratio and lays the loaded
    /// tiles out again without loading them, and the style loads its sprites
    /// again where the ratio picks their other resolution.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// ratio that isn't finite and positive.
    pub fn set_pixel_ratio(&mut self, pixel_ratio: f32) -> Result<(), Error> {
        if !pixel_ratio.is_finite() || pixel_ratio <= 0.0 {
            return Err(Error::invalid_argument(format!("pixel ratio {pixel_ratio} isn't finite and positive")));
        }
        self.process_events();
        if pixel_ratio == self.pixel_ratio {
            return Ok(());
        }
        self.pixel_ratio = pixel_ratio;
        self.style.impl_.set_pixel_ratio(pixel_ratio);
        self.on_update();
        Ok(())
    }

    pub fn get_pixel_ratio(&self) -> f32 {
        self.pixel_ratio
    }

    pub fn set_frustum_offset(&mut self, frustum_offset: &EdgeInsets) {
        self.process_events();
        self.transform.set_frustum_offset(frustum_offset);
        self.on_update();
    }

    pub fn get_map_options(&self) -> MapOptions {
        let transform = &self.transform;
        let state = transform.get_state();
        MapOptions {
            north_orientation: transform.get_north_orientation(),
            map_mode: self.mode,
            constrain_mode: state.get_constrain_mode(),
            viewport_mode: state.get_viewport_mode(),
            cross_source_collisions: self.cross_source_collisions,
            size: state.get_size(),
            pixel_ratio: self.pixel_ratio,
            ..Default::default()
        }
    }

    // MARK: - Projection mode

    pub fn set_projection_mode(&mut self, options: &ProjectionMode) {
        self.process_events();
        self.transform.set_projection_mode(options);
        self.on_update();
    }

    pub fn get_projection_mode(&self) -> ProjectionMode {
        self.transform.get_projection_mode()
    }

    // MARK: - Transform

    pub fn get_transform_state(&self) -> TransformState {
        self.transform.get_state().clone()
    }

    // MARK: - Tile LOD

    pub fn set_tile_lod_min_radius(&mut self, radius: f64) {
        self.process_events();
        if self.tile_lod_min_radius == radius {
            return;
        }
        self.tile_lod_min_radius = radius;
        self.on_update();
    }

    pub fn set_tile_lod_scale(&mut self, scale: f64) {
        self.process_events();
        if self.tile_lod_scale == scale {
            return;
        }
        self.tile_lod_scale = scale;
        self.on_update();
    }

    pub fn set_tile_lod_pitch_threshold(&mut self, threshold: f64) {
        self.process_events();
        if self.tile_lod_pitch_threshold == threshold {
            return;
        }
        self.tile_lod_pitch_threshold = threshold;
        self.on_update();
    }

    pub fn get_tile_lod_pitch_threshold(&self) -> f64 {
        self.tile_lod_pitch_threshold
    }

    pub fn set_tile_lod_zoom_shift(&mut self, shift: f64) {
        self.process_events();
        if self.tile_lod_zoom_shift == shift {
            return;
        }
        self.tile_lod_zoom_shift = shift;
        self.on_update();
    }

    pub fn set_tile_lod_mode(&mut self, mode: TileLodMode) {
        self.process_events();
        if self.tile_lod_mode == mode {
            return;
        }
        self.tile_lod_mode = mode;
        self.on_update();
    }

    // MARK: - Rendered features
    //
    // With `MapOptions::capture_rendered_features` on, these give the
    // features rendered in the most recently completed frame.

    /// The stats of the most recently completed frame, whose
    /// `frame_rendered_features` holds every rendered feature by source and
    /// layer. `None` before the first frame.
    pub(crate) fn get_last_frame_stats(&self) -> Option<Arc<RenderingStats>> {
        self.last_frame_stats.clone()
    }

    /// Calls `result` with the ID and info of each feature rendered in the
    /// last frame that matches the given feature, layer and source IDs
    /// (`None` matches any), until it returns `false`.
    pub fn get_rendered_features(
        &self,
        feature_id: Option<&str>,
        layer_id: Option<&str>,
        source_id: Option<&str>,
        mut result: impl FnMut(&str, &FeatureInfo) -> bool,
    ) {
        let Some(stats) = self.get_last_frame_stats() else {
            return;
        };
        let features = &stats.frame_rendered_features;

        let mut search_features = |layer_features: &LayerFeaturesMap| {
            for (id, info) in layer_features {
                if feature_id.is_none_or(|feature_id| id == feature_id) && !result(id, info) {
                    return false;
                }
            }
            true
        };

        if let (Some(source_id), Some(layer_id)) = (source_id, layer_id) {
            // Both source and layer are specified, look for an exact match
            let key = SourceLayerID { source_id: source_id.to_owned(), layer_id: layer_id.to_owned() };
            if let Some(layer_features) = features.get(&key) {
                search_features(layer_features);
            }
        } else {
            // Linear search for matching items
            for (id, layer_features) in features {
                if source_id.is_none_or(|source_id| id.source_id == source_id)
                    && layer_id.is_none_or(|layer_id| id.layer_id == layer_id)
                    && !search_features(layer_features)
                {
                    break;
                }
            }
        }
    }

    /// The number of matching features rendered in the last frame (see
    /// [`get_rendered_features`](Self::get_rendered_features)).
    pub fn get_rendered_feature_count(
        &self,
        feature_id: Option<&str>,
        layer_id: Option<&str>,
        source_id: Option<&str>,
    ) -> usize {
        let mut count = 0;
        self.get_rendered_features(feature_id, layer_id, source_id, |_, _| {
            count += 1;
            true
        });
        count
    }

    /// Whether any matching feature was rendered in the last frame (see
    /// [`get_rendered_features`](Self::get_rendered_features)).
    pub fn is_feature_rendered(
        &self,
        feature_id: Option<&str>,
        layer_id: Option<&str>,
        source_id: Option<&str>,
    ) -> bool {
        let mut found = false;
        self.get_rendered_features(feature_id, layer_id, source_id, |_, _| {
            found = true;
            false
        });
        found
    }

    // gtests-core: projection and prefetch zoom delta (test/map, test/api).
    // MARK: - Projection

    pub fn pixel_for_lat_lng(&self, lat_lng: &LatLng) -> ScreenCoordinate {
        let transform = &self.transform;
        transform.lat_lng_to_screen_coordinate(&transform.get_state().lat_lng_near_center(lat_lng))
    }

    /// The location under `pixel`. Fails with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a NaN or infinite pixel.
    pub fn lat_lng_for_pixel(&self, pixel: &ScreenCoordinate, wrap_mode: WrapMode) -> Result<LatLng, Error> {
        self.transform.screen_coordinate_to_lat_lng(pixel, wrap_mode)
    }

    /// Whether the globe hides a location from the camera; a Mercator map
    /// hides nothing.
    #[cfg(test)]
    pub(crate) fn is_location_occluded(&self, lat_lng: &LatLng) -> bool {
        self.transform.is_location_occluded(lat_lng)
    }

    pub fn lat_lngs_for_pixels(
        &self,
        screen_coords: &[ScreenCoordinate],
        wrap_mode: WrapMode,
    ) -> Result<Vec<LatLng>, Error> {
        screen_coords.iter().map(|point| self.lat_lng_for_pixel(point, wrap_mode)).collect()
    }

    pub fn set_prefetch_zoom_delta(&mut self, delta: u8) {
        self.process_events();
        if self.prefetch_zoom_delta == delta {
            return;
        }
        self.prefetch_zoom_delta = delta;
        self.on_update();
    }

    pub fn get_prefetch_zoom_delta(&self) -> u8 {
        self.prefetch_zoom_delta
    }

    /// Places the camera by its position and orientation, and cancels the
    /// running camera commands. A NaN position, or a NaN or zero-length
    /// orientation, is ignored; the position's height is clamped to the zoom
    /// range.
    pub fn set_free_camera_options(&mut self, camera: &mln_map_types::FreeCameraOptions) {
        self.process_events();
        self.transform.set_free_camera_options(camera);
        self.camera_mutated = true;
        self.on_update();
    }

    // MARK: - Debug

    pub fn set_debug(&mut self, debug_options: MapDebugOptions) {
        self.process_events();
        self.debug_options = debug_options;
        self.on_update();
    }

    pub fn get_debug(&self) -> MapDebugOptions {
        self.debug_options
    }

    pub fn is_rendering_stats_view_enabled(&self) -> bool {
        self.rendering_stats_view.is_some()
    }

    /// Shows the rendering stats of the last frames in the top right corner,
    /// with a symbol layer (and its source) that the view adds to the style
    /// and keeps there, even when the style is replaced. The text needs the
    /// style's glyphs. Disabling the view removes them.
    pub fn enable_rendering_stats_view(&mut self, value: bool) {
        self.process_events();
        if value != self.is_rendering_stats_view_enabled() {
            self.changed();
        }
        if value {
            if self.is_rendering_stats_view_enabled() {
                return;
            }
            let view = RenderingStatsView::new(RenderingStatsViewOptions::default());
            view.create(&mut self.style);
            self.rendering_stats_view = Some(view);
        } else if let Some(view) = self.rendering_stats_view.take() {
            view.destroy(&mut self.style);
        }
        self.process_events();
    }

    /// Grows with each change to the map's state: each call that changes it,
    /// and each change that loading, rendering and animations make.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Counts the updates that the map has published for its renderer.
    /// An update that a command group holds back counts when it shows.
    pub fn update_generation(&self) -> u64 {
        self.updates.generation()
    }

    /// The generation of the update that shows the map's changes so far:
    /// [`update_generation`](Self::update_generation), or while a command
    /// group holds an update back, the one that the outermost group's end
    /// shows.
    pub fn pending_update_generation(&self) -> u64 {
        self.updates.pending_generation()
    }

    /// Copies the map's state.
    pub fn snapshot(&self) -> MapSnapshot {
        let transform = &self.transform;
        let state = transform.get_state();
        let bounds = state.get_lat_lng_bounds();
        let free_camera = state.get_free_camera_options();
        MapSnapshot {
            generation: self.generation,
            update_generation: self.updates.generation(),
            debug: self.debug_options,
            fully_loaded: self.is_fully_loaded(),
            rendering_stats_view_enabled: self.is_rendering_stats_view_enabled(),
            gesture_in_progress: transform.is_gesture_in_progress(),
            transition_in_progress: transform.in_transition(),
            tile: TileSettings {
                prefetch_zoom_delta: self.prefetch_zoom_delta,
                lod_min_radius: self.tile_lod_min_radius,
                lod_scale: self.tile_lod_scale,
                lod_pitch_threshold: mln_math::rad2deg(self.tile_lod_pitch_threshold),
                lod_zoom_shift: self.tile_lod_zoom_shift,
                lod_mode: self.tile_lod_mode,
            },
            bounds: CameraBounds {
                bounds: (bounds != LatLngBounds::infinite()).then_some(bounds),
                min_zoom: state.get_min_zoom(),
                max_zoom: state.get_max_zoom(),
                min_pitch: mln_math::rad2deg(state.get_min_pitch()),
                max_pitch: mln_math::rad2deg(state.get_max_pitch()),
            },
            free_camera: FreeCamera {
                position: free_camera.position.unwrap_or_default(),
                orientation: free_camera.orientation.unwrap_or_default(),
            },
            view: MapView::from_state(state, f64::from(self.pixel_ratio)),
            globe_rendering: state.is_globe_rendering(),
            globe_free_bearing: transform.is_globe_free_bearing(),
        }
    }

    pub fn is_fully_loaded(&self) -> bool {
        self.style.impl_.is_loaded() && self.renderer_fully_loaded
    }

    /// Logs the style's sources and their state.
    pub fn dump_debug_logs(&self) {
        use mln_util::logging::{Event, Log};
        let rule = "-".repeat(80);
        Log::info(Event::General, &rule);
        self.style.impl_.dump_debug_logs();
        Log::info(Event::General, &rule);
    }
}

// gtests-core: the free function `cameraForLatLngs(latLngs, transform,
// padding)` of map.cpp (Map::cameraForLatLngs and MapProjection use it).
pub(crate) fn camera_for_lat_lngs(
    lat_lngs: &[LatLng],
    transform: &Transform,
    padding: &EdgeInsets,
) -> Result<CameraOptions, Error> {
    if lat_lngs.is_empty() {
        return Ok(CameraOptions::default());
    }
    if transform.get_state().is_globe_rendering() {
        camera_for_lat_lngs_on_globe(lat_lngs, transform, padding)
    } else {
        camera_for_lat_lngs_on_screen(lat_lngs, transform, padding)
    }
}

/// Fails as `Map::camera_for_lat_lngs` does.
fn camera_for_lat_lngs_on_screen(
    lat_lngs: &[LatLng],
    transform: &Transform,
    padding: &EdgeInsets,
) -> Result<CameraOptions, Error> {
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
        min_scale = mln_math::fmin(scale_x, scale_y);
    }

    let mut zoom = transform.get_zoom();
    if min_scale > 0.0 {
        zoom = mln_math::clamp_f64(
            zoom + min_scale.log2(),
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
    let center_pixel =
        ScreenCoordinate::new(f64::midpoint(ne_pixel.x, sw_pixel.x), f64::midpoint(ne_pixel.y, sw_pixel.y));

    Ok(CameraOptions {
        center: Some(transform.screen_coordinate_to_lat_lng(&center_pixel, WrapMode::Wrapped)?),
        padding: Some(*padding),
        zoom: Some(zoom),
        ..Default::default()
    })
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
/// Fails as `Map::camera_for_lat_lngs` does.
fn camera_for_lat_lngs_on_globe(
    lat_lngs: &[LatLng],
    transform: &Transform,
    padding: &EdgeInsets,
) -> Result<CameraOptions, Error> {
    let mut flat = Transform::from_state(transform.get_state().clone());
    flat.set_projection_definition(&ProjectionDefinition::new(ProjectionType::Mercator));
    flat.jump_to(&CameraOptions { pitch: Some(0.0), roll: Some(0.0), ..Default::default() });
    let fit = camera_for_lat_lngs_on_screen(lat_lngs, &flat, padding)?;
    let (Some(fit_center), Some(fit_zoom)) = (fit.center, fit.zoom) else {
        return Ok(fit);
    };

    // Solve with the padding the returned camera is shown with.
    let mut globe = Transform::from_state(transform.get_state().clone());
    globe.jump_to(&CameraOptions {
        center: Some(fit_center),
        zoom: Some(fit_zoom),
        padding: Some(*padding),
        pitch: Some(0.0),
        roll: Some(0.0),
        ..Default::default()
    });
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
        LatLng::new(north, west)?,
        LatLng::new(north, east)?,
        LatLng::new(south, west)?,
        LatLng::new(south, east)?,
        LatLng::new(mid_latitude, east)?,
        LatLng::new(mid_latitude, west)?,
        LatLng::new(north, mid_longitude)?,
        LatLng::new(south, mid_longitude)?,
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
        return Ok(fit);
    }
    Ok(CameraOptions {
        zoom: Some(f64::min(globe.get_zoom() + state.scale_zoom(smallest), state.get_max_zoom())),
        ..fit
    })
}

/// GL JS's globe bounds: the farthest of eight edge points on each side of
/// the center, where points off the planet fall on its horizon, widened to
/// every longitude when a pole is on screen.
/// Fails as `Map::lat_lng_bounds_for_camera` does.
fn globe_bounds(transform: &Transform) -> Result<LatLngBounds, Error> {
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
        let lat_lng = transform.screen_coordinate_to_lat_lng(&point, WrapMode::Wrapped)?;
        let longitude =
            VerticalPerspectiveProjection::difference_of_angles_degrees(center.longitude(), lat_lng.longitude());
        let latitude =
            VerticalPerspectiveProjection::difference_of_angles_degrees(center.latitude(), lat_lng.latitude());
        west = f64::min(west, longitude);
        east = f64::max(east, longitude);
        south = f64::min(south, latitude);
        north = f64::max(north, latitude);
    }
    west += center.longitude();
    east += center.longitude();
    south += center.latitude();
    north += center.latitude();

    let on_screen = |pole: &LatLng| {
        // In front of the camera and inside its depth range, as GL JS's
        // isSurfacePointOnScreen asks.
        let (_, clip) = VerticalPerspectiveProjection::lat_lng_to_screen_coordinate(transform.get_state(), pole);
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
    if on_screen(&LatLng::new(90.0, 0.0)?) {
        north = 90.0;
        west = -180.0;
        east = 180.0;
    }
    if on_screen(&LatLng::new(-90.0, 0.0)?) {
        south = -90.0;
        west = -180.0;
        east = 180.0;
    }
    Ok(LatLngBounds::hull(LatLng::new(south, west)?, LatLng::new(north, east)?))
}
