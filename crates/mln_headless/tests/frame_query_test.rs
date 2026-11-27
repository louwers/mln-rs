// A rendered-feature query of a layer of the tests' own type, which draws
// without a source: it answers for the view, its hits take the query's
// filter, `within` included, and they come in the layers' order with a
// circle layer's.

use mln_engine::exec::pool::Pool;
use mln_engine::exec::queue::Queue;
use mln_engine::geo::ScreenCoordinate;
use mln_engine::map::{Map, MapMode, MapOptions};
use mln_engine::query::RenderedQueryOptions;
use mln_headless::HeadlessFrontend;
use mln_style::conversion::{Convertible, Error};
use mln_style::expression::Functions;
use mln_style::filter::{Filter, convert_filter};
use mln_style::size::Size;

/// The tests' `org.example.mark` layer type, whose layers show a mark 20
/// pixels across at 0, 0 that a query finds as a point there.
mod marks {
    use std::sync::Arc;

    use mln_engine::layer::{EvaluateCx, Evaluation, FrameQuery, Layer, LayerValues, NoTiles};
    use mln_engine::tile::feature::Feature;
    use mln_engine::{Registry, RegistryBuilder};
    use mln_style::feature::{PropertyMap, Value};
    use mln_style::geo::LatLng;
    use mln_style::layer::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
    use mln_style::property::table::{PropertyTable, PropertyVisitor};

    /// The built-ins and the mark layer type.
    pub(super) fn registry() -> Arc<Registry> {
        let mut builder = RegistryBuilder::new();
        mln_builtins::register_builtins(&mut builder).unwrap();
        builder.add_layer_type::<Marking>().unwrap();
        builder.build()
    }

    pub(super) struct Mark;

    static PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

    impl StyleLayerType for Mark {
        const INFO: LayerTypeInfo =
            LayerTypeInfo { name: "org.example.mark", source: SourceKind::None, from_json: true };
        const PROPERTIES: &'static PropertyTable = &PROPERTIES;

        type LayoutProperties = NoProperties;
        type PaintProperties = NoProperties;
        type Extra = ();

        fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
            None
        }
    }

    struct Marking;

    impl Layer for Marking {
        type Style = Mark;
        type Input = NoTiles;
        type Tile = ();

        fn new(_values: &LayerValues<Mark>) -> Self {
            Marking
        }

        fn evaluate(&mut self, _cx: EvaluateCx<'_, Self>) -> Evaluation {
            Evaluation::TRANSLUCENT
        }

        fn hit_frame(&self, query: &FrameQuery<'_, Self>, hits: &mut Vec<Feature>) {
            let view = query.view();
            // The view's screen points are measured from the bottom edge.
            let at = view.lat_lng_to_screen_coordinate(&LatLng::default());
            let (x, y) = (at.x, f64::from(view.size().height) - at.y);
            if query.geometry().iter().any(|point| (point.x - x).abs() <= 10.0 && (point.y - y).abs() <= 10.0) {
                let mut properties = PropertyMap::default();
                properties.insert("kind".to_owned(), Value::String("mark".to_owned()));
                let geometry = geo_types::Geometry::Point(geo_types::Point::new(0.0, 0.0));
                hits.push(Feature { geometry: Some(geometry), properties, ..Feature::default() });
            }
        }
    }
}

const SIDE: u32 = 256;

/// A frontend that drew a still of a style whose layers are `layers`, over a
/// GeoJSON source `point` of a point at 0, 0, centered there at zoom 4.
fn render(layers: &str, run_loop: &Queue<()>) -> HeadlessFrontend {
    let size = Size::new(SIDE, SIDE);
    let pool = Pool::new(2);
    let mut frontend = HeadlessFrontend::with_pixel_ratio(1.0, run_loop, &pool).expect("a frontend");
    let options = MapOptions { map_mode: MapMode::Static, size, pixel_ratio: 1.0, ..Default::default() };
    let mut map = Map::new(options, marks::registry(), &pool).unwrap();
    frontend.attach(&mut map);
    map.load_style_json(&format!(
        r#"{{
            "version": 8,
            "center": [0, 0],
            "zoom": 4,
            "sources": {{"point": {{"type": "geojson", "data": {{"type": "Point", "coordinates": [0, 0]}}}}}},
            "layers": {layers}
        }}"#
    ));
    frontend.render(&mut map).expect("a frame");
    frontend
}

const CIRCLE: &str = r#"{"id": "circle", "type": "circle", "source": "point", "paint": {"circle-radius": 20}}"#;
const MARK: &str = r#"{"id": "mark", "type": "org.example.mark"}"#;

fn center() -> ScreenCoordinate {
    ScreenCoordinate::new(f64::from(SIDE) / 2.0, f64::from(SIDE) / 2.0)
}

fn parse_filter(json: &str) -> Filter {
    let json: serde_json::Value = serde_json::from_str(json).expect("filter JSON");
    let mut error = Error::default();
    convert_filter(&Convertible::from(&json), &Functions::with_builtins(), &mut error)
        .unwrap_or_else(|| panic!("filter: {}", error.message))
}

/// The ids of the layers of what a query at `point` with `filter` finds.
fn found(frontend: &mut HeadlessFrontend, point: ScreenCoordinate, filter: Option<&str>) -> Vec<String> {
    let options = RenderedQueryOptions::new(None, filter.map(parse_filter));
    let features = frontend.renderer().query_rendered_features_at_point(&point, &options).unwrap();
    features.into_iter().map(|feature| feature.layer).collect()
}

#[test]
fn a_layer_without_tiles_answers_a_query_from_the_frame() {
    let run_loop = Queue::new();
    let mut frontend = render(&format!("[{MARK}]"), &run_loop);
    assert_eq!(found(&mut frontend, center(), None), ["mark"]);
    let beside = ScreenCoordinate::new(center().x + 30.0, center().y);
    assert_eq!(found(&mut frontend, beside, None), Vec::<String>::new());

    let options = RenderedQueryOptions::default();
    let features = frontend.renderer().query_rendered_features_at_point(&center(), &options).unwrap();
    assert_eq!(features[0].properties["kind"], mln_style::feature::Value::String("mark".to_owned()));
}

#[test]
fn the_query_filter_takes_or_drops_a_frame_hit() {
    let run_loop = Queue::new();
    let mut frontend = render(&format!("[{MARK}]"), &run_loop);
    assert_eq!(found(&mut frontend, center(), Some(r#"["==", ["get", "kind"], "mark"]"#)), ["mark"]);
    assert_eq!(found(&mut frontend, center(), Some(r#"["==", ["get", "kind"], "other"]"#)), Vec::<String>::new());
}

#[test]
fn within_places_a_frame_hit_without_a_tile() {
    let run_loop = Queue::new();
    let mut frontend = render(&format!("[{MARK}]"), &run_loop);
    let within = |west: f64, east: f64| {
        let ring = format!("[[{west},-1],[{east},-1],[{east},1],[{west},1],[{west},-1]]");
        format!(r#"["within", {{"type": "Polygon", "coordinates": [{ring}]}}]"#)
    };
    assert_eq!(found(&mut frontend, center(), Some(&within(-0.5, 0.5))), ["mark"]);
    assert_eq!(found(&mut frontend, center(), Some(&within(0.5, 1.5))), Vec::<String>::new());
}

#[test]
fn frame_hits_come_in_the_layers_order_with_tile_features() {
    let run_loop = Queue::new();
    let mut frontend = render(&format!("[{CIRCLE}, {MARK}]"), &run_loop);
    assert_eq!(found(&mut frontend, center(), None), ["mark", "circle"]);
    let mut frontend = render(&format!("[{MARK}, {CIRCLE}]"), &run_loop);
    assert_eq!(found(&mut frontend, center(), None), ["circle", "mark"]);
}
