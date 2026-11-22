use glam::dcamera::rh::proj::opengl::orthographic;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TextureWrapType};
use mln_render::draw::{Depth, Draw, Draws, Mesh, MeshDesc, Target};
use mln_render::layer::{
    EvaluateCx, Evaluation, Features, FrameTile, Layer, LayerBucket, NewCx, PrepareCx, TileCx, TileTransform, UpdateCx,
};
use mln_style::StyleValue as _;
use mln_util::Color;
use mln_util::size::Size;

use super::bucket::HeatmapBucket;
use super::layout::HeatmapLayout;
use super::programs::{HEATMAP, HEATMAP_TEXTURE};
use super::ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO, HeatmapTexturePropsUBO};
use super::{Heatmap, default_heatmap_color};
use crate::color_ramp::ColorRamp;

/// A heatmap layer: the tiles add up their points' heat in a target, which
/// the layer then colors onto the frame through its color ramp.
pub struct HeatmapLayer {
    /// The heat, at half the viewport's size.
    heat: Target,
    /// The ramp of `heatmap-color`.
    ramp: ColorRamp,
    /// The viewport's quad, which colors the heat.
    colorize: Mesh,
}

/// How the heat and the ramp are sampled.
const LINEAR: SamplerState = SamplerState {
    filter: TextureFilterType::Linear,
    wrap_u: TextureWrapType::Clamp,
    wrap_v: TextureWrapType::Clamp,
    max_anisotropy: 1,
    mipmapped: false,
};

impl Layer for HeatmapLayer {
    type Style = Heatmap;
    type Input = Features<HeatmapLayout>;
    type Tile = Mesh<HeatmapDrawableUBO>;

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        let res = &mut *cx.res;
        let heat = res.target(Size::new(0, 0), TextureChannelDataType::HalfFloat, Color::new(0.0, 0.0, 0.0, 1.0));
        let ramp = ColorRamp::new(res, 256);
        let corners = res.buffer::<[i16; 2]>(&[[0, 0], [1, 0], [0, 1], [1, 1]]);
        let quad = res.index_buffer(&[0, 1, 2, 1, 2, 3]);
        let colorize = {
            let mut desc = MeshDesc::new(&HEATMAP_TEXTURE, &quad);
            desc.vertices(&corners).texture(heat.texture(), LINEAR).texture(ramp.texture(), LINEAR).screen_space();
            res.mesh(&desc)
        };
        HeatmapLayer { heat, ramp, colorize }
    }

    /// Makes the color ramp again where its value may have changed.
    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let value = cx.layer.unevaluated.heatmap_color.get_value();
        if value.is_undefined() {
            self.ramp.evaluate(&default_heatmap_color(), cx.parameters);
        } else {
            self.ramp.evaluate(value, cx.parameters);
        }
        Evaluation::translucent_if(*cx.evaluated.paint.heatmap_opacity.uniform() > 0.0)
    }

    fn update(&mut self, cx: &mut UpdateCx<'_, Self>) {
        let size = cx.frame.camera.size();
        cx.res.resize(&self.heat, Size::new(size.width / 2, size.height / 2));
        self.ramp.update(cx.res);
    }

    fn tile(
        &mut self,
        cx: &mut TileCx<'_, Self>,
        _tile: FrameTile,
        laid: LayerBucket<'_, Heatmap, HeatmapBucket>,
    ) -> Option<Mesh<HeatmapDrawableUBO>> {
        let bucket = laid.bucket;
        let mut mesh = MeshDesc::new(&HEATMAP, laid.indexes(&bucket.triangles));
        mesh.vertices(laid.buffer(&bucket.corners))
            .paint(laid.binders(&bucket.binders), &cx.layer.evaluated.paint)
            .blend(ColorMode::additive());
        Some(cx.res.mesh(&mesh))
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let paint = &cx.layer.evaluated.paint;
        let heat = out.target(&self.heat);
        heat.block(&HeatmapEvaluatedPropsUBO {
            intensity: *paint.heatmap_intensity.uniform(),
            ..HeatmapEvaluatedPropsUBO::default()
        });
        for (tile, _, mesh) in cx.tiles() {
            let at = tile.project(cx.frame, &TileTransform::default());
            let draw = HeatmapDrawableUBO {
                matrix: at.fallback_matrix,
                extrude_scale: at.pixels_to_tile_units,
                globe_extrude_scale: at.globe_extrude_scale,
                ..HeatmapDrawableUBO::default()
            };
            heat.push_at(mesh, &at, Draw { depth: Depth::Off, ..Draw::DEFAULT }, draw);
        }
        let size = cx.frame.target_size;
        let matrix = orthographic(0.0, f64::from(size.width), f64::from(size.height), 0.0, -1.0, 1.0);
        out.main()
            .block(&HeatmapTexturePropsUBO {
                matrix: matrix.as_mat4().to_cols_array(),
                opacity: *paint.heatmap_opacity.uniform(),
                ..HeatmapTexturePropsUBO::default()
            })
            .push(&self.colorize, Draw::DEFAULT, ());
    }
}
