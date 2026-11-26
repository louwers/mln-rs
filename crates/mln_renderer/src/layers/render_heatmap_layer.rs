// The tiles are drawn (additively, kernel density) into an offscreen
// half-float render target; the layer's draw on the frame colors that
// texture through the color ramp.

use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use glam::dcamera::rh::proj::opengl::orthographic;
use mln_gfx::gpu::Gpu;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::expression::{Dependency, GlobalStateMap, global_state_refs_intersect};
use mln_style::layer::LayerStyle;
use mln_style::layers::Heatmap;
use mln_style::layers::heatmap_layer_properties::{HeatmapPaintProperties, default_heatmap_color};
use mln_style::size::Size;
use mln_style::types::TranslateAnchorType;
use mln_style::{PropertyEvaluationParameters, StyleValue as _};
use mln_tile_id::OverscaledTileID;

use super::render_line_layer::apply_color_ramp;
use super::{cast_matrix, refresh_paint_attributes};
use crate::bucket_geometry::{bucket_indexes, bucket_vertices, copied_vertices, paint_vertices};
use crate::buckets::heatmap_bucket::{HeatmapBinders, HeatmapBucket, HeatmapLayoutVertex};
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::gfx::context::Context;
use crate::gfx::indexes::DrawIndexes;
use crate::gfx::segment::SegmentBase;
use crate::gfx::store::SharedTexture;
use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::gfx::vertices::SharedVertices;
use crate::layer_drawing::{
    DrawDepth, DrawGroup, DrawList, DrawOrder, FrameDraw, FrameDraws, KeptDraw, LayerDrawing, LayerDraws, TileMap,
    UniformWriter, finish_tiles, tile_objects,
};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layers::heatmap::programs;
use crate::layers::heatmap::ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO, HeatmapTexturePropsUBO};
use crate::layout::heatmap_layout::HeatmapLayout;
use crate::layout::layout::{LayoutParameters, SourceLayer};
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    Atlases, LayerBucket, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_target::RenderTarget;
use crate::render_tile::FrameTile;
use crate::terrain::StandingDraw;
use crate::tile_matrix::{bind_no_projection, globe_extrude_scale, projection_data, to_projection_ubo};

/// The ids of the buffers that the draws bind.
const DRAWABLE_ID: usize = programs::HEATMAP.buffer_id("drawableVector");
const PROPS_ID: usize = programs::HEATMAP.buffer_id("props");
const TEXTURE_PROPS_ID: usize = programs::HEATMAP_TEXTURE.buffer_id("ubo");

/// The slots of the programs' textures that the draws bind.
const COLOR_RAMP_SLOT: usize = programs::HEATMAP_TEXTURE.texture_slot("color_ramp");
const IMAGE_SLOT: usize = programs::HEATMAP_TEXTURE.texture_slot("image");

/// The pass the layer's draws draw in, onto the frame and into its render
/// target.
const RENDER_PASS: RenderPass = RenderPass::Translucent;

/// A tile's draw's entries in the render target's storage buffers.
struct HeatmapUniforms {
    draw_ubo: HeatmapDrawableUBO,
    projection: ProjectionUBO,
}

/// The layer's drawing: the tiles' draws into the render target, and the
/// layer's draw of the frame, which colors the render target's heat onto it.
#[derive(Default)]
pub(crate) struct HeatmapDrawing {
    tiles: TileMap<StandingDraw>,
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

impl HeatmapDrawing {
    /// Uploads the GPU objects of the tiles' draws into the render target.
    fn upload_target(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if let Some(target) = &self.target {
            target.upload(gpu, &mut self.tiles, tile_objects, context);
        }
    }

    /// Draws the tiles' draws into the render target into the open pass of
    /// `gpu`, the context's, which draws into it.
    fn render_target(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if let Some(target) = &self.target {
            target.render(&mut self.tiles, tile_objects, gpu, parameters);
        }
    }
}

impl LayerDrawing for HeatmapDrawing {
    type Tile = StandingDraw;

    fn tiles(&mut self) -> Option<&mut TileMap<StandingDraw>> {
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

impl LayerType for Heatmap {
    type Layout = HeatmapLayout;
    type Bucket = HeatmapBucket;
    type TileBucket<'a> = LayerBucket<'a, Heatmap>;

    fn create_layout(
        group: &[Arc<EvaluatedLayer<Heatmap>>],
        parameters: &mut LayoutParameters<'_>,
        source_layer: SourceLayer,
    ) -> Result<Option<HeatmapLayout>, Error> {
        HeatmapLayout::new(parameters.bucket_parameters, group, source_layer, parameters.filter_groups).map(Some)
    }

    fn render_layer(style: Arc<LayerStyle<Heatmap>>) -> Box<dyn RenderLayer> {
        Box::new(RenderHeatmapLayer::new(style))
    }
}

pub(crate) struct RenderHeatmapLayer {
    base: RenderLayerBase,
    values: LayerValues<Heatmap>,
    drawing: HeatmapDrawing,
    /// The tiles' draws into the render target as the update makes them.
    tile_draws: DrawList<HeatmapUniforms>,
    color_ramp: Rc<PremultipliedImage>,
    /// The global-state snapshot used to generate the cached color ramp and
    /// detect when it needs to be rebuilt.
    color_ramp_global_state: Option<Arc<GlobalStateMap>>,

    /// The texture the tiles draw their heat into, which the layer's draw
    /// colors onto the frame.
    render_target: Option<RenderTarget>,

    shared_texture_vertices: Option<SharedVertices>,
}

impl RenderHeatmapLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Heatmap>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderHeatmapLayer {
            base,
            values,
            drawing: HeatmapDrawing::default(),
            tile_draws: DrawList::default(),
            color_ramp: Rc::new(PremultipliedImage::new(Size::new(256, 1))),
            color_ramp_global_state: None,
            render_target: None,
            shared_texture_vertices: None,
        }
    }

    fn update_color_ramp(&mut self) {
        let mut color_value = self.values.unevaluated.heatmap_color.get_value().clone();
        if color_value.is_undefined() {
            color_value = default_heatmap_color();
        }

        // The texture shares the ramp image, so a new one replaces it.
        let mut image = (*self.color_ramp).clone();
        apply_color_ramp(&color_value, &mut image, self.color_ramp_global_state.as_deref());
        self.color_ramp = Rc::new(image);
    }
}

impl TypedLayer for RenderHeatmapLayer {
    type Type = Heatmap;
    type Drawing = HeatmapDrawing;

    const STANDS: bool = true;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    fn complete_evaluation(
        &mut self,
        parameters: &PropertyEvaluationParameters,
        _evaluated: &mut EvaluatedLayer<Heatmap>,
    ) {
        // Build the ramp here rather than in transition(), which has no global
        // state. A new or changed layer always needs one; otherwise only a
        // change to a state key that the ramp reads does.
        let global_state_differs = match (&self.color_ramp_global_state, &parameters.global_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        let ramp_value = self.values.unevaluated.heatmap_color.get_value();
        let ramp_affected = parameters.layer_changed
            || (global_state_differs
                && ramp_value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
                && global_state_refs_intersect(
                    ramp_value.get_global_state_refs(),
                    parameters.changed_global_state_keys.as_deref(),
                ));
        self.color_ramp_global_state = parameters.global_state.clone();
        if ramp_affected {
            self.update_color_ramp();
        }
    }

    fn passes(&self, evaluated: &EvaluatedLayer<Heatmap>) -> RenderPass {
        if *evaluated.paint.heatmap_opacity.uniform() > 0.0 {
            RenderPass::Translucent | RenderPass::Pass3D
        } else {
            RenderPass::None
        }
    }

    /// Makes the tiles' draws into the render target, which it builds for a
    /// tile it keeps none of, and the layer's draw of the frame, which colors
    /// the render target's heat onto it, and writes the buffers they read.
    fn update_draws(
        &mut self,
        gpu: &mut dyn Gpu,
        paint_parameters: &mut PaintParameters<'_>,
        _atlases: &Atlases,
        render_tiles: &[FrameTile<'_, Heatmap>],
    ) {
        let state = paint_parameters.view;

        let viewport_size = state.size();
        let size = mln_gfx::Size::new(viewport_size.width / 2, viewport_size.height / 2);
        let render_target = self.render_target.get_or_insert_with(|| {
            RenderTarget::new(paint_parameters.context, size, TextureChannelDataType::HalfFloat)
        });
        render_target.set_size(paint_parameters.context, size);
        if self.draw_tiles(gpu, render_tiles, paint_parameters) {
            self.draw_texture(gpu, paint_parameters);
        }
    }
}

impl RenderLayer for RenderHeatmapLayer {
    fn upload_render_targets(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        if self.base.is_renderable && self.render_target.is_some() {
            self.drawing.upload_target(gpu, context);
        }
    }

    fn render_targets(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if self.base.is_renderable
            && let Some(render_target) = &mut self.render_target
            && let Some(mut passes) = render_target.begin(gpu, parameters)
        {
            while passes.next(parameters) {
                self.drawing.render_target(gpu, parameters);
            }
            passes.end(gpu, parameters);
        }
    }
}

impl RenderHeatmapLayer {
    /// Makes the tiles' draws into the render target, with their GPU objects
    /// in the tiles' caches, and writes the buffers they read. Returns false
    /// if the layer draws nothing.
    fn draw_tiles(
        &mut self,
        gpu: &mut dyn Gpu,
        render_tiles: &[FrameTile<'_, Heatmap>],
        paint_parameters: &mut PaintParameters<'_>,
    ) -> bool {
        let RenderHeatmapLayer { base, values, drawing, tile_draws, .. } = self;

        let evaluated = &values.evaluated.paint;
        // The draws keep the order they're pushed in, the tiles'.
        let order = drawing.target.get_or_insert_default().id();
        for tile in render_tiles {
            let tile_id = tile.id();

            let Some(render_data) = tile.bucket_for_pass(RENDER_PASS) else {
                drawing.tiles.drop_value(&tile_id);
                continue;
            };
            let bucket = render_data.bucket;
            let Some(binders) = bucket.paint_property_binders.get(&base.style.base().id) else {
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
            entry.set_bucket(bucket.base.bucket_id);

            // The draw's attributes read the bucket's vertices in the
            // context's store, which upload again when they change.
            let changes = entry.take_changes();
            let context = &mut *paint_parameters.context;
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
                let shader = context.shader_key(&programs::HEATMAP, uniforms, base.projection_variant);
                attrs.set_shared(
                    "pos",
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
                    mode: DrawMode::Triangles(),
                    indexes: indexes.into(),
                    segments: bucket.segments.iter().map(SegmentBase::copy).collect(),
                    instances: None,
                };
                let objects = DrawObjects::new(
                    shader,
                    ColorMode::additive(),
                    CullFaceMode::disabled(),
                    geometry,
                    Textures::default(),
                );
                *entry.value = Some(StandingDraw::new(objects));
            }

            let Some(draw) = entry.value.as_mut() else {
                continue;
            };
            // Over terrain, the kernels stand on the frame's ground. They
            // don't hide behind it: their density adds up on the screen, as
            // GL JS's does.
            let ground = paint_parameters.ground.as_deref();
            let centers = &bucket.centers;
            draw.stand(context, ground, tile_id.to_unwrapped(), "ground_height", centers);
            let draw = FrameDraw { depth: DrawDepth::Off, ..FrameDraw::new(Some(tile_id), order) };
            tile_draws.push(draw, Self::tile_uniforms(&tile_id, binders, paint_parameters));
        }

        let target = drawing.target.get_or_insert_default();
        finish_tiles(tile_draws, target, &mut drawing.tiles, gpu, paint_parameters.context, |writer| {
            Self::write_tile_uniforms(evaluated, writer);
        });
        true
    }

    /// The entries in the render target's storage buffers of the draw of
    /// `tile_id`, whose bucket's binders are `binders`.
    fn tile_uniforms(
        tile_id: &OverscaledTileID,
        binders: &HeatmapBinders,
        parameters: &PaintParameters<'_>,
    ) -> HeatmapUniforms {
        const NEAR_CLIPPED: bool = false;
        const IN_VIEWPORT_PIXEL_UNITS: bool = false;
        let zoom = parameters.view.zoom() as f32;
        let tile_id = tile_id.to_unwrapped();
        let projection = projection_data(
            &tile_id,
            parameters,
            [0.0, 0.0],
            TranslateAnchorType::Viewport,
            NEAR_CLIPPED,
            IN_VIEWPORT_PIXEL_UNITS,
            None,
            false,
        );
        let latitude_scale = parameters.view.projection().circle_radius_correction(parameters.view);
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
        writer.uniform(PROPS_ID, &props);
        writer.storage(DRAWABLE_ID, |uniforms| uniforms.draw_ubo);
        writer.storage(PROJECTION_ID, |uniforms| uniforms.projection);
    }

    /// Makes the layer's draw of the frame, which colors the heat of the
    /// render target onto it through the layer's color ramp, with GPU
    /// objects it builds once, and writes the buffers it reads.
    fn draw_texture(&mut self, gpu: &mut dyn Gpu, paint_parameters: &mut PaintParameters<'_>) {
        let RenderHeatmapLayer { values, drawing, color_ramp, render_target, shared_texture_vertices, .. } = self;
        let Some(render_target) = render_target.as_ref() else {
            return;
        };
        if drawing.frame.is_none() {
            return;
        }
        let context = &mut *paint_parameters.context;
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
            draw.objects.set_texture(IMAGE_SLOT, Some(render_target_texture));
            draw.objects.set_texture(COLOR_RAMP_SLOT, color_ramp);
        } else {
            let vertices = shared_texture_vertices
                .get_or_insert_with(|| copied_vertices(context, &RenderStaticData::heatmap_texture_vertices()))
                .clone();
            let mut attrs = VertexAttributeArray::new();
            attrs.set_shared(
                "pos",
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
                mode: DrawMode::Triangles(),
                indexes: DrawIndexes::new(RenderStaticData::quad_triangle_indices()),
                segments: RenderStaticData::heatmap_texture_segments(),
                instances: None,
            };
            let mut textures = Textures::default();
            textures[IMAGE_SLOT] = Some(render_target_texture.clone());
            textures[COLOR_RAMP_SLOT] = color_ramp.cloned();
            let objects = DrawObjects::new(
                heatmap_texture_shader,
                ColorMode::alpha_blended(),
                CullFaceMode::disabled(),
                geometry,
                textures,
            );
            drawing.texture = Some(KeptDraw::new(objects));
        }
        let (Some(draw), Some(frame)) = (&drawing.texture, drawing.frame.as_mut()) else {
            return;
        };
        let frame_draw = draw.frame_draw(None);
        frame.push(frame_draw);
        let size = paint_parameters.renderable_size;
        let m = orthographic(0.0, f64::from(size.width), f64::from(size.height), 0.0, -1.0, 1.0);
        let props = HeatmapTexturePropsUBO {
            matrix: cast_matrix(&m),
            opacity: *values.evaluated.paint.heatmap_opacity.uniform(),
            pad1: 0.0,
            pad2: 0.0,
            pad3: 0.0,
        };
        let context = &mut *paint_parameters.context;
        if frame.uniform_buffers.write_pod(gpu, TEXTURE_PROPS_ID, &props, context.device())
            && let Some(texture) = &mut drawing.texture
        {
            texture.objects.drop_bindings();
        }
        bind_no_projection(gpu, &mut frame.uniform_buffers, context);
    }
}
