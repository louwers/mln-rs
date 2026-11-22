use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::TextureFilterType;
use mln_render::draw::{Composite, Draw, Draws, Mesh, MeshDesc, Sampler};
use mln_render::layer::{
    EvaluateCx, Evaluation, FrameTile, Layer, NewCx, PrepareCx, RasterTile, RasterTiles, TileCx, TileTransform,
    UpdateCx,
};
use mln_style_types::{RasterResamplingType, ResamplingType};

use super::programs::RASTER;
use super::ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use super::{Raster, RasterPaintProperties};

/// A raster layer: each tile's image, or an image source's image in each of
/// its tiles.
pub struct RasterLayer {
    /// How the tiles sample their images: as the resampling properties say.
    resampling: Sampler,
}

impl Layer for RasterLayer {
    type Style = Raster;
    type Input = RasterTiles;
    type Tile = Mesh<RasterDrawableUBO>;

    fn new(_cx: &mut NewCx<'_, Self>) -> Self {
        RasterLayer { resampling: Sampler::new(SamplerState::default()) }
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        if *cx.evaluated.paint.raster_opacity.uniform() > 0.0 { Evaluation::TRANSLUCENT } else { Evaluation::NONE }
    }

    fn update(&mut self, cx: &mut UpdateCx<'_, Self>) {
        let paint = &cx.layer.evaluated.paint;
        // Either property asking for nearest samples nearest.
        let nearest = *paint.raster_resampling.uniform() == RasterResamplingType::Nearest
            || *paint.resampling.uniform() == ResamplingType::Nearest;
        let filter = if nearest { TextureFilterType::Nearest } else { TextureFilterType::Linear };
        self.resampling.set(SamplerState { filter, ..SamplerState::default() });
    }

    fn tile(&mut self, cx: &mut TileCx<'_, Self>, _tile: FrameTile, raster: RasterTile<'_>) -> Option<Self::Tile> {
        let surface = raster.surface(cx.res, cx.frame);
        let mut desc = MeshDesc::new(&RASTER, &surface.triangles);
        desc.vertices(&surface.vertices)
            .texture(raster.texture(), &self.resampling)
            .texture(raster.texture(), &self.resampling);
        Some(cx.res.mesh(&desc))
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let paint = &cx.layer.evaluated.paint;
        let main = out.main();
        if let Some(composite) = Composite::of(1.0, *paint.raster_layer_blend.uniform()) {
            main.composite(composite);
        }
        main.block(&props(paint));
        // On the pixel grid while the camera stands still.
        let how = TileTransform { aligned: !cx.frame.camera_moving, ..TileTransform::default() };
        for (tile, _, mesh) in cx.tiles() {
            let at = tile.project(cx.frame, &how);
            main.push_at(mesh, &at, Draw::DEFAULT, RasterDrawableUBO { matrix: at.fallback_matrix });
        }
    }
}

/// The block of `paint` that the layer's draws share.
fn props(paint: &RasterPaintProperties::PossiblyEvaluated) -> RasterEvaluatedPropsUBO {
    let spin = mln_math::deg2radf(*paint.raster_hue_rotate.uniform());
    let (s, c, sqrt3) = (spin.sin(), spin.cos(), 3.0f32.sqrt());
    let saturation = *paint.raster_saturation.uniform();
    let contrast = *paint.raster_contrast.uniform();
    RasterEvaluatedPropsUBO {
        spin_weights: [(2.0 * c + 1.0) / 3.0, (-sqrt3 * s - c + 1.0) / 3.0, (sqrt3 * s - c + 1.0) / 3.0, 0.0],
        tl_parent: [0.0, 0.0],
        scale_parent: 1.0,
        buffer_scale: 1.0,
        fade_t: 1.0,
        opacity: *paint.raster_opacity.uniform(),
        brightness_low: *paint.raster_brightness_min.uniform(),
        brightness_high: *paint.raster_brightness_max.uniform(),
        saturation_factor: if saturation > 0.0 { 1.0 - 1.0 / (1.001 - saturation) } else { -saturation },
        contrast_factor: if contrast > 0.0 { 1.0 / (1.0 - contrast) } else { 1.0 + contrast },
        ..RasterEvaluatedPropsUBO::default()
    }
}
