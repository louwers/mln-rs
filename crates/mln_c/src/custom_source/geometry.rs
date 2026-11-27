//! Custom geometry sources, whose tiles' GeoJSON the host gives on demand,
//! which each tile cuts as a GeoJSON source's would.

use std::sync::Arc;

use geojson::GeoJson;
use mln_builtins::sources::geojson::{Coordinates, TileFeatures, tile};
use mln_style::constants;
use mln_style::crs::{CrsMapping, same_mapping};
use mln_style::layer::SourceKind;
use mln_style::range::Range;
use mln_style::source::{SourceData, SourceTypeInfo, StyleSourceType};
use mln_style::sources::geojson::features_to_world;
use mln_style::tile_id::CanonicalTileID;

use super::TileFunctions;
use crate::log_target;

/// How the host's GeoJSON is cut into tiles, as for a GeoJSON source.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TileOptions {
    pub(crate) tolerance: f64,
    /// The size of a tile, in pixels: a power of two up to the tile extent
    /// (8192).
    pub(crate) tile_size: u16,
    pub(crate) buffer: u16,
    pub(crate) clip: bool,
    pub(crate) wrap: bool,
}

impl Default for TileOptions {
    fn default() -> Self {
        TileOptions { tolerance: 0.375, tile_size: constants::TILE_SIZE_I, buffer: 128, clip: false, wrap: false }
    }
}

/// The custom geometry source type, whose tiles' GeoJSON the host gives.
pub(crate) struct CustomGeometrySourceType;

/// A custom geometry source's options.
#[derive(Clone)]
pub(crate) struct CustomGeometrySourceOptions {
    pub(crate) tile_options: Arc<TileOptions>,
    pub(crate) zoom_range: Range<u8>,
    /// The host's functions that give the tiles' GeoJSON.
    pub(crate) tile_functions: Arc<TileFunctions<GeoJson>>,
    /// The registered projection that the source's style is drawn in, whose
    /// world square the host's GeoJSON is cut into tiles in and the regions
    /// it invalidates lie in; `None` in the built-in projections, which cut it
    /// in lng/lat. The style sets it when the source joins it and when its
    /// projection changes.
    pub(crate) crs: Option<Arc<CrsMapping>>,
}

impl std::fmt::Debug for CustomGeometrySourceOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomGeometrySourceOptions")
            .field("tile_options", &self.tile_options)
            .field("zoom_range", &self.zoom_range)
            .field("tile_functions", &self.tile_functions)
            .field("crs", &self.crs)
            .finish()
    }
}

impl StyleSourceType for CustomGeometrySourceType {
    const INFO: SourceTypeInfo =
        SourceTypeInfo { name: "custom-geometry", layer_kind: SourceKind::Geometry, from_json: false };

    type Options = CustomGeometrySourceOptions;

    /// The source with `crs` in its options, where its tiles are cut.
    fn in_projection(source: &SourceData<Self>, crs: Option<&Arc<CrsMapping>>) -> Option<SourceData<Self>> {
        if same_mapping(source.options.crs.as_ref(), crs) {
            return None;
        }
        let mut source = source.clone();
        source.options.crs = crs.cloned();
        Some(source)
    }
}

impl CustomGeometrySourceOptions {
    /// Whether tiles made for one source don't suit the other.
    pub(crate) fn differs(&self, other: &Self) -> bool {
        !Arc::ptr_eq(&self.tile_options, &other.tile_options)
            || self.zoom_range != other.zoom_range
            || !Arc::ptr_eq(&self.tile_functions, &other.tile_functions)
            || !same_mapping(self.crs.as_ref(), other.crs.as_ref())
    }
}

/// Cuts the host's GeoJSON for tile `tile_id` with a source's `options`, in
/// the world square of the registered projection `crs`, which doesn't wrap,
/// or in lng/lat with `None`. Anything but a non-empty feature collection is
/// an empty tile, and so is one whose geometry `geo_types` can't hold, which
/// is logged.
pub(crate) fn tile_features(
    geojson: GeoJson,
    tile_id: &CanonicalTileID,
    options: &TileOptions,
    crs: Option<&CrsMapping>,
) -> TileFeatures {
    if !matches!(&geojson, GeoJson::FeatureCollection(c) if !c.features.is_empty()) {
        return TileFeatures::new();
    }
    let features = match mln_style::feature::geojson::features(geojson) {
        Ok(features) => features,
        Err(e) => {
            log::error!(target: log_target::GENERAL, "Invalid custom geometry tile data: {e}");
            return TileFeatures::new();
        }
    };
    let scale = f64::from(constants::EXTENT / i32::from(options.tile_size.max(1)));
    let (features, coordinates, wrap) = match crs {
        Some(crs) => (features_to_world(features, crs), Coordinates::UnitSquare, false),
        None => (features, Coordinates::LngLat, options.wrap),
    };
    let tile_options = tile::TileOptions {
        extent: constants::EXTENT as u16,
        buffer: (scale * f64::from(options.buffer)).round() as u16,
        tolerance: scale * options.tolerance,
        coordinates,
        ..Default::default()
    };
    tile::cut_tile(&features, tile_id.z, tile_id.x, tile_id.y, &tile_options, wrap, options.clip)
}

#[cfg(test)]
mod tests {
    use mln_style::Edit;
    use mln_style::source::Source;

    use super::*;
    use crate::custom_source::TileFunctions;
    use crate::registry::registry;

    /// The box from 0 to 20 E and 20 to 40 N, as a host gives it.
    fn box_from_the_host() -> GeoJson {
        r#"{"type": "FeatureCollection", "features": [{"type": "Feature", "properties": {},
            "geometry": {"type": "Polygon", "coordinates": [[[0, 20], [20, 20], [20, 40], [0, 40], [0, 20]]]}}]}"#
            .parse()
            .unwrap()
    }

    /// The corners, least and greatest, of the box that tile 0/0/0 of
    /// `features` has.
    fn box_corners(features: &TileFeatures) -> [(i16, i16); 2] {
        let [feature] = &features[..] else { panic!("one feature: {features:?}") };
        let Some(geo_types::Geometry::Polygon(polygon)) = &feature.geometry else { panic!("a polygon") };
        let ring = &polygon.exterior().0;
        let (xs, ys) = (ring.iter().map(|c| c.x), ring.iter().map(|c| c.y));
        [(xs.clone().min().unwrap(), ys.clone().min().unwrap()), (xs.max().unwrap(), ys.max().unwrap())]
    }

    #[test]
    fn the_hosts_geojson_is_cut_in_the_world_square_of_a_registered_projection() {
        let registry = registry();
        let identity = registry.style().projection("identity").unwrap();
        let root = CanonicalTileID::new(0, 0, 0);

        let tile = tile_features(box_from_the_host(), &root, &TileOptions::default(), Some(identity.as_ref()));

        // 0 to 20 E is 0.5 to 0.61 across the identity projection's square,
        // and 40 to 20 N 0.28 to 0.39 down it.
        assert_eq!([(4096, 2276), (5006, 3186)], box_corners(&tile));
        // Mercator's square spans 0 to 20 E from 0.5 to 0.56 across it, and
        // 40 to 20 N from 0.38 to 0.44 down it.
        let tile = tile_features(box_from_the_host(), &root, &TileOptions::default(), None);
        assert_eq!([(4096, 3101), (4551, 3631)], box_corners(&tile));
    }

    #[test]
    fn a_source_cuts_its_tiles_in_the_world_square_of_the_style_it_joins() {
        let options = CustomGeometrySourceOptions {
            tile_options: Arc::new(TileOptions::default()),
            zoom_range: Range::new(0, 18),
            tile_functions: Arc::new(TileFunctions { fetch_tile: None, cancel_tile: None }),
            crs: None,
        };
        let source = Source::new("custom", SourceData::<CustomGeometrySourceType>::new(options));
        let mut style = mln_style::Style::from_json(
            registry().style(),
            r#"{"version": 8, "projection": {"type": "identity"}, "sources": {}, "layers": []}"#,
        )
        .unwrap();

        style.apply(Edit::AddSource(source.clone())).unwrap();

        let data = |source| SourceData::<CustomGeometrySourceType>::recover(source).unwrap();
        let joined = data(style.get_source("custom").unwrap());
        assert_eq!(Some("identity"), joined.options.crs.as_ref().map(|crs| crs.crs().name.as_str()));
        assert!(joined.options.differs(&data(&source).options));
    }
}
