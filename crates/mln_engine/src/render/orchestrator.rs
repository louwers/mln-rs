// The orchestrator owns the render sources and the tile store their tiles
// are in, and turns what the store reports into the renderer's events.

use std::sync::Arc;
use std::time::Duration;

use mln_gfx::gpu::Gpu;
use mln_style::constants::DEFAULT_TRANSITION_DURATION;
use mln_style::diff::{ItemChanges, LayerChange, LayerChanges, StyleDiff, sources_by_id};
use mln_style::error::Error;
use mln_style::expression::{Dependency, GlobalStateMap};
use mln_style::hash::{HashMap, HashSet};
use mln_style::image::Impl as ImageImpl;
use mln_style::layer::{ErasedLayerStyle, SourceKind};
use mln_style::property::evaluation::ZoomHistory;
use mln_style::time::Instant;
use mln_style::{
    GlobalStateChange, Layer, LayerList, PropertyEvaluationParameters, Source, Style, TransitionOptions,
    TransitionParameters, layer,
};
use mln_vec_map::VecSet;
use smallvec::SmallVec;

use crate::camera::View;
use crate::draw::context::Context;
use crate::exec::pool::{Pool, TaskGroup};
use crate::image::image_store::ImageStore;
use crate::layer::layer_drawing::{ChangedBuckets, DrawGroup, DrawOrder, SingleDraw};
use crate::layer::layer_type::ErasedEvaluatedLayer;
use crate::layer::render_layer::{FrameSourceTiles, LayerUpdateParameters, OnTerrain, RenderLayer};
use crate::layers::custom::EndedHosts;
use crate::map::MapMode;
use crate::placement::Placements;
use crate::render::events::RendererEvent;
use crate::render::frame::{Frame, FrameLayer};
use crate::render::globe::depth_pass::GlobeDepthPass;
use crate::render::paint_parameters::PaintParameters;
use crate::render::pass::RenderPass;
use crate::render::terrain::{DrapeStacks, Draped, RenderTerrain};
use crate::render::tile::{FoundTile, TileContent};
use crate::render::update_parameters::UpdateParameters;
use crate::source::render_source::{self, RenderSource, RenderTileParameters, TileOrder};
use crate::text::GlyphCache;
use crate::text::glyph_store::GlyphStore;
use crate::text::local_fonts::LocalGlyphs;
use crate::tile_store::inbox::Waker;
use crate::tile_store::source_tiles::SourceTiles;
use crate::tile_store::store::{StoreEvent, TileStore};
use crate::tile_store::tile_parameters::TileParameters;
use crate::worker::tile_workers::PoolWorkers;
use crate::worker::worker_codec::RemoteWorkers;
use crate::{Registry, log_target};

/// The keys whose values differ between two global state snapshots. Values
/// of unchanged keys are copied verbatim between snapshots, so exact
/// equality is sufficient here.
fn diff_global_state_keys(previous: &GlobalStateMap, current: &GlobalStateMap) -> VecSet<String> {
    let mut keys = VecSet::new();
    for (key, value) in current {
        match previous.get(key) {
            Some(prev) if prev == value => {}
            _ => {
                keys.insert(key.clone());
            }
        }
    }
    for key in previous.keys() {
        if !current.contains_key(key) {
            keys.insert(key.clone());
        }
    }
    keys
}

/// The jobs in flight per thread of the pool.
const JOBS_PER_THREAD: usize = 2;

/// The jobs in flight on a pool without threads, whose host runs them
/// between its own work.
const JOBS_WITHOUT_THREADS: usize = 8;

fn ptr_eq_option<T: ?Sized>(a: Option<&Arc<T>>, b: Option<&Arc<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

/// The render layer of the layer `style`, from its type in `registry`. A
/// type that the registry lacks breaks the contract that a map and its
/// renderer register alike; the layer isn't drawn.
fn create_render_layer(registry: &Registry, style: &Arc<dyn ErasedLayerStyle>) -> Option<Box<dyn RenderLayer>> {
    let name = style.info().name;
    let render_layer = registry.layer_type(name).and_then(|layer_type| layer_type.create_render_layer(style));
    debug_assert!(render_layer.is_some(), "the renderer's registry has no layer type '{name}'");
    if render_layer.is_none() {
        log::error!(target: log_target::RENDER, "no render layer for layer type '{name}'");
    }
    render_layer
}

/// How an update evaluates and transitions the layers, the light and the
/// sky.
struct FrameClock {
    /// Whether the map is continuous; a still image doesn't transition.
    continuous: bool,
    zoom_changed: bool,
    global_state_changed: bool,
    /// The global state keys whose values changed, if the update knows them.
    changed_global_state_keys: Option<Arc<VecSet<String>>>,
    /// How a layer added in this update transitions: from nothing.
    added_layer_transition: TransitionParameters,
    /// How a changed layer, light or sky transitions.
    transition: TransitionParameters,
    /// How the values that read a changed global state key transition,
    /// where nothing else changed.
    global_state_transition: Option<TransitionParameters>,
    evaluation: PropertyEvaluationParameters,
}

pub(crate) struct RenderOrchestrator {
    registry: Arc<Registry>,
    /// What the orchestrator and its stores reported since the renderer
    /// last took them, oldest first.
    events: Vec<RendererEvent>,

    zoom_history: ZoomHistory,
    /// The still image request the last update was for.
    still_image_request_id: u64,
    view: View,

    /// The sources' tiles, with the glyph and image stores their layouts
    /// resolve through and the atlas they pack them into.
    tile_store: TileStore,
    /// The tiles the frame's layers draw, as their updates found them, each
    /// layer's in a range of its own.
    found_tiles: Vec<FoundTile>,

    /// The version of the style the last update drew, or `None` before
    /// the first and after `clear_data`: no layers, sources or images.
    style: Option<Arc<Style>>,
    /// The style's sources in id order, the order of the per-source pass.
    sources_by_id: Vec<Source>,

    render_sources: HashMap<String, Box<dyn RenderSource>>,
    /// The render layers, in the style's order: a layer's index is its
    /// place in the drawing order. A layer whose type has no render layer
    /// has none.
    layers: Vec<Box<dyn RenderLayer>>,
    /// The index in `layers` of each render layer, by layer id.
    layer_indexes: HashMap<String, usize>,
    render_light: crate::render::light::RenderLight,
    render_sky: Option<crate::render::sky::RenderSky>,

    /// The placements of the layers that place, such as symbols.
    placements: Placements,
    background_layer_as_color: bool,
    /// The center's longitude at the last update, which tells a wrap jump.
    longitude: f32,
    tile_cache_enabled: bool,

    /// Whether the next update makes the render layers again, for new
    /// shaders.
    remake_render_layers: bool,
    /// The hosts of the custom layers that left since the renderer last
    /// [took them](Self::take_ended_hosts).
    ended_hosts: EndedHosts,
    filtered_layers_for_source: Vec<Arc<dyn ErasedEvaluatedLayer>>,

    thread_pool: TaskGroup,

    global_state: Option<Arc<GlobalStateMap>>,

    globe_depth_pass: GlobeDepthPass,
    sky_pass: crate::render::sky_pass::SkyPass,
    terrain: RenderTerrain,
    /// The frame's stacks of draped layers, whose surfaces the translucent
    /// pass draws among the layers; none without terrain.
    stacks: DrapeStacks,

    /// Last, so that it drops after the fields above: the render sources'
    /// tiles free their data on the thread pool (`drop_on_workers`).
    _wait_for_thread_pool: WaitOnDrop,
}

impl RenderOrchestrator {
    /// An orchestrator whose background work runs in a group of its own on
    /// `pool`, which its drop waits for, and whose tiles and glyphs call
    /// `waker` for an update (see `Renderer::new`).
    pub(crate) fn new(
        background_layer_as_color: bool,
        local_glyphs: LocalGlyphs,
        glyph_cache: Arc<GlyphCache>,
        font_face_engine: Arc<dyn crate::text::font_face_engine::FontFaceEngine>,
        registry: Arc<Registry>,
        waker: Waker,
        pool: &Pool,
    ) -> Self {
        let thread_pool = TaskGroup::new(pool);
        let glyph_store =
            GlyphStore::new(local_glyphs, glyph_cache, thread_pool.clone(), font_face_engine, Arc::clone(&waker));
        let jobs = match pool.threads() {
            0 => JOBS_WITHOUT_THREADS,
            threads => JOBS_PER_THREAD * threads,
        };
        let pool_workers = PoolWorkers::new(thread_pool.clone(), jobs);
        let tile_store =
            TileStore::new(Box::new(pool_workers), glyph_store, ImageStore::default(), thread_pool.clone(), waker);
        let placements = Placements::new(&registry);
        RenderOrchestrator {
            registry,
            events: Vec::new(),
            zoom_history: ZoomHistory::default(),
            still_image_request_id: 0,
            view: View::default(),
            tile_store,
            placements,
            found_tiles: Vec::new(),
            style: None,
            sources_by_id: Vec::new(),
            render_sources: HashMap::default(),
            layers: Vec::new(),
            layer_indexes: HashMap::default(),
            render_light: crate::render::light::RenderLight::new(Arc::default()),
            render_sky: None,
            background_layer_as_color,
            longitude: 0.0,
            tile_cache_enabled: true,
            remake_render_layers: false,
            ended_hosts: Vec::new(),
            filtered_layers_for_source: Vec::new(),
            _wait_for_thread_pool: WaitOnDrop(thread_pool.clone()),
            thread_pool,
            global_state: None,
            globe_depth_pass: GlobeDepthPass::default(),
            sky_pass: crate::render::sky_pass::SkyPass::default(),
            terrain: RenderTerrain::default(),
            stacks: DrapeStacks::default(),
        }
    }

    /// Runs the tiles' jobs in `workers` (by default on the thread
    /// pool). The jobs in flight elsewhere run again there.
    pub(crate) fn set_tile_workers(&mut self, workers: RemoteWorkers) {
        self.tile_store.set_workers(Box::new(workers));
    }

    /// What the orchestrator and its stores reported since the last call,
    /// oldest first.
    pub(crate) fn take_events(&mut self) -> Vec<RendererEvent> {
        std::mem::take(&mut self.events)
    }

    /// The first phase of a frame: brings the orchestrator to
    /// `update_parameters` and returns the frame for the encode, or `None`
    /// for a still image that hasn't loaded yet, which isn't drawn.
    ///
    /// It applies what changed in the style since the version the last
    /// update drew, evaluates the layers, updates the sources' tiles and
    /// their feature state, chooses the layers the frame draws and gives
    /// them their tiles, places the symbols and brings the symbol buckets
    /// to the placement. The encode then changes none of this: it changes
    /// only what draws it on the GPU, uploading the tiles' atlas and
    /// buckets and making the layers' draws.
    pub(crate) fn update(&mut self, update_parameters: &Arc<UpdateParameters>) -> Option<Frame> {
        let mut frame = Frame::new(&update_parameters.view, update_parameters.camera_moving);
        let mut clock = self.advance_clock(update_parameters);
        let style = &update_parameters.style;

        self.tile_store.glyphs().set_url(style.get_glyph_url());
        self.tile_store.glyphs().set_font_faces(update_parameters.font_faces.clone());
        self.tile_store.set_file_source(update_parameters.file_source.clone());

        // What arrived for the tiles before the sources update, so that a
        // tile laid out since the last update renders in this one.
        self.receive_tiles();

        self.update_light_and_sky(style, &clock);
        frame.light = self.render_light.evaluated().clone();
        frame.sky = self.render_sky.as_ref().map(|render_sky| render_sky.evaluated().clone());

        // The previous sources may hold the last references to replaced
        // data, such as a GeoJSON source's index, which can take
        // milliseconds to free. The workers drop them once this update has
        // moved the render sources and their tiles to the new ones. Declared
        // before the diff, which holds previous sources too, so that it
        // drops after the diff. The previous version itself drops here, on
        // the render thread, with its layers and images.
        let previous_sources = self.style.as_ref().map(|previous| previous.get_sources());
        let sources_changed = previous_sources.is_none_or(|previous| !Arc::ptr_eq(previous, style.get_sources()));
        let _previous_sources = previous_sources
            .filter(|_| sources_changed)
            .map(|previous| DropOnWorkers::new(Arc::clone(previous), self.thread_pool.clone()));
        let StyleDiff { layers: layer_changes, sources: source_diff, images: image_diff } =
            StyleDiff::between(self.style.as_deref(), style);
        if sources_changed {
            self.sources_by_id = sources_by_id(style.get_sources());
        }
        self.style = Some(Arc::clone(style));
        let layers = Arc::clone(style.get_layer_list());

        self.apply_image_diff(&image_diff, update_parameters.sprite_loaded);
        // The drapes' keys don't tell what the style's images draw.
        if !(image_diff.added.is_empty() && image_diff.removed.is_empty() && image_diff.changed.is_empty()) {
            self.terrain.redraw_drapes();
        }

        let render_layers_remade = self.apply_layer_changes(&layer_changes, &layers, &clock);
        let constants_mask_changed = self.evaluate_layers(&layer_changes, render_layers_remade, &mut clock);
        self.apply_source_diff(&source_diff, update_parameters);

        // A still image fails at the first resource error, and nothing
        // requests a failed resource again on its own: each new still image
        // retries them instead of waiting on them or rendering without them.
        if !clock.continuous
            && update_parameters.still_image_request
            && update_parameters.still_image_request_id != self.still_image_request_id
        {
            self.still_image_request_id = update_parameters.still_image_request_id;
            // A tile laid out without the glyphs of a failed range lays out
            // again, which requests the range again.
            self.tile_store.retry_failed();
        }
        self.view = update_parameters.view.clone();

        // The terrain's cover is made of its DEM's, which the DEM's origin
        // makes of its source as the style has it now.
        let style = &update_parameters.style;
        if let Some(terrain) = style.get_terrain()
            && let Some(style_source) = style.get_source(&terrain.source)
            && let Some(source) = self.render_sources.get_mut(&terrain.source)
        {
            source.update_origin(&mut self.tile_store, style_source, update_parameters);
        }
        let (store, render_sources) = (&self.tile_store, &self.render_sources);
        let dem = |id: &str| store.source(render_sources.get(id)?.tiles()?);
        frame.terrain = self.terrain.update(update_parameters, dem).is_some();
        let will_render = self.update_sources(
            update_parameters,
            &layers,
            &layer_changes,
            &constants_mask_changed,
            &clock,
            &mut frame,
        );
        let (store, render_sources) = (&self.tile_store, &self.render_sources);
        let dem = |id: &str| store.source(render_sources.get(id)?.tiles()?);
        let grounded = self.terrain.bind(dem, update_parameters, &mut self.events);

        #[cfg(feature = "bench")]
        crate::bench::observe_cover(&self.view, &self.tile_store);

        // A synchronous source's tile laid out by its answer isn't in this
        // update's render tiles yet: the next update has it.
        let answered = self.tile_store.drive();
        self.forward_store_events();
        if answered {
            self.events.push(RendererEvent::Invalidate);
        }

        // The sources' tiles are in other world copies once the center has
        // crossed the antimeridian (`SourceTiles::handle_wrap_jump`), and
        // the layers' draws go with them.
        let longitude = update_parameters.view.center(mln_style::geo::WrapMode::Unwrapped).longitude() as f32;
        let wrap_delta = ((longitude - self.longitude) / 360.0).round() as i16;
        self.longitude = longitude;
        if wrap_delta != 0 {
            self.terrain.handle_wrap_jump();
        }
        for (layer, will_render) in self.layers.iter_mut().zip(will_render) {
            if wrap_delta != 0 {
                layer.handle_wrap_jump(wrap_delta);
            }
            if layer.is_renderable() != will_render {
                layer.set_renderable(will_render);
            }
        }

        // A frame whose camera would move on the terrain's current ground
        // isn't complete: the map answers the new ground with an update, and
        // a still image draws only the frame of an update that stands on it.
        frame.loaded = update_parameters.style_loaded && !answered && grounded && self.is_loaded();
        if !clock.continuous && !frame.loaded {
            return None;
        }

        // The buckets whose vertex data the frame's feature state and symbol
        // placement change, which the draws that copied it copy again.
        let mut changed_buckets = ChangedBuckets::default();
        self.update_render_tiles(&frame, &mut changed_buckets);
        self.update_layer_tiles(update_parameters, &frame);

        frame.symbols_started = Instant::now();
        let placed = self.place(update_parameters, &mut frame);

        if !frame.needs_repaint && frame.loaded {
            // Ask the app to remove the unused images it added once the map
            // is fully loaded and there are no ongoing transitions.
            if self.tile_store.images().exceeds_cache_size_limit() {
                let used = self.tile_store.images_used();
                self.tile_store.images_mut().reduce_memory_use(&used);
                self.events.extend(self.tile_store.take_resource_events());
            }
        }

        // The buckets of the layers that place, and of those that draw
        // buckets other layers place, follow the placement.
        self.placements.update(
            &frame.transform.view,
            update_parameters.mode,
            placed,
            &mut self.tile_store,
            &mut changed_buckets,
        );
        if !changed_buckets.is_empty() {
            for layer in &mut self.layers {
                layer.mark_changed(&changed_buckets);
            }
            self.terrain.mark_changed(&changed_buckets);
        }

        Some(frame)
    }

    /// Brings the feature state of the enabled sources' tiles to the
    /// sources', recording the buckets it changes in `changed_buckets`, and
    /// computes their tiles' matrices for the frame. A source that a layer
    /// of the frame draws in [`TileOrder::ByY`] sorts its tiles so.
    fn update_render_tiles(&mut self, frame: &Frame, changed_buckets: &mut ChangedBuckets) {
        let mut sorted_by_y: SmallVec<[&str; 4]> = SmallVec::new();
        for (&index, frame_layer) in &frame.layers {
            if let Some(source) = frame_layer.source.as_deref()
                && self.layers[index as usize].tile_order() == TileOrder::ByY
                && !sorted_by_y.contains(&source)
            {
                sorted_by_y.push(source);
            }
        }

        for (name, render_source) in &mut self.render_sources {
            if render_source.is_enabled() {
                if let Some((tiles, _)) = render_source.tiles().and_then(|key| self.tile_store.source_mut(key)) {
                    tiles.coalesce_feature_state(changed_buckets);
                }
                render_source.update_render_tiles(
                    &mut self.tile_store,
                    &RenderTileParameters {
                        transform: &frame.transform,
                        sort_by_y: sorted_by_y.contains(&name.as_str()),
                    },
                );
            } else {
                render_source.release_render_data();
            }
        }
    }

    /// Gives each layer of the frame its tiles, in the order it draws them,
    /// and the layers of the types that place to their participants.
    fn update_layer_tiles(&mut self, update_parameters: &UpdateParameters, frame: &Frame) {
        let mut contents = SourceContents::default();
        self.found_tiles.clear();
        self.placements.begin_frame();
        for (&index, frame_layer) in &frame.layers {
            let index = index as usize;
            let render_layer = &mut self.layers[index];
            let source = frame_layer.source.as_ref().and_then(|id| self.render_sources.get_key_value(id));
            let source = source.map(|(id, source)| (id.as_str(), &**source));
            let source_contents = contents.of(source, &self.tile_store);
            let source = source.map(|(_, source)| source);
            render_layer.update(&mut LayerUpdateParameters {
                view: &frame.transform.view,
                update_parameters,
                source,
                contents: source_contents,
                images: self.tile_store.images(),
                found: &mut self.found_tiles,
            });
            let tiles =
                source.map(|source| FrameSourceTiles::new(source.render_tiles(), source_contents, &self.found_tiles));
            self.placements.collect(index, &**render_layer, tiles, source.and_then(RenderSource::tiles));
        }
    }

    /// Advances the zoom history and the global state to the update's, and
    /// says how the update evaluates and transitions the layers.
    fn advance_clock(&mut self, update_parameters: &UpdateParameters) -> FrameClock {
        let continuous = update_parameters.mode == MapMode::Continuous;
        if !continuous {
            // Reset zoom history state.
            self.zoom_history.first = true;
        }

        let zoom_changed = self.zoom_history.update(update_parameters.view.zoom() as f32, update_parameters.time_point);

        let style = &update_parameters.style;
        let transition_options = if continuous { style.get_transition_options() } else { TransitionOptions::default() };

        let transition_duration = transition_options.duration.unwrap_or(if continuous {
            DEFAULT_TRANSITION_DURATION
        } else {
            Duration::ZERO
        });

        let new_global_state = Some(Arc::clone(style.get_global_state()));
        let global_state_changed = !ptr_eq_option(self.global_state.as_ref(), new_global_state.as_ref());
        let mut changed_global_state_keys: Option<Arc<VecSet<String>>> = None;
        let mut global_state_change = None;
        if global_state_changed && let (Some(previous), Some(current)) = (&self.global_state, &new_global_state) {
            let changed_keys = Arc::new(diff_global_state_keys(previous, current));
            changed_global_state_keys = Some(Arc::clone(&changed_keys));
            global_state_change =
                Some(GlobalStateChange { prior_state: Arc::clone(previous), changed_keys, state_only: false });
        }
        self.global_state = new_global_state;

        // A layer added this frame has shown nothing yet, so a global-state
        // change leaves it nothing to transition from.
        let added_layer_transition = TransitionParameters::new(update_parameters.time_point, transition_options);
        let transition = TransitionParameters { global_state_change, ..added_layer_transition.clone() };

        // Transitions the values that read a changed global-state key, where
        // nothing else changed.
        let global_state_transition = transition.global_state_change.as_ref().map(|change| TransitionParameters {
            global_state_change: Some(GlobalStateChange { state_only: true, ..change.clone() }),
            ..transition.clone()
        });

        let mut evaluation = PropertyEvaluationParameters::with_zoom_history(
            self.zoom_history,
            update_parameters.time_point,
            transition_duration,
        );
        evaluation.zoom_changed = zoom_changed;
        evaluation.global_state = self.global_state.clone();
        evaluation.global_state_changed = global_state_changed;
        evaluation.changed_global_state_keys = changed_global_state_keys.clone();

        FrameClock {
            continuous,
            zoom_changed,
            global_state_changed,
            changed_global_state_keys,
            added_layer_transition,
            transition,
            global_state_transition,
            evaluation,
        }
    }

    /// Transitions and evaluates the light and the sky, keeping the running
    /// transitions of a sky that changes.
    fn update_light_and_sky(&mut self, style: &Style, clock: &FrameClock) {
        let light_changed = !Arc::ptr_eq(&self.render_light.light, style.get_light());

        if light_changed {
            self.render_light.light = Arc::clone(style.get_light());
            self.render_light.transition(&clock.transition);
        } else if let Some(parameters) = &clock.global_state_transition {
            self.render_light.transition(parameters);
        }

        if light_changed || clock.zoom_changed || clock.global_state_changed || self.render_light.has_transition() {
            self.render_light.evaluate(&clock.evaluation);
        }

        let mut sky_changed = false;
        if let Some(sky) = style.get_sky() {
            match &mut self.render_sky {
                None => {
                    self.render_sky = Some(crate::render::sky::RenderSky::new(Arc::clone(sky)));
                    sky_changed = true;
                }
                Some(render_sky) if !Arc::ptr_eq(&render_sky.sky, sky) => {
                    render_sky.sky = Arc::clone(sky);
                    render_sky.transition(&clock.transition);
                    sky_changed = true;
                }
                Some(render_sky) => {
                    if let Some(parameters) = &clock.global_state_transition {
                        render_sky.transition(parameters);
                    }
                }
            }
        } else if self.render_sky.is_some() {
            self.render_sky = None;
            sky_changed = true;
        }

        if let Some(render_sky) = &mut self.render_sky
            && (sky_changed || clock.zoom_changed || clock.global_state_changed || render_sky.has_transition())
        {
            render_sky.evaluate(&clock.evaluation);
        }
    }

    /// Adding or removing an image changes the layout of the tiles whose
    /// dependencies name it, and updating one does when its size or SDF
    /// flag changes. An update of the same size patches its atlas bins.
    fn apply_image_diff(&mut self, image_diff: &ItemChanges<Arc<ImageImpl>>, sprite_loaded: bool) {
        let images = self.tile_store.images_mut();
        images.begin_update();
        let mut relayout_images: HashSet<String> = HashSet::default();
        for id in image_diff.removed.keys() {
            images.remove_image(id);
            relayout_images.insert(id.clone());
        }
        for image in image_diff.added.values() {
            images.add_image(Arc::clone(image));
            relayout_images.insert(image.id.clone());
        }
        let mut patched = Vec::new();
        for image in image_diff.changed.values() {
            if images.update_image(Arc::clone(image)) {
                relayout_images.insert(image.id.clone());
            }
            patched.push(Arc::clone(image));
        }
        images.set_loaded(sprite_loaded);
        for image in patched {
            self.tile_store.atlas().patch_image(&image);
        }
        if !relayout_images.is_empty() {
            self.tile_store.images_changed(&relayout_images);
        }
    }

    /// Makes, removes and restyles the render layers for the layers that
    /// changed, and puts them in the style's order. Returns whether it made
    /// every render layer again, for new shaders.
    fn apply_layer_changes(&mut self, layer_changes: &LayerChanges, layers: &LayerList, clock: &FrameClock) -> bool {
        // Make each render layer again from its layer, without the relayout
        // that adding the layer would take.
        let render_layers_remade = std::mem::take(&mut self.remake_render_layers);
        if render_layers_remade {
            for render_layer in &mut self.layers {
                let style = Arc::clone(&render_layer.base().style);
                let Some(mut new_layer) = create_render_layer(&self.registry, &style) else {
                    continue;
                };
                new_layer.transition(&clock.added_layer_transition);
                render_layer.leave(&mut self.ended_hosts);
                *render_layer = new_layer;
            }
        }

        if layer_changes.order_changed {
            let mut by_id: HashMap<String, Box<dyn RenderLayer>> =
                self.layers.drain(..).map(|layer| (layer.id().to_owned(), layer)).collect();

            // Remove render layers for removed layers.
            for (id, _) in layer_changes.iter().filter(|(_, change)| change.removes()) {
                if let Some(mut removed) = by_id.remove(&**id) {
                    removed.leave(&mut self.ended_hosts);
                }
            }

            // Create render layers for newly added layers.
            for (id, _) in layer_changes.iter().filter(|(_, change)| change.adds()) {
                let Some(layer) = layers.get(id) else { continue };
                if let Some(mut render_layer) = create_render_layer(&self.registry, layer.style()) {
                    render_layer.transition(&clock.added_layer_transition);
                    // A layer added twice keeps its first render layer.
                    by_id.entry(id.to_string()).or_insert(render_layer);
                }
            }

            // A layer type without a render layer is skipped.
            self.layers = layers.iter().filter_map(|layer| by_id.remove(layer.get_id())).collect();
            for mut left in by_id.into_values() {
                left.leave(&mut self.ended_hosts);
            }
            self.layer_indexes =
                self.layers.iter().enumerate().map(|(index, layer)| (layer.id().to_owned(), index)).collect();
        }

        // Update render layers for changed layers.
        for (id, _) in layer_changes.iter().filter(|(_, change)| change.restyles()) {
            if let (Some(&index), Some(layer)) = (self.layer_indexes.get(&**id), layers.get(id)) {
                let new_layer = layer.style();
                let render_layer = &mut self.layers[index];
                render_layer.restyle(&clock.transition, new_layer, &mut self.ended_hosts);
            }
        }

        if render_layers_remade || layer_changes.order_changed || !layer_changes.is_empty() {
            self.tile_store.glyphs().evict(&layer::font_stacks(layers.iter().map(Layer::style)));
        }
        render_layers_remade
    }

    /// Transitions and evaluates the layers whose values may have changed:
    /// the added and changed ones, the ones whose values depend on a zoom or
    /// global state that changed, and the ones still transitioning or
    /// crossfading. Returns the ids of the layers whose evaluated constants
    /// changed, which lays their sources out again.
    fn evaluate_layers(
        &mut self,
        layer_changes: &LayerChanges,
        render_layers_remade: bool,
        clock: &mut FrameClock,
    ) -> VecSet<String> {
        let mut constants_mask_changed: VecSet<String> = VecSet::new();
        for layer in &mut self.layers {
            let id = layer.id().to_owned();
            let layer_added_or_changed =
                render_layers_remade || layer_changes.get(&id).is_some_and(|change| change != LayerChange::Removed);
            clock.evaluation.layer_changed = layer_added_or_changed;
            clock.evaluation.has_crossfade = layer.has_crossfade();

            // Only re-evaluate on change of zoom if the style has some
            // reference to it
            let zoom_changed_and_matters = clock.zoom_changed
                && !layer_added_or_changed
                && layer.style_dependencies().intersects(Dependency::ZOOM);

            // Similarly, only re-evaluate on a global state change if the
            // layer references the global state.
            let global_state_changed_and_matters = clock.global_state_changed
                && !layer_added_or_changed
                && layer.style_dependencies().intersects(Dependency::GLOBAL_STATE);

            // A running transition may read the changed state too, from a
            // value the layer no longer has.
            if let Some(parameters) = &clock.global_state_transition
                && !layer_added_or_changed
                && (global_state_changed_and_matters || layer.has_transition())
            {
                layer.transition(parameters);
            }

            if layer_added_or_changed
                || zoom_changed_and_matters
                || global_state_changed_and_matters
                || clock.evaluation.has_crossfade
                || layer.has_transition()
            {
                let previous_mask = layer.base().evaluated_properties.constants_mask();
                layer.evaluate(&clock.evaluation);
                if previous_mask != layer.base().evaluated_properties.constants_mask() {
                    constants_mask_changed.insert(id);
                }
            }
        }
        constants_mask_changed
    }

    /// Makes and removes the render sources for the sources that changed,
    /// and gives every source the update's feature state.
    fn apply_source_diff(&mut self, source_diff: &ItemChanges<Source>, update_parameters: &UpdateParameters) {
        for id in source_diff.removed.keys() {
            if let Some(key) = self.render_sources.remove(id).and_then(|source| source.tiles()) {
                self.tile_store.remove_source(key);
            }
        }

        for (id, style_source) in &source_diff.added {
            if self.render_sources.contains_key(id) {
                continue;
            }
            let Some(mut render_source) = render_source::create(&self.registry, style_source, &mut self.tile_store)
            else {
                continue;
            };
            render_source.set_fast_pfor_enabled(update_parameters.fast_pfor_enabled);
            self.render_sources.insert(id.clone(), render_source);
        }

        // The update carries the map's whole feature state, so a render
        // source, whether new, kept or rebuilt after `clear_data`, shows the
        // state of the update it renders. A source whose state the update
        // didn't change does nothing.
        for (id, render_source) in &self.render_sources {
            if let Some((source, _)) = render_source.tiles().and_then(|key| self.tile_store.source_mut(key)) {
                source.info.feature_state.set_states(update_parameters.feature_states.get(id));
            }
        }
    }

    /// Chooses the layers the frame draws, into `frame`, and updates each
    /// source's tiles for the layers it has: whether it needs tiles at all,
    /// and whether they lay out again. Returns whether each layer draws.
    fn update_sources(
        &mut self,
        update_parameters: &UpdateParameters,
        layers: &LayerList,
        layer_changes: &LayerChanges,
        constants_mask_changed: &VecSet<String>,
        clock: &FrameClock,
        frame: &mut Frame,
    ) -> Vec<bool> {
        let mut tile_parameters = TileParameters {
            pixel_ratio: update_parameters.pixel_ratio,
            view: &update_parameters.view,
            min_zoom: update_parameters.min_zoom,
            max_zoom: update_parameters.max_zoom,
            file_source: update_parameters.file_source.clone(),
            mode: update_parameters.mode,
            prefetch_zoom_delta: update_parameters.prefetch_zoom_delta,
            tile_lod_min_radius: update_parameters.tile_lod_min_radius,
            tile_lod_scale: update_parameters.tile_lod_scale,
            tile_lod_pitch_threshold: update_parameters.tile_lod_pitch_threshold,
            tile_lod_zoom_shift: update_parameters.tile_lod_zoom_shift,
            tile_lod_mode: update_parameters.tile_lod_mode,
            subdivision_granularity: update_parameters.subdivision_granularity,
            global_state: self.global_state.clone(),
            is_update_synchronous: false,
            font_faces: update_parameters.font_faces.clone(),
            glyph_pbfs: !update_parameters.style.get_glyph_url().is_empty(),
            placement_commits: self.placements.commits(),
            terrain: self.terrain.cover(),
        };

        if !self.sources_by_id.is_empty() {
            self.filtered_layers_for_source.reserve(layers.len());
        }

        let mut will_render: Vec<bool> = vec![false; self.layers.len()];

        // Each source in turn takes its layers. The layers without a source
        // go with the first, and with no sources the loop runs once for
        // them.
        let source_count = std::cmp::max(self.sources_by_id.len(), 1);
        for source_index in 0..source_count {
            let style_source = self.sources_by_id.get(source_index).cloned();
            let mut source_needs_rendering = false;
            let mut source_needs_relayout = false;
            // The layers of a source type without a render source are
            // skipped.
            let has_render_source =
                style_source.as_ref().is_some_and(|style_source| self.render_sources.contains_key(style_source.id()));
            // The id of the source whose layers this pass takes, if it has a
            // render source.
            let render_source_id = style_source.as_ref().filter(|_| has_render_source).map(Source::id);

            for (index, layer) in self.layers.iter().enumerate() {
                let style = &layer.base().style;
                let layer_info = style.info();

                if layer_info.source != SourceKind::None {
                    // Every source's pass sees every layer, so the cheap
                    // source check goes before anything else is read.
                    if let Some(style_source) = &style_source
                        && render_source_id == Some(style.base().source.as_str())
                    {
                        let layer_is_visible = style.base().visibility != mln_style::types::VisibilityType::None;
                        let zoom_fits_layer = layer.supports_zoom(self.zoom_history.last_zoom);
                        let layer_id = layer.id().to_owned();
                        // A global state change requires a relayout of
                        // this layer's source if one of the changed state
                        // properties is referenced by anything that is
                        // baked into the tile's buckets.
                        let global_state_requires_relayout = {
                            if !clock.global_state_changed
                                || !(style.layout_dependencies()
                                    | layer.base().evaluated_properties.evaluated_dependencies())
                                .intersects(Dependency::GLOBAL_STATE)
                            {
                                false
                            } else if clock.changed_global_state_keys.is_none() {
                                true
                            } else {
                                let mut refs = VecSet::new();
                                style.collect_layout_global_state_refs(&mut refs);
                                layer.base().evaluated_properties.collect_evaluated_global_state_refs(&mut refs);
                                mln_style::expression::global_state_refs_intersect(
                                    Some(&refs),
                                    clock.changed_global_state_keys.as_deref(),
                                )
                            }
                        };
                        source_needs_relayout = source_needs_relayout
                            || constants_mask_changed.contains(&layer_id)
                            || layer_changes.get(&layer_id).is_some_and(LayerChange::relayouts)
                            || global_state_requires_relayout;
                        if layer_is_visible {
                            self.filtered_layers_for_source.push(Arc::clone(&layer.base().evaluated_properties));
                            if zoom_fits_layer {
                                source_needs_rendering = true;
                                let source_id = style_source.id().to_owned();
                                frame
                                    .layers
                                    .entry(index as u32)
                                    .or_insert_with(|| FrameLayer { source: Some(source_id) });
                                will_render[index] = true;
                            }
                        }
                    }
                    continue;
                }

                // Handle layers without source.
                let is_first_source = source_index == 0;
                if is_first_source
                    && style.base().visibility != mln_style::types::VisibilityType::None
                    && layer.supports_zoom(self.zoom_history.last_zoom)
                {
                    // On the globe the background is a sphere mesh, not the
                    // backdrop, and over terrain it drapes, so it stays a
                    // layer.
                    if self.background_layer_as_color
                        && frame.sky.is_none()
                        && !update_parameters.view.is_globe()
                        && !frame.terrain
                        && layers.first().is_some_and(|first| Arc::ptr_eq(style, first.style()))
                        && let Some(solid_background) = layer.base().evaluation.solid
                    {
                        frame.background_color = solid_background;
                        continue; // This layer is shown with background color,
                        // and it shall not be added to render items.
                    }
                    frame.layers.entry(index as u32).or_insert_with(|| FrameLayer { source: None });
                    will_render[index] = true;
                }
            }
            if let Some(style_source) = &style_source {
                tile_parameters.is_update_synchronous = style_source.data().is_update_synchronous();
                let filtered_layers = std::mem::take(&mut self.filtered_layers_for_source);
                if let Some(source) = self.render_sources.get_mut(style_source.id()) {
                    source.update(
                        &mut self.tile_store,
                        style_source.clone(),
                        &filtered_layers,
                        source_needs_rendering,
                        source_needs_relayout,
                        &tile_parameters,
                    );
                }
                self.filtered_layers_for_source = filtered_layers;
                self.filtered_layers_for_source.clear();
            }
        }
        will_render
    }

    /// The frame's placement step: places across tiles for a continuous
    /// map, when the last placement isn't recent, or all at once for a still
    /// image. Sets the frame's fade and repaint parameters, and returns
    /// whether the frame placed, or dropped the placement.
    fn place(&mut self, update_parameters: &Arc<UpdateParameters>, frame: &mut Frame) -> bool {
        let fading_tiles = self.tile_store.has_fading_tiles();
        let placed =
            self.placements.place(update_parameters, self.terrain.ground(), &mut self.tile_store, fading_tiles);
        frame.placement_changed = placed.changed;
        frame.symbol_fade_change = placed.fade;
        if placed.repaint_at.is_some() {
            frame.repaint_at = placed.repaint_at;
        }
        frame.needs_repaint =
            update_parameters.mode == MapMode::Continuous && self.has_transitions(update_parameters.time_point);
        placed.changed
    }

    pub(crate) fn has_transitions(&self, time_point: Option<Instant>) -> bool {
        if self.render_light.has_transition() {
            return true;
        }

        if self.render_sky.as_ref().is_some_and(crate::render::sky::RenderSky::has_transition) {
            return true;
        }

        if self.layers.iter().any(|layer| layer.has_transition()) {
            return true;
        }

        if self.placements.has_transitions(time_point) {
            return true;
        }

        self.tile_store.has_fading_tiles()
    }

    pub(crate) fn is_loaded(&self) -> bool {
        // do the simple boolean check before iterating over all the tiles in
        // all the sources
        if !self.tile_store.images().is_loaded() {
            return false;
        }

        for source in self.render_sources.values() {
            if !source.is_loaded(&self.tile_store) {
                return false;
            }
        }

        true
    }

    pub(crate) fn clear_data(&mut self) {
        // The next update finds every layer, source and image added. The
        // global state, light and sky stay, so the next update doesn't
        // transition them again.
        self.style = None;
        self.sources_by_id.clear();

        self.render_sources.clear();
        self.tile_store.clear();
        self.leave_layers();
        self.layer_indexes.clear();

        self.placements.forget_buckets();

        self.tile_store.images_mut().clear();
        self.tile_store.glyphs().evict(&VecSet::new());
        self.terrain.clear_data(&mut self.events);
    }

    /// Drops what holds the renderer's shaders: the sky and globe depth
    /// passes and the terrain's draws now, and the render layers in the next
    /// update, which makes them again. The draws made after that use the
    /// shaders the renderer has then. The sources keep their tiles, and the
    /// tiles their layout.
    pub(crate) fn release_shaders(&mut self) {
        self.remake_render_layers = true;
        self.sky_pass = crate::render::sky_pass::SkyPass::default();
        self.globe_depth_pass = GlobeDepthPass::default();
        self.terrain.release_shaders();
    }

    /// Makes the GPU textures of the tiles' atlas, and uploads what it
    /// queued.
    pub(crate) fn upload_atlas(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        self.tile_store.atlas().upload(gpu, context);
    }

    /// Takes in what arrived for the tiles, and starts the jobs that it
    /// makes ready.
    pub(crate) fn receive_tiles(&mut self) {
        self.tile_store.receive();
        self.tile_store.drive();
        self.forward_store_events();
    }

    /// Turns what the tile store and its image and glyph stores reported
    /// into the renderer's events.
    fn forward_store_events(&mut self) {
        self.events.extend(self.tile_store.take_resource_events());
        for event in self.tile_store.take_events() {
            match event {
                StoreEvent::Changed => self.events.push(RendererEvent::Invalidate),
                StoreEvent::Error { source, tile, error } => {
                    log::error!(target: log_target::STYLE, "Failed to load tile {tile} for source {source}: {error}");
                    self.events.push(RendererEvent::ResourceError(error));
                    // A failed tile is done loading, and its parents or
                    // children may stand in for it, as when it changes: a
                    // continuous map needs a frame to see that, or it never
                    // reports that it finished loading. The error goes
                    // first, so the still image it fails isn't rendered
                    // again.
                    self.events.push(RendererEvent::Invalidate);
                }
                StoreEvent::Action { op, tile, source } => {
                    self.events.push(RendererEvent::TileAction { op, id: tile, source_id: source });
                }
            }
        }
    }

    /// The tiles of source `id`, if it has any.
    fn source_tiles(&self, id: &str) -> Option<&SourceTiles> {
        self.tile_store.source(self.render_sources.get(id)?.tiles()?)
    }

    fn source_tiles_mut(&mut self, id: &str) -> Option<&mut SourceTiles> {
        let key = self.render_sources.get(id)?.tiles()?;
        Some(self.tile_store.source_mut(key)?.0)
    }

    /// Visits the layers' draw groups of the frame, in drawing order.
    pub(crate) fn visit_draw_groups<'a>(&'a self, mut visit: impl FnMut(DrawGroup<'a>)) {
        for layer in &self.layers {
            layer.visit_draw_groups(&mut visit);
        }
    }

    /// The number of the frame's draw groups above those of its lowest 3D
    /// layer that draws, which draw without a depth test; 0 without one. A
    /// layer may have no draw group or several.
    pub(crate) fn opaque_pass_cutoff(&self) -> u32 {
        let mut above: Option<u32> = None;
        for layer in &self.layers {
            let mut groups = 0;
            layer.visit_draw_groups(&mut |_| groups += 1);
            match &mut above {
                Some(above) => *above += groups,
                None if groups > 0 && layer.volume() => above = Some(0),
                None => {}
            }
        }
        above.unwrap_or(0)
    }

    /// Uploads the GPU objects of the frame's draws: the layers' draw
    /// groups in drawing order, the globe's depth, the terrain, the sky and
    /// the atmosphere, and what draws into the layers' render targets.
    pub(crate) fn upload_draws(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        for layer in &mut self.layers {
            layer.upload(gpu, context);
        }
        self.globe_depth_pass.upload(gpu, context);
        self.terrain.upload(gpu, context);
        if let Some(sky) = self.sky_pass.sky() {
            sky.upload(gpu, context);
        }
        if let Some(atmosphere) = self.sky_pass.atmosphere() {
            atmosphere.upload(gpu, context);
        }
        for layer in &mut self.layers {
            layer.upload_render_targets(gpu, context);
        }
    }

    /// Draws the pass's draws of the layers' draw groups into the open pass
    /// of `gpu`, the context's, in `order`, each at the depth of
    /// `parameters.current_layer`, which `order` steps after each group.
    /// The translucent pass draws each stack's surface under the stack's
    /// first layer, where it draws in the frame.
    pub(crate) fn render_draw_groups(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        order: DrawOrder,
    ) {
        match order {
            DrawOrder::BottomUp if parameters.pass == RenderPass::Translucent && !self.stacks.is_empty() => {
                let mut starts = self.stacks.starts().enumerate().peekable();
                for (index, layer) in self.layers.iter_mut().enumerate() {
                    if let Some((stack, _)) = starts.next_if(|&(_, start)| start as usize == index) {
                        self.terrain.render(stack, gpu, parameters);
                    }
                    layer.render(gpu, parameters, order);
                }
            }
            DrawOrder::BottomUp => {
                for layer in &mut self.layers {
                    layer.render(gpu, parameters, order);
                }
            }
            DrawOrder::TopDown => {
                for layer in self.layers.iter_mut().rev() {
                    layer.render(gpu, parameters, order);
                }
            }
        }
    }

    /// Readies the buckets of the frame's sources for the frame's draws.
    pub(crate) fn upload_sources(&mut self, context: &mut Context) {
        for source in self.render_sources.values_mut() {
            if source.is_enabled() {
                source.upload(&mut self.tile_store, context);
            }
        }
    }

    /// Makes every render layer leave, and drops them.
    pub(crate) fn leave_layers(&mut self) {
        for mut layer in self.layers.drain(..) {
            layer.leave(&mut self.ended_hosts);
        }
    }

    /// The hosts of the custom layers that left since the last call, which
    /// drew and hear that the renderer stopped.
    pub(crate) fn take_ended_hosts(&mut self) -> EndedHosts {
        std::mem::take(&mut self.ended_hosts)
    }

    /// Visits every render layer, in no order.
    pub(crate) fn visit_render_layers(&mut self, mut f: impl FnMut(&mut dyn RenderLayer)) {
        for layer in &mut self.layers {
            f(&mut **layer);
        }
    }

    /// Makes the draws of the layers the frame draws, of the terrain, and of
    /// the sky and globe depth passes. Over terrain, the layers that drape
    /// over it make theirs in the terrain's drapes rather than in the frame,
    /// and those that stand on it stand on the frame's ground, whose depth
    /// draws first where a layer hides behind it.
    pub(crate) fn update_layer_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        update_parameters: &UpdateParameters,
        paint_parameters: &mut PaintParameters<'_>,
        frame: &Frame,
    ) {
        let layers = || frame.layers.keys().map(|&index| &*self.layers[index as usize]);
        let stands = frame.terrain && layers().any(|layer| layer.terrain().stands());
        let hides = layers().any(|layer| layer.terrain().hides_behind_ground());
        paint_parameters.ground = self.terrain.update_ground(paint_parameters, stands, hides);
        // Whether a 3D layer draws before the terrain's first surface, which
        // writes the depth of the ground and of the planet.
        let mut needs_depth = false;
        let mut surface = false;
        let mut contents = SourceContents::default();
        for (&index, frame_layer) in &frame.layers {
            let render_layer = &mut self.layers[index as usize];
            let layer_source =
                layer_source(frame_layer, &self.render_sources, &self.tile_store, &self.found_tiles, &mut contents);
            // Over terrain, a layer that drapes draws nothing onto the frame.
            if frame.terrain && render_layer.terrain().drapes() {
                render_layer.remove_all_draws();
                surface = true;
                continue;
            }
            render_layer.update_draws(gpu, paint_parameters, layer_source);
            needs_depth = needs_depth || (render_layer.volume() && !surface);
        }
        self.stacks = if frame.terrain {
            let layers = &mut self.layers;
            let draped = |&index: &u32| {
                let layer = &mut layers[index as usize];
                let draped = layer.terrain().drapes().then(|| match layer.surface_light(paint_parameters) {
                    Some(light) => Draped::Lit(light),
                    None if layer.blends() => Draped::Blends,
                    None => Draped::Plain,
                });
                (index, draped)
            };
            DrapeStacks::of(frame.layers.keys().map(draped))
        } else {
            DrapeStacks::default()
        };
        self.terrain.update_dems(paint_parameters, |id| self.tile_store.source(self.render_sources.get(id)?.tiles()?));
        if let Some(drapes) = self.terrain.begin_drapes(paint_parameters, self.stacks.len()) {
            let mut contents = SourceContents::default();
            for stack in 0..self.stacks.len() {
                for index in self.stacks.layers(stack) {
                    let layer_source = layer_source(
                        &frame.layers[index],
                        &self.render_sources,
                        &self.tile_store,
                        &self.found_tiles,
                        &mut contents,
                    );
                    let lit = self.stacks.light(stack).is_some();
                    drapes.key_layer(stack, &*self.layers[*index as usize], layer_source, lit);
                }
            }
            drapes.choose(paint_parameters);
            for stack in 0..self.stacks.len() {
                for index in self.stacks.layers(stack) {
                    let layer_source = layer_source(
                        &frame.layers[index],
                        &self.render_sources,
                        &self.tile_store,
                        &self.found_tiles,
                        &mut contents,
                    );
                    let render_layer = &mut *self.layers[*index as usize];
                    let lit = self.stacks.light(stack).is_some();
                    drapes.update_layer(stack, gpu, paint_parameters, render_layer, layer_source, lit);
                }
            }
            drapes.finish_update(paint_parameters.context);
        }
        let state = paint_parameters.view;
        let (renderable_size, pixel_ratio) = (paint_parameters.renderable_size, paint_parameters.pixel_ratio);
        let context = &mut *paint_parameters.context;
        self.sky_pass.update(gpu, context, state, renderable_size, pixel_ratio, frame.sky.as_ref(), &frame.light);
        // Where the terrain's surface draws, its depth is the planet's: a
        // sea-level planet would hide the ground below sea level and the
        // drawn ground's limb. The 3D layers before the first surface stand
        // behind the surface's depth, drawn first.
        let needs_globe_depth = (needs_depth || self.sky_pass.has_atmosphere()) && !surface;
        self.globe_depth_pass.update(gpu, context, state, update_parameters, needs_globe_depth);
        let fog = crate::render::terrain::terrain_fog(frame.sky.as_ref(), paint_parameters.view);
        let ground = paint_parameters.ground.as_ref().is_some_and(|ground| ground.depth().is_some());
        self.terrain.update_draws(gpu, paint_parameters, &self.stacks, fog, (needs_depth && surface, ground));
    }

    /// The render layer at `index` in the drawing order.
    pub(crate) fn layer_mut(&mut self, index: usize) -> &mut dyn RenderLayer {
        &mut *self.layers[index]
    }

    /// The planar sky, which draws before the layers, if the frame draws it.
    pub(crate) fn sky(&mut self) -> Option<&mut SingleDraw> {
        self.sky_pass.sky()
    }

    /// The globe's atmosphere, which draws after the layers, if the frame
    /// draws it.
    pub(crate) fn atmosphere(&mut self) -> Option<&mut SingleDraw> {
        self.sky_pass.atmosphere()
    }

    /// The pass that draws the planet's depth.
    pub(crate) fn globe_depth(&mut self) -> &mut GlobeDepthPass {
        &mut self.globe_depth_pass
    }

    /// The terrain, whose drapes draw before the frame's pass.
    pub(crate) fn terrain(&mut self) -> &mut RenderTerrain {
        &mut self.terrain
    }

    /// Queries the rendered features in `geometry`. `Err` if the query fails
    /// (a malformed tile).
    pub(crate) fn query_rendered_features(
        &mut self,
        geometry: &[crate::geo::ScreenCoordinate],
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<crate::tile::feature::Feature>, Error> {
        // A query answers from the layouts that arrived since the update.
        self.receive_tiles();
        let render_layers: Vec<&dyn RenderLayer> = match &options.layer_ids {
            Some(layer_ids) => layer_ids
                .iter()
                .filter_map(|id| self.layer_indexes.get(id))
                .map(|&index| &*self.layers[index])
                .collect(),
            None => self.layers.iter().map(AsRef::as_ref).collect(),
        };
        let mut layers = crate::query::feature_index::QueryLayers::default();
        for layer in render_layers {
            layers.entry(layer.id().to_owned()).or_insert(layer);
        }

        self.query_rendered_features_in(geometry, options, &layers)
    }

    fn query_rendered_symbols(
        &self,
        results_by_layer: &mut crate::query::feature_index::QueryResults,
        geometry: &[crate::geo::ScreenCoordinate],
        layers: &crate::query::feature_index::QueryLayers<'_>,
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<(), Error> {
        let cross_tile_symbol_index_layers: crate::query::feature_index::QueryLayers<'_> = layers
            .iter()
            .filter(|(_, layer)| layer.base().style.info().cross_tile_index)
            .map(|(id, layer)| (id.clone(), *layer))
            .collect();

        if cross_tile_symbol_index_layers.is_empty() {
            return Ok(());
        }
        let index = self.placements.index();
        let mut rendered_symbols = index.query_rendered(&geometry.to_vec());
        let mut bucket_query_data = Vec::with_capacity(rendered_symbols.len());
        for bucket_instance_id in rendered_symbols.keys() {
            bucket_query_data.push(index.query_data(*bucket_instance_id).map_err(Error::native)?);
        }
        // Symbols sort only within their bucket, so the buckets go in the
        // order of their tiles for a predictable order of all of them.
        bucket_query_data.sort_by(|a, b| {
            (a.tile_id.canonical.z, a.tile_id.canonical.y, a.tile_id.wrap, a.tile_id.canonical.x).cmp(&(
                b.tile_id.canonical.z,
                b.tile_id.canonical.y,
                b.tile_id.wrap,
                b.tile_id.canonical.x,
            ))
        });

        for query_data in bucket_query_data {
            let Some(feature_index) = &query_data.feature_index else {
                continue;
            };
            let symbol_features = rendered_symbols.remove(&query_data.bucket_instance_id).unwrap_or_default();
            let selection = crate::query::feature_index::FeatureSelection {
                layers: &cross_tile_symbol_index_layers,
                options,
                global_state: self.global_state.as_deref(),
                tile_id: &query_data.tile_id,
                feature_state: self.tile_store.source(query_data.source).map(|source| &source.info.feature_state),
            };
            let bucket_symbols =
                feature_index.lookup_symbol_features(&symbol_features, &selection, &query_data.feature_sort_order)?;

            for (layer_id, features) in bucket_symbols {
                results_by_layer.entry(layer_id).or_default().extend(features);
            }
        }
        Ok(())
    }

    fn query_rendered_features_in(
        &self,
        geometry: &[crate::geo::ScreenCoordinate],
        options: &crate::query::RenderedQueryOptions,
        layers: &crate::query::feature_index::QueryLayers<'_>,
    ) -> Result<Vec<crate::tile::feature::Feature>, Error> {
        let mut source_ids: VecSet<String> = VecSet::new();
        let mut filtered_layers = crate::query::feature_index::QueryLayers::default();
        for (id, layer) in layers {
            if !layer.needs_rendering() || !layer.supports_zoom(self.zoom_history.last_zoom) {
                continue;
            }
            filtered_layers.insert(id.clone(), *layer);
            source_ids.insert(layer.base().style.base().source.clone());
        }

        let proj_matrix = self.view.projection_matrix_with(self.view.near_z(), false);

        // Over terrain, a layer that drapes is found on the ground the frame
        // drew, where the query's lines of sight meet it, and so is a 3D
        // layer that stands on it, by its footprint there. One whose points
        // stand on the ground is found wherever they show, raised, as its
        // hit test decides. The others draw flat, at sea level, and are
        // found there.
        let elevation = self.terrain.ground().map(|ground| &**ground);
        let ground_of = |layer: &dyn RenderLayer| {
            let terrain = layer.terrain();
            let elevation = elevation.filter(|_| terrain != OnTerrain::Flat)?;
            let standing = terrain.stands() && !layer.volume();
            Some(crate::query::feature_index::QueryGround { elevation, standing })
        };
        // Traced once for every source. Symbols are found where they were
        // placed, on the screen, so they take the geometry as it is.
        let traced = crate::query::feature_index::query_screen_geometry(&self.view, geometry, elevation.is_some());
        let mut results_by_layer = crate::query::feature_index::QueryResults::default();
        // Flat, draped, then standing.
        for pass in [None, Some(false), Some(true)] {
            let layers: crate::query::feature_index::QueryLayers<'_> = filtered_layers
                .iter()
                .filter(|(_, layer)| ground_of(**layer).map(|ground| ground.standing) == pass)
                .map(|(id, layer)| (id.clone(), *layer))
                .collect();
            if layers.is_empty() {
                continue;
            }
            let ground = pass.and_then(|standing| {
                elevation.map(|elevation| crate::query::feature_index::QueryGround { elevation, standing })
            });
            for source_id in &source_ids {
                if let Some(source) = self.source_tiles(source_id) {
                    let source_results = source.query_rendered_features(
                        &traced,
                        ground,
                        &self.view,
                        &layers,
                        options,
                        self.global_state.as_deref(),
                        &proj_matrix,
                    )?;
                    for (layer_id, features) in source_results {
                        results_by_layer.entry(layer_id).or_insert(features);
                    }
                }
            }
        }

        self.query_rendered_symbols(&mut results_by_layer, geometry, &filtered_layers, options)?;

        let mut result = Vec::new();

        if results_by_layer.is_empty() {
            return Ok(result);
        }

        // The results go from the topmost style layer down.
        let layers = self.style.as_ref().map(|style| Arc::clone(style.get_layer_list())).unwrap_or_default();
        for style_layer in layers.iter().rev() {
            let id = style_layer.get_id();
            if !filtered_layers.contains_key(id) {
                continue;
            }
            if let Some(mut features) = results_by_layer.remove(id) {
                for feature in &mut features {
                    id.clone_into(&mut feature.layer);
                }
                result.extend(features);
            }
        }

        Ok(result)
    }

    /// A copy of the placed symbols' data (the placement is shared).
    pub(crate) fn placed_symbols_data(&self) -> Vec<crate::placement::PlacedSymbolData> {
        self.placements.placed_symbols_data().to_vec()
    }

    /// Queries the features of the source `source_id`. `Err`: as
    /// [`query_rendered_features`](Self::query_rendered_features).
    pub(crate) fn query_source_features(
        &mut self,
        source_id: &str,
        options: &crate::query::SourceQueryOptions,
    ) -> Result<Vec<crate::tile::feature::Feature>, Error> {
        self.receive_tiles();
        let Some(source) = self.source_tiles(source_id) else {
            return Ok(Vec::new());
        };

        source.query_source_features(options, self.global_state.as_deref())
    }

    /// Queries a feature extension of the source `source_id`; a null value
    /// for a source the style doesn't have.
    pub(crate) fn query_feature_extensions(
        &self,
        source_id: &str,
        feature: &crate::tile::feature::Feature,
        extension: &str,
        extension_field: &str,
        args: Option<&feature::PropertyMap>,
    ) -> Result<crate::query::FeatureExtensionValue, Error> {
        match self.render_sources.get(source_id) {
            Some(source) => source.query_feature_extensions(feature, extension, extension_field, args),
            None => Ok(crate::query::FeatureExtensionValue::default()),
        }
    }

    pub(crate) fn dump_debug_logs(&self) {
        for source in self.render_sources.values() {
            source.dump_debug_logs();
        }
        self.tile_store.dump_debug_logs();
    }

    pub(crate) fn set_feature_state(
        &mut self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
        state: &mln_style::feature::FeatureState,
    ) {
        if let Some(source) = self.source_tiles_mut(source_id) {
            source.info.feature_state.update_state(source_layer_id, feature_id, state);
        }
    }

    pub(crate) fn feature_state(
        &self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
    ) -> mln_style::feature::FeatureState {
        match self.source_tiles(source_id) {
            Some(source) => source.info.feature_state.state(source_layer_id, feature_id),
            None => mln_style::feature::FeatureState::default(),
        }
    }

    pub(crate) fn remove_feature_state(
        &mut self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: Option<&str>,
        state_key: Option<&str>,
    ) {
        if let Some(source) = self.source_tiles_mut(source_id) {
            source.info.feature_state.remove_state(source_layer_id, feature_id, state_key);
        }
    }

    pub(crate) fn collect_placed_symbol_data(&mut self, enable: bool) {
        self.placements.collect_placed_symbol_data(enable);
    }

    pub(crate) fn set_tile_cache_enabled(&mut self, enable: bool) {
        self.tile_cache_enabled = enable;
        self.tile_store.set_cache_enabled(enable);
    }

    pub(crate) fn reduce_memory_use(&mut self) {
        self.filtered_layers_for_source.shrink_to_fit();
        self.tile_store.reduce_memory_use();
        let used = self.tile_store.images_used();
        self.tile_store.images_mut().reduce_memory_use(&used);
        self.events.extend(self.tile_store.take_resource_events());
        self.events.push(RendererEvent::Invalidate);
    }

    pub(crate) fn tile_cache_enabled(&self) -> bool {
        self.tile_cache_enabled
    }
}

impl RenderOrchestrator {
    /// Drops `value` on the renderer's workers instead of this thread. The
    /// orchestrator's drop waits for it.
    pub(crate) fn drop_on_workers<T: Send + 'static>(&self, value: T) {
        drop_on_workers(&self.thread_pool, value);
    }
}

/// What the render tiles of a source show, at their indexes, for the layers
/// of a phase of the frame, which go through their sources in runs.
#[derive(Default)]
struct SourceContents<'a> {
    source: Option<&'a str>,
    contents: Vec<Option<TileContent<'a>>>,
}

impl<'a> SourceContents<'a> {
    /// What the render tiles of `source`, an id and its render source,
    /// show, from `store`; none for no source.
    fn of(
        &mut self,
        source: Option<(&'a str, &'a dyn RenderSource)>,
        store: &'a TileStore,
    ) -> &[Option<TileContent<'a>>] {
        let Some((id, source)) = source else {
            return &[];
        };
        if self.source != Some(id) {
            self.source = Some(id);
            self.contents.clear();
            source.tile_contents(store, &mut self.contents);
        }
        &self.contents
    }
}

/// What `frame_layer`, a layer of the frame, draws from its source among
/// `render_sources`: the source's render tiles with what `contents` finds
/// they show in `tile_store`, among which it draws those of `found` its
/// update found.
fn layer_source<'a, 'c>(
    frame_layer: &'a FrameLayer,
    render_sources: &'a HashMap<String, Box<dyn RenderSource>>,
    tile_store: &'a TileStore,
    found: &'a [FoundTile],
    contents: &'c mut SourceContents<'a>,
) -> Option<FrameSourceTiles<'c>> {
    let source = frame_layer.source.as_ref().and_then(|id| render_sources.get_key_value(id));
    let (id, source) = source.map(|(id, source)| (id.as_str(), &**source))?;
    Some(FrameSourceTiles::new(source.render_tiles(), contents.of(Some((id, source)), tile_store), found))
}

/// Drops `value` on `thread_pool`, the orchestrator's workers, instead of
/// this thread. The orchestrator's drop waits for it.
pub(crate) fn drop_on_workers<T: Send + 'static>(thread_pool: &TaskGroup, value: T) {
    thread_pool.spawn(move || drop(value));
}

/// A value that [`RenderOrchestrator::drop_on_workers`] drops once this
/// drops.
struct DropOnWorkers<T: Send + 'static> {
    value: Option<T>,
    thread_pool: TaskGroup,
}

impl<T: Send + 'static> DropOnWorkers<T> {
    fn new(value: T, thread_pool: TaskGroup) -> Self {
        DropOnWorkers { value: Some(value), thread_pool }
    }
}

impl<T: Send + 'static> Drop for DropOnWorkers<T> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            drop_on_workers(&self.thread_pool, value);
        }
    }
}

/// Waits for its thread pool's tasks when it drops.
struct WaitOnDrop(TaskGroup);

impl Drop for WaitOnDrop {
    fn drop(&mut self) {
        self.0.wait();
    }
}

impl Drop for RenderOrchestrator {
    fn drop(&mut self) {
        // Hosts that left and that the renderer didn't take, as when it
        // goes without its tear-down, lose their graphics objects.
        for host in std::mem::take(&mut self.ended_hosts) {
            host.context_lost();
        }
        // The render sources and their tiles go before the wait, since
        // their data drops on the thread pool.
        self.render_sources.clear();
        self.tile_store.clear();
        self.thread_pool.wait();
    }
}
