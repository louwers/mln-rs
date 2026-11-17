//! Expressions evaluated over features, through the public expression API.

use std::hint::black_box;

use feature::{Feature, Identifier, Value};
use mln_conversion::Convertible;
use mln_expression::expression::with_geojson_feature;
use mln_expression::{EvaluationContext, Expr, Functions, ParsingContext};

use crate::scenario::{Bench, Kind, Scenario};
use crate::support::random::Random;

pub(crate) const SCENARIOS: &[Scenario] = &[Scenario {
    name: "micro/expression/evaluate",
    kind: Kind::Micro,
    question: "What does evaluating a style's kinds of expressions over features cost?",
    run: evaluate,
}];

/// Zoom curves, feature lookups, arithmetic, strings, colors and filters:
/// the kinds of expressions a style's properties and filters hold.
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

const FEATURES: usize = 50_000;

fn evaluate(bench: &mut Bench) {
    let expressions: Vec<Expr> = EXPRESSIONS.iter().map(|json| parse(json)).collect();
    let mut random = Random::new(1);
    let features: Vec<Feature> = (0..FEATURES).map(|id| feature(id, &mut random)).collect();
    bench.measure(|| {
        for feature in &features {
            with_geojson_feature(feature, EvaluationContext::new(8.0), |cx| {
                for expression in &expressions {
                    let _ = black_box(expression.evaluate_typed(cx));
                }
            });
        }
    });
    bench.count("evaluations", (features.len() * expressions.len()) as u64);
}

/// Parses `json` as an expression of any type.
fn parse(json: &str) -> Expr {
    let document: serde_json::Value = serde_json::from_str(json).expect("an expression is JSON");
    let mut context = ParsingContext::new(&Functions::with_builtins());
    let parsed = context.parse_expression(&Convertible::from(&document), None);
    parsed.unwrap_or_else(|| panic!("{json}: {}", context.get_combined_errors()))
}

/// A place: a name, half of them with an English one, a class, a rank, a
/// population and a density.
fn feature(id: usize, random: &mut Random) -> Feature {
    const CLASSES: [&str; 4] = ["city", "town", "village", "hamlet"];
    let mut feature: Feature = Feature { id: Identifier::Uint(id as u64), ..Feature::default() };
    let properties = &mut feature.properties;
    properties.insert("name".to_owned(), Value::String(format!("Place {id}")));
    if random.below(2) == 0 {
        properties.insert("name:en".to_owned(), Value::String(format!("Town {id}")));
    }
    properties.insert("class".to_owned(), Value::String(CLASSES[random.below(CLASSES.len())].to_owned()));
    properties.insert("rank".to_owned(), Value::Uint(random.below(10) as u64));
    properties.insert("population".to_owned(), Value::Uint(random.below(100_000) as u64));
    properties.insert("density".to_owned(), Value::Double(random.range(0.0, 100.0)));
    feature
}
