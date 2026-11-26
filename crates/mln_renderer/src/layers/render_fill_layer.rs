use std::mem::offset_of;
use std::sync::Arc;

use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::Program;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode};
use mln_style::PropertyEvaluationParameters;
use mln_style::color::Color;
use mln_style::constants::TILE_SIZE_D;
use mln_style::error::Error;
use mln_style::layer::LayerStyle;
use mln_style::layers::Fill;
use mln_style::layers::fill_layer_properties::FillPaintProperties;
use mln_style::types::{AlignmentType, LayerBlendType, TranslateAnchorType};
use mln_util::identity::SimpleIdentity;

use super::{cast_matrix, refresh_paint_attributes};
use crate::bucket::Bucket;
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, copied_vertices, paint_vertices};
use crate::buckets::fill_bucket::{FillBinders, FillBucket, FillLayoutVertex};
use crate::draw::{DrawGeometry, DrawObjects, copy_shared_attributes};
use crate::geometry::feature_index::QueryRegion;
use crate::gfx::atlas_binding::AtlasBinding;
use crate::gfx::context::Context;
use crate::gfx::indexes::{DrawIndexes, Indexes, SharedIndexes};
use crate::gfx::segment::SegmentBase;
use crate::gfx::texture2d::Texture2D;
use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::layer_composite::Composite;
use crate::layer_drawing::{DrawDepth, DrawList, DrawObjectsMut, FrameDraw, TileDraws, TileObjects, UniformWriter};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layers::fill::programs;
use crate::layers::fill::ubo::{
    FillDrawableUBO, FillDrawableUnionUBO, FillEvaluatedPropsUBO, FillOutlineDrawableUBO, FillPatternDrawableUBO,
    FillPatternTilePropsUBO, FillTilePropsUnionUBO,
};
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::layout::pattern_layout::{FillLayoutSpec, PatternLayout, uses_pattern};
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    Atlases, LayerBucket, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tile::FrameTile;
use crate::tile_matrix::{projection_data, to_projection_ubo};

/// The ids of the buffers that the draws bind.
const DRAWABLE_ID: usize = programs::FILL.buffer_id("drawableVector");
const TILE_PROPS_ID: usize = programs::FILL_PATTERN.buffer_id("tilePropsVector");
const PROPS_ID: usize = programs::FILL.buffer_id("props");
const OUTLINE_VERTICES_ID: usize = programs::FILL_OUTLINE_PULLED.buffer_id("outlineVertices");
const OUTLINE_ENDS_ID: usize = programs::FILL_OUTLINE_PULLED.buffer_id("outlineEnds");
const OUTLINE_PAINT_ID: usize = programs::FILL_OUTLINE_PULLED.buffer_id("outlinePaint");

/// The slots of the programs' textures that the draws bind.
const IMAGE_SLOT: usize = programs::FILL_PATTERN.texture_slot("image");

/// The outlines' line width, which the style spec doesn't give. The render
/// tests' expected images have outlines of 1 px and of 2 px; the outline
/// shaders fade the pixels on both sides of a fill's edge by their distance
/// from it, which a 2 px line covers. A device that
/// draws no lines this wide draws each segment of an outline as an instance
/// of a quad, which the instanced outline shaders lay over the pixels such a
/// line covers. One that rasterizes on the CPU, where an instance costs about
/// as much as a draw, draws its 1 px lines instead.
///
/// A device that binds vertex buffers for shaders to read as storage buffers
/// draws the quads without instances, four vertices a segment without
/// attributes: the shader reads the segment's ends and paint values from the
/// buffers the instances would, by the vertex's index. A layer with a
/// pattern draws instances there too.
const LINE_WIDTH: f32 = 2.0;

/// Whether the layer draws the outlines as quads, which read the buckets'
/// outline ends, rather than as lines (`LINE_WIDTH`).
pub(crate) fn draws_outline_quads(context: &Context) -> bool {
    let caps = context.caps();
    caps.max_line_width < LINE_WIDTH && !caps.cpu_rasterizer
}

/// How a layer's outlines draw.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outlines {
    /// The bucket's lines.
    Lines,
    /// Instances of a quad, one per bucket vertex.
    Instanced,
    /// Quads without attributes, whose shader reads the bucket's vertices,
    /// their outline ends and paint values by vertex index.
    Pulled,
}

impl Outlines {
    /// How the outlines of a layer draw on `context`'s device.
    fn of(context: &Context, pattern: bool) -> Outlines {
        if !draws_outline_quads(context) {
            Outlines::Lines
        } else if context.caps().binds_vertex_buffers && !pattern {
            Outlines::Pulled
        } else {
            Outlines::Instanced
        }
    }
}

/// Where the shader of the pulled outlines finds a vertex's paint values in
/// the vertices that `attrs`, the paint attributes, read
/// (`FillOutlineDrawableUBO::paint_layout`).
pub(crate) fn outline_paint_layout(attrs: &VertexAttributeArray) -> u32 {
    let (color, opacity) = ("outline_color", "opacity");
    let shared = |name: &str| attrs.get(name).and_then(|attribute| attribute.shared());
    let words = |bytes: u32| (bytes / 4) & 0xff;
    let offset = |name: &str| shared(name).map_or(0, |attribute| words(attribute.offset));
    let stride = [color, opacity].into_iter().find_map(shared).map_or(0, |attribute| words(attribute.stride));
    stride | offset(color) << 8 | offset(opacity) << 16
}

/// The pass the layer's draws draw in.
const RENDER_PASS: RenderPass = RenderPass::Translucent;

/// A draw the layer keeps of a tile.
struct FillDraw {
    /// The id of the draw's GPU objects, made with them.
    id: SimpleIdentity,
    objects: DrawObjects,
    /// The sub-layer of the layer's depth the draw is at.
    sub_layer: u8,
    depth: DepthMaskType,
    /// Of a pulled outline, where its shader finds a vertex's paint values
    /// (`FillOutlineDrawableUBO::paint_layout`).
    paint_layout: Option<u32>,
}

impl FillDraw {
    /// A draw of `geometry` with `shader` and `color_mode`, without culling,
    /// which binds the textures of `atlases`.
    fn new(
        shader: ShaderKey,
        color_mode: ColorMode,
        sub_layer: u8,
        depth: DepthMaskType,
        geometry: DrawGeometry,
        atlases: Option<&AtlasBinding>,
    ) -> FillDraw {
        let textures = atlases.map(|atlases| atlases.textures(&shader)).unwrap_or_default();
        let objects = DrawObjects::new(shader, color_mode, CullFaceMode::disabled(), geometry, textures);
        FillDraw { id: SimpleIdentity::new(), objects, sub_layer, depth, paint_layout: None }
    }
}

/// The geometry of `vertex_count` vertices in `vertices`, and the `segments`
/// of `indexes` drawn as `mode`, without instances; with no segments, it
/// draws nothing and has no vertices.
fn fill_geometry(
    vertices: VertexAttributeArray,
    vertex_count: usize,
    mode: DrawMode,
    indexes: DrawIndexes,
    segments: &[SegmentBase],
) -> DrawGeometry {
    DrawGeometry {
        vertices,
        vertex_count: if segments.is_empty() { 0 } else { vertex_count },
        mode,
        indexes,
        segments: segments.iter().map(SegmentBase::copy).collect(),
        instances: None,
    }
}

/// What the layer keeps of a tile: its fill and its outline, as they were
/// built for the tile's bucket.
pub(crate) struct FillTile {
    /// Whether the draws fill with the layer's pattern.
    pattern: bool,
    /// The fill of the bucket's triangles, if it has any.
    fill: Option<FillDraw>,
    /// The outline of the bucket's lines, if the layer outlines and the
    /// bucket has any.
    outline: Option<FillDraw>,
    /// Gives the pattern draws the texture of the tile's icon atlas.
    atlases: Option<AtlasBinding>,
}

/// Which of a tile's draws a frame's draw draws.
#[derive(Clone, Copy)]
pub(crate) enum FillPart {
    Fill,
    Outline,
}

impl TileObjects for FillTile {
    type Part = FillPart;

    fn objects(&mut self, part: FillPart) -> Option<DrawObjectsMut<'_>> {
        let draw = match part {
            FillPart::Fill => &mut self.fill,
            FillPart::Outline => &mut self.outline,
        };
        Some(DrawObjectsMut { objects: &mut draw.as_mut()?.objects, atlases: self.atlases.as_ref() })
    }

    fn drop_bindings(&mut self) {
        for draw in self.fill.iter_mut().chain(&mut self.outline) {
            draw.objects.drop_bindings();
        }
    }
}

/// A draw's entries in the layer's storage buffers.
struct FillUniforms {
    draw_ubo: FillDrawableUnionUBO,
    tile_props: FillTilePropsUnionUBO,
    projection: ProjectionUBO,
}

/// The instanced outlines' quad: its vertices and indexes, and the vertex
/// attributes every instanced outline reads of it.
struct OutlineQuad {
    vertex_count: usize,
    indexes: SharedIndexes,
    attributes: VertexAttributeArray,
}

impl OutlineQuad {
    /// The quad, whose corners every instanced outline reads per vertex, in
    /// `context`'s stores.
    fn new(context: &mut Context) -> OutlineQuad {
        let vertices = copied_vertices(context, &RenderStaticData::fill_outline_vertices());
        let indexes = context.add_indexes(Indexes::new(RenderStaticData::quad_strip_indices()));
        let vertex_count = vertices.count();
        let mut attributes = VertexAttributeArray::new();
        {
            let stride = size_of::<[i16; 2]>() as u32;
            attributes.set_shared("pos", SharedAttribute::new(vertices, AttributeDataType::Short2, 0, stride));
        }
        OutlineQuad { vertex_count, indexes, attributes }
    }
}

/// What building the draws of an update's tiles reads of the layer.
struct TileBuilder<'a> {
    /// The programs of the fills and the outlines, with or without a
    /// pattern.
    fill_program: &'static Program,
    outline_program: &'static Program,
    /// Whether the layer fills with a pattern.
    pattern: bool,
    /// Whether the layer's outline color is its fill color.
    outline_is_fill_color: bool,
    projection_variant: ProjectionVariant,
    /// The quad of the instanced outlines, where the layer draws them.
    outline_quad: Option<&'a OutlineQuad>,
    /// The triangles of the pulled outlines' quads
    /// (`RenderStaticData::wall_indexes`), where the layer draws them.
    pulled_indexes: Option<&'a SharedIndexes>,
    /// The depth mask of the fills without a pattern, which the first tile
    /// they are built for sets.
    fill_depth: Option<DepthMaskType>,
}

impl TileBuilder<'_> {
    /// The draws of `tile`'s `bucket`, laid out with `evaluated`, if it has
    /// any to draw.
    fn build(
        &mut self,
        context: &mut Context,
        tile_bucket: LayerBucket<'_, Fill>,
        binders: &FillBinders,
        evaluated: &FillPaintProperties::PossiblyEvaluated,
    ) -> Option<FillTile> {
        let (bucket, geometry) = (tile_bucket.bucket, tile_bucket.geometry());
        let mut uniforms = Uniforms::default();
        let buffer = &binders.interleaved_vertex_buffer;
        let paint = paint_vertices(context, geometry, buffer, binders.depends_on_feature_state());
        let mut vertex_attrs = VertexAttributeArray::new();
        binders.bind(evaluated, paint.as_ref(), &mut vertex_attrs, &mut uniforms);

        let fill_vertex_count = bucket.vertices.len();
        if let Some(vertices) = bucket_vertices(geometry, &bucket.vertices) {
            vertex_attrs.set_shared(
                "pos",
                SharedAttribute::new(
                    vertices,
                    AttributeDataType::Short2,
                    offset_of!(FillLayoutVertex, a1) as u32,
                    size_of::<FillLayoutVertex>() as u32,
                ),
            );
        }

        // The outline draws in the fill color unless it has one of its own,
        // which a pattern doesn't default to.
        let do_outline = *evaluated.fill_antialias.uniform() && (!self.pattern || self.outline_is_fill_color);
        let color_mode = ColorMode::alpha_blended();
        let fill_shader = context.shader_key(self.fill_program, uniforms, self.projection_variant);
        let outline_shader =
            do_outline.then(|| context.shader_key(self.outline_program, uniforms, self.projection_variant));

        let (fill_depth, outline_sub_layer) = if self.pattern {
            (DepthMaskType::ReadWrite, 2)
        } else {
            // Only opaque fills write depth.
            let opaque = evaluated.fill_color.constant_or(&Color::default()).a >= 1.0
                && evaluated.fill_opacity.constant_or(&0.0) >= 1.0;
            let depth = if opaque { DepthMaskType::ReadWrite } else { DepthMaskType::ReadOnly };
            (*self.fill_depth.get_or_insert(depth), if self.outline_is_fill_color { 2 } else { 0 })
        };
        if fill_vertex_count == 0 {
            return None;
        }

        let atlases = Some(tile_bucket.atlas_textures()).filter(|atlases| self.pattern && atlases.icon().is_some());
        let atlases = atlases.map(|atlases| {
            AtlasBinding::new(
                Some(atlases.clone()),
                None,
                Some(IMAGE_SLOT),
                /*is_text*/ false,
                /*sdf_icons*/ true, // to force linear filter
                AlignmentType::Auto,
                /*icon_scaled*/ false,
                /*text_size_is_zoom_constant*/ false,
            )
        });

        let fill = (!bucket.triangles.is_empty()).then(|| bucket_indexes(geometry, &bucket.triangles)).flatten();
        let fill = fill.map(|indexes| {
            // The fill and a line outline read the same attributes.
            let fill_geometry = fill_geometry(
                copy_shared_attributes(&vertex_attrs),
                fill_vertex_count,
                DrawMode::Triangles(),
                indexes.into(),
                &bucket.triangle_segments,
            );
            FillDraw::new(fill_shader, color_mode, 1, fill_depth, fill_geometry, atlases.as_ref())
        });

        // The bucket's lines, or a quad for each vertex, from it to its
        // outline end.
        let outline = outline_shader.filter(|_| !bucket.basic_lines.is_empty()).and_then(|shader| {
            if let Some(indexes) = self.pulled_indexes {
                let quad_geometry = fill_geometry(
                    VertexAttributeArray::new(),
                    4 * fill_vertex_count,
                    DrawMode::Triangles(),
                    DrawIndexes::Shared(indexes.clone()),
                    &RenderStaticData::fill_extrusion_wall_segments(fill_vertex_count),
                );
                let mut draw = FillDraw::new(
                    shader,
                    color_mode,
                    outline_sub_layer,
                    DepthMaskType::ReadOnly,
                    quad_geometry,
                    atlases.as_ref(),
                );
                for (id, vertices) in [(OUTLINE_VERTICES_ID, &bucket.vertices), (OUTLINE_ENDS_ID, &bucket.outline_ends)]
                {
                    draw.objects.read_as_storage(id, Some(bucket_vertices(geometry, vertices)?));
                }
                draw.objects.read_as_storage(OUTLINE_PAINT_ID, paint.clone());
                draw.paint_layout = Some(outline_paint_layout(&vertex_attrs));
                Some(draw)
            } else if let Some(quad) = self.outline_quad {
                let stride = size_of::<FillLayoutVertex>() as u32;
                let mut instance_attrs = VertexAttributeArray::new();
                binders.bind(evaluated, paint.as_ref(), &mut instance_attrs, &mut Uniforms::default());
                for (name, ends) in [("outline_pos", &bucket.vertices), ("outline_end", &bucket.outline_ends)] {
                    if let Some(ends) = bucket_vertices(geometry, ends) {
                        instance_attrs
                            .set_shared(name, SharedAttribute::new(ends, AttributeDataType::Short2, 0, stride));
                    }
                }
                let quad_geometry = DrawGeometry {
                    instances: Some(instance_attrs),
                    ..fill_geometry(
                        copy_shared_attributes(&quad.attributes),
                        quad.vertex_count,
                        DrawMode::TriangleStrip(),
                        DrawIndexes::Shared(quad.indexes.clone()),
                        &[RenderStaticData::fill_outline_segment(fill_vertex_count)],
                    )
                };
                Some(FillDraw::new(
                    shader,
                    color_mode,
                    outline_sub_layer,
                    DepthMaskType::ReadOnly,
                    quad_geometry,
                    atlases.as_ref(),
                ))
            } else {
                let line_geometry = fill_geometry(
                    copy_shared_attributes(&vertex_attrs),
                    fill_vertex_count,
                    DrawMode::Lines(LINE_WIDTH),
                    bucket_indexes(geometry, &bucket.basic_lines)?.into(),
                    &bucket.basic_line_segments,
                );
                Some(FillDraw::new(
                    shader,
                    color_mode,
                    outline_sub_layer,
                    DepthMaskType::ReadOnly,
                    line_geometry,
                    atlases.as_ref(),
                ))
            }
        });

        (fill.is_some() || outline.is_some()).then_some(FillTile { pattern: self.pattern, fill, outline, atlases })
    }
}

impl LayerType for Fill {
    type Layout = PatternLayout<FillLayoutSpec>;
    type Bucket = FillBucket;
    type TileBucket<'a> = LayerBucket<'a, Fill>;
    const CROSSFADES: bool = true;

    /// Whether a layer of the group has a pattern.
    fn layout_can_need_dependencies(group: &[Arc<EvaluatedLayer<Fill>>]) -> bool {
        uses_pattern::<FillLayoutSpec>(group)
    }

    fn create_layout(
        group: &[Arc<EvaluatedLayer<Fill>>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<PatternLayout<FillLayoutSpec>>, Error> {
        PatternLayout::new(parameters, group, source_layer).map(Some)
    }

    fn query_radius(_bucket: &FillBucket, layer: &EvaluatedLayer<Fill>) -> f32 {
        let translate = layer.paint.fill_translate.uniform();
        (translate[0] * translate[0] + translate[1] * translate[1]).sqrt()
    }

    fn render_layer(style: Arc<LayerStyle<Fill>>) -> Box<dyn RenderLayer> {
        Box::new(RenderFillLayer::new(style))
    }
}

pub(crate) struct RenderFillLayer {
    base: RenderLayerBase,
    values: LayerValues<Fill>,
    drawing: TileDraws<FillTile>,
    /// The frame's draws as the update makes them.
    draw_list: DrawList<FillUniforms, FillPart>,

    /// The instanced outlines' quad (`RenderStaticData::fill_outline_vertices`),
    /// from the first update that draws them.
    outline_quad: Option<OutlineQuad>,
    /// The triangles of the pulled outlines' quads
    /// (`RenderStaticData::wall_indexes`), from the first update that draws
    /// them.
    pulled_indexes: Option<SharedIndexes>,
}

impl RenderFillLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Fill>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderFillLayer {
            base,
            values,
            drawing: TileDraws::default(),
            draw_list: DrawList::default(),
            outline_quad: None,
            pulled_indexes: None,
        }
    }

    /// The entries in the layer's storage buffers of `draw`, the fill or the
    /// `outline` of `tile`, with or without a `pattern`, of the layer
    /// `layer_id` with the values `layer`: its matrix and the interpolation
    /// of the layer's values, and for a pattern, where the pattern is in the
    /// tile's atlas. `None` for a pattern while the tile has no atlas.
    #[expect(
        clippy::too_many_arguments,
        reason = "the layer, the draw, its tile and bucket, and the frame's parameters"
    )]
    fn uniforms(
        layer_id: &str,
        layer: &EvaluatedLayer<Fill>,
        tile: FrameTile<'_, Fill>,
        tile_bucket: LayerBucket<'_, Fill>,
        binders: &FillBinders,
        pattern: bool,
        draw: &FillDraw,
        outline: bool,
        parameters: &PaintParameters<'_>,
    ) -> Option<FillUniforms> {
        const IN_VIEWPORT_PIXEL_UNITS: bool = false;
        const NEAR_CLIPPED: bool = false;
        let evaluated = &layer.paint;
        let zoom = parameters.view.zoom() as f32;
        let tile_id = tile.draw_id(parameters).to_unwrapped();

        let projection = projection_data(
            &tile_id,
            parameters,
            *evaluated.fill_translate.uniform(),
            *evaluated.fill_translate_anchor.uniform(),
            NEAR_CLIPPED,
            IN_VIEWPORT_PIXEL_UNITS,
            None,
            false,
        );
        let matrix = cast_matrix(&projection.fallback_matrix);
        let mut uniforms = FillUniforms {
            draw_ubo: FillDrawableUnionUBO::default(),
            tile_props: FillTilePropsUnionUBO::default(),
            projection: to_projection_ubo(&projection),
        };

        if !pattern {
            if outline {
                uniforms.draw_ubo.set_fill_outline(&FillOutlineDrawableUBO {
                    matrix,
                    outline_color_t: binders.fill_outline_color.interpolation_factor(zoom),
                    opacity_t: binders.fill_opacity.interpolation_factor(zoom),
                    paint_layout: draw.paint_layout.unwrap_or(0),
                    pad2: 0.0,
                });
            } else {
                uniforms.draw_ubo.set_fill(&FillDrawableUBO {
                    matrix,
                    color_t: binders.fill_color.interpolation_factor(zoom),
                    opacity_t: binders.fill_opacity.interpolation_factor(zoom),
                    pad1: 0.0,
                    pad2: 0.0,
                });
            }
            return Some(uniforms);
        }

        // Where the tile is in pixels at the nearest integer zoom, which the
        // pattern is laid out over.
        let state = parameters.view;
        let int_zoom = state.integer_zoom();
        let tile_ratio = 1.0f32 / tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom));
        let tile_size_at_nearest_zoom =
            (TILE_SIZE_D * mln_map_types::zoom_scale(f64::from(int_zoom) - f64::from(tile_id.canonical.z))) as i32;
        let pixel_x = (f64::from(tile_size_at_nearest_zoom)
            * (f64::from(tile_id.canonical.x)
                + f64::from(tile_id.wrap) * mln_map_types::zoom_scale(f64::from(tile_id.canonical.z))))
            as i32;
        let pixel_y = tile_size_at_nearest_zoom.wrapping_mul(tile_id.canonical.y as i32);
        uniforms.draw_ubo.set_fill_pattern(&FillPatternDrawableUBO {
            matrix,
            pixel_coord_upper: [(pixel_x >> 16) as f32, (pixel_y >> 16) as f32],
            pixel_coord_lower: [(pixel_x & 0xFFFF) as f32, (pixel_y & 0xFFFF) as f32],
            tile_ratio,
            pattern_from_t: binders.fill_pattern.interpolation_factor(zoom).0,
            pattern_to_t: binders.fill_pattern.interpolation_factor(zoom).0,
            opacity_t: binders.fill_opacity.interpolation_factor(zoom),
            color_t: binders.fill_color.interpolation_factor(zoom),
            pad1: 0.0,
            pad2: 0.0,
            pad3: 0.0,
        });

        let faded = evaluated.fill_pattern.faded();
        let tlbr = |id: &str| tile_bucket.pattern(id).map_or([0.0; 4], |position| position.tlbr().map(f32::from));
        let texture = draw.objects.texture(IMAGE_SLOT).and_then(|texture| parameters.context.texture(texture));
        let texture_size = texture.map(Texture2D::size).unwrap_or_default();
        // A pattern has nothing to sample before its tile has an atlas, and
        // the shaders divide by the atlas size.
        if texture_size.width == 0 || texture_size.height == 0 {
            return None;
        }
        uniforms.tile_props.fill_pattern_tile_props_ubo = FillPatternTilePropsUBO {
            pattern_from: tlbr(faded.from.id()),
            pattern_to: tlbr(faded.to.id()),
            texsize: [texture_size.width as f32, texture_size.height as f32],
            sdf: if tile_bucket.bucket.is_sdf_pattern(layer_id) { 1.0 } else { 0.0 },
            pad2: 0.0,
        };
        Some(uniforms)
    }

    /// Writes the layer's buffers of the frame's draws.
    fn write_uniforms(layer: &EvaluatedLayer<Fill>, writer: &mut UniformWriter<'_, FillUniforms>) {
        let evaluated = &layer.paint;
        let props = FillEvaluatedPropsUBO {
            color: evaluated.fill_color.constant_or(&FillPaintProperties::Defaults::fill_color()).to_float_array(),
            outline_color: evaluated
                .fill_outline_color
                .constant_or(&FillPaintProperties::Defaults::fill_outline_color())
                .to_float_array(),
            opacity: evaluated.fill_opacity.constant_or(&FillPaintProperties::Defaults::fill_opacity()),
            fade: layer.crossfade.t,
            from_scale: layer.crossfade.from_scale,
            to_scale: layer.crossfade.to_scale,
        };
        writer.uniform(PROPS_ID, &props);
        writer.storage(DRAWABLE_ID, |uniforms| uniforms.draw_ubo);
        writer.storage(TILE_PROPS_ID, |uniforms| uniforms.tile_props);
        writer.storage(PROJECTION_ID, |uniforms| uniforms.projection);
    }
}

impl TypedLayer for RenderFillLayer {
    type Type = Fill;
    type Drawing = TileDraws<FillTile>;
    const DRAPES: bool = true;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    /// An undefined outline color is the fill color.
    fn complete_evaluation(
        &mut self,
        _parameters: &PropertyEvaluationParameters,
        evaluated: &mut EvaluatedLayer<Fill>,
    ) {
        let evaluated = &mut evaluated.paint;
        if self.values.unevaluated.fill_outline_color.is_undefined() {
            evaluated.fill_outline_color = evaluated.fill_color.clone();
        }
    }

    fn drapes_by_bearing(&self, evaluated: &EvaluatedLayer<Fill>) -> bool {
        let paint = &evaluated.paint;
        *paint.fill_translate_anchor.uniform() == TranslateAnchorType::Viewport
            && *paint.fill_translate.uniform() != [0.0, 0.0]
    }

    fn blends(&self, evaluated: &EvaluatedLayer<Fill>) -> bool {
        *evaluated.paint.fill_layer_blend.uniform() != LayerBlendType::Normal
    }

    fn passes(&self, evaluated: &EvaluatedLayer<Fill>) -> RenderPass {
        let evaluated = &evaluated.paint;
        let opaque = self.values.unevaluated.fill_pattern.is_undefined()
            && evaluated.fill_color.constant_or(&Color::default()).a >= 1.0
            && evaluated.fill_opacity.constant_or(&0.0) >= 1.0;
        let shows = *evaluated.fill_layer_opacity.uniform() > 0.0;
        if shows && opaque {
            RenderPass::Translucent | RenderPass::Opaque
        } else if shows {
            RenderPass::Translucent
        } else {
            RenderPass::None
        }
    }

    /// Makes the frame's draws of the layer, those of each of its tiles,
    /// which it builds for a tile it keeps none of, and writes the layer's
    /// buffers they read.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        render_tiles: &[FrameTile<'_, Fill>],
    ) {
        let pattern = !self.values.unevaluated.fill_pattern.is_undefined();
        let outlines = Outlines::of(paint_parameters.context, pattern);
        // The outlines draw as lines, or as quads on a device that draws no
        // `LINE_WIDTH` lines and rasterizes on the GPU.
        let (fill_program, outline_program) = match (pattern, outlines) {
            (false, Outlines::Lines) => (&programs::FILL, &programs::FILL_OUTLINE),
            (false, Outlines::Instanced) => (&programs::FILL, &programs::FILL_OUTLINE_INSTANCED),
            (false, Outlines::Pulled) => (&programs::FILL, &programs::FILL_OUTLINE_PULLED),
            (true, Outlines::Lines) => (&programs::FILL_PATTERN, &programs::FILL_OUTLINE_PATTERN),
            (true, Outlines::Instanced | Outlines::Pulled) => {
                (&programs::FILL_PATTERN, &programs::FILL_OUTLINE_PATTERN_INSTANCED)
            }
        };

        if outlines == Outlines::Instanced && self.outline_quad.is_none() {
            self.outline_quad = Some(OutlineQuad::new(paint_parameters.context));
        }
        if outlines == Outlines::Pulled && self.pulled_indexes.is_none() {
            self.pulled_indexes = Some(paint_parameters.static_data.wall_indexes(paint_parameters.context));
        }

        let mut builder = TileBuilder {
            fill_program,
            outline_program,
            pattern,
            outline_is_fill_color: self.values.unevaluated.fill_outline_color.is_undefined(),
            projection_variant: self.base.projection_variant,
            outline_quad: self.outline_quad.as_ref().filter(|_| outlines == Outlines::Instanced),
            pulled_indexes: self.pulled_indexes.as_ref().filter(|_| outlines == Outlines::Pulled),
            fill_depth: None,
        };

        let layer_id = self.base.style.base().id.as_str();
        let layer = &*self.values.evaluated;
        for &tile in render_tiles {
            let tile_id = tile.draw_id(paint_parameters);
            let Some(mut entry) = self.drawing.tiles_mut().entry(&tile_id) else {
                continue;
            };

            let render_data = tile.bucket_for_pass(RENDER_PASS);
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
                *entry.value = None;
                continue;
            };
            let bucket = render_data.bucket;
            let Some(binders) = bucket.paint_property_binders.get(layer_id) else {
                continue;
            };

            // Draws built from another bucket go.
            entry.set_bucket(bucket.base.bucket_id);

            let evaluated = &render_data.layer.paint;

            // The draws' attributes read the bucket's vertices in the
            // context's store, which upload again when they change.
            let changes = entry.take_changes();
            let context = &mut *paint_parameters.context;
            if let Some(cached) = entry.value.as_mut() {
                let geometry = render_data.geometry();
                // Reads the paint attributes, and returns the vertices they
                // read.
                let mut refresh = |attrs: &mut VertexAttributeArray| {
                    let buffer = &binders.interleaved_vertex_buffer;
                    let paint = paint_vertices(context, geometry, buffer, binders.depends_on_feature_state());
                    binders.bind(evaluated, paint.as_ref(), attrs, &mut Uniforms::default());
                    paint
                };
                if let Some(fill) = &mut cached.fill {
                    refresh_paint_attributes(&mut fill.objects, changes, |attrs| drop(refresh(attrs)));
                }
                if let Some(outline) = &mut cached.outline {
                    match &cached.fill {
                        // A pulled outline has no attributes: it reads the
                        // paint values' vertices themselves.
                        _ if outline.paint_layout.is_some() => {
                            if changes.paint {
                                let mut attrs = VertexAttributeArray::new();
                                let paint = refresh(&mut attrs);
                                outline.objects.read_as_storage(OUTLINE_PAINT_ID, paint);
                                outline.paint_layout = Some(outline_paint_layout(&attrs));
                            }
                        }
                        // A line outline reads the fill's attributes, and so
                        // binds the buffers the fill uploads.
                        Some(fill) if changes.paint && !outline.objects.has_instances() => {
                            outline.objects.share_vertex_attributes(&fill.objects);
                        }
                        _ => refresh_paint_attributes(&mut outline.objects, changes, |attrs| drop(refresh(attrs))),
                    }
                }
            } else if let Some(built) = builder.build(context, render_data, binders, evaluated) {
                *entry.value = Some(built);
            }

            let Some(cached) = entry.value.as_ref() else {
                continue;
            };
            let tile_draws = [(&cached.fill, FillPart::Fill), (&cached.outline, FillPart::Outline)];
            for (draw, part) in tile_draws {
                let Some(draw) = draw else {
                    continue;
                };
                let Some(uniforms) = Self::uniforms(
                    layer_id,
                    layer,
                    tile,
                    render_data,
                    binders,
                    cached.pattern,
                    draw,
                    matches!(part, FillPart::Outline),
                    paint_parameters,
                ) else {
                    continue;
                };
                let tile_draw = FrameDraw {
                    pass: RENDER_PASS,
                    depth: DrawDepth::Layer { sub_layer: draw.sub_layer, mask: draw.depth },
                    clip: true,
                    ..FrameDraw::new(Some(tile_id), draw.id)
                };
                self.draw_list.push_part(tile_draw, part, uniforms);
            }
        }

        let frame = self.drawing.frame_mut();
        frame.set_composite(Composite::of(
            *layer.paint.fill_layer_opacity.uniform(),
            *layer.paint.fill_layer_blend.uniform(),
        ));
        frame.set_stencil_tiles(render_tiles.iter().map(|tile| tile.draw_id(paint_parameters).to_unwrapped()));
        self.drawing.finish(&mut self.draw_list, gpu, paint_parameters.context, |writer| {
            Self::write_uniforms(layer, writer);
        });
    }
}

impl RenderLayer for RenderFillLayer {
    // query
    fn query_intersects_feature(
        &self,
        region: &QueryRegion<'_>,
        _feature: &dyn mln_style::feature::GeometryTileFeature,
        geometry: &mln_style::feature::GeometryCollection,
        _feature_state: &mln_style::feature::FeatureState,
    ) -> bool {
        let evaluated = &self.values.evaluated.paint;
        let translated_query_geometry = crate::geometry::feature_index::FeatureIndex::translate_query_geometry(
            region.geometry,
            *evaluated.fill_translate.uniform(),
            *evaluated.fill_translate_anchor.uniform(),
            region.view.angle() as f32,
            region.pixels_to_tile_units,
        );

        mln_util::intersection_tests::polygon_intersects_multi_polygon(
            translated_query_geometry.as_ref().unwrap_or(region.geometry),
            geometry,
        )
    }
}
