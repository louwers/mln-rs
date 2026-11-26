// The binders of a layer's data-driven paint properties, which write each
// feature's values into the vertices its draws read: `PaintPropertyBinder<T>`
// for a constant, source or composite function, and
// `CrossFadedPaintPropertyBinder<T>` for the cross-faded (pattern) ones.
// `paint_property_binders!` declares a layer type's set of them.
//
// The binders write into the bucket's `InterleavedVertexBuffer`, which is
// passed in, in the vertices' in-memory layout (native-endian floats and
// u16s), so the buffer is the GPU vertex data as it uploads.

use mln_gfx::gfx_types::AttributeDataType;
use mln_style::color::Color;
use mln_style::expression::{Dependency, EvaluationContext, Image, Value, ValueConverter};
use mln_style::feature::GeometryTileFeature;
use mln_style::property::evaluated::{Evaluated, Faded};
use mln_style::property::expression::PropertyExpression;
use mln_style::range::Range;
use mln_style::tile_id::CanonicalTileID;
use mln_vec_map::VecMap;

use crate::draw::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::draw::vertices::SharedVertices;
use crate::layer::bucket_geometry::GeometryVec;
use crate::layer::paint_property_statistics::{PaintPropertyStatistics, StatisticValue};
use crate::layout::pattern_layout::PatternDependency;
use crate::text::atlas::ImagePositions;
use crate::tile::TileLayer;
use crate::tile::feature::feature_id_to_string;

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
pub(crate) struct InterleavedVertexBuffer {
    pub(crate) stride: usize,
    pub(crate) vertex_count: usize,
    pub(crate) shared_vertex_vector: GeometryVec<u8>,
}

impl InterleavedVertexBuffer {
    /// Writes `value` at `offset` in each of the vertices `indexes`.
    pub(crate) fn fill(&mut self, indexes: std::ops::Range<usize>, offset: usize, value: &[u8]) {
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
    pub(crate) fn extend_vertex_format(&mut self, size: usize, alignment: usize) -> usize {
        let mut offset = self.stride & (alignment - 1);
        if offset != 0 {
            offset = alignment - offset;
        }

        let vertex_offset = self.stride + offset;
        self.stride += size + offset;

        vertex_offset
    }
}

/// The value types of data-driven paint properties with a single attribute
/// (`float` and `Color` in circle, fill and line): the attribute's value of
/// a value ([`attribute_value`](Self::attribute_value)), and its type.
pub(crate) trait BinderValue: Clone + Default + ValueConverter + StatisticValue + std::fmt::Debug {
    /// The number of floats `attribute_value` uses.
    const DIMENSIONS: usize;
    const BASE_DATA_TYPE: AttributeDataType;
    const INTERPOLATED_DATA_TYPE: AttributeDataType;

    /// The attribute's floats; only the first `DIMENSIONS` entries are used.
    fn attribute_value(&self) -> [f32; 2];
}

impl BinderValue for f32 {
    const DIMENSIONS: usize = 1;
    const BASE_DATA_TYPE: AttributeDataType = AttributeDataType::Float;
    const INTERPOLATED_DATA_TYPE: AttributeDataType = AttributeDataType::Float2;

    fn attribute_value(&self) -> [f32; 2] {
        [*self, 0.0]
    }
}

/// Packs two values into a `u16`: `a as u16 * 256 + b as u16`, computed as an
/// `i32` and truncated.
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
    pub(crate) fn offset(&self) -> Option<usize> {
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
        state: &mln_style::feature::FeatureState,
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

#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) struct PaintPropertyBinder<T> {
    pub(crate) kind: PaintPropertyBinderKind<T>,
    pub(crate) statistics: PaintPropertyStatistics<T>,
}

impl<T: BinderValue> PaintPropertyBinder<T> {
    pub(crate) fn create(value: &Evaluated<T>, zoom: f32) -> Self {
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
        states: &crate::layer::bucket::FeatureStates,
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

    /// The factor that interpolates between the vertex's two zoom stops; 0
    /// unless the binder is a composite function.
    pub(crate) fn interpolation_factor(&self, current_zoom: f32) -> f32 {
        match &self.kind {
            PaintPropertyBinderKind::Constant { .. } | PaintPropertyBinderKind::SourceFunction { .. } => 0.0,
            PaintPropertyBinderKind::CompositeFunction { expression, zoom_range, .. } => {
                let possibly_rounded_zoom =
                    if expression.get_use_integer_zoom() { current_zoom.floor() } else { current_zoom };

                mln_style::math::fmaxf(
                    0.0,
                    mln_style::math::fminf(1.0, expression.interpolation_factor(*zoom_range, possibly_rounded_zoom)),
                )
            }
        }
    }

    /// Where the property's attribute reads its values in `shared`, the
    /// vertices of `buffer`; `None` without values.
    pub(crate) fn apply_paint_property(
        &self,
        buffer: &InterleavedVertexBuffer,
        shared: &SharedVertices,
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

/// The binder of a `*-pattern` property, whose two attributes are four `u16`s
/// each.
#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) struct CrossFadedPaintPropertyBinder<T> {
    pub(crate) kind: CrossFadedPaintPropertyBinderKind<T>,
}

/// The size of a zoom-interpolated pattern vertex: eight `u16`s.
const PATTERN_ZOOM_INTERPOLATED_SIZE: usize = 16;

impl CrossFadedPaintPropertyBinder<Image> {
    #[must_use]
    pub(crate) fn create(value: &Evaluated<Image>, zoom: f32) -> Self {
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

    /// The zoom interpolation factors, always (0, 0) for pattern binders.
    #[must_use]
    #[expect(clippy::unused_self, reason = "the paint property macros call it on every binder alike")]
    pub(crate) fn interpolation_factor(&self, _current_zoom: f32) -> (f32, f32) {
        (0.0, 0.0)
    }

    /// Where the property's attributes read the pattern values in `shared`,
    /// the vertices of `buffer`: both read the same values. `None` without
    /// values.
    pub(crate) fn apply_paint_property(
        &self,
        buffer: &InterleavedVertexBuffer,
        shared: &SharedVertices,
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

/// What a layer's set of binders and [`bind_paint_property`] do
/// with either binder enum.
pub(crate) trait PaintPropertyBinderBase {
    fn set_interleaved(&mut self, buffer: &mut InterleavedVertexBuffer);
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
        _states: &crate::layer::bucket::FeatureStates,
        _layer: &TileLayer<'_>,
    ) {
    }
    /// True if a feature state change rewrites this binder's vertex data.
    fn depends_on_feature_state(&self) -> bool {
        false
    }
    fn binder_vertex_count(&self, buffer: &InterleavedVertexBuffer) -> usize;
    fn apply(&self, buffer: &InterleavedVertexBuffer, shared: &SharedVertices) -> Option<SharedAttribute>;
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
        states: &crate::layer::bucket::FeatureStates,
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
    fn apply(&self, buffer: &InterleavedVertexBuffer, shared: &SharedVertices) -> Option<SharedAttribute> {
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
    fn apply(&self, buffer: &InterleavedVertexBuffer, shared: &SharedVertices) -> Option<SharedAttribute> {
        self.apply_paint_property(buffer, shared)
    }
}

/// An attribute that reads the values at `offset` in each of the `stride`
/// bytes of `shared`, the vertices of a bucket's paint values, as
/// `data_type`.
pub(crate) fn shared_paint(
    shared: &SharedVertices,
    offset: usize,
    stride: u32,
    data_type: AttributeDataType,
) -> SharedAttribute {
    SharedAttribute::new(shared.clone(), data_type, offset as u32, stride)
}

/// Binds `binder`'s values, `shared`, the vertices of `buffer`, to the
/// attributes `attributes` in `attrs`, or records the properties of those
/// names in `uniforms` if the property is constant or has no values.
pub(crate) fn bind_paint_property(
    binder: &dyn PaintPropertyBinderBase,
    buffer: &InterleavedVertexBuffer,
    shared: Option<&SharedVertices>,
    attributes: &[&'static str],
    is_constant: bool,
    attrs: &mut VertexAttributeArray,
    uniforms: &mut Uniforms,
) {
    if is_constant || binder.binder_vertex_count(buffer) == 0 {
        for &attribute in attributes {
            uniforms.insert(attribute);
        }
    } else if let Some(values) = shared.and_then(|shared| binder.apply(buffer, shared)) {
        for &attribute in attributes {
            attrs.set_shared(attribute, values.clone());
        }
    }
}

/// Declares the binders of one layer type's data-driven paint properties: a
/// struct with one binder per property, each with the names of the attributes
/// a program reads its values from (fields in the interleaved vertex order),
/// and the shared `InterleavedVertexBuffer`.
macro_rules! paint_property_binders {
    (
        $(#[$meta:meta])*
        pub struct $name:ident for $props:path {
            $($field:ident: $binder:ty => [$($attribute:expr),+]),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, mln_wire::Wire)]
        pub(crate) struct $name {
            pub interleaved_vertex_buffer: $crate::layer::paint_property_binder::InterleavedVertexBuffer,
            $(pub $field: $binder,)+
        }

        impl $name {
            pub(crate) fn new(properties: &$props, z: f32) -> Self {
                let mut result = $name {
                    interleaved_vertex_buffer: Default::default(),
                    $($field: <$binder>::create(&properties.$field, z),)+
                };
                $($crate::layer::paint_property_binder::PaintPropertyBinderBase::set_interleaved(
                    &mut result.$field, &mut result.interleaved_vertex_buffer);)+
                result
            }

            #[expect(clippy::too_many_arguments, reason = "the feature and its index range, its patterns, the tile and the formatted section that evaluate every property")]
            pub(crate) fn populate_vertex_vectors(
                &mut self,
                feature: &dyn mln_style::feature::GeometryTileFeature,
                length: usize,
                index: usize,
                pattern_positions: &crate::text::atlas::ImagePositions,
                pattern_dependencies: Option<&$crate::layout::pattern_layout::PatternDependency>,
                canonical: &mln_style::tile_id::CanonicalTileID,
                formatted_section: &mln_style::expression::Value,
            ) {
                let buffer = &mut self.interleaved_vertex_buffer;
                let added = (length - buffer.vertex_count) * buffer.stride;
                let data = buffer.shared_vertex_vector.get_mut();
                data.resize(data.len() + added, 0);

                $($crate::layer::paint_property_binder::PaintPropertyBinderBase::populate(
                    &mut self.$field,
                    &mut self.interleaved_vertex_buffer,
                    feature,
                    length,
                    index,
                    pattern_positions,
                    pattern_dependencies,
                    canonical,
                    formatted_section,
                );)+

                let buffer = &mut self.interleaved_vertex_buffer;
                buffer.vertex_count += length - buffer.vertex_count;
                // The render thread copies the vector when it builds draws.
            }

            pub(crate) fn update_vertex_vectors(
                &mut self,
                states: &$crate::layer::bucket::FeatureStates,
                layer: &crate::tile::TileLayer<'_>,
                _image_positions: &crate::text::atlas::ImagePositions,
            ) {
                // Values that moved to the render thread don't depend on
                // feature state (`depends_on_feature_state`).
                if self.interleaved_vertex_buffer.shared_vertex_vector.is_moved() {
                    return;
                }
                $($crate::layer::paint_property_binder::PaintPropertyBinderBase::update_vectors(
                    &mut self.$field, &mut self.interleaved_vertex_buffer, states, layer);)+
            }

            /// True if a feature state change rewrites the data-driven values.
            pub(crate) fn depends_on_feature_state(&self) -> bool {
                false $(|| $crate::layer::paint_property_binder::PaintPropertyBinderBase::depends_on_feature_state(
                    &self.$field))+
            }

            /// Binds each property's values, `shared`, the vertices of the
            /// paint values, to its attributes in `attrs`, or records the
            /// properties of its attributes' names in `uniforms` where
            /// `evaluated` is constant.
            pub(crate) fn bind(
                &self,
                evaluated: &$props,
                shared: Option<&$crate::draw::vertices::SharedVertices>,
                attrs: &mut $crate::draw::vertex_attribute::VertexAttributeArray,
                uniforms: &mut $crate::draw::vertex_attribute::Uniforms,
            ) {
                $($crate::layer::paint_property_binder::bind_paint_property(
                    &self.$field,
                    &self.interleaved_vertex_buffer,
                    shared,
                    &[$($attribute),+],
                    evaluated.$field.is_constant(),
                    attrs,
                    uniforms,
                );)+
            }
        }
    };
}
pub(crate) use paint_property_binders;
