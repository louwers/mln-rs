//! Feature state applied to a laid-out tile: the paint data of the buckets
//! whose paint depends on it, evaluated again for each feature that has a
//! state, as a tile does when an application sets its features' state.
//!
//! The buckets are the renderer's, reached through its `bench` entry points
//! (`mln_engine::bench`).

use mln_engine::layer::bucket::LayerFeatureStates;
use mln_engine::tile::TileData;
use mln_style::Style;
use mln_style::feature::GeometryTileFeature;
use mln_util::feature::feature_id_to_string;

use super::{DENSE, SPARSE, StreetLayout, StreetTile, registry, with_sprite};
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "micro/feature-state/apply/sparse",
        kind: Kind::Micro,
        question: "What does applying feature state to a sparse tile's roads and buildings cost?",
        run: apply_sparse,
    },
    Scenario {
        name: "micro/feature-state/apply/dense",
        kind: Kind::Micro,
        question: "What does applying feature state to a dense tile's roads and buildings cost?",
        run: apply_dense,
    },
];

/// The source layers whose features get a state.
const SELECTED: [&str; 2] = ["transportation", "building"];

/// The sparse street tile ([`SPARSE`]) laid out by Liberty, whose colors of
/// roads and buildings an application has made depend on a `selected`
/// feature state, as one that highlights what the user picks does. Every
/// road and building of the tile gets a state, every other one selected,
/// and the tile applies it: each bucket of a road or building layer whose
/// color depends on it evaluates the color again for each feature, and
/// writes it into the feature's vertices. The runs alternate between that
/// state and its opposite, so that each one changes every feature.
fn apply_sparse(bench: &mut Bench) {
    apply(bench, SPARSE);
}

/// [`apply_sparse`] for the dense tile ([`DENSE`]).
fn apply_dense(bench: &mut Bench) {
    apply(bench, DENSE);
}

fn apply(bench: &mut Bench, tile: StreetTile) {
    let fixtures = Fixtures::open(bench);
    let layout = StreetLayout::new(&fixtures, &selectable(&fixtures), |_| true, tile.id);
    let mut laid_out = layout.run();
    let states = [states(&layout.data(), 0), states(&layout.data(), 1)];
    assert!(states[0].values().any(|states| !states.is_empty()), "the tile has roads and buildings");

    for repeat in 0..tile.repeats {
        let states = &states[(repeat % 2) as usize];
        bench.measure(|| layout.job.set_feature_state(&mut laid_out, states));
    }
    bench.per("tile", tile.repeats);
}

/// Liberty as a map holds it once loaded, with the colors of its road and
/// building layers chosen by a `selected` feature state: red where it's
/// true, the layer's own color otherwise. A layer whose color changes with
/// zoom keeps it, since a zoom curve can't depend on feature state.
fn selectable(fixtures: &Fixtures) -> Style {
    let mut style = fixtures.style_value(Tiles::Street);
    let mut selectable = 0;
    for layer in style["layers"].as_array_mut().expect("a style's layers") {
        if !SELECTED.contains(&layer["source-layer"].as_str().unwrap_or_default()) {
            continue;
        }
        let property = match layer["type"].as_str() {
            Some("line") => "line-color",
            Some("fill") => "fill-color",
            Some("fill-extrusion") => "fill-extrusion-color",
            _ => continue,
        };
        let Some(color) = layer["paint"].get(property).filter(|color| color.is_string()).cloned() else { continue };
        let selected = serde_json::json!(["boolean", ["feature-state", "selected"], false]);
        layer["paint"][property] = serde_json::json!(["case", selected, "#e04040", color]);
        selectable += 1;
    }
    assert!(selectable > 0, "Liberty colors its roads and buildings");
    with_sprite(fixtures, Style::from_json(&registry(), &style.to_string()).expect("the style parses"))
}

/// A state for each road and building of `data`, every other one selected,
/// starting from the `first`th.
fn states(data: &TileData, first: usize) -> LayerFeatureStates {
    let mut states = LayerFeatureStates::default();
    for name in SELECTED {
        let Some(layer) = data.layer(name).expect("a tile that decodes") else { continue };
        let layer_states = states.entry(name.to_owned()).or_default();
        for index in 0..layer.len() {
            let feature = layer.feature(index).expect("a feature that decodes");
            let Some(id) = feature_id_to_string(&feature.get_id()) else { continue };
            let mut state = mln_style::feature::FeatureState::new();
            state.insert("selected".to_owned(), feature::Value::Bool((index + first).is_multiple_of(2)));
            layer_states.insert(id, state);
        }
    }
    states
}
