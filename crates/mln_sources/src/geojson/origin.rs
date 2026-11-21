//! The render half of GeoJSON sources: tiles cut from the source's data, on
//! the renderer's pool or, for a source with synchronous updates, as they're
//! asked for. New data loads every tile of the cover again.

use std::sync::Arc;

use mln_map_types::MapMode;
use mln_renderer::source::{
    Cover, DataOrigin, FeatureExtensionValue, LoadContext, NewTile, OriginUpdate, Replier, SourceType, Supply,
    TileInput, TileKind, TileLoad,
};
use mln_style::Source;
use mln_style::source_type::SourceData;
use mln_tile::TileData;
use mln_util::constants;
use mln_util::error::Error;
use mln_util::pool::Pool;
use parking_lot::Mutex;

use super::{GeoJSONData, GeoJsonSourceType, TileFeatures};

impl SourceType for GeoJsonSourceType {
    fn supply(_source: &Source) -> Supply {
        Supply::single_layer_tiles(TileKind::Geometry, GeoJsonOrigin { data: None, synchronous: false })
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

/// A GeoJSON tile's load: the engine keeps which is the latest.
struct GeoJsonLoad;

impl TileLoad for GeoJsonLoad {}

impl DataOrigin for GeoJsonOrigin {
    type Load = GeoJsonLoad;
    type Reply = TileData;

    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let source = SourceData::<GeoJsonSourceType>::recover(update.source())?;
        // A tile lays out in the mode it was created with, so a mode change
        // recreates the tiles.
        if update.is_synchronous() != self.synchronous {
            self.synchronous = update.is_synchronous();
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
                update.reduce_memory_use();
                let max_z = source.options.get_zoom_range().max;
                let relayout = update.needs_relayout();
                update.update_tiles(|id| id.canonical.z <= max_z, |_, cx| load(data, relayout, cx));
            }
        }
        data?;
        Some(Cover { tile_size: constants::TILE_SIZE_I, zoom_range: source.options.get_zoom_range(), bounds: None })
    }

    fn new_load(&mut self, _tile: &NewTile<'_>) -> Option<GeoJsonLoad> {
        self.data.as_ref().map(|_| GeoJsonLoad)
    }

    fn start(&mut self, _load: &mut GeoJsonLoad, cx: &mut LoadContext<'_, '_, TileData>) {
        if let Some(data) = &self.data {
            load(data, false, cx);
        }
    }

    fn receive(&mut self, _load: &mut GeoJsonLoad, data: TileData, cx: &mut LoadContext<'_, '_, TileData>) {
        cx.deliver(TileInput::Geometry { data: Some(data), missing: false });
    }

    /// The "supercluster" extension of a GeoJSON source: a cluster feature's
    /// "children", "leaves" (`args` "limit", default 10, and "offset",
    /// default 0) or "expansion-zoom". Anything else is a null value.
    fn query_feature_extensions(
        &self,
        feature: &mln_util::feature::Feature,
        extension: &str,
        extension_field: &str,
        args: Option<&feature::PropertyMap>,
    ) -> Result<FeatureExtensionValue, Error> {
        if extension != "supercluster" {
            return Ok(FeatureExtensionValue::default());
        }
        let Some(cluster_id) = get_uint_property(&feature.properties, "cluster_id") else {
            return Ok(FeatureExtensionValue::default());
        };
        let Some(json_data) = &self.data else {
            return Ok(FeatureExtensionValue::default());
        };
        supercluster_extension(&**json_data, extension_field, cluster_id as u32, args)
            .unwrap_or_else(|| Ok(FeatureExtensionValue::default()))
    }
}

/// Loads the tile's features from `data`, cut on the renderer's pool unless
/// the source's updates are synchronous. With `relayout`, the tile first
/// drops its data and style.
fn load(data: &Arc<dyn GeoJSONData>, relayout: bool, cx: &mut LoadContext<'_, '_, TileData>) {
    if relayout {
        cx.reset();
    }
    let replier = cx.begin_load();
    let pool = (!cx.is_synchronous()).then(|| cx.pool());
    if let Some(features) = get_tile(data, cx.id().canonical, replier, pool) {
        cx.deliver(TileInput::Geometry { data: Some(features), missing: false });
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

/// The property `name`, if it is an unsigned integer.
fn get_uint_property(properties: &feature::PropertyMap, name: &str) -> Option<u64> {
    match properties.get(name) {
        Some(feature::Value::Uint(value)) => Some(*value),
        _ => None,
    }
}

/// The supercluster `extensionGetters`: the `extension_field` of cluster
/// `cluster_id`, or `None` for a field it doesn't have.
fn supercluster_extension(
    cluster_data: &dyn GeoJSONData,
    extension_field: &str,
    cluster_id: u32,
    args: Option<&feature::PropertyMap>,
) -> Option<Result<FeatureExtensionValue, Error>> {
    Some(match extension_field {
        "children" => cluster_data.get_children(cluster_id).map(FeatureExtensionValue::FeatureCollection),
        "leaves" => {
            // Offset cannot be set without limit.
            let (limit, offset) = match args.and_then(|args| Some((get_uint_property(args, "limit")?, args))) {
                Some((limit, args)) => {
                    (limit as u32, get_uint_property(args, "offset").map_or(0, |offset| offset as u32))
                }
                None => (10, 0),
            };
            cluster_data.get_leaves(cluster_id, limit, offset).map(FeatureExtensionValue::FeatureCollection)
        }
        "expansion-zoom" => cluster_data
            .get_cluster_expansion_zoom(cluster_id)
            .map(|zoom| FeatureExtensionValue::Value(feature::Value::Uint(u64::from(zoom)))),
        _ => return None,
    })
}
