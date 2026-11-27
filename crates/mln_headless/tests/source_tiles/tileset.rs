//! How a tileset source's tiles load: from the cache, then the network
//! while the view needs them; which data they share and keep; and what
//! their requests name. Time passes only in the responses' headers: data
//! that expired long ago, or expires far ahead.
//!
//! A check that a tile made no request, or no layout, reads what the map
//! did by a frame's end. A request still on its way through the cache's
//! thread, or a layout still on a worker, would come after it, so such a
//! check catches what the frames did and can miss what they set off.

use std::sync::Arc;

use mln_engine::RenderMode;
use mln_engine::camera::LogicalExtent;
use mln_engine::map::MapMode;
use mln_engine::source::TileOperation;
use mln_engine::storage::chrono::Timestamp;
use mln_engine::storage::resource::{Priority, TileFormat};
use mln_engine::storage::response::Reason;
use mln_engine::storage::{LoadingMethod, ResourceError, Response};
use mln_engine::tile::OverscaledTileID;
use mln_style::Edit;
use mln_style::size::Size;
use mln_style::source::SourceCommon;

use super::host::{Host, point_tile};

/// A white map of a vector source "tiles" at `test://tiles/{z}/{x}/{y}`,
/// with `members` added to the source, whose points draw as red circles.
fn style(members: &str) -> String {
    format!(
        r##"{{"version":8,"transition":{{"duration":0}},
        "sources":{{"tiles":{{"type":"vector","tiles":["test://tiles/{{z}}/{{x}}/{{y}}"]{members}}}}},
        "layers":[{{"id":"white","type":"background","paint":{{"background-color":"#ffffff"}}}},
            {{"id":"points","type":"circle","source":"tiles","source-layer":"points",
              "paint":{{"circle-color":"#ff0000","circle-radius":8}}}}]}}"##
    )
}

/// One of the tiles a map of 2 zooms on the equator at longitude 0 shows,
/// which leaves the view at longitude 90, and the view leaves it far behind
/// at 150.
const URL: &str = "test://tiles/2/1/1";

fn tile() -> OverscaledTileID {
    OverscaledTileID::from_zxy(2, 1, 1)
}

const LONG_AGO: Timestamp = Timestamp::from_secs(0);

const FAR_AHEAD: Timestamp = Timestamp::from_secs(i64::MAX);

/// `response`, which expires at `expires`.
fn expiring(mut response: Response, expires: Option<Timestamp>) -> Response {
    response.caching.expires = expires;
    response
}

/// [`point_tile`] with other bytes: an unknown field the tile's reader skips.
fn changed_point_tile() -> Response {
    let mut response = point_tile();
    let mut bytes = response.data.as_deref().unwrap().clone();
    // Field 15, a varint of 1.
    bytes.extend_from_slice(&[0x78, 0x01]);
    response.data = Some(Arc::new(bytes));
    response
}

/// A "not modified" answer, whose data has expired again.
fn not_modified() -> Response {
    let mut response = expiring(Response::new(), Some(LONG_AGO));
    response.not_modified = true;
    response
}

/// What a case answers a request with.
type Answer = fn() -> Response;

fn failed(reason: Reason, message: &str) -> Response {
    let mut response = Response::new();
    response.error = Some(Box::new(ResourceError::new(reason, message)));
    response
}

/// How many layouts of `actions` began, each of which ended.
fn layouts(actions: &[TileOperation]) -> usize {
    let began = actions.iter().filter(|op| **op == TileOperation::StartParse).count();
    let ended = actions.iter().filter(|op| **op == TileOperation::EndParse).count();
    assert_eq!(began, ended, "every layout ends: {actions:?}");
    began
}

/// A continuous map of `style` at 2 zooms on the equator at longitude 0,
/// whose every request is answered with `response`, loaded.
fn loaded(style: &str, response: impl Fn() -> Response + Send + 'static) -> Host {
    let host = Host::new(MapMode::Continuous, style);
    host.network.answer_with(move |_| Some(response()));
    host.move_to(0.0, 2.0);
    host.settle();
    host
}

/// Moves `host`'s view to `longitude` and back, which makes [`URL`]'s
/// tile optional and required again, with requests for it held for the
/// test from then on, and the others' answered with `response`.
fn away_and_back(host: &Host, longitude: f64, response: impl Fn() -> Response + Send + 'static) {
    host.network.answer_with(move |resource| (resource.url != URL).then(&response));
    host.move_to(longitude, 2.0);
    host.settle();
    host.move_to(0.0, 2.0);
    host.frame();
}

/// Gives the source `common` as its common members.
fn set_common(host: &Host, common: SourceCommon) {
    host.edit(Edit::SetSourceCommon { id: "tiles".to_owned(), common });
}

/// A white map of a source of `kind` whose tileset ends at zoom 1, with
/// `members` added to the source, which draws red.
fn kind_style(kind: &str, members: &str) -> String {
    let source = format!(r#"{{"type":"{kind}","tiles":["test://tiles/{{z}}/{{x}}/{{y}}"],"maxzoom":1{members}}}"#);
    let layer = match kind {
        "raster" => r#"{"id":"red","type":"raster","source":"tiles","paint":{"raster-fade-duration":0}}"#,
        // Every elevation past the last stop takes its color.
        "raster-dem" => {
            r##"{"id":"red","type":"color-relief","source":"tiles",
               "paint":{"color-relief-color":["interpolate",["linear"],["elevation"],0,"#ff0000",1,"#ff0000"]}}"##
        }
        _ => {
            r##"{"id":"red","type":"circle","source":"tiles","source-layer":"points",
               "paint":{"circle-color":"#ff0000","circle-radius":8}}"##
        }
    };
    format!(
        r##"{{"version":8,"transition":{{"duration":0}},"sources":{{"tiles":{source}}},
        "layers":[{{"id":"white","type":"background","paint":{{"background-color":"#ffffff"}}}},{layer}]}}"##
    )
}

/// A tile required again keeps network data without an expiry: it neither
/// requests nor lays it out again. Data that expires is requested again,
/// with its validators and expiry, while the tile shows it. A "not
/// modified" answer and the same bytes keep the data, as does a failure,
/// and new data lays out once.
#[test]
fn a_tile_required_again_asks_again_for_data_that_expires() {
    for expires in [None, Some(FAR_AHEAD), Some(LONG_AGO)] {
        let case = format!("data that expires at {expires:?}");
        let host = loaded(&style(""), move || expiring(point_tile(), expires));
        assert_eq!(layouts(&host.actions(tile())), 1, "{case}: one layout per response");
        let network = host.network.requests(URL);
        assert!(network.iter().all(|(resource, _)| resource.order.get() > 0), "{case}: in the tile's order");
        let shown = host.features("tiles", Some("points"));

        away_and_back(&host, 90.0, move || expiring(point_tile(), expires));
        host.settle();
        assert_eq!(layouts(&host.actions(tile())), 0, "{case}: nothing to lay out again");
        if expires.is_none() {
            assert_eq!(host.network.requests(URL).len(), 1, "{case}: no request for lasting data");
            continue;
        }
        // The tile shows its data while it revalidates.
        let revalidation = host.network.answer(URL, not_modified());
        assert_eq!(revalidation.prior_caching.expires, expires, "{case}");
        assert_eq!(revalidation.prior_etag.as_deref(), Some("a"), "{case}");
        host.settle();
        assert_eq!(layouts(&host.actions(tile())), 0, "{case}: \"not modified\" keeps the data");

        let answers: [(Response, usize); 3] = [
            (expiring(point_tile(), Some(LONG_AGO)), 0),
            (failed(Reason::Connection, "offline"), 0),
            (expiring(changed_point_tile(), Some(LONG_AGO)), 1),
        ];
        for (answer, expected) in answers {
            let failure = answer.error.is_some();
            away_and_back(&host, 90.0, move || expiring(point_tile(), expires));
            host.network.answer(URL, answer);
            host.settle();
            let actions = host.actions(tile());
            assert_eq!(layouts(&actions), expected, "{case}: {actions:?}");
            assert_eq!(actions.contains(&TileOperation::Error), failure, "{case}: {actions:?}");
            assert_eq!(host.features("tiles", Some("points")), shown, "{case}: the tile keeps showing data");
        }
    }
}

/// A tile cut from the tileset's last zoom takes the data that the tile at
/// that zoom brought fresh from the network from memory, and the tile at
/// that zoom takes its cut tiles', without a request; expired data is
/// requested again. The source loads no tiles of lower zooms first.
#[test]
fn a_tile_and_its_cut_tiles_share_fresh_data() {
    const WHOLE: &str = "test://tiles/0/0/0";
    for (first, second) in [(0.0, 2.0), (2.0, 0.0)] {
        for expires in [None, Some(LONG_AGO)] {
            let case = format!("zoom {first}, then {second}, data that expires at {expires:?}");
            let host = Host::new(MapMode::Continuous, &style(r#","maxzoom":0"#));
            host.network.answer_with(move |_| Some(expiring(point_tile(), expires)));
            set_common(&host, SourceCommon { prefetch_zoom_delta: Some(0), ..SourceCommon::default() });
            host.move_to(0.0, first);
            host.settle();
            let requests = host.network.requests(WHOLE).len();
            host.move_to(0.0, second);
            host.settle();
            let again = host.network.requests(WHOLE).len() > requests;
            assert_eq!(again, expires.is_some(), "{case}");
        }
    }
}

/// A tile in a new world copy whose resource is another than its copy's,
/// as after the pixel ratio changed, doesn't take the copy's data: it loads
/// its own, without the copy's validators.
#[test]
fn a_world_copy_of_another_resource_loads_its_own() {
    let style = style("").replace("{y}", "{y}{ratio}");
    let host = Host::new(MapMode::Continuous, &style);
    host.network.answer_with(|resource| (!resource.url.ends_with("@2x")).then(point_tile));
    host.move_to(0.0, 0.0);
    host.settle();

    // 170 degrees east shows tile 0/0/0 of the next world copy too.
    host.command(host.session.resize(LogicalExtent { size: Size::new(64, 64), scale_factor: 2.0 }));
    host.move_to(170.0, 0.0);
    host.frame();

    host.network.wait_for("test://tiles/0/0/0@2x", 1);
    let requests = host.network.requests("test://tiles/0/0/0@2x");
    assert!(requests.iter().all(|(resource, _)| resource.prior_etag.is_none()), "no validators of the copy's");
    assert_ne!(host.frame().mode, RenderMode::Full, "the new world copy waits for its own tile");
}

/// A tile cut from the tileset's last zoom whose resource is another than
/// the one a tile at that zoom loaded, as after the pixel ratio changed with
/// `{ratio}` in the URL, loads its own ancestor, without the other's
/// validators, rather than take the other's from memory.
#[test]
fn a_cut_tile_takes_no_ancestor_of_another_resource() {
    let style = style(r#","maxzoom":0"#).replace("{y}", "{y}{ratio}");
    let host = Host::new(MapMode::Continuous, &style);
    host.network.answer_with(|resource| (!resource.url.ends_with("@2x")).then(point_tile));
    host.move_to(0.0, 0.0);
    host.settle();

    host.command(host.session.resize(LogicalExtent { size: Size::new(64, 64), scale_factor: 2.0 }));
    host.move_to(0.0, 1.0);
    host.frame();

    host.network.wait_for("test://tiles/0/0/0@2x", 1);
    let requests = host.network.requests("test://tiles/0/0/0@2x");
    assert!(requests.iter().all(|(resource, _)| resource.prior_etag.is_none()), "no validators of the 1x tile's");
}

/// A tile whose data fails to lay out no longer holds it for the tiles cut
/// from it, though the data has no expiry: they load it again.
#[test]
fn a_tile_whose_data_fails_to_lay_out_is_requested_again() {
    let host = Host::new(MapMode::Continuous, &style(r#","maxzoom":0"#));
    set_common(&host, SourceCommon { prefetch_zoom_delta: Some(0), ..SourceCommon::default() });
    host.network.answer_with(|_| {
        let mut response = Response::new();
        response.data = Some(Arc::new(b"not a tile".to_vec()));
        Some(response)
    });
    host.move_to(0.0, 0.0);
    host.settle();
    assert!(host.actions(OverscaledTileID::from_zxy(0, 0, 0)).contains(&TileOperation::Error));
    host.move_to(0.0, 2.0);
    host.frame();
    host.network.wait_for("test://tiles/0/0/0", 2);
}

/// A stale copy from the cache shows while it revalidates at low priority,
/// and stays when the server can't be reached; one that must revalidate
/// waits for the network, which a 304 shows it after, and stays hidden when
/// the server can't be reached. The cache has it from a source of the same
/// tiles that the style had before.
#[test]
fn a_stale_copy_shows_only_while_caching_rules_allow() {
    let offline = || failed(Reason::Connection, "offline");
    let cases: [(bool, Answer); 3] = [(false, offline), (true, not_modified), (true, offline)];
    for (must_revalidate, revalidated) in cases {
        let failure = revalidated().error.is_some();
        let case = format!("must revalidate: {must_revalidate}, failed: {failure}");
        let style = style(r#","maxzoom":0"#);
        let earlier = style
            .replace(r#""tiles":{"type""#, r#""earlier":{"type""#)
            .replace(r#""source":"tiles""#, r#""source":"earlier""#);
        let host = Host::new(MapMode::Continuous, &earlier);
        host.network.answer_with(move |_| {
            let mut response = expiring(point_tile(), Some(LONG_AGO));
            response.caching.must_revalidate = must_revalidate;
            Some(response)
        });
        host.move_to(0.0, 0.0);
        host.settle();

        host.network.hold();
        host.command(host.map.set_style_json(style));
        host.frame();
        if must_revalidate {
            host.network.wait_for("test://tiles/0/0/0", 2);
            assert_ne!(host.frame().mode, RenderMode::Full, "{case}: held back");
        } else {
            host.settle();
            assert_eq!(host.features("tiles", Some("points")), 1, "{case}: shown at once");
        }
        let network = host.network.answer("test://tiles/0/0/0", revalidated());
        assert_eq!(network.loading_method, LoadingMethod::NetworkOnly, "{case}");
        let priority = if must_revalidate { Priority::Regular } else { Priority::Low };
        assert_eq!(network.priority, priority, "{case}");
        assert_eq!(network.prior_etag.as_deref(), Some("a"), "{case}");
        host.settle();
        let shows = host.features("tiles", Some("points")) == 1;
        assert_eq!(shows, !must_revalidate || !failure, "{case}");
        assert_eq!(
            host.actions(OverscaledTileID::from_zxy(0, 0, 0)).contains(&TileOperation::Error),
            failure,
            "{case}"
        );
    }
}

/// The format that a request for a tile names: what its source decodes.
#[test]
fn a_tile_request_names_the_format_its_source_decodes() {
    let cases = [
        ("vector", "", TileFormat::Mvt),
        ("vector", r#","encoding":"mlt""#, TileFormat::Mlt),
        ("raster", "", TileFormat::Image),
        ("raster-dem", "", TileFormat::Image),
    ];
    for (kind, members, format) in cases {
        let host = Host::new(MapMode::Continuous, &kind_style(kind, members));
        host.move_to(0.0, 0.0);
        host.frame();
        host.network.wait_for("test://tiles/0/0/0", 1);
        let resource = host.network.requests("test://tiles/0/0/0").remove(0).0;
        assert_eq!(resource.tile_data.map(|tile| tile.format), Some(format), "{kind}{members}");
    }
}
