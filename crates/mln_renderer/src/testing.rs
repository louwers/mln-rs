//! Drivers of a source type's tiles as the renderer drives them, for the
//! tests of a crate that adds source types: [`OriginTiles`] drives tiles of
//! a [`DataOrigin`] that the test names, and [`OneSource`] a source, whose
//! tiles cover a view, through a registry. Built only with the `testing`
//! feature, and hidden from the documentation: they aren't API.

use std::sync::Arc;

use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_storage::FileSource;
use mln_style::{Layer, PropertyEvaluationParameters, Source};
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::feature::Feature;
use mln_util::pool::TaskGroup;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::Registry;
use crate::erased_layer::ErasedEvaluatedLayer;
use crate::image_store::ImageStore;
use crate::layout::tile_layout::LayoutParams;
use crate::query::SourceQueryOptions;
use crate::render_source::RenderSource;
use crate::source::{DataOrigin, TileKind, TileNecessity, TileOperation};
use crate::text::font_face_engine::default_engine;
use crate::text::glyph_store::GlyphStore;
use crate::tile::job::{SourceKey, SourceStyle};
use crate::tile::store::{StoreEvent, TileStore};
use crate::tile::tile_workers::{InlineWorkers, PoolWorkers, TileWorkers};
use crate::tile_parameters::TileParameters;

/// What a store reported of its tiles.
#[derive(Debug)]
pub enum Event {
    /// A tile changed: the renderer needs a frame.
    Changed,
    /// What a tile's load did.
    Action(TileOperation),
    /// A tile failed to load.
    Error { tile: OverscaledTileID, error: Error },
}

/// A tile store whose jobs run on `workers` and which frees and parses
/// glyphs on `pool`, with no images and no sprite loaded.
pub(crate) fn tile_store(pool: &TaskGroup, workers: Box<dyn TileWorkers>) -> TileStore {
    let glyphs = GlyphStore::new(
        mln_text::local_glyph_rasterizer::LocalGlyphs::default(),
        Arc::default(),
        pool.clone(),
        default_engine(),
        Arc::new(|| {}),
    );
    TileStore::new(workers, glyphs, ImageStore::default(), pool.clone(), Arc::new(|| {}))
}

/// One update of `store`'s tiles, as the renderer's: gives the images asked
/// for their update, takes in what arrived and starts the next jobs.
/// Returns whether a tile of a source with synchronous updates laid out.
pub(crate) fn update(store: &mut TileStore) -> bool {
    store.images_mut().begin_update();
    store.receive();
    store.drive()
}

/// What `store` reported since the last call.
fn take_events(store: &mut TileStore) -> Vec<Event> {
    store
        .take_events()
        .into_iter()
        .map(|event| match event {
            StoreEvent::Changed => Event::Changed,
            StoreEvent::Action { op, .. } => Event::Action(op),
            StoreEvent::Error { tile, error, .. } => Event::Error { tile, error },
        })
        .collect()
}

/// The style of `layers`, of the layer types of `registry`, evaluated at
/// `zoom`. Fails for a layer whose type `registry` lacks.
fn style(registry: &Registry, layers: &[Layer], zoom: f32) -> Result<Arc<SourceStyle>, Error> {
    let parameters = PropertyEvaluationParameters::new(zoom);
    let evaluated = layers
        .iter()
        .map(|layer| {
            let mut render_layer = registry
                .layer_type(layer.info().name)
                .and_then(|layer_type| layer_type.create_render_layer(layer.style()))
                .ok_or_else(|| Error::invalid_argument(format!("no render layer for {}", layer.get_id())))?;
            render_layer.evaluate(&parameters);
            Ok(render_layer.evaluated())
        })
        .collect::<Result<Vec<Arc<dyn ErasedEvaluatedLayer>>, Error>>()?;
    Ok(Arc::new(SourceStyle { version: 0, layers: evaluated, global_state: None, font_faces: None, glyph_pbfs: true }))
}

/// Tiles of one source, whose data comes from an origin, driven as the
/// renderer drives them, and laid out on this thread. The test decides
/// which tiles there are and how much each is needed, as a cover would. A
/// change to a tile that isn't there changes nothing, and it is neither
/// complete nor renderable.
pub struct OriginTiles {
    store: TileStore,
    key: SourceKey,
    params: LayoutParams,
}

impl OriginTiles {
    /// Tiles `ids` of a source of `kind` whose data comes from `origin`, in
    /// a store that frees on `pool`, laid out for `mode`; `single_layer`
    /// for a source whose data has one layer. Their loads start as a cover
    /// starts them. A tile that `origin` makes no load of is left out.
    pub fn new(
        pool: &TaskGroup,
        kind: TileKind,
        single_layer: bool,
        origin: impl DataOrigin,
        ids: &[OverscaledTileID],
        mode: MapMode,
    ) -> Self {
        let mut store = tile_store(pool, Box::new(InlineWorkers));
        let key = store.add_source("source".to_owned(), kind, single_layer, Box::new(origin));
        let params = LayoutParams {
            mode,
            pixel_ratio: 1.0,
            show_collision_boxes: false,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            first_load: true,
        };
        let mut tiles = OriginTiles { store, key, params };
        if let Some((source, ctx)) = tiles.store.source_mut(key) {
            for id in ids {
                let Some(tile) = source.info.new_tile(*id, params) else {
                    continue;
                };
                source.insert(tile);
                source.update(id, ctx, |tile| {
                    tile.start_load();
                });
            }
        }
        tiles
    }

    /// Gives every tile the style of `layers`, of the layer types of
    /// `registry`, evaluated at zoom 0.
    ///
    /// # Errors
    ///
    /// When `registry` lacks the type of one of `layers`.
    pub fn set_layers(&mut self, registry: &Registry, layers: &[Layer]) -> Result<(), Error> {
        let style = style(registry, layers, 0.0)?;
        let (pixel_ratio, granularity) = (self.params.pixel_ratio, self.params.subdivision_granularity);
        if let Some((source, ctx)) = self.store.source_mut(self.key) {
            let ids: Vec<OverscaledTileID> = source.slots().map(|slot| slot.tile.id).collect();
            for id in ids {
                source.update(&id, ctx, |tile| tile.set_style(&style, pixel_ratio, granularity));
            }
        }
        Ok(())
    }

    /// Sets how much tile `id` is needed.
    pub fn set_necessity(&mut self, id: &OverscaledTileID, necessity: TileNecessity) {
        if let Some((source, ctx)) = self.store.source_mut(self.key) {
            source.update(id, ctx, |tile| tile.set_necessity(necessity));
        }
    }

    /// Moves tile `from` to id `to`, as a wrap jump does.
    pub fn rekey(&mut self, from: &OverscaledTileID, to: OverscaledTileID) {
        if let Some((source, ctx)) = self.store.source_mut(self.key) {
            source.rekey(from, to, ctx);
        }
    }

    /// Lets go of tile `id`, as its source does when it leaves the cache.
    pub fn drop_tile(&mut self, id: &OverscaledTileID) {
        if let Some((source, ctx)) = self.store.source_mut(self.key) {
            source.release(id, ctx);
        }
    }

    /// Takes in what arrived for the tiles.
    pub fn receive(&mut self) {
        self.store.receive();
    }

    /// One update of the tiles, as the renderer's.
    pub fn update(&mut self) {
        update(&mut self.store);
    }

    /// The engine retries the required tiles' failed loads.
    pub fn retry_failed(&mut self) {
        self.store.retry_failed();
    }

    /// What the store reported since the last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        take_events(&mut self.store)
    }

    /// Whether tile `id` has done what it can to load and lay out.
    #[must_use]
    pub fn is_complete(&self, id: &OverscaledTileID) -> bool {
        self.tile(id).is_some_and(crate::tile::tile::Tile::is_complete)
    }

    /// Whether tile `id` can draw, if only some of its layers.
    #[must_use]
    pub fn is_renderable(&self, id: &OverscaledTileID) -> bool {
        self.tile(id).is_some_and(crate::tile::tile::Tile::is_renderable)
    }

    /// The features of `source_layer` that tile `id`'s layout shows; none
    /// before it shows one.
    ///
    /// # Errors
    ///
    /// When the query fails, as for data that doesn't decode.
    pub fn features(&self, id: &OverscaledTileID, source_layer: &str) -> Result<Vec<Feature>, Error> {
        let mut features = Vec::new();
        let (Some(source), Some(tile)) = (self.store.source(self.key), self.tile(id)) else {
            return Ok(features);
        };
        if let Some(contents) = tile.contents().and_then(|contents| contents.geometry()) {
            let options = SourceQueryOptions { source_layers: Some(vec![source_layer.to_owned()]), filter: None };
            contents.query_source_features(id, source.info.single_layer, &mut features, &options, None)?;
        }
        Ok(features)
    }

    fn tile(&self, id: &OverscaledTileID) -> Option<&crate::tile::tile::Tile> {
        self.store.source(self.key).and_then(|source| source.tile(id))
    }
}

/// What a [`OneSource`] updates for: the map's view, mode and file source.
pub struct View<'a> {
    pub state: &'a TransformState,
    pub mode: MapMode,
    pub file_source: Option<Arc<dyn FileSource>>,
    /// Whether the source's tiles lay out anew, as at the style's first
    /// update.
    pub needs_relayout: bool,
}

/// A source of a type that a registry has, whose tiles cover the view it
/// updates for, as the renderer's sources' do, and lay out on a pool.
pub struct OneSource {
    store: TileStore,
    render_source: Box<dyn RenderSource>,
    layers: Vec<Arc<dyn ErasedEvaluatedLayer>>,
}

impl OneSource {
    /// The render source of `source`, of a type that `registry` has, drawn
    /// by `layers`, whose jobs and frees run on `pool`.
    ///
    /// # Errors
    ///
    /// When `registry` lacks the type of `source` or of one of `layers`.
    pub fn new(registry: &Registry, source: &Source, layers: &[Layer], pool: &TaskGroup) -> Result<Self, Error> {
        let mut store = tile_store(pool, Box::new(PoolWorkers::new(pool.clone(), 8)));
        let render_source = crate::render_source::create(registry, source, &mut store)
            .ok_or_else(|| Error::invalid_argument(format!("no render source for {}", source.id())))?;
        let layers = style(registry, layers, 0.0)?.layers.clone();
        Ok(OneSource { store, render_source, layers })
    }

    /// Updates the render source with `source`, a version of its source, for
    /// `view`, then starts its tiles' jobs, as a frame does.
    pub fn update(&mut self, source: &Source, view: &View<'_>) {
        let parameters = TileParameters {
            pixel_ratio: 1.0,
            debug_options: MapDebugOptions::NO_DEBUG,
            view: view.state.view(),
            min_zoom: view.state.min_zoom(),
            max_zoom: view.state.limits().max_zoom,
            file_source: view.file_source.clone(),
            mode: view.mode,
            prefetch_zoom_delta: 0,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            global_state: None,
            is_update_synchronous: source.data().is_update_synchronous(),
            font_faces: None,
            glyph_pbfs: true,
            placement_commits: 0,
        };
        let (store, layers) = (&mut self.store, &self.layers);
        self.render_source.update(store, source.clone(), layers, true, view.needs_relayout, &parameters);
        store.drive();
    }

    /// One update of the tiles between frames: takes in what arrived and
    /// starts the next jobs.
    pub fn step(&mut self) {
        update(&mut self.store);
    }

    /// Whether every tile the view needs has loaded.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.render_source.is_loaded(&self.store)
    }

    /// What the store reported since the last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        take_events(&mut self.store)
    }
}
