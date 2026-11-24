mod ground;
mod map_events;
mod map_snapshot;
mod map_updates;
mod style_loader;
mod style_sources;
mod terrain_camera;
mod terrain_conversions;
mod terrain_gestures;
mod terrain_rest;

/// A pool for a test's map.
fn pool() -> mln_util::pool::Pool {
    mln_util::pool::Pool::new(4)
}
