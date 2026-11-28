//! A GeoJSON source's style half in a style drawn in a registered
//! projection: the projection it takes, when its GeoJSON loads again, and
//! what it keeps. How the data is tiled is `mln_builtins`' tests'.

use std::sync::Arc;

use geojson::GeoJson;

use super::{GeoJSONOptions, GeoJsonSourceOptions, GeoJsonSourceType, geojson_source};
use crate::conversion::Convertible;
use crate::projection::Projection;
use crate::source::{Source, SourceData};
use crate::tests::util::parse_json;
use crate::{Edit, Style, StyleRegistry};

/// The style of `json`, against the registry of these tests.
fn style(json: &str) -> Style {
    Style::from_json(&StyleRegistry::with_builtins(), json).expect("a style")
}

/// A style drawn in `projection`, a projection type's style JSON, with a
/// GeoJSON source "inline" of the style's GeoJSON, "url" of a URL's and
/// "set", whose data the app sets.
fn style_in(projection: &str) -> Style {
    style(&format!(
        r#"{{"version": 8, "projection": {{"type": {projection}}},
            "sources": {{
                "inline": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [45, 45]}}}},
                "url": {{"type": "geojson", "data": "http://example.com/point.geojson"}},
                "set": {{"type": "geojson", "data": {{"type": "FeatureCollection", "features": []}}}}}},
            "layers": []}}"#
    ))
}

/// The options and the load of the style's GeoJSON source `id`.
fn source(style: &Style, id: &str) -> (GeoJsonSourceOptions, u64) {
    let data = SourceData::<GeoJsonSourceType>::recover(style.get_source(id).expect("the source")).expect("GeoJSON");
    (data.options.clone(), data.load)
}

/// The name of the registered projection that the style's GeoJSON source
/// `id` tiles in.
fn tiled_in(style: &Style, id: &str) -> Option<String> {
    source(style, id).0.crs.map(|crs| crs.crs().name.clone())
}

/// The projection whose `type` is the style JSON `type_`.
fn projection(type_: &str) -> Projection {
    let mut projection = Projection::new();
    let functions = Arc::clone(StyleRegistry::with_builtins().functions());
    assert!(projection.set_property("type", &Convertible::from(&parse_json(type_)), &functions).is_none());
    projection
}

/// The edit that gives the style's GeoJSON source `id` data, as the
/// runtime sets data, built from `kept`, which it keeps.
fn set(id: &str, kept: Option<Arc<GeoJson>>) -> Edit {
    Edit::update_source::<GeoJsonSourceType>(id, "a GeoJSON source", move |data| {
        data.options.url = None;
        data.options.inline = None;
        data.options.kept = kept;
        data.description = Some(Arc::new(()));
        data.load += 1;
        true
    })
}

fn point() -> Arc<GeoJson> {
    Arc::new(r#"{"type": "Point", "coordinates": [45, 45]}"#.parse().expect("GeoJSON"))
}

// gtest: Source.GeoJSONSourceTilesInlineDataInTheStylesProjection
#[test]
fn a_parsed_source_tiles_in_the_styles_registered_projection() {
    let style = style_in(r#""identity""#);
    for id in ["inline", "url", "set"] {
        assert_eq!(Some("identity"), tiled_in(&style, id).as_deref(), "{id}");
    }
    let mercator = style_in(r#""mercator""#);
    assert_eq!(None, tiled_in(&mercator, "inline"));
}

// gtest: Source.GeoJSONSourceTilesDataItGotBeforeJoiningAStyleInTheStylesProjection
#[test]
fn a_source_that_joins_a_style_tiles_in_its_projection() {
    let mut style = style_in(r#""identity""#);
    style.apply(Edit::AddSource(geojson_source("added", GeoJSONOptions::default_options()))).unwrap();
    assert_eq!(Some("identity"), tiled_in(&style, "added").as_deref());
}

// GL JS and MapLibre Native tile a GeoJSON source's data again when the
// projection changes, from the GeoJSON it has: its URL's, which it requests
// again, the style's, or the GeoJSON set on it, which it keeps in a
// registered projection. A source without any keeps its load.
#[test]
fn a_projection_change_loads_again_the_geojson_a_source_has() {
    let mut style = style_in(r#""identity""#);
    style.apply(set("set", None)).unwrap();
    style.apply(Edit::AddSource(geojson_source("empty", GeoJSONOptions::default_options()))).unwrap();
    let before = ["inline", "url", "set", "empty"].map(|id| source(&style, id).1);

    style.apply(Edit::SetProjection(projection(r#""mercator""#))).unwrap();
    let after = ["inline", "url", "set", "empty"].map(|id| source(&style, id).1);
    assert_eq!([before[0] + 1, before[1] + 1, before[2], before[3]], after);
    for id in ["inline", "url", "set", "empty"] {
        assert_eq!(None, tiled_in(&style, id), "{id}");
    }

    style.apply(set("set", Some(point()))).unwrap();
    let kept = source(&style, "set").1;
    style.apply(Edit::SetProjection(projection(r#""identity""#))).unwrap();
    assert_eq!(kept + 1, source(&style, "set").1);
    assert_eq!(Some("identity"), tiled_in(&style, "set").as_deref());
}

// A change between built-in projections leaves the sources as they are.
#[test]
fn a_change_between_built_in_projections_keeps_the_sources() {
    let mut style = style_in(r#""mercator""#);
    let before: Source = style.get_source("inline").unwrap().clone();

    style.apply(Edit::SetProjection(projection(r#""globe""#))).unwrap();
    assert!(Source::ptr_eq(&before, style.get_source("inline").unwrap()));
}

/// Gives the style's source `id` the description its current load found.
fn describe(style: &mut Style, id: &str) {
    let source = style.get_source(id).unwrap();
    let (instance, load) = (source.instance(), source.data().load());
    let description: crate::source::ErasedDescription = Arc::new(());
    style
        .apply(Edit::SetSourceDescription { id: id.to_owned(), instance, load, description: Some(description) })
        .unwrap();
}

// MapLibre Native's "GeoJSON tiled in Mercator is not kept": the GeoJSON set
// on a source stays while the style is in a registered projection, and goes
// once the source has tiled it in a built-in one.
#[test]
fn kept_geojson_goes_once_it_is_tiled_in_a_built_in_projection() {
    let mut style = style_in(r#""identity""#);
    style.apply(set("set", Some(point()))).unwrap();
    describe(&mut style, "set");
    assert!(source(&style, "set").0.kept.is_some());

    style.apply(Edit::SetProjection(projection(r#""mercator""#))).unwrap();
    assert!(source(&style, "set").0.kept.is_some(), "kept until it is tiled again");
    describe(&mut style, "set");
    assert!(source(&style, "set").0.kept.is_none());
}

// A URL replaces the GeoJSON that a source keeps.
#[test]
fn a_url_replaces_the_kept_geojson() {
    let mut style = style_in(r#""identity""#);
    style.apply(set("set", Some(point()))).unwrap();
    style.apply(GeoJsonSourceType::set_url("set", "http://example.com/point.geojson")).unwrap();
    assert!(source(&style, "set").0.kept.is_none());
}
