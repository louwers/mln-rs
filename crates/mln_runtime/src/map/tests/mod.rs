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
    std::sync::Arc::clone(mln_rs::builtin_registry().style())
}
