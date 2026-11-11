// Ports src/mln/renderer/buckets/fill_extrusion_bucket.hpp and
// fill_extrusion_bucket.cpp (`!MLN_USE_FILL_EXTRUSION_INSTANCING`, as for the
// WebGPU backend), plus the `PatternLayout` instantiation of
// FillExtrusionLayerFactory::createLayout (layermanager/fill_extrusion_layer_factory.cpp).
//
// The vertex/index vectors are plain `Vec`s (the bucket is built on the
// worker thread and shared with `Arc`); the render layer copies them into
// the shared gfx vectors its drawables reference.

use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use geometry::Point;
use mln_gfx::UploadPass;
use mln_gfx::shaders::segment::{MAX_SEGMENT_VERTICES, SegmentBase};
use mln_map_types::TransformState;
use mln_style::PropertyEvaluationParameters;
use mln_style::expression::{Image, Value};
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::fill_extrusion_layer_properties::*;
use mln_style::light::{LightAnchor, LightColor, LightIntensity, LightPosition};
use mln_style::properties::Property;
use mln_style::renderer::possibly_evaluated_property_value::PossiblyEvaluatedFadedPropertyValue;
use mln_style_types::LightAnchorType;
use mln_style_types::image::ImagePositions;
use mln_tile::geometry_tile_data::{classify_rings, limit_holes, round_polygon_corners};
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinates, GeometryTileFeature};
use mln_util::Color;
use mln_util::exception::ExceptionPtr;
use mln_util::immutable::Immutable;
use mln_util::mat3::{Mat3, matrix as mat3};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::sync::RwLockExt;
use mln_util::vectors::Vec3f;
use mln_vec_map::VecMap;
use serde::{Deserialize, Serialize};

use crate::bucket::{Bucket, BucketBase};
use crate::bucket_geometry::GeometryVec;
use crate::gfx::fill_large_mesh_arrays::fill_large_mesh_arrays;
use crate::layout::pattern_layout::{PatternLayerMap, PatternLayoutSpec};
use crate::paint_property_binder::{CrossFadedPaintPropertyBinder, PaintPropertyBinder};
use crate::render_layer::RenderLayer;
use crate::util::subdivision::{SubdivisionResult, subdivide_polygon, subdivide_vertex_line};

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
/// attributes::decimals_ed, attributes::normal2d>>`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct FillExtrusionLayoutVertex {
    pub a1: [i16; 2],
    pub a2: [u16; 2],
    pub a3: [i16; 2],
}

/// `EvaluatedLight = style::LightProperties::PossiblyEvaluated`.
pub use crate::render_light::EvaluatedLight;

/// The `GeometryTooLongException` of fill_extrusion_bucket.cpp (its own
/// struct there, also without `what()`).
#[derive(Debug)]
struct GeometryTooLongException;

impl std::fmt::Display for GeometryTooLongException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("std::exception")
    }
}

impl std::error::Error for GeometryTooLongException {}

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

// Each ring vertex can add a wall quad beside itself.
fn wall_vertex_count(ring_vertices: usize) -> usize {
    if ring_vertices == 0 { 0 } else { 5 * (ring_vertices - 1) + 1 }
}

/// `util::dist<uint16_t>(a, b)`: `static_cast<uint16_t>` of the double
/// distance (x86-64: through a 32-bit conversion, keeping the low bits).
fn dist_u16(a: Point<f64>, b: Point<f64>) -> u16 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    ((dx * dx + dy * dy).sqrt() as i64) as u16
}

/// `util::unit(util::perp(v))` for a `Point<double>`.
fn unit_perp(v: Point<f64>) -> Point<f64> {
    let p = Point::new(-v.y, v.x);
    let magnitude = (p.x * p.x + p.y * p.y).sqrt();
    if magnitude == 0.0 {
        return p;
    }
    Point::new(p.x * (1.0 / magnitude), p.y * (1.0 / magnitude))
}

impl FillExtrusionBucket {
    /// `FillExtrusionBucket(layout, layerPaintProperties, zoom, overscaling)`.
    pub fn new(
        layout: FillExtrusionLayoutProperties::PossiblyEvaluated,
        layer_paint_properties: &VecMap<String, Immutable<dyn LayerProperties>>,
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

    /// `layoutVertex(p, nx, ny, t, edgeDistance)` (non-instanced).
    pub fn layout_vertex(p: Point<f64>, nx: f64, ny: f64, t: u16, edge_distance: u16) -> FillExtrusionLayoutVertex {
        let factor = 2f64.powi(14);
        let int_part = Point::new(p.x.floor(), p.y.floor());
        // Multiply factional part by 2^7 to pack them into integers [0..127]
        let frac_x = ((p.x - int_part.x) * 128.0) as u8;
        let frac_y = ((p.y - int_part.y) * 128.0) as u8;

        FillExtrusionLayoutVertex {
            a1: [int_part.x as i16, int_part.y as i16],
            a2: [
                // We pack a bool (`t`) indicating whether it is an upper or
                // lower vertex
                ((i32::from(frac_x) * 256 + i32::from(frac_y)) * 2 + i32::from(t)) as u16,
                // The edgedistance attribute is used for wrapping
                // fill_extrusion patterns
                edge_distance,
            ],
            // Multiply normal vector components in the 2D plane by 2^14 to
            // pack them into integers
            a3: [(nx * factor) as i16, (ny * factor) as i16],
        }
    }

    /// Adds the walls and the roof of one polygon (its outer ring, then its
    /// holes), with integer or, after corner rounding, float coordinates.
    /// With a subdivided `roof` (the globe's), the rings make only the walls
    /// and the roof is that mesh.
    fn add_polygon<T: Copy + Into<f64>, R: AsRef<[Point<T>]>>(
        &mut self,
        polygon: &[R],
        roof: Option<&SubdivisionResult>,
    ) -> Result<(), ExceptionPtr> {
        let mut total_vertices: usize = 0;
        for ring in polygon {
            total_vertices += ring.as_ref().len();
            if total_vertices > usize::from(u16::MAX) {
                return Err(Arc::new(GeometryTooLongException));
            }
        }

        if total_vertices == 0 {
            return Ok(());
        }

        let mut flat_indices: Vec<u32> = Vec::with_capacity(total_vertices);

        let wall_vertices = wall_vertex_count(total_vertices);

        let start_vertices = self.vertices.len();

        if self.triangle_segments.is_empty()
            || self.triangle_segments.last().expect("segment").vertex_length + wall_vertices > usize::from(u16::MAX)
        {
            self.triangle_segments.push(SegmentBase::new(start_vertices, self.triangles.len()));
        }

        let segment_index = self.triangle_segments.len() - 1;
        debug_assert!(self.triangle_segments[segment_index].vertex_length <= usize::from(u16::MAX));
        let mut triangle_index = self.triangle_segments[segment_index].vertex_length as u16;

        debug_assert!(usize::from(triangle_index) + wall_vertices <= usize::from(u16::MAX));

        for ring in polygon {
            let ring = ring.as_ref();
            let n_vertices = ring.len();

            if n_vertices == 0 {
                continue;
            }

            let mut edge_distance: usize = 0;

            for i in 0..n_vertices {
                let p1 = Point::new(ring[i].x.into(), ring[i].y.into());
                let p2 = if i < n_vertices - 1 {
                    Some(Point::new(ring[i + 1].x.into(), ring[i + 1].y.into()))
                } else {
                    None
                };

                // processRingPoints(p1, p2, edgeDistance)
                self.vertices.get_mut().push(Self::layout_vertex(p1, 0.0, 0.0, 1, edge_distance as u16));
                flat_indices.push(u32::from(triangle_index));
                triangle_index = triangle_index.wrapping_add(1);

                if let Some(p2) = p2 {
                    let perp = unit_perp(Point::new(p2.x - p1.x, p2.y - p1.y));

                    let dist = usize::from(dist_u16(p1, p2));
                    if edge_distance + dist > usize::from(u16::MAX) {
                        edge_distance = 0;
                    }

                    self.vertices.get_mut().push(Self::layout_vertex(p2, perp.x, perp.y, 0, edge_distance as u16));
                    self.vertices.get_mut().push(Self::layout_vertex(p2, perp.x, perp.y, 1, edge_distance as u16));

                    edge_distance += dist;

                    self.vertices.get_mut().push(Self::layout_vertex(p1, perp.x, perp.y, 0, edge_distance as u16));
                    self.vertices.get_mut().push(Self::layout_vertex(p1, perp.x, perp.y, 1, edge_distance as u16));

                    // ┌──────┐
                    // │ 0  1 │ Counter-Clockwise winding order.
                    // │      │ Triangle 1: 0 => 2 => 1
                    // │ 2  3 │ Triangle 2: 1 => 2 => 3
                    // └──────┘
                    let t = triangle_index;
                    self.triangles.get_mut().extend_from_slice(&[t, t.wrapping_add(2), t.wrapping_add(1)]);
                    self.triangles.get_mut().extend_from_slice(&[
                        t.wrapping_add(1),
                        t.wrapping_add(2),
                        t.wrapping_add(3),
                    ]);
                    triangle_index = triangle_index.wrapping_add(4);
                    let segment = &mut self.triangle_segments[segment_index];
                    segment.vertex_length += 4;
                    segment.index_length += 6;
                }
            }
        }

        let indices: Vec<u32> = if roof.is_none() { earcut::earcut::<u32, _, _>(polygon) } else { Vec::new() };

        let n_indices = indices.len();
        debug_assert!(n_indices % 3 == 0);

        for i in (0..n_indices).step_by(3) {
            // Counter-Clockwise winding order.
            self.triangles.get_mut().extend_from_slice(&[
                flat_indices[indices[i] as usize] as u16,
                flat_indices[indices[i + 2] as usize] as u16,
                flat_indices[indices[i + 1] as usize] as u16,
            ]);
        }

        let segment = &mut self.triangle_segments[segment_index];
        segment.vertex_length += total_vertices;
        segment.index_length += n_indices;

        // The roof's triangles index only its own vertices, so it
        // continues the walls' segment or starts its own.
        if let Some(roof) = roof {
            fill_large_mesh_arrays(
                self.vertices.get_mut(),
                |x, y| Self::layout_vertex(Point::new(f64::from(x), f64::from(y)), 0.0, 0.0, 1, 0),
                &mut self.triangle_segments,
                self.triangles.get_mut(),
                &roof.vertices,
                &roof.triangle_indices,
                None,
                MAX_SEGMENT_VERTICES,
            );
        }
        // (instanceSegments, and the roof's vertices counted into them:
        // MLN_USE_FILL_EXTRUSION_INSTANCING only.)
        Ok(())
    }

    /// `lightColor(const EvaluatedLight&)`.
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
    ) -> Result<(), ExceptionPtr> {
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
            // needs. Walls too long for a segment are drawn as on Mercator.
            // Rounded corners are not subdivided.
            let granularity = self.subdivision_granularity.fill.get_granularity_for_zoom_level(canonical.z);
            if granularity >= 2 {
                let subdivided_rings: Vec<GeometryCoordinates> =
                    polygon.iter().map(|ring| subdivide_vertex_line(ring, granularity, true)).collect();
                let ring_vertices = subdivided_rings.iter().map(|ring| ring.len()).sum();
                if wall_vertex_count(ring_vertices) <= MAX_SEGMENT_VERTICES {
                    let roof = subdivide_polygon(&polygon, canonical, granularity, false);
                    self.add_polygon(&subdivided_rings, Some(&roof))?;
                    continue;
                }
            }
            self.add_polygon(&polygon[..], None)?;
        }

        let length = self.vertices.len();
        for (id, binders) in self.paint_property_binders.iter_mut() {
            binders.get_mut_unpoisoned().populate_vertex_vectors(
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
            binders.write_unpoisoned().update_vertex_vectors(states, layer, image_positions);
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
        properties: &Immutable<dyn LayerProperties>,
        parameters: &PropertyEvaluationParameters,
    ) -> Self::Layout {
        let properties = (**properties)
            .as_any()
            .downcast_ref::<FillExtrusionLayerProperties>()
            .expect("FillExtrusionLayerProperties");
        properties.layer_impl().layout.evaluate(parameters)
    }

    fn pattern_property(properties: &Immutable<dyn LayerProperties>) -> &PossiblyEvaluatedFadedPropertyValue<Image> {
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
        layer_properties_map: &VecMap<String, Immutable<dyn LayerProperties>>,
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
        let v = FillExtrusionBucket::layout_vertex(Point::new(10.0, 20.0), 0.6, -0.8, 1, 7);
        assert_eq!(v.a1, [10, 20]);
        assert_eq!(v.a2, [1, 7]);
        assert_eq!(v.a3, [9830, -13107]);
    }
}
