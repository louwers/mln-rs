//! The render half of GeoJSON sources: their tiles take their features
//! from the source's data, cut off the tiles' thread unless the source
//! updates synchronously.

use std::sync::Arc;

use mln_style::error::Error;
use mln_style::source::SourceData;
use mln_style::sources::GeoJsonSourceType;
use mln_style::sources::geojson::GeoJSONData;
use mln_style::{Source, constants};
use parking_lot::Mutex;

use crate::exec::pool::TaskGroup;
use crate::map::MapMode;
use crate::query::FeatureExtensionValue;
use crate::source::{
    Cover, DataOrigin, GeometryTiles, LoadContext, OriginUpdate, Replier, SourceType, Supply, WorldCopy,
};
use crate::tile::TileData;

impl SourceType for GeoJsonSourceType {
    type Data = GeometryTiles;

    fn supply(_source: &Source) -> Supply<GeometryTiles> {
        Supply::tiles(GeoJsonOrigin { data: None, synchronous: false })
    }
}

/// A GeoJSON source's tiles: the data they were made from, and whether they
/// were made with synchronous updates.
struct GeoJsonOrigin {
    /// The data, and where tiles of data that cuts them on demand are cut,
    /// off the tiles' thread: a lane of its own, which takes turns with the
    /// workers' other work.
    data: Option<(Arc<dyn GeoJSONData>, TaskGroup)>,
    synchronous: bool,
}

impl GeoJsonOrigin {
    /// Loads the tile from the source's data: cut on the pool unless the
    /// source updates synchronously. With `relayout`, the tile first drops
    /// its data and style.
    fn load(&self, cx: &mut LoadContext<'_, '_, Self>, relayout: bool) {
        let Some((data, cuts)) = &self.data else {
            return;
        };
        if relayout {
            cx.reset();
        }
        let replier = cx.begin_load();
        let cuts = (!self.synchronous).then_some(cuts);
        if let Some(features) = request_tile(data, cx.id().canonical, replier, cuts) {
            cx.deliver(Some(features));
        }
    }
}

impl DataOrigin for GeoJsonOrigin {
    type Data = GeometryTiles;
    type Tile = ();
    type Reply = TileData;

    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let source = SourceData::<GeoJsonSourceType>::recover(update.source())?;
        // A tile lays out in the mode it was created with, so a mode change
        // recreates the tiles.
        let synchronous = update.source().data().is_update_synchronous();
        if synchronous != self.synchronous {
            self.synchronous = synchronous;
            update.clear_tiles();
        }
        let data = source.description.as_ref();
        let same = match (self.data.as_ref().map(|(data, _)| data), data) {
            (None, None) => true,
            (Some(a), Some(b)) => std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)),
            _ => false,
        };
        if !same {
            self.data = data.map(|data| (Arc::clone(data), TaskGroup::new(update.pool())));
            if update.map().mode != MapMode::Continuous {
                // Clearing the tiles in order to avoid render tests being
                // flaky.
                update.clear_tiles();
            } else if data.is_some() {
                update.clear_cache();
                let max_z = source.options.get_zoom_range().max;
                let relayout = update.needs_relayout();
                update.update_tiles(|id| id.canonical.z <= max_z, |(), cx| self.load(cx, relayout));
            }
        }
        data?;
        Some(Cover { tile_size: constants::TILE_SIZE_I, zoom_range: source.options.get_zoom_range(), bounds: None })
    }

    fn start(&mut self, cx: &mut LoadContext<'_, '_, Self>, _copy: Option<WorldCopy<'_, ()>>) {
        self.load(cx, false);
    }

    fn receive(&mut self, (): &mut (), features: TileData, cx: &mut LoadContext<'_, '_, Self>) {
        cx.deliver(Some(features));
    }
}

/// Asks GeoJSON `data` for tile `id`'s features, cut in `cuts` if there is
/// one and the data cuts its tiles on demand: returns them if they answer
/// before this returns, else `replier` sends them.
fn request_tile(
    data: &Arc<dyn GeoJSONData>,
    id: mln_style::tile_id::CanonicalTileID,
    replier: Replier<TileData>,
    cuts: Option<&TaskGroup>,
) -> Option<TileData> {
    let Some(cuts) = cuts.filter(|_| data.cuts_on_demand()) else {
        return Some(TileData::Features(data.get_tile(&id)));
    };
    let slot: Arc<Mutex<Answered>> = Arc::default();
    let task_slot = Arc::clone(&slot);
    let data_for_task = Arc::clone(data);
    cuts.spawn(move || {
        let data = TileData::Features(data_for_task.get_tile(&id));
        let mut slot = task_slot.lock();
        if matches!(*slot, Answered::Later) {
            drop(slot);
            replier.send(data);
        } else {
            *slot = Answered::Now(data);
        }
    });
    match std::mem::replace(&mut *slot.lock(), Answered::Later) {
        Answered::Now(data) => Some(data),
        Answered::Pending | Answered::Later => None,
    }
}

/// When a tile cut off the tiles' thread answered: before its request
/// returned, or after.
#[derive(Default)]
enum Answered {
    #[default]
    Pending,
    Now(TileData),
    Later,
}

/// The "supercluster" extension of a GeoJSON source, `source`: a cluster
/// feature's "children", "leaves" (`args` "limit", default 10, and
/// "offset", default 0) or "expansion-zoom". Anything else, or a source of
/// another type, is a null value.
pub(crate) fn query_feature_extensions(
    source: &Source,
    feature: &crate::tile::feature::Feature,
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
    let Some(json_data) = SourceData::<GeoJsonSourceType>::recover(source).and_then(|data| data.description.as_ref())
    else {
        return Ok(FeatureExtensionValue::default());
    };
    supercluster_extension(&**json_data, extension_field, cluster_id as u32, args)
        .unwrap_or_else(|| Ok(FeatureExtensionValue::default()))
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
