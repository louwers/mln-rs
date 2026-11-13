// The geometry algorithms on these types (ring classification, coordinate
// conversion, polygon fixup, ...) are in `mln_tile`.

use std::ops::{Deref, DerefMut, Range};
use std::sync::Arc;

use feature::{Identifier, PropertyMap, Value};
use geometry::Point;
use mln_util::error::Error;
use mln_util::geometry::FeatureType;
use mln_util::serialization::payload::shared_bytes;
use serde::{Deserialize, Serialize};

/// Normalized vector tile coordinates. Each geometry coordinate represents a
/// point in a bidimensional space, varying from -V...0...+V, where V is the
/// maximum extent applicable.
pub type GeometryCoordinate = Point<i16>;

pub type GeometryCoordinates = Vec<GeometryCoordinate>;

/// The rings or lines of a feature's geometry, with the triangles of a
/// pre-tessellated polygon.
///
/// `Clone` keeps the triangles, as the C++ `clone()` does (C++ makes the
/// copy constructor private). `==` compares only the rings, as the
/// `std::vector` base does in C++.
#[derive(Clone, Debug, Default)]
pub struct GeometryCollection {
    rings: Vec<GeometryCoordinates>,
    /// The pre-tessellated triangles of an MLT polygon feature: C++
    /// `std::shared_ptr<const mlt::MapLibreTile> triangleOwner` plus the
    /// `std::span<const std::uint32_t> triangles` into its index buffer; here
    /// the shared buffer and the span's range in it.
    triangles: Option<(Arc<[u32]>, Range<usize>)>,
}

impl GeometryCollection {
    #[must_use]
    pub const fn new() -> Self {
        GeometryCollection { rings: Vec::new(), triangles: None }
    }

    /// The triangles of a pre-tessellated polygon: three indices per
    /// triangle into the vertices of the rings, numbered from 0 across the
    /// rings in order. Empty unless the tile carried triangles. Nothing
    /// checks that the indices are in range.
    #[must_use]
    pub fn get_triangles(&self) -> &[u32] {
        match &self.triangles {
            Some((owner, range)) => &owner[range.clone()],
            None => &[],
        }
    }

    /// Sets the triangles to `owner[range]`. Panics if `range` is out of
    /// bounds.
    pub fn set_triangles(&mut self, owner: Arc<[u32]>, range: Range<usize>) {
        assert!(range.start <= range.end && range.end <= owner.len());
        self.triangles = Some((owner, range));
    }
}

impl PartialEq for GeometryCollection {
    fn eq(&self, other: &Self) -> bool {
        self.rings == other.rings
    }
}

impl From<Vec<GeometryCoordinates>> for GeometryCollection {
    fn from(rings: Vec<GeometryCoordinates>) -> Self {
        GeometryCollection { rings, triangles: None }
    }
}

impl Deref for GeometryCollection {
    type Target = Vec<GeometryCoordinates>;
    fn deref(&self) -> &Self::Target {
        &self.rings
    }
}

impl DerefMut for GeometryCollection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rings
    }
}

static EMPTY_PROPERTIES: PropertyMap = PropertyMap::new();
static EMPTY_GEOMETRIES: GeometryCollection = GeometryCollection::new();

/// A feature of a tile layer.
///
/// C++ `getValue`, `getProperties` and `getGeometries` throw on malformed
/// tiles, from inside expression evaluation as well. Here they return null
/// or empty instead and keep the first exception, which
/// [`error`](Self::error) returns: the code that C++ lets the exception
/// escape from (the tile worker, layouts, queries) checks it after using
/// the feature and returns it as its error. A tile errors for the same
/// inputs and message as in C++; the rest of the work on the failing
/// feature runs on the empty value before the check, and its result is
/// dropped.
pub trait GeometryTileFeature {
    fn get_type(&self) -> FeatureType;

    fn get_value(&self, key: &str) -> Option<Value>;

    /// The feature's properties; empty by default.
    fn get_properties(&self) -> &PropertyMap {
        &EMPTY_PROPERTIES
    }

    /// The feature's id; [`Identifier::Null`] by default.
    fn get_id(&self) -> Identifier {
        Identifier::Null
    }

    /// The feature's geometries; empty by default.
    fn get_geometries(&self) -> &GeometryCollection {
        &EMPTY_GEOMETRIES
    }

    /// The first exception a C++ `getValue`, `getProperties` or
    /// `getGeometries` call on this feature would have thrown (port-only).
    fn error(&self) -> Option<Error> {
        None
    }

    /// `Err` with [`error`](Self::error): where the C++ exception escapes.
    fn check(&self) -> Result<(), Error> {
        self.error().map_or(Ok(()), Err)
    }
}

pub trait GeometryTileLayer {
    fn feature_count(&self) -> usize;

    /// Returns the feature object at the given position within the layer.
    /// `Err`: the exception C++ throws (malformed tile, index past the end).
    fn get_feature(&self, index: usize) -> Result<Box<dyn GeometryTileFeature + '_>, Error>;

    fn get_name(&self) -> String;

    /// Port-only: the features in groups whose members have the same
    /// geometry type and the same values under each of `keys` (or lack each
    /// alike), so that an expression reading only those of a feature gives
    /// every member the same result. `None` if the layer can't group them
    /// cheaply, or nearly every feature would be a group of its own.
    fn group_features(&self, keys: &[String]) -> Option<FeatureGroups> {
        let _ = keys;
        None
    }
}

/// Port-only: [`GeometryTileLayer::group_features`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureGroups {
    /// Each feature's group.
    pub of_feature: Vec<u32>,
    /// Each group's first feature; groups are numbered in feature order.
    pub first: Vec<usize>,
}

pub trait GeometryTileData {
    fn clone_data(&self) -> Box<dyn GeometryTileData>;

    /// Returns the layer with the given name. `Err`: the exception C++
    /// throws (malformed tile).
    fn get_layer(&self, name: &str) -> Result<Option<Box<dyn GeometryTileLayer + '_>>, Error>;

    /// Port-only: what the data is made from, for a tile worker in another
    /// wasm instance, which makes the data again (`mln_renderer`'s
    /// `tile/worker_codec)`: a vector tile's bytes or a GeoJSON tile's
    /// features. `None` for data that no longer has its bytes.
    fn encoded(&self) -> Option<EncodedTileData> {
        None
    }
}

/// Port-only: what tile data is made from (`GeometryTileData::encoded`).
/// The bytes travel beside a message (`mln_util::serialization::payload`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EncodedTileData {
    Mvt(#[serde(with = "shared_bytes")] Arc<Vec<u8>>),
    Mlt {
        #[serde(with = "shared_bytes")]
        data: Arc<Vec<u8>>,
        fast_pfor_enabled: bool,
    },
    /// `GeoJSONTileData`'s features.
    GeoJson(Arc<Vec<feature::Feature<i16>>>),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Feature;

    impl GeometryTileFeature for Feature {
        fn get_type(&self) -> FeatureType {
            FeatureType::Point
        }
        fn get_value(&self, _key: &str) -> Option<Value> {
            None
        }
    }

    #[test]
    fn defaults() {
        let f = Feature;
        assert!(f.get_properties().is_empty());
        assert_eq!(f.get_id(), Identifier::Null);
        assert!(f.get_geometries().is_empty());
        assert!(f.error().is_none());
        let mut c = GeometryCollection::new();
        c.push(vec![Point::new(1, 2)]);
        assert_eq!(c.clone()[0][0], Point::new(1, 2));
    }

    #[test]
    fn triangles() {
        let mut c = GeometryCollection::from(vec![vec![Point::new(0, 0), Point::new(1, 0), Point::new(0, 1)]]);
        assert_eq!(c.get_triangles(), []);
        let owner: Arc<[u32]> = Arc::from(vec![9, 9, 0, 1, 2, 9]);
        c.set_triangles(owner, 2..5);
        assert_eq!(c.get_triangles(), &[0, 1, 2]);
        // The copy keeps them; == ignores them, as the C++ vector base does.
        assert_eq!(c.clone().get_triangles(), &[0, 1, 2]);
        assert_eq!(c, GeometryCollection::from(c.to_vec()));
    }
}
