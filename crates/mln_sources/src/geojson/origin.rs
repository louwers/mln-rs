//! The render half of GeoJSON sources: tiles cut from the source's data, on
//! the renderer's pool or, for a source with synchronous updates, as they're
//! asked for. New data loads every tile of the cover again.

use std::sync::Arc;

use mln_map_types::MapMode;
use mln_render::source::{Cover, DataOrigin, GeometryTiles, LoadContext, OriginUpdate, Replier, SourceType, Supply};
use mln_style::Source;
use mln_style::source_type::SourceData;
use mln_tile::TileData;
use mln_util::constants;
use mln_util::pool::Pool;
use parking_lot::Mutex;

use super::{GeoJSONData, GeoJsonSourceType, TileFeatures};

impl SourceType for GeoJsonSourceType {
    type Data = GeometryTiles;

    fn supply(_source: &Source) -> Supply<GeometryTiles> {
        Supply::tiles(GeoJsonOrigin { data: None, synchronous: false })
    }
}

/// A GeoJSON source's tiles: the data they are cut from, and whether they
/// were made with synchronous updates.
struct GeoJsonOrigin {
    data: Option<Arc<dyn GeoJSONData>>,
    synchronous: bool,
}

/// Whether `a` and `b` are both empty or both the same object.
fn same_data(a: Option<&Arc<dyn GeoJSONData>>, b: Option<&Arc<dyn GeoJSONData>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)),
        _ => false,
    }
}

impl DataOrigin for GeoJsonOrigin {
    type Data = GeometryTiles;
    /// The engine keeps which of a tile's loads is the latest.
    type Tile = ();
    type Reply = TileData;

    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let source = SourceData::<GeoJsonSourceType>::recover(update.source())?;
        // A tile lays out in the mode it was created with, so a mode change
        // recreates the tiles.
        let synchronous = source.options.synchronous_update;
        if synchronous != self.synchronous {
            self.synchronous = synchronous;
            update.clear_tiles();
        }
        let data = source.description.clone();
        if !same_data(self.data.as_ref(), data.as_ref()) {
            self.data.clone_from(&data);
            if update.mode() != MapMode::Continuous {
                // Clearing the tiles in order to avoid render tests being
                // flaky.
                update.clear_tiles();
            } else if let Some(data) = &data {
                update.clear_cache();
                let max_z = source.options.get_zoom_range().max;
                let relayout = update.needs_relayout();
                update.update_tiles(|id| id.canonical.z <= max_z, |(), cx| load(data, synchronous, relayout, cx));
            }
        }
        data?;
        Some(Cover {
            tile_size: constants::TILE_SIZE_I,
            zoom_range: source.options.get_zoom_range(),
            bounds: None,
            round_zoom: false,
            overscale: true,
            prefetch: false,
            synchronous,
        })
    }

    fn start(&mut self, cx: &mut LoadContext<'_, '_, Self>, _copy: Option<&()>) {
        if let Some(data) = &self.data {
            load(data, self.synchronous, false, cx);
        }
    }

    fn receive(&mut self, (): &mut (), data: TileData, cx: &mut LoadContext<'_, '_, Self>) {
        cx.deliver(Some(data));
    }
}

/// Loads the tile's features from `data`, cut on the renderer's pool unless
/// `synchronous`. With `relayout`, the tile first drops its data and style.
fn load(data: &Arc<dyn GeoJSONData>, synchronous: bool, relayout: bool, cx: &mut LoadContext<'_, '_, GeoJsonOrigin>) {
    if relayout {
        cx.reset();
    }
    let replier = cx.begin_load();
    let pool = (!synchronous).then(|| cx.pool());
    if let Some(features) = get_tile(data, cx.id().canonical, replier, pool) {
        cx.deliver(Some(features));
    }
}

/// Asks `data` for tile `id`'s features, cut on `pool` if there is one:
/// returns them if they answer before this returns, else `replier` sends
/// them.
fn get_tile(
    data: &Arc<dyn GeoJSONData>,
    id: mln_tile_id::CanonicalTileID,
    replier: Replier<TileData>,
    pool: Option<&Pool>,
) -> Option<TileData> {
    let slot: Arc<Mutex<Answered>> = Arc::default();
    let callback_slot = Arc::clone(&slot);
    data.get_tile(
        &id,
        Box::new(move |features: Arc<TileFeatures>| {
            let data = TileData::Features(features);
            let mut slot = callback_slot.lock();
            if matches!(*slot, Answered::Later) {
                drop(slot);
                replier.send(data);
            } else {
                *slot = Answered::Now(data);
            }
        }),
        pool,
    );
    match std::mem::replace(&mut *slot.lock(), Answered::Later) {
        Answered::Now(data) => Some(data),
        Answered::Pending | Answered::Later => None,
    }
}

/// When GeoJSON data answered: before `get_tile` returned, or after.
#[derive(Default)]
enum Answered {
    #[default]
    Pending,
    Now(TileData),
    Later,
}
