// The tiles are drawn (additively, kernel density) into a half-float
// target of the layer's own; the layer's draw on the frame colors that
// texture through the color ramp.

use std::mem::offset_of;
use std::rc::Rc;
use std::sync::Arc;

use glam::dcamera::rh::proj::opengl::orthographic;
use mln_engine::draw::ground::StandingDraw;
use mln_engine::draw::{
    Depth, Draw, DrawGeometry, DrawIndexes, Draws, Group, LayerFrame, Mesh, RenderPass, RenderStaticData, SegmentBase,
    SharedAttribute, SharedTexture, SharedVertices, Target, Textures, Uniforms, VertexAttributeArray, cast_matrix,
    globe_extrude_scale, projection_data, refresh_paint_attributes, to_projection_ubo,
};
use mln_engine::layer::binders::PaintBinders as _;
use mln_engine::layer::{
    EvaluateCx, Evaluation, Features, FrameTile, Layer, LayerValues, OnTerrain, PrepareCx, TileCx, TileData,
    bucket_indexes, bucket_vertices, copied_vertices, paint_vertices,
};
use mln_engine::tile::OverscaledTileID;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode, Rgba};
use mln_style::StyleValue as _;
use mln_style::bitmap::PremultipliedImage;
use mln_style::expression::{Dependency, GlobalStateMap, global_state_refs_intersect};
use mln_style::layers::Heatmap;
use mln_style::layers::heatmap_layer_properties::{HeatmapPaintProperties, default_heatmap_color};
use mln_style::size::Size;
use mln_style::types::TranslateAnchorType;

use crate::layers::heatmap::bucket::{HeatmapBinders, HeatmapLayoutVertex};
use crate::layers::heatmap::layout::HeatmapLayout;
use crate::layers::heatmap::programs;
use crate::layers::heatmap::ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO, HeatmapTexturePropsUBO};
use crate::layers::line::layer::apply_color_ramp;

/// The slots of the programs' textures that the draws bind.
const COLOR_RAMP_SLOT: usize = programs::HEATMAP_TEXTURE.texture_slot("color_ramp");
const IMAGE_SLOT: usize = programs::HEATMAP_TEXTURE.texture_slot("image");

/// The pass the layer's draws draw in, onto the frame and into its render
/// target.
const RENDER_PASS: RenderPass = RenderPass::Translucent;

pub(crate) struct RenderHeatmapLayer {
    color_ramp: Rc<PremultipliedImage>,
    /// The global-state snapshot used to generate the cached color ramp and
    /// detect when it needs to be rebuilt.
    color_ramp_global_state: Option<Arc<GlobalStateMap>>,

    /// The target the tiles draw their heat into, which the layer's draw
    /// colors onto the frame.
    target: Option<Target>,

    shared_texture_vertices: Option<SharedVertices>,
    /// The mesh of the layer's draw of the frame, built on its first frame.
    texture: Option<Mesh>,
    /// The texture of the color ramp image that `texture` samples, made
    /// again for a new image.
    color_ramp_texture: Option<(Rc<PremultipliedImage>, SharedTexture)>,
}

impl RenderHeatmapLayer {
    fn update_color_ramp(&mut self, values: &LayerValues<Heatmap>) {
        let mut color_value = values.unevaluated.heatmap_color.get_value().clone();
        if color_value.is_undefined() {
            color_value = default_heatmap_color();
        }

        // The texture shares the ramp image, so a new one replaces it.
        let mut image = (*self.color_ramp).clone();
        apply_color_ramp(&color_value, &mut image, self.color_ramp_global_state.as_deref());
        self.color_ramp = Rc::new(image);
    }
}

impl Layer for RenderHeatmapLayer {
    type Style = Heatmap;
    type Input = Features<HeatmapLayout>;
    type Tile = StandingDraw;

    const TERRAIN: OnTerrain = OnTerrain::Stands { hides_behind_ground: false };

    fn new(_values: &LayerValues<Heatmap>) -> Self {
        RenderHeatmapLayer {
            color_ramp: Rc::new(PremultipliedImage::new(Size::new(256, 1))),
            color_ramp_global_state: None,
            target: None,
            shared_texture_vertices: None,
            texture: None,
            color_ramp_texture: None,
        }
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let parameters = cx.parameters;
        // Build the ramp here rather than in transition(), which has no global
        // state. A new or changed layer always needs one; otherwise only a
        // change to a state key that the ramp reads does.
        let global_state_differs = match (&self.color_ramp_global_state, &parameters.global_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        let ramp_value = cx.values.unevaluated.heatmap_color.get_value();
        let ramp_affected = parameters.layer_changed
            || (global_state_differs
                && ramp_value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
                && global_state_refs_intersect(
                    ramp_value.get_global_state_refs(),
                    parameters.changed_global_state_keys.as_deref(),
                ));
        self.color_ramp_global_state = parameters.global_state.clone();
        if ramp_affected {
            self.update_color_ramp(cx.values);
        }
        let evaluated = &*cx.evaluated;
        if *evaluated.paint.heatmap_opacity.uniform() > 0.0 { Evaluation::TRANSLUCENT } else { Evaluation::NONE }
    }

    /// The draw of a tile's kernels into the layer's target.
    fn tile(
        &mut self,
        cx: &mut TileCx<'_, '_, Self>,
        tile: FrameTile<'_, RenderHeatmapLayer>,
        _data: TileData<'_, RenderHeatmapLayer>,
        _previous: Option<StandingDraw>,
    ) -> Option<StandingDraw> {
        let render_data = tile.data_for_pass(RENDER_PASS)?;
        let bucket = render_data.bucket;
        let binders = bucket.paint_property_binders.get(&cx.base.style.base().id)?;
        let geometry = render_data.geometry();
        if bucket.vertices.is_empty() {
            return None;
        }
        let (vertices, indexes) =
            (bucket_vertices(geometry, &bucket.vertices)?, bucket_indexes(geometry, &bucket.triangles)?);
        let context = &mut *cx.res.context;
        let paint =
            paint_vertices(context, geometry, &binders.interleaved_vertex_buffer, binders.depends_on_feature_state());
        let mut uniforms = Uniforms::default();
        let mut attrs = VertexAttributeArray::new();
        binders.bind(&cx.values.evaluated.paint, paint.as_ref(), &mut attrs, &mut uniforms);
        let shader = context.shader_key(&programs::HEATMAP, uniforms, cx.base.projection_variant);
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
        let mesh = cx.res.mesh(shader, ColorMode::additive(), CullFaceMode::disabled(), geometry, Textures::default());
        Some(StandingDraw::new(mesh))
    }

    /// Pushes the tiles' draws into the layer's target, of half the
    /// view's size, and the layer's draw of the frame, which colors the
    /// target's heat onto it.
    fn prepare(
        &mut self,
        cx: &mut PrepareCx<'_, '_, Self>,
        render_tiles: &[FrameTile<'_, RenderHeatmapLayer>],
        out: &mut Draws,
    ) {
        let viewport_size = cx.frame.view.size();
        let size = mln_gfx::Size::new(viewport_size.width / 2, viewport_size.height / 2);
        // The tiles' draws add their heat to none.
        let target = self
            .target
            .get_or_insert_with(|| cx.res.target(size, TextureChannelDataType::HalfFloat, Rgba::TRANSPARENT));
        target.set_size(&mut cx.res, size);
        Self::draw_tiles(cx, render_tiles, out.target(target));
        self.draw_texture(cx, out);
    }
}

impl RenderHeatmapLayer {
    /// Pushes the tiles' draws into `target`, the group of the layer's
    /// target, with their meshes in the tiles' values.
    fn draw_tiles(
        cx: &mut PrepareCx<'_, '_, Self>,
        render_tiles: &[FrameTile<'_, RenderHeatmapLayer>],
        target: &mut Group,
    ) {
        let PrepareCx { base, values, tiles, res, frame, .. } = cx;

        let evaluated = &values.evaluated.paint;
        for tile in render_tiles {
            let tile_id = tile.id();

            let Some(render_data) = tile.data_for_pass(RENDER_PASS) else {
                continue;
            };
            let bucket = render_data.bucket;
            let Some(binders) = bucket.paint_property_binders.get(&base.style.base().id) else {
                continue;
            };
            let Some(mut entry) = tiles.entry(&tile_id) else {
                continue;
            };
            // The draw's attributes read the bucket's vertices in the
            // context's store, which upload again when they change.
            let changes = entry.take_changes();
            let Some(draw) = entry.value.as_mut() else {
                continue;
            };
            let context = &mut *res.context;
            refresh_paint_attributes(&draw.mesh, context, changes, |context, attrs| {
                let geometry = render_data.geometry();
                let buffer = &binders.interleaved_vertex_buffer;
                let paint = paint_vertices(context, geometry, buffer, binders.depends_on_feature_state());
                binders.bind(evaluated, paint.as_ref(), attrs, &mut Uniforms::default());
            });
            // Over terrain, the kernels stand on the frame's ground. They
            // don't hide behind it: their density adds up on the screen, as
            // GL JS's does.
            let ground = frame.ground.as_deref();
            let centers = &bucket.centers;
            draw.stand(context, ground, tile_id.to_unwrapped(), "ground_height", centers);
            let (drawable, projection) = Self::tile_uniforms(&tile_id, binders, frame);
            // The draws keep the order they're pushed in, the tiles'.
            let target_draw = Draw { depth: Depth::Off, ..Draw::DEFAULT };
            target.push_at(&draw.mesh, tile_id, &projection, target_draw, drawable);
        }

        target.block(&Self::tile_props(evaluated));
    }

    /// The block and the projection of the draw of `tile_id`, whose bucket's
    /// binders are `binders`.
    fn tile_uniforms(
        tile_id: &OverscaledTileID,
        binders: &HeatmapBinders,
        frame: &LayerFrame<'_>,
    ) -> (HeatmapDrawableUBO, ProjectionUBO) {
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
        let drawable = HeatmapDrawableUBO {
            matrix: cast_matrix(&projection.fallback_matrix),
            extrude_scale: tile_id.pixels_to_tile_units(1.0, zoom),
            weight_t: binders.heatmap_weight.interpolation_factor(zoom),
            radius_t: binders.heatmap_radius.interpolation_factor(zoom),
            globe_extrude_scale: globe_extrude_scale(&tile_id, zoom, latitude_scale),
        };
        (drawable, to_projection_ubo(&projection))
    }

    /// The block that the tiles' draws into the target share, of a
    /// layer of the paint values `evaluated`.
    fn tile_props(evaluated: &HeatmapPaintProperties::PossiblyEvaluated) -> HeatmapEvaluatedPropsUBO {
        HeatmapEvaluatedPropsUBO {
            weight: evaluated.heatmap_weight.constant_or(&HeatmapPaintProperties::Defaults::heatmap_weight()),
            radius: evaluated.heatmap_radius.constant_or(&HeatmapPaintProperties::Defaults::heatmap_radius()),
            intensity: *evaluated.heatmap_intensity.uniform(),
            padding: 0.0,
        }
    }

    /// Pushes into `out`'s main group the layer's draw of the frame, which
    /// colors the heat of the layer's target onto it through the layer's
    /// color ramp, with a mesh it builds once.
    fn draw_texture(&mut self, cx: &mut PrepareCx<'_, '_, Self>, out: &mut Draws) {
        let RenderHeatmapLayer { color_ramp, target, shared_texture_vertices, texture, color_ramp_texture, .. } = self;
        let Some(target) = target.as_ref() else {
            return;
        };
        let context = &mut *cx.res.context;
        let heatmap_texture_shader =
            context.shader_key(&programs::HEATMAP_TEXTURE, Uniforms::default(), ProjectionVariant::Mercator);
        if !color_ramp_texture.as_ref().is_some_and(|(image, _)| Rc::ptr_eq(image, color_ramp)) {
            let texture = context.add_texture_with(|texture| {
                texture.set_image(Some(Rc::clone(color_ramp)));
                texture.set_sampler(SamplerState {
                    filter: TextureFilterType::Linear,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            });
            *color_ramp_texture = Some((Rc::clone(color_ramp), texture));
        }
        let color_ramp = color_ramp_texture.as_ref().map(|(_, texture)| texture);
        if let Some(draw) = texture {
            draw.set_texture(context, COLOR_RAMP_SLOT, color_ramp);
        } else {
            let vertices = shared_texture_vertices
                .get_or_insert_with(|| copied_vertices(context, &crate::layers::heatmap::heatmap_texture_vertices()))
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
                segments: crate::layers::heatmap::heatmap_texture_segments(),
                instances: None,
            };
            let mut textures = Textures::default();
            textures[IMAGE_SLOT] = Some(target.texture().clone());
            textures[COLOR_RAMP_SLOT] = color_ramp.cloned();
            let mesh = cx.res.mesh(
                heatmap_texture_shader,
                ColorMode::alpha_blended(),
                CullFaceMode::disabled(),
                geometry,
                textures,
            );
            *texture = Some(mesh);
        }
        let Some(draw) = texture.as_ref() else {
            return;
        };
        let size = cx.frame.renderable_size;
        let m = orthographic(0.0, f64::from(size.width), f64::from(size.height), 0.0, -1.0, 1.0);
        let props = HeatmapTexturePropsUBO {
            matrix: cast_matrix(&m),
            opacity: *cx.values.evaluated.paint.heatmap_opacity.uniform(),
            pad1: 0.0,
            pad2: 0.0,
            pad3: 0.0,
        };
        out.main().push(draw, Draw::DEFAULT, ()).block(&props);
    }
}
