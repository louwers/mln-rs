mod map_events;
mod map_snapshot;
mod map_updates;
mod style_loader;
mod style_sources;

/// A pool for a test's map.
fn pool() -> mln_util::pool::Pool {
    mln_util::pool::Pool::new(4).expect("a pool")
}
