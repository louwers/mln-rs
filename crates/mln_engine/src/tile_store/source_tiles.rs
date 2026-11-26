//! A source's tiles, in its cover or its cache, and which of them each
//! update renders. `update_cover` is the pyramid update: the ideal and
//! prefetched covers of the view, the tiles that stand in for those that
//! haven't loaded, the cache, wrap jumps and fade holds.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use mln_map_types::{MapMode, TileLodMode, VerticalPerspectiveProjection};
use mln_style::geo::{LatLngBounds, WrapMode};
use mln_style::hash::HashSet;
use mln_style::property::value::PropertyValue;
use mln_style::range::Range;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::{Source, constants};
use mln_tile_id::tile_range::TileRange;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::projection::Projection;
use mln_vec_map::VecMap;
use smallvec::SmallVec;

use crate::draw::context::Context;
use crate::draw::dynamic_texture_atlas::DynamicTextureAtlas;
use crate::layer::bucket::UploadNeeds;
use crate::layer::bucket_geometry::BucketGeometry;
use crate::layer::layer_drawing::ChangedBuckets;
use crate::layer::layer_type::ErasedEvaluatedLayer;
use crate::layer::tile_layout::LayoutParams;
use crate::log_target;
use crate::tile_store::contents::Contents;
use crate::tile_store::dem;
use crate::tile_store::store::{SourceInfo, StoreContext};
use crate::tile_store::tile::{Tile, TileKind, TileLoadOrder, TileNecessity, TileUpdateParameters};
use crate::tile_store::tile_cover::{
    TileCoverParameters, covering_zoom_level, globe_level_cover, terrain_tile_cover, tile_cover,
};
use crate::tile_store::tile_mut::{TileMut, WorldCopy, retire};
use crate::tile_store::tile_parameters::TileParameters;
use crate::tile_store::update_renderables::{PrefetchedTile, RenderablesContext, update_renderables};
use crate::tile_store::update_tile_masks::update_tile_masks;
use crate::worker::job::SourceStyle;

/// How many times the tiles in view the cache keeps, at least.
const VIEWS_CACHED: usize = 3;

/// The coarsest zoom a source can start at and still have its whole first
/// level kept loaded on the globe: four tiles, no more than a view's prefetch
/// cover holds.
const GLOBE_BASE_LEVEL_MAX_ZOOM: u8 = 1;

/// The largest planet, as its radius over the viewport's longer side, whose
/// base level is kept loaded. Up to here that side spans about 29 degrees of
/// arc or more, so a fling across a few view widths reaches ground that no
/// tile loaded for the view covers.
const GLOBE_BASE_LEVEL_MAX_RADIUS: f64 = 2.0;

pub(crate) struct CoverUpdate<'a, 'p> {
    /// The source's visible layers, in style order.
    pub(crate) layers: &'a [Arc<dyn ErasedEvaluatedLayer>],
    pub(crate) needs_rendering: bool,
    pub(crate) needs_relayout: bool,
    /// The DEM tiles that the terrain reads, of the terrain's source: they
    /// are kept loaded, with or without a layer, and render nothing.
    pub(crate) terrain_reads: &'a [OverscaledTileID],
    pub(crate) parameters: &'a TileParameters<'p>,
    pub(crate) style_source: &'a Source,
    pub(crate) tile_size: u16,
    pub(crate) zoom_range: Range<u8>,
    pub(crate) bounds: Option<LatLngBounds>,
}

pub(crate) struct Slot {
    pub(crate) tile: Tile,
    place: Place,
}

impl Slot {
    pub(crate) fn place(&self) -> Place {
        self.place
    }

    /// Moves the tile to `place`, whose order its requests take.
    fn set_place(&mut self, place: Place) {
        self.place = place;
        self.tile.load.set_order(place.order());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// `order` is `UNWANTED` until the cover retains the tile.
    Cover { order: TileLoadOrder },
    /// The cache lets go of the lowest `since` first.
    Cached { since: u64 },
}

impl Place {
    /// A cached tile's requests and jobs come after the cover's.
    pub(crate) fn order(self) -> TileLoadOrder {
        match self {
            Place::Cover { order } => order,
            Place::Cached { .. } => TileLoadOrder::UNWANTED,
        }
    }

    pub(crate) fn is_cached(self) -> bool {
        matches!(self, Place::Cached { .. })
    }
}

/// How many placements an entry the view leaves stays rendered for: the
/// first fades its symbols out and the second ends the fade.
const PLACEMENTS_HELD_FOR_FADE: u64 = 2;

/// A tile an update renders under one unwrapped id.
#[derive(Clone, Copy, Debug)]
struct Rendered {
    key: OverscaledTileID,
    /// For an entry the view left, held so that its symbols fade out, how
    /// many placements had committed when it left.
    held_since: Option<u64>,
    /// Whether the tile draws a visible layer here.
    needs_rendering: bool,
}

impl Rendered {
    fn ideal(key: OverscaledTileID) -> Self {
        Rendered { key, held_since: None, needs_rendering: false }
    }

    fn is_held(&self) -> bool {
        self.held_since.is_some()
    }
}

pub(crate) struct RenderedTile<'a> {
    pub(crate) id: &'a UnwrappedTileID,
    pub(crate) tile: &'a Tile,
    /// Where the tile is among the source's tiles (`contents_at`).
    pub(crate) slot: usize,
    /// The entry is no longer ideal, and stays so that its symbols fade out.
    pub(crate) hold_for_fade: bool,
}

pub(crate) struct SourceTiles {
    pub(crate) info: SourceInfo,
    tiles: VecMap<OverscaledTileID, Slot>,
    /// Stamps the tiles that enter the cache.
    cache_clock: u64,
    cache_size: usize,
    rendered: VecMap<UnwrappedTileID, Rendered>,
    /// The raster or raster-dem tiles the last frame drew masked, in order,
    /// whose quads go when they stop drawing, and the list before it, whose
    /// memory the next frame's takes.
    masked: Vec<OverscaledTileID>,
    masked_before: Vec<OverscaledTileID>,
    prev_lng: f32,
    cache_enabled: bool,
    last_subdivision_granularity: SubdivisionGranularitySetting,
    /// The highest `symbol-height-offset` seen since the tiles were last
    /// cleared, in meters. It doesn't fall when the tile that held it
    /// unloads, or the tile could not come back while its symbols show.
    max_content_elevation: f32,
    style: Option<Arc<SourceStyle>>,
    /// The last update's ideal cover, inside the source's bounds, for the
    /// benchmarks' measure of what a frame shows (`crate::bench`).
    #[cfg(feature = "bench")]
    pub(crate) ideal: Vec<OverscaledTileID>,
}

/// Whether every tile of the source's minimum zoom is kept loaded: on the
/// globe, for a source whose first level is a few tiles, while the planet is
/// small enough in the view that it soon turns to its far side, and while the
/// search for a loaded parent reaches that level from the tiles of the view.
fn keeps_globe_base_level(
    parameters: &TileParameters<'_>,
    zoom_range: Range<u8>,
    tile_zoom: i32,
    max_parent_overscale_factor: Option<u8>,
) -> bool {
    let state = parameters.view;
    if !state.is_globe() || zoom_range.min > GLOBE_BASE_LEVEL_MAX_ZOOM {
        return false;
    }
    if max_parent_overscale_factor.is_some_and(|factor| tile_zoom - i32::from(zoom_range.min) > i32::from(factor)) {
        return false;
    }
    let size = state.size();
    let radius = VerticalPerspectiveProjection::globe_radius_pixels(
        Projection::world_size(state.scale()),
        state.center(WrapMode::Unwrapped).latitude(),
    );
    radius <= GLOBE_BASE_LEVEL_MAX_RADIUS * f64::from(std::cmp::max(size.width, size.height))
}

/// The tiles a view wants: its ideal and prefetched tiles.
struct WantedTiles(HashSet<(u8, u8, u32, u32)>);

impl WantedTiles {
    fn new<'a>(ids: impl Iterator<Item = &'a OverscaledTileID>) -> Self {
        WantedTiles(ids.map(|id| (id.overscaled_z, id.canonical.z, id.canonical.x, id.canonical.y)).collect())
    }

    /// Whether `id` is a wanted tile or next to one at its zoom level, in any
    /// world copy. A view that turns or swings back and forth shows such
    /// tiles at its edges for moments at a time.
    fn is_near(&self, id: &OverscaledTileID) -> bool {
        let tiles = 1i64 << id.canonical.z;
        let (x, y) = (i64::from(id.canonical.x), i64::from(id.canonical.y));
        (y - 1..=y + 1).filter(|y| (0..tiles).contains(y)).any(|ny| {
            (x - 1..=x + 1).any(|nx| {
                let nx = nx.rem_euclid(tiles);
                self.0.contains(&(id.overscaled_z, id.canonical.z, nx as u32, ny as u32))
            })
        })
    }
}

struct CoverContext<'s, 'a, 'p> {
    source: &'s mut SourceTiles,
    ctx: &'s mut StoreContext,
    previously_rendered: &'s mut VecMap<UnwrappedTileID, Rendered>,
    retain: &'s mut BTreeSet<OverscaledTileID>,
    style: &'s Arc<SourceStyle>,
    update: &'s CoverUpdate<'a, 'p>,
    /// The tiles the passes cover: the coarse cover's, the terrain's reads
    /// and the view's.
    pass_tiles: &'s HashSet<OverscaledTileID>,
    tile_range: Option<TileRange>,
    needs_relayout: bool,
    minimum_update_interval: std::time::Duration,
    is_volatile: bool,
    /// The ideal pass renders; the pan pass doesn't.
    render: bool,
    /// The tiles the pass has retained, nearest the view's centre first,
    /// each followed by the parents that stand in for it.
    pass_rank: usize,
}

impl CoverContext<'_, '_, '_> {
    /// Whether a tile lookup that misses looks under the neighbouring wraps:
    /// on the full globe only, as a flat map places a tile by its wrap.
    fn finds_tiles_across_wraps(&self) -> bool {
        self.update.parameters.view.projection_transition() >= 1.0
    }

    fn tile(&self, key: &OverscaledTileID) -> Option<&Tile> {
        self.source.tile(key)
    }

    fn in_cover(&self, key: &OverscaledTileID) -> bool {
        self.source.tiles.get(key).is_some_and(|slot| !slot.place.is_cached())
    }
}

impl RenderablesContext for CoverContext<'_, '_, '_> {
    fn existing_tile(&mut self, id: &OverscaledTileID) -> Option<OverscaledTileID> {
        if self.in_cover(id) {
            return Some(*id);
        }
        // On the globe every wrap of a tile is the same piece of the sphere,
        // and the cover gives the tiles either side of the antimeridian
        // different wraps: a tile there finds its parent under the wrap of
        // the tiles on the other side. The cover's wraps are at most two
        // apart.
        if self.finds_tiles_across_wraps() {
            return [-1, 1, -2, 2]
                .into_iter()
                .map(|delta| id.unwrap_to(id.wrap.wrapping_add(delta)))
                .find(|other| self.in_cover(other));
        }
        None
    }

    fn create_tile(&mut self, id: &OverscaledTileID) -> Option<OverscaledTileID> {
        if let Some(tile_range) = &self.tile_range
            && !tile_range.contains(&id.canonical)
        {
            return None;
        }
        let parameters = self.update.parameters;
        let mut found = self.source.cache_pop(id, self.ctx);
        if !found && parameters.view.is_globe() {
            found = self.source.cache_pop_any_wrap(id, self.ctx);
        }
        // A tile cached before the pixel ratio changed lays out again as
        // it is retained.
        if found {
            return Some(*id);
        }
        let params = LayoutParams {
            mode: parameters.mode,
            pixel_ratio: parameters.pixel_ratio,
            subdivision_granularity: parameters.subdivision_granularity,
            first_load: true,
        };
        let tile = self.source.info.new_tile(*id, params)?;
        // A flat map draws each world copy of a tile from a tile of its own,
        // which starts from a loaded copy rather than leave its world bare
        // until it has loaded and laid out the same data.
        let copy = if parameters.view.is_globe() { None } else { self.source.world_copy(id) };
        self.source.insert(tile);
        let style = self.style;
        self.source.update(id, self.ctx, |tile| {
            match copy {
                Some(copy) => tile.start_as_copy(copy),
                None => tile.start_load(),
            }
            tile.set_style(style, params.pixel_ratio, params.subdivision_granularity);
        });
        Some(*id)
    }

    fn retain_tile(&mut self, key: &OverscaledTileID, necessity: TileNecessity) {
        if self.tile(key).is_none() {
            return;
        }
        let first = self.retain.insert(*key);
        // A tile that a pass covers is required, also where a pass reaches
        // it first as a fallback, such as a view's tile that stands in for
        // the coarse tile above it until that loads: its necessity is set
        // only once a pass, and an optional one would stop it loading and
        // drop the request that keeps its data fresh.
        let necessity = if self.pass_tiles.contains(key) { TileNecessity::Required } else { necessity };
        if first {
            // The pan pass comes first, so a tile of both covers loads with
            // the coarse ones.
            let order =
                if self.render { TileLoadOrder::ideal(self.pass_rank) } else { TileLoadOrder::coarse(self.pass_rank) };
            self.pass_rank += 1;
            if let Some(slot) = self.source.tiles.get_mut(key).filter(|slot| !slot.place.is_cached()) {
                slot.set_place(Place::Cover { order });
            }
        }
        let update_parameters = TileUpdateParameters {
            minimum_update_interval: self.minimum_update_interval,
            is_volatile: self.is_volatile,
        };
        let parameters = self.update.parameters;
        let (style, needs_relayout) = (self.style, self.needs_relayout);
        self.source.update(key, self.ctx, |tile| {
            if first {
                tile.set_update_parameters(update_parameters);
                tile.set_necessity(necessity);
            }
            // A new pixel ratio lays the tile out again, without the
            // relayout that drops the cached tiles.
            let pixel_ratio_changed =
                tile.tile().pipeline.layout().is_some_and(|layout| layout.params.pixel_ratio != parameters.pixel_ratio);
            if needs_relayout || pixel_ratio_changed {
                tile.set_style(style, parameters.pixel_ratio, parameters.subdivision_granularity);
            }
        });
    }

    fn render_tile(&mut self, id: &UnwrappedTileID, key: &OverscaledTileID) {
        if !self.render {
            return;
        }
        // A globe tile found under another wrap is drawn once, under its own.
        let id = if self.finds_tiles_across_wraps() { &key.to_unwrapped() } else { id };
        debug_assert!(self.tile(key).is_some_and(Tile::is_renderable));
        self.source.rendered.entry(*id).or_insert(Rendered::ideal(*key));
        // Still rendering this tile, no need for special fading logic.
        self.previously_rendered.remove(id);
    }

    fn prefetched_tiles(&self) -> Vec<PrefetchedTile> {
        if !self.render {
            // The pan pass prefetches none.
            return Vec::new();
        }
        // The ideal pass: the cover's tiles.
        self.source.cover().map(|tile| (tile.id, tile.id)).collect()
    }

    fn is_renderable(&self, key: &OverscaledTileID) -> bool {
        self.tile(key).is_some_and(Tile::is_renderable)
    }

    fn has_tried_cache(&self, key: &OverscaledTileID) -> bool {
        self.tile(key).is_some_and(|tile| tile.load.has_tried_cache())
    }

    fn is_loaded(&self, key: &OverscaledTileID) -> bool {
        self.tile(key).is_some_and(Tile::is_settled)
    }
}

impl SourceTiles {
    pub(crate) fn new(info: SourceInfo) -> Self {
        SourceTiles {
            info,
            tiles: VecMap::default(),
            cache_clock: 0,
            cache_size: 0,
            rendered: VecMap::default(),
            masked: Vec::new(),
            masked_before: Vec::new(),
            prev_lng: 0.0,
            cache_enabled: true,
            last_subdivision_granularity: SubdivisionGranularitySetting::none(),
            max_content_elevation: 0.0,
            style: None,
            #[cfg(feature = "bench")]
            ideal: Vec::new(),
        }
    }

    pub(crate) fn tile(&self, id: &OverscaledTileID) -> Option<&Tile> {
        self.tiles.get(id).map(|slot| &slot.tile)
    }

    /// What tile `id` shows, which is at `slot` of the tiles if they haven't
    /// changed since `rendered` said so.
    pub(crate) fn contents_at(&self, id: &OverscaledTileID, slot: Option<usize>) -> Option<&Contents> {
        match slot.and_then(|slot| self.tiles.get_index(slot)) {
            Some((key, slot)) if key == id => slot.tile.contents(),
            _ => self.tile(id)?.contents(),
        }
    }

    /// What tile `id` shows, whose buckets the frame changes, see
    /// `contents_at`.
    pub(crate) fn contents_at_mut(&mut self, id: &OverscaledTileID, slot: Option<usize>) -> Option<&mut Contents> {
        let index = self.index_at(id, slot)?;
        self.tiles.get_index_mut(index)?.1.tile.contents_mut()
    }

    /// The index of tile `id` among the tiles, which is `slot` if they
    /// haven't changed since `rendered` said so.
    fn index_at(&self, id: &OverscaledTileID, slot: Option<usize>) -> Option<usize> {
        match slot.filter(|&slot| self.tiles.get_index(slot).is_some_and(|(key, _)| key == id)) {
            Some(slot) => Some(slot),
            None => self.tiles.index_of(id),
        }
    }

    /// Readies the buckets of what tile `id` shows, see `contents_at_mut`,
    /// for the frame's draws, which read what `needs` says, finding its
    /// atlas pages' textures in `atlas`. A geometry tile's buckets share
    /// what those of its other world copies, copies of them or they of
    /// its, moved into `context`'s stores; and the copies laid out from the
    /// same data then take what it moved, freeing their own.
    pub(crate) fn upload_at(
        &mut self,
        id: &OverscaledTileID,
        slot: Option<usize>,
        atlas: &DynamicTextureAtlas,
        context: &mut Context,
        needs: UploadNeeds,
    ) {
        let Some(index) = self.index_at(id, slot) else {
            return;
        };
        let Some((&id, slot)) = self.tiles.get_index_mut(index) else {
            return;
        };
        let feature_index = match slot.tile.contents_mut() {
            Some(Contents::Geometry(geometry)) => {
                geometry.textures.resolve(atlas);
                if !geometry.needs_upload() {
                    return;
                }
                Arc::clone(&geometry.result.feature_index)
            }
            Some(Contents::Raster(_) | Contents::Dem(_) | Contents::Empty) | None => return,
        };
        let mut same: SmallVec<[usize; 4]> = self.same_place(&id);
        same.retain(|&mut at| at != index);
        let slots = self.tiles.values_mut().into_slice();
        upload_sharing(slots, index, &same, context, needs);
        for &copy in &same {
            let laid_out_alike =
                slots.get(copy).and_then(|slot| slot.tile.contents()?.geometry()).is_some_and(|geometry| {
                    geometry.needs_upload() && Arc::ptr_eq(&geometry.result.feature_index, &feature_index)
                });
            if laid_out_alike {
                let others: SmallVec<[usize; 4]> =
                    same.iter().copied().chain([index]).filter(|&other| other != copy).collect();
                upload_sharing(slots, copy, &others, context, needs);
            }
        }
    }

    /// The indexes of the tiles that are `id` in any world copy, `id`'s own
    /// among them if there is one. The tiles are in order of zoom, then
    /// world copy, then place, so each world copy's tile is a search away.
    fn same_place(&self, id: &OverscaledTileID) -> SmallVec<[usize; 4]> {
        let keys = self.tiles.keys().as_slice();
        let mut found = SmallVec::new();
        let mut at = keys.partition_point(|key| key.overscaled_z < id.overscaled_z);
        while let Some(&OverscaledTileID { overscaled_z, wrap, .. }) = keys.get(at) {
            if overscaled_z != id.overscaled_z {
                break;
            }
            let rest = keys.get(at..).unwrap_or_default();
            let end = at + rest.partition_point(|key| key.overscaled_z == overscaled_z && key.wrap == wrap);
            let place = OverscaledTileID { wrap, ..*id };
            if let Ok(offset) = keys.get(at..end).unwrap_or_default().binary_search(&place) {
                found.push(at + offset);
            }
            at = end;
        }
        found
    }

    pub(crate) fn slots(&self) -> impl Iterator<Item = &Slot> + '_ {
        self.tiles.values()
    }

    pub(crate) fn cover(&self) -> impl Iterator<Item = &Tile> + '_ {
        self.tiles.values().filter(|slot| !slot.place.is_cached()).map(|slot| &slot.tile)
    }

    pub(crate) fn insert(&mut self, tile: Tile) {
        tile.load.set_order(TileLoadOrder::UNWANTED);
        self.tiles.insert(tile.id, Slot { tile, place: Place::Cover { order: TileLoadOrder::UNWANTED } });
    }

    #[must_use]
    pub(crate) fn is_loaded(&self) -> bool {
        self.cover().all(Tile::is_complete)
    }

    /// Changes each tile whose slot `filter` picks through `f`, as `update`.
    pub(crate) fn update_each(
        &mut self,
        store: &mut StoreContext,
        filter: impl Fn(&Slot) -> bool,
        mut f: impl FnMut(&mut TileMut<'_>),
    ) {
        let SourceTiles { info, tiles, .. } = self;
        let mut installed = Vec::new();
        for slot in tiles.values_mut().filter(|slot| filter(slot)) {
            if TileMut::with(info, &mut slot.tile, store, &mut f).1 {
                installed.push(slot.tile.id);
            }
        }
        for id in installed {
            self.contents_installed(&id);
        }
    }

    /// Changes tile `id` through `f`: the only way to change its load, its
    /// pipeline and what it shows. A DEM tile that shows new contents trades
    /// borders with its neighbours.
    pub(crate) fn update<R>(
        &mut self,
        id: &OverscaledTileID,
        store: &mut StoreContext,
        f: impl FnOnce(&mut TileMut<'_>) -> R,
    ) -> Option<R> {
        let SourceTiles { info, tiles, .. } = self;
        let (result, installed) = TileMut::with(info, &mut tiles.get_mut(id)?.tile, store, f);
        if installed {
            self.contents_installed(id);
        }
        Some(result)
    }

    fn contents_installed(&mut self, id: &OverscaledTileID) {
        let dem = matches!(self.info.kind, TileKind::Dem(_));
        if !dem || !self.tile(id).is_some_and(Tile::is_renderable) {
            return;
        }
        // Out of its slot while it trades borders with the neighbours'.
        let Some(mut shown) = self.tiles.get_mut(id).and_then(|slot| slot.tile.shown.take()) else {
            return;
        };
        for (offset, neighbor) in dem::neighbors(id) {
            if let Some(border) = self.tiles.get_mut(&neighbor).and_then(|slot| slot.tile.shown.as_mut()) {
                dem::backfill_pair(&mut shown.contents, &mut border.contents, offset);
            }
        }
        if let Some(slot) = self.tiles.get_mut(id) {
            slot.tile.shown = Some(shown);
        }
    }

    pub(crate) fn update_cover(&mut self, update: &CoverUpdate<'_, '_>, ctx: &mut StoreContext) {
        let parameters = update.parameters;
        let subdivision_granularity = parameters.subdivision_granularity;
        let relayout = update.needs_relayout || subdivision_granularity != self.last_subdivision_granularity;
        self.last_subdivision_granularity = subdivision_granularity;
        self.info.synchronous = parameters.is_update_synchronous;

        if relayout {
            self.cache_clear(ctx);
            self.max_content_elevation = 0.0;
        }

        // Nothing renders or reads them: the cover's tiles go to the cache,
        // or go.
        if !update.needs_rendering && update.terrain_reads.is_empty() {
            let cover: Vec<OverscaledTileID> = self.cover().map(|tile| tile.id).collect();
            for id in cover {
                if relayout {
                    self.release(&id, ctx);
                    continue;
                }
                // Optional, so that their expiry requests nothing; one that
                // hasn't loaded stops loading, and goes.
                self.set_necessity(&id, ctx, TileNecessity::Optional);
                if self.tile(&id).is_some_and(Tile::is_renderable) {
                    self.cache_add(id, ctx);
                } else {
                    self.release(&id, ctx);
                }
            }
            // So are the tiles that went on loading in the cache.
            self.cache_release_loading(ctx, |_| false);
            self.rendered.clear();
            #[cfg(feature = "bench")]
            self.ideal.clear();
            if self.info.kind != TileKind::Geometry {
                std::mem::swap(&mut self.masked, &mut self.masked_before);
                self.masked.clear();
                self.unmask_stopped();
            }
            return;
        }

        self.handle_wrap_jump(parameters.view.center(WrapMode::Unwrapped).longitude() as f32, ctx);

        let zoom = mln_style::math::clamp_f64(
            parameters.view.zoom() + parameters.tile_lod_zoom_shift,
            parameters.min_zoom,
            parameters.max_zoom,
        );

        let style_source = update.style_source;
        let zoom_range = update.zoom_range;
        let tile_size = update.tile_size;
        let info = style_source.info();
        let overscaled_zoom: i32 = covering_zoom_level(zoom, info.round_zoom, tile_size);
        let mut tile_zoom: i32 = overscaled_zoom.max(0);
        let mut pan_zoom: i32 = i32::from(zoom_range.max);

        let source_prefetch_zoom_delta = style_source.common().prefetch_zoom_delta;
        let max_parent_tile_overscale_factor = style_source.common().max_overscale_factor;
        let minimum_update_interval = style_source.common().minimum_tile_update_interval;
        let is_volatile = style_source.common().volatile;

        let mut ideal_tiles: Vec<OverscaledTileID> = Vec::new();
        let mut pan_tiles: Vec<OverscaledTileID> = Vec::new();

        let tile_cover_parameters = TileCoverParameters {
            view: parameters.view.clone(),
            tile_lod_min_radius: parameters.tile_lod_min_radius,
            tile_lod_scale: parameters.tile_lod_scale,
            tile_lod_pitch_threshold: parameters.tile_lod_pitch_threshold,
            tile_lod_mode: parameters.tile_lod_mode,
            requested_zoom: Some(zoom + (constants::TILE_SIZE_D / f64::from(tile_size)).log2()),
            round_zoom: info.round_zoom,
            max_content_elevation: f64::from(self.update_max_content_elevation(update.layers)),
        };

        // While terrain draws, every source covers the view as the terrain
        // does, with the tiles under raised ground, and only a source that
        // cuts overscaled tiles loads them at a deeper zoom.
        let cover = |parameters: &TileCoverParameters, z: u8, overscaled_z: Option<u8>| match update.parameters.terrain
        {
            Some(terrain) => {
                terrain_tile_cover(parameters, z, zoom_range, overscaled_z.filter(|_| info.overscale), &terrain.ranges)
            }
            None => tile_cover(parameters, z, zoom_range, overscaled_z),
        };

        // A source that only the terrain reads covers nothing of the view.
        // Under terrain, tiles near the camera can be finer than the nominal
        // zoom, and the cover skips those below the source's lowest zoom one
        // by one.
        let covers_zoom = overscaled_zoom >= i32::from(zoom_range.min) || parameters.terrain.is_some();
        if update.needs_rendering && covers_zoom {
            let ideal_zoom: i32 = std::cmp::min(i32::from(zoom_range.max), tile_zoom);

            // Past the highest zoom level, draw that level's tiles larger.
            if !info.overscale {
                tile_zoom = ideal_zoom;
            }

            if parameters.mode == MapMode::Continuous && info.prefetch {
                // Lower zoom levels show something sooner, for some bandwidth.
                let prefetch_zoom_delta: u8 = source_prefetch_zoom_delta.unwrap_or(parameters.prefetch_zoom_delta);
                if prefetch_zoom_delta != 0 {
                    pan_zoom = std::cmp::max(tile_zoom - i32::from(prefetch_zoom_delta), i32::from(zoom_range.min));
                }

                if pan_zoom < ideal_zoom {
                    // The prefetch's level of detail falls off from its own,
                    // coarser zoom.
                    let mut pan_parameters = tile_cover_parameters.clone();
                    pan_parameters.requested_zoom = None;
                    pan_parameters.round_zoom = false;
                    pan_tiles = cover(&pan_parameters, pan_zoom as u8, None);
                }
            }

            ideal_tiles = cover(&tile_cover_parameters, ideal_zoom as u8, Some(tile_zoom as u8));

            // The globe cover leaves out the far side of the planet, so a
            // view turned there would find no parent to draw until its tiles
            // load. The source's whole first level stays loaded with the
            // prefetched tiles, ahead of them, and is drawn only where the
            // search for a loaded parent reaches it.
            if parameters.mode == MapMode::Continuous
                && info.prefetch
                && keeps_globe_base_level(parameters, zoom_range, tile_zoom, max_parent_tile_overscale_factor)
            {
                let mut base_tiles =
                    globe_level_cover(&tile_cover_parameters, zoom_range.min, zoom_range, Some(tile_zoom as u8));
                pan_tiles.retain(|id| !base_tiles.contains(id));
                base_tiles.append(&mut pan_tiles);
                pan_tiles = base_tiles;
            }
            if parameters.mode == MapMode::Tile && !info.round_zoom && ideal_tiles.len() > 1 {
                log::warn!(
                    target: log_target::GENERAL,
                    "Provided camera options returned {} tiles, only {} is taken in Tile mode.",
                    ideal_tiles.len(),
                    ideal_tiles[0]
                );
                ideal_tiles = vec![ideal_tiles[0]];
            }
        }

        // The ideal tiles, which may still be loading, and the tiles that
        // stand in for them.
        let mut retain: BTreeSet<OverscaledTileID> = BTreeSet::new();

        // `update_renderables` creates tiles at the ideal zoom or lower, and
        // only uses children from the cache. A cover that picks each tile's
        // zoom by its distance from the camera, as the globe's does, asks for
        // tiles finer than the nominal zoom, up to the source's maximum.
        let mut tile_range: Option<TileRange> = None;
        if let Some(bounds) = &update.bounds {
            let variable_zoom = parameters.tile_lod_mode == TileLodMode::Distance
                || parameters.view.is_globe()
                || parameters.terrain.is_some();
            let max_zoom: i32 = if variable_zoom {
                i32::from(zoom_range.max)
            } else {
                std::cmp::min(tile_zoom, i32::from(zoom_range.max))
            };
            tile_range = Some(TileRange::from_lat_lng_bounds_range(bounds, zoom_range.min, max_zoom as u8));
        }

        #[cfg(feature = "bench")]
        {
            self.ideal.clear();
            let inside = |id: &&OverscaledTileID| tile_range.as_ref().is_none_or(|range| range.contains(&id.canonical));
            self.ideal.extend(ideal_tiles.iter().filter(inside));
        }

        // A tile that failed in the cache goes before the cover could take it
        // back: there it would wait for a retry that may never come.
        self.cache_release_loading(ctx, |_| true);

        if parameters.view.is_globe() {
            self.move_tiles_to_wanted_wraps(&ideal_tiles, &pan_tiles, ctx);
        }

        let mut previously_rendered = std::mem::take(&mut self.rendered);

        let style = self.source_style(update.layers, parameters);
        let pass_tiles: HashSet<OverscaledTileID> =
            pan_tiles.iter().chain(update.terrain_reads).chain(&ideal_tiles).copied().collect();
        {
            let mut cover = CoverContext {
                source: self,
                ctx,
                previously_rendered: &mut previously_rendered,
                retain: &mut retain,
                style: &style,
                update,
                pass_tiles: &pass_tiles,
                tile_range,
                needs_relayout: relayout,
                minimum_update_interval,
                is_volatile,
                render: false,
                pass_rank: 0,
            };

            if !pan_tiles.is_empty() {
                // `[](const UnwrappedTileID&, Tile&) {}` and no prefetched tiles.
                cover.render = false;
                update_renderables(&mut cover, &pan_tiles, zoom_range, max_parent_tile_overscale_factor);
            }

            // The terrain's DEM tiles load with the coarse ones, and stand in
            // for each other as the view's do, but render nothing.
            if !update.terrain_reads.is_empty() {
                cover.render = false;
                update_renderables(&mut cover, update.terrain_reads, zoom_range, max_parent_tile_overscale_factor);
            }

            cover.render = true;
            cover.pass_rank = 0;
            update_renderables(&mut cover, &ideal_tiles, zoom_range, max_parent_tile_overscale_factor);

            // An entry the view left stays, held, until enough placements
            // have committed since it left.
            let commits = parameters.placement_commits;
            let previously: Vec<(UnwrappedTileID, Rendered)> =
                cover.previously_rendered.iter().map(|(id, entry)| (*id, *entry)).collect();
            for (unwrapped, entry) in previously {
                let key = entry.key;
                let Some(tile) = cover.tile(&key) else {
                    continue;
                };
                let held_since = entry.held_since.unwrap_or(commits);
                if tile.fades_out() && commits < held_since + PLACEMENTS_HELD_FOR_FADE {
                    let entry = Rendered { held_since: Some(held_since), needs_rendering: false, ..entry };
                    // Optional, so that it requests nothing.
                    cover.retain_tile(&key, TileNecessity::Optional);
                    debug_assert!(cover.is_renderable(&key));
                    cover.source.rendered.entry(unwrapped).or_insert(entry);
                }
            }
        }

        if self.cache_enabled {
            let size = parameters.view.size();
            let conservative_cache_size = (f64::max(f64::from(size.width) / f64::from(tile_size), 1.0)
                * f64::max(f64::from(size.height) / f64::from(tile_size), 1.0)
                * (parameters.max_zoom - parameters.min_zoom + 1.0)
                * 0.5) as usize;
            // A tilted view holds more tiles than its size suggests, and as it
            // turns it passes over more again.
            let view_cache_size = (ideal_tiles.len() + update.terrain_reads.len()) * VIEWS_CACHED;
            self.cache_set_size(conservative_cache_size.max(view_cache_size), ctx);
        } else {
            self.cache_set_size(0, ctx);
        }

        // A tile that leaves the view while it loads goes on loading in the
        // cache while it is next to the view's tiles: a view that turns back
        // and forth needs the tiles at its corners for moments too short to
        // load them each time. A tile the view has left further behind, such
        // as one of a zoom level that a fast zoom passed, stops loading, since
        // it would hold up the requests and workers the view's tiles need. A
        // tile whose load failed isn't loading: it goes, and is requested
        // anew if the view comes back, rather than wait out a backoff.
        let wanted = WantedTiles::new(ideal_tiles.iter().chain(&pan_tiles).chain(update.terrain_reads));
        self.cache_release_loading(ctx, |id| wanted.is_near(id));

        // The cover's tiles that weren't retained go to the cache, or go.
        let stale: Vec<OverscaledTileID> = self.cover().map(|tile| tile.id).filter(|id| !retain.contains(id)).collect();
        for id in stale {
            let Some(tile) = self.tile(&id) else {
                continue;
            };
            let renderable = tile.is_renderable();
            let loading_near =
                tile.load.necessity == TileNecessity::Required && !tile.load.has_failed() && wanted.is_near(&id);
            if relayout {
                self.release(&id, ctx);
            } else if renderable {
                self.set_necessity(&id, ctx, TileNecessity::Optional);
                self.cache_add(id, ctx);
            } else if loading_near {
                // Still loading, near the view: it keeps its necessity and
                // goes on loading in the cache.
                self.cache_add(id, ctx);
            } else {
                // A fallback that looked only in the cache, and found nothing
                // there, loads nothing: kept, it would only push the tiles
                // that the view needed out of the cache. A tile still loading
                // far from the view stops loading, and so does one that failed.
                self.set_necessity(&id, ctx, TileNecessity::Optional);
                self.release(&id, ctx);
            }
        }

        let SourceTiles { tiles, rendered, .. } = self;
        for entry in rendered.values_mut() {
            entry.needs_rendering = false;
            let Some(Slot { tile, .. }) = tiles.get_mut(&entry.key) else {
                continue;
            };
            debug_assert!(tile.is_renderable());
            for layer_properties in update.layers {
                let type_info = layer_properties.style().info();
                if entry.is_held() && !type_info.fading_tiles {
                    continue;
                }
                let updated = tile.layer_properties_updated(layer_properties);
                entry.needs_rendering |= updated;
            }
        }

        if self.info.kind != TileKind::Geometry {
            let renderables: Vec<(UnwrappedTileID, bool)> =
                self.rendered.iter().map(|(id, entry)| (*id, entry.needs_rendering)).collect();
            let masks = update_tile_masks(&renderables);
            let SourceTiles { tiles, rendered, masked, masked_before, .. } = self;
            std::mem::swap(masked, masked_before);
            masked.clear();
            for (entry, mask) in rendered.values().zip(masks) {
                let Some(contents) = tiles.get_mut(&entry.key).and_then(|slot| slot.tile.contents_mut()) else {
                    continue;
                };
                if let Some(mask) = mask {
                    contents.set_mask(mask);
                }
                if contents.is_masked() {
                    masked.push(entry.key);
                }
            }
            masked.sort_unstable();
            self.unmask_stopped();
        }
    }

    /// Unmasks the tiles the frame before drew masked and this frame
    /// doesn't: a tile that stopped drawing drops its quads until it draws
    /// again, which masks it anew. `masked` is sorted.
    fn unmask_stopped(&mut self) {
        let SourceTiles { tiles, masked, masked_before, .. } = self;
        for key in masked_before.iter().filter(|key| masked.binary_search(key).is_err()) {
            if let Some(contents) = tiles.get_mut(key).and_then(|slot| slot.tile.contents_mut()) {
                contents.unmask();
            }
        }
    }

    /// Raises the high-water mark of the content elevation, such as
    /// `symbol-height-offset`, to `layers`' and the cover's, and returns it.
    fn update_max_content_elevation(&mut self, layers: &[Arc<dyn ErasedEvaluatedLayer>]) -> f32 {
        let mut max = self.max_content_elevation;
        // Only the buckets know the heights of an expression.
        let mut expression_layers = Vec::new();
        for layer in layers {
            match layer.style().content_elevation() {
                Some(PropertyValue::Constant(height_offset)) => max = max.max(*height_offset),
                Some(PropertyValue::Expression(_)) => expression_layers.push(layer.style()),
                Some(PropertyValue::Undefined) | None => {}
            }
        }
        if !expression_layers.is_empty() {
            let contents = self.cover().filter_map(Tile::contents);
            for contents in contents {
                for layer in &expression_layers {
                    if let Some(bucket) = contents.bucket(*layer) {
                        max = max.max(bucket.max_content_elevation());
                    }
                }
            }
        }
        self.max_content_elevation = max;
        max
    }

    fn set_necessity(&mut self, id: &OverscaledTileID, ctx: &mut StoreContext, necessity: TileNecessity) {
        self.update(id, ctx, |tile| tile.set_necessity(necessity));
    }

    pub(crate) fn release(&mut self, id: &OverscaledTileID, ctx: &mut StoreContext) {
        if let Some(slot) = self.tiles.remove(id) {
            retire(&self.info, slot.tile, ctx);
        }
    }

    /// Lets go of cached tile `id`. One still loading is made optional
    /// first, so that a custom source hears its load stop.
    fn evict(&mut self, id: &OverscaledTileID, ctx: &mut StoreContext) {
        if self.tile(id).is_some_and(|tile| !tile.is_renderable()) {
            self.set_necessity(id, ctx, TileNecessity::Optional);
        }
        self.release(id, ctx);
    }

    fn oldest_cached(&self) -> Option<OverscaledTileID> {
        self.tiles
            .values()
            .filter_map(|slot| match slot.place {
                Place::Cached { since } => Some((since, slot.tile.id)),
                Place::Cover { .. } => None,
            })
            .min_by_key(|(since, _)| *since)
            .map(|(_, id)| id)
    }

    // A scan per eviction: a trim usually evicts one tile.
    fn cache_trim(&mut self, ctx: &mut StoreContext) {
        let mut count = self.tiles.values().filter(|slot| slot.place.is_cached()).count();
        while count > self.cache_size
            && let Some(oldest) = self.oldest_cached()
        {
            self.evict(&oldest, ctx);
            count -= 1;
        }
    }

    /// Moves cover tile `id` into the cache, where one still loading goes on
    /// loading behind the cover's tiles.
    fn cache_add(&mut self, id: OverscaledTileID, ctx: &mut StoreContext) {
        if self.cache_size == 0 {
            self.evict(&id, ctx);
            return;
        }
        let Some(slot) = self.tiles.get_mut(&id) else {
            return;
        };
        self.cache_clock += 1;
        slot.set_place(Place::Cached { since: self.cache_clock });
        self.cache_trim(ctx);
    }

    /// Moves cached tile `id` back into the cover, unless caching rules
    /// forbid using its data: then the tile goes, and one made anew holds its
    /// data back until it revalidates.
    fn cache_pop(&mut self, id: &OverscaledTileID, ctx: &mut StoreContext) -> bool {
        let Some(slot) = self.tiles.get_mut(id).filter(|slot| slot.place.is_cached()) else {
            return false;
        };
        if !slot.tile.load.is_usable(mln_util::chrono::now()) {
            self.evict(id, ctx);
            return false;
        }
        slot.set_place(Place::Cover { order: TileLoadOrder::UNWANTED });
        true
    }

    /// A copy of a renderable tile that is `id` in another world copy: of a
    /// cached tile only if caching rules let it come back from the cache
    /// (`cache_pop`). The copy's buckets, copies of the tile's, draw the
    /// geometry that the first of either to upload moves into the context's
    /// stores (`upload_at`).
    pub(crate) fn world_copy(&self, id: &OverscaledTileID) -> Option<WorldCopy> {
        let now = mln_util::chrono::now();
        let tile = self
            .same_place(id)
            .into_iter()
            .filter_map(|at| Some(self.tiles.get_index(at)?.1))
            .filter(|slot| !slot.place.is_cached() || slot.tile.load.is_usable(now))
            .map(|slot| &slot.tile)
            .find(|tile| tile.id.wrap != id.wrap && tile.is_renderable())?;
        WorldCopy::of(tile)
    }

    /// Moves the tile cached under any wrap of `id` into the cover under
    /// `id`'s: on the globe every wrap of a tile is the same piece of it.
    fn cache_pop_any_wrap(&mut self, id: &OverscaledTileID, ctx: &mut StoreContext) -> bool {
        let Some(found) = self
            .same_place(id)
            .into_iter()
            .filter_map(|at| self.tiles.get_index(at))
            .find(|(_, slot)| slot.place.is_cached())
            .map(|(&found, _)| found)
        else {
            return false;
        };
        if !self.cache_pop(&found, ctx) {
            return false;
        }
        self.rekey(&found, *id, ctx);
        true
    }

    /// Moves tile `from` to `to`, where a cached tile makes way.
    pub(crate) fn rekey(&mut self, from: &OverscaledTileID, to: OverscaledTileID, ctx: &mut StoreContext) {
        if self.tiles.get(&to).is_some_and(|slot| slot.place.is_cached()) {
            self.release(&to, ctx);
        }
        if let Some(mut slot) = self.tiles.remove(from) {
            slot.tile.id = to;
            self.tiles.insert(to, slot);
        }
    }

    fn cache_set_size(&mut self, size: usize, ctx: &mut StoreContext) {
        self.cache_size = size;
        self.cache_trim(ctx);
    }

    fn cache_clear(&mut self, ctx: &mut StoreContext) {
        let cached: Vec<OverscaledTileID> =
            self.tiles.values().filter(|slot| slot.place.is_cached()).map(|slot| slot.tile.id).collect();
        for id in cached {
            self.evict(&id, ctx);
        }
    }

    /// Lets go of the cached tiles still loading that `keep` turns down, and
    /// of those whose load failed.
    fn cache_release_loading(&mut self, ctx: &mut StoreContext, keep: impl Fn(&OverscaledTileID) -> bool) {
        let released: Vec<OverscaledTileID> = self
            .tiles
            .values()
            .filter(|slot| {
                slot.place.is_cached()
                    && !slot.tile.is_renderable()
                    && (slot.tile.load.has_failed() || !keep(&slot.tile.id))
            })
            .map(|slot| slot.tile.id)
            .collect();
        for id in released {
            self.evict(&id, ctx);
        }
    }

    pub(crate) fn rendered(&self) -> impl Iterator<Item = RenderedTile<'_>> + '_ {
        self.rendered.iter().filter_map(|(id, entry)| {
            let slot = self.tiles.index_of(&entry.key)?;
            let tile = &self.tiles.get_index(slot)?.1.tile;
            Some(RenderedTile { id, tile, slot, hold_for_fade: entry.is_held() })
        })
    }

    #[must_use]
    pub(crate) fn rendered_count(&self) -> usize {
        self.rendered.len()
    }

    /// Brings the rendered tiles' buckets up to date with the source's
    /// feature state, and adds the buckets whose vertex data that changed to
    /// `changed`.
    pub(crate) fn coalesce_feature_state(&mut self, changed: &mut ChangedBuckets) {
        let keys: BTreeSet<OverscaledTileID> = self.rendered.values().map(|entry| entry.key).collect();
        let SourceTiles { info, tiles, .. } = self;
        info.feature_state.coalesce_changes(
            tiles.values_mut().filter(|slot| keys.contains(&slot.tile.id)).filter_map(|slot| slot.tile.shown.as_mut()),
            changed,
        );
    }

    /// The features the source's rendered tiles draw in `geometry`, screen
    /// points from the top left, traced past the globe's limb
    /// ([`query_screen_geometry`](crate::query::feature_index::query_screen_geometry)).
    /// With `ground`, the ground a frame drew of the terrain, a point finds
    /// features where its line of sight first meets the ground; else, and
    /// where it meets none, on the flat map at sea level. Every rendered
    /// tile is a candidate for layers that stand on the ground
    /// ([`QueryGround::standing`](crate::query::feature_index::QueryGround::standing)).
    #[expect(clippy::too_many_arguments, reason = "the query's geometry, ground, camera, layers and options")]
    pub(crate) fn query_rendered_features(
        &self,
        geometry: &[mln_util::geo::ScreenCoordinate],
        ground: Option<crate::query::feature_index::QueryGround<'_>>,
        view: &mln_map_types::View,
        layers: &crate::query::feature_index::QueryLayers<'_>,
        options: &crate::query::RenderedQueryOptions,
        global_state: Option<&mln_style::expression::GlobalStateMap>,
        proj_matrix: &glam::DMat4,
    ) -> Result<crate::query::feature_index::QueryResults, mln_style::error::Error> {
        use mln_map_types::tile_coordinate::TileCoordinate;

        let feature_state = &self.info.feature_state;

        let mut result = crate::query::feature_index::QueryResults::default();
        if self.rendered.is_empty() || geometry.is_empty() {
            return Ok(result);
        }

        let height = f64::from(view.size().height);
        let globe = view.is_globe();
        let up = |p: &mln_util::geo::ScreenCoordinate| mln_util::geo::ScreenCoordinate::new(p.x, height - p.y);
        let on_ground =
            |p: &mln_util::geo::ScreenCoordinate| ground.and_then(|ground| view.ground_under(&up(p), ground.elevation));
        let standing = ground.is_some_and(|ground| ground.standing);
        let to_world = |p: &mln_util::geo::ScreenCoordinate| match on_ground(p) {
            // On the main world, where the globe's sea-level points are.
            Some(hit) if globe => geometry::Point::new(hit.world.x.rem_euclid(1.0), hit.world.y),
            Some(hit) => geometry::Point::new(hit.world.x, hit.world.y),
            None => TileCoordinate::from_screen_coordinate(view, 0, &up(p)).p,
        };
        let on_planet = |p: &mln_util::geo::ScreenCoordinate| {
            VerticalPerspectiveProjection::screen_coordinate_hits_globe(view, &up(p), 0.0) || on_ground(p).is_some()
        };

        let mut query_geometry: Vec<geometry::Point<f64>> = geometry.iter().map(to_world).collect();

        // The bounding box of points, of which the geometry has at least one.
        let envelope = |points: &mut dyn Iterator<Item = (f64, f64)>| {
            let (mut min, mut max) = ((f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY));
            for (x, y) in points {
                min = (f64::min(min.0, x), f64::min(min.1, y));
                max = (f64::max(max.0, x), f64::max(max.1, y));
            }
            (min, max)
        };

        if globe {
            // The globe has no world copies, so a box across the
            // antimeridian lands on the rest of the world, which shows when
            // its slightly shrunken corners fall outside it. It moves to the
            // copy west of the main world.
            let (mut screen_min, mut screen_max) = envelope(&mut geometry.iter().map(|p| (p.x, p.y)));
            let shrink = f64::min(screen_max.0 - screen_min.0, screen_max.1 - screen_min.1) * 0.001;
            screen_min.0 += shrink;
            screen_min.1 += shrink;
            screen_max.0 -= shrink;
            screen_max.1 -= shrink;
            let (bounds_min, bounds_max) = envelope(&mut query_geometry.iter().map(|p| (p.x, p.y)));
            // A corner past the planet stands for the nearest point on the
            // horizon, which tells nothing about the antimeridian: a check of
            // the corners alone would lose boxes that reach into space, such
            // as the whole view of a small globe. Such corners are left out,
            // and the box's center, when it is on the planet, decides by
            // longitude for them.
            let mut covered = true;
            let mut reaches_into_space = false;
            for corner in [
                mln_util::geo::ScreenCoordinate::new(screen_min.0, screen_min.1),
                mln_util::geo::ScreenCoordinate::new(screen_max.0, screen_min.1),
                mln_util::geo::ScreenCoordinate::new(screen_min.0, screen_max.1),
                mln_util::geo::ScreenCoordinate::new(screen_max.0, screen_max.1),
            ] {
                if !on_planet(&corner) {
                    reaches_into_space = true;
                    continue;
                }
                let c = to_world(&corner);
                covered =
                    covered && bounds_min.0 <= c.x && c.x <= bounds_max.0 && bounds_min.1 <= c.y && c.y <= bounds_max.1;
            }
            let center = mln_util::geo::ScreenCoordinate::new(
                f64::midpoint(screen_min.0, screen_max.0),
                f64::midpoint(screen_min.1, screen_max.1),
            );
            if reaches_into_space && on_planet(&center) {
                // The box's horizon points include the one due north or south
                // of the center, at its longitude: a center within rounding
                // of the box's edge is past it.
                const EDGE: f64 = 1e-9;
                let c = to_world(&center);
                covered = covered && bounds_min.0 + EDGE < c.x && c.x < bounds_max.0 - EDGE;
            }
            if !covered {
                for c in &mut query_geometry {
                    if c.x > 0.5 {
                        c.x -= 1.0;
                    }
                }
            }
        }

        // The bounding box of the query.
        let (box_min, box_max) = envelope(&mut query_geometry.iter().map(|p| (p.x, p.y)));
        let box_min = geometry::Point::new(box_min.0, box_min.1);
        let box_max = geometry::Point::new(box_max.0, box_max.1);

        // Sorted by (z, y, wrap, x); on the globe by their copy on the main
        // world, (z, y, x, wrap), so that there the wrap only breaks ties.
        let mut sorted_tiles: Vec<(&UnwrappedTileID, &OverscaledTileID)> =
            self.rendered.iter().map(|(id, entry)| (id, &entry.key)).collect();
        sorted_tiles.sort_by(|(a, _), (b, _)| {
            if globe {
                return (a.canonical.z, a.canonical.y, a.canonical.x, a.wrap).cmp(&(
                    b.canonical.z,
                    b.canonical.y,
                    b.canonical.x,
                    b.wrap,
                ));
            }
            (a.canonical.z, a.canonical.y, a.wrap, a.canonical.x).cmp(&(
                b.canonical.z,
                b.canonical.y,
                b.wrap,
                b.canonical.x,
            ))
        });

        let max_pitch_scale_factor = view.max_pitch_scale_factor() as f32;

        // The globe returns a feature once per tile, though the tile is
        // tested at three copies and can be rendered at two wraps: these are
        // where this tile's features start in each layer's results.
        let mut canonical_tile: Option<mln_style::tile_id::CanonicalTileID> = None;
        let mut canonical_tile_start: mln_style::hash::HashMap<String, usize> = mln_style::hash::HashMap::default();

        for (id, key) in sorted_tiles {
            let Some(tile) = self.tile(key) else {
                continue;
            };
            let Some(contents) = tile.contents().and_then(Contents::geometry) else {
                continue;
            };

            // 2 to the power of the zoom's distance above the tile's.
            let scale = (view.scale() / f64::from(1u32 << id.canonical.z)) as f32;
            let query_padding =
                f64::from(max_pitch_scale_factor * contents.query_padding(layers) * constants::EXTENT as f32)
                    / constants::TILE_SIZE_D
                    / f64::from(scale);

            if globe && canonical_tile != Some(id.canonical) {
                canonical_tile = Some(id.canonical);
                canonical_tile_start.clear();
                for (layer_id, features) in &result {
                    canonical_tile_start.insert(layer_id.clone(), features.len());
                }
            }

            // The query geometry lies on the main world or, across the
            // antimeridian, west of it, and its padding reaches past either
            // side of it: a globe tile is tested at its copies west of, on and
            // east of the main world, so that a query just west of the
            // antimeridian finds the features just east of it.
            let globe_copies = [
                UnwrappedTileID::with_wrap(-1, id.canonical),
                UnwrappedTileID::with_wrap(0, id.canonical),
                UnwrappedTileID::with_wrap(1, id.canonical),
            ];
            let copies: &[UnwrappedTileID] = if globe { &globe_copies } else { std::slice::from_ref(id) };
            for copy in copies {
                let tile_space_bounds_min = TileCoordinate::to_geometry_coordinate(copy, &box_min);
                if !standing
                    && (f64::from(tile_space_bounds_min.x) - query_padding >= f64::from(constants::EXTENT)
                        || f64::from(tile_space_bounds_min.y) - query_padding >= f64::from(constants::EXTENT))
                {
                    continue;
                }

                let tile_space_bounds_max = TileCoordinate::to_geometry_coordinate(copy, &box_max);
                if !standing
                    && (f64::from(tile_space_bounds_max.x) + query_padding < 0.0
                        || f64::from(tile_space_bounds_max.y) + query_padding < 0.0)
                {
                    continue;
                }

                let tile_space_query_geometry: mln_style::feature::GeometryCoordinates =
                    query_geometry.iter().map(|c| TileCoordinate::to_geometry_coordinate(copy, c)).collect();

                if !globe {
                    contents.query_rendered_features(
                        &tile.id,
                        &mut result,
                        &tile_space_query_geometry,
                        (geometry, ground),
                        view,
                        layers,
                        options,
                        global_state,
                        proj_matrix,
                        feature_state,
                    )?;
                    continue;
                }

                let mut found = crate::query::feature_index::QueryResults::default();
                contents.query_rendered_features(
                    &tile.id,
                    &mut found,
                    &tile_space_query_geometry,
                    (geometry, ground),
                    view,
                    layers,
                    options,
                    global_state,
                    proj_matrix,
                    feature_state,
                )?;
                for (layer_id, features) in found {
                    let start = canonical_tile_start.get(&layer_id).copied().unwrap_or(0);
                    let layer_result = result.entry(layer_id).or_default();
                    let end = layer_result.len();
                    for feature in features {
                        // `other == feature` compares the GeoJSON feature
                        // base: id, geometry and properties.
                        if !layer_result[start..end].iter().any(|other| {
                            other.id == feature.id
                                && other.geometry == feature.geometry
                                && other.properties == feature.properties
                        }) {
                            layer_result.push(feature);
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    pub(crate) fn query_source_features(
        &self,
        options: &crate::query::SourceQueryOptions,
        global_state: Option<&mln_style::expression::GlobalStateMap>,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_style::error::Error> {
        let mut result = Vec::new();
        for tile in self.cover() {
            if let Some(contents) = tile.contents().and_then(Contents::geometry) {
                contents.query_source_features(&tile.id, self.info.single_layer, &mut result, options, global_state)?;
            }
        }
        Ok(result)
    }

    /// On the globe every wrap of a tile is the same piece of the sphere, and
    /// the cover gives tiles the wraps of the world copy the center has
    /// reached. A tile the cover now wants under another wrap moves there
    /// instead of loading again, which would leave its part of the globe bare
    /// until it did: the whole globe at the zoom-0 tile, which changes wrap
    /// whenever the center crosses the antimeridian.
    fn move_tiles_to_wanted_wraps(
        &mut self,
        ideal_tiles: &[OverscaledTileID],
        pan_tiles: &[OverscaledTileID],
        ctx: &mut StoreContext,
    ) {
        let mut wanted: BTreeSet<OverscaledTileID> = ideal_tiles.iter().copied().collect();
        wanted.extend(pan_tiles.iter().copied());
        for id in &wanted {
            if self.tiles.get(id).is_some_and(|slot| !slot.place.is_cached()) {
                continue;
            }
            let Some(loaded) = self
                .cover()
                .find(|tile| {
                    tile.id.overscaled_z == id.overscaled_z
                        && tile.id.canonical == id.canonical
                        && !wanted.contains(&tile.id)
                })
                .map(|tile| tile.id)
            else {
                continue;
            };
            let rendered_id = loaded.to_unwrapped();
            self.rekey(&loaded, *id, ctx);
            // An entry already at the key stays.
            if let Some(entry) = self.rendered.remove(&rendered_id) {
                self.rendered.entry(id.to_unwrapped()).or_insert(Rendered { key: *id, ..entry });
            }
        }
    }

    /// Reassigns the cover's tiles' wrap values when the map pans by whole
    /// worlds.
    ///
    /// On top of the regular z/x/y values, `TileIDs` have a `wrap` value that
    /// specify which copy of the world the tile belongs to. For example, at
    /// `lng: 10` you might render z/x/y/0 while at `lng: 370` you would
    /// render z/x/y/1.
    ///
    /// When lng values get wrapped (going from `lng: 370` to `long: 10`) you
    /// expect to see the same thing on the screen (370 degrees and 10
    /// degrees is the same place in the world) but all the `TileIDs` will have
    /// different wrap values.
    ///
    /// In order to make this transition seamless, we calculate the rounded
    /// difference of "worlds" between the last frame and the current frame.
    /// If the map panned by a world, then we can assign all the tiles new
    /// `TileIDs` with updated wrap values. For example, assign z/x/y/1 a new
    /// id: z/x/y/0. It is the same tile, just rendered in a different
    /// position.
    ///
    /// This enables us to reuse the tiles at more ideal locations and
    /// prevent flickering.
    pub(crate) fn handle_wrap_jump(&mut self, lng: f32, ctx: &mut StoreContext) {
        let lng_difference = lng - self.prev_lng;
        let world_difference = lng_difference / 360.0f32;
        let wrap_delta = world_difference.round() as i16;
        self.prev_lng = lng;
        if wrap_delta == 0 {
            return;
        }

        let cover: Vec<OverscaledTileID> = self.cover().map(|tile| tile.id).collect();
        let mut moved: Vec<Slot> = Vec::with_capacity(cover.len());
        let mut key_map: BTreeMap<OverscaledTileID, OverscaledTileID> = BTreeMap::new();
        for id in cover {
            if let Some(mut slot) = self.tiles.remove(&id) {
                let new_id = id.unwrap_to(id.wrap.wrapping_add(wrap_delta));
                key_map.insert(id, new_id);
                slot.tile.id = new_id;
                moved.push(slot);
            }
        }
        for slot in moved {
            let id = slot.tile.id;
            match self.tiles.get(&id) {
                // A cached tile at the id makes way for the cover's.
                Some(existing) if existing.place.is_cached() => self.release(&id, ctx),
                // `emplace` keeps the first tile with an id.
                Some(_) => {
                    retire(&self.info, slot.tile, ctx);
                    continue;
                }
                None => {}
            }
            self.tiles.insert(id, slot);
        }

        let mut new_render_tiles: VecMap<UnwrappedTileID, Rendered> = VecMap::default();
        for (id, entry) in std::mem::take(&mut self.rendered) {
            let new_id = id.unwrap_to(id.wrap.wrapping_add(wrap_delta));
            let key = key_map.get(&entry.key).copied().unwrap_or(entry.key);
            new_render_tiles.entry(new_id).or_insert(Rendered { key, ..entry });
        }
        self.rendered = new_render_tiles;
    }

    pub(crate) fn set_cache_enabled(&mut self, enable: bool) {
        self.cache_enabled = enable;
    }

    pub(crate) fn reduce_memory_use(&mut self, ctx: &mut StoreContext) {
        self.cache_clear(ctx);
    }

    pub(crate) fn dump_debug_logs(&self) {
        for tile in self.cover() {
            tile.dump_debug_logs(self.info.kind);
        }
    }

    /// The source's style: the last one, while its values stay the same.
    fn source_style(
        &mut self,
        layers: &[Arc<dyn ErasedEvaluatedLayer>],
        parameters: &TileParameters<'_>,
    ) -> Arc<SourceStyle> {
        let global_state = parameters.global_state.as_ref();
        let font_faces = parameters.font_faces.as_ref();
        match &self.style {
            Some(style) if style.is(layers, global_state, font_faces, parameters.glyph_pbfs) => Arc::clone(style),
            previous => {
                let style = Arc::new(SourceStyle {
                    version: previous.as_ref().map_or(0, |style| style.version + 1),
                    layers: layers.to_vec(),
                    global_state: global_state.cloned(),
                    font_faces: font_faces.cloned(),
                    glyph_pbfs: parameters.glyph_pbfs,
                });
                self.style = Some(Arc::clone(&style));
                style
            }
        }
    }

    pub(crate) fn clear_all(&mut self, ctx: &mut StoreContext) {
        self.max_content_elevation = 0.0;
        self.rendered.clear();
        for (_, slot) in std::mem::take(&mut self.tiles) {
            retire(&self.info, slot.tile, ctx);
        }
    }

    /// Whether the last update held an entry the view left, which waits for
    /// placements to fade its symbols out, or for the next update to drop
    /// it once they have.
    #[must_use]
    pub(crate) fn has_fading_tiles(&self) -> bool {
        self.rendered.values().any(Rendered::is_held)
    }
}

/// Readies the buckets of the geometry tile at `at` of `slots` for the
/// frame's draws, sharing what the buckets of the tiles at `copies`, its
/// other world copies, moved into `context`'s stores.
fn upload_sharing(slots: &mut [Slot], at: usize, copies: &[usize], context: &mut Context, needs: UploadNeeds) {
    let Some((before, rest)) = slots.split_at_mut_checked(at) else {
        return;
    };
    let Some((slot, after)) = rest.split_first_mut() else {
        return;
    };
    let Some(geometry) = slot.tile.contents_mut().and_then(Contents::geometry_mut) else {
        return;
    };
    let shared: SmallVec<[&BucketGeometry; 4]> = copies
        .iter()
        .filter_map(|&copy| match copy.checked_sub(at + 1) {
            Some(after_at) => after.get(after_at),
            None => before.get(copy),
        })
        .filter_map(|slot| Some(&slot.tile.contents()?.geometry()?.bucket_geometry))
        .collect();
    geometry.upload(&shared, context, needs);
}
