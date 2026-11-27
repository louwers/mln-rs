//! The registry of the C API's runtimes and tile workers.

use std::sync::{Arc, LazyLock};

use mln_builtins::register_builtins;
use mln_engine::{Registry, RegistryBuilder};

use crate::custom_layer::CustomLayer;
use crate::custom_source::{CustomGeometrySourceType, CustomVectorSourceType};

/// The built-ins, the custom source types and the `custom` layer type.
pub(crate) fn registry() -> Arc<Registry> {
    static REGISTRY: LazyLock<Arc<Registry>> = LazyLock::new(|| {
        let mut builder = RegistryBuilder::new();
        // Into an empty builder, the built-ins and the C API's types, whose
        // names differ, register.
        let registered = register_builtins(&mut builder)
            .and_then(|()| builder.add_source_type::<CustomGeometrySourceType>())
            .and_then(|()| builder.add_source_type::<CustomVectorSourceType>())
            .and_then(|()| builder.add_layer_type::<CustomLayer>());
        debug_assert_eq!(registered, Ok(()));
        builder.build()
    });
    Arc::clone(&REGISTRY)
}
