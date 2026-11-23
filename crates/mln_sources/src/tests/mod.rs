//! Ported gtests of the built-in source types (test/style/source.test.cpp,
//! test/style/conversion/*.test.cpp, test/tile/custom_*_tile.test.cpp), one
//! module per C++ file, and the port's own. They register this crate's
//! source types: the facade's registry would hold another copy of them.
//! Their tiles run through `mln_render::testing`.

mod conversion_geojson_options;
mod conversion_source_options;
mod conversion_tileset;
mod custom_geometry_tile;
mod custom_vector_tile;
mod render_source;
mod source;
mod tileset_tile;

use std::sync::Arc;

use mln_map_types::{ConstrainMode, TransformState, ViewportMode};
use mln_render::testing::{Event, OriginTiles};
use mln_render::{Registry, RegistryBuilder};
use mln_style::source_type::Load;
use mln_style::{Source, StyleRegistry};
use mln_util::geo::LatLng;
use mln_util::pool::{Pool, TaskGroup};
use mln_util::size::Size;

/// The built-in expression functions, the layer types of `mln_layers` and
/// this crate's source types.
pub(crate) fn registry() -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    mln_style::expression::functions::register_builtin_functions(builder.functions_mut()).unwrap();
    mln_layers::register_layer_types(&mut builder).unwrap();
    crate::register_source_types(&mut builder).unwrap();
    builder.build()
}

/// What styles parse against: [`registry`]'s style half.
pub(crate) fn style_registry() -> Arc<StyleRegistry> {
    Arc::clone(registry().style())
}

/// A 512 by 512 view at `zoom` over the center of the world.
pub(crate) fn view_state(zoom: f64) -> TransformState {
    let mut state = TransformState::new(ConstrainMode::HeightOnly, ViewportMode::Default);
    state.set_size(Size::new(512, 512));
    state.set_lat_lng_zoom(&LatLng::default(), zoom);
    state
}

/// `source` with the description that its type knows without a fetch, such
/// as the tileset a style lists, as the map's loader gives it.
pub(crate) fn ready(source: Source) -> Source {
    match source.data().plan_load() {
        Load::Ready(description) => {
            source.with_description(Some(description)).expect("a description of the source's type")
        }
        _ => source,
    }
}

pub(crate) fn parse_json(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("test JSON must parse")
}

/// The fixture of the C++ tile tests: the workers' pool, which the tiles'
/// stores free on, and a pool without threads, which the test runs itself.
pub(crate) struct TileTest {
    pub(crate) thread_pool: TaskGroup,
    pub(crate) local_pool: Pool,
}

impl TileTest {
    pub(crate) fn new() -> Self {
        TileTest { thread_pool: TaskGroup::new(&Pool::new(4)), local_pool: Pool::local() }
    }
}

impl Drop for TileTest {
    fn drop(&mut self) {
        self.thread_pool.wait();
    }
}

/// The errors that `tiles` reported since the last call.
pub(crate) fn errors(tiles: &mut OriginTiles) -> Vec<String> {
    let events = tiles.take_events().into_iter();
    events
        .filter_map(|event| match event {
            Event::Error { error, .. } => Some(error.to_string()),
            Event::Changed | Event::Action(_) => None,
        })
        .collect()
}

/// How many changes `tiles` reported since the last call, after they took
/// in what arrived.
pub(crate) fn changes(tiles: &mut OriginTiles) -> usize {
    tiles.receive();
    tiles.take_events().into_iter().filter(|event| matches!(event, Event::Changed)).count()
}
