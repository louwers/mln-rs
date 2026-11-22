// The binders of a layer's data-driven paint properties, which write each
// feature's values into the vertices its draws read: `PaintPropertyBinder<T>`
// for a constant, source or composite function, and
// `CrossFadedPaintPropertyBinder<T>` for the cross-faded (pattern) ones.
// `paint_property_binders!` declares a layer type's set of them.
//
// The binders write into the bucket's `InterleavedVertexBuffer`, which is
// passed in, in the vertices' in-memory layout (native-endian floats and
// u16s), so the buffer is the GPU vertex data as it uploads.

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::VertexStep;
use mln_gfx::shaders::block::MemberType;
use mln_gfx::shaders::program::{BufferKind, Program, PropertyType};
use mln_style::evaluated::{Evaluated, Faded};
use mln_style::expression::range::Range;
use mln_style::expression::{Dependency, EvaluationContext, Image, Value, ValueConverter};
use mln_style::layer_type::{PaintSet, StyleLayerType};
use mln_style::property_expression::{PropertyExpression, PropertyExpressionBase};
use mln_style_types::image::ImagePositions;
use mln_tile::TileLayer;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::GeometryTileFeature;
use mln_util::Color;
use mln_util::feature::feature_id_to_string;
use mln_vec_map::VecMap;
use mln_wire::Wire;

use crate::bucket_geometry::GpuVec;
use crate::erased_layer::EvaluatedLayer;
use crate::gfx::buffer_store::SharedBuffer;
use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms};
use crate::handles::Buffer;
use crate::paint_property_statistics::{PaintPropertyStatistics, StatisticValue};

/// The patterns of a feature for each layer of its group whose `*-pattern`
/// value differs by feature, which
/// [`LayoutCx::need_patterns`](crate::layer::LayoutCx::need_patterns) asks
/// for and
/// [`BuildCx::populate_patterns`](crate::layer::BuildCx::populate_patterns)
/// writes where they are.
#[derive(Clone, Debug, Default)]
pub struct FeaturePatterns(pub(crate) Vec<(String, PatternDependency)>);

impl FeaturePatterns {
    /// The ids of the images of the feature's pattern for the layer
    /// `layer`, one zoom level below the tile's, at it and one above, if
    /// its value differs by feature.
    #[must_use]
    pub fn images(&self, layer: &str) -> Option<[&str; 3]> {
        let (_, images) = self.0.iter().find(|(id, _)| id == layer)?;
        Some([&images.min, &images.mid, &images.max])
    }
}

/// The images of a feature's `*-pattern` value one zoom level below the
/// tile's, at it, and one above.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PatternDependency {
    /// The image one zoom level below the tile's.
    pub min: String,
    /// The image at the tile's zoom level.
    pub mid: String,
    /// The image one zoom level above the tile's.
    pub max: String,
}

/// Maps vertex range to feature index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, mln_wire::Wire)]
pub(crate) struct FeatureVertexRange {
    pub(crate) feature_index: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

pub(crate) type FeatureVertexRangeMap = VecMap<String, Vec<FeatureVertexRange>>;

/// The data-driven attribute values of all the binders of one
/// `paint_property_binders!` struct, one interleaved vertex per layout
/// vertex.
#[derive(Clone, Debug, Default, PartialEq, mln_wire::Wire)]
pub struct InterleavedVertexBuffer {
    pub(crate) stride: usize,
    pub(crate) vertex_count: usize,
    pub(crate) shared_vertex_vector: GpuVec<u8>,
}

impl InterleavedVertexBuffer {
    /// Grows the vertices to `length`, for `populate` to write the values
    /// of the new ones, from the current count up.
    #[inline]
    pub fn populate(&mut self, length: usize, populate: impl FnOnce(&mut Self)) {
        let added = (length - self.vertex_count) * self.stride;
        let data = self.shared_vertex_vector.get_mut();
        data.resize(data.len() + added, 0);
        populate(self);
        self.vertex_count += length - self.vertex_count;
        // The render thread copies the vector when it builds draws.
    }

    /// The vertices' size in bytes: their fields' sizes and padding.
    #[must_use]
    #[inline]
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// How many vertices there are.
    #[must_use]
    #[inline]
    pub fn vertex_count(&self) -> usize {
        self.vertex_count
    }

    /// The interleaved vertices, `stride` bytes each.
    #[must_use]
    #[inline]
    pub fn vertices(&self) -> &GpuVec<u8> {
        &self.shared_vertex_vector
    }

    /// [`vertices`](Self::vertices), to upload.
    pub(crate) fn vertices_mut(&mut self) -> &mut GpuVec<u8> {
        &mut self.shared_vertex_vector
    }

    /// Whether the values moved to the render thread.
    #[must_use]
    #[inline]
    pub fn is_moved(&self) -> bool {
        self.shared_vertex_vector.is_moved()
    }

    /// Writes `value` at `offset` in each of the vertices `indexes`.
    #[inline]
    pub fn fill(&mut self, indexes: std::ops::Range<usize>, offset: usize, value: &[u8]) {
        // Taking the vertices copies a worker's buffer into wasm memory.
        if indexes.is_empty() {
            return;
        }
        let stride = self.stride;
        let data = self.shared_vertex_vector.get_mut();
        for index in indexes {
            let start = stride * index + offset;
            debug_assert!(start + value.len() <= data.len());
            data[start..start + value.len()].copy_from_slice(value);
        }
    }

    /// Appends a field of `size` bytes, aligned to `alignment`, to the vertex
    /// format and returns its offset.
    #[inline]
    pub fn extend_vertex_format(&mut self, size: usize, alignment: usize) -> usize {
        let mut offset = self.stride & (alignment - 1);
        if offset != 0 {
            offset = alignment - offset;
        }

        let vertex_offset = self.stride + offset;
        self.stride += size + offset;

        vertex_offset
    }
}

/// The value types of data-driven paint properties with a single attribute,
/// `f32` and `Color`: the attribute's value of a value
/// ([`attribute_value`](Self::attribute_value)), and its type.
pub trait BinderValue: Clone + Default + ValueConverter + StatisticValue + std::fmt::Debug {
    /// The number of floats `attribute_value` uses.
    const DIMENSIONS: usize;
    /// The attribute's type, of one value.
    const BASE_DATA_TYPE: AttributeDataType;
    /// The attribute's type, of the values at two zoom stops that the
    /// shader interpolates between.
    const INTERPOLATED_DATA_TYPE: AttributeDataType;

    /// The attribute's floats; only the first `DIMENSIONS` entries are used.
    fn attribute_value(&self) -> [f32; 2];
}

impl BinderValue for f32 {
    const DIMENSIONS: usize = 1;
    const BASE_DATA_TYPE: AttributeDataType = AttributeDataType::Float;
    const INTERPOLATED_DATA_TYPE: AttributeDataType = AttributeDataType::Float2;

    #[inline]
    fn attribute_value(&self) -> [f32; 2] {
        [*self, 0.0]
    }
}

/// Packs two values into a `u16`: `a as u16 * 256 + b as u16`, computed as an
/// `i32` and truncated.
#[inline]
fn pack_uint8_pair(a: f32, b: f32) -> u16 {
    (i32::from(a as u16) * 256 + i32::from(b as u16)) as u16
}

impl BinderValue for Color {
    const DIMENSIONS: usize = 2;
    const BASE_DATA_TYPE: AttributeDataType = AttributeDataType::Float2;
    const INTERPOLATED_DATA_TYPE: AttributeDataType = AttributeDataType::Float4;

    /// Encode a four-component color value into a pair of floats, with 8
    /// bits for each component: for each float we use the upper 8 bits for
    /// one component (e.g. (color.r * 255) * 256), and the lower 8 for
    /// another.
    ///
    /// Also note that colors come in as floats 0..1, so we scale by 255.
    #[inline]
    fn attribute_value(&self) -> [f32; 2] {
        [
            f32::from(pack_uint8_pair(255.0 * self.r, 255.0 * self.g)),
            f32::from(pack_uint8_pair(255.0 * self.b, 255.0 * self.a)),
        ]
    }
}

/// The attribute values of a property that the shader interpolates between
/// `min` and `max`, its values at the zoom stops: `min`'s, then `max`'s, in
/// the first `2 * T::DIMENSIONS` floats.
fn zoom_stop_values<T: BinderValue>(min: &T, max: &T) -> [f32; 4] {
    let mut values = [0.0; 4];
    values[..T::DIMENSIONS].copy_from_slice(&min.attribute_value()[..T::DIMENSIONS]);
    values[T::DIMENSIONS..2 * T::DIMENSIONS].copy_from_slice(&max.attribute_value()[..T::DIMENSIONS]);
    values
}

/// The data of a `PaintPropertyBinder` subclass.
#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) enum PaintPropertyBinderKind<T> {
    Constant {
        constant: T,
    },
    SourceFunction {
        expression: PropertyExpression<T>,
        default_value: T,
        feature_map: FeatureVertexRangeMap,
        vertex_data: InterleavedVertexData,
    },
    CompositeFunction {
        expression: PropertyExpression<T>,
        default_value: T,
        zoom_range: Range<f32>,
        feature_map: FeatureVertexRangeMap,
        vertex_data: InterleavedVertexData,
    },
}

#[derive(Clone, Debug, mln_wire::Wire, PartialEq, Eq)]
pub(crate) enum InterleavedVertexData {
    Uninitialized,
    Interleaved { offset: usize },
}

impl InterleavedVertexData {
    #[inline]
    fn offset(&self) -> Option<usize> {
        match self {
            Self::Uninitialized => None,
            Self::Interleaved { offset } => Some(*offset),
        }
    }
}

impl<T: BinderValue> PaintPropertyBinderKind<T> {
    fn vertex_offset(&self) -> Option<usize> {
        match self {
            Self::Constant { .. } => None,
            Self::SourceFunction { vertex_data, .. } | Self::CompositeFunction { vertex_data, .. } => {
                vertex_data.offset()
            }
        }
    }

    fn update_vertex_vector(
        &self,
        statistics: &mut PaintPropertyStatistics<T>,
        buffer: &mut InterleavedVertexBuffer,
        vertex_offset: usize,
        range: &FeatureVertexRange,
        feature: &dyn GeometryTileFeature,
        state: &mln_style::expression::expr::FeatureState,
    ) {
        match self {
            Self::Constant { .. } => {}
            Self::SourceFunction { expression, default_value, .. } => {
                let evaluated = expression.evaluate(
                    &EvaluationContext::from_feature(Some(feature)).with_feature_state(Some(state)),
                    default_value.clone(),
                );
                statistics.add(&evaluated);
                let value = evaluated.attribute_value();
                buffer.fill(range.start..range.end, vertex_offset, bytemuck::cast_slice(&value[..T::DIMENSIONS]));
            }
            Self::CompositeFunction { expression, default_value, zoom_range, .. } => {
                let min = expression.evaluate(
                    &EvaluationContext::with_zoom_feature_state(zoom_range.min, Some(feature), Some(state)),
                    default_value.clone(),
                );
                let max = expression.evaluate(
                    &EvaluationContext::with_zoom_feature_state(zoom_range.max, Some(feature), Some(state)),
                    default_value.clone(),
                );
                statistics.add(&min);
                statistics.add(&max);
                let value = zoom_stop_values(&min, &max);
                buffer.fill(range.start..range.end, vertex_offset, bytemuck::cast_slice(&value[..2 * T::DIMENSIONS]));
            }
        }
    }
}

/// The binder of a `f32` or `Color` property: a constant, or each
/// feature's value, or its values at the zoom stops around the tile's zoom,
/// which the shader interpolates.
#[derive(Clone, Debug, mln_wire::Wire)]
pub struct PaintPropertyBinder<T> {
    pub(crate) kind: PaintPropertyBinderKind<T>,
    pub(crate) statistics: PaintPropertyStatistics<T>,
}

impl<T: BinderValue> PaintPropertyBinder<T> {
    /// The largest value the binder wrote of an `f32` property; `None` for
    /// one of another type, or that wrote none.
    #[must_use]
    pub fn max(&self) -> Option<f32> {
        self.statistics.max()
    }

    /// Where the binder's values are in each vertex of its set's paint
    /// values, in bytes ([`PaintVertices::stride`]), for a program that
    /// reads them itself; `None` for a value the same for every feature.
    #[must_use]
    pub fn offset(&self) -> Option<usize> {
        self.kind.vertex_offset()
    }

    /// The binder of `value` in a tile at `zoom`.
    #[must_use]
    pub fn create(value: &Evaluated<T>, zoom: f32) -> Self {
        let kind = match value {
            Evaluated::Constant(constant) | Evaluated::CrossFaded(Faded { to: constant, .. }) => {
                PaintPropertyBinderKind::Constant { constant: constant.clone() }
            }
            Evaluated::PerFeature { expression, default } => {
                let default_value = default.clone();
                if expression.is_zoom_constant() {
                    PaintPropertyBinderKind::SourceFunction {
                        expression: expression.clone(),
                        default_value,
                        feature_map: FeatureVertexRangeMap::new(),
                        vertex_data: InterleavedVertexData::Uninitialized,
                    }
                } else {
                    PaintPropertyBinderKind::CompositeFunction {
                        expression: expression.clone(),
                        default_value,
                        zoom_range: Range::new(zoom, zoom + 1.0),
                        feature_map: FeatureVertexRangeMap::new(),
                        vertex_data: InterleavedVertexData::Uninitialized,
                    }
                }
            }
        };
        PaintPropertyBinder { kind, statistics: PaintPropertyStatistics::default() }
    }

    /// The size in bytes of the zoom-interpolated vertex: `2 * DIMENSIONS`
    /// floats.
    fn zoom_interpolated_size() -> usize {
        2 * T::DIMENSIONS * 4
    }

    /// Adds the binder's zoom-interpolated vertex to the vertex format. The
    /// constant binder has no vertex data and does nothing.
    pub(crate) fn set_interleaved_buffer(&mut self, buffer: &mut InterleavedVertexBuffer) {
        let vertex_data = match &mut self.kind {
            PaintPropertyBinderKind::Constant { .. } => return,
            PaintPropertyBinderKind::SourceFunction { vertex_data, .. }
            | PaintPropertyBinderKind::CompositeFunction { vertex_data, .. } => vertex_data,
        };
        let new_vertex_offset = buffer.extend_vertex_format(Self::zoom_interpolated_size(), align_of::<f32>());
        if matches!(vertex_data, InterleavedVertexData::Uninitialized) {
            *vertex_data = InterleavedVertexData::Interleaved { offset: new_vertex_offset };
        }
    }

    pub(crate) fn vertex_count(&self, buffer: &InterleavedVertexBuffer) -> usize {
        if self.kind.vertex_offset().is_some() { buffer.vertex_count } else { 0 }
    }

    pub(crate) fn is_interpolated(&self) -> bool {
        matches!(self.kind, PaintPropertyBinderKind::CompositeFunction { .. })
    }

    pub(crate) fn populate_vertex_vector(
        &mut self,
        buffer: &mut InterleavedVertexBuffer,
        feature: &dyn GeometryTileFeature,
        length: usize,
        index: usize,
        canonical: &CanonicalTileID,
        formatted_section: &Value,
    ) {
        let Some(vertex_offset) = self.kind.vertex_offset() else { return };
        let elements = self.vertex_count(buffer);
        match &mut self.kind {
            PaintPropertyBinderKind::Constant { .. } => {}
            PaintPropertyBinderKind::SourceFunction { expression, default_value, feature_map, .. } => {
                let evaluated = expression.evaluate(
                    &EvaluationContext::from_feature(Some(feature))
                        .with_formatted_section(Some(formatted_section))
                        .with_canonical_tile_id(Some(canonical)),
                    default_value.clone(),
                );
                self.statistics.add(&evaluated);
                let value = evaluated.attribute_value();
                // The base attribute only.
                buffer.fill(elements..length, vertex_offset, bytemuck::cast_slice(&value[..T::DIMENSIONS]));
                if let Some(id_str) = feature_id_to_string(&feature.get_id()) {
                    feature_map.entry(id_str).or_default().push(FeatureVertexRange {
                        feature_index: index,
                        start: elements,
                        end: length,
                    });
                }
            }
            PaintPropertyBinderKind::CompositeFunction {
                expression, default_value, zoom_range, feature_map, ..
            } => {
                let min = expression.evaluate(
                    &EvaluationContext::with_zoom_feature(zoom_range.min, Some(feature))
                        .with_formatted_section(Some(formatted_section))
                        .with_canonical_tile_id(Some(canonical)),
                    default_value.clone(),
                );
                let max = expression.evaluate(
                    &EvaluationContext::with_zoom_feature(zoom_range.max, Some(feature))
                        .with_formatted_section(Some(formatted_section))
                        .with_canonical_tile_id(Some(canonical)),
                    default_value.clone(),
                );
                self.statistics.add(&min);
                self.statistics.add(&max);
                let value = zoom_stop_values(&min, &max);
                buffer.fill(elements..length, vertex_offset, bytemuck::cast_slice(&value[..2 * T::DIMENSIONS]));
                if let Some(id_str) = feature_id_to_string(&feature.get_id()) {
                    feature_map.entry(id_str).or_default().push(FeatureVertexRange {
                        feature_index: index,
                        start: elements,
                        end: length,
                    });
                }
            }
        }
    }

    pub(crate) fn update_vertex_vectors(
        &mut self,
        buffer: &mut InterleavedVertexBuffer,
        states: &crate::bucket::FeatureStates,
        layer: &TileLayer<'_>,
    ) {
        let Self { kind, statistics } = self;
        let (feature_map, vertex_data) = match &*kind {
            PaintPropertyBinderKind::Constant { .. } => return,
            PaintPropertyBinderKind::SourceFunction { feature_map, vertex_data, .. }
            | PaintPropertyBinderKind::CompositeFunction { feature_map, vertex_data, .. } => (feature_map, vertex_data),
        };
        let Some(vertex_offset) = vertex_data.offset() else {
            return;
        };
        for (id, state) in states {
            let Some(positions) = feature_map.get(id) else {
                continue;
            };
            for pos in positions {
                // The worker read the feature from the same data, so it
                // can be read again.
                let Ok(feature) = layer.feature(pos.feature_index) else {
                    continue;
                };
                kind.update_vertex_vector(statistics, buffer, vertex_offset, pos, &feature, state);
            }
        }
    }

    /// How the factor that mixes a vertex's values at its two zoom stops
    /// follows the camera's zoom.
    pub(crate) fn mix(&self) -> Mix {
        match &self.kind {
            PaintPropertyBinderKind::Constant { .. } | PaintPropertyBinderKind::SourceFunction { .. } => Mix::None,
            PaintPropertyBinderKind::CompositeFunction { expression, zoom_range, .. } => {
                Mix::Curve { expression: PropertyExpressionBase::clone(expression), zoom_range: *zoom_range }
            }
        }
    }

    /// The factor that mixes a vertex's values at its two zoom stops at
    /// `current_zoom`; 0 unless the binder is a composite function.
    #[must_use]
    pub fn interpolation_factor(&self, current_zoom: f32) -> f32 {
        self.mix().factor(current_zoom)
    }

    /// Where the property's attribute reads its values in `shared`, the
    /// vertices of `buffer`; `None` without values.
    pub(crate) fn apply_paint_property(
        &self,
        buffer: &InterleavedVertexBuffer,
        shared: &SharedBuffer,
    ) -> Option<SharedAttribute> {
        if let Some(vertex_offset) = self.kind.vertex_offset() {
            let raw_size = buffer.stride as u32;
            let data_type = if self.is_interpolated() { T::INTERPOLATED_DATA_TYPE } else { T::BASE_DATA_TYPE };

            debug_assert_eq!(shared.count() / buffer.stride.max(1), self.vertex_count(buffer));
            Some(shared_paint(shared, vertex_offset, raw_size, data_type))
        } else {
            // Without an interleaved buffer there are no vertices to read.
            debug_assert_eq!(self.vertex_count(buffer), 0);
            None
        }
    }
}

/// How the factor that mixes a property's values at the two zoom stops
/// around a tile's zoom follows the camera's zoom.
#[derive(Clone, Debug)]
pub enum Mix {
    /// The values don't depend on the zoom: the factor is 0.
    None,
    /// The values are of `expression`'s zoom curve at the ends of
    /// `zoom_range`.
    Curve { expression: PropertyExpressionBase, zoom_range: Range<f32> },
}

impl Mix {
    /// The factor at the camera's `zoom`, from 0 to 1.
    pub(crate) fn factor(&self, zoom: f32) -> f32 {
        match self {
            Mix::None => 0.0,
            Mix::Curve { expression, zoom_range } => {
                let zoom = if expression.get_use_integer_zoom() { zoom.floor() } else { zoom };
                mln_math::fmaxf(0.0, mln_math::fminf(1.0, expression.interpolation_factor(*zoom_range, zoom)))
            }
        }
    }
}

/// The data of a cross-faded `PaintPropertyBinder` subclass.
#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) enum CrossFadedPaintPropertyBinderKind<T> {
    Constant {
        constant: Faded<T>,
    },
    Composite {
        expression: PropertyExpression<T>,
        default_value: T,
        zoom_range: Range<f32>,
        vertex_data: InterleavedVertexData,
        /// The number of vertices `populate_vertex_vector` has written. It
        /// lags the buffer's count when a feature's patterns are missing,
        /// so the next feature's pattern also covers those vertices.
        #[wire(skip)]
        populated_count: usize,
    },
}

impl<T> CrossFadedPaintPropertyBinderKind<T> {
    fn vertex_offset(&self) -> Option<usize> {
        match self {
            Self::Constant { .. } => None,
            Self::Composite { vertex_data, .. } => vertex_data.offset(),
        }
    }
}

/// The binder of a `*-pattern` property, a cross-faded image, which writes
/// where each feature's pattern is in its tile's atlas
/// ([`LayoutCx::need_patterns`](crate::layer::LayoutCx::need_patterns)):
/// four `u16`s, the values of both of its program's properties.
#[derive(Clone, Debug, mln_wire::Wire)]
pub struct CrossFadedPaintPropertyBinder<T> {
    pub(crate) kind: CrossFadedPaintPropertyBinderKind<T>,
}

/// The size of a zoom-interpolated pattern vertex: eight `u16`s.
const PATTERN_ZOOM_INTERPOLATED_SIZE: usize = 16;

impl CrossFadedPaintPropertyBinder<Image> {
    /// The binder of `value` in a tile at `zoom`.
    #[must_use]
    pub fn create(value: &Evaluated<Image>, zoom: f32) -> Self {
        let kind = match value {
            Evaluated::Constant(constant) => CrossFadedPaintPropertyBinderKind::Constant {
                constant: Faded { from: constant.clone(), to: constant.clone() },
            },
            Evaluated::CrossFaded(constant) => {
                CrossFadedPaintPropertyBinderKind::Constant { constant: constant.clone() }
            }
            Evaluated::PerFeature { expression, default } => CrossFadedPaintPropertyBinderKind::Composite {
                expression: expression.clone(),
                default_value: default.clone(),
                zoom_range: Range::new(zoom, zoom + 1.0),
                vertex_data: InterleavedVertexData::Uninitialized,
                populated_count: 0,
            },
        };
        CrossFadedPaintPropertyBinder { kind }
    }

    /// Adds the binder's vertex data to the vertex format; only the composite
    /// binder has any.
    pub(crate) fn set_interleaved_buffer(&mut self, buffer: &mut InterleavedVertexBuffer) {
        let CrossFadedPaintPropertyBinderKind::Composite { vertex_data, .. } = &mut self.kind else {
            return;
        };
        let new_vertex_offset = buffer.extend_vertex_format(PATTERN_ZOOM_INTERPOLATED_SIZE, align_of::<u16>());
        if matches!(vertex_data, InterleavedVertexData::Uninitialized) {
            *vertex_data = InterleavedVertexData::Interleaved { offset: new_vertex_offset };
        }
    }

    pub(crate) fn vertex_count(&self, buffer: &InterleavedVertexBuffer) -> usize {
        if self.kind.vertex_offset().is_some() { buffer.vertex_count } else { 0 }
    }

    pub(crate) fn populate_vertex_vector(
        &mut self,
        buffer: &mut InterleavedVertexBuffer,
        length: usize,
        pattern_positions: &ImagePositions,
        pattern_dependencies: Option<&PatternDependency>,
    ) {
        let CrossFadedPaintPropertyBinderKind::Composite { populated_count, vertex_data, .. } = &mut self.kind else {
            return;
        };
        let InterleavedVertexData::Interleaved { offset: vertex_offset } = vertex_data else {
            return;
        };
        match pattern_dependencies {
            Some(dependencies) if !dependencies.mid.is_empty() => {
                if !pattern_positions.is_empty() {
                    // Only the middle pattern reaches the GPU, but a feature
                    // needs all three.
                    let (Some(_), Some(image_mid), Some(_)) = (
                        pattern_positions.get(&dependencies.min),
                        pattern_positions.get(&dependencies.mid),
                        pattern_positions.get(&dependencies.max),
                    ) else {
                        return;
                    };

                    buffer.fill(*populated_count..length, *vertex_offset, bytemuck::cast_slice(&image_mid.tlbr()));
                    *populated_count = (*populated_count).max(length);
                }
            }
            _ => {
                // Unlike other properties with expressions that evaluate to
                // null, the default value for `*-pattern` properties is an
                // empty string and will not have a valid entry in the
                // pattern positions. We still need to populate the attribute
                // buffers to avoid crashes when we try to draw the layer
                // because we don't know at draw time if all features were
                // evaluated to valid pattern dependencies.
                buffer.fill(*populated_count..length, *vertex_offset, bytemuck::cast_slice(&[0u16; 4]));
                *populated_count = (*populated_count).max(length);
            }
        }
    }

    /// Where the property's attributes read the pattern values in `shared`,
    /// the vertices of `buffer`: both read the same values. `None` without
    /// values.
    pub(crate) fn apply_paint_property(
        &self,
        buffer: &InterleavedVertexBuffer,
        shared: &SharedBuffer,
    ) -> Option<SharedAttribute> {
        if let Some(vertex_offset) = self.kind.vertex_offset() {
            let raw_size = buffer.stride as u32;
            // Patterns don't interpolate: one value of four shorts.
            Some(shared_paint(shared, vertex_offset, raw_size, AttributeDataType::UShort4))
        } else {
            None
        }
    }
}

/// What a layer's set of binders does with each binder: a binder writes each feature's values of its property
/// into its place in the interleaved vertices of the set's
/// [`InterleavedVertexBuffer`], and binds them to its program's attributes.
pub trait PaintPropertyBinderBase {
    /// Takes the binder's place in `buffer`'s vertex format, if it writes
    /// any values.
    fn set_interleaved(&mut self, buffer: &mut InterleavedVertexBuffer);
    /// Writes `feature`'s values of the property, the feature at `index` of
    /// its layer in the tile `canonical`, into the vertices of `buffer` up
    /// to `length`, with the images of its patterns where the property is
    /// one, and `formatted_section`, the section of a formatted text value
    /// that the vertices draw.
    #[expect(
        clippy::too_many_arguments,
        reason = "the vertex buffer, the feature and its index range, its patterns, the tile and the formatted section that evaluate a property"
    )]
    fn populate(
        &mut self,
        buffer: &mut InterleavedVertexBuffer,
        feature: &dyn GeometryTileFeature,
        length: usize,
        index: usize,
        pattern_positions: &ImagePositions,
        pattern_dependencies: Option<&PatternDependency>,
        canonical: &CanonicalTileID,
        formatted_section: &Value,
    );
    /// Rewrites the vertex data that depends on feature state. The default,
    /// for the constant and cross-faded binders, does nothing.
    fn update_vectors(
        &mut self,
        _buffer: &mut InterleavedVertexBuffer,
        _states: &crate::bucket::FeatureStates,
        _layer: &TileLayer<'_>,
    ) {
    }
    /// True if a feature state change rewrites this binder's vertex data.
    fn depends_on_feature_state(&self) -> bool {
        false
    }
    /// How many vertices of `buffer` the binder wrote values into: none for
    /// a constant property.
    fn binder_vertex_count(&self, buffer: &InterleavedVertexBuffer) -> usize;
    /// How the factor that mixes the values at two zooms follows the
    /// camera's zoom, for a property whose values mix; `None` for one whose
    /// don't.
    fn mix(&self) -> Option<Mix> {
        None
    }
    /// The attribute that reads the binder's values in `shared`, the
    /// vertices of `buffer` in the context's store; `None` where it wrote
    /// none.
    fn apply(&self, buffer: &InterleavedVertexBuffer, shared: &SharedBuffer) -> Option<SharedAttribute>;
}

impl<T: BinderValue> PaintPropertyBinderBase for PaintPropertyBinder<T> {
    fn set_interleaved(&mut self, buffer: &mut InterleavedVertexBuffer) {
        self.set_interleaved_buffer(buffer);
    }
    fn populate(
        &mut self,
        buffer: &mut InterleavedVertexBuffer,
        feature: &dyn GeometryTileFeature,
        length: usize,
        index: usize,
        _pattern_positions: &ImagePositions,
        _pattern_dependencies: Option<&PatternDependency>,
        canonical: &CanonicalTileID,
        formatted_section: &Value,
    ) {
        self.populate_vertex_vector(buffer, feature, length, index, canonical, formatted_section);
    }
    fn update_vectors(
        &mut self,
        buffer: &mut InterleavedVertexBuffer,
        states: &crate::bucket::FeatureStates,
        layer: &TileLayer<'_>,
    ) {
        self.update_vertex_vectors(buffer, states, layer);
    }
    fn depends_on_feature_state(&self) -> bool {
        match &self.kind {
            PaintPropertyBinderKind::Constant { .. } => false,
            PaintPropertyBinderKind::SourceFunction { expression, .. }
            | PaintPropertyBinderKind::CompositeFunction { expression, .. } => {
                expression.get_expression().has(Dependency::FEATURE_STATE)
            }
        }
    }
    fn binder_vertex_count(&self, buffer: &InterleavedVertexBuffer) -> usize {
        self.vertex_count(buffer)
    }
    fn mix(&self) -> Option<Mix> {
        Some(PaintPropertyBinder::mix(self))
    }
    fn apply(&self, buffer: &InterleavedVertexBuffer, shared: &SharedBuffer) -> Option<SharedAttribute> {
        self.apply_paint_property(buffer, shared)
    }
}

impl PaintPropertyBinderBase for CrossFadedPaintPropertyBinder<Image> {
    fn set_interleaved(&mut self, buffer: &mut InterleavedVertexBuffer) {
        self.set_interleaved_buffer(buffer);
    }
    fn populate(
        &mut self,
        buffer: &mut InterleavedVertexBuffer,
        _feature: &dyn GeometryTileFeature,
        length: usize,
        _index: usize,
        pattern_positions: &ImagePositions,
        pattern_dependencies: Option<&PatternDependency>,
        _canonical: &CanonicalTileID,
        _formatted_section: &Value,
    ) {
        self.populate_vertex_vector(buffer, length, pattern_positions, pattern_dependencies);
    }
    fn binder_vertex_count(&self, buffer: &InterleavedVertexBuffer) -> usize {
        self.vertex_count(buffer)
    }
    fn apply(&self, buffer: &InterleavedVertexBuffer, shared: &SharedBuffer) -> Option<SharedAttribute> {
        self.apply_paint_property(buffer, shared)
    }
}

/// An attribute that reads the values at `offset` in each of the `stride`
/// bytes of `shared`, the vertices of a bucket's paint values, as
/// `data_type`.
fn shared_paint(shared: &SharedBuffer, offset: usize, stride: u32, data_type: AttributeDataType) -> SharedAttribute {
    SharedAttribute::new(shared.clone(), data_type, offset as u32, stride)
}

/// The binders of a layer's data-driven paint properties, which a struct
/// that [`paint_property_binders!`](crate::paint_property_binders!) declares
/// implements: they write each feature's values into the vertices that a
/// mesh's attributes read ([`MeshDesc::paint`](crate::draw::MeshDesc::paint)).
/// A layer reaches them through
/// [`BuildCx::binders`](crate::layer::BuildCx::binders),
/// [`BuildCx::populate`](crate::layer::BuildCx::populate),
/// [`Upload::binders`](crate::layer::Upload::binders) and
/// [`LayerBucket::binders`](crate::layer::LayerBucket::binders); the macro
/// writes the rest.
pub trait PaintBinders: BinderSet {}

/// What [`paint_property_binders!`](crate::paint_property_binders!) writes
/// of a struct of binders, which the engine calls.
pub trait BinderSet: Clone + Send + Wire + 'static {
    /// The evaluated paint values whose data-driven properties the binders
    /// write.
    type Paint: 'static;

    /// The binders of `paint` in a tile at `zoom`.
    fn new(paint: &Self::Paint, zoom: f32) -> Self;

    /// The values, one interleaved vertex per vertex of the bucket.
    fn vertices(&self) -> &InterleavedVertexBuffer;

    /// [`vertices`](Self::vertices), to change.
    fn vertices_mut(&mut self) -> &mut InterleavedVertexBuffer;

    /// Writes the values of `feature`, the feature at `index` of the tile
    /// `canonical`, in its text's formatted section `section`, with the
    /// images of its patterns `pattern` among `positions`, into the
    /// vertices up to `length`.
    #[expect(clippy::too_many_arguments, reason = "the feature, its vertices, its tile, its section and its patterns")]
    fn populate(
        &mut self,
        feature: &dyn GeometryTileFeature,
        length: usize,
        index: usize,
        canonical: &CanonicalTileID,
        section: &Value,
        positions: &ImagePositions,
        pattern: Option<&PatternDependency>,
    );

    /// Rewrites the values that read feature state for `states`, the states
    /// of the features of `layer`.
    fn feature_state(&mut self, states: &crate::bucket::FeatureStates, layer: &TileLayer<'_>);

    /// Whether a feature state change rewrites the values.
    fn depends_on_feature_state(&self) -> bool;

    /// Binds each property's values to the attribute of its program's
    /// property of its name, or reads it from its uniform where `paint` is
    /// the same for every feature.
    fn bind(&self, paint: &Self::Paint, bind: &mut Bind<'_>);

    /// Writes the value of each property of `paint` that a program reads
    /// from its uniform where it is the same for every feature, as `write`
    /// takes the program's properties' names and the value's bytes.
    fn uniforms(paint: &Self::Paint, write: &mut dyn FnMut(&[&str], &[u8]));
}

/// A value of a data-driven paint property as a program reads it from its
/// uniform, which [`paint_property_binders!`](crate::paint_property_binders!)
/// writes for each property.
pub trait UniformValue {
    /// Writes `value`'s bytes as the uniform holds them, for a value that
    /// has a uniform.
    fn write(value: &Evaluated<Self>, write: impl FnOnce(&[u8]))
    where
        Self: Sized;
}

impl UniformValue for f32 {
    fn write(value: &Evaluated<f32>, write: impl FnOnce(&[u8])) {
        write(bytemuck::bytes_of(value.uniform()));
    }
}

impl UniformValue for Color {
    fn write(value: &Evaluated<Color>, write: impl FnOnce(&[u8])) {
        write(bytemuck::bytes_of(&value.uniform().to_float_array()));
    }
}

impl UniformValue for Image {
    /// A pattern has no uniform of its value: its program reads where its
    /// images are in an atlas.
    fn write(_value: &Evaluated<Image>, _write: impl FnOnce(&[u8])) {}
}

/// What the engine writes of a mesh's paint values into its draws' blocks:
/// the factor that mixes each mixed property's values at two zooms, at its
/// member `<name>_t` among the bytes of the blocks of each draw, and each
/// property's value where it is the same for every feature, at its member
/// `<name>` of the layer's uniform blocks.
#[derive(Clone)]
pub(crate) struct MeshPaint {
    /// The offset of each mixed property's factor among the bytes of the
    /// blocks of each draw, and how it follows the zoom.
    pub(crate) mixes: Rc<[(usize, Mix)]>,
    /// Writes the properties' values that their uniforms hold, of the
    /// layer's evaluated paint values, for each program property's name.
    pub(crate) uniforms: WriteUniforms,
}

/// Writes the uniform values of a layer's evaluated paint values, as
/// [`BinderSet::uniforms`] does.
pub(crate) type WriteUniforms = fn(&dyn Any, &mut dyn FnMut(&[&str], &[u8]));

/// [`BinderSet::uniforms`] of `paint`, `P::Paint`.
fn uniforms_of<P: BinderSet>(paint: &dyn Any, write: &mut dyn FnMut(&[&str], &[u8])) {
    if let Some(paint) = paint.downcast_ref::<P::Paint>() {
        P::uniforms(paint, write);
    } else {
        debug_assert!(false, "a mesh's paint values of another type");
    }
}

impl MeshPaint {
    /// Writes the mix factors at the camera's `zoom` into `blocks`, the
    /// bytes of a draw's blocks.
    pub(crate) fn write_mixes(&self, zoom: f32, blocks: &mut [u8]) {
        for (offset, mix) in self.mixes.iter() {
            blocks[*offset..*offset + 4].copy_from_slice(&mix.factor(zoom).to_ne_bytes());
        }
    }

    /// Writes the uniform values of `paint`, the layer's evaluated paint
    /// values, into `bytes`, the bytes of the uniform block `block` of
    /// `program`, at the members of the properties' names.
    pub(crate) fn write_uniforms(
        &self,
        paint: &dyn Any,
        program: &Program,
        block: &mln_gfx::shaders::block::Block,
        bytes: &mut [u8],
    ) {
        (self.uniforms)(paint, &mut |names, value| {
            for name in names {
                if !program.properties.iter().any(|property| property.name == *name) {
                    continue;
                }
                let fits = |ty: MemberType| match ty {
                    MemberType::Float => value.len() == 4,
                    MemberType::Vec4 => value.len() == 16,
                    _ => false,
                };
                if let Some(member) = block.members.iter().find(|member| member.name == *name) {
                    debug_assert!(fits(member.ty), "the uniform member {name} of a type its property's value isn't");
                    if fits(member.ty) {
                        bytes[member.offset..member.offset + value.len()].copy_from_slice(value);
                    }
                }
            }
        });
    }
}

/// A set of binders `P` for each layer of a layout's group, which their
/// bucket holds.
#[derive(Clone, Debug, mln_wire::Wire)]
#[wire(bound = "P: Wire")]
pub struct LayerBinders<P> {
    /// Each layer's id and binders, in the group's order.
    layers: Vec<(String, P)>,
}

impl<P> Default for LayerBinders<P> {
    /// The binders of no layer.
    fn default() -> Self {
        LayerBinders { layers: Vec::new() }
    }
}

impl<P: PaintBinders> LayerBinders<P> {
    /// The binders of each of `layers`, of their paint values at `zoom`.
    pub(crate) fn new<S: StyleLayerType<PaintProperties: PaintSet<Evaluated = P::Paint>>>(
        layers: &[Arc<EvaluatedLayer<S>>],
        zoom: f32,
    ) -> Self {
        let mut binders: Vec<(String, P)> = Vec::with_capacity(layers.len());
        for layer in layers {
            if !binders.iter().any(|(id, _)| *id == layer.style.id) {
                binders.push((layer.style.id.clone(), P::new(&layer.paint, zoom)));
            }
        }
        LayerBinders { layers: binders }
    }

    /// Writes `feature`'s values into each layer's binders, with its
    /// `patterns` among `positions`, as [`BinderSet::populate`] does.
    #[expect(clippy::too_many_arguments, reason = "the feature, its vertices, its tile, its section and its patterns")]
    pub(crate) fn populate(
        &mut self,
        feature: &dyn GeometryTileFeature,
        index: usize,
        length: usize,
        canonical: &CanonicalTileID,
        section: &Value,
        positions: &ImagePositions,
        patterns: &FeaturePatterns,
    ) {
        for (id, binders) in &mut self.layers {
            let pattern = patterns.0.iter().find(|(layer, _)| layer == id).map(|(_, pattern)| pattern);
            binders.populate(feature, length, index, canonical, section, positions, pattern);
        }
    }

    /// The binders of the layer `id`, if it is one of the group's.
    pub(crate) fn of_layer(&self, id: &str) -> Option<&P> {
        self.layers.iter().find(|(layer, _)| layer == id).map(|(_, binders)| binders)
    }

    /// Each layer's id and binders, to change.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut P)> {
        self.layers.iter_mut().map(|(id, binders)| (id.as_str(), binders))
    }
}

/// A layer's binders in a tile's bucket, with the buffer that their values
/// uploaded into and the layer's paint values that they were made of, which
/// a mesh binds ([`MeshDesc::paint`](crate::draw::MeshDesc::paint)). It
/// reads as the binders themselves.
pub struct PaintVertices<'a, P: PaintBinders> {
    binders: &'a P,
    buffer: Option<&'a Buffer<u8>>,
    paint: &'a P::Paint,
}

impl<P: PaintBinders> Clone for PaintVertices<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: PaintBinders> Copy for PaintVertices<'_, P> {}

impl<'a, P: PaintBinders> PaintVertices<'a, P> {
    pub(crate) fn new(binders: &'a P, buffer: Option<&'a Buffer<u8>>, paint: &'a P::Paint) -> Self {
        PaintVertices { binders, buffer, paint }
    }

    /// The buffer of the values, which a program that reads them itself
    /// binds ([`MeshDesc::pulled`](crate::draw::MeshDesc::pulled)); `None`
    /// where every value is the same for every feature.
    #[must_use]
    pub fn buffer(&self) -> Option<&'a Buffer<u8>> {
        self.buffer
    }

    /// The bytes of the values of each vertex, where each binder's are at
    /// its [`offset`](PaintPropertyBinder::offset).
    #[must_use]
    pub fn stride(&self) -> usize {
        self.binders.vertices().stride()
    }

    /// Binds the values to `program`'s properties in `attributes`, or records
    /// those that the paint values they were made of have the same for every
    /// feature in `uniforms`, and returns what the engine writes of them into
    /// the draws' blocks.
    pub(crate) fn bind(
        self,
        program: &'static Program,
        attributes: &mut Vec<(VertexStep, usize, SharedAttribute)>,
        uniforms: &mut Uniforms,
    ) -> MeshPaint {
        let shared = self.buffer.map(|buffer| &buffer.entry);
        let mut mixes = Vec::new();
        self.binders.bind(self.paint, &mut Bind { program, shared, attributes, uniforms, mixes: &mut mixes });
        MeshPaint { mixes: mixes.into(), uniforms: uniforms_of::<P> }
    }
}

impl<P: PaintBinders> std::ops::Deref for PaintVertices<'_, P> {
    type Target = P;

    fn deref(&self) -> &P {
        self.binders
    }
}

/// What [`BinderSet::bind`] binds the binders' values with.
pub struct Bind<'a> {
    program: &'static Program,
    shared: Option<&'a SharedBuffer>,
    attributes: &'a mut Vec<(VertexStep, usize, SharedAttribute)>,
    uniforms: &'a mut Uniforms,
    mixes: &'a mut Vec<(usize, Mix)>,
}

impl Bind<'_> {
    /// Binds `binder`'s values among `buffer`'s to the attributes of the
    /// program's properties `properties`, of a vertex or of an instance, or
    /// records them as read from their uniforms where `constant` or the
    /// binder wrote none, and records where each mixed property's factor
    /// goes among the bytes of the blocks of each draw. A property the
    /// program doesn't read is skipped, and one without an attribute is
    /// read by the program itself.
    pub fn property(
        &mut self,
        binder: &dyn PaintPropertyBinderBase,
        buffer: &InterleavedVertexBuffer,
        properties: &[&str],
        constant: bool,
    ) {
        let values = if constant || binder.binder_vertex_count(buffer) == 0 {
            None
        } else {
            self.shared.and_then(|shared| binder.apply(buffer, shared))
        };
        for name in properties {
            let Some((index, property)) =
                self.program.properties.iter().enumerate().find(|(_, property)| property.name == *name)
            else {
                continue;
            };
            match (&values, self.program.property_attribute(property)) {
                (Some(values), Some((step, attribute))) => self.attributes.push((step, attribute, values.clone())),
                (Some(_), None) => {}
                (None, _) => self.uniforms.insert(index),
            }
            if let Some(mix) = binder.mix().filter(|_| matches!(property.ty, PropertyType::Float | PropertyType::Color))
                && let Some(offset) = mix_offset(self.program, name)
            {
                self.mixes.push((offset, mix));
            }
        }
    }
}

/// The offset of the member `<name>_t` among the bytes of the blocks of each
/// draw of `program`, its storage buffers' blocks in order, if one of them
/// has it.
fn mix_offset(program: &Program, name: &str) -> Option<usize> {
    let mut start = 0;
    for buffer in program.buffers {
        let BufferKind::Storage { block, .. } = buffer.kind else { continue };
        if let Some(member) = block
            .members
            .iter()
            .find(|member| member.ty == MemberType::Float && member.name.strip_suffix("_t") == Some(name))
        {
            return Some(start + member.offset);
        }
        start += block.size;
    }
    None
}

/// Declares the binders of a layer type's data-driven paint properties: a
/// struct with a binder for each property, which writes each feature's
/// values of the property into the vertices that a program's attributes
/// read, and implements [`PaintBinders`](crate::layer::PaintBinders).
///
/// ```ignore
/// mln_render::paint_property_binders!(
///     pub struct DotBinders for DotPaint::PossiblyEvaluated {
///         dot_color: PaintPropertyBinder<Color> => ["color"],
///         dot_radius: PaintPropertyBinder<f32> => ["radius"],
///     }
/// );
/// ```
///
/// Each field is a property of the evaluated paint set (an
/// [`mln_style::properties!`] set's `PossiblyEvaluated`), its binder, a
/// [`PaintPropertyBinder`](crate::layer::PaintPropertyBinder), or a
/// [`CrossFadedPaintPropertyBinder`](crate::layer::CrossFadedPaintPropertyBinder)
/// of a `*-pattern` property, and the names of the program's properties
/// ([`Property`](mln_gfx::shaders::program::Property)) that read its values.
/// The fields are as visible as the struct.
#[macro_export]
macro_rules! paint_property_binders {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident for $props:path {
            $($field:ident: $binder:ty => [$($property:literal),+]),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, $crate::__private::mln_wire::Wire)]
        #[wire(crate = $crate::__private::mln_wire)]
        $vis struct $name {
            vertices: $crate::__private::InterleavedVertexBuffer,
            $($vis $field: $binder,)+
        }

        impl $crate::layer::PaintBinders for $name {}

        impl $crate::__private::BinderSet for $name {
            type Paint = $props;

            fn new(paint: &$props, zoom: f32) -> Self {
                let mut binders = $name {
                    vertices: ::core::default::Default::default(),
                    $($field: <$binder>::create(&paint.$field, zoom),)+
                };
                $($crate::__private::PaintPropertyBinderBase::set_interleaved(&mut binders.$field, &mut binders.vertices);)+
                binders
            }

            fn vertices(&self) -> &$crate::__private::InterleavedVertexBuffer {
                &self.vertices
            }

            fn vertices_mut(&mut self) -> &mut $crate::__private::InterleavedVertexBuffer {
                &mut self.vertices
            }

            fn populate(
                &mut self,
                feature: &dyn $crate::__private::mln_tile_id::geometry_tile_data::GeometryTileFeature,
                length: usize,
                index: usize,
                canonical: &$crate::__private::mln_tile_id::CanonicalTileID,
                section: &$crate::__private::mln_style::expression::Value,
                positions: &$crate::__private::ImagePositions,
                pattern: ::core::option::Option<&$crate::__private::PatternDependency>,
            ) {
                let $name { vertices, $($field,)+ } = self;
                vertices.populate(length, |vertices| {
                    $($crate::__private::PaintPropertyBinderBase::populate(
                        $field,
                        vertices,
                        feature,
                        length,
                        index,
                        positions,
                        pattern,
                        canonical,
                        section,
                    );)+
                });
            }

            fn feature_state(
                &mut self,
                states: &$crate::__private::FeatureStates,
                layer: &$crate::__private::mln_tile::TileLayer<'_>,
            ) {
                let $name { vertices, $($field,)+ } = self;
                $($crate::__private::PaintPropertyBinderBase::update_vectors($field, vertices, states, layer);)+
            }

            fn depends_on_feature_state(&self) -> bool {
                false $(|| $crate::__private::PaintPropertyBinderBase::depends_on_feature_state(&self.$field))+
            }

            fn bind(&self, paint: &$props, bind: &mut $crate::__private::Bind<'_>) {
                $(bind.property(&self.$field, &self.vertices, &[$($property),+], paint.$field.is_constant());)+
            }

            fn uniforms(paint: &$props, write: &mut dyn FnMut(&[&str], &[u8])) {
                $($crate::__private::UniformValue::write(&paint.$field, |value| write(&[$($property),+], value));)+
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use mln_gfx::shaders::block::UniformBlock;
    use mln_gfx::shaders::program::{Buffer as ProgramBuffer, Projection, Property, Sources, Stages};

    use super::*;

    mln_gfx::uniform_block! {
        struct Drawn { pub extrude: [f32; 2], pub color_t: f32, #[padding] pub pad: f32 }
    }

    mln_gfx::uniform_block! {
        struct Props { pub color: [f32; 4], pub radius: f32, #[padding] pub pad1: f32, #[padding] pub pad2: [f32; 2] }
    }

    static PROGRAM: Program = Program {
        name: "test",
        buffers: &[
            ProgramBuffer::storage::<Drawn>("draws", Stages::Vertex),
            ProgramBuffer::uniform::<Props>("props", 0),
        ],
        attributes: &[],
        instance_attributes: &[],
        textures: &[],
        properties: &[Property::color("color")],
        projection: Projection::None,
        sources: Sources::NONE,
    };

    #[test]
    fn a_mixed_propertys_factor_goes_at_its_member_among_the_blocks_of_each_draw() {
        assert_eq!(mix_offset(&PROGRAM, "color"), Some(8));
        assert_eq!(mix_offset(&PROGRAM, "radius"), None);
        let paint = MeshPaint { mixes: Rc::from([(8, Mix::None)]), uniforms: |_, _| {} };
        let mut bytes = [0xff; 16];
        paint.write_mixes(3.5, &mut bytes);
        assert_eq!(bytes[8..12], 0.0f32.to_ne_bytes());
        assert_eq!(bytes[..8], [0xff; 8]);
    }

    #[test]
    fn a_propertys_uniform_value_goes_at_its_member_of_the_layers_blocks() {
        fn uniforms(_: &dyn Any, write: &mut dyn FnMut(&[&str], &[u8])) {
            write(&["color"], bytemuck::bytes_of(&[1.0f32, 0.5, 0.25, 1.0]));
            // A property the program doesn't read.
            write(&["radius"], bytemuck::bytes_of(&4.0f32));
        }
        let paint = MeshPaint { mixes: Rc::from([]), uniforms };
        let mut props = Props::default();
        paint.write_uniforms(&(), &PROGRAM, Props::BLOCK, bytemuck::bytes_of_mut(&mut props));
        assert_eq!(props.color, [1.0, 0.5, 0.25, 1.0]);
        assert_eq!(props.radius, 0.0);
    }
}
