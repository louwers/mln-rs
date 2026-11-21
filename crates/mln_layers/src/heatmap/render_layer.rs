// The tiles are drawn (additively, kernel density) into an offscreen
// half-float render target; the layer's draw on the frame colors that
// texture through the color ramp.

use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use glam::dcamera::rh::proj::opengl::orthographic;
use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::program::PROJECTION_BUFFER;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_render::draw::{
    Context, DrawDepth, DrawFrame, DrawGeometry, DrawGroup, DrawIndexes, DrawList, DrawObjects, DrawOrder, FrameDraw,
    FrameDraws, KeptDraw, LayerDrawing, LayerDraws, PaintParameters, RenderStaticData, RenderTarget, SharedAttribute,
    SharedTexture, SharedVertices, Textures, TileMap, UniformWriter, Uniforms, VertexAttributeArray, bucket_indexes,
    bucket_vertices, cast_matrix, copied_vertices, finish_tiles, globe_extrude_scale, paint_vertices, projection_data,
    tile_objects, to_projection_ubo,
};
use mln_render::layer::{
    DrawContext, EvaluateCx, EvaluatedLayer, Evaluation, LayerBucket, LayerType, LayerValues, LayoutParameters,
    OffscreenPass, SourceLayer,
};
use mln_style::StyleValue as _;
use mln_style::expression::{Dependency, GlobalStateMap, global_state_refs_intersect};
use mln_style_types::TranslateAnchorType;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use super::bucket::{HeatmapBinders, HeatmapBucket, HeatmapLayoutVertex};
use super::layout::HeatmapLayout;
use super::programs::{
    DRAWABLE, ID_HEATMAP_COLOR_RAMP_TEXTURE, ID_HEATMAP_IMAGE_TEXTURE, ID_HEATMAP_POS_VERTEX_ATTRIBUTE, PROPS,
    TEXTURE_PROPS,
};
use super::properties::{HeatmapPaintProperties, default_heatmap_color};
use super::ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO, HeatmapTexturePropsUBO};
use super::{Heatmap, programs};
use crate::color_ramp::apply_color_ramp;
use crate::refresh_paint_attributes;

/// A tile's draw's entries in the render target's storage buffers.
struct HeatmapUniforms {
    draw_ubo: HeatmapDrawableUBO,
    projection: ProjectionUBO,
}

/// The corners of the quad that colors the render target onto the frame.
const TEXTURE_CORNERS: [HeatmapLayoutVertex; 4] = [
    HeatmapLayoutVertex { a1: [0, 0] },
    HeatmapLayoutVertex { a1: [1, 0] },
    HeatmapLayoutVertex { a1: [0, 1] },
    HeatmapLayoutVertex { a1: [1, 1] },
];

/// The layer's drawing: the tiles' draws into the render target, and the
/// layer's draw of the frame, which colors the render target's heat onto it.
#[derive(Default)]
pub struct HeatmapDrawing {
    tiles: TileMap<KeptDraw>,
    /// The texture the tiles draw their heat into, which the layer's draw
    /// colors onto the frame.
    render_target: Option<RenderTarget>,
    /// The tiles' draws into the render target.
    target: Option<LayerDraws>,
    /// The layer's draw, built on its first frame.
    texture: Option<KeptDraw>,
    /// The texture of the color ramp image that `texture` samples, made
    /// again for a new image.
    color_ramp: Option<(Rc<PremultipliedImage>, SharedTexture)>,
    /// The layer's draw of the frame, from its first update that draws the
    /// tiles.
    frame: Option<FrameDraws>,
}

/// The tiles' draws into the render target.
impl OffscreenPass for HeatmapDrawing {
    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if self.render_target.is_some()
            && let Some(target) = &self.target
        {
            target.upload(gpu, &mut self.tiles, tile_objects, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let Some(render_target) = &mut self.render_target else { return };
        let Some(mut passes) = render_target.begin(gpu, parameters) else { return };
        while passes.next(parameters) {
            if let Some(target) = &self.target {
                target.render(&mut self.tiles, tile_objects, gpu, parameters);
            }
        }
        passes.end(gpu, parameters);
    }
}

impl LayerDrawing for HeatmapDrawing {
    type Tile = KeptDraw;

    fn tiles(&mut self) -> Option<&mut TileMap<KeptDraw>> {
        Some(&mut self.tiles)
    }

    fn visit_groups<'a>(&'a self, visit: &mut dyn FnMut(DrawGroup<'a>)) {
        if let Some(frame) = &self.frame {
            visit(DrawGroup::Draws(frame));
        }
    }

    fn clear(&mut self) {
        self.tiles.clear();
        self.texture = None;
        self.color_ramp = None;
        if let Some(target) = &mut self.target {
            target.clear();
        }
        if let Some(frame) = &mut self.frame {
            frame.clear();
        }
    }

    fn begin_frame(&mut self) {
        self.frame.get_or_insert_default().clear();
    }

    fn clear_frame(&mut self) {
        if let Some(target) = &mut self.target {
            target.clear();
        }
        if let Some(frame) = &mut self.frame {
            frame.clear();
        }
    }

    fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if let (Some(frame), Some(texture)) = (&self.frame, &mut self.texture) {
            frame.upload_one(gpu, &mut texture.objects, context);
        }
    }

    fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>, order: DrawOrder) {
        if let Some(frame) = &self.frame {
            if let Some(texture) = &mut self.texture {
                frame.render_one(&mut texture.objects, gpu, parameters);
            }
            order.step(parameters);
        }
    }
}

/// What a heatmap layer keeps besides its drawing.
pub struct HeatmapState {
    /// The tiles' draws into the render target as the update makes them.
    tile_draws: DrawList<HeatmapUniforms>,
    color_ramp: Rc<PremultipliedImage>,
    /// The global-state snapshot used to generate the cached color ramp and
    /// detect when it needs to be rebuilt.
    color_ramp_global_state: Option<Arc<GlobalStateMap>>,

    shared_texture_vertices: Option<SharedVertices>,
}

impl HeatmapState {
    /// Makes the color ramp of `layer`'s `heatmap-color` again.
    fn update_color_ramp(&mut self, layer: &LayerValues<Heatmap>) {
        let mut color_value = layer.unevaluated.heatmap_color.get_value().clone();
        if color_value.is_undefined() {
            color_value = default_heatmap_color();
        }

        // The texture shares the ramp image, so a new one replaces it.
        let mut image = (*self.color_ramp).clone();
        apply_color_ramp(&color_value, &mut image, self.color_ramp_global_state.as_deref());
        self.color_ramp = Rc::new(image);
    }
}

impl LayerType for Heatmap {
    type Layout = HeatmapLayout;
    type Bucket = HeatmapBucket;
    type View<'a> = LayerBucket<'a, Heatmap>;
    type Drawing = HeatmapDrawing;
    type State = HeatmapState;

    fn create_layout(
        group: &[Arc<EvaluatedLayer<Heatmap>>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<HeatmapLayout>, Error> {
        HeatmapLayout::new(parameters.bucket_parameters, group, source_layer, parameters.filter_groups).map(Some)
    }

    fn new_layer(_layer: &LayerValues<Heatmap>) -> (HeatmapState, HeatmapDrawing) {
        let state = HeatmapState {
            tile_draws: DrawList::default(),
            color_ramp: Rc::new(PremultipliedImage::new(Size::new(256, 1))),
            color_ramp_global_state: None,
            shared_texture_vertices: None,
        };
        (state, HeatmapDrawing::default())
    }

    /// Remakes the color ramp where its value may have changed.
    fn evaluate(state: &mut HeatmapState, cx: EvaluateCx<'_, Heatmap>) -> Evaluation {
        let (layer, parameters) = (cx.layer, cx.parameters);
        // Build the ramp here rather than in transition(), which has no global
        // state. A new or changed layer always needs one; otherwise only a
        // change to a state key that the ramp reads does.
        let global_state_differs = match (&state.color_ramp_global_state, &parameters.global_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        let ramp_value = layer.unevaluated.heatmap_color.get_value();
        let ramp_affected = parameters.layer_changed
            || (global_state_differs
                && ramp_value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
                && global_state_refs_intersect(
                    ramp_value.get_global_state_refs(),
                    parameters.changed_global_state_keys.as_deref(),
                ));
        state.color_ramp_global_state = parameters.global_state.clone();
        if ramp_affected {
            state.update_color_ramp(layer);
        }
        if *cx.evaluated.paint.heatmap_opacity.uniform() > 0.0 { Evaluation::TRANSLUCENT } else { Evaluation::NONE }
    }

    /// Makes the tiles' draws into the render target, which it builds for a
    /// tile it keeps none of, and the layer's draw of the frame, which colors
    /// the render target's heat onto it, and writes the buffers they read.
    fn make_draws(
        state: &mut HeatmapState,
        drawing: &mut HeatmapDrawing,
        gpu: &mut dyn Gpu,
        cx: &mut DrawContext<'_, '_, Heatmap>,
    ) {
        let viewport_size = cx.frame.view.size();
        let size = Size::new(viewport_size.width / 2, viewport_size.height / 2);
        let render_target = drawing
            .render_target
            .get_or_insert_with(|| RenderTarget::new(cx.context, size, TextureChannelDataType::HalfFloat));
        render_target.set_size(cx.context, size);
        if state.draw_tiles(drawing, gpu, cx) {
            state.draw_texture(drawing, gpu, cx);
        }
    }

    fn offscreen_pass<'a>(
        _state: &'a mut HeatmapState,
        drawing: &'a mut HeatmapDrawing,
    ) -> Option<&'a mut dyn OffscreenPass> {
        Some(drawing)
    }
}

impl HeatmapState {
    /// Makes the tiles' draws into the render target of the layer, with
    /// their GPU objects in the tiles' caches, and writes the buffers they
    /// read. Returns false if the layer draws nothing.
    fn draw_tiles(
        &mut self,
        drawing: &mut HeatmapDrawing,
        gpu: &mut dyn Gpu,
        cx: &mut DrawContext<'_, '_, Heatmap>,
    ) -> bool {
        let (layer, variant, render_tiles, frame) = (cx.layer, cx.projection_variant, cx.tiles, &cx.frame);
        let tile_draws = &mut self.tile_draws;
        let evaluated = &layer.evaluated.paint;
        for tile in render_tiles {
            let tile_id = tile.id();

            let Some(render_data) = tile.view_to_draw() else {
                drawing.tiles.drop_value(&tile_id);
                continue;
            };
            let bucket = render_data.bucket;
            let Some(binders) = bucket.paint_property_binders.get(layer.id()) else {
                continue;
            };
            let geometry = render_data.geometry();
            let paint = |context: &mut _| {
                paint_vertices(
                    context,
                    geometry,
                    &binders.interleaved_vertex_buffer,
                    binders.depends_on_feature_state(),
                )
            };

            let Some(mut entry) = drawing.tiles.entry(&tile_id) else {
                continue;
            };
            // A draw built from another bucket goes.
            entry.set_bucket(bucket.base.id());

            // The draw's attributes read the bucket's vertices in the
            // context's store, which upload again when they change.
            let changes = entry.take_changes();
            let context = &mut *cx.context;
            if let Some(cached) = entry.value.as_mut() {
                refresh_paint_attributes(&mut cached.objects, changes, |attrs| {
                    binders.bind(evaluated, paint(context).as_ref(), attrs, &mut Uniforms::default());
                });
            } else if !bucket.vertices.is_empty()
                && let (Some(vertices), Some(indexes)) =
                    (bucket_vertices(geometry, &bucket.vertices), bucket_indexes(geometry, &bucket.triangles))
            {
                let mut uniforms = Uniforms::default();
                let mut attrs = VertexAttributeArray::new();
                binders.bind(evaluated, paint(context).as_ref(), &mut attrs, &mut uniforms);
                let shader = context.shader_key(&programs::HEATMAP, uniforms, variant);
                attrs.set_shared(
                    ID_HEATMAP_POS_VERTEX_ATTRIBUTE,
                    SharedAttribute::new(
                        vertices,
                        AttributeDataType::Short2,
                        offset_of!(HeatmapLayoutVertex, a1) as u32,
                        size_of::<HeatmapLayoutVertex>() as u32,
                    ),
                );
                let geometry = DrawGeometry {
                    vertices: attrs,
                    vertex_count: if bucket.segments.is_empty() { 0 } else { bucket.vertices.len() },
                    mode: DrawMode::Triangles,
                    indexes: indexes.into(),
                    segments: bucket.segments.clone(),
                    instances: None,
                };
                let objects = DrawObjects::new(
                    shader,
                    ColorMode::additive(),
                    CullFaceMode::disabled(),
                    geometry,
                    Textures::default(),
                );
                *entry.value = Some(KeptDraw::new(objects));
            }

            let Some(cached) = entry.value.as_ref() else {
                continue;
            };
            let draw = FrameDraw { depth: DrawDepth::Off, ..cached.frame_draw(Some(tile_id)) };
            tile_draws.push(draw, Self::tile_uniforms(&tile_id, binders, frame));
        }

        let target = drawing.target.get_or_insert_default();
        finish_tiles(tile_draws, target, &mut drawing.tiles, gpu, cx.context, |writer| {
            Self::write_tile_uniforms(evaluated, writer);
        });
        true
    }

    /// The entries in the render target's storage buffers of the draw of
    /// `tile_id`, whose bucket's binders are `binders`.
    fn tile_uniforms(tile_id: &OverscaledTileID, binders: &HeatmapBinders, frame: &DrawFrame<'_>) -> HeatmapUniforms {
        const NEAR_CLIPPED: bool = false;
        const IN_VIEWPORT_PIXEL_UNITS: bool = false;
        let zoom = frame.view.zoom() as f32;
        let tile_id = tile_id.to_unwrapped();
        let projection = projection_data(
            &tile_id,
            frame,
            [0.0, 0.0],
            TranslateAnchorType::Viewport,
            NEAR_CLIPPED,
            IN_VIEWPORT_PIXEL_UNITS,
            None,
            false,
        );
        let latitude_scale = frame.view.projection().circle_radius_correction(frame.view);
        HeatmapUniforms {
            draw_ubo: HeatmapDrawableUBO {
                matrix: cast_matrix(&projection.fallback_matrix),
                extrude_scale: tile_id.pixels_to_tile_units(1.0, zoom),
                weight_t: binders.heatmap_weight.interpolation_factor(zoom),
                radius_t: binders.heatmap_radius.interpolation_factor(zoom),
                globe_extrude_scale: globe_extrude_scale(&tile_id, zoom, latitude_scale),
            },
            projection: to_projection_ubo(&projection),
        }
    }

    /// Writes the buffers of the tiles' draws into the render target, of a
    /// layer of the paint values `evaluated`.
    fn write_tile_uniforms(
        evaluated: &HeatmapPaintProperties::PossiblyEvaluated,
        writer: &mut UniformWriter<'_, HeatmapUniforms>,
    ) {
        let props = HeatmapEvaluatedPropsUBO {
            weight: evaluated.heatmap_weight.constant_or(&HeatmapPaintProperties::Defaults::heatmap_weight()),
            radius: evaluated.heatmap_radius.constant_or(&HeatmapPaintProperties::Defaults::heatmap_radius()),
            intensity: *evaluated.heatmap_intensity.uniform(),
            padding: 0.0,
        };
        writer.uniform(PROPS.id, &props);
        writer.storage(DRAWABLE.id, |uniforms| uniforms.draw_ubo);
        writer.storage(PROJECTION_BUFFER.id, |uniforms| uniforms.projection);
    }

    /// Makes the layer's draw of the frame, which colors the heat of the
    /// render target onto it through the layer's color ramp, with GPU
    /// objects it builds once, and writes the buffers it reads.
    fn draw_texture(&mut self, drawing: &mut HeatmapDrawing, gpu: &mut dyn Gpu, cx: &mut DrawContext<'_, '_, Heatmap>) {
        let (layer, frame) = (cx.layer, &cx.frame);
        let HeatmapState { color_ramp, shared_texture_vertices, .. } = self;
        let Some(render_target) = drawing.render_target.as_ref() else {
            return;
        };
        if drawing.frame.is_none() {
            return;
        }
        let context = &mut *cx.context;
        let heatmap_texture_shader =
            context.shader_key(&programs::HEATMAP_TEXTURE, Uniforms::default(), ProjectionVariant::Mercator);
        if !drawing.color_ramp.as_ref().is_some_and(|(image, _)| Rc::ptr_eq(image, color_ramp)) {
            let texture = context.add_texture_with(|texture| {
                texture.set_image(Some(Rc::clone(color_ramp)));
                texture.set_sampler(SamplerState {
                    filter: TextureFilterType::Linear,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            });
            drawing.color_ramp = Some((Rc::clone(color_ramp), texture));
        }
        let color_ramp = drawing.color_ramp.as_ref().map(|(_, texture)| texture);
        let render_target_texture = render_target.texture();
        if let Some(draw) = &mut drawing.texture {
            // The render target's texture is made again with its size.
            draw.objects.set_texture(ID_HEATMAP_IMAGE_TEXTURE, Some(render_target_texture));
            draw.objects.set_texture(ID_HEATMAP_COLOR_RAMP_TEXTURE, color_ramp);
        } else {
            let vertices =
                shared_texture_vertices.get_or_insert_with(|| copied_vertices(context, &TEXTURE_CORNERS)).clone();
            let mut attrs = VertexAttributeArray::new();
            attrs.set_shared(
                ID_HEATMAP_POS_VERTEX_ATTRIBUTE,
                SharedAttribute::new(
                    vertices,
                    AttributeDataType::Short2,
                    offset_of!(HeatmapLayoutVertex, a1) as u32,
                    size_of::<HeatmapLayoutVertex>() as u32,
                ),
            );
            let geometry = DrawGeometry {
                vertices: attrs,
                vertex_count: 4,
                mode: DrawMode::Triangles,
                indexes: DrawIndexes::new(RenderStaticData::quad_triangle_indices()),
                segments: RenderStaticData::quad_triangle_segments(),
                instances: None,
            };
            let mut textures = Textures::default();
            textures[ID_HEATMAP_IMAGE_TEXTURE] = Some(render_target_texture.clone());
            textures[ID_HEATMAP_COLOR_RAMP_TEXTURE] = color_ramp.cloned();
            let objects = DrawObjects::new(
                heatmap_texture_shader,
                context.color_mode(ColorMode::alpha_blended()),
                CullFaceMode::disabled(),
                geometry,
                textures,
            );
            drawing.texture = Some(KeptDraw::new(objects));
        }
        let (Some(draw), Some(draws)) = (&drawing.texture, drawing.frame.as_mut()) else {
            return;
        };
        draws.push(draw.frame_draw(None));
        let size = frame.renderable_size;
        let m = orthographic(0.0, f64::from(size.width), f64::from(size.height), 0.0, -1.0, 1.0);
        let props = HeatmapTexturePropsUBO {
            matrix: cast_matrix(&m),
            opacity: *layer.evaluated.paint.heatmap_opacity.uniform(),
            pad1: 0.0,
            pad2: 0.0,
            pad3: 0.0,
        };
        if draws.write_pod(gpu, TEXTURE_PROPS.id, &props, context)
            && let Some(texture) = &mut drawing.texture
        {
            texture.objects.drop_bindings();
        }
        draws.bind_no_projection(gpu, context);
    }
}
