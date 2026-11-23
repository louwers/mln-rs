// The bucket is built on a worker and then owned by its tile's contents on
// the render thread, where placement, cross-tile indexing and sorting
// change its `state` in the frame's update. What placement writes, the
// bucket keeps (`Upload::keep`), and its buffers take again.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::sync::Arc;

use glam::Vec2;
use mln_gfx::gfx_types::AttributeDataType::{Float, Float2, Float3, Short2, Short4, UShort2, UShort4};
use mln_render::draw::Vertex;
use mln_render::layer::{Bucket, GpuVec, Indexes, LayerBinders, PaintPropertyBinder, Upload};
use mln_style::expression::EvaluationContext;
use mln_style::property_expression::PropertyExpression;
use mln_style::property_value::PropertyValue;
use mln_style_types::TextWritingModeType;
use mln_tile_id::geometry_tile_data::{GeometryCoordinate, GeometryCoordinates, GeometryTileFeature};
use mln_util::Color;
use mln_util::range::Range;
use mln_util::serialization::payload::pod;

use crate::symbol::instance::{SymbolInstance, SymbolInstancePools, WritingModes};
use crate::symbol::pool::PoolRange;
use crate::symbol::quads::SymbolQuad;
use crate::symbol::{SymbolLayoutProperties, SymbolPaintProperties};

mln_render::paint_property_binders!(
    pub(crate) struct SymbolIconBinders for SymbolPaintProperties::PossiblyEvaluated {
        icon_opacity: PaintPropertyBinder<f32> => ["opacity"],
        icon_color: PaintPropertyBinder<Color> => ["fill_color"],
        icon_halo_color: PaintPropertyBinder<Color> => ["halo_color"],
        icon_halo_width: PaintPropertyBinder<f32> => ["halo_width"],
        icon_halo_blur: PaintPropertyBinder<f32> => ["halo_blur"],
    }
);

mln_render::paint_property_binders!(
    pub(crate) struct SymbolTextBinders for SymbolPaintProperties::PossiblyEvaluated {
        text_opacity: PaintPropertyBinder<f32> => ["opacity"],
        text_color: PaintPropertyBinder<Color> => ["fill_color"],
        text_halo_color: PaintPropertyBinder<Color> => ["halo_color"],
        text_halo_width: PaintPropertyBinder<f32> => ["halo_width"],
        text_halo_blur: PaintPropertyBinder<f32> => ["halo_blur"],
    }
);

/// The layout attributes of a symbol quad's vertex, and the quad's atlas
/// origin.
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct SymbolLayoutVertex {
    pub(crate) a1: [i16; 4],
    pub(crate) a2: [u16; 4],
    pub(crate) a3: [i16; 4],
    /// The top-left of the quad's atlas rectangle, the same on all four of its
    /// vertices. The shaders interpolate the texture coordinate relative to it
    /// and add it back per fragment, as interpolating the absolute coordinate
    /// rounds differently at different atlas positions, and which tile reserves
    /// an image first, and so its position, varies from run to run.
    pub(crate) a4: [u16; 2],
    /// The symbol's `symbol-height-offset`, in meters.
    pub(crate) a5: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct SymbolDynamicLayoutVertex {
    pub(crate) a1: [f32; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct SymbolOpacityVertex {
    pub(crate) a1: [f32; 1],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct CollisionBoxLayoutVertex {
    pub(crate) a1: [i16; 2],
    pub(crate) a2: [i16; 2],
    pub(crate) a3: [i16; 2],
    /// The symbol's `symbol-height-offset`, in meters.
    pub(crate) a4: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct CollisionBoxDynamicVertex {
    pub(crate) a1: [u16; 2],
    pub(crate) a2: [f32; 2],
}

impl Vertex for SymbolLayoutVertex {
    const ATTRIBUTES: &'static [mln_gfx::gfx_types::AttributeDataType] = &[Short4, UShort4, Short4, UShort2, Float];
    const NAMES: &'static [&'static str] = &["pos_offset", "data", "pixeloffset", "tex_origin", "height_offset"];
}

impl Vertex for SymbolDynamicLayoutVertex {
    const ATTRIBUTES: &'static [mln_gfx::gfx_types::AttributeDataType] = &[Float3];
    const NAMES: &'static [&'static str] = &["projected_pos"];
}

impl Vertex for SymbolOpacityVertex {
    const ATTRIBUTES: &'static [mln_gfx::gfx_types::AttributeDataType] = &[Float];
    const NAMES: &'static [&'static str] = &["fade_opacity"];
}

impl Vertex for CollisionBoxLayoutVertex {
    const ATTRIBUTES: &'static [mln_gfx::gfx_types::AttributeDataType] = &[Short2, Short2, Short2, Float];
    const NAMES: &'static [&'static str] = &["pos", "anchor_pos", "extrude", "height_offset"];
}

impl Vertex for CollisionBoxDynamicVertex {
    const ATTRIBUTES: &'static [mln_gfx::gfx_types::AttributeDataType] = &[UShort2, Float2];
    const NAMES: &'static [&'static str] = &["placed", "shift"];
}

/// A corner of a quad: its offset from the anchor, its texture coordinate
/// and its pixel offset.
#[derive(Clone, Copy, Debug)]
pub(crate) struct QuadCorner {
    pub(crate) offset: Vec2,
    pub(crate) tex: [u16; 2],
    pub(crate) pixel_offset: Vec2,
}

pub(crate) const MAX_GLYPH_ICON_SIZE: u16 = 255;
pub(crate) const SIZE_PACK_FACTOR: u16 = 128;
pub(crate) const MAX_PACKED_SIZE: u16 = MAX_GLYPH_ICON_SIZE * SIZE_PACK_FACTOR;

#[derive(Clone, Copy, Debug, PartialEq, mln_wire::Wire)]
pub(crate) struct SortKeyRange {
    pub(crate) sort_key: f32,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ZoomEvaluatedSize {
    pub(crate) is_zoom_constant: bool,
    pub(crate) is_feature_constant: bool,
    pub(crate) size_t: f32,
    pub(crate) size: f32,
    pub(crate) layout_size: f32,
}

/// Evaluates a symbol's `text-size` or `icon-size`: a constant, a source
/// function or a composite function.
#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) enum SymbolSizeBinder {
    Constant {
        layout_size: f32,
        covering_ranges: Option<(Range<f32>, Range<f32>)>,
        expression: Option<PropertyExpression<f32>>,
    },
    SourceFunction {
        expression: PropertyExpression<f32>,
        default_value: f32,
    },
    CompositeFunction {
        expression: PropertyExpression<f32>,
        default_value: f32,
        layout_zoom: f32,
        covering_zoom_stops: Range<f32>,
    },
}

impl SymbolSizeBinder {
    #[must_use]
    pub(crate) fn create(tile_zoom: f32, size_property: &PropertyValue<f32>, default_value: f32) -> Self {
        let expression = match size_property {
            PropertyValue::Undefined => {
                return SymbolSizeBinder::Constant {
                    layout_size: default_value,
                    covering_ranges: None,
                    expression: None,
                };
            }
            PropertyValue::Constant(layout_size) => {
                return SymbolSizeBinder::Constant {
                    layout_size: *layout_size,
                    covering_ranges: None,
                    expression: None,
                };
            }
            PropertyValue::Expression(expression) => expression.clone(),
        };
        if expression.is_feature_constant() {
            let layout_size = expression.evaluate(&EvaluationContext::new(tile_zoom + 1.0), 0.0);
            // A feature-constant expression without a zoom dependency (e.g.
            // one that only depends on the global state) has no zoom curve.
            let covering_ranges = if expression.is_zoom_constant() {
                None
            } else {
                let zoom_levels = expression.get_covering_stops(tile_zoom, tile_zoom + 1.0);
                Some((
                    zoom_levels,
                    Range {
                        min: expression.evaluate(&EvaluationContext::new(zoom_levels.min), 0.0),
                        max: expression.evaluate(&EvaluationContext::new(zoom_levels.max), 0.0),
                    },
                ))
            };
            SymbolSizeBinder::Constant { layout_size, covering_ranges, expression: Some(expression) }
        } else if expression.is_zoom_constant() {
            SymbolSizeBinder::SourceFunction { expression, default_value }
        } else {
            let covering_zoom_stops = expression.get_covering_stops(tile_zoom, tile_zoom + 1.0);
            SymbolSizeBinder::CompositeFunction {
                expression,
                default_value,
                layout_zoom: tile_zoom + 1.0,
                covering_zoom_stops,
            }
        }
    }

    pub(crate) fn vertex_size_data(&self, feature: &dyn GeometryTileFeature) -> Range<f32> {
        match self {
            SymbolSizeBinder::Constant { .. } => Range { min: 0.0, max: 0.0 },
            SymbolSizeBinder::SourceFunction { expression, default_value } => {
                let size = expression.evaluate(&EvaluationContext::from_feature(Some(feature)), *default_value);
                Range { min: size, max: size }
            }
            SymbolSizeBinder::CompositeFunction { expression, default_value, covering_zoom_stops, .. } => Range {
                min: expression.evaluate(
                    &EvaluationContext::with_zoom_feature(covering_zoom_stops.min, Some(feature)),
                    *default_value,
                ),
                max: expression.evaluate(
                    &EvaluationContext::with_zoom_feature(covering_zoom_stops.max, Some(feature)),
                    *default_value,
                ),
            },
        }
    }

    #[must_use]
    pub(crate) fn evaluate_for_zoom(&self, current_zoom: f32) -> ZoomEvaluatedSize {
        let unused = 0.0_f32;
        match self {
            SymbolSizeBinder::Constant { layout_size, covering_ranges, expression } => {
                let mut size = *layout_size;
                let is_zoom_constant = !(covering_ranges.is_some() || expression.is_some());
                if let (Some((zoom_levels, size_levels)), Some(expression)) = (covering_ranges, expression) {
                    // Even though we could get the exact value of the camera
                    // function at the current zoom, we intentionally do not:
                    // instead, we interpolate between the camera function
                    // values at a pair of zoom stops covering [tile zoom,
                    // tile zoom + 1] in order to be consistent with this
                    // restriction on composite functions.
                    let t = mln_math::clamp_f32(expression.interpolation_factor(*zoom_levels, current_zoom), 0.0, 1.0);
                    size = size_levels.min + t * (size_levels.max - size_levels.min);
                } else if let Some(expression) = expression {
                    size = expression.evaluate(&EvaluationContext::new(current_zoom), 0.0);
                }
                ZoomEvaluatedSize {
                    is_zoom_constant,
                    is_feature_constant: true,
                    size_t: unused,
                    size,
                    layout_size: *layout_size,
                }
            }
            SymbolSizeBinder::SourceFunction { .. } => ZoomEvaluatedSize {
                is_zoom_constant: true,
                is_feature_constant: false,
                size_t: unused,
                size: unused,
                layout_size: unused,
            },
            SymbolSizeBinder::CompositeFunction { expression, covering_zoom_stops, .. } => {
                let size_interpolation_t =
                    mln_math::clamp_f32(expression.interpolation_factor(*covering_zoom_stops, current_zoom), 0.0, 1.0);
                ZoomEvaluatedSize {
                    is_zoom_constant: false,
                    is_feature_constant: false,
                    size_t: size_interpolation_t,
                    size: unused,
                    layout_size: unused,
                }
            }
        }
    }
}

/// A run of a buffer's quads that shows together: the glyphs of a symbol's
/// text at one justification or orientation, or its icon.
#[derive(Clone, Debug, mln_wire::Wire)]
pub(crate) struct PlacedSymbol {
    pub(crate) anchor_point: Vec2,
    pub(crate) segment: usize,
    pub(crate) lower_size: f32,
    pub(crate) upper_size: f32,
    pub(crate) line_offset: [f32; 2],
    pub(crate) writing_modes: WritingModes,
    /// The line the symbol is placed along, in its buffer's `pools.lines`.
    pub(crate) line: PoolRange,
    /// The distance along `line` from the anchor to each vertex, in its
    /// buffer's `pools.tile_distances`. Empty for icons.
    pub(crate) tile_distances: PoolRange,
    /// The x offset of each glyph, or of the icon, in its buffer's
    /// `pools.glyph_offsets`. There are four vertices for each.
    pub(crate) glyph_offsets: PoolRange,
    /// The index of the run's first vertex.
    pub(crate) start_index: usize,
    pub(crate) angle: f32,
    /// The symbol's `symbol-height-offset`, in meters above the ground.
    pub(crate) height_offset: f32,
    /// Faded out or a duplicate, as the last opacity update found it.
    pub(crate) hidden: bool,
}

/// A kind of a bucket's quads: its vertices, where placement draws and
/// shows them, and the runs that show together.
#[derive(Clone, Debug, Default, mln_wire::Wire)]
pub(crate) struct Buffer {
    pub(crate) attribute_data: GpuVec<SymbolLayoutVertex>,
    /// Where placement draws each vertex.
    pub(crate) dynamic_attribute_data: GpuVec<SymbolDynamicLayoutVertex>,
    /// Positions for each layer that shares the bucket at another
    /// translation than its leader's. A layer keeps them for the bucket's
    /// life, once it has them. Placement makes them once the bucket is on
    /// the map side, so they don't cross with it from a worker.
    #[wire(skip)]
    pub(crate) layer_dynamic_attribute_data: BTreeMap<String, GpuVec<SymbolDynamicLayoutVertex>>,
    /// Each vertex's fade opacity, which placement writes.
    pub(crate) opacity_attribute_data: GpuVec<SymbolOpacityVertex>,
    /// The quads' triangles, which placement sorts.
    pub(crate) triangles: Indexes,
    pub(crate) placed_symbols: Vec<PlacedSymbol>,
    pub(crate) pools: PlacedSymbolPools,
    /// What of the buffer can draw at the opacities placement last wrote,
    /// once it wrote them.
    #[wire(skip)]
    pub(crate) visible: Option<Visible>,
}

/// Which of a buffer's segments have a vertex that can draw, and whether
/// any of its vertices can.
#[derive(Clone, Debug, Default)]
pub(crate) struct Visible {
    segments: Vec<bool>,
    any: bool,
}

/// The geometry of a buffer's placed symbols, which they hold ranges of.
#[derive(Clone, Debug, Default, mln_wire::Wire)]
pub(crate) struct PlacedSymbolPools {
    pub(crate) lines: GeometryCoordinates,
    #[wire(with = "pod")]
    pub(crate) tile_distances: Vec<f32>,
    #[wire(with = "pod")]
    pub(crate) glyph_offsets: Vec<f32>,
}

impl PlacedSymbolPools {
    pub(crate) fn line(&self, symbol: &PlacedSymbol) -> &[GeometryCoordinate] {
        symbol.line.of(&self.lines)
    }

    pub(crate) fn tile_distances(&self, symbol: &PlacedSymbol) -> &[f32] {
        symbol.tile_distances.of(&self.tile_distances)
    }

    pub(crate) fn glyph_offsets(&self, symbol: &PlacedSymbol) -> &[f32] {
        symbol.glyph_offsets.of(&self.glyph_offsets)
    }
}

impl Buffer {
    /// The positions `layer_id` draws: its own when it has them, the
    /// leader's otherwise.
    pub(crate) fn dynamic_attribute_data_for(&self, layer_id: &str) -> &GpuVec<SymbolDynamicLayoutVertex> {
        self.layer_dynamic_attribute_data.get(layer_id).unwrap_or(&self.dynamic_attribute_data)
    }

    /// The positions placement lays out, the leader's or, with a layer ID,
    /// that layer's (made on first use), with the placed symbols they are
    /// for and their pools.
    pub(crate) fn positions_and_symbols(
        &mut self,
        layer_id: Option<&str>,
    ) -> (&mut Vec<SymbolDynamicLayoutVertex>, &[PlacedSymbol], &PlacedSymbolPools) {
        let positions = match layer_id {
            None => &mut self.dynamic_attribute_data,
            Some(layer_id) => self.layer_dynamic_attribute_data.entry(layer_id.to_owned()).or_default(),
        };
        (positions.get_mut(), &self.placed_symbols, &self.pools)
    }

    /// Whether any of the `count` opacity vertices from `offset` can draw. A
    /// range that placement hasn't filled yet counts as visible.
    pub(crate) fn has_visible_vertices(&self, offset: usize, count: usize) -> bool {
        let opacities = self.opacity_attribute_data.get();
        let Some(range) = offset.checked_add(count).and_then(|end| opacities.get(offset..end)) else {
            return true;
        };
        // Zero is an unplaced symbol at zero opacity. A placed symbol whose
        // fade-in starts at zero, and one still fading out, are not zero.
        range.iter().any(|vertex| vertex.a1[0] != 0.0)
    }

    /// Whether the draw of `segment` (one segment, or `None` for the
    /// whole buffer) has any vertex that can draw.
    pub(crate) fn segment_has_visible_vertices(&self, segment: Option<usize>) -> bool {
        match (&self.visible, segment) {
            (Some(visible), Some(i)) => visible.segments.get(i).copied().unwrap_or(true),
            (Some(visible), None) => visible.any,
            (None, segment) => self.finds_visible_vertices(segment),
        }
    }

    /// Notes what of the buffer can draw at the opacities placement just
    /// wrote, which [`segment_has_visible_vertices`](Self::segment_has_visible_vertices)
    /// then reads.
    pub(crate) fn opacities_written(&mut self) {
        let segments = (0..self.triangles.segments().len()).map(|i| self.finds_visible_vertices(Some(i))).collect();
        let any = self.finds_visible_vertices(None);
        self.visible = Some(Visible { segments, any });
    }

    /// [`segment_has_visible_vertices`](Self::segment_has_visible_vertices),
    /// from the opacities.
    fn finds_visible_vertices(&self, segment: Option<usize>) -> bool {
        match segment {
            Some(i) => self
                .triangles
                .segments()
                .get(i)
                .is_none_or(|segment| self.has_visible_vertices(segment.vertex_offset, segment.vertex_length)),
            None => self.has_visible_vertices(0, self.attribute_data.len()),
        }
    }
}

/// The vertices and indices of a symbol bucket's collision boxes or circles.
#[derive(Clone, Debug, Default, mln_wire::Wire)]
pub(crate) struct CollisionBuffer {
    pub(crate) vertices: GpuVec<CollisionBoxLayoutVertex>,
    /// Whether each box shows and where it moved, which placement writes.
    pub(crate) dynamic_vertices: GpuVec<CollisionBoxDynamicVertex>,
    /// Lines of the boxes' outlines, or triangles of the circles.
    pub(crate) indices: Indexes,
}

impl CollisionBuffer {
    /// Whether it has anything to draw.
    pub(crate) fn has_data(&self) -> bool {
        !self.indices.is_empty()
    }
}

/// The state of a `SymbolBucket` that placement, cross-tile indexing and
/// sorting change on the render thread.
#[derive(Clone, Debug, Default, mln_wire::Wire)]
pub(crate) struct SymbolBucketState {
    pub(crate) sorted_angle: f32,
    // Set and used by placement.
    pub(crate) just_reloaded: bool,
    pub(crate) has_variable_placement: bool,
    pub(crate) has_uninitialized_symbols: bool,
    /// Counts the times the cross-tile index changed the ids of the
    /// bucket's symbols, which placement keys its symbols by.
    pub(crate) cross_tile_ids_revision: u32,
    /// The tile-unit translations the leader's dynamic positions carry,
    /// which layers sharing the bucket compare.
    pub(crate) dynamic_text_translation: Vec2,
    pub(crate) dynamic_icon_translation: Vec2,

    pub(crate) symbol_instances: Vec<SymbolInstance>,
    pub(crate) symbol_instance_pools: SymbolInstancePools,

    pub(crate) text: Buffer,
    pub(crate) icon: Buffer,
    pub(crate) sdf_icon: Buffer,

    pub(crate) icon_collision_box: Option<CollisionBuffer>,
    pub(crate) text_collision_box: Option<CollisionBuffer>,
    pub(crate) icon_collision_circle: Option<CollisionBuffer>,
    pub(crate) text_collision_circle: Option<CollisionBuffer>,

    /// The bucket's features in the order it last drew them, where it
    /// sorts its symbols by viewport y. Only placement sorts them, so the
    /// worker codec doesn't carry it.
    #[wire(skip)]
    pub(crate) feature_sort_order: Option<Arc<[u32]>>,
}

impl SymbolBucketState {
    pub(crate) fn has_layer_dynamic_attribute_data(&self, layer_id: &str) -> bool {
        self.text.layer_dynamic_attribute_data.contains_key(layer_id)
            || self.icon.layer_dynamic_attribute_data.contains_key(layer_id)
            || self.sdf_icon.layer_dynamic_attribute_data.contains_key(layer_id)
    }
}

/// What layout found of a bucket's symbols that drawing and placement read.
#[derive(Clone, Debug, Default, mln_wire::Wire)]
pub(crate) struct SymbolLayoutFlags {
    pub(crate) icons_need_linear: bool,
    pub(crate) sort_features_by_y: bool,
    pub(crate) icons_in_text: bool,
    pub(crate) allow_vertical_placement: bool,
    pub(crate) placement_modes: Vec<TextWritingModeType>,
}

#[derive(Clone, mln_wire::Wire)]
pub struct SymbolBucket {
    pub(crate) layout: Arc<SymbolLayoutProperties::PossiblyEvaluated>,
    pub(crate) bucket_leader_id: Arc<str>,
    pub(crate) flags: SymbolLayoutFlags,

    pub(crate) sort_key_ranges: Vec<SortKeyRange>,

    pub(crate) icon_binders: LayerBinders<SymbolIconBinders>,
    pub(crate) text_binders: LayerBinders<SymbolTextBinders>,

    pub(crate) text_size_binder: SymbolSizeBinder,
    pub(crate) icon_size_binder: SymbolSizeBinder,

    pub(crate) tile_pixel_ratio: f32,
    /// The bucket's [`Bucket::max_elevation`], in meters.
    pub(crate) max_height_offset: f32,
    /// The id by which the cross-tile index and placement tell symbol
    /// buckets apart, which the index gives the bucket the first time it
    /// indexes it; 0 until then.
    #[wire(skip)]
    pub(crate) bucket_instance_id: u32,

    /// What placement, cross-tile indexing and sorting change.
    pub(crate) state: SymbolBucketState,
}

impl std::fmt::Debug for SymbolBucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SymbolBucket").field("bucket_leader_id", &self.bucket_leader_id).finish_non_exhaustive()
    }
}

impl SymbolBucket {
    pub(crate) fn has_text_data(&self) -> bool {
        !self.state.text.triangles.is_empty()
    }
    pub(crate) fn has_icon_data(&self) -> bool {
        !self.state.icon.triangles.is_empty()
    }
    pub(crate) fn has_sdf_icon_data(&self) -> bool {
        !self.state.sdf_icon.triangles.is_empty()
    }
    pub(crate) fn has_icon_collision_circle_data(&self) -> bool {
        self.state.icon_collision_circle.as_ref().is_some_and(CollisionBuffer::has_data)
    }
    pub(crate) fn has_text_collision_circle_data(&self) -> bool {
        self.state.text_collision_circle.as_ref().is_some_and(CollisionBuffer::has_data)
    }

    /// Whether the bucket has anything to draw: text, icons, or collision
    /// boxes or circles.
    pub(crate) fn has_data(&self) -> bool {
        let state = &self.state;
        let collision = [
            &state.icon_collision_box,
            &state.text_collision_box,
            &state.icon_collision_circle,
            &state.text_collision_circle,
        ];
        self.has_text_data()
            || self.has_icon_data()
            || self.has_sdf_icon_data()
            || collision.into_iter().flatten().any(CollisionBuffer::has_data)
    }

    pub(crate) fn has_variable_text_anchors(&self) -> bool {
        // A value that differs by feature may have offsets for some.
        let has_text_variable_anchor_offset =
            self.layout.text_variable_anchor_offset.constant().is_none_or(|offsets| !offsets.is_empty());
        has_text_variable_anchor_offset || !self.layout.text_variable_anchor.uniform().is_empty()
    }

    /// Sorts the symbols by their position along the screen's y axis at
    /// the map's bearing `angle`, where the layout asks for that.
    pub(crate) fn sort_features(&mut self, angle: f32) {
        if !self.flags.sort_features_by_y {
            return;
        }
        debug_assert!(angle != f32::MAX);

        let state = &mut self.state;
        if state.sorted_angle == angle {
            return;
        }

        state.sorted_angle = angle;

        // The current approach to sorting doesn't sort across text and icon
        // segments so don't try. Sorting within segments separately seemed
        // not to be worth the complexity.
        if [&state.text, &state.icon, &state.sdf_icon].iter().any(|buffer| buffer.triangles.segments().len() > 1) {
            return;
        }

        state.text.triangles.get_mut().clear();
        state.icon.triangles.get_mut().clear();
        state.sdf_icon.triangles.get_mut().clear();

        let mut symbols_sort_order = Vec::with_capacity(state.symbol_instances.len());

        // If the symbols are allowed to overlap sort them by their vertical
        // screen position. The index array buffer is rewritten to reference
        // the (unchanged) vertices in the sorted order.
        let mut order = Vec::new();
        symbols_by_viewport_y(&state.symbol_instances, angle, &mut order);
        for index in order {
            let symbol = &state.symbol_instances[index];
            symbols_sort_order.push(symbol.data_feature_index as u32);
            if let Some(text) = &symbol.text {
                for (run, _) in text.runs.all() {
                    add_placed_symbol(state.text.triangles.get_mut(), &state.text.placed_symbols[run]);
                }
            }
            if let Some(icon) = &symbol.icon {
                let buffer = if icon.sdf { &mut state.sdf_icon } else { &mut state.icon };
                for (run, _) in icon.runs() {
                    add_placed_symbol(buffer.triangles.get_mut(), &buffer.placed_symbols[run]);
                }
            }
        }

        state.feature_sort_order = Some(Arc::from(symbols_sort_order));
    }

    /// The indices into `symbol_instances` of a sort key range, or all
    /// `count` of them if `range` is `None` or invalid.
    #[expect(clippy::unused_self, reason = "the bucket's view of its sort key ranges")]
    pub(crate) fn symbols_in(&self, range: Option<&SortKeyRange>, count: usize) -> std::ops::Range<usize> {
        match range {
            Some(range) if range.start < range.end && range.end <= count => range.start..range.end,
            _ => 0..count,
        }
    }

    /// The layout vertex of `corner` of `quad`, of a symbol at `anchor`
    /// whose size is `size`, `height_offset` meters up.
    #[must_use]
    pub(crate) fn layout_vertex(
        anchor: Vec2,
        quad: &SymbolQuad,
        corner: QuadCorner,
        size: Range<f32>,
        height_offset: f32,
    ) -> SymbolLayoutVertex {
        let pack = |size: f32| MAX_PACKED_SIZE.min((size * f32::from(SIZE_PACK_FACTOR)) as u16);
        let QuadCorner { offset, tex, pixel_offset } = corner;
        SymbolLayoutVertex {
            // The position and the offset share an attribute, as some
            // devices take only eight.
            a1: [
                anchor.x as i16,
                anchor.y as i16,
                // In 1/32 pixels.
                (offset.x * 32.0).round() as i16,
                ((offset.y + quad.glyph_offset.y) * 32.0).round() as i16,
            ],
            a2: [tex[0], tex[1], (pack(size.min) << 1).wrapping_add(u16::from(quad.is_sdf)), pack(size.max)],
            a3: [
                (pixel_offset.x * 16.0) as i16,
                (pixel_offset.y * 16.0) as i16,
                (quad.min_font_scale.x * 256.0) as i16,
                (quad.min_font_scale.y * 256.0) as i16,
            ],
            a4: [quad.tex.x, quad.tex.y],
            a5: height_offset,
        }
    }

    #[must_use]
    pub(crate) fn dynamic_layout_attributes(anchor_point: Vec2, label_angle: f32) -> SymbolDynamicLayoutVertex {
        SymbolDynamicLayoutVertex { a1: [anchor_point.x, anchor_point.y, label_angle] }
    }

    #[must_use]
    pub(crate) fn opacity_attributes(placed: bool, opacity: f32) -> SymbolOpacityVertex {
        SymbolOpacityVertex { a1: [((u32::from((opacity * 127.0) as u8) << 1) | u32::from(placed)) as f32] }
    }

    #[must_use]
    pub(crate) fn collision_layout_vertex(
        a: Vec2,
        anchor: Vec2,
        o: Vec2,
        height_offset: f32,
    ) -> CollisionBoxLayoutVertex {
        CollisionBoxLayoutVertex {
            a1: [a.x as i16, a.y as i16],
            a2: [anchor.x as i16, anchor.y as i16],
            a3: [o.x.round() as i16, o.y.round() as i16],
            a4: height_offset,
        }
    }

    #[must_use]
    pub(crate) fn collision_dynamic_vertex(placed: bool, not_used: bool, shift: Vec2) -> CollisionBoxDynamicVertex {
        CollisionBoxDynamicVertex { a1: [u16::from(placed), u16::from(not_used)], a2: [shift.x, shift.y] }
    }
}

/// Adds the two triangles of each quad of `placed_symbol`.
fn add_placed_symbol(triangles: &mut Vec<u16>, placed_symbol: &PlacedSymbol) {
    let start = placed_symbol.start_index;
    for quad in (start..start + placed_symbol.glyph_offsets.len() * 4).step_by(4) {
        let [a, b, c, d] = [quad, quad + 1, quad + 2, quad + 3].map(|i| i as u16);
        triangles.extend_from_slice(&[a, b, c, b, c, d]);
    }
}

/// Makes `order` the indices of `symbol_instances`, sorted by viewport Y at
/// `angle`, then by descending data feature index. Equal keys keep their
/// index order.
pub(crate) fn symbols_by_viewport_y(symbol_instances: &[SymbolInstance], angle: f32, order: &mut Vec<usize>) {
    order.clear();
    order.extend(0..symbol_instances.len());
    let sin = angle.sin();
    let cos = angle.cos();
    order.sort_by_key(|&i| {
        let s = &symbol_instances[i];
        let p = s.anchor.point;
        ((sin * p.x + cos * p.y).round() as i64, Reverse(s.data_feature_index))
    });
}

impl Bucket for SymbolBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        let state = &mut self.state;
        for buffer in [&mut state.text, &mut state.icon, &mut state.sdf_icon] {
            up.vec(&mut buffer.attribute_data);
            up.keep(&mut buffer.dynamic_attribute_data);
            for positions in buffer.layer_dynamic_attribute_data.values_mut() {
                up.keep(positions);
            }
            up.keep(&mut buffer.opacity_attribute_data);
            up.keep_indexes(&mut buffer.triangles);
        }
        let collision = [
            &mut state.icon_collision_box,
            &mut state.text_collision_box,
            &mut state.icon_collision_circle,
            &mut state.text_collision_circle,
        ];
        for buffer in collision.into_iter().flatten() {
            up.vec(&mut buffer.vertices);
            up.keep(&mut buffer.dynamic_vertices);
            up.indexes(&mut buffer.indices);
        }
        up.binders(&mut self.icon_binders);
        up.binders(&mut self.text_binders);
    }

    fn max_elevation(&self) -> f32 {
        self.max_height_offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // gtest: SymbolRendering.KeepFadingSymbolsVisible
    #[test]
    fn keep_fading_symbols_visible() {
        let mut buffer = Buffer::default();
        // Without placement results, leave rendering enabled.
        assert!(buffer.has_visible_vertices(0, 1));

        buffer.opacity_attribute_data.get_mut().extend([
            SymbolBucket::opacity_attributes(false, 0.0),
            SymbolBucket::opacity_attributes(true, 0.0),
            SymbolBucket::opacity_attributes(false, 0.5),
            SymbolBucket::opacity_attributes(true, 1.0),
        ]);
        assert!(!buffer.has_visible_vertices(0, 1));
        assert!(buffer.has_visible_vertices(1, 1)); // Just starting to fade in.
        assert!(buffer.has_visible_vertices(2, 1)); // Still fading out.
        assert!(buffer.has_visible_vertices(3, 1));
        assert!(buffer.has_visible_vertices(0, 4)); // Mixed visibility in one segment.
        assert!(buffer.has_visible_vertices(4, 1)); // Not placed yet.
        assert!(buffer.has_visible_vertices(usize::MAX, 2));
    }

    #[test]
    fn segment_visibility_reads_the_segment_range() {
        let mut buffer = Buffer::default();
        buffer.opacity_attribute_data.get_mut().extend([
            SymbolBucket::opacity_attributes(false, 0.0),
            SymbolBucket::opacity_attributes(false, 0.0),
            SymbolBucket::opacity_attributes(true, 1.0),
            SymbolBucket::opacity_attributes(true, 1.0),
        ]);
        // Two segments of two vertices each, apart by their sort keys.
        let mut vertices = vec![0u8; 2];
        buffer.triangles.add_keyed(&mut vertices, 0, 0.0, [[0, 1, 1]]);
        vertices.extend([0, 0]);
        buffer.triangles.add_keyed(&mut vertices, 2, 1.0, [[0, 1, 1]]);
        for written in [false, true] {
            if written {
                buffer.opacities_written();
            }
            assert!(!buffer.segment_has_visible_vertices(Some(0)));
            assert!(buffer.segment_has_visible_vertices(Some(1)));
            // An unknown segment draws.
            assert!(buffer.segment_has_visible_vertices(Some(2)));
        }
    }
}
