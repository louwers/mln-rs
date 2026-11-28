//! The registries of the C API's runtimes and tile workers.

use std::sync::{Arc, LazyLock};

use mln_builtins::register_builtins;
use mln_engine::{Registry, RegistryBuilder};
use mln_style::crs::Crs;

use crate::custom_layer::CustomLayer;
use crate::custom_source::{CustomGeometrySourceType, CustomVectorSourceType};
use crate::error::Error;

/// A builder with the built-ins, the custom source types, the `custom` layer
/// type and the `org.maplibre.location-puck` layer type.
fn builder() -> RegistryBuilder {
    let mut builder = RegistryBuilder::new();
    // Into an empty builder, the built-ins and the other types, whose names
    // differ, register.
    let registered = register_builtins(&mut builder)
        .and_then(|()| mln_location_puck::register(&mut builder))
        .and_then(|()| builder.add_source_type::<CustomGeometrySourceType>())
        .and_then(|()| builder.add_source_type::<CustomVectorSourceType>())
        .and_then(|()| builder.add_layer_type::<CustomLayer>());
    debug_assert_eq!(registered, Ok(()));
    builder
}

/// [`builder`]'s registrations: the registry of a runtime without projections
/// of its host's, and of the tile workers of every runtime.
pub(crate) fn registry() -> Arc<Registry> {
    static REGISTRY: LazyLock<Arc<Registry>> = LazyLock::new(|| builder().build());
    Arc::clone(&REGISTRY)
}

/// [`registry`]'s registrations and `projections`. Fails for a projection
/// that the registry refuses, which the message names by its index.
pub(crate) fn with_projections(projections: Vec<Crs>) -> Result<Arc<Registry>, Error> {
    if projections.is_empty() {
        return Ok(registry());
    }
    let mut builder = builder();
    for (index, crs) in projections.into_iter().enumerate() {
        builder.add_projection(crs).map_err(|error| Error::invalid_argument(format!("projection {index}: {error}")))?;
    }
    Ok(builder.build())
}
