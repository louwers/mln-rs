//! Source types for the tests of this crate, which can't reach the built-in
//! source types outside it: `shapes` and `points`, two source types of
//! geometry, and `heights`, a source type of raster-dem tiles. Style JSON
//! makes one from any object, and its sources load nothing.

use std::sync::Arc;

use crate::conversion::{Convertible, Error};
use crate::expression::Functions;
use crate::layer_type::SourceKind;
use crate::source_type::{Source, SourceData, SourceTypeInfo, StyleSourceType};

pub(crate) struct Shapes;
pub(crate) struct Points;
pub(crate) struct Heights;

macro_rules! fixture_source_type {
    ($type:ident, $name:literal, $kind:ident) => {
        impl StyleSourceType for $type {
            const INFO: SourceTypeInfo = SourceTypeInfo {
                name: $name,
                layer_kind: SourceKind::$kind,
                from_json: true,
                round_zoom: false,
                overscale: true,
                prefetch: true,
            };

            type Options = ();
            type Description = ();

            fn from_json(
                _value: &Convertible<'_>,
                _functions: &Arc<Functions>,
                _error: &mut Error,
            ) -> Option<SourceData<Self>> {
                Some(SourceData::new(()))
            }
        }
    };
}

fixture_source_type!(Shapes, "shapes", Geometry);
fixture_source_type!(Points, "points", Geometry);
fixture_source_type!(Heights, "heights", RasterDem);

/// A source `id` of the fixture type `T`.
pub(crate) fn source<T: StyleSourceType<Options = ()>>(id: &str) -> Source {
    Source::new(id, SourceData::<T>::new(()))
}
