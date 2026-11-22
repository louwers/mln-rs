use std::sync::{Arc, Once};

use geometry::Point;
use mln_render::globe::subdivide_line;
use mln_render::layer::{BuildCx, EvaluatedLayer, FeaturePatterns, Layout, LayoutCx};
use mln_style::StyleValue as _;
use mln_style::evaluated::Evaluated;
use mln_style::expression::EvaluationContext;
use mln_style_types::LineCapType;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::GeometryTileFeature;
use mln_util::error::Error;
use mln_util::feature::numeric_value_f64;
use mln_util::geometry::FeatureType;
use mln_util::logging::{Event, Log};

use super::bucket::{DashVertex, FeatureDashes, LineBucket};
use super::dashes::DashCap;
use super::mesh::{LineOptions, LineScratch, add_line};
use super::{Line, LineLayoutProperties};
use crate::features::{uses_pattern, with_patterns};

/// The features a tile's lines draw, by index, in `line-sort-key` order,
/// each with its patterns, and the layout they draw with.
pub struct LineLayout {
    features: Vec<(usize, FeaturePatterns)>,
    layout: LineLayoutProperties::PossiblyEvaluated,
    /// The tile's zoom, and how many times it magnifies its data's zoom.
    zoom: f32,
    overscaling: u32,
}

impl Layout<Line> for LineLayout {
    type Bucket = LineBucket;

    /// Whether a layer of the group has a pattern, whose images it waits
    /// for.
    fn needs_resources(group: &[Arc<EvaluatedLayer<Line>>]) -> bool {
        uses_pattern(group, |paint| &paint.line_pattern)
    }

    fn new(cx: &mut LayoutCx<'_, Line>) -> Result<Self, Error> {
        let style = &cx.layers()[0].style.layout;
        let layout = style.evaluate(cx.parameters());
        let sorted = !style.line_sort_key.is_undefined();
        let features = with_patterns(cx, |paint| &paint.line_pattern, sorted.then_some(&layout.line_sort_key))?;
        let tile = cx.tile();
        Ok(LineLayout { features, layout, zoom: f32::from(tile.overscaled_z), overscaling: tile.overscale_factor() })
    }

    fn build(self, cx: &mut BuildCx<'_, Line>) -> Result<Option<LineBucket>, Error> {
        let tile = cx.tile();
        let granularity = cx.subdivision_granularity().line.granularity_for_zoom_level(tile.z);
        // The dash arrays of the layers whose dash array differs by
        // feature, and their dash patterns in the bucket.
        let (dasharrays, dashes): (Vec<Evaluated<Vec<f32>>>, Vec<FeatureDashes>) = cx
            .layers()
            .iter()
            .filter(|layer| !layer.paint.line_dasharray.is_constant())
            .map(|layer| {
                let dashes = FeatureDashes { layer: layer.style.id.clone(), ..FeatureDashes::default() };
                (layer.paint.line_dasharray.clone(), dashes)
            })
            .unzip();
        let round = self.layout.line_cap.constant_or(&LineCapType::Butt) == LineCapType::Round;
        let mut bucket = LineBucket {
            binders: cx.binders(),
            dashes,
            dash_cap: if round { DashCap::Round } else { DashCap::Square },
            ..LineBucket::default()
        };
        let mut scratch = LineScratch::default();
        for (i, patterns) in &self.features {
            let feature = cx.feature(*i)?;
            let geometry = feature.geometry()?;
            for line in geometry.iter() {
                self.add(&mut bucket, line, &feature, &tile, granularity, &mut scratch);
            }
            let vertices = bucket.vertices.len();
            cx.populate_patterns(&mut bucket.binders, *i, &feature, vertices, patterns);
            let context = self.context(&feature, &tile);
            let cap = if self.layout.line_cap.evaluate(&context) == LineCapType::Round {
                DashCap::Round
            } else {
                DashCap::Square
            };
            for (dashes, dasharray) in bucket.dashes.iter_mut().zip(&dasharrays) {
                let entry = dashes.atlas.add(&dasharray.evaluate(&context), cap);
                let vertex = DashVertex([entry.y, entry.height, entry.width, 0.0]);
                dashes.vertices.get_mut().resize(vertices, vertex);
            }
            cx.index(*i, &geometry);
        }
        Ok((!bucket.triangles.is_empty()).then_some(bucket))
    }
}

impl LineLayout {
    /// What the layout's properties evaluate with for `feature` in the tile
    /// `tile`.
    fn context<'a>(&self, feature: &'a dyn GeometryTileFeature, tile: &'a CanonicalTileID) -> EvaluationContext<'a> {
        EvaluationContext::with_zoom_feature(self.zoom, Some(feature)).with_canonical_tile_id(Some(tile))
    }

    /// Adds the mesh of `line`, one of `feature`'s lines or rings, in the
    /// tile `tile`, subdivided for the globe in `granularity` parts where
    /// it is 2 or more, to `bucket`.
    fn add(
        &self,
        bucket: &mut LineBucket,
        line: &[Point<i16>],
        feature: &dyn GeometryTileFeature,
        tile: &CanonicalTileID,
        granularity: u32,
        scratch: &mut LineScratch,
    ) {
        let closed = feature.get_type() == FeatureType::Polygon;
        if line.len() < if closed { 3 } else { 2 } {
            // A line layer can't draw a source's points, such as a GeoJSON
            // `MultiPoint`. The layer and source aren't known here.
            static WARNED_INVALID_GEOMETRY: Once = Once::new();
            WARNED_INVALID_GEOMETRY.call_once(|| Log::warning(Event::General, "Invalid geometry in line layer"));
            return;
        }
        let subdivided;
        let line = if granularity >= 2 {
            subdivided = subdivide_line(line, granularity);
            &subdivided
        } else {
            line
        };
        let clip = match (feature.get_value("mapbox_clip_start"), feature.get_value("mapbox_clip_end")) {
            (Some(start), Some(end)) => numeric_value_f64(&start).zip(numeric_value_f64(&end)),
            _ => None,
        };
        let context = self.context(feature, tile);
        let options = LineOptions {
            closed,
            join: self.layout.line_join.evaluate(&context),
            cap: self.layout.line_cap.evaluate(&context),
            miter_limit: self.layout.line_miter_limit.evaluate(&context),
            round_limit: self.layout.line_round_limit.evaluate(&context),
            overscaling: self.overscaling,
            clip: clip.filter(|(start, end)| start < end),
        };
        add_line(bucket.vertices.get_mut(), &mut bucket.triangles, scratch, line, &options);
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use feature::{Identifier, PropertyMap, Value};
    use mln_style::PropertyEvaluationParameters;
    use mln_style::conversion::{Convertible, Error as ConversionError, convert_layer};
    use mln_style::layer_type::LayerStyle;
    use mln_tile_id::geometry_tile_data::GeometryCollection;
    use serde_json::json;

    use super::*;
    use crate::line::mesh::LineVertex;

    /// A feature of `type_` with `properties`.
    struct Stub(FeatureType, PropertyMap);

    impl GeometryTileFeature for Stub {
        fn get_type(&self) -> FeatureType {
            self.0
        }
        fn get_value(&self, key: &str) -> Option<Value> {
            self.1.get(key).cloned()
        }
        fn get_id(&self) -> Identifier {
            Identifier::Null
        }
        fn get_geometries(&self) -> Result<Cow<'_, GeometryCollection>, Error> {
            Ok(Cow::Owned(GeometryCollection::default()))
        }
    }

    /// The vertices of a line layer with `layout`, at zoom 10, of one feature
    /// of `type_` per `(properties, line)`.
    fn vertices(
        layout: &serde_json::Value,
        type_: FeatureType,
        features: &[(serde_json::Value, &[(i16, i16)])],
    ) -> Vec<LineVertex> {
        let json = json!({"type": "line", "id": "line", "source": "s", "layout": layout});
        let mut error = ConversionError::default();
        let layer = convert_layer(crate::tests::registry().style(), &Convertible::from(&json), &mut error)
            .unwrap_or_else(|| panic!("{}", error.message));
        let style = LayerStyle::<Line>::recover_ref(&**layer.style()).expect("a line layer");
        let layout = style.layout.evaluate(&PropertyEvaluationParameters::new(10.0));
        let layout = LineLayout { features: Vec::new(), layout, zoom: 10.0, overscaling: 1 };
        let (mut bucket, mut scratch) = (LineBucket::default(), LineScratch::default());
        for (properties, line) in features {
            let properties = properties.as_object().expect("properties").iter().map(|(key, value)| {
                let value = match value {
                    serde_json::Value::String(s) => Value::String(s.clone()),
                    v => Value::Double(v.as_f64().expect("a string or number")),
                };
                (key.clone(), value)
            });
            let line: Vec<Point<i16>> = line.iter().map(|&(x, y)| Point::new(x, y)).collect();
            let feature = Stub(type_, properties.collect());
            layout.add(&mut bucket, &line, &feature, &CanonicalTileID::new(0, 0, 0), 1, &mut scratch);
        }
        bucket.vertices.get().to_vec()
    }

    // gtest: Buckets.LineBucket
    #[test]
    fn a_line_has_a_mesh_and_a_point_none() {
        assert_eq!(vertices(&json!({}), FeatureType::Point, &[(json!({}), &[(0, 0)])]).len(), 0);
        assert_ne!(vertices(&json!({}), FeatureType::LineString, &[(json!({}), &[(0, 0), (1, 1)])]).len(), 0);
    }

    #[test]
    fn a_lines_cap_and_limits_evaluate_per_feature() {
        // A sharp corner: its miter length is about 3.2, so a miter limit of
        // 2 bevels it and 10 doesn't, and a round limit of 1 rounds it and 10
        // doesn't.
        const CORNER: &[(i16, i16)] = &[(0, 0), (100, 400), (200, 0)];
        for (property, values) in [
            ("line-cap", [json!("butt"), json!("round")]),
            ("line-miter-limit", [json!(2), json!(10)]),
            ("line-round-limit", [json!(1), json!(10)]),
        ] {
            let join = if property == "line-round-limit" { "round" } else { "miter" };
            let line = FeatureType::LineString;
            let constant = |value: &serde_json::Value| {
                vertices(&json!({"line-join": join, property: value}), line, &[(json!({}), CORNER)])
            };
            let (first, second) = (constant(&values[0]), constant(&values[1]));
            assert_ne!(first, second, "{property}: the two values make the same line");
            let data_driven = vertices(
                &json!({"line-join": join, property: ["get", "v"]}),
                line,
                &[(json!({"v": values[0]}), CORNER), (json!({"v": values[1]}), CORNER)],
            );
            assert_eq!(data_driven, [first, second].concat(), "{property}");
        }
    }
}
