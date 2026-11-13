// Ports src/mln/renderer/buckets/fill_extrusion_bucket.hpp and
// fill_extrusion_bucket.cpp (`MLN_USE_FILL_EXTRUSION_INSTANCING`, on every
// backend), plus the `PatternLayout` instantiation of
// FillExtrusionLayerFactory::createLayout (layermanager/fill_extrusion_layer_factory.cpp).
//
// The vertex/index vectors are plain `Vec`s (the bucket is built on the
// worker thread and shared with `Arc`); the render layer copies them into
// the shared gfx vectors its drawables reference.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use geometry::Point;
use mln_gfx::UploadPass;
use mln_gfx::shaders::segment::{MAX_SEGMENT_VERTICES, SegmentBase};
use mln_map_types::TransformState;
use mln_style::PropertyEvaluationParameters;
use mln_style::expression::{Image, Value};
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::fill_extrusion_layer_properties::{
    FillExtrusionBase, FillExtrusionColor, FillExtrusionHeight, FillExtrusionLayerProperties,
    FillExtrusionLayoutProperties, FillExtrusionPaintProperties, FillExtrusionPattern,
    FillExtrusionRoundedCornerDistance, FillExtrusionTranslate,
};
use mln_style::light::{LightAnchor, LightColor, LightIntensity, LightPosition};
use mln_style::properties::Property;
use mln_style::renderer::possibly_evaluated_property_value::PossiblyEvaluatedFadedPropertyValue;
use mln_style_types::LightAnchorType;
use mln_style_types::image::ImagePositions;
use mln_tile::geometry_tile_data::{classify_rings, limit_holes, round_polygon_corners};
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinates, GeometryTileFeature};
use mln_util::Color;
use mln_util::error::Error;
use mln_util::mat3::{Mat3, matrix as mat3};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::vectors::Vec3f;
use mln_vec_map::VecMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::bucket::{Bucket, BucketBase};
use crate::bucket_geometry::GeometryVec;
use crate::gfx::fill_large_mesh_arrays::fill_large_mesh_arrays;
use crate::layout::pattern_layout::{PatternLayerMap, PatternLayoutSpec};
use crate::paint_property_binder::{CrossFadedPaintPropertyBinder, PaintPropertyBinder};
use crate::render_layer::RenderLayer;
use crate::util::subdivision::{SubdivisionResult, subdivide_polygon, subdivide_vertex_line};
use crate::util::triangulate::triangulate;

crate::paint_property_binders!(
    /// `FillExtrusionBinders = PaintPropertyBinders<FillExtrusionPaintProperties::DataDrivenProperties>`.
    pub struct FillExtrusionBinders for FillExtrusionPaintProperties::PossiblyEvaluated {
        fill_extrusion_base: FillExtrusionBase => PaintPropertyBinder<f32>,
        fill_extrusion_color: FillExtrusionColor => PaintPropertyBinder<Color>,
        fill_extrusion_height: FillExtrusionHeight => PaintPropertyBinder<f32>,
        fill_extrusion_pattern: FillExtrusionPattern => CrossFadedPaintPropertyBinder<Image>,
    }
);

/// `FillExtrusionLayoutVertex = gfx::Vertex<TypeList<attributes::pos,
/// attributes::decimals_ed>>`: a vertex of the roof, and the wall instance
/// from it to the next vertex of its ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct FillExtrusionLayoutVertex {
    pub a1: [i16; 2],
    pub a2: [u16; 2],
}

/// `FillExtrusionStaticVertex = gfx::Vertex<TypeList<attributes::pos>>`: a
/// corner of the wall quad every instance draws, x picking the instance's
/// vertex (0) or the next one (1), y the base (0) or the top (1).
pub type FillExtrusionStaticVertex = [i16; 2];

/// `EvaluatedLight = style::LightProperties::PossiblyEvaluated`.
pub use crate::render_light::EvaluatedLight;

#[derive(Serialize, Deserialize)]
pub struct FillExtrusionBucket {
    pub base: BucketBase,
    pub layout: FillExtrusionLayoutProperties::PossiblyEvaluated,

    pub vertices: GeometryVec<FillExtrusionLayoutVertex>,
    /// `gfx::IndexVector<gfx::Triangles>`.
    pub triangles: GeometryVec<u16>,
    pub triangle_segments: Vec<SegmentBase>,
    pub subdivision_granularity: SubdivisionGranularitySetting,

    /// Behind `RwLock`s: `update` (feature state) changes them while the
    /// bucket is shared.
    pub paint_property_binders: VecMap<String, RwLock<FillExtrusionBinders>>,
}

static EMPTY_FORMATTED_SECTION: Value = Value::Null;

/// `util::dist<uint16_t>(a, b)`: `static_cast<uint16_t>` of the double
/// distance (x86-64: through a 32-bit conversion, keeping the low bits).
fn dist_u16(a: Point<f64>, b: Point<f64>) -> u16 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    ((dx * dx + dy * dy).sqrt() as i64) as u16
}

impl FillExtrusionBucket {
    /// `FillExtrusionBucket(layout, layerPaintProperties, zoom, overscaling)`.
    #[must_use]
    pub fn new(
        layout: FillExtrusionLayoutProperties::PossiblyEvaluated,
        layer_paint_properties: &VecMap<String, Arc<dyn LayerProperties>>,
        zoom: f32,
        _overscaling: u32,
    ) -> Self {
        let mut paint_property_binders = VecMap::new();
        for (id, properties) in layer_paint_properties {
            paint_property_binders.entry(id.clone()).or_insert_with(|| {
                RwLock::new(FillExtrusionBinders::new(get_evaluated::<FillExtrusionLayerProperties>(properties), zoom))
            });
        }
        FillExtrusionBucket {
            base: BucketBase::default(),
            layout,
            vertices: GeometryVec::default(),
            triangles: GeometryVec::default(),
            triangle_segments: Vec::new(),
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            paint_property_binders,
        }
    }

    pub fn set_subdivision_granularity(&mut self, value: &SubdivisionGranularitySetting) {
        self.subdivision_granularity = *value;
    }

    /// `layoutVertex(p, edgeDistance, isDiscarded)`.
    #[must_use]
    pub fn layout_vertex(p: Point<f64>, edge_distance: u16, is_discarded: bool) -> FillExtrusionLayoutVertex {
        let int_part = Point::new(p.x.floor(), p.y.floor());
        // Multiply factional part by 2^7 to pack them into integers [0..127]
        let frac_x = ((p.x - int_part.x) * 128.0) as u8;
        let frac_y = ((p.y - int_part.y) * 128.0) as u8;

        FillExtrusionLayoutVertex {
            a1: [int_part.x as i16, int_part.y as i16],
            a2: [
                // We pack a bool (`isDiscarded`) indicating whether this
                // instance is discarded
                ((i32::from(frac_x) * 256 + i32::from(frac_y)) * 2 + i32::from(is_discarded)) as u16,
                // The edgeDistance attribute is used for wrapping
                // fill_extrusion patterns
                edge_distance,
            ],
        }
    }

    /// Appends the outline of `polygon`: each vertex of a ring is a wall
    /// instance up to the next one, the last vertex ends the ring.
    fn add_outline<T: Copy + Into<f64>, R: AsRef<[Point<T>]>>(&mut self, polygon: &[R]) {
        let vertices = self.vertices.get_mut();
        for ring in polygon {
            let ring = ring.as_ref();
            let mut edge_distance: usize = 0;
            for (i, p) in ring.iter().enumerate() {
                let p1 = Point::new(p.x.into(), p.y.into());
                let p2 = ring.get(i + 1).map(|p| Point::new(p.x.into(), p.y.into()));

                // processRingPoints(p1, p2, edgeDistance)
                vertices.push(Self::layout_vertex(p1, edge_distance as u16, p2.is_none()));

                if let Some(p2) = p2 {
                    let dist = usize::from(dist_u16(p1, p2));
                    if edge_distance + dist > usize::from(u16::MAX) {
                        edge_distance = 0;
                    }

                    edge_distance += dist;
                }
            }
        }
    }

    /// Adds the walls and the roof of one polygon (its outer ring, then its
    /// holes), with integer or, after corner rounding, float coordinates.
    /// The outline's vertices are the roof's, unless a subdivided `roof`
    /// (the globe's) is that mesh, with vertices of its own: the outline
    /// then stays out of the segments, as only its walls draw it.
    fn add_polygon<T: Copy + Into<f64>, R: AsRef<[Point<T>]>>(
        &mut self,
        polygon: &[R],
        roof: Option<&SubdivisionResult>,
    ) -> Result<(), Error> {
        let mut total_vertices: usize = 0;
        for ring in polygon {
            total_vertices += ring.as_ref().len();
            if total_vertices > usize::from(u16::MAX) {
                return Err(crate::bucket::geometry_too_long());
            }
        }

        if total_vertices == 0 {
            return Ok(());
        }

        if let Some(roof) = roof {
            self.add_outline(polygon);
            // The roof's vertices have no walls.
            fill_large_mesh_arrays(
                self.vertices.get_mut(),
                |x, y| Self::layout_vertex(Point::new(f64::from(x), f64::from(y)), 0, true),
                &mut self.triangle_segments,
                self.triangles.get_mut(),
                &roof.vertices,
                &roof.triangle_indices,
                None,
                MAX_SEGMENT_VERTICES,
            );
            return Ok(());
        }

        // The roof continues the last segment if that ends at the outline
        // and has room for it.
        let start_vertices = self.vertices.len();
        let continues = self.triangle_segments.last().is_some_and(|last| {
            last.vertex_offset + last.vertex_length == start_vertices
                && last.vertex_length + total_vertices <= MAX_SEGMENT_VERTICES
        });
        if !continues {
            self.triangle_segments.push(SegmentBase::new(start_vertices, self.triangles.len()));
        }
        let segment_index = self.triangle_segments.len() - 1;
        let first_index = self.triangle_segments[segment_index].vertex_length;

        self.add_outline(polygon);

        let indices: Vec<u32> = triangulate(polygon);
        debug_assert!(indices.len().is_multiple_of(3));
        let index = |i: u32| (first_index + i as usize) as u16;
        for triangle in indices.as_chunks::<3>().0 {
            // Counter-Clockwise winding order.
            self.triangles.get_mut().extend_from_slice(&[index(triangle[0]), index(triangle[2]), index(triangle[1])]);
        }

        let segment = &mut self.triangle_segments[segment_index];
        segment.vertex_length += total_vertices;
        segment.index_length += indices.len();
        Ok(())
    }

    /// `lightColor(const EvaluatedLight&)`.
    #[must_use]
    pub fn light_color(light: &EvaluatedLight) -> [f32; 3] {
        let color = light.get::<LightColor>();
        [color.r, color.g, color.b]
    }

    /// `lightPosition(const EvaluatedLight&, const TransformState&)`.
    pub fn light_position(light: &EvaluatedLight, state: &TransformState) -> [f32; 3] {
        let mut light_pos: Vec3f = light.get::<LightPosition>().get_cartesian();
        let mut light_mat: Mat3 = [0.0; 9];
        mat3::identity(&mut light_mat);
        if *light.get::<LightAnchor>() == LightAnchorType::Viewport {
            let a = light_mat;
            mat3::rotate(&mut light_mat, &a, -state.get_bearing());
        }
        let a = light_pos;
        mat3::transform_mat3f(&mut light_pos, &a, &light_mat);
        light_pos
    }

    /// `lightIntensity(const EvaluatedLight&)`.
    #[must_use]
    pub fn light_intensity(light: &EvaluatedLight) -> f32 {
        *light.get::<LightIntensity>()
    }
}

impl Bucket for FillExtrusionBucket {
    fn base(&self) -> &BucketBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut BucketBase {
        &mut self.base
    }

    fn add_feature(
        &mut self,
        feature: &dyn GeometryTileFeature,
        geometry: &GeometryCollection,
        pattern_positions: &ImagePositions,
        pattern_dependencies: &PatternLayerMap,
        index: usize,
        canonical: &CanonicalTileID,
    ) -> Result<(), Error> {
        let vertex_offset = self.vertices.len();

        for mut polygon in classify_rings(geometry) {
            // Optimize polygons with many interior rings for earcut
            // tessellation.
            limit_holes(&mut polygon, 500);

            let rounded_corner_distance = f64::from(*self.layout.get::<FillExtrusionRoundedCornerDistance>());
            if rounded_corner_distance > 0.0 {
                self.add_polygon(&round_polygon_corners(&polygon, rounded_corner_distance), None)?;
                continue;
            }

            // On the globe the walls follow subdivided rings and the roof is a
            // subdivided mesh of its own, spread over as many segments as it
            // needs. Rings with more vertices than a polygon may have are
            // extruded as on Mercator. Rounded corners are not subdivided.
            let granularity = self.subdivision_granularity.fill.get_granularity_for_zoom_level(canonical.z);
            if granularity >= 2 {
                let subdivided_rings: Vec<GeometryCoordinates> =
                    polygon.iter().map(|ring| subdivide_vertex_line(ring, granularity, true)).collect();
                let ring_vertices: usize = subdivided_rings.iter().map(Vec::len).sum();
                if ring_vertices <= MAX_SEGMENT_VERTICES {
                    let roof = subdivide_polygon(&polygon, canonical, granularity, false);
                    self.add_polygon(&subdivided_rings, Some(&roof))?;
                    continue;
                }
            }
            self.add_polygon(&polygon[..], None)?;
        }

        let length = self.vertices.len();
        for (id, binders) in &mut self.paint_property_binders {
            binders.get_mut().populate_vertex_vectors(
                feature,
                length,
                index,
                pattern_positions,
                pattern_dependencies.get(id),
                canonical,
                &EMPTY_FORMATTED_SECTION,
            );
        }

        if self.base.retain_features_by_id {
            let vertex_count = self.vertices.len() - vertex_offset;
            if vertex_count > 0 {
                self.retain_feature(feature, vertex_offset, vertex_count);
            }
        }
        Ok(())
    }

    fn upload(&self, _upload_pass: &dyn UploadPass) {
        self.base.uploaded.store(true, Ordering::SeqCst);
    }

    fn has_data(&self) -> bool {
        !self.triangle_segments.is_empty()
    }

    fn get_query_radius(&self, layer: &dyn RenderLayer) -> f32 {
        let evaluated = get_evaluated::<FillExtrusionLayerProperties>(&layer.base().evaluated_properties);
        let translate = evaluated.get::<FillExtrusionTranslate>();
        (translate[0] * translate[0] + translate[1] * translate[1]).sqrt()
    }

    fn update(
        &self,
        states: &crate::bucket::FeatureStates,
        layer: &dyn mln_tile_id::geometry_tile_data::GeometryTileLayer,
        layer_id: &str,
        image_positions: &ImagePositions,
    ) {
        if let Some(binders) = self.paint_property_binders.get(layer_id) {
            binders.write().update_vertex_vectors(states, layer, image_positions);
            self.base.uploaded.store(false, Ordering::SeqCst);

            // sharedVertices->updateModified()
            self.base.version.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// `PatternLayout<FillExtrusionBucket, FillExtrusionLayerProperties,
/// FillExtrusionPattern, FillExtrusionLayoutProperties>` (no sort key).
pub struct FillExtrusionLayoutSpec;

impl PatternLayoutSpec for FillExtrusionLayoutSpec {
    type Bucket = FillExtrusionBucket;
    type Layout = FillExtrusionLayoutProperties::PossiblyEvaluated;

    fn evaluate_layout(
        properties: &Arc<dyn LayerProperties>,
        parameters: &PropertyEvaluationParameters,
    ) -> Self::Layout {
        let properties =
            (**properties).downcast_ref::<FillExtrusionLayerProperties>().expect("FillExtrusionLayerProperties");
        properties.layer_impl().layout.evaluate(parameters)
    }

    fn pattern_property(properties: &Arc<dyn LayerProperties>) -> &PossiblyEvaluatedFadedPropertyValue<Image> {
        get_evaluated::<FillExtrusionLayerProperties>(properties).get::<FillExtrusionPattern>()
    }

    fn pattern_default() -> Image {
        FillExtrusionPattern::default_value()
    }

    fn sort_key(
        _layout: &Self::Layout,
        _feature: &dyn GeometryTileFeature,
        _zoom: f32,
        _canonical: &CanonicalTileID,
    ) -> f32 {
        // SortKeyPropertyType = void
        0.0
    }

    fn constant_sort_key(_layout: &Self::Layout) -> Option<f32> {
        None
    }

    fn create_bucket(
        layout: &Self::Layout,
        layer_properties_map: &VecMap<String, Arc<dyn LayerProperties>>,
        zoom: f32,
        overscaling: u32,
    ) -> FillExtrusionBucket {
        FillExtrusionBucket::new(layout.clone(), layer_properties_map, zoom, overscaling)
    }
    fn set_subdivision_granularity(bucket: &mut FillExtrusionBucket, value: &SubdivisionGranularitySetting) {
        bucket.set_subdivision_granularity(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_vertex_packs_like_cpp() {
        let v = FillExtrusionBucket::layout_vertex(Point::new(10.0, 20.0), 7, true);
        assert_eq!(v.a1, [10, 20]);
        assert_eq!(v.a2, [1, 7]);

        let v = FillExtrusionBucket::layout_vertex(Point::new(-1.5, 2.25), 9, false);
        assert_eq!(v.a1, [-2, 2]);
        assert_eq!(v.a2, [(64 * 256 + 32) * 2, 9]);
    }
}
