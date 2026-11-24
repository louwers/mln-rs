//! How a tileset source's tiles load: from the cache, then the network
//! while the view needs them; what each kind of response leaves them with;
//! and when they load again. Time passes only in the responses' headers:
//! data that expired long ago, or expires far ahead.
//!
//! A check that a tile made no request, or no layout, reads what the map
//! did by a frame's end. A request still on its way through the cache's
//! thread, or a layout still on a worker, would come after it, so such a
//! check catches what the frames did and can miss what they set off.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mln_rs::LogicalExtent;
use mln_rs::mln_image::encode_png;
use mln_rs::mln_map_types::{CameraOptions, CameraUpdate, MapMode};
use mln_rs::mln_render::RenderMode;
use mln_rs::mln_render::source::TileOperation;
use mln_rs::mln_sources::VectorSourceType;
use mln_rs::mln_storage::resource::Priority;
use mln_rs::mln_storage::response::Reason;
use mln_rs::mln_storage::{LoadingMethod, Resource, ResourceError, Response, StoragePolicy, TileFormat};
use mln_rs::mln_style::Edit;
use mln_rs::mln_style::source_type::SourceCommon;
use mln_rs::mln_tile_id::OverscaledTileID;
use mln_rs::mln_util::chrono::Timestamp;
use mln_rs::mln_util::geo::LatLng;
use mln_rs::mln_util::image::PremultipliedImage;
use mln_rs::mln_util::size::Size;

use crate::host::{Host, point_tile};

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

/// A raster-dem source seen past its maxzoom draws its deepest tiles
/// larger, as a raster source does: zooming in keeps the maxzoom's tile,
/// loaded, drawn and with that zoom in its id, and requests it once.
#[test]
fn raster_dem_tiles_past_the_maxzoom_stay_at_their_zoom() {
    const DEEPEST: &str = "test://tiles/1/1/0";
    let host = Host::new(MapMode::Continuous, &kind_style("raster-dem", ""));
    host.network.answer_with(|_| Some(red_png()));
    set_common(&host, SourceCommon { prefetch_zoom_delta: Some(0), ..SourceCommon::default() });
    for zoom in [1.0, 2.0, 3.0, 4.0] {
        // The view sees the middle of tile 1/1/0 alone.
        let camera = CameraOptions { center: LatLng::new(45.0, 90.0).ok(), zoom: Some(zoom), ..Default::default() };
        host.command(host.map.update_camera(CameraUpdate { camera, ..Default::default() }));
        host.settle();
        assert_eq!(host.network.requests(DEEPEST).len(), 1, "tile requests by zoom {zoom}");
        let overscaled = OverscaledTileID::with_zxy(zoom as u8, 0, 1, 1, 0);
        let actions = host.actions(overscaled);
        if zoom == 1.0 {
            let loaded = actions.contains(&TileOperation::EndParse) && !actions.contains(&TileOperation::Error);
            assert!(loaded, "tile {overscaled:?} loaded: {actions:?}");
        } else {
            assert!(actions.is_empty(), "no tile {overscaled:?}: {actions:?}");
        }
        assert_eq!(host.center_pixel(), [255, 0, 0, 255], "the maxzoom tile draws at zoom {zoom}");
    }
}

/// A red PNG.
fn red_png() -> Response {
    let size = Size::new(4, 4);
    let image = PremultipliedImage::from_vec(size, [255, 0, 0, 255].repeat(16));
    let mut response = Response::new();
    response.data = Some(Arc::new(encode_png(&image)));
    response
}

fn no_content() -> Response {
    let mut response = Response::new();
    response.no_content = true;
    response
}

/// An empty response leaves a tile empty, which draws nothing; a 404 leaves
/// a vector tile empty but a raster tile missing, which its parent draws in
/// place of; and a sparse tileset's empty tiles are missing. The parent at
/// zoom 0 draws red at the map's center, where the four tiles of zoom 1
/// meet.
#[test]
fn empty_and_missing_tiles_by_kind() {
    let not_found = || failed(Reason::NotFound, "not found");
    let cases: [(&str, Answer, &str, bool); 6] = [
        ("vector", no_content, "", false),
        ("vector", not_found, "", false),
        ("vector", no_content, r#","sparse":true"#, true),
        ("raster", no_content, "", false),
        ("raster", not_found, "", true),
        ("raster", no_content, r#","sparse":true"#, true),
    ];
    for (kind, child, members, parent_drawn) in cases {
        let case = format!("{kind} tile{members}, {:?}", child());
        let host = Host::new(MapMode::Continuous, &kind_style(kind, members));
        let parent = if kind == "raster" { red_png } else { point_tile };
        host.network
            .answer_with(move |resource| Some(if resource.url.ends_with("0/0/0") { parent() } else { child() }));
        host.move_to(0.0, 0.0);
        host.settle();
        host.move_to(0.0, 1.0);
        host.settle();
        let red = host.center_pixel() == [255, 0, 0, 255];
        assert_eq!(red, parent_drawn, "{case}");
    }
}

/// A failed load fails a still image, and the next still image requests it
/// again.
#[test]
fn a_failed_load_is_requested_again_on_retry() {
    let host = Host::new(MapMode::Static, &style(r#","maxzoom":0"#));
    let answers = AtomicUsize::new(0);
    host.network.answer_with(move |_| {
        Some(if answers.fetch_add(1, Ordering::SeqCst) == 0 { failed(Reason::Other, "failed") } else { point_tile() })
    });
    let error = host.still().expect_err("a still image of a tile that failed");
    assert!(error.message.contains("failed"), "{error}");
    host.still().unwrap();
    assert_eq!(host.network.requests("test://tiles/0/0/0").len(), 2);
    assert_eq!(host.features("tiles", Some("points")), 1);
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

/// A tile that becomes optional stops its request, here a revalidation, and
/// asks anew when required again. A tile whose load failed, with nothing to
/// show, goes as the view leaves it rather than wait for the network's
/// retry, and loads anew as the view comes back.
#[test]
fn a_tile_that_stops_being_needed_asks_anew_when_needed_again() {
    let host = loaded(&style(""), || expiring(point_tile(), Some(LONG_AGO)));
    away_and_back(&host, 90.0, point_tile);
    host.network.wait_for(URL, 2);
    host.move_to(90.0, 2.0);
    host.frame();
    assert!(host.network.requests(URL)[1].1, "the revalidation stops");
    host.move_to(0.0, 2.0);
    host.frame();
    host.network.wait_for(URL, 3);

    let host = Host::new(MapMode::Continuous, &style(""));
    host.network.answer_with(|resource| (resource.url != URL).then(point_tile));
    host.move_to(0.0, 2.0);
    host.frame();
    host.network.answer(URL, failed(Reason::Connection, "offline"));
    host.settle();
    away_and_back(&host, 90.0, point_tile);
    host.network.wait_for(URL, 2);
    assert_eq!(host.actions(tile()).first(), Some(&TileOperation::RequestedFromCache), "a new tile");
}

/// What a network request asks of its data's freshness: its minimum update
/// interval, and whether it stays out of the offline cache.
fn freshness(resource: &Resource) -> (Duration, bool) {
    (resource.minimum_update_interval, resource.storage_policy == StoragePolicy::Volatile)
}

/// Gives the source `common` as its common members.
fn set_common(host: &Host, common: SourceCommon) {
    host.command(host.map.apply_style_edit(Edit::SetSourceCommon { id: "tiles".to_owned(), common }));
}

/// A required tile's network request is made again, with the source's new
/// minimum update interval or volatility, as the next frame retains the
/// tile, and so is that of a tile that left the view and goes on loading,
/// as the view comes back; a frame that changes neither makes no request.
#[test]
fn a_network_request_restarts_when_the_update_interval_changes() {
    let changes = [
        SourceCommon { minimum_tile_update_interval: Duration::from_secs(1), ..SourceCommon::default() },
        SourceCommon { volatile: true, ..SourceCommon::default() },
    ];
    for (common, out_of_view) in changes.into_iter().flat_map(|common| [(common, false), (common, true)]) {
        let case = format!("{common:?}, out of view: {out_of_view}");
        let host = Host::new(MapMode::Continuous, &style(""));
        host.network.answer_with(|resource| (resource.url != URL).then(point_tile));
        host.move_to(0.0, 2.0);
        host.frame();
        host.network.wait_for(URL, 1);
        assert_eq!(freshness(&host.network.requests(URL)[0].0), (Duration::ZERO, false), "{case}");

        if out_of_view {
            // At longitude 90 the tile goes on loading out of view.
            host.move_to(90.0, 2.0);
            set_common(&host, common);
            host.frame();
            let requests = host.network.requests(URL);
            assert!(requests.len() == 1 && !requests[0].1, "{case}: the tile goes on loading");
            host.move_to(0.0, 2.0);
        } else {
            set_common(&host, common);
        }
        host.frame();
        host.network.wait_for(URL, 2);
        let requests = host.network.requests(URL);
        assert!(requests[0].1, "{case}: the request before stops");
        assert_eq!(freshness(&requests[1].0), (common.minimum_tile_update_interval, common.volatile), "{case}");
        host.frame();
        host.frame();
        assert_eq!(host.network.requests(URL).len(), 2, "{case}");
    }
}

/// A tile that comes back from the cache shows its data at once while it
/// revalidates if caching rules allow it; a tile whose stale data must
/// revalidate goes, and loads anew.
#[test]
fn a_cached_tile_shows_its_data_only_while_caching_rules_allow() {
    for (expires, must_revalidate, reused) in
        [(FAR_AHEAD, false, true), (LONG_AGO, false, true), (LONG_AGO, true, false)]
    {
        let case = format!("data that expires at {expires:?}, must revalidate: {must_revalidate}");
        let response = move || {
            let mut response = expiring(point_tile(), Some(expires));
            response.caching.must_revalidate = must_revalidate;
            response
        };
        let host = loaded(&style(""), response);
        away_and_back(&host, 90.0, response);
        let frame = host.frame();
        assert_eq!(frame.mode == RenderMode::Full, reused, "{case}: the view shows its tiles at once");
        host.network.wait_for(URL, 2);
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

/// A tile whose request fails, or whose data doesn't decode, reports why,
/// which fails a still image; one without content loads empty, whatever
/// its kind.
#[test]
fn a_tile_reports_why_it_failed_to_load() {
    let corrupt = || {
        let mut response = Response::new();
        response.data = Some(Arc::new(b"CORRUPTED".to_vec()));
        response
    };
    let fail = || failed(Reason::Other, "Failed by the test case");
    for kind in ["vector", "raster", "raster-dem"] {
        let decode_error = if kind == "vector" { "protobuf decode error: invalid wire type: 7" } else { "" };
        let cases: [(Answer, Option<&str>); 3] =
            [(fail, Some("Failed by the test case")), (corrupt, Some(decode_error)), (no_content, None)];
        for (answer, error) in cases {
            let host = Host::new(MapMode::Static, &kind_style(kind, ""));
            host.network.answer_with(move |_| Some(answer()));
            let result = host.still();
            let case = format!("{kind}: {result:?}");
            match error {
                // Not the decoders' messages for images, which differ by platform.
                Some(message) => assert!(result.unwrap_err().message.contains(message), "{case}"),
                None => assert!(result.is_ok(), "{case}"),
            }
        }
    }
}

/// A source whose tileset has no tiles makes none, and counts as loaded,
/// until its tiles are set.
#[test]
fn a_tileset_without_tiles_makes_no_tiles() {
    let set_tiles = |host: &Host, tiles: &[&str]| {
        let tiles: Vec<String> = tiles.iter().map(|&tile| tile.to_owned()).collect();
        host.command(host.map.apply_style_edit(Edit::update_source::<VectorSourceType>(
            "tiles",
            "a vector source",
            |data| {
                data.description.as_mut().expect("an inline tileset").tiles = tiles;
                true
            },
        )));
    };
    for kind in ["vector", "raster", "raster-dem"] {
        let style = kind_style(kind, "").replace(r#""tiles":["test://tiles/{z}/{x}/{y}"]"#, r#""tiles":[]"#);
        let host = Host::new(MapMode::Continuous, &style);
        host.settle();
        assert!(host.network.requests("test://tiles/0/0/0").is_empty(), "{kind}");
        if kind != "vector" {
            continue;
        }
        set_tiles(&host, &["test://tiles/{z}/{x}/{y}"]);
        host.frame();
        host.network.wait_for("test://tiles/0/0/0", 1);
        assert_ne!(host.frame().mode, RenderMode::Full, "the tile waits for its data");
        set_tiles(&host, &[]);
        host.settle();
    }
}
