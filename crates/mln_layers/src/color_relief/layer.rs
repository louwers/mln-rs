use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::gpu::TextureFormat;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::TextureFilterType;
use mln_render::draw::{Draw, Draws, Mesh, MeshDesc, Resources, Sampler, Texture};
use mln_render::layer::{
    DemTile, DemTiles, EvaluateCx, Evaluation, FrameTile, Layer, NewCx, PrepareCx, TileCx, TileTransform, UpdateCx,
};
use mln_style::color_ramp_property_value::ColorRampPropertyValue;
use mln_style::expression::{EvaluationContext, GlobalStateMap, from_expression_value};
use mln_style_types::ResamplingType;
use mln_util::Color;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use super::ColorRelief;
use super::programs::COLOR_RELIEF;
use super::ubo::{ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO};
use crate::color_ramp;

/// A color-relief layer: each tile's terrain, colored as the stops of
/// `color-relief-color` say for its elevation.
pub struct ColorReliefLayer {
    /// The stops, and those the textures hold.
    stops: Rc<Stops>,
    written: Rc<Stops>,
    /// The global state the stops were made with.
    state: Option<Arc<GlobalStateMap>>,
    /// The stops' elevations, a texel each, and their colors.
    elevations: Texture,
    colors: Texture,
    /// How the tiles sample their elevations: as `resampling` says.
    resampling: Sampler,
}

/// The stops of a color ramp of elevations: each one's elevation, in the
/// red of a texel, and its color, in a row of pixels.
struct Stops {
    elevations: Vec<[f32; 4]>,
    colors: PremultipliedImage,
}

type ReliefBlocks = (ColorReliefDrawableUBO, ColorReliefTilePropsUBO);

/// How the stops' elevations are sampled: texel by texel.
const ELEVATIONS: SamplerState = SamplerState {
    filter: TextureFilterType::Nearest,
    wrap_u: mln_gfx::types::TextureWrapType::Clamp,
    wrap_v: mln_gfx::types::TextureWrapType::Clamp,
    max_anisotropy: 1,
    mipmapped: false,
};

impl Layer for ColorReliefLayer {
    type Style = ColorRelief;
    type Input = DemTiles;
    type Tile = Mesh<ReliefBlocks>;

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        let value = cx.layer.unevaluated.color_relief_color.get_value();
        let stops = Rc::new(Stops::of(value, None).unwrap_or_else(|| Stops::transparent(256)));
        let (elevations, colors) = (cx.res.texture(|_| ()), cx.res.texture(|_| ()));
        let mut layer = ColorReliefLayer {
            written: Rc::clone(&stops),
            stops,
            state: None,
            elevations,
            colors,
            resampling: Sampler::new(SamplerState::default()),
        };
        layer.write(cx.res);
        layer
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let value = cx.layer.unevaluated.color_relief_color.get_value();
        if color_ramp::changed(value, &mut self.state, cx.parameters)
            && let Some(stops) = Stops::of(value, self.state.as_deref())
        {
            self.stops = Rc::new(stops);
        }
        if *cx.evaluated.paint.color_relief_opacity.uniform() > 0.0 {
            Evaluation::TRANSLUCENT
        } else {
            Evaluation::NONE
        }
    }

    fn update(&mut self, cx: &mut UpdateCx<'_, Self>) {
        let filter = match *cx.layer.evaluated.paint.resampling.uniform() {
            ResamplingType::Linear => TextureFilterType::Linear,
            ResamplingType::Nearest => TextureFilterType::Nearest,
        };
        self.resampling.set(SamplerState { filter, ..SamplerState::default() });
        if !Rc::ptr_eq(&self.written, &self.stops) {
            self.write(cx.res);
        }
    }

    fn tile(&mut self, cx: &mut TileCx<'_, Self>, _tile: FrameTile, dem: DemTile<'_>) -> Option<Self::Tile> {
        let surface = dem.surface(cx.res, cx.frame);
        let mut desc = MeshDesc::new(&COLOR_RELIEF, &surface.triangles);
        desc.vertices(&surface.vertices)
            .texture(dem.texture(), &self.resampling)
            .texture(&self.elevations, ELEVATIONS)
            .texture(&self.colors, SamplerState { filter: TextureFilterType::Linear, ..SamplerState::default() });
        Some(cx.res.mesh(&desc))
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let main = out.main();
        let opacity = *cx.layer.evaluated.paint.color_relief_opacity.uniform();
        main.block(&ColorReliefEvaluatedPropsUBO { opacity, ..ColorReliefEvaluatedPropsUBO::default() });
        for (tile, dem, mesh) in cx.tiles() {
            let at = tile.project(cx.frame, &TileTransform::default());
            // The elevations' image has a border of a texel.
            let stride = dem.image().size.width as f32;
            let props = ColorReliefTilePropsUBO {
                unpack: dem.unpack(),
                dimension: [stride, stride],
                color_ramp_size: self.stops.elevations.len() as i32,
                ..ColorReliefTilePropsUBO::default()
            };
            main.push_at(mesh, &at, Draw::DEFAULT, (ColorReliefDrawableUBO { matrix: at.fallback_matrix }, props));
        }
    }
}

impl ColorReliefLayer {
    /// Writes the stops into their textures.
    fn write(&mut self, res: &mut Resources) {
        let stops = &self.stops;
        let size = Size::new(stops.elevations.len() as u32, 1);
        let texels: Rc<[u8]> = bytemuck::cast_slice(&stops.elevations).into();
        if let Some(mut texture) = res.texture_mut(&self.elevations) {
            // RGBA rather than red alone, which llvmpipe can't sample.
            texture.set_texels(TextureFormat::Rgba32Float, size, texels);
        }
        if let Some(mut texture) = res.texture_mut(&self.colors) {
            texture.set_image(Some(Rc::new(stops.colors.clone())));
        }
        self.written = Rc::clone(&self.stops);
    }
}

impl Stops {
    /// `count` transparent stops at elevation 0.
    fn transparent(count: u32) -> Stops {
        Stops { elevations: vec![[0.0; 4]; count as usize], colors: PremultipliedImage::new(Size::new(count, 1)) }
    }

    /// The stops of `value`, with `global_state` for its `global-state`
    /// expressions: those of an `interpolate` expression, or else the ramp
    /// sampled from -500 m to 9000 m. `None` for a value without stops.
    fn of(value: &ColorRampPropertyValue, global_state: Option<&GlobalStateMap>) -> Option<Stops> {
        let expression = value.get_expression()?;
        let (elevations, colors): (Vec<f32>, Vec<Color>) = if let Some(interpolate) = expression.as_interpolate() {
            let mut elevations = Vec::with_capacity(interpolate.get_stop_count());
            interpolate.each_stop(|elevation, _| elevations.push(elevation as f32));
            let colors = elevations
                .iter()
                .map(|&elevation| {
                    let context = EvaluationContext::with_color_ramp(None, None, Some(f64::from(elevation)))
                        .with_global_state(global_state);
                    let color = expression.evaluate(&context).ok().and_then(|value| from_expression_value(&value));
                    color.unwrap_or(Color::new(0.0, 0.0, 0.0, 0.0))
                })
                .collect();
            (elevations, colors)
        } else {
            const SAMPLES: u32 = 256;
            const LOWEST: f32 = -500.0;
            const HIGHEST: f32 = 9000.0;
            let elevations: Vec<f32> =
                (0..SAMPLES).map(|i| LOWEST + i as f32 / (SAMPLES - 1) as f32 * (HIGHEST - LOWEST)).collect();
            let colors =
                elevations.iter().map(|&elevation| value.evaluate_ramp(f64::from(elevation), global_state)).collect();
            (elevations, colors)
        };
        if elevations.is_empty() {
            return None;
        }
        let mut stops = Stops::transparent(elevations.len() as u32);
        for ((texel, elevation), (pixel, color)) in stops
            .elevations
            .iter_mut()
            .zip(elevations)
            .zip(stops.colors.data.as_chunks_mut::<4>().0.iter_mut().zip(colors))
        {
            *texel = [elevation, 0.0, 0.0, 1.0];
            // Colors as they are, not premultiplied, to interpolate.
            *pixel = [color.r, color.g, color.b, color.a].map(|channel| (channel * 255.0) as u8);
        }
        Some(stops)
    }
}
