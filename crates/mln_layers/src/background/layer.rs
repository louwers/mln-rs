use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::segment::SegmentBase;
use mln_render::draw::{Buffer, Depth, Draw, Draws, Mesh, MeshDesc, Resources};
use mln_render::globe::tile_grid;
use mln_render::layer::{
    EvaluateCx, Evaluation, FrameTile, Layer, NewCx, PrepareCx, TileCx, TileTransform, UpdateCx, ViewCover,
};
use mln_style_types::TranslateAnchorType;
use mln_style_types::image::ImagePosition;
use mln_util::constants::{EXTENT, TILE_SIZE_D};

use super::Background;
use super::pattern_atlas::PatternAtlas;
use super::programs::{BACKGROUND, BACKGROUND_PATTERN};
use super::ubo::{BackgroundDrawableUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO, BackgroundPropsUBO};

/// A background layer: a color or a pattern over each tile that covers the
/// view.
pub struct BackgroundLayer {
    /// The patterns of `background-pattern`.
    patterns: PatternAtlas,
    /// Where the frame's patterns are in the atlas, from and to, when it
    /// draws a pattern.
    pattern: Option<(ImagePosition, ImagePosition)>,
    /// A tile's four corners and its two triangles, which every tile draws
    /// with on Mercator.
    quad: Geometry,
    /// Whether the last evaluation found the layer opaque.
    opaque: bool,
}

/// A tile's meshes: of its color and of its pattern.
pub struct BackgroundTile {
    color: Mesh<BackgroundDrawableUBO>,
    pattern: Mesh<BackgroundPatternDrawableUBO>,
}

/// The vertices and triangles that a tile draws with.
struct Geometry {
    vertices: Buffer<[i16; 2]>,
    triangles: Buffer<u16>,
    segments: Vec<SegmentBase>,
}

impl Geometry {
    fn new(res: &mut Resources, vertices: &[[i16; 2]], triangles: &[u16]) -> Self {
        let segments = vec![SegmentBase::with_lengths(0, 0, vertices.len(), triangles.len())];
        Geometry { vertices: res.buffer(vertices), triangles: res.buffer(triangles), segments }
    }

    fn mesh<'a>(&'a self, program: &'static Program) -> MeshDesc<'a> {
        let mut mesh = MeshDesc::new(program);
        mesh.vertices(&self.vertices).indexes(&self.triangles, &self.segments);
        mesh
    }
}

impl Layer for BackgroundLayer {
    type Style = Background;
    type Input = ViewCover;
    type Tile = BackgroundTile;

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        let e = EXTENT as i16;
        BackgroundLayer {
            patterns: PatternAtlas::new(cx.res),
            pattern: None,
            quad: Geometry::new(cx.res, &[[0, 0], [e, 0], [0, e], [e, e]], &[0, 1, 2, 1, 2, 3]),
            opaque: false,
        }
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
        self.opaque = evaluation.opaque;
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
        // A background isn't depth tested, so on the globe its grid needs no
        // border.
        let grid = cx.frame.camera.is_globe().then(|| {
            let grid = tile_grid(&tile.id().canonical);
            Geometry::new(cx.res, &grid.vertices, grid.triangles.as_flattened())
        });
        let geometry = grid.as_ref().unwrap_or(&self.quad);
        let color = cx.res.mesh(&geometry.mesh(&BACKGROUND));
        let pattern = cx.res.mesh(geometry.mesh(&BACKGROUND_PATTERN).texture(&self.patterns.texture));
        Some(BackgroundTile { color, pattern })
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let paint = &cx.layer.evaluated.paint;
        let opacity = *paint.background_opacity.uniform();
        let has_pattern = !paint.background_pattern.faded().to.empty();
        let how = TileTransform { anchor: TranslateAnchorType::Viewport, ..TileTransform::default() };
        let main = out.main();
        if !has_pattern {
            let color = paint.background_color.uniform().to_float_array();
            main.block(&BackgroundPropsUBO { color, opacity, ..BackgroundPropsUBO::default() });
            let draw = Draw { opaque: self.opaque, depth: Depth::Layer { sub_layer: 0, write: true }, ..Draw::DEFAULT };
            for (tile, (), meshes) in cx.tiles() {
                let at = tile.project(cx.frame, &how);
                main.push_at(&meshes.color, &at, draw, BackgroundDrawableUBO { matrix: at.fallback_matrix });
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
        let zoom = cx.frame.camera.integer_zoom();
        for (tile, (), meshes) in cx.tiles() {
            let at = tile.project(cx.frame, &how);
            let id = tile.unwrapped_id();
            let tile_size =
                (TILE_SIZE_D * mln_map_types::zoom_scale(f64::from(zoom) - f64::from(id.canonical.z))) as i32;
            let x = (f64::from(tile_size)
                * (f64::from(id.canonical.x)
                    + f64::from(id.wrap) * mln_map_types::zoom_scale(f64::from(id.canonical.z))))
                as i32;
            let y = tile_size.wrapping_mul(id.canonical.y as i32);
            let pixels_to_tile_units = id.pixels_to_tile_units(1.0, f32::from(zoom));
            let block = BackgroundPatternDrawableUBO {
                matrix: at.fallback_matrix,
                pixel_coord_upper: [(x >> 16) as f32, (y >> 16) as f32],
                pixel_coord_lower: [(x & 0xFFFF) as f32, (y & 0xFFFF) as f32],
                tile_units_to_pixels: if pixels_to_tile_units == 0.0 { 0.0 } else { 1.0 / pixels_to_tile_units },
                ..BackgroundPatternDrawableUBO::default()
            };
            main.push_at(&meshes.pattern, &at, draw, block);
        }
    }
}
