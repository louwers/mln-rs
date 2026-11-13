// The feature state that a map keeps, and its way to the frames of a render
// session. They follow maplibre-native-ffi's ABI tests (tests/native/abi/
// style/feature_state.c and render/invalidation.c at eae96a42c) where those
// test the same rules.

mod support;

use feature::{Identifier, Value};
use mln_runtime::{
    AttachOptions, DemandMode, ErrorKind, FeatureState, FeatureStateSelector, FrameDemand, FrameDisposition, Map,
    RenderSession, RenderTarget, RenderedQueryGeometry, RenderedQueryOptions,
};
use mln_util::geo::ScreenCoordinate;
use mln_util::size::Size;
use support::{EMPTY_STYLE, eventually, map, runtime, wait, wait_err};

const SIZE: Size = Size { width: 64, height: 64 };
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

/// Point 1 off to the east, and point 2 at the center, green while selected
/// and red otherwise.
const POINTS_STYLE: &str = r##"{"version":8,"transition":{"duration":0},"sources":{"points":{"type":"geojson","data":{"type":"FeatureCollection","features":[{"type":"Feature","id":1,"properties":{},"geometry":{"type":"Point","coordinates":[30,0]}},{"type":"Feature","id":2,"properties":{},"geometry":{"type":"Point","coordinates":[0,0]}}]}}},"layers":[{"id":"points","type":"circle","source":"points","paint":{"circle-radius":20,"circle-color":["case",["boolean",["feature-state","selected"],false],"#00ff00","#ff0000"]}}]}"##;

fn point(id: u64) -> FeatureStateSelector {
    FeatureStateSelector::feature("points", Identifier::Uint(id))
}

fn selected() -> FeatureState {
    state(&[("selected", Value::Bool(true))])
}

/// An offscreen session of `SIZE` that has attached.
fn attach(map: &Map) -> RenderSession {
    let target = RenderTarget::Offscreen { size: SIZE, device: None };
    let (session, attached) = map.attach(target, AttachOptions::default()).unwrap();
    attached.wait().unwrap();
    session
}

/// Renders one frame, whether or not the map changed.
fn render(session: &RenderSession) -> mln_runtime::FrameResult {
    session.demand_frame(FrameDemand::new(DemandMode::Force, 0)).unwrap();
    let mut results = Vec::new();
    assert!(eventually(|| {
        results.extend(session.drain_frame_results().unwrap());
        !results.is_empty()
    }));
    assert_eq!(FrameDisposition::Rendered, results[0].disposition);
    results[0]
}

/// Renders until a frame shows every source loaded.
fn render_loaded(session: &RenderSession) {
    let loaded = eventually(|| {
        let frame = render(session);
        frame.mode == mln_map::RenderMode::Full && !frame.needs_repaint
    });
    assert!(loaded, "the map never loaded");
}

/// The premultiplied RGBA of the center pixel of the session's last frame.
fn center_pixel(session: &RenderSession) -> [u8; 4] {
    let image = wait(session.read_texture());
    let offset = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

/// The state of the features drawn at the center in the session's last
/// frame.
fn center_states(session: &RenderSession) -> Vec<FeatureState> {
    let center = RenderedQueryGeometry::Point(ScreenCoordinate::new(32.0, 32.0));
    let features = wait(session.query_rendered_features(center, RenderedQueryOptions::default()));
    features.into_iter().map(|feature| feature.state).collect()
}

#[test]
fn a_frame_shows_and_queries_the_state_of_the_update_it_renders() {
    let runtime = runtime();
    let map = map(&runtime);
    wait(map.set_style_json(POINTS_STYLE));
    // Set before the source first renders.
    wait(map.set_feature_state(&point(1), selected()));
    let session = attach(&map);
    render_loaded(&session);
    assert_eq!(RED, center_pixel(&session));
    assert_eq!(vec![FeatureState::new()], center_states(&session));

    // The state moves to the center point. A rendered query reads the last
    // frame, so it sees the move once a frame renders it.
    wait(map.remove_feature_state(&point(1)));
    wait(map.set_feature_state(&point(2), selected()));
    assert_eq!(vec![FeatureState::new()], center_states(&session));
    render(&session);
    assert_eq!(GREEN, center_pixel(&session));
    assert_eq!(vec![selected()], center_states(&session));

    // A command group holds the update, and the state with it.
    let group = map.command_group().unwrap();
    wait(map.remove_feature_state(&point(2)));
    render(&session);
    assert_eq!(GREEN, center_pixel(&session));
    wait(group.end());
    render(&session);
    assert_eq!(RED, center_pixel(&session));
    assert_eq!(vec![FeatureState::new()], center_states(&session));

    // Renderer data cleared takes the state with it, and the next frame
    // gives it again.
    wait(map.set_feature_state(&point(2), selected()));
    render(&session);
    wait(session.clear_data());
    render_loaded(&session);
    assert_eq!(GREEN, center_pixel(&session));
}

#[test]
fn a_kept_source_loses_the_state_of_a_removed_one() {
    let runtime = runtime();
    let map = map(&runtime);
    wait(map.set_style_json(POINTS_STYLE));
    wait(map.set_feature_state(&point(2), selected()));
    let session = attach(&map);
    render_loaded(&session);
    assert_eq!(GREEN, center_pixel(&session));

    // No frame renders between the removal and the new source, so the
    // renderer keeps the source that it drew under the ID.
    let style: serde_json::Value = serde_json::from_str(POINTS_STYLE).unwrap();
    wait(map.remove_style_layer("points"));
    wait(map.remove_style_source("points"));
    wait(map.add_style_source_json("points", style["sources"]["points"].to_string()));
    wait(map.add_style_layer_json(style["layers"][0].to_string(), None));
    render_loaded(&session);
    assert_eq!(RED, center_pixel(&session));

    // So does a style load.
    wait(map.set_feature_state(&point(2), selected()));
    render(&session);
    assert_eq!(GREEN, center_pixel(&session));
    wait(map.set_style_json(POINTS_STYLE));
    render_loaded(&session);
    assert_eq!(RED, center_pixel(&session));
}

#[test]
fn only_a_change_of_state_publishes_a_render_update() {
    // The state needs no style, and without one nothing else publishes an
    // update while the test runs.
    let runtime = runtime();
    let map = map(&runtime);
    let mut generation = wait(map.with_core(|core| core.pending_update_generation()));
    let mut publishes = |completion| {
        let applied: mln_runtime::Applied = wait(completion);
        let published = applied.render_update_generation > generation;
        generation = applied.render_update_generation;
        published
    };
    assert!(publishes(map.set_feature_state(&point(1), selected())));
    assert!(!publishes(map.set_feature_state(&point(1), selected())));
    assert!(!publishes(map.set_feature_state(&point(1), FeatureState::new())));
    assert!(publishes(map.set_feature_state(&point(1), state(&[("selected", Value::Bool(false))]))));
    assert!(!publishes(map.remove_feature_state(&point(1).with_state_key("hover"))));
    assert!(!publishes(map.remove_feature_state(&point(2))));
    assert!(publishes(map.remove_feature_state(&point(1))));
    assert!(!publishes(map.remove_feature_state(&point(1))));
}

fn state(entries: &[(&str, Value)]) -> FeatureState {
    entries.iter().map(|(key, value)| ((*key).to_owned(), value.clone())).collect()
}

fn feature(id: &str) -> FeatureStateSelector {
    FeatureStateSelector::feature("points", Identifier::String(id.to_owned()))
}

#[test]
fn state_merges_on_set_and_needs_no_style() {
    let runtime = runtime();
    let map = map(&runtime);
    assert!(wait(map.feature_state(&feature("a"))).is_empty());
    wait(map.set_feature_state(&feature("a"), state(&[("hover", Value::Bool(true))])));
    wait(map.set_feature_state(&feature("a"), state(&[("count", Value::Uint(4))])));
    assert_eq!(
        state(&[("hover", Value::Bool(true)), ("count", Value::Uint(4))]),
        wait(map.feature_state(&feature("a")))
    );

    wait(map.remove_feature_state(&feature("a").with_state_key("hover")));
    assert_eq!(state(&[("count", Value::Uint(4))]), wait(map.feature_state(&feature("a"))));
    // A number names the feature of its text.
    let seven = FeatureStateSelector::feature("points", Identifier::Uint(7));
    wait(map.set_feature_state(&seven, state(&[("n", Value::Uint(1))])));
    assert!(!wait(map.feature_state(&feature("7"))).is_empty());
    let source = FeatureStateSelector { feature_id: None, ..feature("") };
    wait(map.remove_feature_state(&source));
    assert!(wait(map.feature_state(&feature("a"))).is_empty());
}

#[test]
fn a_style_load_and_a_removed_source_clear_the_state() {
    let runtime = runtime();
    let map = map(&runtime);
    wait(map.set_style_json(EMPTY_STYLE));
    wait(
        map.add_style_source_json("points", r#"{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}"#),
    );
    let other = FeatureStateSelector::feature("other", Identifier::Uint(1));
    wait(map.set_feature_state(&feature("a"), state(&[("n", Value::Uint(1))])));
    wait(map.set_feature_state(&other, state(&[("n", Value::Uint(2))])));

    wait(map.remove_style_source("points"));
    assert!(wait(map.feature_state(&feature("a"))).is_empty());
    assert!(!wait(map.feature_state(&other)).is_empty());

    wait(map.set_style_json(EMPTY_STYLE));
    assert!(wait(map.feature_state(&other)).is_empty());
}

#[test]
fn selectors_are_checked_when_sent() {
    fn kind<T>(result: Result<T, mln_runtime::Error>) -> ErrorKind {
        result.map(drop).unwrap_err().kind
    }

    let runtime = runtime();
    let map = map(&runtime);
    let source = FeatureStateSelector { feature_id: None, ..feature("") };
    assert_eq!(ErrorKind::InvalidArgument, kind(map.set_feature_state(&source, state(&[("n", Value::Null)]))));
    assert_eq!(ErrorKind::InvalidArgument, kind(map.feature_state(&source)));
    assert_eq!(ErrorKind::InvalidArgument, kind(map.remove_feature_state(&source.with_state_key("n"))));
    let nan = FeatureStateSelector::feature("points", Identifier::Double(f64::NAN));
    assert_eq!(ErrorKind::InvalidArgument, kind(map.feature_state(&nan)));
    // The map still takes the commands after the rejected ones.
    assert_eq!(ErrorKind::InvalidState, wait_err(map.remove_style_layer("missing")).kind);
}
