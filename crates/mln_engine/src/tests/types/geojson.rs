//! The tests' engine half of the GeoJSON source type: features from the
//! style's GeoJSON, the GeoJSON at the source's URL or the test's own,
//! described in the background and cut into tiles as the tiles start, off
//! the tiles' thread unless the source updates synchronously or the test
//! gives them. Unlike the built-in type's, it doesn't cluster, clip or
//! simplify: a tile takes whole each feature whose bounds meet the tile and
//! its buffer.

use std::sync::Arc;

use geo_types::{
    Coord, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon,
};
use geojson::GeoJson;
use mln_style::error::Error;
use mln_style::geo::{LatLng, WrapMode};
use mln_style::source::SourceData;
use mln_style::sources::GeoJsonSourceType;
use mln_style::sources::geojson::{GeoJSONOptions, GeoJsonSourceOptions};
use mln_style::tile_id::CanonicalTileID;
use mln_style::{Edit, Source, constants};

use crate::exec::pool::TaskGroup;
use crate::geo::projection::Projection;
use crate::map::MapMode;
use crate::source::{
    Cover, DataOrigin, Described, Fetch, GeometryTiles, Load, LoadContext, OfflineResources, OriginUpdate, Respond,
    SourceType, Supply, WorldCopy,
};
use crate::storage::Data;
use crate::tile::{GeoJsonFeature, TileData};

/// The tests' engine half of the GeoJSON source type.
pub(crate) struct TestGeoJsonSource;

/// The features of a tile.
type TileFeatures = dyn Fn(&CanonicalTileID) -> Arc<Vec<GeoJsonFeature>> + Send + Sync;

/// A test GeoJSON source's description: the features of each tile, and
/// whether it cuts them as they are asked for, work worth taking off the
/// tiles' thread.
pub(crate) struct TestGeoJson {
    tiles: Box<TileFeatures>,
    cuts: bool,
}

impl TestGeoJson {
    /// The features of `geojson`, cut into tiles with the buffer of
    /// `options`.
    pub(crate) fn new(geojson: GeoJson, options: &GeoJSONOptions) -> Result<Arc<Self>, Error> {
        let features =
            mln_style::feature::geojson::features(geojson).map_err(|e| Error::invalid_argument(e.to_string()))?;
        let buffer = f64::from(constants::EXTENT) / constants::TILE_SIZE_D * f64::from(options.buffer);
        Ok(Arc::new(TestGeoJson { tiles: Box::new(move |id| Arc::new(cut(&features, id, buffer))), cuts: true }))
    }

    /// `features` in every tile.
    pub(crate) fn every_tile(features: Vec<GeoJsonFeature>) -> Arc<Self> {
        let features = Arc::new(features);
        Arc::new(TestGeoJson { tiles: Box::new(move |_| Arc::clone(&features)), cuts: false })
    }

    /// The features of tile `id`.
    pub(crate) fn get_tile(&self, id: &CanonicalTileID) -> Arc<Vec<GeoJsonFeature>> {
        (self.tiles)(id)
    }

    /// The data of `text`, or `None` for text that isn't GeoJSON this cuts.
    fn of_text(text: &[u8], options: &GeoJSONOptions) -> Option<Arc<Self>> {
        Self::new(serde_json::from_slice(text).ok()?, options).ok()
    }
}

/// The features of `features` whose bounds meet tile `id` and `buffer`
/// (in tile units) around it, whole, in the tile's coordinates.
fn cut(features: &[mln_style::feature::Feature<f64>], id: &CanonicalTileID, buffer: f64) -> Vec<GeoJsonFeature> {
    let extent = f64::from(constants::EXTENT);
    let (min, max) = (-buffer, extent + buffer);
    features
        .iter()
        .filter_map(|feature| {
            let (mut low, mut high) =
                (Coord { x: f64::INFINITY, y: f64::INFINITY }, Coord { x: -f64::INFINITY, y: -f64::INFINITY });
            let geometry = map_coords(feature.geometry.as_ref()?, &mut |c| {
                let world =
                    Projection::project_zoom(&LatLng::saturating(c.y, c.x, WrapMode::Unwrapped), i32::from(id.z));
                let p = Coord { x: (world.x - f64::from(id.x)) * extent, y: (world.y - f64::from(id.y)) * extent };
                (low.x, low.y, high.x, high.y) = (low.x.min(p.x), low.y.min(p.y), high.x.max(p.x), high.y.max(p.y));
                Coord { x: p.x.round() as i16, y: p.y.round() as i16 }
            });
            (high.x >= min && low.x <= max && high.y >= min && low.y <= max).then(|| GeoJsonFeature {
                geometry: Some(geometry),
                properties: Arc::new(feature.properties.clone()),
                id: feature.id.clone(),
            })
        })
        .collect()
}

/// `geometry` with `f` of each of its coordinates.
fn map_coords(geometry: &Geometry<f64>, f: &mut dyn FnMut(Coord<f64>) -> Coord<i16>) -> Geometry<i16> {
    let mut line = |line: &LineString<f64>| LineString(line.0.iter().map(|&c| f(c)).collect());
    match geometry {
        Geometry::Point(p) => Point(f(p.0)).into(),
        Geometry::MultiPoint(points) => MultiPoint(points.iter().map(|p| Point(f(p.0))).collect()).into(),
        Geometry::Line(l) => line(&LineString(vec![l.start, l.end])).into(),
        Geometry::LineString(l) => line(l).into(),
        Geometry::MultiLineString(lines) => MultiLineString(lines.iter().map(line).collect()).into(),
        Geometry::Polygon(polygon) => map_polygon(polygon, &mut line).into(),
        Geometry::MultiPolygon(polygons) => {
            MultiPolygon(polygons.iter().map(|polygon| map_polygon(polygon, &mut line)).collect()).into()
        }
        Geometry::Rect(rect) => map_polygon(&rect.to_polygon(), &mut line).into(),
        Geometry::Triangle(triangle) => map_polygon(&triangle.to_polygon(), &mut line).into(),
        Geometry::GeometryCollection(geometries) => Geometry::GeometryCollection(GeometryCollection(
            geometries.iter().map(|geometry| map_coords(geometry, f)).collect(),
        )),
    }
}

/// `polygon` with `line` of each of its rings.
fn map_polygon(polygon: &Polygon<f64>, line: &mut impl FnMut(&LineString<f64>) -> LineString<i16>) -> Polygon<i16> {
    Polygon::new(line(polygon.exterior()), polygon.interiors().iter().map(line).collect())
}

impl SourceType for TestGeoJsonSource {
    type Style = GeoJsonSourceType;
    type Data = GeometryTiles;
    type Description = Arc<TestGeoJson>;

    const DESCRIBES_IN_BACKGROUND: bool = true;

    fn load(source: &SourceData<GeoJsonSourceType>) -> Load<Arc<TestGeoJson>> {
        match (&source.options.url, &source.options.inline) {
            (Some(url), _) => Load::Fetch(Fetch::Source(url.clone())),
            (None, Some(_)) => Load::Describe,
            (None, None) => Load::Done,
        }
    }

    /// GeoJSON that doesn't parse describes an empty source.
    fn describe(options: &GeoJsonSourceOptions, data: Option<Data>, respond: Respond<Described<Arc<TestGeoJson>>>) {
        respond(match (data, &options.inline) {
            (Some(data), _) => Ok(TestGeoJson::of_text(&data, &options.options)),
            (None, Some(inline)) => Ok(inline
                .as_ref()
                .as_ref()
                .ok()
                .and_then(|geojson| TestGeoJson::new(geojson.clone(), &options.options).ok())),
            (None, None) => Err(Error::native("unexpectedly empty GeoJSON")),
        });
    }

    fn attribution(source: &SourceData<GeoJsonSourceType>) -> Option<String> {
        source.options.options.attribution.clone()
    }

    fn offline(source: &SourceData<GeoJsonSourceType>) -> OfflineResources {
        match &source.options.url {
            Some(url) => OfflineResources::Source { url: url.clone() },
            None => OfflineResources::None,
        }
    }

    fn supply(_source: &Source) -> Supply<GeometryTiles> {
        Supply::tiles(TestGeoJsonOrigin { data: None, synchronous: false })
    }
}

/// The features of `geojson`, cut into tiles with `options`.
pub(crate) fn create_geojson_data(geojson: GeoJson, options: &GeoJSONOptions) -> Result<Arc<TestGeoJson>, Error> {
    TestGeoJson::new(geojson, options)
}

/// The data of `source`, if it's a GeoJSON source with data.
pub(crate) fn geojson_data(source: &Source) -> Option<&Arc<TestGeoJson>> {
    SourceData::<GeoJsonSourceType>::recover(source)?.description::<Arc<TestGeoJson>>()
}

/// The edit that replaces the data of the GeoJSON source `id`, which no
/// longer loads it from a URL or the style.
pub(crate) fn set_data(id: impl Into<String>, data: Option<Arc<TestGeoJson>>) -> Edit {
    Edit::update_source::<GeoJsonSourceType>(id, "a GeoJSON source", |source| {
        source.options.url = None;
        source.options.inline = None;
        source.description = data.map(|data| -> mln_style::source::ErasedDescription { Arc::new(data) });
        source.load += 1;
        true
    })
}

/// A test GeoJSON source's tiles: the data they were made from, and whether
/// they were made with synchronous updates.
struct TestGeoJsonOrigin {
    /// The data, and where its tiles are cut.
    data: Option<(Arc<TestGeoJson>, TaskGroup)>,
    synchronous: bool,
}

impl TestGeoJsonOrigin {
    /// Delivers the tile's features, after dropping its data and style for
    /// a `relayout`.
    fn load(&self, cx: &mut LoadContext<'_, '_, Self>, relayout: bool) {
        let Some((data, cuts)) = &self.data else {
            return;
        };
        if relayout {
            cx.reset();
        }
        let replier = cx.begin_load();
        let id = cx.id().canonical;
        if self.synchronous || !data.cuts {
            cx.deliver(Some(TileData::Features(data.get_tile(&id))));
        } else {
            let data = Arc::clone(data);
            cuts.spawn(move || replier.send(TileData::Features(data.get_tile(&id))));
        }
    }
}

impl DataOrigin for TestGeoJsonOrigin {
    type Data = GeometryTiles;
    type Tile = ();
    type Reply = TileData;

    /// New data loads the tiles again: on a continuous map in place, keeping
    /// what they show until it lays out, and otherwise as new tiles. A
    /// change of the synchronous updates makes the tiles anew.
    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let source = SourceData::<GeoJsonSourceType>::recover(update.source())?;
        let synchronous = source.options.synchronous_update;
        if synchronous != self.synchronous {
            self.synchronous = synchronous;
            update.clear_tiles();
        }
        let data = source.description::<Arc<TestGeoJson>>();
        let same = match (self.data.as_ref().map(|(data, _)| data), data) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        if !same {
            self.data = data.map(|data| (Arc::clone(data), TaskGroup::new(update.pool())));
            if update.map().mode != MapMode::Continuous {
                update.clear_tiles();
            } else if data.is_some() {
                update.clear_cache();
                let max_z = source.options.get_zoom_range().max;
                let relayout = update.needs_relayout();
                update.update_tiles(|id| id.canonical.z <= max_z, |(), cx| self.load(cx, relayout));
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

    fn start(&mut self, cx: &mut LoadContext<'_, '_, Self>, _copy: Option<WorldCopy<'_, ()>>) {
        self.load(cx, false);
    }

    fn receive(&mut self, (): &mut (), features: TileData, cx: &mut LoadContext<'_, '_, Self>) {
        cx.deliver(Some(features));
    }
}
