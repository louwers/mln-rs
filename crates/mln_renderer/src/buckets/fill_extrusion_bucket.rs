// The vertices and indices are `GeometryVec`s, which a worker fills and the
// bucket's upload moves into the context's stores, which its draws read.

use std::f64::consts::PI;
use std::sync::Arc;

use geometry::Point;
use mln_gfx::shaders::segment::{MAX_SEGMENT_VERTICES, SegmentBase};
use mln_map_types::View;
use mln_style::PropertyEvaluationParameters;
use mln_style::evaluated::Evaluated;
use mln_style::expression::{Image, Value};
use mln_style::layers::FillExtrusion;
use mln_style::layers::fill_extrusion_layer_properties::{FillExtrusionLayoutProperties, FillExtrusionPaintProperties};
use mln_style_types::LightAnchorType;
use mln_style_types::image::ImagePositions;
use mln_tile::geometry_tile_data::{
    classify_rings, is_boundary_edge, is_entirely_outside, limit_holes, round_polygon_corners,
};
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinates, GeometryTileFeature};
use mln_util::Color;
use mln_util::constants::{EARTH_RADIUS_M, EXTENT, M2PI};
use mln_util::error::Error;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecMap;

use crate::bucket::{Bucket, BucketBase, UploadNeeds};
use crate::bucket_geometry::{GeometryUpload, GeometryVec};
use crate::gfx::fill_large_mesh_arrays::fill_large_mesh_arrays;
use crate::layer_type::EvaluatedLayer;
use crate::layout::pattern_layout::{PatternLayerMap, PatternLayoutSpec};
use crate::paint_property_binder::{CrossFadedPaintPropertyBinder, PaintPropertyBinder};
use crate::render_static_data::RenderStaticData;
use crate::util::subdivision::{SubdivisionResult, subdivide_polygon, subdivide_vertex_line};
use crate::util::triangulate::triangulate;

crate::paint_property_binders!(
    pub struct FillExtrusionBinders for FillExtrusionPaintProperties::PossiblyEvaluated {
        fill_extrusion_base => PaintPropertyBinder<f32>,
        fill_extrusion_color => PaintPropertyBinder<Color>,
        fill_extrusion_height => PaintPropertyBinder<f32>,
        fill_extrusion_pattern => CrossFadedPaintPropertyBinder<Image>,
    }
);

/// A vertex of the roof, and the wall instance from it to the next vertex of
/// its ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct FillExtrusionLayoutVertex {
    pub(crate) a1: [i16; 2],
    pub(crate) a2: [u16; 2],
}

/// A corner of the wall quad every instance draws: x picks the instance's
/// vertex (0) or the next one (1), y the base (0) or the top (1).
pub(crate) type FillExtrusionStaticVertex = [i16; 2];

/// A corner of a wall drawn without instancing, and the outline vertex the
/// wall starts at. A wall reads the next outline vertex four corners
/// further on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct FillExtrusionWallVertex {
    pub(crate) corner: FillExtrusionStaticVertex,
    pub(crate) vertex: FillExtrusionLayoutVertex,
}

pub(crate) use crate::render_light::EvaluatedLight;

/// The quads of the walls of `vertices`: each vertex with each corner of the
/// static quad, in its order. The quad of the wall from a vertex is its four
/// corners, and the next four are the next vertex's.
pub(crate) fn wall_quad_vertices(vertices: &[FillExtrusionLayoutVertex]) -> Vec<FillExtrusionWallVertex> {
    let corners = RenderStaticData::fill_extrusion_vertices();
    let quads: Vec<FillExtrusionWallVertex> = vertices
        .iter()
        .flat_map(|&vertex| corners.iter().map(move |&corner| FillExtrusionWallVertex { corner, vertex }))
        .collect();
    quads
}

/// The paint values of the quads of `wall_quad_vertices`: those of each of
/// the first `walls` vertices in `paint`, `stride` bytes each, for each
/// corner of its wall.
pub(crate) fn wall_quad_paint(paint: &[u8], stride: usize, walls: usize) -> Vec<u8> {
    let mut values = Vec::with_capacity(4 * stride * walls);
    if stride > 0 {
        for vertex in paint.chunks_exact(stride).take(walls) {
            for _ in 0..4 {
                values.extend_from_slice(vertex);
            }
        }
    }
    values
}

#[derive(Clone, mln_wire::Wire)]
pub(crate) struct FillExtrusionBucket {
    pub(crate) base: BucketBase,
    pub(crate) layout: FillExtrusionLayoutProperties::PossiblyEvaluated,

    pub(crate) vertices: GeometryVec<FillExtrusionLayoutVertex>,
    pub(crate) triangles: GeometryVec<u16>,
    pub(crate) triangle_segments: Vec<SegmentBase>,
    pub(crate) subdivision_granularity: SubdivisionGranularitySetting,

    pub(crate) paint_property_binders: VecMap<String, FillExtrusionBinders>,
}

static EMPTY_FORMATTED_SECTION: Value = Value::Null;

/// The tile units in a meter at the center of `tile`, the scale that
/// distances in meters on its features take.
pub(crate) fn tile_units_per_meter(tile: &CanonicalTileID) -> f64 {
    let tiles = 2f64.powi(i32::from(tile.z));
    let latitude = (PI * (1.0 - 2.0 * (f64::from(tile.y) + 0.5) / tiles)).sinh().atan();
    f64::from(EXTENT) * tiles / (latitude.cos() * M2PI * EARTH_RADIUS_M)
}

/// The distance from `a` to `b`, cast to `u16` through a wider integer,
/// keeping the low bits.
fn dist_u16(a: Point<f64>, b: Point<f64>) -> u16 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    ((dx * dx + dy * dy).sqrt() as i64) as u16
}

impl FillExtrusionBucket {
    #[must_use]
    pub(crate) fn new(
        layout: FillExtrusionLayoutProperties::PossiblyEvaluated,
        layer_paint_properties: &VecMap<String, Arc<EvaluatedLayer<FillExtrusion>>>,
        zoom: f32,
        _overscaling: u32,
    ) -> Self {
        let mut paint_property_binders = VecMap::new();
        for (id, properties) in layer_paint_properties {
            paint_property_binders
                .entry(id.clone())
                .or_insert_with(|| FillExtrusionBinders::new(&properties.paint, zoom));
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

    pub(crate) fn set_subdivision_granularity(&mut self, value: &SubdivisionGranularitySetting) {
        self.subdivision_granularity = *value;
    }

    #[must_use]
    pub(crate) fn layout_vertex(p: Point<f64>, edge_distance: u16, is_discarded: bool) -> FillExtrusionLayoutVertex {
        let int_part = Point::new(p.x.floor(), p.y.floor());
        // Multiply factional part by 2^7 to pack them into integers [0..127]
        let frac_x = ((p.x - int_part.x) * 128.0) as u8;
        let frac_y = ((p.y - int_part.y) * 128.0) as u8;

        FillExtrusionLayoutVertex {
            a1: [int_part.x as i16, int_part.y as i16],
            a2: [
                // Packs whether this instance is discarded in the low bit.
                ((i32::from(frac_x) * 256 + i32::from(frac_y)) * 2 + i32::from(is_discarded)) as u16,
                // The edge distance, which fill-extrusion patterns wrap
                // along.
                edge_distance,
            ],
        }
    }

    /// Appends the outline of `polygon`: each vertex of a ring is a wall
    /// instance up to the next one, the last vertex ends the ring. Edges on
    /// the lines the tile was clipped at have no walls, and neither do rings
    /// in the tile's buffer, which the neighbouring tile draws.
    fn add_outline<R: AsRef<[Point<i16>]>>(&mut self, polygon: &[R]) {
        let vertices = self.vertices.get_mut();
        for ring in polygon {
            let ring = ring.as_ref();
            let outside = is_entirely_outside(ring);
            let mut edge_distance: usize = 0;
            for (i, &p) in ring.iter().enumerate() {
                let next = ring.get(i + 1).copied();
                let is_discarded = outside || next.is_none_or(|next| is_boundary_edge(p, next));
                let p1 = Point::new(f64::from(p.x), f64::from(p.y));
                let p2 = next.map(|p| Point::new(f64::from(p.x), f64::from(p.y)));

                vertices.push(Self::layout_vertex(p1, edge_distance as u16, is_discarded));

                // Walls that aren't drawn don't move the pattern along.
                if let Some(p2) = p2.filter(|_| !is_discarded) {
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
    /// holes).
    /// The outline's vertices are the roof's, unless a subdivided `roof`
    /// (the globe's) is that mesh, with vertices of its own: the outline
    /// then stays out of the segments, as only its walls draw it.
    fn add_polygon<R: AsRef<[Point<i16>]>>(
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

    #[must_use]
    pub(crate) fn light_color(light: &EvaluatedLight) -> [f32; 3] {
        let color = light.light_color.uniform();
        [color.r, color.g, color.b]
    }

    pub(crate) fn light_position(light: &EvaluatedLight, state: &View) -> [f32; 3] {
        let light_pos = glam::Vec3::from_array(light.light_position.uniform().get_cartesian());
        if *light.light_anchor.uniform() == LightAnchorType::Viewport {
            // A viewport-anchored light turns with the map.
            (glam::Mat3::from_rotation_z(-state.angle() as f32) * light_pos).to_array()
        } else {
            light_pos.to_array()
        }
    }

    #[must_use]
    pub(crate) fn light_intensity(light: &EvaluatedLight) -> f32 {
        *light.light_intensity.uniform()
    }
}

impl Bucket for FillExtrusionBucket {
    fn base(&self) -> &BucketBase {
        &self.base
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
        for mut polygon in classify_rings(geometry) {
            // Optimize polygons with many interior rings for earcut
            // tessellation.
            limit_holes(&mut polygon, 500);

            let rounded_corner_distance = f64::from(*self.layout.fill_extrusion_rounded_corner_distance.uniform());
            if rounded_corner_distance > 0.0 {
                polygon = round_polygon_corners(&polygon, rounded_corner_distance * tile_units_per_meter(canonical));
            }

            // The neighbouring tile draws a polygon that lies in this tile's
            // buffer.
            if polygon.first().is_none_or(|outer| is_entirely_outside(outer)) {
                continue;
            }

            // On the globe the walls follow subdivided rings and the roof is a
            // subdivided mesh of its own, spread over as many segments as it
            // needs. Rings with more vertices than a polygon may have are
            // extruded as on Mercator.
            let granularity = self.subdivision_granularity.fill.granularity_for_zoom_level(canonical.z);
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
            binders.populate_vertex_vectors(
                feature,
                length,
                index,
                pattern_positions,
                pattern_dependencies.get(id),
                canonical,
                &EMPTY_FORMATTED_SECTION,
            );
        }
        Ok(())
    }

    fn upload(&mut self, geometry: &mut GeometryUpload<'_>, needs: UploadNeeds) {
        // Walls that aren't instances of the static quad are quads of their
        // own, made of the vertices and paint values before those move.
        let walls = self.vertices.len().saturating_sub(1);
        if needs.wall_quads {
            geometry.derive_vertices(&self.vertices, wall_quad_vertices);
        }
        geometry.move_vertices(&mut self.vertices);
        geometry.move_indexes(&mut self.triangles);
        for binders in self.paint_property_binders.values_mut() {
            let depends_on_feature_state = binders.depends_on_feature_state();
            let buffer = &mut binders.interleaved_vertex_buffer;
            if needs.wall_quads && !depends_on_feature_state {
                let stride = buffer.stride;
                geometry.derive_vertices(&buffer.shared_vertex_vector, |paint| wall_quad_paint(paint, stride, walls));
            }
            geometry.move_paint(buffer, depends_on_feature_state);
        }
        self.base.uploaded = true;
    }

    fn has_data(&self) -> bool {
        !self.triangle_segments.is_empty()
    }

    fn write_wire(&self, w: &mut mln_wire::Writer) {
        mln_wire::Wire::write(self, w);
    }

    fn update(
        &mut self,
        states: &crate::bucket::FeatureStates,
        layer: &mln_tile::TileLayer<'_>,
        layer_id: &str,
        image_positions: &ImagePositions,
    ) -> bool {
        let Some(binders) = self.paint_property_binders.get_mut(layer_id) else {
            return false;
        };
        binders.update_vertex_vectors(states, layer, image_positions);
        self.base.uploaded = false;
        true
    }
}

/// The pattern layout of fill-extrusion layers, which have no sort key.
pub(crate) struct FillExtrusionLayoutSpec;

impl PatternLayoutSpec for FillExtrusionLayoutSpec {
    type Type = FillExtrusion;
    type Bucket = FillExtrusionBucket;
    type Layout = FillExtrusionLayoutProperties::PossiblyEvaluated;

    fn evaluate_layout(
        layer: &EvaluatedLayer<FillExtrusion>,
        parameters: &PropertyEvaluationParameters,
    ) -> Self::Layout {
        layer.style.layout.evaluate(parameters)
    }

    fn pattern_property(layer: &EvaluatedLayer<FillExtrusion>) -> &Evaluated<Image> {
        &layer.paint.fill_extrusion_pattern
    }

    fn sorted(_leader: &EvaluatedLayer<FillExtrusion>) -> bool {
        // Fill extrusions have no sort key.
        false
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
        layer_properties_map: &VecMap<String, Arc<EvaluatedLayer<FillExtrusion>>>,
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
    fn layout_vertex_packs_the_position_and_wall_data() {
        let v = FillExtrusionBucket::layout_vertex(Point::new(10.0, 20.0), 7, true);
        assert_eq!(v.a1, [10, 20]);
        assert_eq!(v.a2, [1, 7]);

        let v = FillExtrusionBucket::layout_vertex(Point::new(-1.5, 2.25), 9, false);
        assert_eq!(v.a1, [-2, 2]);
        assert_eq!(v.a2, [(64 * 256 + 32) * 2, 9]);
    }
}
