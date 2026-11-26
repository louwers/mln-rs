// The map owns its style versions, its transform and its style loader. An
// edit reacts in the call that makes it; the loader and the renderer report
// through queues that the map drains in `process_events`.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use geo_types::Geometry;
use glam::{DMat4, DVec3};
use mln_map_types::{
    AnimationOptions, BoundOptions, CameraDelta, CameraError, CameraLimits, CameraOptions, CameraUpdate, Elevation,
    MapMode, ProjectionMode, TerrainOptions, TileLodMode, TransformState, VerticalPerspectiveProjection, Viewport,
};
use mln_renderer::{
    Registry, RendererEvent, RendererEvents, RenderingStats, UpdateParameters, UpdateReader, UpdateSlot,
};
use mln_storage::FileSource;
#[cfg(not(target_family = "wasm"))]
use mln_storage::{Loader, ResourceOptions};
use mln_style::conversion::Convertible;
use mln_style::geometry::for_each_point;
use mln_style::source::convert_source;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::types::{ProjectionDefinition, ProjectionType};
use mln_style::{Edit, Source, Style, StyleRegistry};
use mln_text::glyph::{FontFace, FontFaces};
use mln_util::error::Error;
use mln_util::event_queue::{EventQueue, Waker};
use mln_util::geo::{EdgeInsets, LatLng, LatLngBounds, ScreenCoordinate, WrapMode};
use mln_util::pool::Pool;
use mln_util::size::Size;
use mln_util::time::Instant;

use crate::feature_state::FeatureStateStore;
use crate::log_target;
use crate::map_event::{MapEvent, RenderFrameStatus};
use crate::map_options::{MapOptions, UnusedStyleImagePolicy};
use crate::map_snapshot::{CameraBounds, FreeCamera, MapSnapshot, TileSettings};
use crate::style_loader::{LoadEvent, StyleLoader};
use crate::transform::{CameraCommandId, CommandEnd, FrameClock, TerrainGround, Transform, TransformEvent};

/// How many times the terrain may move the camera while a still image
/// waits, before the map stops following it for that image: a camera whose
/// ground changes with each move would otherwise never settle.
const MAX_GROUND_MOVES_PER_STILL: u32 = 8;

/// The camera options of `style`'s default camera, which the map jumps to
/// once the style loads, unless a camera command came first.
#[must_use]
pub fn style_camera(style: &Style) -> CameraOptions {
    let camera = style.get_default_camera();
    CameraOptions {
        center: camera.center,
        center_altitude: camera.center_altitude,
        zoom: camera.zoom,
        bearing: camera.bearing,
        pitch: camera.pitch,
        roll: camera.roll,
        ..CameraOptions::default()
    }
}

/// How long a continuous map waits to draw again after the first of a run
/// of frames that failed on the GPU, in case the failure passes.
const RETRY_DELAY: Duration = Duration::from_millis(200);

/// Where a still image that [`Map::render_still`] asked for stands.
#[derive(Debug, PartialEq)]
enum StillImage {
    None,
    /// Waits for a complete frame.
    Pending,
    /// Ended, until [`Map::take_still_image`] takes it.
    Ended(Result<(), Error>),
}

/// A map: its style, its camera, and the updates it publishes for its
/// renderer.
///
/// The map publishes its state for a renderer in an update slot, which a
/// renderer reads through [`update_reader`](Self::update_reader) and
/// reports back to through [`renderer_events`](Self::renderer_events).
/// The style is an `Arc<Style>` version: [`edit_style`](Self::edit_style)
/// makes the next one and publishes it at once. The style's loader and the
/// renderer report to the map through queues, which the map handles only
/// in [`process_events`](Self::process_events) (which
/// [`drain_events`](Self::drain_events) calls): an owner calls it once per
/// phase, before the commands that should see what was reported, such as a
/// camera command that should land after the style's default camera. What
/// the map reports to its owner waits in another queue, for `drain_events`.
/// An owner sets a waker ([`set_waker`](Self::set_waker)) that schedules
/// those calls on its thread.
pub struct Map {
    updates: UpdateSlot,
    transform: Transform,

    mode: MapMode,
    pixel_ratio: f32,
    cross_source_collisions: bool,
    fast_pfor_enabled: bool,
    unused_style_image_policy: UnusedStyleImagePolicy,
    /// How the map draws its terrain.
    terrain: TerrainOptions,
    /// The ground of the terrain as the renderer last drew it, while
    /// terrain draws. Each update carries it, and the renderer completes a
    /// frame only of an update that carries its newest ground.
    elevation: Option<Arc<dyn Elevation>>,
    /// The terrain's moves of the camera while the still image waits.
    ground_moves: u32,

    /// The current version of the style.
    style: Arc<Style>,
    /// The faces that the map draws the style's text with: its
    /// `font-faces`, then the fallback faces in effect when it parsed.
    font_faces: Option<Arc<FontFaces>>,
    /// Loads the style's document, its sprites and its sources'
    /// descriptions.
    loader: StyleLoader,
    /// The state of the sources' features, which each update carries.
    feature_state: FeatureStateStore,

    camera_mutated: bool,
    /// How the map picks the tiles to load, as its snapshot reports it.
    tile: TileSettings,
    loading: bool,
    renderer_fully_loaded: bool,
    /// When the last frame said the map changes next on its own, until an
    /// update goes out.
    repaint_at: Option<Instant>,
    /// Whether the last frame of a continuous map failed on the GPU.
    render_failing: bool,
    /// A still image that [`Map::render_still`] asked for, until it ends.
    still_image: StillImage,
    /// Counts the still image requests, so the renderer can tell a new one.
    still_image_request_id: u64,
    /// The `update_id` of the last update the map made.
    update_id: u64,

    /// Grows with each change to the map's state.
    generation: u64,

    renderer_events: RendererEvents,
    /// What the map reports to its owner.
    events: EventQueue<MapEvent>,
    /// The ends of the camera commands, for its owner.
    command_ends: Vec<CommandEnd>,

    /// Declared after `style`, so the style and its requests go first.
    file_source: Option<Arc<dyn FileSource>>,
    /// Where the map's background work runs.
    pool: Pool,
}

impl Map {
    /// A map with a [`Loader`] of its own for the default
    /// `ResourceOptions`, whose background work runs on `pool`.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an empty `map_options.size`.
    #[cfg(not(target_family = "wasm"))]
    pub fn new(map_options: MapOptions, pool: &Pool) -> Result<Map, Error> {
        Self::with_resource_options(map_options, &ResourceOptions::default(), pool)
    }

    /// A map with a [`Loader`] of its own for `resource_options`, on
    /// `pool`.
    #[cfg(not(target_family = "wasm"))]
    pub fn with_resource_options(
        map_options: MapOptions,
        resource_options: &ResourceOptions,
        pool: &Pool,
    ) -> Result<Map, Error> {
        let loader: Arc<dyn FileSource> = Arc::new(Loader::new(resource_options, pool));
        Self::with_registry(loader, map_options, Arc::clone(Registry::with_builtins().style()), pool)
    }

    // gtests-core: the C++ tests' `MapAdapter` (test/include/mln/test/
    // map_adapter.hpp): a map over a given file source.
    /// A map whose resources come from `file_source`, on `pool`.
    pub fn with_file_source(
        file_source: Arc<dyn FileSource>,
        map_options: MapOptions,
        pool: &Pool,
    ) -> Result<Map, Error> {
        Self::with_registry(file_source, map_options, Arc::clone(Registry::with_builtins().style()), pool)
    }

    /// A map whose resources come from `file_source` and whose style parses
    /// against `registry`, on `pool`. The other constructors
    /// use the built-in registrations.
    ///
    /// The renderer that draws the map must be given a registry filled by
    /// the same calls as `registry` ([`mln_renderer::Registry::style`]): it
    /// finds a layer's type by name, and draws no layer whose type it lacks.
    pub fn with_registry(
        file_source: Arc<dyn FileSource>,
        map_options: MapOptions,
        registry: Arc<StyleRegistry>,
        pool: &Pool,
    ) -> Result<Map, Error> {
        let events = EventQueue::new();
        let renderer_events = RendererEvents::new();
        let pixel_ratio = map_options.pixel_ratio;
        let style = Arc::new(Style::new(registry));
        let loader = StyleLoader::new(Some(Arc::clone(&file_source)), pool, pixel_ratio);
        let mut transform = Transform::new(map_options.constrain_mode, map_options.viewport_mode);
        transform.set_north_orientation(map_options.north_orientation);
        transform.set_center_clamped_to_ground(map_options.center_clamped_to_ground);
        let mut map = Map {
            updates: UpdateSlot::new(),
            transform,
            mode: map_options.map_mode,
            pixel_ratio,
            cross_source_collisions: map_options.cross_source_collisions,
            fast_pfor_enabled: map_options.fast_pfor_enabled,
            unused_style_image_policy: map_options.unused_style_image_policy,
            terrain: map_options.terrain,
            elevation: None,
            ground_moves: 0,
            style,
            font_faces: None,
            loader,
            feature_state: FeatureStateStore::default(),
            camera_mutated: false,
            tile: TileSettings {
                // util::DEFAULT_PREFETCH_ZOOM_DELTA
                prefetch_zoom_delta: 4,
                lod_min_radius: 3.0,
                lod_scale: 1.0,
                lod_pitch_threshold: 60.0,
                lod_zoom_shift: 0.0,
                lod_mode: TileLodMode::Default,
            },
            loading: false,
            renderer_fully_loaded: false,
            repaint_at: None,
            render_failing: false,
            still_image: StillImage::None,
            still_image_request_id: 0,
            update_id: 0,
            generation: 1,
            renderer_events,
            events,
            command_ends: Vec::new(),
            file_source: Some(file_source),
            pool: pool.clone(),
        };
        map.share_projection_with_transform();
        map.transform.resize(map_options.size)?;
        Ok(map)
    }

    /// Where the map's background work runs.
    #[must_use]
    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    // MARK: - Renderer

    /// A reader of the updates that the map publishes for its renderer.
    #[must_use]
    pub fn update_reader(&self) -> UpdateReader {
        self.updates.reader()
    }

    /// The queue that the map's renderer reports its events to.
    #[must_use]
    pub fn renderer_events(&self) -> RendererEvents {
        self.renderer_events.clone()
    }

    /// Holds the map's updates back from its renderer until the matching
    /// [`end_command_group`](Self::end_command_group), so the changes in
    /// between show in the same frame. Groups nest, and the changes show
    /// when the outermost group ends.
    pub fn begin_command_group(&mut self) {
        self.updates.begin_group();
    }

    /// Ends the group that [`begin_command_group`](Self::begin_command_group)
    /// began. Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind)
    /// when no group is open.
    pub fn end_command_group(&mut self) -> Result<(), Error> {
        self.updates.end_group()
    }

    /// How many command groups are open.
    #[must_use]
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
        self.updates.release();
    }

    // MARK: - Events

    /// Sets the waker that runs, on any thread, when the map has events to
    /// handle or to drain. It must not call the map: it schedules a call of
    /// [`drain_events`](Self::drain_events) or
    /// [`process_events`](Self::process_events) on the map's thread, as by
    /// posting to the owner's `Queue`.
    pub fn set_waker(&mut self, waker: Option<Waker>) {
        self.loader.set_waker(waker.clone());
        self.renderer_events.set_waker(waker.clone());
        self.events.set_waker(waker);
    }

    /// Whether the style or the renderer reported something that
    /// [`process_events`](Self::process_events) hasn't handled. A report
    /// counts once it is queued, though its waker may not have run yet.
    #[must_use]
    pub fn has_reports(&self) -> bool {
        self.loader.has_replies() || !self.renderer_events.is_empty()
    }

    /// Sets a waker as [`set_waker`](Self::set_waker) does, that runs only
    /// when the style or the renderer reports something for the map to
    /// handle, which can come from any thread. The events that the map's own
    /// calls report don't run it, for an owner that drains them after each
    /// call.
    pub fn set_reports_waker(&mut self, waker: Option<Waker>) {
        self.loader.set_waker(waker.clone());
        self.renderer_events.set_waker(waker);
        self.events.set_waker(None);
    }

    /// Handles what the style and the renderer reported since the last
    /// call: loads, frames, errors and missing images. The map's reactions,
    /// such as an update for the renderer or a still image's callback, run
    /// here, and its events wait for [`drain_events`](Self::drain_events).
    pub fn process_events(&mut self) {
        loop {
            let replies = self.loader.take_replies();
            let renderer_events = self.renderer_events.take();
            if replies.is_empty() && renderer_events.is_empty() {
                break;
            }
            for reply in replies {
                let events = self.loader.on_reply(&self.style, reply);
                self.handle_load_events(events);
            }
            for event in renderer_events {
                self.handle_renderer_event(event);
            }
        }
    }

    /// Handles what the style and the renderer reported, as
    /// [`process_events`](Self::process_events) does, and takes the map's
    /// events, oldest first.
    pub fn drain_events(&mut self) -> Vec<MapEvent> {
        self.process_events();
        self.forward_camera_events();
        self.events.take()
    }

    fn emit(&mut self, event: MapEvent) {
        self.forward_camera_events();
        self.events.push(event);
    }

    /// Moves what the transform reported to the map's events, after the
    /// events before it, and to its command ends.
    fn forward_camera_events(&mut self) {
        for event in self.transform.drain_events() {
            match event {
                TransformEvent::CameraWillChange(change) => self.events.push(MapEvent::CameraWillChange(change)),
                TransformEvent::CameraIsChanging => self.events.push(MapEvent::CameraIsChanging),
                TransformEvent::CameraDidChange(change) => self.events.push(MapEvent::CameraDidChange(change)),
                TransformEvent::CommandEnded(end) => self.command_ends.push(end),
            }
        }
    }

    /// Drains the ends of the camera commands that ended since the last
    /// call, oldest first: each command's end comes once, unless the map
    /// drops while the command runs (see [`close_camera`](Self::close_camera)).
    /// They come with the camera events of the same calls, which
    /// [`drain_events`](Self::drain_events) takes. The map keeps its buffer
    /// for the ends that come next.
    pub fn drain_command_ends(&mut self) -> std::vec::Drain<'_, CommandEnd> {
        self.forward_camera_events();
        self.command_ends.drain(..)
    }

    /// Does what the loader's events ask, in their order.
    fn handle_load_events(&mut self, events: Vec<LoadEvent>) {
        for event in events {
            match event {
                LoadEvent::Loading => {
                    self.changed();
                    self.loading = true;
                    self.renderer_fully_loaded = false;
                    self.emit(MapEvent::LoadingStarted);
                }
                LoadEvent::Parsed(style, font_faces) => {
                    self.style = Arc::new(style);
                    self.font_faces = font_faces;
                    self.watch_sources();
                    // A style that loads takes every source's feature state
                    // with it.
                    self.feature_state.clear();
                    self.share_projection_with_transform();
                    self.on_update();
                }
                LoadEvent::Loaded => {
                    self.changed();
                    if !self.camera_mutated {
                        let camera = style_camera(&self.style);
                        self.jump_to(&camera);
                    }
                    self.emit(MapEvent::StyleLoaded);
                }
                LoadEvent::Failed(error) => self.emit(MapEvent::LoadingFailed(error)),
                LoadEvent::ResourceError(error) => self.on_resource_error(error),
                LoadEvent::SpriteRequested(sprite) => self.emit(MapEvent::SpriteRequested(sprite)),
                LoadEvent::SpriteImages(sprite, images) => {
                    // The update this publishes is for *-pattern properties.
                    let _ = self.apply_loaded(Edit::AddImages(images));
                    self.emit(MapEvent::SpriteLoaded(sprite));
                }
                LoadEvent::SpriteFailed(sprite, error) => {
                    // Unblocks the tiles that wait for the sprite.
                    self.on_update();
                    self.emit(MapEvent::SpriteError(sprite, error));
                }
                LoadEvent::SourceDescribed { load, result: Err(error) } => {
                    log::error!(target: log_target::STYLE, "Failed to load source {}: {error}", load.id);
                    self.on_resource_error(error);
                }
                LoadEvent::SourceDescribed { load, result: Ok(description) } => {
                    let previous = self.style.get_source(&load.id).cloned();
                    let edit = Edit::SetSourceDescription {
                        id: load.id.to_string(),
                        instance: load.instance,
                        load: load.load,
                        result: Ok(description),
                    };
                    if !self.apply_loaded_changes(edit) {
                        // The source is loaded now, which the update says.
                        self.on_update();
                    }
                    if let (Some(previous), Some(source)) = (previous, self.style.get_source(&load.id))
                        && source.metadata_changed_from(&previous)
                    {
                        self.emit(MapEvent::SourceChanged { source_id: load.id.to_string() });
                        self.on_update();
                    }
                }
            }
        }
    }

    /// Loads the descriptions of the sources whose loads the loader hasn't
    /// seen, and gives the style the ones their types know without a fetch,
    /// before the version is published.
    fn watch_sources(&mut self) {
        let edits = self.loader.watch_sources(&self.style);
        if edits.is_empty() {
            return;
        }
        let style = Arc::make_mut(&mut self.style);
        for edit in edits {
            // A description that the loader found for the version can't fail.
            let _ = style.apply(edit);
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
                repaint_at,
                placement_changed,
                still_image_request_id,
                update_id,
                stats,
                error,
            } => {
                self.repaint_at = repaint_at.filter(|_| continuous && !needs_repaint);
                match error {
                    Some(error) => self.on_render_failure(*error, still_image_request_id),
                    None => self.render_failing = false,
                }
                self.on_render_frame_finished(
                    mode,
                    needs_repaint,
                    placement_changed,
                    still_image_request_id,
                    update_id,
                    stats,
                );
            }
            // The renderer completes a frame only of an update that carries
            // its newest ground, so the map answers new ground with one,
            // whether or not the camera moves onto it (`follow_terrain`).
            // The renderer tells it again of ground that an update in flight
            // already carries.
            RendererEvent::TerrainElevation(elevation) => {
                let held = match (&self.elevation, &elevation) {
                    (Some(held), Some(told)) => Arc::ptr_eq(held, told),
                    (held, told) => held.is_none() && told.is_none(),
                };
                if !held {
                    self.elevation = elevation;
                    self.on_update();
                }
            }
            RendererEvent::RenderMapStarted => {
                if continuous {
                    self.emit(MapEvent::RenderMapStarted);
                }
            }
            RendererEvent::RenderMapFinished => {
                if continuous && self.loading {
                    self.emit(MapEvent::RenderMapFinished(mln_renderer::RenderMode::Full));
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
                            let _ = self.apply_loaded(Edit::RemoveImage(id));
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
        }
    }

    /// Reports a frame that failed on the GPU, and fails the still image it
    /// was drawn for. A continuous map reports a run of failed frames once,
    /// and draws again after [`RETRY_DELAY`] unless a frame comes sooner.
    fn on_render_failure(&mut self, error: Error, still_image_request_id: u64) {
        if self.mode == MapMode::Continuous {
            if std::mem::replace(&mut self.render_failing, true) {
                return;
            }
            self.repaint_at.get_or_insert(Instant::now() + RETRY_DELAY);
        } else if still_image_request_id != 0 && still_image_request_id == self.still_image_request_id {
            self.on_resource_error(error.clone());
        }
        self.emit(MapEvent::RenderError(error));
    }

    /// Fails the still image being rendered.
    fn on_resource_error(&mut self, error: Error) {
        if self.mode != MapMode::Continuous && self.still_image == StillImage::Pending {
            self.still_image = StillImage::Ended(Err(error));
        }
    }

    /// Handles the end of a frame. Only a `Full` frame completes a still
    /// image; one whose camera hadn't stood on the terrain's newest ground
    /// isn't `Full`. Only a `Full` frame of the map's last update leaves a
    /// continuous map idle: the map may have made an update since the
    /// renderer took the frame's, such as its answer to the new ground or to
    /// a tile that the frame itself took in, and that update's frame is still
    /// to come.
    fn on_render_frame_finished(
        &mut self,
        mode: mln_renderer::RenderMode,
        needs_repaint: bool,
        placement_changed: bool,
        still_image_request_id: u64,
        update_id: u64,
        stats: Arc<RenderingStats>,
    ) {
        let fully_loaded = mode == mln_renderer::RenderMode::Full;
        if std::mem::replace(&mut self.renderer_fully_loaded, fully_loaded) != fully_loaded {
            self.changed();
        }

        if self.mode == MapMode::Continuous {
            self.emit(MapEvent::RenderFrameFinished(RenderFrameStatus {
                mode,
                needs_repaint: needs_repaint || self.repaint_at.is_some(),
                placement_changed,
                rendering_stats: stats,
            }));

            // A host on a frame clock has each frame sample the running
            // animations (`sample_frame`), so a frame's report adds no update
            // for them, which would only repeat the frame's camera.
            let animating = self.transform.in_transition();
            if needs_repaint || (animating && self.transform.frame_time().is_none()) {
                self.on_update();
            } else if !animating
                && self.renderer_fully_loaded
                && self.repaint_at.is_none()
                && update_id == self.update_id
            {
                self.emit(MapEvent::Idle);
            }
        } else if self.renderer_fully_loaded
            && still_image_request_id == self.still_image_request_id
            && self.still_image == StillImage::Pending
        {
            self.still_image = StillImage::Ended(Ok(()));
        }
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
        if self.mode != MapMode::Continuous && self.still_image != StillImage::Pending {
            // The camera stands on the ground all the same.
            self.follow_terrain();
            return;
        }

        // Still modes have no clock: they show every transition as ended.
        let time_point =
            (self.mode == MapMode::Continuous).then(|| self.transform.frame_time().unwrap_or_else(Instant::now));
        // An update once the time the map changes next came shows the map at
        // that time, though the host's last frame came before it.
        let time_point = match (time_point, self.repaint_at.take()) {
            (Some(time), Some(at)) if Instant::now() >= at => Some(time.max(at)),
            (time_point, _) => time_point,
        };

        // The frame's commands keep their eye above the newest ground.
        let ground = self.terrain_ground();
        self.transform.set_terrain(ground);
        self.transform.update_transitions(time_point);
        let projection = self.projection_at(self.transform.view().zoom());
        self.transform.set_projection_definition(&projection);
        self.follow_terrain();

        self.update_id += 1;
        let params = UpdateParameters {
            style_loaded: self.style_loaded(),
            mode: self.mode,
            pixel_ratio: self.pixel_ratio,
            time_point,
            view: self.transform.state().view().clone(),
            min_zoom: self.transform.state().min_zoom(),
            max_zoom: self.transform.state().limits().max_zoom,
            camera_moving: self.transform.is_camera_moving(),
            style: Arc::clone(&self.style),
            font_faces: self.font_faces.clone(),
            sprite_loaded: self.loader.sprites_loaded(),
            feature_states: self.feature_state.shared(),
            file_source: self.file_source.clone(),
            prefetch_zoom_delta: self.tile.prefetch_zoom_delta,
            still_image_request: self.still_image == StillImage::Pending,
            still_image_request_id: self.still_image_request_id,
            update_id: self.update_id,
            cross_source_collisions: self.cross_source_collisions,
            fast_pfor_enabled: self.fast_pfor_enabled,
            terrain: self.terrain,
            elevation: self.elevation.clone(),
            tile_lod_min_radius: self.tile.lod_min_radius,
            tile_lod_scale: self.tile.lod_scale,
            tile_lod_pitch_threshold: mln_math::deg2rad(self.tile.lod_pitch_threshold),
            tile_lod_zoom_shift: self.tile.lod_zoom_shift,
            tile_lod_mode: self.tile.lod_mode,
            subdivision_granularity: if self.mode == MapMode::Tile {
                SubdivisionGranularitySetting::none()
            } else {
                self.style.get_projection().get_subdivision_granularity()
            },
        };

        self.updates.publish(Arc::new(params));
        self.forward_camera_events();
    }

    pub fn jump_to(&mut self, camera: &CameraOptions) {
        self.camera_mutated = true;
        // The style's projection decides how the camera is constrained, so it
        // has to be in place first.
        let zoom = camera.zoom.unwrap_or_else(|| self.transform.view().zoom());
        let projection = self.projection_at(zoom);
        self.transform.set_projection_definition(&projection);
        // So does the terrain, which applies only on Mercator.
        let ground = self.terrain_ground();
        self.transform.set_terrain(ground);
        self.transform.jump_to(camera);
        self.on_update();
    }

    /// The style's projection at the zoom; always Mercator in tile mode,
    /// whose tiles are Mercator tiles.
    fn projection_at(&self, zoom: f64) -> ProjectionDefinition {
        if self.mode == MapMode::Tile {
            return ProjectionDefinition::new(ProjectionType::Mercator);
        }
        self.style.get_projection().evaluate(zoom as f32)
    }

    /// Gives the transform the style's current projection, which constrains
    /// the target of an animation it starts. The style reports each change
    /// to its projection, and the map calls this then.
    fn share_projection_with_transform(&mut self) {
        let projection = (self.mode != MapMode::Tile).then(|| Arc::clone(self.style.get_projection()));
        self.transform.set_projection_for_zoom(Some(Rc::new(move |zoom: f64| match &projection {
            Some(projection) => projection.evaluate(zoom as f32),
            None => ProjectionDefinition::new(ProjectionType::Mercator),
        })));
    }

    // MARK: - Still images

    /// Asks for a still image: the map's renderer draws frames until one
    /// has everything loaded, and [`take_still_image`](Self::take_still_image)
    /// then reports it. Fails with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) for a continuous map
    /// and while another still image is on its way, and with the style's
    /// error when it failed to load.
    pub fn render_still(&mut self) -> Result<(), Error> {
        if self.mode != MapMode::Static && self.mode != MapMode::Tile {
            return Err(Error::invalid_state("Map is not in static or tile image render modes"));
        }
        if self.still_image == StillImage::Pending {
            return Err(Error::invalid_state("Map is currently rendering an image"));
        }
        if let Some(error) = self.loader.last_error() {
            return Err(error.clone());
        }
        self.still_image = StillImage::Pending;
        self.still_image_request_id += 1;
        self.ground_moves = 0;
        self.on_update();
        Ok(())
    }

    /// How the still image that [`render_still`](Self::render_still) asked
    /// for ended, once it has: `Ok` once a complete frame rendered, or why
    /// the image failed. The map handles its renderer's reports in
    /// [`process_events`](Self::process_events), so call that first.
    pub fn take_still_image(&mut self) -> Option<Result<(), Error>> {
        match std::mem::replace(&mut self.still_image, StillImage::None) {
            StillImage::Ended(result) => Some(result),
            other => {
                self.still_image = other;
                None
            }
        }
    }

    /// The still image request that waits for a frame, as the
    /// `still_image_request_id` of the updates that ask for it, or `None`
    /// once it ended.
    #[must_use]
    pub fn pending_still_image(&self) -> Option<u64> {
        (self.still_image == StillImage::Pending).then_some(self.still_image_request_id)
    }

    /// Triggers a repaint.
    pub fn trigger_repaint(&mut self) {
        self.on_update();
    }

    /// When the map changes next on its own, though its last frame asked for
    /// no other: the owner calls [`trigger_repaint`](Self::trigger_repaint)
    /// then, by the frame clock. `None` once an update went out since.
    #[must_use]
    pub fn repaint_at(&self) -> Option<Instant> {
        self.repaint_at
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
        state: mln_style::feature::FeatureState,
    ) -> bool {
        let changed = self.feature_state.set(source_id, source_layer_id.unwrap_or(""), feature_id, state);
        if changed {
            self.on_update();
        }
        changed
    }

    /// The state of the feature `feature_id` of the source `source_id`, in
    /// its source layer `source_layer_id`, empty when it has none.
    #[must_use]
    pub fn feature_state(
        &self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
    ) -> mln_style::feature::FeatureState {
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
        let changed = self.feature_state.remove_source(source_id);
        if changed {
            self.on_update();
        }
        changed
    }

    // MARK: - Style

    /// The map's style.
    #[must_use]
    pub fn style(&self) -> &Arc<Style> {
        &self.style
    }

    /// Edits the style: `edit` changes the next version of it, and the map
    /// publishes that version for the renderer if anything changed. A
    /// version that `edit` left as it was stays the same `Arc`, so it
    /// publishes nothing. Returns what `edit` returns, and keeps what it
    /// changed before it failed.
    ///
    /// Each source that the edit changed, rather than added or removed,
    /// reports [`MapEvent::SourceChanged`].
    pub fn edit_style<R, E>(&mut self, edit: impl FnOnce(&mut Style) -> Result<R, E>) -> Result<R, E> {
        let before = Arc::clone(&self.style);
        let result = edit(Arc::make_mut(&mut self.style));
        let changed_sources = changed_sources(&before, &self.style);
        if self.react_to_change(before) {
            // A revalidated document no longer replaces the edited style.
            self.loader.edited();
        }
        for source_id in changed_sources {
            self.emit(MapEvent::SourceChanged { source_id });
        }
        result
    }

    /// As [`edit_style`](Self::edit_style), but fails with
    /// [`ErrorKind::InvalidState`](mln_util::ErrorKind) ("style is not
    /// loaded") until a style document parsed, and from the start of each
    /// [`load_style_url`](Self::load_style_url) until its document parses:
    /// the parse replaces the whole style, so an edit made before it would
    /// be lost.
    pub fn edit_loaded_style<R>(&mut self, edit: impl FnOnce(&mut Style) -> Result<R, Error>) -> Result<R, Error> {
        if !self.loader.document_loaded() {
            return Err(Error::invalid_state("style is not loaded"));
        }
        self.edit_style(edit)
    }

    /// Whether a style document parsed, and no
    /// [`load_style_url`](Self::load_style_url) began since.
    #[must_use]
    pub fn style_document_loaded(&self) -> bool {
        self.loader.document_loaded()
    }

    /// Makes an edit that the map itself causes, such as a sprite's images,
    /// which doesn't count as an edit of the loaded document.
    pub(crate) fn apply_loaded(&mut self, edit: Edit) -> Result<(), Error> {
        let before = Arc::clone(&self.style);
        let result = Arc::make_mut(&mut self.style).apply(edit);
        self.react_to_change(before);
        result
    }

    /// As [`apply_loaded`](Self::apply_loaded), for an edit that can't fail.
    /// Returns whether it changed the style.
    fn apply_loaded_changes(&mut self, edit: Edit) -> bool {
        let before = Arc::clone(&self.style);
        let _ = Arc::make_mut(&mut self.style).apply(edit);
        self.react_to_change(before)
    }

    /// Reacts to the change from `before` to the current version: keeps
    /// `before` when nothing changed, and otherwise starts the loads of new
    /// sources, shares a new projection with the transform and publishes an
    /// update. Returns whether anything changed.
    fn react_to_change(&mut self, before: Arc<Style>) -> bool {
        if before.is_same_version(&self.style) {
            self.style = before;
            return false;
        }
        if !Arc::ptr_eq(before.get_sources(), self.style.get_sources()) {
            self.watch_sources();
        }
        if !Arc::ptr_eq(before.get_projection(), self.style.get_projection()) {
            self.share_projection_with_transform();
        }
        self.on_update();
        true
    }

    // MARK: - Sources

    /// Adds `source` to the style, last, and starts loading its description.
    /// Fails with [`ErrorKind::AlreadyExists`](mln_util::ErrorKind) if the
    /// style has a source with its id, and with `InvalidArgument` if a layer
    /// that names its id can't draw a source of its kind.
    pub fn add_source(&mut self, source: Source) -> Result<(), Error> {
        self.edit_style(|style| style.apply(Edit::AddSource(source)))
    }

    /// Adds the [source](https://maplibre.org/maplibre-style-spec/sources/)
    /// that `json` describes, with the ID `id`.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) while the
    /// style isn't loaded, with `AlreadyExists` if the style has a source
    /// named `id`, and with `InvalidArgument` if `json` isn't a valid source
    /// or a layer that names `id` can't draw it.
    pub fn add_source_json(&mut self, id: &str, json: &str) -> Result<(), Error> {
        if !self.loader.document_loaded() {
            return Err(Error::invalid_state("style is not loaded"));
        }
        if self.style.get_source(id).is_some() {
            return Err(Error::already_exists(format!("source \"{id}\" already exists")));
        }
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|error| Error::invalid_argument(format!("style source \"{id}\" is not valid JSON: {error}")))?;
        let mut error = mln_style::conversion::Error::default();
        let registry = Arc::clone(self.style.registry());
        let Some(source) = convert_source(&Convertible::from(&value), &mut error, id, &registry) else {
            return Err(Error::invalid_argument(format!("style source \"{id}\": {}", error.message)));
        };
        self.add_source(source)
    }

    /// Removes the source `id` from the style and gives it back, unless a
    /// layer uses it, which logs a warning.
    pub fn remove_source(&mut self, id: &str) -> Option<Source> {
        if self.style.get_layers().any(|layer| layer.get_source_id() == id) {
            log::warn!(target: log_target::GENERAL, "Source '{id}' is in use, cannot remove");
            return None;
        }
        let source = self.style.get_source(id)?.clone();
        self.edit_style(|style| style.apply(Edit::RemoveSource(id.to_owned()))).ok()?;
        Some(source)
    }

    /// The style's source `id`.
    #[must_use]
    pub fn source(&self, id: &str) -> Option<&Source> {
        self.style.get_source(id)
    }

    /// The style's sources, in the order they were added.
    pub fn sources(&self) -> impl Iterator<Item = &Source> {
        self.style.get_sources().iter()
    }

    /// Loads the style document `json`, which replaces the style, and
    /// cancels a URL load. A document that doesn't parse leaves the style
    /// as it was and reports [`MapEvent::LoadingFailed`].
    pub fn load_style_json(&mut self, json: &str) {
        let events = self.loader.load_json(self.style.registry(), json);
        self.handle_load_events(events);
    }

    /// Fetches the style document at `url`, which replaces the style when it
    /// arrives and parses. The style stays meanwhile, but isn't loaded.
    pub fn load_style_url(&mut self, url: &str) {
        let events = self.loader.load_url(url);
        self.handle_load_events(events);
    }

    /// The URL of the style document, or `None` for a style loaded from
    /// JSON or none at all.
    #[must_use]
    pub fn style_url(&self) -> Option<&str> {
        self.loader.url()
    }

    /// Sets the font faces that every font stack falls back to, for the
    /// style documents that load after this call. They shape the text that
    /// glyph PBFs can't shape, or all the text of a style without glyph
    /// PBFs, and draw the glyphs that come from local fonts (`LocalGlyphs`)
    /// before the device's fonts do.
    ///
    /// `json` is an array of font files, each an object with a `url` and an
    /// optional `unicode-range`, as in a font face of a style's `font-faces`.
    /// It fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) if
    /// `json` isn't such an array. Without a font-face shaper, the map ignores
    /// them, as it ignores a style's `font-faces`.
    pub fn set_fallback_font_faces(&mut self, json: &str) -> Result<(), Error> {
        let faces = mln_style::parse_fallback_font_faces(json)?;
        self.set_fallback_font_face_list(faces.iter().map(FontFace::from).collect());
        Ok(())
    }

    /// [`set_fallback_font_faces`](Self::set_fallback_font_faces) with faces
    /// an app has made, such as `FontFace::fallback(url, ranges).with_data(bytes)`
    /// for a font file it holds in memory, or `FontFace::system`, for the
    /// device's fonts that the renderer's `LocalGlyphs::chooser` chooses.
    pub fn set_fallback_font_face_list(&mut self, faces: FontFaces) {
        self.loader.set_fallback_font_faces(Some(Arc::new(faces)));
    }

    /// The last error of a sprite or a source of the style since its load
    /// began, which fails still images.
    #[must_use]
    pub fn last_style_error(&self) -> Option<&Error> {
        self.loader.last_error()
    }

    #[cfg(test)]
    pub(crate) fn style_loader(&self) -> &StyleLoader {
        &self.loader
    }

    /// Whether the style document parsed, and its sprites and sources
    /// loaded.
    pub(crate) fn style_loaded(&self) -> bool {
        self.loader.document_loaded() && self.loader.sprites_loaded() && self.loader.sources_loaded(&self.style)
    }

    // MARK: - Transitions

    pub fn cancel_transitions(&mut self) {
        self.transform.cancel_transitions();
        self.on_update();
    }

    /// Cancels the commands whose [`AnimationOptions::transition_id`] equals
    /// `transition_id`, and leaves the others running. Requests an update only
    /// when a command ended.
    pub fn cancel_transitions_with_id(&mut self, transition_id: u64) {
        let ended = self.transform.cancel_transitions_with_id(transition_id);
        if ended {
            self.on_update();
        }
    }

    /// Moves the camera relative to where it is, as a gesture does
    /// (the C API's `mln_map_apply_camera_delta`): a pan, a zoom,
    /// turn and tilt around an anchor, a gesture phase, and the momentum a
    /// gesture's end continues with. See [`CameraDelta`]. Returns the
    /// command, whose end [`drain_command_ends`](Self::drain_command_ends)
    /// reports.
    pub fn apply_camera_delta(&mut self, delta: &CameraDelta) -> Result<CameraCommandId, CameraError> {
        let command = self.transform.apply_camera_delta(delta)?;
        self.camera_mutated = true;
        self.on_update();
        Ok(command)
    }

    /// Moves the camera to absolute values, by a jump, an ease or a flight,
    /// as a command or as part of a gesture (the C API's
    /// `mln_map_update_camera`). See [`CameraUpdate`]. Returns the command,
    /// whose end [`drain_command_ends`](Self::drain_command_ends) reports.
    pub fn update_camera(&mut self, update: &CameraUpdate) -> CameraCommandId {
        self.camera_mutated = true;
        let command = self.transform.update_camera(update);
        self.on_update();
        command
    }

    /// Ends the running camera commands as closed, as when the map closes,
    /// which [`drain_command_ends`](Self::drain_command_ends) reports. An
    /// owner that waits for the commands' ends calls it before it drops the
    /// map.
    pub fn close_camera(&mut self) {
        self.transform.close();
        self.forward_camera_events();
    }

    /// Sets whether drags and zooms on the globe turn the bearing
    /// ([`Viewport::globe_free_bearing`]), as
    /// [`set_viewport`](Self::set_viewport) does.
    pub fn set_globe_free_bearing(&mut self, free_bearing: bool) {
        if free_bearing != self.transform.is_globe_free_bearing() {
            self.transform.set_globe_free_bearing(free_bearing);
            self.changed();
        }
    }

    /// Samples the camera's animations at the host's frame time, from now on,
    /// and starts new ones from it: call it at the start of each frame with
    /// `Instant::now()`, before rendering. One older than
    /// [`STALE_FRAME_TIME`](crate::transform::STALE_FRAME_TIME) counts as
    /// unset, so commands made while the host isn't drawing start now. If an
    /// animation runs, the map updates for this frame at once. `None` goes
    /// back to sampling at `Instant::now()` whenever the map updates. Still
    /// images ignore it. See
    /// [`Transform::set_frame_time`](crate::transform::Transform::set_frame_time).
    pub fn set_frame_time(&mut self, time: Option<Instant>) {
        self.transform.set_frame_time(time);
        self.sample_frame();
    }

    /// The clock that [`set_frame_time`](Self::set_frame_time) sets, for a
    /// host that sets it from the thread that draws, without updating the
    /// map. It then calls [`sample_frame`](Self::sample_frame) for each frame
    /// that an animation runs in.
    #[must_use]
    pub fn frame_clock(&self) -> FrameClock {
        self.transform.frame_clock()
    }

    /// Updates the map for a frame at the frame clock's time, if an
    /// animation runs.
    pub fn sample_frame(&mut self) {
        if self.transform.in_transition() {
            self.on_update();
        }
    }

    // MARK: - Camera

    // query: the render-test runner's tile mode uses it.
    /// The bounds of the area the map shows from `camera`.
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a camera whose
    /// corners have no `LatLng`.
    ///
    /// Over terrain, the area is where the view meets the plane through the
    /// camera's center, as [`camera_for_lat_lngs`](Self::camera_for_lat_lngs)
    /// fits it.
    pub fn lat_lng_bounds_for_camera(&self, camera: &CameraOptions) -> Result<LatLngBounds, Error> {
        let shallow = self.fitting_transform(camera);
        let size = shallow.state().view().size();

        if shallow.state().view().is_globe() {
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
    ///
    /// On the flat map the points stand at sea level, and the center keeps
    /// its altitude. Over terrain, where the center will stand isn't known
    /// until the ground there draws, so the points fit as if they stood as
    /// high as the center: the camera shows them inside the padding at that
    /// height, whatever altitude it comes to. Points on higher ground can
    /// show past the padding, and those on lower ground inside it.
    pub fn camera_for_lat_lngs(
        &self,
        lat_lngs: &[LatLng],
        padding: &EdgeInsets,
        bearing: Option<f64>,
        pitch: Option<f64>,
    ) -> Result<CameraOptions, Error> {
        let transform = self.fitting_transform(&CameraOptions { bearing, pitch, ..Default::default() });
        if bearing.is_none() && pitch.is_none() {
            return camera_for_lat_lngs(lat_lngs, &transform, padding);
        }

        Ok(CameraOptions {
            bearing: Some(mln_math::rad2deg(-transform.view().angle())),
            pitch: Some(mln_math::rad2deg(transform.view().pitch())),
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
        let shallow = self.fitting_transform(camera);
        let size = shallow.state().view().size();

        if shallow.state().view().is_globe() {
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

    /// A copy of the map's transform moved by `camera`, without the map's
    /// terrain, to fit cameras and find what they show. Over terrain its
    /// center stands at sea level, whatever its altitude: a view is the same
    /// raised by any height but for where the ground meets it, so sea-level
    /// conversions then meet the plane through the center.
    fn fitting_transform(&self, camera: &CameraOptions) -> Transform {
        let mut transform = Transform::from_state(self.transform.state().clone());
        let center_altitude = if self.terrain_applies() { Some(0.0) } else { camera.center_altitude };
        let camera = CameraOptions { center_altitude, ..*camera };
        if camera != CameraOptions::default() {
            transform.jump_to(&camera);
        }
        transform
    }

    // MARK: - Bounds

    pub fn set_bounds(&mut self, options: &BoundOptions) -> Result<(), Error> {
        let current = *self.transform.state().limits();
        let limits = CameraLimits {
            bounds: options.bounds.unwrap_or(current.bounds),
            min_zoom: options.min_zoom.unwrap_or(current.min_zoom),
            max_zoom: options.max_zoom.unwrap_or(current.max_zoom),
            min_pitch: options.min_pitch.map_or(current.min_pitch, mln_math::deg2rad),
            max_pitch: options.max_pitch.map_or(current.max_pitch, mln_math::deg2rad),
        };
        self.transform.set_limits(limits)?;
        if *options != BoundOptions::default() {
            self.changed();
        }

        // The camera moves inside the limits that changed.
        let state = self.transform.state();
        let view = state.view();
        let limits = *state.limits();
        let mut camera_options = CameraOptions::default();
        let mut change_camera = options.bounds.is_some();
        if options.min_zoom.is_some() {
            // On the globe the lowest zoom falls with latitude; on Mercator it
            // is the minimum.
            let latitude_term =
                state.min_zoom_at_latitude(view.center(WrapMode::Unwrapped).latitude()) - state.min_zoom();
            let min_zoom = limits.min_zoom + latitude_term;
            if view.zoom() < min_zoom {
                change_camera = true;
                camera_options.zoom = Some(min_zoom);
            }
        }
        if options.max_zoom.is_some() && view.zoom() > limits.max_zoom {
            change_camera = true;
            camera_options.zoom = Some(limits.max_zoom);
        }
        if options.max_pitch.is_some() && view.pitch() > limits.max_pitch {
            change_camera = true;
            camera_options.pitch = Some(mln_math::rad2deg(limits.max_pitch));
        }
        if options.min_pitch.is_some() && view.pitch() < limits.min_pitch {
            change_camera = true;
            camera_options.pitch = Some(mln_math::rad2deg(limits.min_pitch));
        }

        if change_camera {
            self.jump_to(&camera_options);
        }
        Ok(())
    }

    // MARK: - Map options

    /// Sets how the map orients and constrains its view, every setting at
    /// once.
    pub fn set_viewport(&mut self, viewport: &Viewport) {
        let state = self.transform.state();
        let (orientation, constrain_mode, viewport_mode, frustum_offset) = (
            state.view().north_orientation(),
            state.constrain_mode(),
            state.view().viewport_mode(),
            state.view().frustum_offset(),
        );
        if viewport.north_orientation != orientation {
            self.transform.set_north_orientation(viewport.north_orientation);
        }
        if viewport.constrain_mode != constrain_mode {
            self.transform.set_constrain_mode(viewport.constrain_mode);
        }
        if viewport.viewport_mode != viewport_mode {
            self.transform.set_viewport_mode(viewport.viewport_mode);
        }
        if viewport.frustum_offset != frustum_offset {
            self.transform.set_frustum_offset(&viewport.frustum_offset);
        }
        if viewport.center_clamped_to_ground != self.center_clamped_to_ground() {
            self.transform.set_center_clamped_to_ground(viewport.center_clamped_to_ground);
        }
        if viewport.globe_free_bearing != self.transform.is_globe_free_bearing() {
            self.transform.set_globe_free_bearing(viewport.globe_free_bearing);
        }
        self.on_update();
    }

    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for an empty size.
    pub fn set_size(&mut self, size: Size) -> Result<(), Error> {
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
        if pixel_ratio == self.pixel_ratio {
            return Ok(());
        }
        self.pixel_ratio = pixel_ratio;
        let events = self.loader.set_pixel_ratio(pixel_ratio);
        self.handle_load_events(events);
        self.on_update();
        Ok(())
    }

    // MARK: - Projection mode

    pub fn set_projection_mode(&mut self, options: &ProjectionMode) {
        self.transform.set_projection_mode(options);
        self.on_update();
    }

    // MARK: - Transform

    /// The camera, the viewport and the limits the camera moves within.
    #[must_use]
    pub fn transform_state(&self) -> &TransformState {
        self.transform.state()
    }

    // MARK: - Tile settings

    /// Sets how the map picks the tiles to load, every setting at once, in
    /// the units that its snapshot reports them. Publishes an update when
    /// they change.
    pub fn set_tile_settings(&mut self, tile: &TileSettings) {
        if self.tile != *tile {
            self.tile = *tile;
            self.on_update();
        }
    }

    // gtests-core: projection and prefetch zoom delta (test/map, test/api).
    // MARK: - Projection

    /// The screen point of `lat_lng`, in the copy of the world nearest the
    /// camera's center, standing on the ground the map draws: over terrain,
    /// at the height the renderer drew the ground at there ([`Self::ground`]),
    /// or at the center's altitude where it drew none; on the flat map, at
    /// sea level. A location that nearer ground hides still has the point
    /// where its ground draws.
    #[must_use]
    pub fn pixel_for_lat_lng(&self, lat_lng: &LatLng) -> ScreenCoordinate {
        crate::map_projection::pixel_for_lat_lng(self.transform.view(), self.ground().as_deref(), lat_lng)
    }

    /// The location that `pixel` shows on the ground the map draws: over
    /// terrain, where its line of sight first meets the ground the renderer
    /// drew ([`Self::ground`]), or, where it meets none, as in the sky, the
    /// plane at the center's altitude; on the flat map, sea level.
    /// [`pixel_for_lat_lng`](Self::pixel_for_lat_lng) takes the location
    /// back to the pixel, but above the horizon of the plane, which the line
    /// of sight doesn't meet. Fails with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a NaN or
    /// infinite pixel.
    pub fn lat_lng_for_pixel(&self, pixel: &ScreenCoordinate, wrap_mode: WrapMode) -> Result<LatLng, Error> {
        crate::map_projection::lat_lng_for_pixel(self.transform.view(), self.ground().as_deref(), pixel, wrap_mode)
    }

    /// Whether the globe hides a location from the camera; a Mercator map
    /// hides nothing.
    #[cfg(test)]
    pub(crate) fn is_location_occluded(&self, lat_lng: &LatLng) -> bool {
        self.transform.is_location_occluded(lat_lng)
    }

    /// [`lat_lng_for_pixel`](Self::lat_lng_for_pixel) of each pixel. Fails
    /// as that does for any of them.
    pub fn lat_lngs_for_pixels(
        &self,
        screen_coords: &[ScreenCoordinate],
        wrap_mode: WrapMode,
    ) -> Result<Vec<LatLng>, Error> {
        let ground = self.ground();
        let view = self.transform.view();
        screen_coords
            .iter()
            .map(|pixel| crate::map_projection::lat_lng_for_pixel(view, ground.as_deref(), pixel, wrap_mode))
            .collect()
    }

    /// Places the camera by its position and orientation, and cancels the
    /// running camera commands. A NaN position, or a NaN or zero-length
    /// orientation, is ignored; the position's height is clamped to the zoom
    /// range.
    pub fn set_free_camera_options(&mut self, camera: &mln_map_types::FreeCameraOptions) {
        self.transform.set_free_camera_options(camera);
        self.camera_mutated = true;
        self.on_update();
    }

    // MARK: - Terrain

    /// Sets whether the center stands on the terrain, at its height under
    /// the center ([`Viewport::center_clamped_to_ground`]), as
    /// [`set_viewport`](Self::set_viewport) does. Turning it on stands a
    /// camera at rest on the ground at once, without camera events; turning
    /// it off leaves the altitude where it is.
    pub fn set_center_clamped_to_ground(&mut self, clamped: bool) {
        if self.center_clamped_to_ground() != clamped {
            self.transform.set_center_clamped_to_ground(clamped);
            self.on_update();
        }
    }

    /// Whether the center stands on the terrain.
    #[must_use]
    pub fn center_clamped_to_ground(&self) -> bool {
        self.transform.view().center_clamped_to_ground()
    }

    /// Hands the transform the terrain the camera stands on
    /// ([`Self::terrain_ground`]), and moves the camera onto it
    /// ([`Transform::follow_terrain`]).
    ///
    /// The moves report no camera change. A still image stops following the
    /// terrain after [`MAX_GROUND_MOVES_PER_STILL`] of them, though it still
    /// goes back to the flat map, and its updates still carry the newest
    /// ground, so its frame draws.
    fn follow_terrain(&mut self) {
        let ground = self.terrain_ground();
        let grounded = ground.as_ref().is_some_and(|ground| ground.elevation.is_some());
        self.transform.set_terrain(ground);
        let still = self.mode != MapMode::Continuous && self.still_image == StillImage::Pending;
        if still && grounded && self.ground_moves >= MAX_GROUND_MOVES_PER_STILL {
            return;
        }
        if self.transform.follow_terrain() && still && grounded {
            self.ground_moves += 1;
            if self.ground_moves == MAX_GROUND_MOVES_PER_STILL {
                log::warn!(
                    target: log_target::RENDER,
                    "The camera didn't settle on the terrain: the still image shows it where it last moved"
                );
            }
        }
    }

    /// The terrain the camera stands on: the ground the renderer last drew,
    /// while the style has terrain outside tile mode, on every projection;
    /// else none, which turns the camera's rules over terrain off.
    fn terrain_ground(&mut self) -> Option<TerrainGround> {
        // The renderer tells of the ground only after it draws an update
        // without it, so the map, which owns the style, lets go first.
        if self.style.get_terrain().is_none() {
            self.elevation = None;
        }
        self.terrain_applies()
            .then(|| TerrainGround { elevation: self.elevation.clone(), clamped: self.center_clamped_to_ground() })
    }

    /// Whether the style's terrain applies: it has terrain, outside tile
    /// mode, on every projection.
    fn terrain_applies(&self) -> bool {
        self.style.get_terrain().is_some() && self.mode != MapMode::Tile
    }

    /// The ground of the style's terrain as the renderer last drew it, which
    /// the camera stands on and the map's conversions meet
    /// ([`pixel_for_lat_lng`](Self::pixel_for_lat_lng)). `None` while the
    /// map draws flat: without terrain, in tile mode, and until the renderer
    /// draws the terrain.
    #[must_use]
    pub fn ground(&self) -> Option<Arc<dyn Elevation>> {
        self.elevation.clone().filter(|_| self.terrain_applies())
    }

    /// Grows with each change to the map's state: each call that changes it,
    /// and each change that loading, rendering and animations make.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Counts the updates that the map has published for its renderer.
    /// An update that a command group holds back counts when it shows.
    #[must_use]
    pub fn update_generation(&self) -> u64 {
        self.updates.generation()
    }

    /// The generation of the update that shows the map's changes so far:
    /// [`update_generation`](Self::update_generation), or while a command
    /// group holds an update back, the one that the outermost group's end
    /// shows.
    #[must_use]
    pub fn pending_update_generation(&self) -> u64 {
        self.updates.pending_generation()
    }

    /// Copies the map's state.
    #[must_use]
    pub fn snapshot(&self) -> MapSnapshot {
        let transform = &self.transform;
        let state = transform.state();
        let bounds = state.limits().bounds;
        let free_camera = state.view().free_camera_options();
        MapSnapshot {
            generation: self.generation,
            update_generation: self.updates.generation(),
            fully_loaded: self.is_fully_loaded(),
            gesture_in_progress: transform.is_gesture_in_progress(),
            transition_in_progress: transform.in_transition(),
            tile: self.tile,
            bounds: CameraBounds {
                bounds: (bounds != LatLngBounds::infinite()).then_some(bounds),
                min_zoom: state.min_zoom(),
                max_zoom: state.limits().max_zoom,
                min_pitch: mln_math::rad2deg(state.limits().min_pitch),
                max_pitch: mln_math::rad2deg(state.limits().max_pitch),
            },
            free_camera: FreeCamera {
                position: free_camera.position.unwrap_or_default(),
                orientation: free_camera.orientation.unwrap_or_default(),
            },
            view: state.view().map_view(f64::from(self.pixel_ratio)),
            globe_rendering: state.view().is_globe(),
        }
    }

    fn is_fully_loaded(&self) -> bool {
        self.style_loaded() && self.renderer_fully_loaded
    }

    /// Logs the style's sources and their state.
    pub fn dump_debug_logs(&self) {
        let rule = "-".repeat(80);
        log::info!(target: log_target::GENERAL, "{rule}");
        log::info!(target: log_target::GENERAL, "styleURL: {}", self.loader.url().unwrap_or_default());
        for source in self.style.get_sources().iter() {
            log::info!(target: log_target::GENERAL, "Source::id: {}", source.id());
            log::info!(
                target: log_target::GENERAL,
                "Source::loaded: {}",
                i32::from(self.loader.source_loaded(source.id()))
            );
        }
        log::info!(target: log_target::GENERAL, "{rule}");
    }
}

/// The ids of the sources that are in both `before` and `after`, as the same
/// instance, and changed from one to the other, in `after`'s order.
fn changed_sources(before: &Style, after: &Style) -> Vec<String> {
    let (old, new) = (before.get_sources(), after.get_sources());
    if Arc::ptr_eq(old, new) {
        return Vec::new();
    }
    new.iter()
        .filter(|source| {
            old.get(source.id()).is_some_and(|old| old.instance() == source.instance() && !Source::ptr_eq(old, source))
        })
        .map(|source| source.id().to_owned())
        .collect()
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
    if transform.state().view().is_globe() {
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
    let size = transform.state().view().size();
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

    let mut zoom = transform.view().zoom();
    if min_scale > 0.0 {
        zoom = mln_math::clamp_f64(
            zoom + min_scale.log2(),
            transform.state().min_zoom(),
            transform.state().limits().max_zoom,
        );
    } else {
        log::error!(
            target: log_target::GENERAL,
            "Unable to calculate appropriate zoom level for bounds. Vertical or horizontal padding is greater than \
             map's height or width."
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
    vector: DVec3,
    to_center: DVec3,
    projection: &DMat4,
    x_axis: bool,
    target: f64,
) -> Option<f64> {
    // The rows of the clip axis and of w.
    let column = projection.row(usize::from(!x_axis));
    let column_z = projection.row(3);

    let vector_dot = vector.dot(column.truncate());
    let vector_dot_z = vector.dot(column_z.truncate());
    let to_center_dot = to_center.dot(column.truncate());
    let to_center_dot_z = to_center.dot(column_z.truncate());

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
    let mut flat = Transform::from_state(transform.state().clone());
    flat.set_projection_definition(&ProjectionDefinition::new(ProjectionType::Mercator));
    flat.jump_to(&CameraOptions { pitch: Some(0.0), roll: Some(0.0), ..Default::default() });
    let fit = camera_for_lat_lngs_on_screen(lat_lngs, &flat, padding)?;
    let (Some(fit_center), Some(fit_zoom)) = (fit.center, fit.zoom) else {
        return Ok(fit);
    };

    // Solve with the padding the returned camera is shown with.
    let mut globe = Transform::from_state(transform.state().clone());
    globe.jump_to(&CameraOptions {
        center: Some(fit_center),
        zoom: Some(fit_zoom),
        padding: Some(*padding),
        pitch: Some(0.0),
        roll: Some(0.0),
        ..Default::default()
    });
    let state = globe.state();
    let matrix = state.view().globe_view_projection_matrix();
    let size = state.view().size();
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
            keep_smallest_scale(&mut smallest, globe_scale_for_clip_target(vector, to_center, &matrix, true, left));
        }
        if right > 0.0 {
            keep_smallest_scale(&mut smallest, globe_scale_for_clip_target(vector, to_center, &matrix, true, right));
        }
        if top > 0.0 {
            keep_smallest_scale(&mut smallest, globe_scale_for_clip_target(vector, to_center, &matrix, false, top));
        }
        if bottom < 0.0 {
            keep_smallest_scale(&mut smallest, globe_scale_for_clip_target(vector, to_center, &matrix, false, bottom));
        }
    }
    if !smallest.is_finite() || smallest == 0.0 {
        log::warn!(target: log_target::GENERAL, "Unable to fit the bounds on the globe; keeping the flat fit.");
        return Ok(fit);
    }
    Ok(CameraOptions {
        zoom: Some(f64::min(globe.view().zoom() + mln_map_types::scale_zoom(smallest), state.limits().max_zoom)),
        ..fit
    })
}

/// GL JS's globe bounds: the farthest of eight edge points on each side of
/// the center, where points off the planet fall on its horizon, widened to
/// every longitude when a pole is on screen.
/// Fails as `Map::lat_lng_bounds_for_camera` does.
fn globe_bounds(transform: &Transform) -> Result<LatLngBounds, Error> {
    let width = f64::from(transform.state().view().size().width);
    let height = f64::from(transform.state().view().size().height);
    let center = transform.view().center(WrapMode::Wrapped);
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
        let (_, clip) =
            VerticalPerspectiveProjection::lat_lng_to_screen_coordinate(transform.state().view(), pole, 0.0);
        if clip[3] <= 0.0 || (clip[2] / clip[3]).abs() >= 1.0 {
            return false;
        }
        let point = transform.lat_lng_to_screen_coordinate(pole);
        !transform.state().view().is_location_occluded(pole)
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
