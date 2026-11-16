//! The render half of the source types: each marker type's [`SourceType`]
//! impl, which says how the renderer draws the type's sources. The registry
//! holds the types as `&'static dyn ErasedSourceType` ([`entry`]).
//!
//! A source with tiles draws through a
//! [`RenderTileSource`](crate::sources::render_tile_source::RenderTileSource),
//! which keeps its type's [`TileOrigin`]: what the type remembers between
//! updates and how a new version of the source changes the tiles.

use std::marker::PhantomData;
use std::sync::{Arc, Weak};

use mln_map_types::MapMode;
use mln_storage::FileSource;
use mln_style::Source;
use mln_style::source_type::{ErasedStyleSourceType, SourceData, SourceTypeEntry, StyleSourceType};
use mln_style::sources::custom::{CustomVectorTileData, TileDataFormat};
use mln_style::sources::geojson::{GeoJSONData, TileFeatures};
use mln_style::sources::tileset::{RasterDemKind, RasterKind, TilesetKind, TilesetSourceType, VectorKind};
use mln_style::sources::{CustomGeometrySourceType, CustomVectorSourceType, GeoJsonSourceType, ImageSourceType};
use mln_tile::TileData;
use mln_util::constants;
use mln_util::error::Error;
use mln_util::geo::LatLngBounds;
use mln_util::range::Range;
use mln_util::tileset::{RasterEncoding, Tileset, VectorEncoding};

use crate::query::FeatureExtensionValue;
use crate::render_source::RenderSource;
use crate::sources::render_image_source::RenderImageSource;
use crate::sources::render_tile_source::RenderTileSource;
use crate::tile::loading::{CustomLoader, CustomLoaderOf, DataOrigin, VectorTileEncoding};
use crate::tile::source_tiles::SourceTiles;
use crate::tile::store::{StoreContext, TileStore};
use crate::tile::tile::TileKind;
use crate::tile_parameters::TileParameters;

/// A source type that the renderer draws.
pub(crate) trait SourceType: StyleSourceType {
    /// The render source of `source`, a source of the type, whose tiles, if
    /// it has any, go in `store`.
    fn render_source(source: &Source, store: &mut TileStore) -> Box<dyn RenderSource>;
}

/// What a source's tiles cover: their size in pixels, the zoom levels that
/// have tiles, and the bounds outside which there are none.
pub(crate) struct Cover {
    pub(crate) tile_size: u16,
    pub(crate) zoom_range: Range<u8>,
    pub(crate) bounds: Option<LatLngBounds>,
}

/// What a [`TileOrigin`] takes in with a new version of its source.
pub(crate) struct OriginUpdate<'a, 'p> {
    pub(crate) parameters: &'a TileParameters<'p>,
    pub(crate) needs_relayout: bool,
    pub(crate) fast_pfor_enabled: bool,
}

/// How a source type's tiles get their data: what its render source
/// remembers between updates, and how a new version of the source changes
/// the tiles.
pub(crate) trait TileOrigin {
    /// Takes in `source`, which replaces `previous`: sets the tiles' data
    /// origin and clears or reloads them as the type's rules say. Returns
    /// what the tiles cover, or `None` while the source has nothing to make
    /// tiles from.
    fn update(
        &mut self,
        previous: &Source,
        source: &Source,
        tiles: &mut SourceTiles,
        ctx: &mut StoreContext,
        update: &OriginUpdate<'_, '_>,
    ) -> Option<Cover>;

    /// The value of a feature extension of a feature of the source's tiles;
    /// a null value for a type without extensions.
    fn query_feature_extensions(
        &self,
        _feature: &mln_util::feature::Feature,
        _extension: &str,
        _extension_field: &str,
        _args: Option<&feature::PropertyMap>,
    ) -> Result<FeatureExtensionValue, Error> {
        Ok(FeatureExtensionValue::default())
    }

    /// The highest zoom level that has tiles, as far as the origin knows.
    fn max_zoom(&self) -> u8 {
        constants::TERRAIN_RGB_MAXZOOM
    }
}

/// A source type, whichever it is, that the renderer draws.
pub(crate) trait ErasedSourceType: ErasedStyleSourceType {
    /// [`SourceType::render_source`].
    fn create_render_source(&self, source: &Source, store: &mut TileStore) -> Box<dyn RenderSource>;
}

impl<T: SourceType> ErasedSourceType for SourceTypeEntry<T> {
    fn create_render_source(&self, source: &Source, store: &mut TileStore) -> Box<dyn RenderSource> {
        T::render_source(source, store)
    }
}

/// The source type `T` as a value.
pub(crate) const fn entry<T: SourceType>() -> &'static dyn ErasedSourceType {
    SourceTypeEntry::<T>::INSTANCE
}

// MARK: - Tilesets

/// The tiles of a tileset source type.
pub(crate) trait TilesetTiles: TilesetKind {
    const TILE_KIND: TileKind;
    /// Whether the type's tiles are vector tiles: always the default size,
    /// in the encoding that the tileset names.
    const VECTOR: bool;
}

impl TilesetTiles for VectorKind {
    const TILE_KIND: TileKind = TileKind::Geometry;
    const VECTOR: bool = true;
}

impl TilesetTiles for RasterKind {
    const TILE_KIND: TileKind = TileKind::Raster;
    const VECTOR: bool = false;
}

impl TilesetTiles for RasterDemKind {
    const TILE_KIND: TileKind = TileKind::Dem(RasterEncoding::Mapbox);
    const VECTOR: bool = false;
}

impl<K: TilesetTiles> SourceType for TilesetSourceType<K> {
    fn render_source(source: &Source, store: &mut TileStore) -> Box<dyn RenderSource> {
        let origin = TilesetOrigin::<K> { tileset: None, kind: PhantomData };
        Box::new(RenderTileSource::new(source.clone(), K::TILE_KIND, false, Box::new(origin), store))
    }
}

/// A tileset source's tiles: the tileset they were made from.
struct TilesetOrigin<K> {
    tileset: Option<Tileset>,
    kind: PhantomData<fn() -> K>,
}

impl<K: TilesetTiles> TileOrigin for TilesetOrigin<K> {
    fn update(
        &mut self,
        _previous: &Source,
        source: &Source,
        tiles: &mut SourceTiles,
        ctx: &mut StoreContext,
        update: &OriginUpdate<'_, '_>,
    ) -> Option<Cover> {
        let data = SourceData::<TilesetSourceType<K>>::recover(source)?;
        let parameters = update.parameters;
        let loaded = data.description.clone();
        // In Continuous mode, keep the existing tiles if the new tileset is
        // not yet available, thus providing smart style transitions without
        // flickering. In other modes, allow clearing the tiles first, before
        // the early return in order to avoid render tests being flaky.
        let can_update_tileset = loaded.is_some() || parameters.mode != MapMode::Continuous;
        if can_update_tileset && self.tileset != loaded {
            self.tileset = loaded;
            // TODO: this removes existing buckets, and will cause flickering.
            // Should instead refresh tile data in place.
            tiles.clear_all(ctx);
            tiles.info.origin = None;
        }
        // A tileset without tiles makes no tiles, and the source counts as
        // loaded until its tiles are set.
        let tileset = self.tileset.clone().filter(|tileset| !tileset.tiles.is_empty())?;
        let encoding = if K::VECTOR && tileset.vector_encoding == Some(VectorEncoding::MLT) {
            // The encoding is decided once, from the first tileset (`isMLT`).
            VectorTileEncoding::Mlt { fast_pfor_enabled: update.fast_pfor_enabled }
        } else {
            VectorTileEncoding::Mvt
        };
        if let TileKind::Dem(_) = tiles.info.kind {
            tiles.info.kind = TileKind::Dem(tileset.raster_encoding.unwrap_or(RasterEncoding::Mapbox));
        }
        let file_source = parameters.file_source.clone();
        let current = matches!(
            &tiles.info.origin,
            Some(DataOrigin::Tileset { tileset: origin, encoding: current, file_source: current_file_source })
                if *origin == tileset
                    && *current == encoding
                    && same_file_source(current_file_source.as_ref(), file_source.as_ref())
        );
        if !current {
            tiles.info.origin = Some(DataOrigin::Tileset { tileset: tileset.clone(), encoding, file_source });
        }
        let tile_size = if K::VECTOR { constants::TILE_SIZE_I } else { data.options.tile_size };
        Some(Cover { tile_size, zoom_range: tileset.zoom_range, bounds: tileset.bounds })
    }

    fn max_zoom(&self) -> u8 {
        self.tileset.as_ref().map_or(constants::TERRAIN_RGB_MAXZOOM, |tileset| tileset.zoom_range.max)
    }
}

/// Whether `a` and `b` are both absent or the same file source.
fn same_file_source(a: Option<&Arc<dyn FileSource>>, b: Option<&Arc<dyn FileSource>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)),
        _ => false,
    }
}

// MARK: - GeoJSON

impl SourceType for GeoJsonSourceType {
    fn render_source(source: &Source, store: &mut TileStore) -> Box<dyn RenderSource> {
        let origin = GeoJsonOrigin { data: None, synchronous: false };
        Box::new(RenderTileSource::new(source.clone(), TileKind::Geometry, true, Box::new(origin), store))
    }
}

/// A GeoJSON source's tiles: the data they were made from, and whether they
/// were made with synchronous updates.
struct GeoJsonOrigin {
    data: Option<Weak<dyn GeoJSONData>>,
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

impl TileOrigin for GeoJsonOrigin {
    fn update(
        &mut self,
        _previous: &Source,
        source: &Source,
        tiles: &mut SourceTiles,
        ctx: &mut StoreContext,
        update: &OriginUpdate<'_, '_>,
    ) -> Option<Cover> {
        let source = SourceData::<GeoJsonSourceType>::recover(source)?;
        let parameters = update.parameters;
        // A tile lays out in the mode it was created with, so a mode change
        // recreates the tiles.
        if parameters.is_update_synchronous != self.synchronous {
            self.synchronous = parameters.is_update_synchronous;
            tiles.clear_all(ctx);
        }
        let data = source.description.clone();
        let previous = self.data.as_ref().and_then(Weak::upgrade);
        if !same_data(previous.as_ref(), data.as_ref()) {
            self.data = data.as_ref().map(Arc::downgrade);
            tiles.info.origin = data.as_ref().map(|data| DataOrigin::GeoJson(Arc::clone(data)));
            tiles.info.synchronous = parameters.is_update_synchronous;
            if parameters.mode != MapMode::Continuous {
                // Clearing the tiles in order to avoid render tests being
                // flaky.
                tiles.clear_all(ctx);
            } else if let Some(data) = &data {
                tiles.reduce_memory_use(ctx);
                let max_z = source.options.get_zoom_range().max;
                let SourceTiles { info, tiles, .. } = tiles;
                for tile in tiles.values_mut().filter(|tile| !tile.cached && tile.id.canonical.z <= max_z) {
                    info.load_geojson(tile, ctx, data, update.needs_relayout);
                }
            }
        }
        data?;
        Some(Cover { tile_size: constants::TILE_SIZE_I, zoom_range: source.options.get_zoom_range(), bounds: None })
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
        let Some(json_data) = self.data.as_ref().and_then(Weak::upgrade) else {
            return Ok(FeatureExtensionValue::default());
        };
        supercluster_extension(&*json_data, extension_field, cluster_id as u32, args)
            .unwrap_or_else(|| Ok(FeatureExtensionValue::default()))
    }
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

// MARK: - Images

impl SourceType for ImageSourceType {
    fn render_source(source: &Source, _store: &mut TileStore) -> Box<dyn RenderSource> {
        Box::new(RenderImageSource::new(source.clone()))
    }
}

// MARK: - Custom sources

impl SourceType for CustomGeometrySourceType {
    fn render_source(source: &Source, store: &mut TileStore) -> Box<dyn RenderSource> {
        Box::new(RenderTileSource::new(source.clone(), TileKind::Geometry, true, Box::new(CustomGeometryOrigin), store))
    }
}

impl SourceType for CustomVectorSourceType {
    fn render_source(source: &Source, store: &mut TileStore) -> Box<dyn RenderSource> {
        Box::new(RenderTileSource::new(source.clone(), TileKind::Geometry, false, Box::new(CustomVectorOrigin), store))
    }
}

/// Points the tiles of a custom source at its loader. New options, zoom
/// range or loader clear them.
fn update_custom<T: StyleSourceType>(
    previous: &Source,
    source: &Source,
    tiles: &mut SourceTiles,
    ctx: &mut StoreContext,
    differs: impl FnOnce(&SourceData<T>, &SourceData<T>) -> bool,
    loader: impl FnOnce(&SourceData<T>) -> Option<Arc<dyn CustomLoader>>,
    zoom_range: impl FnOnce(&SourceData<T>) -> (u16, Range<u8>),
) -> Option<Cover> {
    let data = SourceData::<T>::recover(source)?;
    let same = Source::ptr_eq(previous, source);
    let differs = SourceData::<T>::recover(previous).is_none_or(|previous| differs(data, previous));
    if !same && differs {
        tiles.clear_all(ctx);
    }
    let loader = loader(data);
    let has_loader = loader.is_some();
    if tiles.info.origin.is_none() || !same {
        tiles.info.origin = loader.map(DataOrigin::Custom);
    }
    let (tile_size, zoom_range) = zoom_range(data);
    has_loader.then_some(Cover { tile_size, zoom_range, bounds: None })
}

/// A custom geometry source's tiles, which take their features from its
/// loader.
struct CustomGeometryOrigin;

impl TileOrigin for CustomGeometryOrigin {
    fn update(
        &mut self,
        previous: &Source,
        source: &Source,
        tiles: &mut SourceTiles,
        ctx: &mut StoreContext,
        _update: &OriginUpdate<'_, '_>,
    ) -> Option<Cover> {
        update_custom::<CustomGeometrySourceType>(
            previous,
            source,
            tiles,
            ctx,
            SourceData::<CustomGeometrySourceType>::differs,
            |data| {
                data.description.as_ref().map(|loader| -> Arc<dyn CustomLoader> {
                    Arc::new(CustomLoaderOf {
                        loader: loader.clone(),
                        convert: Arc::new(|features: Arc<TileFeatures>| Some(TileData::Features(features))),
                    })
                })
            },
            |data| (data.options.tile_options.tile_size, data.options.zoom_range),
        )
    }
}

/// A custom vector source's tiles, which decode the bytes its loader gives.
struct CustomVectorOrigin;

impl TileOrigin for CustomVectorOrigin {
    fn update(
        &mut self,
        previous: &Source,
        source: &Source,
        tiles: &mut SourceTiles,
        ctx: &mut StoreContext,
        update: &OriginUpdate<'_, '_>,
    ) -> Option<Cover> {
        let fast_pfor_enabled = update.fast_pfor_enabled;
        update_custom::<CustomVectorSourceType>(
            previous,
            source,
            tiles,
            ctx,
            SourceData::<CustomVectorSourceType>::differs,
            |data| {
                data.description.as_ref().map(|loader| -> Arc<dyn CustomLoader> {
                    Arc::new(CustomLoaderOf {
                        loader: loader.clone(),
                        convert: Arc::new(move |(data, format): CustomVectorTileData| {
                            data.map(|data| match format {
                                TileDataFormat::Mvt => TileData::Mvt(mln_tile::MvtTile::new(data)),
                                TileDataFormat::Mlt => TileData::Mlt(mln_tile::MltTile::new(data, fast_pfor_enabled)),
                            })
                        }),
                    })
                })
            },
            |data| (constants::TILE_SIZE_I, data.options.zoom_range),
        )
    }
}
