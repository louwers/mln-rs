use std::sync::Arc;

use glam::dcamera::rh::proj::opengl::orthographic;
use glam::{DMat4, DVec3};
use mln_gfx::color_mode::ColorMode;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TextureWrapType};
use mln_map_types::View;
use mln_math::deg2radf;
use mln_render::draw::{Buffer, Draw, Draws, ImageVertex, IndexBuffer, Mesh, MeshDesc, Sampler, Target};
use mln_render::layer::{
    DemTile, DemTiles, EvaluateCx, EvaluatedLayer, Evaluation, FrameTile, Layer, NewCx, PrepareCx, TileCx,
    TileTransform, UpdateCx,
};
use mln_style_types::HillshadeIlluminationAnchorType;
use mln_tile_id::UnwrappedTileID;
use mln_util::Color;
use mln_util::constants::{EXTENT, TERRAIN_RGB_MAXZOOM};
use mln_util::geo::LatLng;
use mln_util::size::Size;

use super::Hillshade;
use super::programs::{HILLSHADE, HILLSHADE_PREPARE};
use super::properties::paint;
use super::ubo::{
    HillshadeDrawableUBO, HillshadeEvaluatedPropsUBO, HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO,
    HillshadeTilePropsUBO, MAX_ILLUMINATION_SOURCES,
};
use crate::resampling;

/// A hillshade layer: each tile's slopes, prepared once from its elevations
/// into a target of the tile's, and shaded as lit from the layer's light
/// sources.
pub struct HillshadeLayer {
    /// The tile's quad, which prepares its slopes.
    corners: Buffer<ImageVertex>,
    quad: IndexBuffer,
    /// How the tiles sample their slopes: as `resampling` says.
    resampling: Sampler,
    /// The deepest zoom of the source's own tiles, which the slopes scale
    /// to.
    max_zoom: u8,
    /// The light sources of the evaluation that the tiles last drew with,
    /// before a viewport anchor turns them by the frame's bearing.
    lights: Option<(Arc<EvaluatedLayer<Hillshade>>, Lights)>,
}

/// What the layer keeps of a tile: its slopes, the mesh that prepares them,
/// and the mesh that shades them.
pub struct HillshadeTile {
    slopes: Target,
    prepare: Mesh<(HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO)>,
    shade: Mesh<(HillshadeDrawableUBO, HillshadeTilePropsUBO)>,
}

/// The light sources, as the shader reads them, and how many it reads, and
/// whether they turn with the map.
#[derive(Clone, Copy)]
struct Lights {
    props: HillshadeEvaluatedPropsUBO,
    count: i32,
    viewport: bool,
}

/// How the slopes are prepared from the elevations: texel by texel.
const ELEVATIONS: SamplerState = SamplerState {
    filter: TextureFilterType::Nearest,
    wrap_u: TextureWrapType::Clamp,
    wrap_v: TextureWrapType::Clamp,
    max_anisotropy: 1,
    mipmapped: false,
};

impl Layer for HillshadeLayer {
    type Style = Hillshade;
    type Input = DemTiles;
    type Tile = HillshadeTile;

    /// A tile's slopes and meshes are made of its DEM, which the layer's
    /// resampling sampler samples them with.
    const RESTYLE_KEEPS_TILES: bool = true;

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        let e = EXTENT as i16;
        let corners =
            [[0, 0], [e, 0], [0, e], [e, e]].map(|p| ImageVertex { position: p, texture: p.map(|c| c as u16) });
        HillshadeLayer {
            corners: cx.res.buffer(&corners),
            quad: cx.res.index_buffer(&[0, 1, 2, 1, 2, 3]),
            resampling: Sampler::new(ELEVATIONS),
            max_zoom: TERRAIN_RGB_MAXZOOM,
            lights: None,
        }
    }

    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        Evaluation::translucent_if(*cx.evaluated.paint.hillshade_exaggeration.uniform() > 0.0)
    }

    fn update(&mut self, cx: &mut UpdateCx<'_, Self>) {
        let paint = &cx.layer.evaluated.paint;
        self.resampling.set(SamplerState { filter: resampling::filter(*paint.resampling.uniform()), ..ELEVATIONS });
        if let Some(max_zoom) = cx.source_max_zoom {
            self.max_zoom = max_zoom;
        }
        let evaluated = &cx.layer.evaluated;
        if !self.lights.as_ref().is_some_and(|(of, _)| Arc::ptr_eq(of, evaluated))
            && cx.tiles.iter().any(|(_, dem, _)| dem.is_some())
        {
            self.lights = Some((Arc::clone(evaluated), Lights::of(&evaluated.paint)));
        }
    }

    fn tile(&mut self, cx: &mut TileCx<'_, Self>, _tile: FrameTile, dem: DemTile<'_>) -> Option<HillshadeTile> {
        let res = &mut *cx.res;
        let mut desc = MeshDesc::new(&HILLSHADE_PREPARE, &self.quad);
        desc.vertices(&self.corners).texture(dem.texture(), ELEVATIONS).blend(ColorMode::unblended()).screen_space();
        let prepare = res.mesh(&desc);
        let size = Size::new(dem.size(), dem.size());
        let slopes = res.target(size, TextureChannelDataType::UnsignedByte, Color::new(0.0, 0.0, 0.0, 1.0));
        let surface = dem.surface(res, cx.frame);
        let shade = {
            let mut desc = MeshDesc::new(&HILLSHADE, &surface.triangles);
            desc.vertices(&surface.vertices).texture(slopes.texture(), &self.resampling);
            res.mesh(&desc)
        };
        Some(HillshadeTile { slopes, prepare, shade })
    }

    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let Some((_, lights)) = &self.lights else { return };
        let extent = f64::from(EXTENT);
        let to_texture =
            orthographic(0.0, extent, -extent, 0.0, -1.0, 1.0) * DMat4::from_translation(DVec3::new(0.0, -extent, 0.0));
        let to_texture = HillshadePrepareDrawableUBO { matrix: to_texture.as_mat4().to_cols_array() };
        // The slopes depend on nothing but the tile's elevations and zoom, so
        // a target that holds them keeps them.
        for (tile, dem, value) in cx.tiles().filter(|(_, _, value)| !cx.res.is_drawn(&value.slopes)) {
            let stride = dem.image().size.width as f32;
            let props = HillshadePrepareTilePropsUBO {
                unpack: dem.unpack(),
                dimension: [stride, stride],
                zoom: f32::from(tile.id().canonical.z),
                maxzoom: f32::from(self.max_zoom),
            };
            out.target(&value.slopes).push(&value.prepare, Draw::DEFAULT, (to_texture, props));
        }
        let paint = &cx.layer.evaluated.paint;
        let main = out.main();
        main.block(&lights.turned(cx.frame.camera));
        // Aligned to the pixel grid at rest only: a moving camera moves the
        // shading smoothly, as it does the layers around it.
        let how = TileTransform { aligned: !cx.frame.camera_moving, ..TileTransform::default() };
        for (tile, _, value) in cx.tiles() {
            let at = tile.project(cx.frame, &how);
            let props = HillshadeTilePropsUBO {
                latrange: latitudes(tile.unwrapped_id()),
                exaggeration: *paint.hillshade_exaggeration.uniform(),
                method: *paint.hillshade_method.uniform() as i32,
                num_lights: lights.count,
                ..HillshadeTilePropsUBO::default()
            };
            main.push_at(&value.shade, &at, Draw::DEFAULT, (HillshadeDrawableUBO::default(), props));
        }
    }
}

impl Lights {
    /// The light sources of `paint` on a map turned to the north, up to the
    /// most the shader reads: each property's values, its last one repeated
    /// for the sources that it has no value of.
    fn of(paint: &paint::PossiblyEvaluated) -> Lights {
        let directions = paint.hillshade_illumination_direction.uniform();
        let altitudes = paint.hillshade_illumination_altitude.uniform();
        let highlights = paint.hillshade_highlight_color.uniform();
        let shadows = paint.hillshade_shadow_color.uniform();
        let sources = [directions.len(), altitudes.len(), highlights.len(), shadows.len()]
            .into_iter()
            .max()
            .unwrap_or(0)
            .min(MAX_ILLUMINATION_SOURCES);
        let mut props = HillshadeEvaluatedPropsUBO {
            accent: paint.hillshade_accent_color.uniform().to_float_array(),
            ..HillshadeEvaluatedPropsUBO::default()
        };
        for (i, direction) in padded(directions, sources).take(4).enumerate() {
            props.azimuths[i] = deg2radf(direction);
        }
        for (i, altitude) in padded(altitudes, sources).take(4).enumerate() {
            props.altitudes[i] = deg2radf(altitude);
        }
        for (i, color) in padded(shadows, sources).take(4).enumerate() {
            props.shadows[i] = color.to_float_array();
        }
        for (i, color) in padded(highlights, sources).take(4).enumerate() {
            props.highlights[i] = color.to_float_array();
        }
        let viewport = *paint.hillshade_illumination_anchor.uniform() == HillshadeIlluminationAnchorType::Viewport;
        Lights { props, count: padded(directions, sources).count() as i32, viewport }
    }

    /// The light sources as `camera` sees them: those anchored to the
    /// viewport turn with the map, by its bearing.
    fn turned(&self, camera: &View) -> HillshadeEvaluatedPropsUBO {
        let mut props = self.props;
        if self.viewport {
            let bearing = camera.angle() as f32;
            let lights = (self.count as usize).min(props.azimuths.len());
            for azimuth in &mut props.azimuths[..lights] {
                *azimuth -= bearing;
            }
        }
        props
    }
}

/// `values`, for at least `sources` sources and one, the last value for
/// those past it.
fn padded<T: Clone + Default>(values: &[T], sources: usize) -> impl Iterator<Item = T> + '_ {
    let last = values.last().cloned().unwrap_or_default();
    (0..sources.max(values.len()).max(1)).map(move |i| values.get(i).cloned().unwrap_or_else(|| last.clone()))
}

/// The latitudes of the top and bottom edges of `tile`.
fn latitudes(tile: UnwrappedTileID) -> [f32; 2] {
    let id = tile.canonical;
    let top = LatLng::from_unwrapped_tile(id.z, id.x, id.y, tile.wrap);
    let below = UnwrappedTileID::new(id.z, i64::from(id.x), i64::from(id.y) + 1);
    let bottom = LatLng::from_unwrapped_tile(below.canonical.z, below.canonical.x, below.canonical.y, below.wrap);
    [top.latitude() as f32, bottom.latitude() as f32]
}
