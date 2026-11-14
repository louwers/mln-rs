//! Ports `mapbox::feature`: mapbox/feature.hpp of geometry.hpp (the C++
//! core's vendor/maplibre-native-base/deps/geometry.hpp), which
//! `include/mln/util/feature.hpp` re-exports into the `mln` namespace:
//! [`Value`] (`mapbox::feature::value`, also `mapbox::base::Value`),
//! [`Identifier`] and [`Feature`], whose geometry is `geo_types`'. The
//! [`geojson`] module converts the `geojson` crate's features to these.
//!
//! Deliberately not `serde_json::Value`: [`Value`] keeps the `u64`/`i64`/`f64`
//! distinction that the expression and conversion code relies on.

mod feature;
pub mod geojson;
mod value;

pub use feature::{Feature, Identifier, PropertyMap};
pub use value::{Value, ValueObject};
