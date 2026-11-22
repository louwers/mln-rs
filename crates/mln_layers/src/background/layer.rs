use mln_render::draw::{Depth, Draw, Draws, Mesh, MeshDesc};
use mln_render::layer::{EvaluateCx, Evaluation, FrameTile, Layer, NewCx, PrepareCx, TileCx, UpdateCx, ViewCover};
use mln_style_types::image::ImagePosition;

use super::Background;
use super::pattern_atlas::PatternAtlas;
use super::programs::{BACKGROUND, BACKGROUND_PATTERN};
use super::ubo::{BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO, BackgroundPropsUBO};

/// A background layer: a color or a pattern over each tile that covers the
/// view.
pub struct BackgroundLayer {
    /// The patterns of `background-pattern`.
    patterns: PatternAtlas,
    /// Where the frame's patterns are in the atlas, from and to, when it
    /// draws a pattern.
    pattern: Option<(ImagePosition, ImagePosition)>,
}

/// A tile's meshes: of its color and of its pattern.
pub struct BackgroundTile {
    color: Mesh,
    pattern: Mesh<BackgroundPatternDrawableUBO>,
}

impl Layer for BackgroundLayer {
    type Style = Background;
    type Input = ViewCover;
    type Tile = BackgroundTile;

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        BackgroundLayer { patterns: PatternAtlas::new(cx.res), pattern: None }
    }

    /// Draws nothing where the layer is transparent, and opaque where it
    /// is. Without a pattern, the layer is one solid color.
    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let paint = &cx.evaluated.paint;
        let opacity = *paint.background_opacity.uniform();
        let color = *paint.background_color.uniform();
        let evaluation = if opacity == 0.0 {
            Evaluation::NONE
        } else if !cx.layer.unevaluated.background_pattern.is_undefined() || opacity < 1.0 || color.a < 1.0 {
            Evaluation::TRANSLUCENT
        } else {
            Evaluation::OPAQUE
        };
        let pattern = !paint.background_pattern.faded().from.empty();
        let solid = if pattern || opacity <= 0.0 { None } else { Some(color * opacity) };
        Evaluation { solid, ..evaluation }
    }

    /// Packs the patterns, again those whose images the style replaced.
    fn update(&mut self, cx: &mut UpdateCx<'_, Self>) {
        let faded = cx.layer.evaluated.paint.background_pattern.faded();
        self.pattern = None;
        if !faded.to.empty() {
            let patterns = &mut self.patterns;
            patterns.remove_stale(cx.images);
            patterns.add(faded.from.id(), cx.images);
            patterns.add(faded.to.id(), cx.images);
            patterns.upload(cx.res);
            // A pattern that isn't in the atlas draws nothing.
            self.pattern = patterns.pattern(faded.from.id()).zip(patterns.pattern(faded.to.id()));
        }
    }

    fn tile(&mut self, cx: &mut TileCx<'_, Self>, tile: FrameTile, (): ()) -> Option<BackgroundTile> {
        let surface = cx.res.tile_surface(tile, cx.frame);
        let mut color = MeshDesc::new(&BACKGROUND, &surface.triangles);
        color.vertices(&surface.vertices);
        let mut pattern = MeshDesc::new(&BACKGROUND_PATTERN, &surface.triangles);
        pattern.vertices(&surface.vertices).texture(&self.patterns.texture, PatternAtlas::SAMPLER);
        Some(BackgroundTile { color: cx.res.mesh(&color), pattern: cx.res.mesh(&pattern) })
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let paint = &cx.layer.evaluated.paint;
        let opacity = *paint.background_opacity.uniform();
        let has_pattern = !paint.background_pattern.faded().to.empty();
        let main = out.main();
        if !has_pattern {
            let color = paint.background_color.uniform().to_float_array();
            main.block(&BackgroundPropsUBO { color, opacity, ..BackgroundPropsUBO::default() });
            let draw = Draw { opaque: true, depth: Depth::Layer { sub_layer: 0, write: true }, ..Draw::DEFAULT };
            for (tile, (), meshes) in cx.tiles() {
                main.push_at(&meshes.color, &tile.project(cx.frame, &Default::default()), draw, ());
            }
            return;
        }
        let Some((from, to)) = &self.pattern else { return };
        let crossfade = &cx.layer.evaluated.crossfade;
        let size = self.patterns.size();
        main.block(&BackgroundPatternPropsUBO {
            pattern_tl_a: from.tl().map(f32::from),
            pattern_br_a: from.br().map(f32::from),
            pattern_tl_b: to.tl().map(f32::from),
            pattern_br_b: to.br().map(f32::from),
            pattern_size_a: from.display_size(),
            pattern_size_b: to.display_size(),
            scale_a: crossfade.from_scale,
            scale_b: crossfade.to_scale,
            mix: crossfade.t,
            opacity,
            texsize: [size.width as f32, size.height as f32],
            ..BackgroundPatternPropsUBO::default()
        });
        let draw = Draw { depth: Depth::Layer { sub_layer: 0, write: true }, ..Draw::DEFAULT };
        for (tile, (), meshes) in cx.tiles() {
            let origin = tile.pattern_origin(cx.frame);
            let block = BackgroundPatternDrawableUBO {
                pixel_coord_upper: origin.upper,
                pixel_coord_lower: origin.lower,
                tile_units_to_pixels: origin.tile_units_to_pixels,
                ..BackgroundPatternDrawableUBO::default()
            };
            main.push_at(&meshes.pattern, &tile.project(cx.frame, &Default::default()), draw, block);
        }
    }
}
