mod map_events;
mod map_snapshot;
mod map_updates;
mod style_loader;
mod style_sources;

/// A pool for a test's map.
fn pool() -> mln_util::pool::Pool {
    mln_util::pool::Pool::new(4)
}

/// The built-in registrations that a test's map parses its style against.
fn registry() -> std::sync::Arc<mln_style::StyleRegistry> {
    let mut builder = mln_renderer::RegistryBuilder::new();
    mln_style::expression::functions::register_builtin_functions(builder.functions_mut()).unwrap();
    mln_layers::register_layer_types(&mut builder).unwrap();
    mln_renderer::builtins::register_layer_types(&mut builder).unwrap();
    mln_renderer::builtins::register_source_types(&mut builder).unwrap();
    std::sync::Arc::clone(builder.build().style())
}
