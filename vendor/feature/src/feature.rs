// Ports mapbox::feature::{identifier, feature} (mapbox/feature.hpp),
// aliased as mln::FeatureIdentifier / mln::Feature in
// include/mln/util/feature.hpp.

use geometry::Geometry;
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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Feature<T = f64> {
    pub geometry: Geometry<T>,
    pub properties: PropertyMap,
    pub id: Identifier,
}

impl<T> Feature<T> {
    pub fn new(geometry: Geometry<T>) -> Self {
        Feature { geometry, properties: PropertyMap::new(), id: Identifier::Null }
    }
}
