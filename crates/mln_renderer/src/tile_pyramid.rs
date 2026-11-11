// Ports src/mln/renderer/tile_pyramid.hpp and src/mln/renderer/tile_pyramid.cpp.
//
// `renderedTiles` (`std::map<UnwrappedTileID, std::reference_wrapper<Tile>>`)
// maps to the key of the tile in `tiles`: every rendered tile is one of the
// pyramid's tiles.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use mln_actor::TaggedScheduler;
use mln_map_types::{MapDebugOptions, MapMode, TileLodMode};
use mln_style::layer::layer_type_info;
use mln_style::layer_properties::LayerProperties;
use mln_style::source;
use mln_style_types::SourceType;
use mln_tile_id::tile_range::TileRange;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::geo::{LatLngBounds, WrapMode};
use mln_util::immutable::Immutable;
use mln_util::logging::{Event, Log};
use mln_util::range::Range;

use crate::algorithm::update_renderables::{PrefetchedTile, RenderablesContext, update_renderables};
use crate::tile::tile::{Tile, TileNecessity, TileUpdateParameters};
use crate::tile::tile_cache::TileCache;
use crate::tile::tile_observer::{NullTileObserver, TileObserver};
use crate::tile_parameters::TileParameters;
use crate::util::tile_cover::{TileCoverParameters, covering_zoom_level, tile_cover};

/// `util::clamp<double>(value, min, max)`.
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    let lower = if value < max { value } else { max };
    if min < lower { lower } else { min }
}

/// `TilePyramid`.
pub struct TilePyramid {
    tiles: BTreeMap<OverscaledTileID, Box<dyn Tile>>,
    cache: TileCache,

    /// Sorted by tile id.
    rendered_tiles: BTreeMap<UnwrappedTileID, OverscaledTileID>,
    observer: Rc<dyn TileObserver>,

    prev_lng: f32,

    fading_tiles: bool,
    cache_enabled: bool,
}

/// The `TilePyramid::update` callbacks of `algorithm::updateRenderables`.
struct UpdateContext<'p, 'a, F>
where
    F: FnMut(&OverscaledTileID, Rc<dyn TileObserver>) -> Option<Box<dyn Tile>>,
{
    tiles: &'p mut BTreeMap<OverscaledTileID, Box<dyn Tile>>,
    cache: &'p mut TileCache,
    rendered_tiles: &'p mut BTreeMap<UnwrappedTileID, OverscaledTileID>,
    previously_rendered_tiles: &'p mut BTreeMap<UnwrappedTileID, OverscaledTileID>,
    retain: &'p mut BTreeSet<OverscaledTileID>,
    observer: &'p Rc<dyn TileObserver>,
    layers: &'p [Immutable<dyn LayerProperties>],
    parameters: &'p TileParameters<'a>,
    tile_range: Option<TileRange>,
    needs_relayout: bool,
    minimum_update_interval: mln_util::chrono::Duration,
    is_volatile: bool,
    create_tile: F,
    /// Whether `renderTile` marks tiles and updates `previouslyRenderedTiles`
    /// (the ideal pass; the pan pass passes a no-op).
    render: bool,
}

impl<F> RenderablesContext for UpdateContext<'_, '_, F>
where
    F: FnMut(&OverscaledTileID, Rc<dyn TileObserver>) -> Option<Box<dyn Tile>>,
{
    fn get_tile(&mut self, id: &OverscaledTileID) -> Option<OverscaledTileID> {
        self.tiles.contains_key(id).then_some(*id)
    }

    fn create_tile(&mut self, id: &OverscaledTileID) -> Option<OverscaledTileID> {
        if let Some(tile_range) = &self.tile_range {
            if !tile_range.contains(&id.canonical) {
                return None;
            }
        }
        let tile = match self.cache.pop(id) {
            Some(tile) => tile,
            None => {
                let tile = (self.create_tile)(id, self.observer.clone())?;
                tile.set_layers(self.layers, self.parameters.global_state.clone());
                tile
            }
        };

        // `tiles.emplace(tileID, std::move(tile))` keeps an existing tile.
        self.tiles.entry(*id).or_insert(tile);
        Some(*id)
    }

    fn retain_tile(&mut self, key: &OverscaledTileID, necessity: TileNecessity) {
        let tile = &self.tiles[key];
        if self.retain.insert(tile.id()) {
            tile.set_update_parameters(&TileUpdateParameters {
                minimum_update_interval: self.minimum_update_interval,
                is_volatile: self.is_volatile,
            });
            tile.set_necessity(necessity);
        }

        if self.needs_relayout {
            tile.set_layers(self.layers, self.parameters.global_state.clone());
        }
    }

    fn render_tile(&mut self, id: &UnwrappedTileID, key: &OverscaledTileID) {
        if !self.render {
            return;
        }
        // addRenderTile(tileID, tile)
        debug_assert!(self.tiles[key].is_renderable());
        self.rendered_tiles.entry(*id).or_insert(*key);
        // Still rendering this tile, no need for special fading logic.
        self.previously_rendered_tiles.remove(id);
        self.tiles[key].mark_rendered_ideal();
    }

    fn prefetched_tiles(&self) -> Vec<PrefetchedTile> {
        if !self.render {
            // The pan pass: `emptyPrefetchedTiles`.
            return Vec::new();
        }
        // The ideal pass: `tiles`.
        self.tiles.keys().map(|id| (*id, *id)).collect()
    }

    fn is_renderable(&self, key: &OverscaledTileID) -> bool {
        self.tiles[key].is_renderable()
    }

    fn has_tried_cache(&self, key: &OverscaledTileID) -> bool {
        self.tiles[key].has_tried_cache()
    }

    fn is_loaded(&self, key: &OverscaledTileID) -> bool {
        self.tiles[key].is_loaded()
    }
}

impl TilePyramid {
    /// `TilePyramid(const TaggedScheduler&)`.
    pub fn new(thread_pool: TaggedScheduler) -> Self {
        TilePyramid {
            tiles: BTreeMap::new(),
            cache: TileCache::new(thread_pool, 0),
            rendered_tiles: BTreeMap::new(),
            observer: Rc::new(NullTileObserver),
            prev_lng: 0.0,
            fading_tiles: false,
            cache_enabled: true,
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.tiles.values().all(|tile| tile.is_complete())
    }

    /// `update(layers, needsRendering, needsRelayout, parameters, sourceImpl,
    /// tileSize, zoomRange, bounds, createTile)`.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        layers: &[Immutable<dyn LayerProperties>],
        needs_rendering: bool,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
        source_impl: &dyn source::Impl,
        tile_size: u16,
        zoom_range: Range<u8>,
        bounds: Option<LatLngBounds>,
        create_tile: impl FnMut(&OverscaledTileID, Rc<dyn TileObserver>) -> Option<Box<dyn Tile>>,
    ) {
        // If we need a relayout, abandon any cached tiles; they're now stale.
        if needs_relayout {
            self.cache.clear();
        }

        // If we're not going to render anything, move our existing tiles
        // into the cache (if they're not stale) or abandon them, and return.
        if !needs_rendering {
            let tiles = std::mem::take(&mut self.tiles);
            for (id, tile) in tiles {
                if !needs_relayout {
                    // These tiles are invisible, we set optional necessity
                    // for them and thus suppress network requests on tiles
                    // expiration (see `OnlineFileRequest`). One that hasn't
                    // loaded then stops loading, and is released, as in C++.
                    tile.set_necessity(TileNecessity::Optional);
                    if tile.is_renderable() {
                        self.cache.add(id, tile);
                    } else {
                        self.cache.deferred_release(tile);
                    }
                } else {
                    self.cache.deferred_release(tile);
                }
            }

            self.rendered_tiles.clear();
            self.cache.defer_pending_releases();

            return;
        }

        self.handle_wrap_jump(parameters.transform_state.get_lat_lng(WrapMode::Unwrapped).longitude() as f32);

        // Optionally shift the zoom level
        let zoom = clamp(
            parameters.transform_state.get_zoom() + parameters.tile_lod_zoom_shift,
            parameters.transform_state.get_min_zoom(),
            parameters.transform_state.get_max_zoom(),
        );

        let type_ = source_impl.type_;
        // Determine the overzooming/underzooming amounts and required tiles.
        let overscaled_zoom: i32 = covering_zoom_level(zoom, type_, tile_size);
        let mut tile_zoom: i32 = overscaled_zoom;
        let mut pan_zoom: i32 = i32::from(zoom_range.max);

        let source_prefetch_zoom_delta = source_impl.get_prefetch_zoom_delta();
        let max_parent_tile_overscale_factor = source_impl.get_max_overscale_factor_for_parent_tiles();
        let minimum_update_interval = source_impl.get_minimum_tile_update_interval();
        let is_volatile = source_impl.is_volatile();

        let mut ideal_tiles: Vec<OverscaledTileID> = Vec::new();
        let mut pan_tiles: Vec<OverscaledTileID> = Vec::new();

        let tile_cover_parameters = TileCoverParameters {
            transform_state: parameters.transform_state.clone(),
            tile_lod_min_radius: parameters.tile_lod_min_radius,
            tile_lod_scale: parameters.tile_lod_scale,
            tile_lod_pitch_threshold: parameters.tile_lod_pitch_threshold,
            tile_lod_mode: parameters.tile_lod_mode,
        };

        if overscaled_zoom >= i32::from(zoom_range.min) {
            let ideal_zoom: i32 = std::cmp::min(i32::from(zoom_range.max), overscaled_zoom);

            // Make sure we're not reparsing overzoomed raster tiles.
            if type_ == SourceType::Raster {
                tile_zoom = ideal_zoom;
            }

            // Only attempt prefetching in continuous mode.
            if parameters.mode == MapMode::Continuous
                && type_ != SourceType::GeoJSON
                && type_ != SourceType::Annotations
            {
                // Request lower zoom level tiles (if configured to do so) in
                // an attempt to show something on the screen faster at the
                // cost of a little of bandwidth.
                let prefetch_zoom_delta: u8 = source_prefetch_zoom_delta.unwrap_or(parameters.prefetch_zoom_delta);
                if prefetch_zoom_delta != 0 {
                    pan_zoom = std::cmp::max(tile_zoom - i32::from(prefetch_zoom_delta), i32::from(zoom_range.min));
                }

                if pan_zoom < ideal_zoom {
                    pan_tiles = tile_cover(&tile_cover_parameters, pan_zoom as u8, zoom_range, None);
                }
            }

            ideal_tiles = tile_cover(&tile_cover_parameters, ideal_zoom as u8, zoom_range, Some(tile_zoom as u8));
            if parameters.mode == MapMode::Tile
                && type_ != SourceType::Raster
                && type_ != SourceType::RasterDEM
                && ideal_tiles.len() > 1
            {
                Log::warning(
                    Event::General,
                    format!(
                        "Provided camera options returned {} tiles, only {} is taken in Tile mode.",
                        ideal_tiles.len(),
                        ideal_tiles[0]
                    ),
                );
                ideal_tiles = vec![ideal_tiles[0]];
            }
        }

        // Stores a list of all the tiles that we're definitely going to
        // retain. There are two kinds of tiles we need: the ideal tiles
        // determined by the tile cover. They may not yet be in use because
        // they're still loading. In addition to that, we also need to retain
        // all tiles that we're actively using, e.g. as a replacement for
        // tile that aren't loaded yet.
        let mut retain: BTreeSet<OverscaledTileID> = BTreeSet::new();

        // The min and max zoom for TileRange are based on the
        // updateRenderables algorithm. Tiles are created at the ideal tile
        // zoom or at lower zoom levels. Child tiles are used from the cache,
        // but not created.
        let mut tile_range: Option<TileRange> = None;
        if let Some(bounds) = &bounds {
            let max_zoom: i32 = if parameters.tile_lod_mode == TileLodMode::Distance {
                i32::from(zoom_range.max)
            } else {
                std::cmp::min(tile_zoom, i32::from(zoom_range.max))
            };
            tile_range = Some(TileRange::from_lat_lng_bounds_range(bounds, zoom_range.min, max_zoom as u8));
        }

        let mut previously_rendered_tiles = std::mem::take(&mut self.rendered_tiles);

        self.rendered_tiles.clear();

        let mut ctx = UpdateContext {
            tiles: &mut self.tiles,
            cache: &mut self.cache,
            rendered_tiles: &mut self.rendered_tiles,
            previously_rendered_tiles: &mut previously_rendered_tiles,
            retain: &mut retain,
            observer: &self.observer,
            layers,
            parameters,
            tile_range,
            needs_relayout,
            minimum_update_interval,
            is_volatile,
            create_tile,
            render: false,
        };

        if !pan_tiles.is_empty() {
            // `[](const UnwrappedTileID&, Tile&) {}` and no prefetched tiles.
            ctx.render = false;
            update_renderables(&mut ctx, &pan_tiles, &zoom_range, max_parent_tile_overscale_factor);
        }

        ctx.render = true;
        update_renderables(&mut ctx, &ideal_tiles, &zoom_range, max_parent_tile_overscale_factor);

        let previously_rendered: Vec<(UnwrappedTileID, OverscaledTileID)> =
            ctx.previously_rendered_tiles.iter().map(|(id, key)| (*id, *key)).collect();
        for (unwrapped, key) in previously_rendered {
            let Some(tile) = ctx.tiles.get(&key) else {
                continue;
            };
            tile.mark_rendered_previously();
            if tile.hold_for_fade() {
                // Since it was rendered in the last frame, we know we have it
                // Don't mark the tile "Required" to avoid triggering a new
                // network request
                ctx.retain_tile(&key, TileNecessity::Optional);
                // addRenderTile(previouslyRenderedTile.first, tile)
                debug_assert!(ctx.tiles[&key].is_renderable());
                ctx.rendered_tiles.entry(unwrapped).or_insert(key);
            }
        }
        drop(ctx);

        if type_ != SourceType::Annotations && self.cache_enabled {
            let size = parameters.transform_state.get_size();
            let conservative_cache_size = (f64::max(f64::from(size.width) / f64::from(tile_size), 1.0)
                * f64::max(f64::from(size.height) / f64::from(tile_size), 1.0)
                * (parameters.transform_state.get_max_zoom() - parameters.transform_state.get_min_zoom() + 1.0)
                * 0.5) as usize;
            self.cache.set_size(conservative_cache_size);
        } else {
            self.cache.set_size(0);
        }

        // Remove stale tiles. This goes through the (sorted!) tiles map and
        // retain set in lockstep and removes items from tiles that don't
        // have the corresponding key in the retain set.
        let stale: Vec<OverscaledTileID> = self.tiles.keys().filter(|key| !retain.contains(key)).copied().collect();
        for key in stale {
            // Remove the tile from the map. If it requires re-layout,
            // discard it asynchronously, otherwise keep it in the cache
            if let Some(tile) = self.tiles.remove(&key) {
                if needs_relayout {
                    self.cache.deferred_release(tile);
                } else {
                    // Port-only: a tile still loading keeps its necessity,
                    // so that it goes on loading in the cache (C++ releases
                    // it, and optional, it would stop loading). A view that
                    // turns back and forth needs the tiles at its corners
                    // for moments too short to load them each time.
                    if tile.is_renderable() {
                        tile.set_necessity(TileNecessity::Optional);
                    }
                    self.cache.add(key, tile);
                }
            }
        }

        for tile in self.tiles.values() {
            tile.set_show_collision_boxes(parameters.debug_options.contains(MapDebugOptions::COLLISION));
        }

        // Initialize renderable tiles and update the contained layer render
        // data.
        for key in self.rendered_tiles.values() {
            let tile = &self.tiles[key];
            debug_assert!(tile.is_renderable());
            tile.base().used_by_rendered_layers.set(false);

            let hold_for_fade = tile.hold_for_fade();
            for layer_properties in layers {
                let type_info = layer_properties.base_impl.get_type_info();
                if hold_for_fade && type_info.fading_tiles == layer_type_info::FadingTiles::NotRequired {
                    continue;
                }
                let updated = tile.layer_properties_updated(layer_properties);
                let used = tile.base().used_by_rendered_layers.get();
                tile.base().used_by_rendered_layers.set(used | updated);
            }
        }

        self.cache.defer_pending_releases();
    }

    /// `getRenderedTiles()`: the rendered tiles, sorted by tile id.
    pub fn get_rendered_tiles(&self) -> impl Iterator<Item = (&UnwrappedTileID, &dyn Tile)> + '_ {
        self.rendered_tiles.iter().map(|(id, key)| (id, self.tiles[key].as_ref()))
    }

    pub fn rendered_tile_count(&self) -> usize {
        self.rendered_tiles.len()
    }

    /// `getTile(tileID)`: a pyramid tile or a cached one.
    pub fn get_tile(&self, tile_id: &OverscaledTileID) -> Option<&dyn Tile> {
        match self.tiles.get(tile_id) {
            Some(tile) => Some(tile.as_ref()),
            None => self.cache.get(tile_id),
        }
    }

    // query
    /// `queryRenderedFeatures(geometry, transformState, layers, options,
    /// globalState, projMatrix, featureState)`.
    #[allow(clippy::too_many_arguments)]
    pub fn query_rendered_features(
        &self,
        geometry: &[mln_util::geo::ScreenCoordinate],
        transform_state: &mln_map_types::TransformState,
        layers: &crate::geometry::feature_index::QueryLayers<'_>,
        options: &crate::query::RenderedQueryOptions,
        global_state: Option<&mln_style::expression::GlobalStateMap>,
        proj_matrix: &mln_util::mat4::Mat4,
        feature_state: &crate::source_state::SourceFeatureState,
    ) -> Result<crate::geometry::feature_index::QueryResults, mln_util::exception::ExceptionPtr> {
        use mln_map_types::tile_coordinate::TileCoordinate;
        use mln_util::constants;

        let mut result = crate::geometry::feature_index::QueryResults::new();
        if self.rendered_tiles.is_empty() || geometry.is_empty() {
            return Ok(result);
        }

        let height = f64::from(transform_state.get_size().height);
        let query_geometry: Vec<geometry::Point<f64>> = geometry
            .iter()
            .map(|p| {
                TileCoordinate::from_screen_coordinate(
                    transform_state,
                    0,
                    &mln_util::geo::ScreenCoordinate::new(p.x, height - p.y),
                )
                .p
            })
            .collect();

        // mapbox::geometry::envelope(queryGeometry)
        let mut box_min = query_geometry[0];
        let mut box_max = query_geometry[0];
        for p in &query_geometry {
            box_min.x = if p.x < box_min.x { p.x } else { box_min.x };
            box_min.y = if p.y < box_min.y { p.y } else { box_min.y };
            box_max.x = if box_max.x < p.x { p.x } else { box_max.x };
            box_max.y = if box_max.y < p.y { p.y } else { box_max.y };
        }

        // Sorted by (z, y, wrap, x).
        let mut sorted_tiles: Vec<(&UnwrappedTileID, &OverscaledTileID)> = self.rendered_tiles.iter().collect();
        sorted_tiles.sort_by(|(a, _), (b, _)| {
            (a.canonical.z, a.canonical.y, a.wrap, a.canonical.x).cmp(&(
                b.canonical.z,
                b.canonical.y,
                b.wrap,
                b.canonical.x,
            ))
        });

        let max_pitch_scale_factor = transform_state.max_pitch_scale_factor();

        for (id, key) in sorted_tiles {
            let tile = self.tiles[key].as_ref();

            // equivalent to std::pow(2, transformState.getZoom() - id.canonical.z);
            let scale = (transform_state.get_scale() / f64::from(1u32 << id.canonical.z)) as f32;
            let query_padding =
                f64::from(max_pitch_scale_factor * tile.get_query_padding(layers) * constants::EXTENT as f32)
                    / constants::TILE_SIZE_D
                    / f64::from(scale);

            let tile_space_bounds_min = TileCoordinate::to_geometry_coordinate(id, &box_min);
            if f64::from(tile_space_bounds_min.x) - query_padding >= f64::from(constants::EXTENT)
                || f64::from(tile_space_bounds_min.y) - query_padding >= f64::from(constants::EXTENT)
            {
                continue;
            }

            let tile_space_bounds_max = TileCoordinate::to_geometry_coordinate(id, &box_max);
            if f64::from(tile_space_bounds_max.x) + query_padding < 0.0
                || f64::from(tile_space_bounds_max.y) + query_padding < 0.0
            {
                continue;
            }

            let tile_space_query_geometry: mln_tile_id::geometry_tile_data::GeometryCoordinates =
                query_geometry.iter().map(|c| TileCoordinate::to_geometry_coordinate(id, c)).collect();

            tile.query_rendered_features(
                &mut result,
                &tile_space_query_geometry,
                transform_state,
                layers,
                options,
                global_state,
                proj_matrix,
                feature_state,
            )?;
        }

        Ok(result)
    }

    /// `querySourceFeatures(options, globalState)`.
    pub fn query_source_features(
        &self,
        options: &crate::query::SourceQueryOptions,
        global_state: Option<&mln_style::expression::GlobalStateMap>,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::exception::ExceptionPtr> {
        let mut result = Vec::new();

        for tile in self.tiles.values() {
            tile.query_source_features(&mut result, options, global_state)?;
        }

        Ok(result)
    }

    /// `getRenderedTile(tileID)`.
    pub fn get_rendered_tile(&self, tile_id: &UnwrappedTileID) -> Option<&dyn Tile> {
        self.rendered_tiles.get(tile_id).map(|key| self.tiles[key].as_ref())
    }

    /// `handleWrapJump(lng)`.
    ///
    /// On top of the regular z/x/y values, TileIDs have a `wrap` value that
    /// specify which copy of the world the tile belongs to. For example, at
    /// `lng: 10` you might render z/x/y/0 while at `lng: 370` you would
    /// render z/x/y/1.
    ///
    /// When lng values get wrapped (going from `lng: 370` to `long: 10`) you
    /// expect to see the same thing on the screen (370 degrees and 10
    /// degrees is the same place in the world) but all the TileIDs will have
    /// different wrap values.
    ///
    /// In order to make this transition seamless, we calculate the rounded
    /// difference of "worlds" between the last frame and the current frame.
    /// If the map panned by a world, then we can assign all the tiles new
    /// TileIDs with updated wrap values. For example, assign z/x/y/1 a new
    /// id: z/x/y/0. It is the same tile, just rendered in a different
    /// position.
    ///
    /// This enables us to reuse the tiles at more ideal locations and
    /// prevent flickering.
    pub fn handle_wrap_jump(&mut self, lng: f32) {
        let lng_difference = lng - self.prev_lng;
        let world_difference = lng_difference / 360.0f32;
        let wrap_delta = world_difference.round() as i16;
        self.prev_lng = lng;

        if wrap_delta != 0 {
            let mut new_tiles: BTreeMap<OverscaledTileID, Box<dyn Tile>> = BTreeMap::new();
            let mut key_map: BTreeMap<OverscaledTileID, OverscaledTileID> = BTreeMap::new();
            for (old_key, tile) in std::mem::take(&mut self.tiles) {
                let old_id = tile.id();
                let new_id = old_id.unwrap_to(old_id.wrap.wrapping_add(wrap_delta));
                tile.base().set_id(new_id);
                key_map.insert(old_key, new_id);
                // `emplace` keeps the first tile with an id.
                new_tiles.entry(new_id).or_insert(tile);
            }
            self.tiles = new_tiles;

            let mut new_render_tiles: BTreeMap<UnwrappedTileID, OverscaledTileID> = BTreeMap::new();
            for (id, key) in std::mem::take(&mut self.rendered_tiles) {
                let new_id = id.unwrap_to(id.wrap.wrapping_add(wrap_delta));
                let new_key = key_map.get(&key).copied().unwrap_or(key);
                new_render_tiles.entry(new_id).or_insert(new_key);
            }
            self.rendered_tiles = new_render_tiles;
        }
    }

    pub fn set_cache_enabled(&mut self, enable: bool) {
        self.cache_enabled = enable;
    }

    pub fn reduce_memory_use(&mut self) {
        self.cache.clear();
    }

    /// `retryFailedTiles()`.
    pub fn retry_failed_tiles(&self) {
        for tile in self.tiles.values() {
            tile.retry_failed_load();
        }
    }

    /// `setObserver(TileObserver*)`.
    pub fn set_observer(&mut self, observer: Rc<dyn TileObserver>) {
        self.observer = observer;
    }

    pub fn dump_debug_logs(&self) {
        for tile in self.tiles.values() {
            tile.dump_debug_logs();
        }
    }

    /// `getTiles()`.
    pub fn get_tiles(&self) -> &BTreeMap<OverscaledTileID, Box<dyn Tile>> {
        &self.tiles
    }

    pub fn clear_all(&mut self) {
        self.fading_tiles = false;
        self.tiles.clear();
        self.rendered_tiles.clear();
        self.cache.clear();
        // C++ drops the cache's released tiles on the thread pool later.
        self.cache.defer_pending_releases();
    }

    pub fn update_fading_tiles(&mut self) {
        self.fading_tiles = false;
        for key in self.rendered_tiles.values() {
            let tile = &self.tiles[key];
            if tile.hold_for_fade() {
                self.fading_tiles = true;
                tile.performed_fade_placement();
            }
        }
    }

    pub fn has_fading_tiles(&self) -> bool {
        self.fading_tiles
    }

    /// The tile cache, for tests.
    pub fn cache(&self) -> &TileCache {
        &self.cache
    }
}
