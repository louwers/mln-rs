//! Expressions: Liberty's filters and property expressions parsed, each into
//! the type its property takes, and evaluated over a tile's features.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::hint::black_box;
use std::sync::Arc;

use feature::{Identifier, PropertyMap, Value};
use mln_style::conversion::Convertible;
use mln_style::error::Error;
use mln_style::expression::{EvaluationContext, Expr, Functions, ParsingContext, Type, is_expression};
use mln_style::feature::{GeometryCollection, GeometryTileFeature};
use mln_style::geometry::FeatureType;
use mln_tile::TileData;

use super::{DENSE, tile_bytes, tile_data};
use crate::scenario::{Bench, Kind, Scenario};
use crate::support::fixtures::{Fixtures, Tiles};

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "micro/expression/parse/liberty",
        kind: Kind::Micro,
        question: "What does parsing a style's filters and property expressions cost?",
        run: parse,
    },
    Scenario {
        name: "micro/expression/evaluate/liberty",
        kind: Kind::Micro,
        question: "What does evaluating a style's filters and property expressions over a tile's features cost?",
        run: evaluate,
    },
];

/// One of Liberty's expressions: a layer's filter, or the value of one of
/// its properties.
struct Source {
    /// The layer's source layer, empty for one that has none.
    source_layer: String,
    json: serde_json::Value,
    /// The type its property takes.
    expected: Type,
    /// A filter, which parses as a filter rather than a property value.
    filter: bool,
}

/// Every filter and every expression-valued paint and layout property of
/// Liberty's layers.
fn sources(fixtures: &Fixtures, functions: &Arc<Functions>) -> Vec<Source> {
    let style = fixtures.style_value(Tiles::Street);
    let mut sources = Vec::new();
    for layer in style["layers"].as_array().expect("a style's layers") {
        let source_layer = layer["source-layer"].as_str().unwrap_or_default().to_owned();
        if let Some(filter) = layer.get("filter") {
            let json = filter.clone();
            sources.push(Source { source_layer: source_layer.clone(), json, expected: Type::Boolean, filter: true });
        }
        for scope in ["layout", "paint"] {
            let Some(properties) = layer.get(scope).and_then(serde_json::Value::as_object) else { continue };
            for (name, value) in properties {
                if is_expression(&Convertible::from(value), functions) {
                    let json = value.clone();
                    let expected = property_type(name);
                    sources.push(Source { source_layer: source_layer.clone(), json, expected, filter: false });
                }
            }
        }
    }
    sources
}

/// The type that a property of Liberty whose value is an expression takes,
/// as the style spec gives it.
fn property_type(name: &str) -> Type {
    match name {
        "text-field" => Type::Formatted,
        "icon-image" => Type::Image,
        "text-font" => Type::array(Type::String),
        "text-offset" => Type::array_n(Type::Number, 2),
        "line-dasharray" => Type::array(Type::Number),
        "symbol-placement" | "text-anchor" | "text-transform" | "line-cap" | "line-join" | "visibility" => Type::String,
        name if name.ends_with("-color") => Type::Color,
        _ => Type::Number,
    }
}

/// Parses `source` as the style parses its filter or property value.
fn parse_one(source: &Source, functions: &Arc<Functions>) -> Option<Expr> {
    let mut context = ParsingContext::with_expected(functions, source.expected.clone());
    let value = Convertible::from(&source.json);
    if source.filter { context.parse_expression(&value, None) } else { context.parse_layer_property_expression(&value) }
}

/// The times the expressions are parsed, so that a run parses enough to
/// count closely.
const PARSES: u64 = 50;

/// Liberty's 108 filters and 126 expression-valued properties, parsed each
/// into the type its property takes, as the style parses them. One size: an
/// expression parses on its own, whatever else the style holds.
fn parse(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let functions = Functions::with_builtins();
    let sources = sources(&fixtures, &functions);
    for source in &sources {
        assert!(parse_one(source, &functions).is_some(), "{} parses as {:?}", source.json, source.expected);
    }
    for _ in 0..PARSES {
        let parsed: Vec<Option<Expr>> =
            bench.measure(|| sources.iter().map(|source| parse_one(black_box(source), &functions)).collect());
        drop(black_box(parsed));
    }
    bench.per("expression", PARSES * sources.len() as u64);
}

/// The times the expressions are evaluated over the tile's features.
const EVALUATIONS: u64 = 4;

/// Each of Liberty's filters and property expressions evaluated at zoom 14
/// over every feature of its layer's source layer in the street tiles'
/// densest tile ([`DENSE`]), whose features are decoded beforehand, so that
/// only the expressions' work counts. One size: each evaluation reads one
/// feature, whatever the tile holds.
fn evaluate(bench: &mut Bench) {
    let fixtures = Fixtures::open(bench);
    let functions = Functions::with_builtins();
    let data = tile_data(Tiles::Street, &tile_bytes(&fixtures, Tiles::Street, DENSE.id));
    let mut features: BTreeMap<String, Vec<Decoded>> = BTreeMap::new();
    let mut expressions: Vec<(Expr, String)> = Vec::new();
    for source in sources(&fixtures, &functions) {
        if source.source_layer.is_empty() {
            continue;
        }
        let expression = parse_one(&source, &functions).expect("Liberty's expressions parse");
        features.entry(source.source_layer.clone()).or_insert_with(|| decoded(&data, &source.source_layer));
        expressions.push((expression, source.source_layer));
    }
    let expressions: Vec<(Expr, &[Decoded])> =
        expressions.into_iter().map(|(expression, layer)| (expression, &*features[&layer])).collect();
    let zoom = f32::from(DENSE.id.0);
    for _ in 0..EVALUATIONS {
        bench.measure(|| {
            for (expression, features) in &expressions {
                for feature in *features {
                    let context = EvaluationContext::with_zoom_feature(zoom, Some(feature));
                    let _ = black_box(expression.evaluate_typed(&context));
                }
            }
        });
    }
    let evaluations: usize = expressions.iter().map(|(_, features)| features.len()).sum();
    bench.per("evaluation", EVALUATIONS * evaluations as u64);
}

/// The features of `source_layer` in `data`, decoded into features that
/// hold their values, as a GeoJSON source's features do.
fn decoded(data: &TileData, source_layer: &str) -> Vec<Decoded> {
    let Some(layer) = data.layer(source_layer).expect("a tile that decodes") else {
        return Vec::new();
    };
    (0..layer.len())
        .map(|index| {
            let feature = layer.feature(index).expect("a feature that decodes");
            Decoded {
                type_: feature.get_type(),
                properties: feature.get_properties().into_owned(),
                id: feature.get_id(),
                geometry: feature.geometry().expect("a geometry that decodes"),
            }
        })
        .collect()
}

/// A tile's feature, decoded.
struct Decoded {
    type_: FeatureType,
    properties: PropertyMap,
    id: Identifier,
    geometry: GeometryCollection,
}

impl GeometryTileFeature for Decoded {
    fn get_type(&self) -> FeatureType {
        self.type_
    }

    fn get_value(&self, key: &str) -> Option<Value> {
        self.properties.get(key).cloned()
    }

    fn get_properties(&self) -> Cow<'_, PropertyMap> {
        Cow::Borrowed(&self.properties)
    }

    fn get_id(&self) -> Identifier {
        self.id.clone()
    }

    fn get_geometries(&self) -> Result<Cow<'_, GeometryCollection>, Error> {
        Ok(Cow::Borrowed(&self.geometry))
    }
}
