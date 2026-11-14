// rapidjson parsing is serde_json (as everywhere in the port); `IsDouble()`
// is `is_f64()` (a number written with a fraction or exponent, or an integer
// too large for 64 bits).

use geo_types::Geometry;
use mln_util::error::Error;
#[cfg(any(test, feature = "sqlite"))]
use mln_util::geo::LatLng;
use mln_util::geo::LatLngBounds;
#[cfg(any(test, feature = "sqlite"))]
use serde_json::{Map, Value as Json};

use crate::response::ResourceError;

fn check_definition(min_zoom: f64, max_zoom: f64, pixel_ratio: f32) -> Result<(), Error> {
    if !min_zoom.is_finite() || min_zoom < 0.0 || max_zoom.is_nan() || max_zoom < min_zoom {
        return Err(Error::invalid_argument("offline region zoom range is invalid"));
    }
    if !pixel_ratio.is_finite() || pixel_ratio < 0.0 {
        return Err(Error::invalid_argument("offline region pixel ratio is invalid"));
    }
    Ok(())
}

/// An offline region defined by a style URL, geographic bounding box, zoom
/// range, and device pixel ratio.
///
/// Both minZoom and maxZoom must be ≥ 0, and maxZoom must be ≥ minZoom.
///
/// maxZoom may be ∞, in which case for each tile source, the region will
/// include tiles from minZoom up to the maximum zoom level provided by that
/// source.
///
/// pixelRatio must be ≥ 0 and should typically be 1.0 or 2.0.
#[derive(Clone, Debug, PartialEq)]
pub struct OfflineTilePyramidRegionDefinition {
    pub(crate) style_url: String,
    pub bounds: LatLngBounds,
    pub(crate) min_zoom: f64,
    pub(crate) max_zoom: f64,
    pub(crate) pixel_ratio: f32,
    pub(crate) include_ideographs: bool,
}

impl OfflineTilePyramidRegionDefinition {
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// negative or reversed zoom range, or a negative or infinite pixel ratio.
    pub fn new(
        style_url: impl Into<String>,
        bounds: LatLngBounds,
        min_zoom: f64,
        max_zoom: f64,
        pixel_ratio: f32,
        include_ideographs: bool,
    ) -> Result<Self, Error> {
        check_definition(min_zoom, max_zoom, pixel_ratio)?;
        Ok(OfflineTilePyramidRegionDefinition {
            style_url: style_url.into(),
            bounds,
            min_zoom,
            max_zoom,
            pixel_ratio,
            include_ideographs,
        })
    }
}

/// An offline region defined by a style URL, geometry, zoom range, and
/// device pixel ratio.
///
/// Both minZoom and maxZoom must be ≥ 0, and maxZoom must be ≥ minZoom.
///
/// maxZoom may be ∞, in which case for each tile source, the region will
/// include tiles from minZoom up to the maximum zoom level provided by that
/// source.
///
/// pixelRatio must be ≥ 0 and should typically be 1.0 or 2.0.
#[derive(Clone, Debug, PartialEq)]
pub struct OfflineGeometryRegionDefinition {
    pub(crate) style_url: String,
    pub geometry: Geometry<f64>,
    pub(crate) min_zoom: f64,
    pub(crate) max_zoom: f64,
    pub(crate) pixel_ratio: f32,
    pub(crate) include_ideographs: bool,
}

impl OfflineGeometryRegionDefinition {
    /// Fails with [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) for a
    /// negative or reversed zoom range, or a negative or infinite pixel ratio.
    pub fn new(
        style_url: impl Into<String>,
        geometry: Geometry<f64>,
        min_zoom: f64,
        max_zoom: f64,
        pixel_ratio: f32,
        include_ideographs: bool,
    ) -> Result<Self, Error> {
        check_definition(min_zoom, max_zoom, pixel_ratio)?;
        Ok(OfflineGeometryRegionDefinition {
            style_url: style_url.into(),
            geometry,
            min_zoom,
            max_zoom,
            pixel_ratio,
            include_ideographs,
        })
    }
}

/// The offline region definition types supported
/// (`std::variant<OfflineTilePyramidRegionDefinition, OfflineGeometryRegionDefinition>`).
#[derive(Clone, Debug, PartialEq)]
pub enum OfflineRegionDefinition {
    TilePyramid(OfflineTilePyramidRegionDefinition),
    Geometry(OfflineGeometryRegionDefinition),
}

impl From<OfflineTilePyramidRegionDefinition> for OfflineRegionDefinition {
    fn from(definition: OfflineTilePyramidRegionDefinition) -> Self {
        OfflineRegionDefinition::TilePyramid(definition)
    }
}

impl From<OfflineGeometryRegionDefinition> for OfflineRegionDefinition {
    fn from(definition: OfflineGeometryRegionDefinition) -> Self {
        OfflineRegionDefinition::Geometry(definition)
    }
}

impl OfflineRegionDefinition {
    // The common members, for the C++ `std::visit([](auto& region) {...})`.

    #[must_use]
    pub fn style_url(&self) -> &str {
        match self {
            OfflineRegionDefinition::TilePyramid(r) => &r.style_url,
            OfflineRegionDefinition::Geometry(r) => &r.style_url,
        }
    }

    #[must_use]
    pub fn min_zoom(&self) -> f64 {
        match self {
            OfflineRegionDefinition::TilePyramid(r) => r.min_zoom,
            OfflineRegionDefinition::Geometry(r) => r.min_zoom,
        }
    }

    #[must_use]
    pub fn max_zoom(&self) -> f64 {
        match self {
            OfflineRegionDefinition::TilePyramid(r) => r.max_zoom,
            OfflineRegionDefinition::Geometry(r) => r.max_zoom,
        }
    }

    #[must_use]
    pub fn pixel_ratio(&self) -> f32 {
        match self {
            OfflineRegionDefinition::TilePyramid(r) => r.pixel_ratio,
            OfflineRegionDefinition::Geometry(r) => r.pixel_ratio,
        }
    }

    #[must_use]
    pub fn include_ideographs(&self) -> bool {
        match self {
            OfflineRegionDefinition::TilePyramid(r) => r.include_ideographs,
            OfflineRegionDefinition::Geometry(r) => r.include_ideographs,
        }
    }
}

/// Decodes a region definition from JSON. A definition that isn't one fails
/// with [`ErrorKind::Native`](mln_util::ErrorKind), and one
/// with invalid values with the constructors' error.
#[cfg(any(test, feature = "sqlite"))]
pub(crate) fn decode_offline_region_definition(region: &[u8]) -> Result<OfflineRegionDefinition, Error> {
    let malformed = || Error::native("Malformed offline region definition");

    // `doc.Parse<0>(region.c_str())` (stops at the first NUL byte).
    let end = region.iter().position(|&b| b == 0).unwrap_or(region.len());
    let doc: Option<Json> = serde_json::from_slice(&region[..end]).ok();
    let Some(doc) = doc else {
        return Err(malformed());
    };
    // `HasMember` on a non-object asserts in C++; a definition is always an
    // object.
    let member = |name: &str| doc.as_object().and_then(|o| o.get(name));
    let is_double = |v: Option<&Json>| v.is_some_and(Json::is_f64);

    // validation

    let has_valid_bounds =
        || member("bounds").and_then(Json::as_array).is_some_and(|b| b.len() == 4 && b.iter().all(Json::is_f64));

    let has_valid_geometry = || member("geometry").is_some_and(Json::is_object);

    if !member("style_url").is_some_and(Json::is_string)
        || !(has_valid_bounds() || has_valid_geometry())
        || !is_double(member("min_zoom"))
        || (member("max_zoom").is_some() && !is_double(member("max_zoom")))
        || !is_double(member("pixel_ratio"))
        || (member("include_ideographs").is_some() && !member("include_ideographs").is_some_and(Json::is_boolean))
    {
        return Err(malformed());
    }

    // Common properties

    let style_url = member("style_url").and_then(Json::as_str).unwrap_or_default().to_owned();
    let min_zoom = member("min_zoom").and_then(Json::as_f64).unwrap_or_default();
    let max_zoom = member("max_zoom").map_or(f64::INFINITY, |v| v.as_f64().unwrap_or_default());
    let pixel_ratio = member("pixel_ratio").and_then(Json::as_f64).unwrap_or_default() as f32;
    let include_ideographs = member("include_ideographs").is_none_or(|v| v.as_bool().unwrap_or_default());

    if let Some(bounds) = member("bounds") {
        // `bounds` is present but may be invalid when `geometry` is valid:
        // C++ then reads the members of whatever it holds (undefined for a
        // non-array); the port treats that as malformed.
        let bounds = bounds.as_array().filter(|b| b.len() == 4).ok_or_else(malformed)?;
        let get = |i: usize| bounds[i].as_f64().unwrap_or_default();
        let latlng = |lat: f64, lon: f64| LatLng::new(lat, lon);
        Ok(OfflineTilePyramidRegionDefinition::new(
            style_url,
            LatLngBounds::hull(latlng(get(0), get(1))?, latlng(get(2), get(3))?),
            min_zoom,
            max_zoom,
            pixel_ratio,
            include_ideographs,
        )?
        .into())
    } else {
        // A null geometry covers no tiles, as an empty collection does.
        let geometry = match member("geometry").expect("validated geometry") {
            Json::Null => Geometry::GeometryCollection(geo_types::GeometryCollection(Vec::new())),
            json => serde_json::from_value::<geojson::Geometry>(json.clone())
                .map_err(geojson::Error::from)
                .and_then(feature::geojson::geometry)
                .map_err(|e| Error::native(e.to_string()))?,
        };
        Ok(OfflineGeometryRegionDefinition::new(
            style_url,
            geometry,
            min_zoom,
            max_zoom,
            pixel_ratio,
            include_ideographs,
        )?
        .into())
    }
}

#[cfg(any(test, feature = "sqlite"))]
#[must_use]
pub(crate) fn encode_offline_region_definition(region: &OfflineRegionDefinition) -> String {
    let mut json = Map::new();

    // Encode common properties
    json.insert("style_url".into(), region.style_url().into());
    json.insert("min_zoom".into(), region.min_zoom().into());
    if region.max_zoom().is_finite() {
        json.insert("max_zoom".into(), region.max_zoom().into());
    }
    json.insert("pixel_ratio".into(), f64::from(region.pixel_ratio()).into());
    json.insert("include_ideographs".into(), region.include_ideographs().into());

    // Encode specific properties
    match region {
        OfflineRegionDefinition::TilePyramid(region) => {
            let bounds = &region.bounds;
            json.insert("bounds".into(), [bounds.south(), bounds.west(), bounds.north(), bounds.east()].into());
        }
        OfflineRegionDefinition::Geometry(region) => {
            let geometry = geojson::Geometry::new((&region.geometry).into());
            json.insert("geometry".into(), serde_json::to_value(geometry).unwrap_or_default());
        }
    }

    Json::Object(json).to_string()
}

/// Arbitrary binary region metadata. The contents are opaque to the mbgl
/// implementation; it just stores and retrieves a BLOB.
pub type OfflineRegionMetadata = Vec<u8>;

/// A region is either inactive (not downloading, but previously-downloaded
/// resources are available for use), or active (resources are being
/// downloaded or will be downloaded, if necessary, when network access is
/// available).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OfflineRegionDownloadState {
    #[default]
    Inactive,
    Active,
}

/// A region's status: its active/inactive state, the number of resources
/// that have completed downloading and their total size in bytes, and the
/// total number of resources that are required.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OfflineRegionStatus {
    pub download_state: OfflineRegionDownloadState,

    /// The number of resources that have been fully downloaded and are ready
    /// for offline access.
    pub completed_resource_count: u64,

    /// The cumulative size, in bytes, of all resources (inclusive of tiles)
    /// that have been fully downloaded.
    pub completed_resource_size: u64,

    /// The number of tiles that are known to be required for this region.
    /// This is a subset of `completedResourceCount`.
    pub completed_tile_count: u64,

    /// The number of tiles that are known to be required for this region.
    pub required_tile_count: u64,

    /// The cumulative size, in bytes, of all tiles that have been fully
    /// downloaded. This is a subset of `completedResourceSize`.
    pub completed_tile_size: u64,

    /// The number of resources that are known to be required for this
    /// region. See the documentation for `requiredResourceCountIsPrecise`
    /// for an important caveat about this number.
    pub required_resource_count: u64,

    /// This property is true when the value of requiredResourceCount is a
    /// precise count of the number of required resources, and false when it
    /// is merely a lower bound.
    pub required_resource_count_is_precise: bool,
}

impl OfflineRegionStatus {
    #[must_use]
    pub fn complete(&self) -> bool {
        self.completed_resource_count >= self.required_resource_count
    }
}

/// A region can have a single observer, which gets notified whenever a
/// change to the region's status occurs. The methods run on the database
/// thread. They take `&self`: the download calls them re-entrantly, e.g. an
/// observer that deactivates the download from `status_changed` gets a
/// nested `status_changed`.
pub trait OfflineRegionObserver {
    /// Notified of a change in the status of an offline region.
    fn status_changed(&self, _status: OfflineRegionStatus) {}

    /// Notified of errors encountered while downloading regional resources.
    fn response_error(&self, _error: ResourceError) {}
}

/// A region in the offline database.
#[derive(Clone, Debug, PartialEq)]
pub struct OfflineRegion {
    /// The database's id for the region.
    pub id: i64,
    /// The area and zoom levels the region downloads.
    pub definition: OfflineRegionDefinition,
    /// The app's bytes for the region.
    pub metadata: OfflineRegionMetadata,
}

pub type OfflineRegions = Vec<OfflineRegion>;
