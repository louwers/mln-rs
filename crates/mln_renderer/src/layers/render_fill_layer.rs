use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::fill_layer_ubo::{
    FillDrawableUBO, FillDrawableUnionUBO, FillEvaluatedPropsUBO, FillOutlineDrawableUBO, FillPatternDrawableUBO,
    FillPatternTilePropsUBO, FillTilePropsUnionUBO,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_FILL_COLOR_VERTEX_ATTRIBUTE, ID_FILL_DRAWABLE_UBO, ID_FILL_EVALUATED_PROPS_UBO, ID_FILL_IMAGE_TEXTURE,
    ID_FILL_OUTLINE_END_ATTRIBUTE, ID_FILL_OUTLINE_POS_ATTRIBUTE, ID_FILL_POS_VERTEX_ATTRIBUTE, ID_FILL_TILE_PROPS_UBO,
    ID_PROJECTION_UBO,
};
use mln_gfx::shaders::shader_program::ShaderProgramPtr;
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode, VertexAttributeArray};
use mln_map_types::TransformState;
use mln_style::PropertyEvaluationParameters;
use mln_style::layer_type::LayerStyle;
use mln_style::layers::Fill;
use mln_style::layers::fill_layer_properties::FillPaintProperties;
use mln_style_types::AlignmentType;
use mln_util::Color;
use mln_util::constants::TILE_SIZE_D;
use mln_util::error::Error;
use mln_util::identity::SimpleIdentity;
use mln_util::size::Size;

use super::{cast_matrix, refresh_paint_attributes, shared_vertices};
use crate::bucket::Bucket;
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, paint_vertices};
use crate::buckets::fill_bucket::{FillBinders, FillBucket, FillLayoutVertex};
use crate::draw::{DrawGeometry, DrawObjects, copy_shared_attributes};
use crate::gfx::atlas_binding::AtlasBinding;
use crate::gfx::context::Context;
use crate::gfx::indexes::Indexes;
use crate::gfx::texture2d::Texture2D;
use crate::layer_composite::Composite;
use crate::layer_drawing::{DrawDepth, DrawList, DrawObjectsMut, FrameDraw, TileDraws, TileObjects, UniformWriter};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::layout::pattern_layout::{FillLayoutSpec, PatternLayout, uses_pattern};
use crate::paint_parameters::PaintParameters;
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{
    Atlases, LayerParts, LayerSource, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tile::FrameTile;
use crate::tile_matrix::{get_projection_data, to_projection_ubo};

const FILL_SHADER_NAME: &str = "FillShader";
const FILL_OUTLINE_SHADER_NAME: &str = "FillOutlineShader";
const FILL_PATTERN_SHADER_NAME: &str = "FillPatternShader";
const FILL_OUTLINE_PATTERN_SHADER_NAME: &str = "FillOutlinePatternShader";
const FILL_OUTLINE_INSTANCED_SHADER_NAME: &str = "FillOutlineInstancedShader";
const FILL_OUTLINE_PATTERN_INSTANCED_SHADER_NAME: &str = "FillOutlinePatternInstancedShader";

/// The outlines' line width, which the style spec doesn't give. GL JS sets
/// none, so it draws 1 px; MapLibre Native draws 2 px on OpenGL. The outline
/// shaders fade the pixels on both sides of a fill's edge by their distance
/// from it, and the render tests' expected images have both. A device that
/// draws no lines this wide draws each segment of an outline as an instance
/// of a quad, which the instanced outline shaders lay over the pixels such a
/// line covers. One that rasterizes on the CPU, where an instance costs about
/// as much as a draw, draws its 1 px lines instead.
const LINE_WIDTH: f32 = 2.0;

/// Whether the layer draws the outlines as instanced quads, which read the
/// buckets' outline ends, rather than as lines (`LINE_WIDTH`).
pub(crate) fn draws_outline_quads(context: &Context) -> bool {
    let caps = context.caps();
    caps.max_line_width < LINE_WIDTH && !caps.cpu_rasterizer
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
}

impl FillDraw {
    /// A draw of `geometry` with `shader` and the layer's color and cull
    /// modes, which binds the textures of `atlases`.
    fn new(
        shader: ShaderProgramPtr,
        sub_layer: u8,
        depth: DepthMaskType,
        geometry: DrawGeometry,
        atlases: Option<&AtlasBinding>,
    ) -> FillDraw {
        let textures = atlases.map(|atlases| atlases.textures(&shader)).unwrap_or_default();
        let objects =
            DrawObjects::new(shader, ColorMode::alpha_blended(), CullFaceMode::disabled(), geometry, textures);
        FillDraw { id: SimpleIdentity::new(), objects, sub_layer, depth }
    }
}

/// The geometry of `vertex_count` vertices in `vertices`, and the `segments`
/// of `indexes` drawn as `mode`, without instances; with no segments, it
/// draws nothing and has no vertices.
fn fill_geometry(
    vertices: VertexAttributeArray,
    vertex_count: usize,
    mode: DrawMode,
    indexes: Indexes,
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
    vertices: VertexVectorBasePtr,
    indexes: Indexes,
    attributes: VertexAttributeArray,
}

impl OutlineQuad {
    /// The quad, whose corners every instanced outline reads per vertex.
    fn new() -> OutlineQuad {
        let vertices = shared_vertices(&RenderStaticData::fill_outline_vertices());
        let indexes = Indexes::new(RenderStaticData::quad_triangle_indices());
        let mut attributes = VertexAttributeArray::new();
        if let Some(attr) = attributes.set(ID_FILL_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            let stride = size_of::<[i16; 2]>() as u32;
            attr.set_shared_raw_data(Some(Rc::clone(&vertices)), 0, 0, stride, AttributeDataType::Short2);
        }
        OutlineQuad { vertices, indexes, attributes }
    }
}

/// What building the draws of an update's tiles reads of the layer.
struct TileBuilder<'a> {
    /// The shaders of the fills and the outlines, with or without a
    /// pattern.
    fill_shaders: &'a ShaderGroupPtr,
    outline_shaders: &'a ShaderGroupPtr,
    /// Whether the layer fills with a pattern.
    pattern: bool,
    /// Whether the layer's outline color is its fill color.
    outline_is_fill_color: bool,
    projection_variant: ProjectionVariant,
    /// The quad of the instanced outlines, on a device that draws them.
    outline_quad: Option<&'a OutlineQuad>,
    /// The depth mask of the fills without a pattern, which the first tile
    /// they are built for sets.
    fill_depth: Option<DepthMaskType>,
}

impl TileBuilder<'_> {
    /// The draws of `tile`'s `bucket`, laid out with `evaluated`, if it has
    /// any to draw.
    fn build(
        &mut self,
        tile: FrameTile<'_, Fill>,
        bucket: &FillBucket,
        binders: &FillBinders,
        evaluated: &FillPaintProperties::PossiblyEvaluated,
    ) -> Option<FillTile> {
        let geometry = tile.bucket_geometry();
        let mut properties_as_uniforms = StringIDSetsPair::default();
        let paint = paint_vertices(geometry, &binders.interleaved_vertex_buffer, binders.depends_on_feature_state());
        let mut vertex_attrs = VertexAttributeArray::new();
        read_fill_attributes(&mut vertex_attrs, binders, &paint, evaluated, &mut properties_as_uniforms);

        let fill_vertex_count = bucket.vertices.len();
        if let Some(attr) = vertex_attrs.set(ID_FILL_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(
                Some(bucket_vertices(geometry, &bucket.vertices)),
                0, // offsetof(FillLayoutVertex, a1)
                /*vertexOffset=*/ 0,
                size_of::<FillLayoutVertex>() as u32,
                AttributeDataType::Short2,
            );
        }

        // The outline draws in the fill color unless it has one of its own,
        // which a pattern doesn't default to.
        let do_outline = *evaluated.fill_antialias.uniform() && (!self.pattern || self.outline_is_fill_color);
        let fill_shader = self.fill_shaders.shader(&properties_as_uniforms, self.projection_variant);
        let outline_shader =
            do_outline.then(|| self.outline_shaders.shader(&properties_as_uniforms, self.projection_variant));

        let (fill_depth, outline_sub_layer) = if self.pattern {
            (DepthMaskType::ReadWrite, 2)
        } else {
            // Only opaque fills write depth, as `fillRenderPass` does in
            // legacy rendering.
            let opaque = evaluated.fill_color.constant_or(&Color::default()).a >= 1.0
                && evaluated.fill_opacity.constant_or(&0.0) >= 1.0;
            let depth = if opaque { DepthMaskType::ReadWrite } else { DepthMaskType::ReadOnly };
            (*self.fill_depth.get_or_insert(depth), if self.outline_is_fill_color { 2 } else { 0 })
        };
        if fill_vertex_count == 0 {
            return None;
        }

        let atlases = tile.atlas_textures().filter(|atlases| self.pattern && atlases.icon().is_some());
        let atlases = atlases.map(|atlases| {
            AtlasBinding::new(
                Some(atlases),
                None,
                Some(ID_FILL_IMAGE_TEXTURE),
                /*is_text*/ false,
                /*sdf_icons*/ true, // to force linear filter
                AlignmentType::Auto,
                /*icon_scaled*/ false,
                /*text_size_is_zoom_constant*/ false,
            )
        });

        let fill = (!bucket.triangles.is_empty()).then(|| {
            // The fill and a line outline read the same attributes.
            let fill_geometry = fill_geometry(
                copy_shared_attributes(&vertex_attrs),
                fill_vertex_count,
                DrawMode::Triangles(),
                bucket_indexes(geometry, &bucket.triangles),
                &bucket.triangle_segments,
            );
            FillDraw::new(fill_shader, 1, fill_depth, fill_geometry, atlases.as_ref())
        });

        // The bucket's lines, or an instance of the quad for each vertex,
        // from it to its outline end.
        let outline = outline_shader.filter(|_| !bucket.basic_lines.is_empty()).map(|shader| {
            if let Some(quad) = self.outline_quad {
                let stride = size_of::<FillLayoutVertex>() as u32;
                let mut instance_attrs = VertexAttributeArray::new();
                read_fill_attributes(&mut instance_attrs, binders, &paint, evaluated, &mut Default::default());
                for (id, ends) in [
                    (ID_FILL_OUTLINE_POS_ATTRIBUTE, &bucket.vertices),
                    (ID_FILL_OUTLINE_END_ATTRIBUTE, &bucket.outline_ends),
                ] {
                    if let Some(attr) = instance_attrs.set(id, -1, AttributeDataType::Invalid, 0) {
                        let ends = bucket_vertices(geometry, ends);
                        attr.set_shared_raw_data(Some(ends), 0, 0, stride, AttributeDataType::Short2);
                    }
                }
                let quad_geometry = DrawGeometry {
                    instances: Some(instance_attrs),
                    ..fill_geometry(
                        copy_shared_attributes(&quad.attributes),
                        quad.vertices.get_raw_count(),
                        DrawMode::Triangles(),
                        quad.indexes.clone(),
                        &[RenderStaticData::fill_outline_segment(fill_vertex_count)],
                    )
                };
                FillDraw::new(shader, outline_sub_layer, DepthMaskType::ReadOnly, quad_geometry, atlases.as_ref())
            } else {
                let line_geometry = fill_geometry(
                    copy_shared_attributes(&vertex_attrs),
                    fill_vertex_count,
                    DrawMode::Lines(LINE_WIDTH),
                    bucket_indexes(geometry, &bucket.basic_lines),
                    &bucket.basic_line_segments,
                );
                FillDraw::new(shader, outline_sub_layer, DepthMaskType::ReadOnly, line_geometry, atlases.as_ref())
            }
        });

        (fill.is_some() || outline.is_some()).then_some(FillTile { pattern: self.pattern, fill, outline, atlases })
    }
}

fn read_fill_attributes(
    attrs: &mut VertexAttributeArray,
    binders: &FillBinders,
    shared: &VertexVectorBasePtr,
    evaluated: &FillPaintProperties::PossiblyEvaluated,
    pau: &mut StringIDSetsPair,
) {
    let buffer = &binders.interleaved_vertex_buffer;
    let mut id = ID_FILL_COLOR_VERTEX_ATTRIBUTE;
    read_data_driven_paint_property(
        attrs,
        &binders.fill_color,
        buffer,
        shared,
        &["a_color"],
        evaluated.fill_color.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &binders.fill_opacity,
        buffer,
        shared,
        &["a_opacity"],
        evaluated.fill_opacity.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &binders.fill_outline_color,
        buffer,
        shared,
        &["a_outline_color"],
        evaluated.fill_outline_color.is_constant(),
        pau,
        &mut id,
    );
    read_data_driven_paint_property(
        attrs,
        &binders.fill_pattern,
        buffer,
        shared,
        &["a_pattern_to", "a_pattern_from"],
        evaluated.fill_pattern.is_constant(),
        pau,
        &mut id,
    );
}

impl LayerType for Fill {
    type Layout = PatternLayout<FillLayoutSpec>;
    type Bucket = FillBucket;
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

    fill_shader_group: Option<ShaderGroupPtr>,
    /// The outline shaders: the line ones, or the instanced ones on a
    /// device that draws no `LINE_WIDTH` lines and rasterizes on the GPU.
    outline_shader_group: Option<ShaderGroupPtr>,
    pattern_shader_group: Option<ShaderGroupPtr>,
    outline_pattern_shader_group: Option<ShaderGroupPtr>,

    /// The instanced outlines' quad (`RenderStaticData::fill_outline_vertices`),
    /// from the first update on a device that draws them.
    outline_quad: Option<OutlineQuad>,
}

impl RenderFillLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Fill>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderFillLayer {
            base,
            values,
            drawing: TileDraws::default(),
            draw_list: DrawList::default(),
            fill_shader_group: None,
            outline_shader_group: None,
            pattern_shader_group: None,
            outline_pattern_shader_group: None,
            outline_quad: None,
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
        bucket: &FillBucket,
        binders: &FillBinders,
        pattern: bool,
        draw: &FillDraw,
        outline: bool,
        parameters: &PaintParameters<'_>,
    ) -> Option<FillUniforms> {
        const IN_VIEWPORT_PIXEL_UNITS: bool = false; // from RenderTile::translatedMatrix
        const NEAR_CLIPPED: bool = false;
        let evaluated = &layer.paint;
        let zoom = parameters.state.get_zoom() as f32;
        let tile_id = tile.id().to_unwrapped();

        let projection = get_projection_data(
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
                    pad1: 0.0,
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

        // from FillPatternProgram::layoutUniformValues
        let state = parameters.state;
        let int_zoom = state.get_integer_zoom();
        let tile_ratio = 1.0f32 / tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom));
        let tile_size_at_nearest_zoom =
            (TILE_SIZE_D * state.zoom_scale(f64::from(int_zoom) - f64::from(tile_id.canonical.z))) as i32;
        let pixel_x = (f64::from(tile_size_at_nearest_zoom)
            * (f64::from(tile_id.canonical.x)
                + f64::from(tile_id.wrap) * state.zoom_scale(f64::from(tile_id.canonical.z))))
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
        let tlbr = |id: &str| tile.pattern(id).map_or([0.0; 4], |position| position.tlbr().map(f32::from));
        let texture =
            draw.objects.texture(ID_FILL_IMAGE_TEXTURE).and_then(|texture| parameters.context.texture(texture));
        let texture_size = texture.map_or(Size::new(0, 0), Texture2D::size);
        // A pattern has nothing to sample before its tile has an atlas, and
        // the shaders divide by the atlas size.
        if texture_size.width == 0 || texture_size.height == 0 {
            return None;
        }
        uniforms.tile_props.fill_pattern_tile_props_ubo = FillPatternTilePropsUBO {
            pattern_from: tlbr(faded.from.id()),
            pattern_to: tlbr(faded.to.id()),
            texsize: [texture_size.width as f32, texture_size.height as f32],
            sdf: if bucket.is_sdf_pattern(layer_id) { 1.0 } else { 0.0 },
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
        writer.uniform(ID_FILL_EVALUATED_PROPS_UBO, &props);
        writer.storage(ID_FILL_DRAWABLE_UBO, |uniforms| uniforms.draw_ubo);
        writer.storage(ID_FILL_TILE_PROPS_UBO, |uniforms| uniforms.tile_props);
        writer.storage(ID_PROJECTION_UBO, |uniforms| uniforms.projection);
    }
}

impl TypedLayer for RenderFillLayer {
    type Type = Fill;
    type Drawing = TileDraws<FillTile>;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    /// The passes: the translucent one where the layer shows, and the
    /// opaque one too where its fill is opaque, for the layers below the
    /// frame's opaque pass cutoff. An undefined outline color is the fill
    /// color.
    fn passes(
        &mut self,
        _parameters: &PropertyEvaluationParameters,
        evaluated: &mut EvaluatedLayer<Fill>,
    ) -> RenderPass {
        let evaluated = &mut evaluated.paint;
        if self.values.unevaluated.fill_outline_color.is_undefined() {
            evaluated.fill_outline_color = evaluated.fill_color.clone();
        }
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
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        source: Option<LayerSource<'_, Fill>>,
    ) {
        let Some(render_tiles) = source.and_then(LayerSource::tiles) else {
            return;
        };
        let shaders = &paint_parameters.static_data.shaders;

        let outline_quads = draws_outline_quads(paint_parameters.context);
        let pattern = !self.values.unevaluated.fill_pattern.is_undefined();
        let (outline_name, outline_pattern_name) = if outline_quads {
            (FILL_OUTLINE_INSTANCED_SHADER_NAME, FILL_OUTLINE_PATTERN_INSTANCED_SHADER_NAME)
        } else {
            (FILL_OUTLINE_SHADER_NAME, FILL_OUTLINE_PATTERN_SHADER_NAME)
        };
        let (fill_group, fill_name, outline_group, outline_name) = if pattern {
            let outline = &mut self.outline_pattern_shader_group;
            (&mut self.pattern_shader_group, FILL_PATTERN_SHADER_NAME, outline, outline_pattern_name)
        } else {
            (&mut self.fill_shader_group, FILL_SHADER_NAME, &mut self.outline_shader_group, outline_name)
        };
        if fill_group.is_none() {
            *fill_group = shaders.shader_group(fill_name);
        }
        if outline_group.is_none() {
            *outline_group = shaders.shader_group(outline_name);
        }
        let (Some(fill_shaders), Some(outline_shaders)) = (&*fill_group, &*outline_group) else {
            self.remove_all_draws();
            return;
        };

        if outline_quads && self.outline_quad.is_none() {
            self.outline_quad = Some(OutlineQuad::new());
        }

        let mut builder = TileBuilder {
            fill_shaders,
            outline_shaders,
            pattern,
            outline_is_fill_color: self.values.unevaluated.fill_outline_color.is_undefined(),
            projection_variant: self.base.projection_variant,
            outline_quad: self.outline_quad.as_ref().filter(|_| outline_quads),
            fill_depth: None,
        };

        let layer_id = self.base.style.base().id.as_str();
        let layer = &*self.values.evaluated;
        for tile in render_tiles.iter() {
            let tile_id = tile.id();
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

            // The draws' attributes point into the bucket's shared vectors,
            // which upload again when they change.
            let changes = entry.take_changes();
            if let Some(cached) = entry.value.as_mut() {
                let geometry = tile.bucket_geometry();
                let refresh = |attrs: &mut VertexAttributeArray| {
                    let buffer = &binders.interleaved_vertex_buffer;
                    let paint = paint_vertices(geometry, buffer, binders.depends_on_feature_state());
                    read_fill_attributes(attrs, binders, &paint, evaluated, &mut Default::default());
                };
                if let Some(fill) = &mut cached.fill {
                    refresh_paint_attributes(&mut fill.objects, changes, refresh);
                }
                if let Some(outline) = &mut cached.outline {
                    match &cached.fill {
                        // A line outline reads the fill's attributes, and so
                        // binds the buffers the fill uploads.
                        Some(fill) if changes.paint && !outline.objects.has_instances() => {
                            outline.objects.share_vertex_attributes(&fill.objects);
                        }
                        _ => refresh_paint_attributes(&mut outline.objects, changes, refresh),
                    }
                }
            } else if let Some(built) = builder.build(tile, bucket, binders, evaluated) {
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
                    bucket,
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
        frame.set_stencil_tiles(render_tiles.render_tiles().map(|tile| tile.id));
        self.draw_list.finish(frame, paint_parameters.context, |writer| Self::write_uniforms(layer, writer));
    }
}

impl RenderLayer for RenderFillLayer {
    // query
    fn query_intersects_feature(
        &self,
        query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        _feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
        geometry: &mln_tile_id::geometry_tile_data::GeometryCollection,
        _zoom: f32,
        transform_state: &TransformState,
        pixels_to_tile_units: f32,
        _projector: &mln_map_types::TileProjector<'_>,
        _feature_state: &mln_style::expression::expression::FeatureState,
    ) -> bool {
        let evaluated = &self.values.evaluated.paint;
        let translated_query_geometry = crate::geometry::feature_index::FeatureIndex::translate_query_geometry(
            query_geometry,
            *evaluated.fill_translate.uniform(),
            *evaluated.fill_translate_anchor.uniform(),
            transform_state.get_bearing() as f32,
            pixels_to_tile_units,
        );

        mln_util::intersection_tests::polygon_intersects_multi_polygon(
            translated_query_geometry.as_ref().unwrap_or(query_geometry),
            geometry,
        )
    }
}
