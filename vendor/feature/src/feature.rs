// Ports mapbox::feature::{identifier, feature} (mapbox/feature.hpp),
// aliased as mln::FeatureIdentifier / mln::Feature in
// include/mln/util/feature.hpp.

use geo_types::{CoordNum, Geometry};
use serde::{Deserialize, Serialize};

use crate::value::ValueObject;

/// `mapbox::feature::identifier`:
/// `variant<null, uint64_t, int64_t, double, string>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Identifier {
    #[default]
    Null,
    Uint(u64),
    Int(i64),
    Double(f64),
    String(String),
}

pub type PropertyMap = ValueObject;

/// `mapbox::feature::feature<T>`.
///
/// `geometry` is `None` where C++ holds an empty geometry, as for a GeoJSON
/// feature whose geometry is `null`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feature<T: CoordNum = f64> {
    pub geometry: Option<Geometry<T>>,
    pub properties: PropertyMap,
    pub id: Identifier,
}

impl<T: CoordNum> Default for Feature<T> {
    fn default() -> Self {
        Feature { geometry: None, properties: PropertyMap::new(), id: Identifier::Null }
    }
}

impl<T: CoordNum> Feature<T> {
    pub fn new(geometry: impl Into<Geometry<T>>) -> Self {
        Feature { geometry: Some(geometry.into()), properties: PropertyMap::new(), id: Identifier::Null }
    }
}
