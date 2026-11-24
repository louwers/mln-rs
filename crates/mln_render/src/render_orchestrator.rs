// The orchestrator owns the render sources and the tile store their tiles
// are in, and turns what the store reports into the renderer's events.

use std::sync::Arc;
use std::time::Duration;

use mln_gfx::gpu::{Gpu, TextureFormat, TextureId};
use mln_map_types::{MapDebugOptions, MapMode, View};
use mln_style::diff::{ItemChanges, LayerChange, LayerChanges, StyleDiff, sources_by_id};
use mln_style::expression::{Dependency, GlobalStateMap};
use mln_style::layer_type::{ErasedLayerStyle, SourceKind};
use mln_style::map::zoom_history::ZoomHistory;
use mln_style::{
    GlobalStateChange, Layer, LayerList, PropertyEvaluationParameters, Source, Style, TransitionOptions,
    TransitionParameters, layer,
};
use mln_style_types::image::Impl as ImageImpl;
use mln_text::GlyphCache;
use mln_text::local_fonts::LocalGlyphs;
use mln_util::constants::DEFAULT_TRANSITION_DURATION;
use mln_util::error::Error;
use mln_util::hash::{HashMap, HashSet};
use mln_util::logging::{Event, Log};
use mln_util::pool::{Pool, TaskGroup};
use mln_util::time::Instant;
use mln_vec_map::{VecMap, VecSet};

use crate::Registry;
use crate::encode::{DrawOrder, FrameDraws};
use crate::erased_layer::ErasedEvaluatedLayer;
use crate::frame::{FrameLayer, FramePlan};
use crate::gfx::context::Resources;
use crate::globe_depth_pass::GlobeDepthPass;
use crate::image_store::ImageStore;
use crate::named_target::{Routes, Run};
use crate::paint_parameters::PaintParameters;
use crate::placement::{PlacementRecord, Placements};
use crate::render_layer::{FrameSourceTiles, LayerUpdate, RenderLayer};
use crate::render_pass::Pass;
use crate::render_source::{RenderSource, RenderTileParameters};
use crate::render_tile::{FoundTile, RenderTile, TileContent};
use crate::renderer_event::RendererEvent;
use crate::sky_pass::FullscreenDraw;
use crate::source::MapSettings;
use crate::sources::render_tile_source::{DebugLayer, DebugLayers};
use crate::text::glyph_store::GlyphStore;
use crate::tile::inbox::Waker;
use crate::tile::job::SourceKey;
use crate::tile::source_tiles::SourceTiles;
use crate::tile::store::{StoreEvent, TileStore};
use crate::tile::tile_workers::PoolWorkers;
use crate::tile::worker_codec::RemoteWorkers;
use crate::tile_parameters::TileParameters;
use crate::update_parameters::UpdateParameters;

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
fn create_render_layer(
    registry: &Registry,
    style: &Arc<dyn ErasedLayerStyle>,
    res: &mut Resources,
) -> Option<Box<dyn RenderLayer>> {
    let name = style.info().name;
    let participant = registry.placing_type(name);
    let render_layer =
        registry.layer_type(name).and_then(|layer_type| layer_type.create_render_layer(style, res, participant));
    debug_assert!(render_layer.is_some(), "the renderer's registry has no layer type '{name}'");
    if render_layer.is_none() {
        Log::error(Event::Render, format!("no render layer for layer type '{name}'"));
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
    /// The style's sources in id order, the order of the per-source pass,
    /// each with its render source: `None` for a source type that the
    /// registry lacks.
    sources: Vec<(Source, Option<RenderSource>)>,
    /// The render layers, in the style's order: a layer's index is its
    /// place in the drawing order. A layer whose type has no render layer
    /// has none.
    layers: Vec<Box<dyn RenderLayer>>,
    /// The index in `layers` of each render layer, by layer id.
    layer_indexes: HashMap<String, usize>,
    render_light: crate::render_light::RenderLight,
    render_sky: Option<crate::render_sky::RenderSky>,

    /// The participants of the registry's placing layer types, and the
    /// engine's side of their placements.
    placement: Placements,
    background_layer_as_color: bool,
    /// The center's longitude at the last update, which tells a wrap jump.
    longitude: f32,
    tile_cache_enabled: bool,

    /// Whether the next update makes the render layers again, for new
    /// shaders.
    remake_render_layers: bool,
    filtered_layers_for_source: Vec<Arc<dyn ErasedEvaluatedLayer>>,

    thread_pool: TaskGroup,

    global_state: Option<Arc<GlobalStateMap>>,

    /// The debug overlays.
    debug_layers: DebugLayers,

    /// The style's named targets, and the frame's routes into them.
    routes: Routes,

    globe_depth_pass: GlobeDepthPass,
    sky_pass: crate::sky_pass::SkyPass,

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
        RenderOrchestrator {
            placement: Placements::new(&registry),
            registry,
            events: Vec::new(),
            zoom_history: ZoomHistory::default(),
            still_image_request_id: 0,
            view: View::default(),
            tile_store,
            found_tiles: Vec::new(),
            style: None,
            sources: Vec::new(),
            layers: Vec::new(),
            layer_indexes: HashMap::default(),
            render_light: crate::render_light::RenderLight::new(Arc::default()),
            render_sky: None,
            background_layer_as_color,
            longitude: 0.0,
            tile_cache_enabled: true,
            remake_render_layers: false,
            filtered_layers_for_source: Vec::new(),
            _wait_for_thread_pool: WaitOnDrop(thread_pool.clone()),
            thread_pool,
            global_state: None,
            globe_depth_pass: GlobeDepthPass::default(),
            sky_pass: crate::sky_pass::SkyPass::default(),
            debug_layers: VecMap::new(),
            routes: Routes::default(),
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
    /// them their tiles, places what they place and brings their buckets
    /// to the placement. The encode then changes none of this: it changes
    /// only what draws it on the GPU, uploading the tiles' atlas and
    /// buckets and making the layers' draws.
    pub(crate) fn update(
        &mut self,
        update_parameters: &Arc<UpdateParameters>,
        res: &mut Resources,
    ) -> Option<FramePlan> {
        let mut frame =
            FramePlan::new(&update_parameters.view, update_parameters.camera_moving, update_parameters.debug_options);
        let mut clock = self.advance_clock(update_parameters);
        let style = &update_parameters.style;

        self.tile_store.glyphs().set_url(style.get_glyph_url());
        self.tile_store.glyphs().set_font_faces(style.get_font_faces().cloned());
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
        self.routes.sync(style.extra(), res);
        self.style = Some(Arc::clone(style));
        let layers = Arc::clone(style.get_layer_list());

        self.apply_image_diff(&image_diff, update_parameters.sprite_loaded);

        let render_layers_remade = self.apply_layer_changes(&layer_changes, &layers, &clock, res);
        let constants_mask_changed = self.evaluate_layers(&layer_changes, render_layers_remade, &mut clock);
        if sources_changed {
            self.apply_source_diff(&source_diff, style);
        }
        self.set_feature_states(update_parameters);

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

        let will_render = self.update_sources(
            update_parameters,
            res.caps().cpu_rasterizer,
            &layers,
            &layer_changes,
            &constants_mask_changed,
            &clock,
            &mut frame,
        );

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
        let longitude = update_parameters.view.center(mln_util::geo::WrapMode::Unwrapped).longitude() as f32;
        let wrap_delta = ((longitude - self.longitude) / 360.0).round() as i16;
        self.longitude = longitude;
        for (layer, will_render) in self.layers.iter_mut().zip(will_render) {
            if wrap_delta != 0 {
                layer.handle_wrap_jump(wrap_delta);
            }
            if layer.is_renderable() != will_render {
                layer.set_renderable(will_render);
            }
        }

        frame.loaded = update_parameters.style_loaded && !answered && self.is_loaded();
        if !clock.continuous && !frame.loaded {
            return None;
        }

        self.update_render_tiles(&frame);
        self.update_layer_tiles(update_parameters, &frame);

        self.placement.place(&self.layers, update_parameters, &mut self.tile_store, &mut frame);
        frame.needs_repaint = clock.continuous && self.has_transitions(update_parameters.time_point);

        if !frame.needs_repaint && frame.loaded {
            // Ask the app to remove the unused images it added once the map
            // is fully loaded and there are no ongoing transitions.
            if self.tile_store.images().exceeds_cache_size_limit() {
                let used = self.tile_store.images_used();
                self.tile_store.images_mut().reduce_memory_use(&used);
                self.events.extend(self.tile_store.take_resource_events());
            }
        }

        self.placement.update(&self.layers, update_parameters, &mut self.tile_store);

        Some(frame)
    }

    /// Brings the feature state of the enabled sources' tiles to the
    /// sources', and computes their tiles' matrices for the frame.
    fn update_render_tiles(&mut self, frame: &FramePlan) {
        for (style_source, render_source) in &mut self.sources {
            let Some(RenderSource::Tiles(render_source)) = render_source else { continue };
            if render_source.enabled {
                if let Some((tiles, _)) = self.tile_store.source_mut(render_source.key) {
                    tiles.coalesce_feature_state();
                }
                render_source.update_render_tiles(
                    &mut self.tile_store,
                    &RenderTileParameters {
                        transform: &frame.transform,
                        debug_options: frame.debug_options,
                        source_name: style_source.id(),
                    },
                );
            } else {
                render_source.release_render_data();
            }
        }
    }

    /// Gives each layer of the frame its tiles, in the order it draws them,
    /// and finds those that place.
    fn update_layer_tiles(&mut self, update_parameters: &UpdateParameters, frame: &FramePlan) {
        self.placement.clear_layers();
        let mut contents = SourceContents::default();
        self.found_tiles.clear();
        for (&index, frame_layer) in &frame.layers {
            let index = index as usize;
            let render_layer = &mut self.layers[index];
            let source = frame_layer.source.and_then(|index| Some((index, self.sources[index].1.as_ref()?)));
            let source_contents = contents.of(source, &self.tile_store);
            let source = source.map(|(_, source)| source);
            let places = render_layer.record().places;
            let start = self.found_tiles.len();
            render_layer.update(&mut LayerUpdate {
                view: &frame.transform.view,
                update_parameters,
                source,
                contents: source_contents,
                found: &mut self.found_tiles,
                places,
            });
            if let Some(participant) = render_layer.placing() {
                let found = &self.found_tiles[start..];
                self.placement.add_layer((index, participant), source, source_contents, found);
            }
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
                    self.render_sky = Some(crate::render_sky::RenderSky::new(Arc::clone(sky)));
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
    fn apply_layer_changes(
        &mut self,
        layer_changes: &LayerChanges,
        layers: &LayerList,
        clock: &FrameClock,
        res: &mut Resources,
    ) -> bool {
        // Make each render layer again from its layer, without the relayout
        // that adding the layer would take.
        let render_layers_remade = std::mem::take(&mut self.remake_render_layers);
        if render_layers_remade {
            for render_layer in &mut self.layers {
                let style = render_layer.shared_style();
                let Some(mut new_layer) = create_render_layer(&self.registry, &style, res) else {
                    continue;
                };
                new_layer.transition(&clock.added_layer_transition);
                *render_layer = new_layer;
            }
        }

        if layer_changes.order_changed {
            let mut by_id: HashMap<String, Box<dyn RenderLayer>> =
                self.layers.drain(..).map(|layer| (layer.id().to_owned(), layer)).collect();

            // Remove render layers for removed layers.
            for (id, _) in layer_changes.iter().filter(|(_, change)| change.removes()) {
                by_id.remove(&**id);
            }

            // Create render layers for newly added layers.
            for (id, _) in layer_changes.iter().filter(|(_, change)| change.adds()) {
                let Some(layer) = layers.get(id) else { continue };
                if let Some(mut render_layer) = create_render_layer(&self.registry, layer.style(), res) {
                    render_layer.transition(&clock.added_layer_transition);
                    // A layer added twice keeps its first render layer.
                    by_id.entry(id.to_string()).or_insert(render_layer);
                }
            }

            // A layer type without a render layer is skipped.
            self.layers = layers.iter().filter_map(|layer| by_id.remove(layer.get_id())).collect();
            self.layer_indexes =
                self.layers.iter().enumerate().map(|(index, layer)| (layer.id().to_owned(), index)).collect();
        }

        // Update render layers for changed layers.
        for (id, change) in layer_changes.iter() {
            if let (Some(&index), Some(layer)) = (self.layer_indexes.get(&**id), layers.get(id)) {
                let render_layer = &mut self.layers[index];
                if change.restyles() {
                    render_layer.restyle(&clock.transition, layer.style());
                } else if change == LayerChange::Data {
                    render_layer.set_data(layer.style());
                }
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
                let previous_mask = layer.evaluated().constants_mask();
                layer.evaluate(&clock.evaluation);
                if previous_mask != layer.evaluated().constants_mask() {
                    constants_mask_changed.insert(id);
                }
            }
        }
        constants_mask_changed
    }

    /// Makes and removes the render sources for the sources that changed,
    /// and puts them in the id order of `style`'s sources.
    fn apply_source_diff(&mut self, source_diff: &ItemChanges<Source>, style: &Style) {
        let mut kept: HashMap<String, RenderSource> = (self.sources.drain(..))
            .filter_map(|(source, render_source)| Some((source.id().to_owned(), render_source?)))
            .collect();
        for id in source_diff.removed.keys() {
            if let Some(key) = kept.remove(id).and_then(|source| source.tiles()) {
                self.tile_store.remove_source(key);
            }
        }
        for (id, style_source) in &source_diff.added {
            if !kept.contains_key(id)
                && let Some(render_source) = RenderSource::new(&self.registry, style_source, &mut self.tile_store)
            {
                kept.insert(id.clone(), render_source);
            }
        }
        self.sources = sources_by_id(style.get_sources())
            .into_iter()
            .map(|source| {
                let render_source = kept.remove(source.id());
                (source, render_source)
            })
            .collect();
    }

    /// Gives every source the update's feature state. The update carries
    /// the map's whole feature state, so a render source, whether new, kept
    /// or rebuilt after `clear_data`, shows the state of the update it
    /// renders. A source whose state the update didn't change does nothing.
    fn set_feature_states(&mut self, update_parameters: &UpdateParameters) {
        for (style_source, render_source) in &self.sources {
            let key = render_source.as_ref().and_then(RenderSource::tiles);
            if let Some((source, _)) = key.and_then(|key| self.tile_store.source_mut(key)) {
                source.info.feature_state.set_states(update_parameters.feature_states.get(style_source.id()));
            }
        }
    }

    /// Chooses the layers the frame draws, into `frame`, and updates each
    /// source's tiles for the layers it has: whether it needs tiles at all,
    /// and whether they lay out again. Returns whether each layer draws.
    #[expect(clippy::too_many_arguments, reason = "the update's inputs, each from another part of the update")]
    fn update_sources(
        &mut self,
        update_parameters: &UpdateParameters,
        cpu_rasterizer: bool,
        layers: &LayerList,
        layer_changes: &LayerChanges,
        constants_mask_changed: &VecSet<String>,
        clock: &FrameClock,
        frame: &mut FramePlan,
    ) -> Vec<bool> {
        let tile_parameters = TileParameters {
            map: MapSettings {
                mode: update_parameters.mode,
                pixel_ratio: update_parameters.pixel_ratio,
                max_zoom: update_parameters.max_zoom,
                file_source: update_parameters.file_source.clone(),
                fast_pfor_enabled: update_parameters.fast_pfor_enabled,
            },
            debug_options: update_parameters.debug_options,
            view: &update_parameters.view,
            min_zoom: update_parameters.min_zoom,
            prefetch_zoom_delta: update_parameters.prefetch_zoom_delta,
            tile_lod_min_radius: update_parameters.tile_lod_min_radius,
            tile_lod_scale: update_parameters.tile_lod_scale,
            tile_lod_pitch_threshold: update_parameters.tile_lod_pitch_threshold,
            tile_lod_zoom_shift: update_parameters.tile_lod_zoom_shift,
            tile_lod_mode: update_parameters.tile_lod_mode,
            subdivision_granularity: update_parameters.subdivision_granularity,
            cpu_rasterizer,
            global_state: self.global_state.clone(),
            font_faces: update_parameters.style.get_font_faces().cloned(),
            glyph_pbfs: !update_parameters.style.get_glyph_url().is_empty(),
            placement_commits: self.placement.schedule().commits(),
        };

        if !self.sources.is_empty() {
            self.filtered_layers_for_source.reserve(layers.len());
        }

        let mut will_render: Vec<bool> = vec![false; self.layers.len()];

        // Each source in turn takes its layers. The layers without a source
        // go with the first, and with no sources the loop runs once for
        // them.
        let source_count = std::cmp::max(self.sources.len(), 1);
        for source_index in 0..source_count {
            let mut source_needs_rendering = false;
            let mut source_needs_relayout = false;
            // The id of the source whose layers this pass takes, if it has a
            // render source: the layers of a source type without one are
            // skipped.
            let render_source_id = (self.sources.get(source_index))
                .and_then(|(style_source, render_source)| render_source.as_ref().map(|_| style_source.id()));

            for (index, layer) in self.layers.iter().enumerate() {
                let record = layer.record();

                if record.input != SourceKind::None {
                    // Every source's pass sees every layer, so the cheap
                    // source check goes before anything else is read.
                    if render_source_id == Some(layer.style().base().source.as_str()) {
                        let style = layer.style();
                        let layer_is_visible = record.visible;
                        let zoom_fits_layer = record.supports_zoom(self.zoom_history.last_zoom);
                        let layer_id = layer.id().to_owned();
                        // A global state change requires a relayout of
                        // this layer's source if one of the changed state
                        // properties is referenced by anything that is
                        // baked into the tile's buckets.
                        let evaluated = layer.evaluated();
                        let global_state_requires_relayout = {
                            if !clock.global_state_changed
                                || !(style.layout_dependencies() | evaluated.evaluated_dependencies())
                                    .intersects(Dependency::GLOBAL_STATE)
                            {
                                false
                            } else if clock.changed_global_state_keys.is_none() {
                                true
                            } else {
                                let mut refs = VecSet::new();
                                style.collect_layout_global_state_refs(&mut refs);
                                evaluated.collect_evaluated_global_state_refs(&mut refs);
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
                            self.filtered_layers_for_source.push(evaluated);
                            if zoom_fits_layer {
                                source_needs_rendering = true;
                                frame
                                    .layers
                                    .entry(index as u32)
                                    .or_insert_with(|| FrameLayer { source: Some(source_index) });
                                will_render[index] = true;
                            }
                        }
                    }
                    continue;
                }

                // Handle layers without source.
                let is_first_source = source_index == 0;
                if is_first_source && record.visible && record.supports_zoom(self.zoom_history.last_zoom) {
                    // On the globe the background is a sphere mesh, not the
                    // backdrop, so it stays a layer, as it does for the
                    // overdraw inspector, which counts its draw.
                    if self.background_layer_as_color
                        && !self.routes.routes(layer.style().base().extra(), record.volume)
                        && frame.sky.is_none()
                        && !update_parameters.view.is_globe()
                        && !update_parameters.debug_options.contains(MapDebugOptions::OVERDRAW)
                        && layers
                            .first()
                            .is_some_and(|first| std::ptr::addr_eq(layer.style(), Arc::as_ptr(first.style())))
                        && let Some(solid_background) = record.evaluation.solid
                    {
                        frame.background_color = solid_background;
                        continue; // This layer is shown with background color,
                        // and it shall not be added to render items.
                    }
                    frame.layers.entry(index as u32).or_insert_with(|| FrameLayer { source: None });
                    will_render[index] = true;
                }
            }
            if let Some((style_source, render_source)) = self.sources.get_mut(source_index) {
                if let Some(source) = render_source {
                    source.update(
                        &mut self.tile_store,
                        style_source,
                        &self.filtered_layers_for_source,
                        source_needs_rendering,
                        source_needs_relayout,
                        &tile_parameters,
                    );
                }
                self.filtered_layers_for_source.clear();
            }
        }
        will_render
    }

    pub(crate) fn has_transitions(&self, time_point: Option<Instant>) -> bool {
        if self.render_light.has_transition() {
            return true;
        }

        if self.render_sky.as_ref().is_some_and(crate::render_sky::RenderSky::has_transition) {
            return true;
        }

        if self.layers.iter().any(|layer| layer.has_transition()) {
            return true;
        }

        if self.placement.schedule().has_transitions(time_point) {
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

        self.sources.iter().filter_map(|(_, source)| source.as_ref()).all(|source| source.is_loaded(&self.tile_store))
    }

    pub(crate) fn clear_data(&mut self) {
        // The next update finds every layer, source and image added. The
        // global state, light and sky stay, so the next update doesn't
        // transition them again.
        self.style = None;
        self.debug_layers.clear();
        self.sources.clear();
        self.tile_store.clear();
        self.layers.clear();
        self.layer_indexes.clear();

        self.placement.forget();

        self.tile_store.images_mut().clear();
        self.tile_store.glyphs().evict(&VecSet::new());
    }

    /// Drops what holds the renderer's shaders: the sky and globe depth
    /// passes and the debug overlays now, and the render layers in the next
    /// update, which makes them again. The draws made after that use the
    /// shaders the renderer has then. The sources keep their tiles, and the
    /// tiles their layout.
    pub(crate) fn release_shaders(&mut self) {
        self.remake_render_layers = true;
        self.debug_layers.clear();
        self.sky_pass = crate::sky_pass::SkyPass::default();
        self.globe_depth_pass = GlobeDepthPass::default();
    }

    /// Makes the GPU textures of the tiles' atlas, and uploads what it
    /// queued.
    pub(crate) fn upload_tile_atlas(&mut self, gpu: &mut dyn Gpu, context: &mut Resources) {
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
                    Log::error(Event::Style, format!("Failed to load tile {tile} for source {source}: {error}"));
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

    /// The key of the tiles of source `id`, if it has any.
    fn source_key(&self, id: &str) -> Option<SourceKey> {
        let index = self.sources.binary_search_by(|(source, _)| source.id().cmp(id)).ok()?;
        self.sources[index].1.as_ref()?.tiles()
    }

    /// The tiles of source `id`, if it has any.
    fn source_tiles(&self, id: &str) -> Option<&SourceTiles> {
        self.tile_store.source(self.source_key(id)?)
    }

    fn source_tiles_mut(&mut self, id: &str) -> Option<&mut SourceTiles> {
        let key = self.source_key(id)?;
        Some(self.tile_store.source_mut(key)?.0)
    }

    /// Visits the layers' draw groups of the frame, in drawing order, each
    /// with whether it draws into a named target and whether its layer
    /// draws 3D geometry ([`Layer::VOLUME`](crate::layer::Layer::VOLUME)).
    pub(crate) fn visit_draw_groups<'a>(&'a self, mut visit: impl FnMut(&'a FrameDraws, bool, bool)) {
        for layer in &self.layers {
            let volume = layer.record().volume;
            layer.visit_draw_groups(&mut |group, routed| visit(group, routed, volume));
        }
    }

    /// Uploads the GPU objects of the frame's draws: the layers' draws into
    /// their targets and draw groups, in drawing order, the globe's depth,
    /// the sky and the atmosphere, and the debug overlays.
    pub(crate) fn upload_draws(&mut self, gpu: &mut dyn Gpu, context: &mut Resources) {
        for layer in &mut self.layers {
            layer.upload(gpu, context);
        }
        self.globe_depth_pass.upload(gpu, context);
        if let Some(sky) = self.sky_pass.sky() {
            sky.upload(gpu, context);
        }
        if let Some(atmosphere) = self.sky_pass.atmosphere() {
            atmosphere.upload(gpu, context);
        }
        for debug in self.debug_layers.values_mut() {
            debug.upload(gpu, context);
        }
    }

    /// Draws what the frame draws before its passes: the layers' draws into
    /// their own targets, bottom to top, and the passes of the named targets
    /// that they draw into or read, each where the frame's routes order it.
    /// A named target's pass draws with `frame_depth`, the frame's depth and
    /// stencil and their format, where a pass can draw into them with any
    /// color.
    pub(crate) fn render_targets(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        frame_depth: Option<(TextureId, TextureFormat)>,
    ) {
        if self.routes.order().is_empty() {
            for layer in &mut self.layers {
                layer.render_targets(gpu, parameters);
            }
            return;
        }
        for index in 0..self.routes.order().len() {
            match self.routes.order()[index] {
                Run::Own(layer) => self.layers[layer].render_targets(gpu, parameters),
                Run::Named(target) => self.render_named_target(gpu, parameters, target, frame_depth),
            }
        }
        parameters.reset_stencil();
    }

    /// Draws the pass of the named target at `target`: the draws of the
    /// layers routed into it, bottom to top, all in its translucent pass,
    /// within the frame's scissor.
    fn render_named_target(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        target: usize,
        frame_depth: Option<(TextureId, TextureFormat)>,
    ) {
        let began = match self.routes.pass(gpu, parameters.context, target, frame_depth) {
            Ok(pass) => parameters.begin_pass(gpu, pass),
            Err(error) => Err(error),
        };
        if let Err(error) = began {
            parameters.context.fail(format!("Beginning a named target's pass failed: {error}"));
            return;
        }
        parameters.reset_stencil();
        // The layers draw as above the lowest layer of 3D geometry on the
        // map: without the opaque pass's depth, each over the last.
        let opaque_pass_cutoff = std::mem::replace(&mut parameters.opaque_pass_cutoff, u32::MAX);
        parameters.pass = Pass::Translucent;
        for layer in self.routes.writers(target) {
            self.layers[layer].render(gpu, parameters, DrawOrder::BottomUp, Some(target));
        }
        parameters.opaque_pass_cutoff = opaque_pass_cutoff;
        if let Err(error) = parameters.end_pass(gpu) {
            parameters.context.fail(format!("Ending a named target's pass failed: {error}"));
        }
    }

    /// Draws the pass's draws of the layers' draw groups into the open pass
    /// of `gpu`, the context's, in `order`, each at the depth of
    /// `parameters.current_layer`, which `order` steps after each group.
    pub(crate) fn render_draw_groups(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        order: DrawOrder,
    ) {
        match order {
            DrawOrder::BottomUp => {
                for layer in &mut self.layers {
                    layer.render(gpu, parameters, order, None);
                }
            }
            DrawOrder::TopDown => {
                for layer in self.layers.iter_mut().rev() {
                    layer.render(gpu, parameters, order, None);
                }
            }
        }
    }

    /// Readies the buckets of the frame's sources for the frame's draws.
    pub(crate) fn upload_sources(&mut self, context: &mut Resources) {
        for source in self.sources.iter_mut().filter_map(|(_, source)| source.as_mut()) {
            source.upload(&mut self.tile_store, context);
        }
    }

    /// Updates the debug overlays' draws from the sources' tiles, and makes
    /// their draws of the frame.
    pub(crate) fn update_debug_layers(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        for (_, source) in &self.sources {
            if let Some(RenderSource::Tiles(source)) = source
                && source.enabled
            {
                source.update_debug_draws(&mut self.debug_layers, parameters);
            }
        }
        for debug in self.debug_layers.values_mut() {
            debug.update_draws(gpu, parameters);
        }
    }

    /// The debug overlays, in `DebugType` order.
    pub(crate) fn debug_layers(&mut self) -> impl Iterator<Item = &mut DebugLayer> {
        self.debug_layers.values_mut()
    }

    /// The layer `id`, of the type `L`, with its values.
    pub(crate) fn layer<L: crate::layer::Layer>(&self, id: &str) -> Option<(&L, &crate::layer::LayerValues<L::Style>)> {
        let layer: &dyn std::any::Any = &**self.layers.get(*self.layer_indexes.get(id)?)?;
        let hosted = layer.downcast_ref::<crate::render_layer::Hosted<L>>()?;
        Some(hosted.parts())
    }

    /// Drops every render layer, with the GPU context they were made in.
    pub(crate) fn leave_layers(&mut self) {
        self.layers.clear();
        self.layer_indexes.clear();
        self.routes.lose();
    }

    /// Makes the draws of the layers the frame draws, and of the sky and
    /// globe depth passes.
    pub(crate) fn update_layer_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        update_parameters: &UpdateParameters,
        paint_parameters: &mut PaintParameters<'_>,
        frame: &FramePlan,
    ) {
        let mut needs_globe_depth = false;
        let mut contents = SourceContents::default();
        self.routes.begin_frame(paint_parameters.frame.target_size, paint_parameters.context);
        for (&index, frame_layer) in &frame.layers {
            let index = index as usize;
            let render_layer = &mut self.layers[index];
            let source = frame_layer.source.and_then(|index| Some((index, self.sources[index].1.as_ref()?)));
            let source_contents = contents.of(source, &self.tile_store);
            let tiles = source.map(|(_, source)| FrameSourceTiles::new(source, source_contents));
            let images = self.tile_store.images();
            render_layer.update_draws(
                gpu,
                paint_parameters,
                images,
                tiles,
                &self.found_tiles,
                index,
                &update_parameters.style,
                &mut self.routes,
            );
            needs_globe_depth = needs_globe_depth || render_layer.record().volume;
        }
        self.routes.finish_frame();
        let state = paint_parameters.frame.camera;
        let (renderable_size, pixel_ratio) = (paint_parameters.frame.target_size, paint_parameters.frame.pixel_ratio);
        let context = &mut *paint_parameters.context;
        self.sky_pass.update(gpu, context, state, renderable_size, pixel_ratio, frame.sky.as_ref(), &frame.light);
        self.globe_depth_pass.update(
            gpu,
            context,
            state,
            update_parameters,
            needs_globe_depth || self.sky_pass.has_atmosphere(),
        );
    }

    /// The planar sky, which draws before the layers, if the frame draws it.
    pub(crate) fn sky(&mut self) -> Option<&mut FullscreenDraw> {
        self.sky_pass.sky()
    }

    /// The globe's atmosphere, which draws after the layers, if the frame
    /// draws it.
    pub(crate) fn atmosphere(&mut self) -> Option<&mut FullscreenDraw> {
        self.sky_pass.atmosphere()
    }

    /// The pass that draws the planet's depth.
    pub(crate) fn globe_depth(&mut self) -> &mut GlobeDepthPass {
        &mut self.globe_depth_pass
    }

    /// Queries the rendered features in `geometry`. `Err` if the query fails
    /// (a malformed tile).
    pub(crate) fn query_rendered_features(
        &mut self,
        geometry: &[mln_util::geo::ScreenCoordinate],
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, Error> {
        // A query answers from the layouts that arrived since the update.
        self.receive_tiles();
        let render_layers: Vec<&dyn RenderLayer> = match &options.layer_ids {
            Some(layer_ids) => layer_ids
                .iter()
                .filter_map(|id| self.layer_indexes.get(id))
                .map(|&index| &*self.layers[index])
                .collect(),
            None => self.layers.iter().map(|layer| &**layer).collect(),
        };
        let mut layers: HashMap<String, &dyn RenderLayer> = HashMap::default();
        for layer in render_layers {
            layers.entry(layer.id().to_owned()).or_insert(layer);
        }

        self.query_rendered_features_in(geometry, options, &layers)
    }

    fn query_rendered_features_in(
        &self,
        geometry: &[mln_util::geo::ScreenCoordinate],
        options: &crate::query::RenderedQueryOptions,
        layers: &HashMap<String, &dyn RenderLayer>,
    ) -> Result<Vec<mln_util::feature::Feature>, Error> {
        let mut source_ids: VecSet<String> = VecSet::new();
        let mut filtered_layers = crate::geometry::feature_index::QueryLayers::default();
        for (id, layer) in layers {
            let record = layer.record();
            if !record.needs_rendering() || !record.supports_zoom(self.zoom_history.last_zoom) {
                continue;
            }
            filtered_layers.insert(id.clone(), *layer);
            source_ids.insert(layer.style().base().source.clone());
        }

        let proj_matrix = self.view.projection_matrix_with(self.view.near_z(), false);

        // Traced once for every source. Symbols are found where they were
        // placed, on the screen, so they take the geometry as it is.
        let traced = crate::geometry::feature_index::query_screen_geometry(&self.view, geometry);
        let mut results_by_layer = crate::geometry::feature_index::QueryResults::default();
        for source_id in &source_ids {
            if let Some(source) = self.source_tiles(source_id) {
                let source_results = source.query_rendered_features(
                    &traced,
                    &self.view,
                    &filtered_layers,
                    options,
                    self.global_state.as_deref(),
                    &proj_matrix,
                )?;
                for (layer_id, features) in source_results {
                    results_by_layer.entry(layer_id).or_insert(features);
                }
            }
        }

        // What the placement shows of the layers that place.
        let placing: crate::geometry::feature_index::QueryLayers<'_> = filtered_layers
            .iter()
            .filter(|(id, _)| layers.get(*id).is_some_and(|layer| layer.record().places))
            .map(|(id, layer)| (id.clone(), *layer))
            .collect();
        if !placing.is_empty() {
            self.placement.placed().query_rendered_features(
                geometry,
                &placing,
                options,
                self.global_state.as_deref(),
                &mut results_by_layer,
            )?;
        }

        for (id, layer) in &filtered_layers {
            let mut hits = Vec::new();
            layer.hit_frame(&self.view, geometry, &mut hits);
            if let Some(filter) = &options.filter {
                let context = mln_style::expression::EvaluationContext::with_zoom_feature(self.view.zoom() as f32, None)
                    .with_global_state(self.global_state.as_deref());
                hits.retain(|hit| {
                    let feature = hit.feature.to_geojson_feature();
                    mln_style::expression::expr::with_geojson_feature(&feature, context.clone(), |cx| filter.evaluate(cx))
                });
            }
            if !hits.is_empty() {
                results_by_layer.entry(id.clone()).or_default().extend(hits);
            }
        }

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
            if let Some(mut hits) = results_by_layer.remove(id) {
                hits.sort_by(|a, b| a.distance.total_cmp(&b.distance));
                result.extend(hits.into_iter().map(|hit| {
                    let mut feature = hit.feature;
                    id.clone_into(&mut feature.layer);
                    feature
                }));
            }
        }

        Ok(result)
    }

    /// See [`Renderer::record_placements`](crate::Renderer::record_placements).
    pub(crate) fn record_placements(&mut self, record: bool) {
        self.placement.record(record);
    }

    /// See [`Renderer::placement_records`](crate::Renderer::placement_records).
    pub(crate) fn placement_records(&self) -> &[PlacementRecord] {
        self.placement.records()
    }

    /// Queries the features of the source `source_id`. `Err`: as
    /// [`query_rendered_features`](Self::query_rendered_features).
    pub(crate) fn query_source_features(
        &mut self,
        source_id: &str,
        options: &crate::query::SourceQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, Error> {
        self.receive_tiles();
        let Some(source) = self.source_tiles(source_id) else {
            return Ok(Vec::new());
        };

        source.query_source_features(options, self.global_state.as_deref())
    }

    pub(crate) fn dump_debug_logs(&self) {
        for (_, source) in &self.sources {
            if let Some(RenderSource::Image(source)) = source {
                source.dump_debug_logs();
            }
        }
        self.tile_store.dump_debug_logs();
    }

    pub(crate) fn set_feature_state(
        &mut self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
        state: &mln_style::expression::expr::FeatureState,
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
    ) -> mln_style::expression::expr::FeatureState {
        match self.source_tiles(source_id) {
            Some(source) => source.info.feature_state.state(source_layer_id, feature_id),
            None => mln_style::expression::expr::FeatureState::default(),
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

/// What the render tiles of a source hold, at their indexes, for the layers
/// of a phase of the frame, which go through their sources in runs.
#[derive(Default)]
struct SourceContents<'a> {
    /// The index of the source among the orchestrator's.
    source: Option<usize>,
    contents: Vec<Option<TileContent<'a>>>,
}

impl<'a> SourceContents<'a> {
    /// What the render tiles of `source`, an index among the
    /// orchestrator's sources and its render source, hold: an image source's
    /// image, or their contents in `store`; none for no source.
    fn of(&mut self, source: Option<(usize, &'a RenderSource)>, store: &'a TileStore) -> &[Option<TileContent<'a>>] {
        let Some((index, source)) = source else {
            return &[];
        };
        if self.source != Some(index) {
            self.source = Some(index);
            self.contents.clear();
            let image = source.image();
            let tiles = source.tiles().and_then(|key| store.source(key));
            let content_of = |tile: &RenderTile| match image {
                Some(image) => Some(TileContent::Image(image)),
                None => tiles?.contents_at(tile.overscaled_tile_id(), tile.slot()).map(TileContent::Tile),
            };
            self.contents.extend(source.render_tiles().iter().map(content_of));
        }
        &self.contents
    }
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
        // The render sources and their tiles go before the wait, since
        // their data drops on the thread pool.
        self.sources.clear();
        self.tile_store.clear();
        self.thread_pool.wait();
    }
}
