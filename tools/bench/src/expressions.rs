//! The expression cases: evaluating parsed expressions over features, and
//! parsing a style's layers, without a map or a renderer.

use std::hint::black_box;
use std::sync::Arc;

use feature::{Feature, Identifier, Value};
use mln_style::StyleRegistry;
use mln_style::conversion::{Convertible, Error, convert_layer};
use mln_style::expression::expression::with_geojson_feature;
use mln_style::expression::{EvaluationContext, Expr, Functions, ParsingContext};

use crate::meter::Meter;
use crate::scene::basemap_style;
use crate::{Context, Metrics};

/// What `Expression/Evaluate` evaluates: zoom curves, feature lookups,
/// arithmetic, strings, colors and filters, the kinds of expressions a
/// style's properties and filters hold.
const EXPRESSIONS: [&str; 12] = [
    r#"["interpolate", ["linear"], ["zoom"], 5, 1, 10, ["*", 2, ["get", "rank"]]]"#,
    r#"["*", ["get", "population"], 0.001]"#,
    r#"["+", ["^", ["get", "rank"], 2], ["sqrt", ["get", "density"]], ["%", ["get", "rank"], 3]]"#,
    r#"["min", ["max", ["-", ["get", "density"], 10], 0], ["/", ["get", "population"], 1000]]"#,
    r#"["round", ["*", ["log2", ["+", 1, ["get", "population"]]], ["abs", ["-", ["get", "rank"], 5]]]]"#,
    r#"["concat", ["upcase", ["get", "name"]], " ", ["to-string", ["get", "rank"]]]"#,
    r#"["coalesce", ["get", "name:en"], ["downcase", ["get", "name"]]]"#,
    r#"["match", ["get", "class"], ["city", "town"], 1, "village", 2, 3]"#,
    r#"["case", [">", ["get", "rank"], 5], ["get", "density"], ["<", ["get", "population"], 500], 0, 1]"#,
    r#"["all", ["has", "name"], [">=", ["get", "rank"], 2], ["!=", ["get", "class"], "hamlet"]]"#,
    r#"["interpolate", ["linear"], ["get", "density"], 0, ["rgb", 255, 0, 0], 100, ["rgb", 0, 0, ["get", "rank"]]]"#,
    r#"["step", ["get", "population"], "small", 1000, ["concat", "big ", ["get", "class"]]]"#,
];

const FEATURES: usize = 10_000;

/// How many times `Expression/Parse` parses the style's layers per
/// iteration, which takes about as long as a frame.
const PARSES: usize = 10;

/// A feature with a name (half with an English one), a class, a rank, a
/// population and a density, which vary over the features.
fn feature(i: usize) -> Feature {
    let classes = ["city", "town", "village", "hamlet"];
    let mut feature: Feature = Feature { id: Identifier::Uint(i as u64), ..Feature::default() };
    let properties = &mut feature.properties;
    properties.insert("name".to_owned(), Value::String(format!("Place {i}")));
    if i.is_multiple_of(2) {
        properties.insert("name:en".to_owned(), Value::String(format!("Town {i}")));
    }
    properties.insert("class".to_owned(), Value::String(classes[i % classes.len()].to_owned()));
    properties.insert("rank".to_owned(), Value::Uint((i % 10) as u64));
    properties.insert("population".to_owned(), Value::Uint((i * 37 % 100_000) as u64));
    properties.insert("density".to_owned(), Value::Double((i % 200) as f64 / 2.0));
    feature
}

/// Parses `json` as an expression of any type.
fn parse_expression(json: &str) -> Expr {
    let document: serde_json::Value = serde_json::from_str(json).expect("an expression is JSON");
    let mut ctx = ParsingContext::new(&Functions::with_builtins());
    let parsed = ctx.parse_expression(&Convertible::from(&document), None);
    parsed.unwrap_or_else(|| panic!("{json}: {}", ctx.get_combined_errors()))
}

/// `EXPRESSIONS`, evaluated typed at zoom 8 over `FEATURES` features.
pub(crate) fn evaluate(cx: &Context) -> Metrics {
    let expressions: Vec<Expr> = EXPRESSIONS.iter().map(|json| parse_expression(json)).collect();
    let features: Vec<Feature> = (0..FEATURES).map(feature).collect();
    let mut meter = Meter::new();
    for _ in 0..cx.iterations(20) {
        meter.time(|| {
            for feature in &features {
                with_geojson_feature(feature, EvaluationContext::new(8.0), |cx| {
                    for expression in &expressions {
                        let _ = black_box(expression.evaluate_typed(cx));
                    }
                });
            }
        });
    }
    meter.finish()
}

/// Every layer of the basemap's style, parsed with its filter and property
/// values, `PARSES` times over.
pub(crate) fn parse(cx: &Context) -> Metrics {
    let style: serde_json::Value = serde_json::from_str(&basemap_style(&cx.fixtures)).expect("the style is JSON");
    let layers = style["layers"].as_array().expect("the style has layers");
    let registry: Arc<StyleRegistry> = StyleRegistry::with_builtins();
    let mut meter = Meter::new();
    for _ in 0..cx.iterations(30) {
        meter.time(|| {
            for layer in std::iter::repeat_n(layers, PARSES).flatten() {
                let mut error = Error::default();
                let parsed = convert_layer(&registry, &Convertible::from(layer), &mut error);
                black_box(parsed.unwrap_or_else(|| panic!("a basemap layer: {}", error.message)));
            }
        });
    }
    meter.finish()
}
