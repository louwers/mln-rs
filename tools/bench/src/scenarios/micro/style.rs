//! The style: its document parsed into a [`Style`], and edits made to a
//! style and compared, as a renderer compares them, with the version before.

use std::hint::black_box;

use mln_style::diff::StyleDiff;
use mln_style::{Edit, Style};

use super::{liberty, parse_liberty, registry};
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::Fixtures;

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "micro/style/parse/small",
        kind: Kind::Micro,
        question: "What does parsing a style document of 110 layers cost?",
        run: parse_small,
    },
    Scenario {
        name: "micro/style/parse/large",
        kind: Kind::Micro,
        question: "What does parsing a style document of 440 layers cost?",
        run: parse_large,
    },
    Scenario {
        name: "micro/style/edit/small",
        kind: Kind::Micro,
        question: "What do an application's edits to a style of 110 layers cost, each compared with the version before?",
        run: edit_small,
    },
    Scenario {
        name: "micro/style/edit/large",
        kind: Kind::Micro,
        question: "What do an application's edits to a style of 440 layers cost, each compared with the version before?",
        run: edit_large,
    },
];

/// The copies of Liberty's 110 layers in the large style.
const LARGE: usize = 4;

/// The times the document is parsed, so that a run parses enough to count
/// closely.
const PARSES: u64 = 20;

/// Liberty, as a map's style loader parses it: the document's JSON, its
/// sources, every layer with its filter and property values, the sprite and
/// glyph URLs.
fn parse_small(bench: &mut Bench) {
    parse(bench, 1);
}

/// [`parse_small`] with Liberty's layers four times over, a style of many
/// layers.
fn parse_large(bench: &mut Bench) {
    parse(bench, LARGE);
}

fn parse(bench: &mut Bench, copies: usize) {
    let fixtures = Fixtures::open(bench);
    let json = liberty(&fixtures, copies);
    let registry = registry();
    for _ in 0..PARSES {
        let style = bench.measure(|| Style::from_json(&registry, black_box(&json)).expect("Liberty parses"));
        drop(black_box(style));
    }
    bench.per("parse", PARSES);
}

/// The layers edited.
const EDITED: usize = 20;

/// The edits an application makes to a running map's style, to every fifth
/// of Liberty's layers: setting a paint property, hiding and showing the
/// layer, setting its filter (but the background's, which has none), moving
/// it, and adding a layer below it and removing that again. After each, the
/// edited version is compared with the one before ([`StyleDiff::between`]),
/// as the renderer finds what to redo.
fn edit_small(bench: &mut Bench) {
    edit(bench, 1);
}

/// [`edit_small`] on Liberty with its layers four times over: the same
/// edits to a style of many layers.
fn edit_large(bench: &mut Bench) {
    edit(bench, LARGE);
}

fn edit(bench: &mut Bench, copies: usize) {
    let fixtures = Fixtures::open(bench);
    let mut style = parse_liberty(&fixtures, copies);
    let original: Vec<(String, &'static str)> =
        parse_liberty(&fixtures, 1).get_layers().map(|layer| (layer.get_id().to_owned(), layer.info().name)).collect();
    let stride = original.len() / EDITED;

    let mut edits = 0;
    let mut step = |bench: &mut Bench, change: &dyn Fn(&mut Style)| {
        bench.measure(|| {
            let before = style.clone();
            change(&mut style);
            black_box(StyleDiff::between(Some(&before), &style));
        });
        edits += 1;
    };
    for index in (0..EDITED).map(|layer| layer * stride) {
        let (id, type_) = &original[index];
        let color = format!("\"#{:06x}\"", index * 0x02_04_06);
        step(bench, &|style| style.set_layer_property_json(id, color_property(type_), &color).expect("a color"));
        step(bench, &|style| style.set_layer_property_json(id, "visibility", "\"none\"").expect("a visibility"));
        step(bench, &|style| style.set_layer_property_json(id, "visibility", "\"visible\"").expect("a visibility"));
        if *type_ != "background" {
            let filter = format!(r#"["!=", ["get", "class"], "class-{index}"]"#);
            step(bench, &|style| style.set_layer_property_json(id, "filter", &filter).expect("a filter"));
        }
        // Up two places.
        let below = &original[index + 3].0;
        step(bench, &|style| style.move_layer(id, Some(below)).expect("a move"));
        let added = format!("added-{index}");
        let layer = format!(
            r##"{{"id": "{added}", "type": "line", "source": "openmaptiles", "source-layer": "transportation",
                "filter": ["==", ["get", "class"], "primary"], "paint": {{"line-color": "#e04040", "line-width": 3}}}}"##
        );
        step(bench, &|style| style.add_layer_json(&layer, Some(id)).expect("a new layer"));
        step(bench, &|style| style.apply(Edit::RemoveLayer(added.clone())).expect("a removal"));
    }
    bench.per("edit", edits);
}

/// The paint property of a layer of type `type_` that holds its color.
fn color_property(type_: &str) -> &'static str {
    match type_ {
        "background" => "background-color",
        "fill" => "fill-color",
        "fill-extrusion" => "fill-extrusion-color",
        "line" => "line-color",
        "symbol" => "text-color",
        type_ => panic!("Liberty has no {type_} layer"),
    }
}
