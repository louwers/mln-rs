//! How a GeoJSON source's tiles follow its data and its synchronous tiling.

use mln_builtins::sources::geojson::set_data;
use mln_engine::RenderMode;
use mln_engine::map::MapMode;

use super::host::Host;

/// A map at zoom 0 of a GeoJSON source "points" whose data has one point,
/// drawn by a circle layer, with `members` added to the source.
fn style(members: &str) -> String {
    format!(
        r#"{{"version":8,"zoom":0,"sources":{{"points":{{"type":"geojson","data":{POINT}{members}}}}},
        "layers":[{{"id":"points","type":"circle","source":"points"}}]}}"#
    )
}

const POINT: &str = r#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},
    "geometry":{"type":"Point","coordinates":[1.1,1.1]}}]}"#;

/// Whether the tiles of new data lay out within the frame that takes it.
fn lays_out_within_the_frame(host: &Host) -> bool {
    host.command(host.map.set_geojson_source_data("points", POINT));
    let laid_out = host.frame().mode == RenderMode::Full;
    host.settle();
    laid_out
}

/// The tiles of a source with synchronous tiling lay out as the frame
/// takes new data, and the others in the background; a change of the
/// setting applies to the tiles there are. The data is clustered, which
/// cuts a tile within the frame either way.
#[test]
fn synchronous_tiling_applies_to_the_tiles_there_are() {
    for synchronous in [true, false] {
        let host =
            Host::new(MapMode::Continuous, &style(&format!(r#","cluster":true,"synchronousUpdate":{synchronous}"#)));
        host.settle();
        assert_eq!(lays_out_within_the_frame(&host), synchronous);
        host.command(host.map.set_geojson_source_synchronous_tiling("points", !synchronous));
        host.settle();
        assert_eq!(lays_out_within_the_frame(&host), !synchronous);
    }
}

/// A source whose data goes keeps showing its tiles in a continuous map,
/// as data that a new frame brings would replace them, but drops them in a
/// static map.
#[test]
fn tiles_stay_after_their_data_goes_only_in_a_continuous_map() {
    for mode in [MapMode::Continuous, MapMode::Static] {
        let host = Host::new(mode, &style(""));
        let render = || if mode == MapMode::Continuous { host.settle() } else { host.still().unwrap() };
        render();
        assert_eq!(host.features("points", None), 1, "{mode:?}");
        host.edit(set_data("points", None));
        render();
        assert_eq!(host.features("points", None), usize::from(mode == MapMode::Continuous), "{mode:?}");
    }
}
