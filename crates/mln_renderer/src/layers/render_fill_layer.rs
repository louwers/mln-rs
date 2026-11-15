use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::shader_group::{ShaderGroupPtr, StringIDSetsPair};
use mln_gfx::shaders::shader_defines::{
    ID_FILL_COLOR_VERTEX_ATTRIBUTE, ID_FILL_IMAGE_TEXTURE, ID_FILL_OUTLINE_END_ATTRIBUTE,
    ID_FILL_OUTLINE_POS_ATTRIBUTE, ID_FILL_POS_VERTEX_ATTRIBUTE,
};
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{
    AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode, ShaderRegistry, VertexAttributeArray,
    VertexAttributeArrayPtr,
};
use mln_map_types::TransformState;
use mln_style::layer::downcast_arc;
use mln_style::layer_properties::{LayerProperties, get_crossfade, get_evaluated};
use mln_style::layers::fill_layer_impl::Impl as FillLayerImpl;
use mln_style::layers::fill_layer_properties::{FillLayerProperties, FillPaintProperties};
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_util::Color;

use super::fill_layer_tweaker::FillLayerTweaker;
use super::{
    DrawableBinders, DrawableRenderTile, TileDrawables, drawable_has_layer_tweaker, refresh_if_bucket_updated,
    shared_vertices,
};
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, paint_vertices};
use crate::buckets::fill_bucket::{FillBinders, FillBucket, FillLayoutVertex};
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::drawable_builder::{DrawableBuilder, UniqueDrawableBuilder};
use crate::gfx::context::Context;
use crate::layer_group::LayerGroup;
use crate::layer_tweaker::LayerTweakerPtr;
use crate::paint_parameters::{PaintParameters, TransformParameters};
use crate::paint_property_binder::read_data_driven_paint_property;
use crate::render_layer::{
    RenderLayer, RenderLayerBase, Stats, capture_retained_features, vertex_color_alpha, vertex_opacity,
};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tile::RenderTile;
use crate::render_tree::RenderTree;
use crate::update_parameters::UpdateParameters;

const FILL_SHADER_NAME: &str = "FillShader";
const FILL_OUTLINE_SHADER_NAME: &str = "FillOutlineShader";
const FILL_PATTERN_SHADER_NAME: &str = "FillPatternShader";
const FILL_OUTLINE_PATTERN_SHADER_NAME: &str = "FillOutlinePatternShader";
const FILL_OUTLINE_INSTANCED_SHADER_NAME: &str = "FillOutlineInstancedShader";
const FILL_OUTLINE_PATTERN_INSTANCED_SHADER_NAME: &str = "FillOutlinePatternInstancedShader";

/// The outlines' line width, which the style spec doesn't give. GL JS sets
/// none, so it draws 1 px; C++ draws 2 px on OpenGL. The outline shaders
/// fade the pixels on both sides of a fill's edge by their distance from it,
/// and the render tests' expected images have both. A device that draws no
/// lines this wide draws each segment of an outline as an instance of a
/// quad, which the instanced outline shaders lay over the pixels such a line
/// covers. One that rasterizes on the CPU, where an instance costs about as
/// much as a draw, draws its 1 px lines instead.
const LINE_WIDTH: f32 = 2.0;

fn impl_cast(impl_: &Arc<dyn layer::Impl>) -> &FillLayerImpl {
    debug_assert!(std::ptr::eq(impl_.get_type_info(), FillLayerImpl::static_type_info()));
    impl_.downcast_ref::<FillLayerImpl>().expect("RenderFillLayer: not a fill layer impl")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum FillVariant {
    Fill,
    FillPattern,
    FillOutline,
    FillOutlinePattern,
}

impl FillVariant {
    /// The fill variant of a drawable type; `None` for other types, which the
    /// tweaker handles in its fallback arm.
    #[must_use]
    pub(crate) fn from_type(type_: usize) -> Option<FillVariant> {
        match type_ {
            0 => Some(FillVariant::Fill),
            1 => Some(FillVariant::FillPattern),
            2 => Some(FillVariant::FillOutline),
            3 => Some(FillVariant::FillOutlinePattern),
            _ => None,
        }
    }
}

/// Captures the tile's rendered features in `stats`, skipping the features
/// that draw fully transparent.
fn capture_rendered_features(
    stats: &mut Stats,
    bucket: &FillBucket,
    tile: &RenderTile,
    binders: &FillBinders,
    evaluated: &FillPaintProperties::PossiblyEvaluated,
    state: &TransformState,
    transform_params: &TransformParameters,
) {
    let alpha = &evaluated.fill_opacity;
    let alpha_is_constant = alpha.is_constant();
    if alpha_is_constant && binders.fill_opacity.uniform_value(alpha) == 0.0 {
        return;
    }

    let color = &evaluated.fill_color;
    let color_is_constant = color.is_constant();
    if color_is_constant && binders.fill_color.uniform_value(color).a == 0.0 {
        return;
    }

    let zoom_fraction = state.get_zoom_fraction();
    let buffer = &binders.interleaved_vertex_buffer;
    let vertices = bucket.vertices.data();
    capture_retained_features(
        stats,
        bucket,
        tile.get_overscaled_tile_id(),
        state,
        transform_params,
        *evaluated.fill_translate.uniform(),
        *evaluated.fill_translate_anchor.uniform(),
        |vertex_offset| {
            if !alpha_is_constant {
                let opacity = binders.fill_opacity.get_vertex_value(buffer, vertex_offset);
                if opacity.and_then(|v| vertex_opacity(&v, zoom_fraction)) == Some(0.0) {
                    return false;
                }
            }
            if !color_is_constant {
                let color = binders.fill_color.get_vertex_value(buffer, vertex_offset);
                if color.and_then(|v| vertex_color_alpha(&v, zoom_fraction)) == Some(0.0) {
                    return false;
                }
            }
            true
        },
        |view, vertex_offset, vertex_count| {
            view.tile_space_bound(vertex_count, |i| {
                let vertex = vertices.get(vertex_offset.checked_add(i)?)?;
                Some([vertex.a1[0].into(), vertex.a1[1].into(), 0.0])
            })
        },
    );
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

pub(crate) struct RenderFillLayer {
    base: RenderLayerBase,
    // Paint properties
    unevaluated: FillPaintProperties::Unevaluated,

    fill_shader_group: Option<ShaderGroupPtr>,
    /// The outline shaders: the line ones, or the instanced ones on a
    /// device that draws no `LINE_WIDTH` lines and rasterizes on the GPU.
    outline_shader_group: Option<ShaderGroupPtr>,
    pattern_shader_group: Option<ShaderGroupPtr>,
    outline_pattern_shader_group: Option<ShaderGroupPtr>,

    /// The instanced outlines' quad (`RenderStaticData::fill_outline_vertices`).
    outline_quad_vertices: Option<VertexVectorBasePtr>,
    outline_quad_indices: Option<IndexVectorBasePtr>,
}

impl RenderFillLayer {
    pub(crate) fn new(impl_: Arc<FillLayerImpl>) -> Self {
        let unevaluated = impl_.paint.untransitioned();
        let properties: Arc<dyn LayerProperties> = Arc::new(FillLayerProperties::new(impl_));
        let mut base = RenderLayerBase::new(properties);
        base.style_dependencies = unevaluated.get_dependencies();
        RenderFillLayer {
            base,
            unevaluated,
            fill_shader_group: None,
            outline_shader_group: None,
            pattern_shader_group: None,
            outline_pattern_shader_group: None,
            outline_quad_vertices: None,
            outline_quad_indices: None,
        }
    }
}

impl RenderLayer for RenderFillLayer {
    fn base(&self) -> &RenderLayerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        &mut self.base
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.unevaluated);
        self.unevaluated = impl_cast(&self.base.base_impl).paint.transitioned(parameters, prior);
        self.base.style_dependencies = self.unevaluated.get_dependencies();
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let previous_evaluated = get_evaluated::<FillLayerProperties>(&self.base.evaluated_properties).clone();
        let mut properties = FillLayerProperties::with_evaluated(
            downcast_arc::<FillLayerImpl>(&self.base.base_impl),
            parameters.get_crossfade_parameters(),
            self.unevaluated.evaluate_with_previous(parameters, &previous_evaluated),
        );
        let evaluated = &mut properties.evaluated;

        if self.unevaluated.fill_outline_color.is_undefined() {
            evaluated.fill_outline_color = evaluated.fill_color.clone();
        }

        self.base.passes = RenderPass::Translucent;

        if self.unevaluated.fill_pattern.is_undefined()
            && evaluated.fill_color.constant_or(&Color::default()).a >= 1.0
            && evaluated.fill_opacity.constant_or(&0.0) >= 1.0
        {
            // Supply both - evaluated based on opaquePassCutoff in render().
            self.base.passes |= RenderPass::Opaque;
        }
        properties.base.render_passes = self.base.passes.underlying_type();
        self.base.evaluated_properties = Arc::new(properties);

        if let Some(layer_tweaker) = &self.base.layer_tweaker {
            layer_tweaker.borrow_mut().update_properties(Arc::clone(&self.base.evaluated_properties));
        }
    }

    fn has_transition(&self) -> bool {
        self.unevaluated.has_transition()
    }

    fn has_crossfade(&self) -> bool {
        get_crossfade::<FillLayerProperties>(&self.base.evaluated_properties).t != 1.0
    }

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
        let evaluated = get_evaluated::<FillLayerProperties>(&self.base.evaluated_properties);
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

    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &Context,
        state: &TransformState,
        update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        render_tree: &RenderTree,
        changes: &mut UniqueChangeRequestVec,
    ) {
        self.base.stats.rendered_features.clear();

        let Some(render_tiles) = self.base.render_tiles.clone().filter(|t| !t.is_empty()) else {
            self.remove_all_drawables();
            return;
        };

        // Set up a layer group for fill
        if self.base.layer_group.is_none() {
            self.set_layer_group(Some(LayerGroup::tiles(self.base.layer_index)), changes);
        }
        let layer_group = self.base.layer_group.clone().expect("layer group");

        if self.base.layer_tweaker.is_none() {
            let tweaker: LayerTweakerPtr = Rc::new(RefCell::new(FillLayerTweaker::new(
                self.get_id().to_owned(),
                Arc::clone(&self.base.evaluated_properties),
            )));
            self.base.layer_tweaker = Some(Rc::clone(&tweaker));
            changes.push(ChangeRequest::AddLayerTweaker(Rc::clone(&layer_group), Rc::downgrade(&tweaker)));
        }

        self.update_projection_variant(state);

        if self.fill_shader_group.is_none() {
            self.fill_shader_group = shaders.shader_group(FILL_SHADER_NAME);
        }
        let caps = context.caps();
        let outline_quads = caps.max_line_width < LINE_WIDTH && !caps.cpu_rasterizer;
        let (outline_name, outline_pattern_name) = if outline_quads {
            (FILL_OUTLINE_INSTANCED_SHADER_NAME, FILL_OUTLINE_PATTERN_INSTANCED_SHADER_NAME)
        } else {
            (FILL_OUTLINE_SHADER_NAME, FILL_OUTLINE_PATTERN_SHADER_NAME)
        };
        if self.outline_shader_group.is_none() {
            self.outline_shader_group = shaders.shader_group(outline_name);
        }
        if self.pattern_shader_group.is_none() {
            self.pattern_shader_group = shaders.shader_group(FILL_PATTERN_SHADER_NAME);
        }
        if self.outline_pattern_shader_group.is_none() {
            self.outline_pattern_shader_group = shaders.shader_group(outline_pattern_name);
        }
        let (Some(fill_shader_group), Some(outline_shader_group), Some(_), Some(_)) = (
            self.fill_shader_group.clone(),
            self.outline_shader_group.clone(),
            self.pattern_shader_group.clone(),
            self.outline_pattern_shader_group.clone(),
        ) else {
            self.remove_all_drawables();
            return;
        };

        let mut fill_builder: Option<UniqueDrawableBuilder> = None;
        let mut outline_builder: Option<UniqueDrawableBuilder> = None;
        // conformance: fill-pattern
        let mut pattern_builder: Option<UniqueDrawableBuilder> = None;
        let mut outline_pattern_builder: Option<UniqueDrawableBuilder> = None;

        let render_pass = RenderPass::Translucent;

        let common_init = |builder: &mut DrawableBuilder| {
            builder.set_cull_face_mode(CullFaceMode::disabled());
            builder.set_enable_stencil(true);
        };

        {
            let mut tile_layer_group = layer_group.borrow_mut();
            let base = &self.base;
            // If the render pass has changed or the tile has dropped out of
            // the cover set, remove it.
            tile_layer_group.remove_drawables_if(|drawable| {
                drawable.tile_id.is_some_and(|id| !base.render_tile_ids.contains_key(&id))
            });

            tile_layer_group.stencil_tiles = Some(Rc::new(render_tiles.iter().map(|tile| tile.id).collect()));
        }

        // The quad's corners, which every instanced outline reads per vertex.
        let outline_quad = outline_quads.then(|| {
            let vertices = Rc::clone(
                self.outline_quad_vertices
                    .get_or_insert_with(|| shared_vertices(&RenderStaticData::fill_outline_vertices())),
            );
            let indices = Rc::clone(self.outline_quad_indices.get_or_insert_with(|| {
                Rc::new(RefCell::new(IndexVectorBase::new(RenderStaticData::quad_triangle_indices())))
            }));
            let mut attributes = VertexAttributeArray::new();
            if let Some(attr) = attributes.set(ID_FILL_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
                let stride = size_of::<[i16; 2]>() as u32;
                attr.set_shared_raw_data(Some(Rc::clone(&vertices)), 0, 0, stride, AttributeDataType::Short2);
            }
            (vertices, indices, Rc::new(RefCell::new(attributes)))
        });

        let layer_tweaker = self.base.layer_tweaker.clone();
        let mut properties_as_uniforms: StringIDSetsPair = Default::default();
        for tile in render_tiles.iter() {
            let tile_id = *tile.get_overscaled_tile_id();

            let base_impl = Arc::clone(&self.base.base_impl);
            let render_data = super::render_data_for_pass(tile, &*base_impl, render_pass);
            let Some(render_data) = render_data.filter(|r| r.bucket.has_data()) else {
                self.remove_tile(render_pass, &tile_id);
                continue;
            };

            let Some(bucket) = (*render_data.bucket).downcast_ref::<FillBucket>() else {
                continue;
            };
            let Some(binders_lock) = bucket.paint_property_binders.get(self.get_id()) else {
                continue;
            };
            let binders_guard = binders_lock.read();
            let binders = &*binders_guard;

            let prev_bucket_id = self.get_render_tile_bucket_id(&tile_id);
            if prev_bucket_id.is_some_and(|prev| prev != bucket.base.bucket_id) {
                // This tile was previously set up from a different bucket,
                // drop and re-create any drawables for it.
                self.remove_tile(render_pass, &tile_id);
            }
            self.set_render_tile_bucket_id(&tile_id, bucket.base.bucket_id);

            let evaluated = get_evaluated::<FillLayerProperties>(&render_data.layer_properties);
            let geometry = tile.get_bucket_geometry();
            let paint_of = || {
                paint_vertices(geometry, bucket, &binders.interleaved_vertex_buffer, binders.depends_on_feature_state())
            };

            // (getAtlasTweaker: in the pattern branch.)

            if update_parameters.capture_rendered_features {
                let params = &render_tree.get_parameters().transform_params;
                capture_rendered_features(&mut self.base.stats, bucket, tile, binders, evaluated, state, params);
            }

            // Drawables' stored attributes point directly into the bucket's
            // shared vectors; the backend upload guard fires automatically
            // when those change.
            let update_existing = |drawable: &mut crate::drawable::Drawable| {
                let current = drawable_has_layer_tweaker(drawable, layer_tweaker.as_ref());
                if current {
                    refresh_if_bucket_updated(drawable, |attrs| {
                        read_fill_attributes(attrs, binders, &paint_of(), evaluated, &mut Default::default());
                    });
                }
                current
            };
            if self.update_tile(render_pass, &tile_id, update_existing) {
                continue;
            }
            if outline_quad.is_none() {
                // Only the instanced outlines read the outline ends.
                bucket.outline_ends.discard();
            }

            properties_as_uniforms.0.clear();
            properties_as_uniforms.1.clear();

            // `Fill*Program` all use `style::FillPaintProperties`
            let paint = paint_of();
            let vertex_attrs: VertexAttributeArrayPtr = VertexAttributeArray::new_ptr();
            read_fill_attributes(
                &mut vertex_attrs.borrow_mut(),
                binders,
                &paint,
                evaluated,
                &mut properties_as_uniforms,
            );

            // Outline always occurs in translucent pass, defaults to fill
            // color. Outline does not default to fill in the pattern case
            let do_outline = *evaluated.fill_antialias.uniform()
                && (self.unevaluated.fill_pattern.is_undefined() || self.unevaluated.fill_outline_color.is_undefined());

            let fill_vertex_count = bucket.vertices.len();

            if let Some(attr) =
                vertex_attrs.borrow_mut().set(ID_FILL_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0)
            {
                attr.set_shared_raw_data(
                    Some(bucket_vertices(geometry, bucket, &bucket.vertices)),
                    0, // offsetof(FillLayoutVertex, a1)
                    /*vertexOffset=*/ 0,
                    size_of::<FillLayoutVertex>() as u32,
                    AttributeDataType::Short2,
                );
            }

            // The outline: the bucket's lines, or an instance of the quad for
            // each vertex, from it to its outline end.
            let set_outline_geometry = |builder: &mut DrawableBuilder| {
                let stride = size_of::<FillLayoutVertex>() as u32;
                if let Some((quad_vertices, quad_indices, quad_attributes)) = &outline_quad {
                    let mut instance_attrs = VertexAttributeArray::new();
                    read_fill_attributes(&mut instance_attrs, binders, &paint, evaluated, &mut Default::default());
                    for (id, ends) in [
                        (ID_FILL_OUTLINE_POS_ATTRIBUTE, &bucket.vertices),
                        (ID_FILL_OUTLINE_END_ATTRIBUTE, &bucket.outline_ends),
                    ] {
                        if let Some(attr) = instance_attrs.set(id, -1, AttributeDataType::Invalid, 0) {
                            let ends = bucket_vertices(geometry, bucket, ends);
                            attr.set_shared_raw_data(Some(ends), 0, 0, stride, AttributeDataType::Short2);
                        }
                    }
                    builder.set_raw_vertices(Vec::new(), quad_vertices.get_raw_count(), AttributeDataType::Short2);
                    builder.set_vertex_attributes(Some(Rc::clone(quad_attributes)));
                    builder.set_instance_attributes(Some(Rc::new(RefCell::new(instance_attrs))));
                    builder.set_segments_shared(
                        DrawMode::Triangles(),
                        Some(Rc::clone(quad_indices)),
                        &[RenderStaticData::fill_outline_segment(fill_vertex_count)],
                    );
                } else {
                    builder.set_raw_vertices(Vec::new(), fill_vertex_count, AttributeDataType::Short2);
                    builder.set_vertex_attributes(Some(Rc::clone(&vertex_attrs)));
                    builder.set_segments_shared(
                        DrawMode::Lines(LINE_WIDTH),
                        Some(bucket_indexes(geometry, bucket, &bucket.basic_lines)),
                        &bucket.basic_line_segments,
                    );
                }
            };

            let layer_id = self.base.base_impl.id.clone();
            let finish = |builder: &mut DrawableBuilder, type_: FillVariant| {
                builder.flush();

                let mut tile_layer_group = layer_group.borrow_mut();
                for mut drawable in builder.clear_drawables() {
                    drawable.tile_id = Some(tile_id);
                    drawable.type_ = type_ as usize;
                    drawable.layer_tweaker = layer_tweaker.clone();
                    drawable.binders = Some(DrawableBinders::new(&render_data.bucket, &layer_id));
                    drawable.render_tile = DrawableRenderTile::new(render_tiles.owner(), tile);
                    tile_layer_group.add_tile_drawable(render_pass, &tile_id, drawable);
                }
            };

            if self.unevaluated.fill_pattern.is_undefined() {
                // Simple fill
                let fill_shader = fill_shader_group.shader(&properties_as_uniforms, self.base.projection_variant);

                let outline_shader = if do_outline {
                    Some(outline_shader_group.shader(&properties_as_uniforms, self.base.projection_variant))
                } else {
                    None
                };

                if fill_builder.is_none() {
                    let mut builder = DrawableBuilder::boxed();
                    // Only write opaque fills to the depth buffer, matching
                    // `fillRenderPass` in legacy rendering
                    let opaque = evaluated.fill_color.constant_or(&Color::default()).a >= 1.0
                        && evaluated.fill_opacity.constant_or(&0.0) >= 1.0;

                    common_init(builder.as_mut());
                    builder.set_depth_type(if opaque { DepthMaskType::ReadWrite } else { DepthMaskType::ReadOnly });
                    builder.set_color_mode(&ColorMode::alpha_blended());
                    builder.set_sub_layer_index(1);
                    builder.set_render_pass(render_pass);
                    fill_builder = Some(builder);
                }
                if do_outline && outline_builder.is_none() && outline_shader.is_some() {
                    let mut builder = DrawableBuilder::boxed();
                    common_init(builder.as_mut());
                    builder.set_depth_type(DepthMaskType::ReadOnly);
                    builder.set_sub_layer_index(if self.unevaluated.fill_outline_color.is_undefined() { 2 } else { 0 });
                    builder.set_color_mode(&ColorMode::alpha_blended());
                    builder.set_render_pass(RenderPass::Translucent);
                    outline_builder = Some(builder);
                }

                if let Some(builder) = fill_builder.as_mut()
                    && !bucket.triangles.is_empty()
                {
                    builder.set_shader(Some(Rc::clone(&fill_shader)));
                    builder.set_vertex_attributes(Some(Rc::clone(&vertex_attrs)));

                    builder.set_raw_vertices(Vec::new(), fill_vertex_count, AttributeDataType::Short2);
                    builder.set_segments_shared(
                        DrawMode::Triangles(),
                        Some(bucket_indexes(geometry, bucket, &bucket.triangles)),
                        &bucket.triangle_segments,
                    );
                    finish(builder.as_mut(), FillVariant::Fill);
                }

                if do_outline
                    && let Some(builder) = outline_builder.as_mut()
                    && !bucket.basic_lines.is_empty()
                {
                    builder.set_shader(outline_shader.clone());
                    set_outline_geometry(builder.as_mut());
                    finish(builder.as_mut(), FillVariant::FillOutline);
                }
            } else {
                // Fill with pattern (conformance)
                if (render_pass.underlying_type() & RenderPass::Translucent.underlying_type()) == 0 {
                    continue;
                }

                let (Some(pattern_shader_group), Some(outline_pattern_shader_group)) =
                    (self.pattern_shader_group.clone(), self.outline_pattern_shader_group.clone())
                else {
                    continue;
                };

                let fill_shader = pattern_shader_group.shader(&properties_as_uniforms, self.base.projection_variant);
                let outline_shader = if do_outline {
                    Some(outline_pattern_shader_group.shader(&properties_as_uniforms, self.base.projection_variant))
                } else {
                    None
                };

                // getAtlasTweaker()
                let atlas_tweaker = tile.get_atlas_textures().filter(|a| a.icon().is_some()).map(|atlases| {
                    Rc::new(crate::gfx::drawable_atlases_tweaker::DrawableAtlasesTweaker::new(
                        Some(Rc::clone(atlases)),
                        None,
                        Some(ID_FILL_IMAGE_TEXTURE),
                        /*is_text*/ false,
                        /*sdf_icons*/ true, // to force linear filter
                        /*rotation_alignment*/ mln_style_types::AlignmentType::Auto,
                        /*icon_scaled*/ false,
                        /*text_size_is_zoom_constant*/ false,
                    ))
                });

                if pattern_builder.is_none() {
                    let mut builder = DrawableBuilder::boxed();
                    common_init(builder.as_mut());
                    builder.set_shader(Some(Rc::clone(&fill_shader)));
                    builder.set_depth_type(DepthMaskType::ReadWrite);
                    builder.set_color_mode(&ColorMode::alpha_blended());
                    builder.set_sub_layer_index(1);
                    builder.set_render_pass(RenderPass::Translucent);
                    pattern_builder = Some(builder);
                }
                if do_outline && outline_pattern_builder.is_none() {
                    let mut builder = DrawableBuilder::boxed();
                    common_init(builder.as_mut());
                    builder.set_shader(outline_shader.clone());
                    builder.set_depth_type(DepthMaskType::ReadOnly);
                    builder.set_color_mode(&ColorMode::alpha_blended());
                    builder.set_sub_layer_index(2);
                    builder.set_render_pass(RenderPass::Translucent);
                    outline_pattern_builder = Some(builder);
                }

                if let Some(builder) = pattern_builder.as_mut() {
                    builder.set_atlases(atlas_tweaker.clone());
                }
                if do_outline && let Some(builder) = outline_pattern_builder.as_mut() {
                    builder.set_atlases(atlas_tweaker.clone());
                }

                if let Some(builder) = pattern_builder.as_mut()
                    && !bucket.triangles.is_empty()
                {
                    builder.set_shader(Some(Rc::clone(&fill_shader)));
                    builder.set_render_pass(render_pass);
                    builder.set_vertex_attributes(Some(Rc::clone(&vertex_attrs)));
                    builder.set_raw_vertices(Vec::new(), fill_vertex_count, AttributeDataType::Short2);
                    builder.set_segments_shared(
                        DrawMode::Triangles(),
                        Some(bucket_indexes(geometry, bucket, &bucket.triangles)),
                        &bucket.triangle_segments,
                    );

                    finish(builder.as_mut(), FillVariant::FillPattern);
                }

                if do_outline
                    && let Some(builder) = outline_pattern_builder.as_mut()
                    && !bucket.basic_lines.is_empty()
                {
                    builder.set_shader(outline_shader.clone());
                    builder.set_render_pass(render_pass);
                    set_outline_geometry(builder.as_mut());

                    finish(builder.as_mut(), FillVariant::FillOutlinePattern);
                }
            }
        }
    }
}
