//! How a custom source's tiles ask the host for their data: a tile the view
//! needs fetches it, one it no longer needs cancels, and the host's answers
//! reach the tiles.

use std::sync::Arc;

use mln_rs::mln_map_types::MapMode;
use mln_rs::mln_render::source::TileOperation;
use mln_rs::mln_sources::custom::TileOptions;
use mln_rs::mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_rs::mln_util::range::Range;
use mln_rs::{CustomSourceCallbacks, CustomSourceCommands};
use parking_lot::{Condvar, Mutex};

use crate::host::{Host, wait, wait_until};

/// A map of a background alone, at zoom 0, where it shows tile 0/0/0.
const STYLE: &str = r#"{"version":8,"zoom":0,"sources":{},"layers":[{"id":"background","type":"background"}]}"#;

/// What the host was asked: true for a fetch, false for a cancel.
#[derive(Default)]
struct Calls {
    calls: Mutex<Vec<(bool, CanonicalTileID)>>,
    changed: Condvar,
}

impl Calls {
    fn callbacks(calls: &Arc<Calls>) -> CustomSourceCallbacks {
        let (fetches, cancels) = (Arc::clone(calls), Arc::clone(calls));
        CustomSourceCallbacks {
            fetch_tile: Box::new(move |tile| fetches.push(true, tile)),
            cancel_tile: Some(Box::new(move |tile| cancels.push(false, tile))),
        }
    }

    fn push(&self, fetch: bool, tile: CanonicalTileID) {
        self.calls.lock().push((fetch, tile));
        self.changed.notify_all();
    }

    /// Waits until the host was asked to fetch (or cancel) the root tile
    /// `count` times.
    fn wait_for(&self, fetch: bool, count: usize) {
        let what = format!("call {count} to {}", if fetch { "fetch" } else { "cancel" });
        wait_until(&self.calls, &self.changed, &what, |calls| {
            (calls.iter().filter(|call| **call == (fetch, root().canonical)).count() >= count).then_some(())
        });
    }
}

fn root() -> OverscaledTileID {
    OverscaledTileID::from_zxy(0, 0, 0)
}

/// A map with a custom geometry source "custom", whose host is `calls`,
/// drawn by a circle layer "points".
fn custom_map(calls: &Arc<Calls>) -> Host {
    let host = Host::new(MapMode::Continuous, STYLE);
    let options = TileOptions::default();
    host.command(host.map.add_custom_geometry_source("custom", Calls::callbacks(calls), Range::new(0, 18), options));
    host.command(host.map.add_style_layer_json(r#"{"id":"points","type":"circle","source":"custom"}"#, None));
    host.frame();
    calls.wait_for(true, 1);
    host
}

fn set_visible(host: &Host, visible: bool) {
    let visibility = if visible { r#""visible""# } else { r#""none""# };
    host.command(host.map.set_style_layer_property("points", "visibility", visibility));
    host.frame();
}

/// A tile that goes while it fetches, as its source's hidden layers let it
/// go, cancels the fetch, and the tile that the shown layers need fetches
/// again.
#[test]
fn a_tile_that_goes_cancels_its_fetch() {
    let calls = Arc::default();
    let host = custom_map(&calls);
    set_visible(&host, false);
    calls.wait_for(false, 1);
    set_visible(&host, true);
    calls.wait_for(true, 2);
}

/// A tile whose host reports an error reports it as a tile action, and has
/// done what it can.
#[test]
fn a_tile_error_is_an_error_action() {
    let calls = Arc::default();
    let host = custom_map(&calls);
    host.command(host.map.set_custom_geometry_source_tile_error("custom", root().canonical, "failed"));
    host.settle();
    assert!(host.actions(root()).contains(&TileOperation::Error));
}

/// A tile that the map lets go of takes the host's data with it: the next
/// tile to need the data asks the host again. The tile leaves the cache
/// that its hidden layers put it in as the session frees memory.
#[test]
fn a_tile_the_map_lets_go_of_is_fetched_again() {
    let calls = Arc::default();
    let host = custom_map(&calls);
    let point = r#"{"type":"Point","coordinates":[0,0]}"#;
    host.command(host.map.set_custom_geometry_source_tile_data("custom", root().canonical, point));
    host.settle();
    assert_eq!(host.features("custom", None), 1);

    set_visible(&host, false);
    wait(host.session.reduce_memory_use());
    set_visible(&host, true);
    calls.wait_for(true, 2);
}
