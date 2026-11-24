//! The drapes: a texture for each terrain tile and stack of draped layers,
//! which the stack's layers draw into, flat, as a small frame of their own,
//! and which the tile's draw of the stack samples.
//!
//! A drape draws the frame's layers with the map's camera, so zoom, bearing
//! and line widths are the map's, through a projection of the tile's square
//! of the world onto its texture ([`TransformParameters::for_drape`]). On the
//! globe the camera is the map's on Mercator ([`View::mercator`]), as GL JS
//! draws its drapes, so the layers draw as flat as on a Mercator map. A
//! drape draws the tiles over its own tile: on Mercator those of its world
//! copy; on the globe, which has no copies, those of any, in its own, so
//! that near the antimeridian it holds the tiles that the cover puts in the
//! next copy. Each
//! draped layer keeps a drawing of its own in each drape of its stack, made
//! by the layer's own draw update ([`LayerParts::update_drape_draws`]), with
//! the drape's uniforms, clip masks and stencil. Every frame draws every
//! drape again. A drape whose layers make no draw has no texture, so a stack
//! that draws nothing in a tile holds no memory there.
//!
//! A draped layer with a layer opacity below 1 or a layer blend other than
//! `normal` draws as a whole inside each drape, as in 2D
//! ([`LayerComposite`]): into a texture of the drape's size, composited onto
//! the drape. The drapes share one such composite, which each drape's pass
//! uses in turn. A drape with a layer that blends in the shader draws into
//! the composite's frame texture, which is copied into the drape before its
//! mips, so a blend's backdrop is the drape's own texels.
//!
//! [`LayerParts::update_drape_draws`]: crate::render_layer::LayerParts::update_drape_draws

use std::sync::Arc;

use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, LoadOp, TargetFormats, TextureFormat};
use mln_gfx::shaders::shader_defines::ID_GLOBAL_PAINT_PARAMS_UBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_map_types::{MapDebugOptions, View};
use mln_style::layer_type::ErasedLayerStyle;
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::size::Size;

use super::TerrainCover;
use crate::clip_masks::ClipMasks;
use crate::gfx::context::Context;
use crate::gfx::resources::GpuTexture;
use crate::gfx::store::SharedTexture;
use crate::layer_composite::LayerComposite;
use crate::layer_drawing::{ChangedBuckets, DrapeDrawing, DrawGroups, DrawOrder};
use crate::paint_parameters::{OpenPass, PaintParameters, TransformParameters};
use crate::render_layer::{Atlases, LayerSource, RenderLayer};
use crate::render_pass::RenderPass;
use crate::tile_matrix::to_projection_ubo;

/// The format of a drape's texture.
const COLOR: TextureFormat = TextureFormat::Rgba8Unorm;
/// The format of the depth and stencil the drapes draw with.
const DEPTH_STENCIL: TextureFormat = TextureFormat::Depth24PlusStencil8;
/// The most samples the terrain takes of a drape along a pixel's footprint
/// (GL JS's).
const MAX_ANISOTROPY: u8 = 16;

/// The side in texels of the drapes of terrain tiles of `tile_size` pixels,
/// on a device whose textures have at most `max_texture_size` texels a side:
/// twice as wide, whatever the pixel ratio (GL JS's quality factor), or the
/// device's largest.
pub(crate) fn drape_size(tile_size: u16, max_texture_size: u32) -> u32 {
    (u32::from(tile_size) * 2).min(max_texture_size).max(1)
}

/// A drawing of a layer that a drape keeps, with the style it was made for.
struct DrapeLayer {
    style: Arc<dyn ErasedLayerStyle>,
    drawing: DrapeDrawing,
}

/// The drape of a terrain tile for one stack of draped layers.
#[derive(Default)]
struct StackDrape {
    /// The texture its layers draw into; `None` while they draw nothing in
    /// the tile.
    texture: Option<SharedTexture>,
    /// The stack's layers' drawings, in drawing order.
    layers: Vec<DrapeLayer>,
    /// The drawings of the last frame that this frame's layers haven't
    /// taken yet, which go once it has updated them.
    previous: Vec<DrapeLayer>,
    /// The draw groups of the layers' drawings, which set the pass's depths
    /// and the masks it clips to.
    groups: DrawGroups,
}

/// The drapes of a terrain tile, one for each stack, which share the clip
/// masks of the tile's projection.
struct Drape {
    tile: OverscaledTileID,
    transform: TransformParameters,
    clip_masks: ClipMasks,
    stacks: Vec<StackDrape>,
}

impl Drape {
    /// The drapes of `tile` in `view`.
    fn new(view: &View, tile: OverscaledTileID) -> Drape {
        Drape {
            tile,
            transform: TransformParameters::for_drape(view, &tile.to_unwrapped()),
            clip_masks: ClipMasks::default(),
            stacks: Vec::new(),
        }
    }
}

/// The drapes of the terrain tiles of a frame, in the cover's order.
#[derive(Default)]
pub(crate) struct Drapes {
    drapes: Vec<Drape>,
    /// The side of the drapes' textures, in texels.
    size: u32,
    /// Whether the frame's view is on the globe, where a drape draws the
    /// tiles over its own whatever their world copy.
    globe: bool,
    /// The depth and stencil every drape draws with in turn, of `size`.
    depth_stencil: Option<GpuTexture>,
    /// What the layers drawn as a whole draw into and composite with, of
    /// `size`, which every drape's pass uses in turn.
    layer_composite: LayerComposite,
}

impl Drapes {
    /// Starts a frame's update of the drapes, those of `cover`'s tiles in
    /// `view`, each with `stacks` stacks, on a device whose textures have at
    /// most `max_texture_size` texels a side: keeps the drapes of the tiles
    /// it had, with their drawings for the layers to take, and drops the
    /// others. The layers draw into the drapes as on Mercator, on the globe
    /// too ([`View::mercator`]).
    pub(crate) fn begin(&mut self, view: &View, cover: &TerrainCover, stacks: usize, max_texture_size: u32) {
        let size = drape_size(cover.tile_size, max_texture_size);
        if size != self.size {
            *self = Drapes { size, ..Drapes::default() };
        }
        self.globe = view.is_globe();
        let flat = view.mercator();
        let mut kept = std::mem::take(&mut self.drapes);
        self.drapes.reserve(cover.tiles.len());
        for tile in &cover.tiles {
            let mut drape = match kept.iter().position(|drape| drape.tile == tile.id) {
                Some(index) => {
                    let mut drape = kept.swap_remove(index);
                    drape.transform = TransformParameters::for_drape(&flat, &tile.id.to_unwrapped());
                    drape
                }
                None => Drape::new(&flat, tile.id),
            };
            drape.stacks.resize_with(stacks, StackDrape::default);
            for stack in &mut drape.stacks {
                stack.previous = std::mem::take(&mut stack.layers);
            }
            self.drapes.push(drape);
        }
    }

    /// Makes the draws of `layer`, a layer of the frame in the stack
    /// `stack`, in every drape of the stack, with the drape's `parameters`,
    /// from its `source`. The layers update in drawing order.
    pub(crate) fn update_layer(
        &mut self,
        stack: usize,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        atlases: &Atlases,
        layer: &mut dyn RenderLayer,
        source: Option<LayerSource<'_>>,
    ) {
        let (size, globe) = (Size::new(self.size, self.size), self.globe);
        let style = Arc::clone(&layer.base().style);
        let pattern_atlas_size = atlases.pattern.pixel_size();
        // The draws' updates don't read what the layers composite with, which
        // the upload makes.
        let layer_composite = &mut LayerComposite::default();
        for drape in &mut self.drapes {
            let Drape { tile, transform, clip_masks, stacks } = drape;
            let Some(StackDrape { layers, previous, .. }) = stacks.get_mut(stack) else {
                continue;
            };
            // A drawing made for another style of the layer goes, as the
            // layer's own draws do.
            let kept = previous.iter().position(|kept| Arc::ptr_eq(&kept.style, &style));
            let kept = kept.map(|index| previous.swap_remove(index)).or_else(|| {
                let drawing = layer.new_drape_drawing();
                debug_assert!(drawing.is_some(), "a layer that doesn't drape in a drape");
                Some(DrapeLayer { style: Arc::clone(&style), drawing: drawing? })
            });
            let Some(mut kept) = kept else { continue };
            let unwrapped = tile.to_unwrapped();
            let mut parameters = parameters.drape(transform, clip_masks, layer_composite, size, unwrapped);
            layer.update_drape_draws(gpu, &mut parameters, atlases, source, (*tile, globe), &mut kept.drawing);
            let global = parameters.global_paint_params(pattern_atlas_size);
            kept.drawing.write_uniform(gpu, ID_GLOBAL_PAINT_PARAMS_UBO, &global, parameters.context);
            layers.push(kept);
        }
    }

    /// Ends the frame's update of the drapes: the drawings of layers that no
    /// longer drape go, a drape whose layers draw something in its tile has
    /// a texture, made in the stores of `context`, and one whose layers draw
    /// nothing there lets go of its texture.
    pub(crate) fn finish_update(&mut self, context: &mut Context) {
        let size = Size::new(self.size, self.size);
        for stack in self.drapes.iter_mut().flat_map(|drape| &mut drape.stacks) {
            stack.previous.clear();
            let mut draws = false;
            for layer in &stack.layers {
                layer.drawing.visit_groups(&mut |group| draws |= group.draws());
            }
            match &stack.texture {
                Some(_) if !draws => stack.texture = None,
                None if draws => stack.texture = Some(drape_texture(context, size)),
                _ => {}
            }
        }
    }

    /// The tile and stack of each drape that has a texture, in the cover's
    /// order.
    #[cfg(test)]
    pub(crate) fn textured(&self) -> Vec<(OverscaledTileID, usize)> {
        let stacks = |drape: &Drape| {
            let tile = drape.tile;
            let textured = drape.stacks.iter().enumerate().filter(|(_, stack)| stack.texture.is_some());
            textured.map(move |(stack, _)| (tile, stack)).collect::<Vec<_>>()
        };
        self.drapes.iter().flat_map(stacks).collect()
    }

    /// How many stacks the drapes of each tile are of.
    pub(crate) fn stacks(&self) -> usize {
        self.drapes.first().map_or(0, |drape| drape.stacks.len())
    }

    /// The texture of the drape of `stack` of the tile at `index` in the
    /// cover, `tile`: `None` where the stack draws nothing in the tile.
    pub(crate) fn texture(&self, stack: usize, index: usize, tile: &OverscaledTileID) -> Option<&SharedTexture> {
        let drape = self.drapes.get(index).filter(|drape| drape.tile == *tile)?;
        drape.stacks.get(stack)?.texture.as_ref()
    }

    /// Records that the buckets in `changed` changed, for the drapes' draws
    /// built from them.
    pub(crate) fn mark_changed(&mut self, changed: &ChangedBuckets) {
        for stack in self.drapes.iter_mut().flat_map(|drape| &mut drape.stacks) {
            for layer in &mut stack.layers {
                layer.drawing.mark_changed(changed);
            }
        }
    }

    /// Makes the drapes' textures and what they draw with, and uploads the
    /// GPU objects of their draws and the masks of the tiles they clip to,
    /// through `gpu`, the context of `parameters`, the frame's. The layer
    /// composite takes the composited draw groups of every drape, and drops
    /// its textures where there are none.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if self.drapes.is_empty() {
            self.depth_stencil = None;
            self.layer_composite = LayerComposite::default();
            return;
        }
        let size = Size::new(self.size, self.size);
        if self.depth_stencil.is_none() {
            match parameters.context.device().attachment(gpu, "drape depth and stencil", size, DEPTH_STENCIL) {
                Ok(depth_stencil) => self.depth_stencil = Some(depth_stencil),
                Err(error) => {
                    parameters.context.fail(format!("Making the drapes' depth and stencil failed: {error}"));
                }
            }
        }
        let layer_composite = &mut self.layer_composite;
        for drape in &mut self.drapes {
            let Drape { tile, transform, clip_masks, stacks } = drape;
            let parameters = parameters.drape(transform, clip_masks, layer_composite, size, tile.to_unwrapped());
            for stack in stacks.iter_mut() {
                let context = &mut *parameters.context;
                for layer in &mut stack.layers {
                    layer.drawing.upload(gpu, context);
                }
                let layers = &stack.layers;
                stack.groups = DrawGroups::of(
                    |visit| {
                        for layer in layers {
                            layer.drawing.visit_groups(visit);
                        }
                    },
                    |tiles| parameters.stencil_clear_tiles(tiles),
                );
            }
            // The masks of every stack's tiles, which their passes clip to
            // in turn.
            let mut clip_tiles: Vec<_> =
                stacks.iter().flat_map(|stack| stack.groups.clip_tiles.iter().copied()).collect();
            clip_tiles.sort_unstable();
            clip_tiles.dedup();
            let transform = parameters.transform_params;
            parameters.clip_masks.update(gpu, parameters.context, false, &clip_tiles, |tile| {
                to_projection_ubo(&transform.projection_data_for_tile(tile, false))
            });
        }
        let stacks = self.drapes.iter().flat_map(|drape| &drape.stacks);
        let composited: Vec<_> = stacks.flat_map(|stack| stack.groups.composited.iter().copied()).collect();
        let overdraw = parameters.debug_options.contains(MapDebugOptions::OVERDRAW);
        self.layer_composite.update(gpu, parameters.context, size, Some(COLOR), overdraw, &composited);
    }

    /// Draws each drape that has a texture into it, through `gpu`, the
    /// context of `parameters`, the frame's, outside any pass, and then its
    /// texture's mips: its layers' opaque draws front to back, then their
    /// translucent ones back to front, over transparent texels. A drape
    /// with a layer that blends in the shader draws into the layer
    /// composite's frame texture, which is then copied into its own.
    pub(crate) fn render(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        let Some(depth_stencil) = self.depth_stencil.as_ref().map(GpuTexture::id) else {
            return;
        };
        let size = Size::new(self.size, self.size);
        let layer_composite = &mut self.layer_composite;
        for drape in &mut self.drapes {
            let Drape { tile, transform, clip_masks, stacks } = drape;
            for stack in stacks.iter_mut() {
                let StackDrape { texture: Some(texture), layers, groups, .. } = stack else {
                    continue;
                };
                let color = match parameters.context.create_texture(gpu, texture) {
                    Ok(color) => color,
                    Err(error) => {
                        parameters.context.fail(format!("Making a drape's texture failed: {error}"));
                        continue;
                    }
                };
                let clear = LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 0.0));
                let frame_texture = layer_composite.frame_texture_for(&groups.composited);
                let pass = OpenPass {
                    label: "drape",
                    color: Some(ColorAttachment { texture: frame_texture.unwrap_or(color), load: clear }),
                    depth_stencil: Some(DepthStencilAttachment {
                        texture: depth_stencil,
                        depth: LoadOp::Clear(1.0),
                        stencil: LoadOp::Clear(0),
                    }),
                    size,
                    targets: TargetFormats { color: Some(COLOR), depth_stencil: Some(DEPTH_STENCIL) },
                };
                let unwrapped = tile.to_unwrapped();
                let mut drape_parameters = parameters.drape(transform, clip_masks, layer_composite, size, unwrapped);
                drape_parameters.stencil_clear.clone_from(&groups.stencil_clear);
                let drawn = drape_parameters.begin_pass(gpu, pass).and_then(|()| {
                    drape_parameters.depth_range_size = groups.depth_range_size();
                    drape_parameters.pass = RenderPass::Opaque;
                    drape_parameters.current_layer = 0;
                    for layer in layers.iter_mut().rev() {
                        layer.drawing.render(gpu, &mut drape_parameters, DrawOrder::TopDown);
                    }
                    drape_parameters.pass = RenderPass::Translucent;
                    drape_parameters.current_layer = (groups.count as u32).wrapping_sub(1);
                    for layer in layers.iter_mut() {
                        layer.drawing.render(gpu, &mut drape_parameters, DrawOrder::BottomUp);
                    }
                    // A drape whose frame doesn't reach it failed, as one
                    // whose pass did.
                    let presented = match frame_texture {
                        Some(_) => {
                            let present = OpenPass {
                                label: "drape",
                                color: Some(ColorAttachment { texture: color, load: clear }),
                                depth_stencil: None,
                                size,
                                targets: TargetFormats { color: Some(COLOR), depth_stencil: None },
                            };
                            drape_parameters.present_frame(gpu, present)
                        }
                        None => Ok(()),
                    };
                    drape_parameters.end_pass(gpu).and(presented)
                });
                if let Err(error) = drawn.and_then(|()| parameters.context.generate_mipmaps(gpu, texture)) {
                    parameters.context.fail(format!("Drawing a drape failed: {error}"));
                }
            }
        }
    }
}

/// A drape's texture of `size`, made in `context`'s store, mipmapped and
/// sampled through its mips, anisotropically, clamped to its edges.
fn drape_texture(context: &mut Context, size: Size) -> SharedTexture {
    context.add_texture_with(|texture| {
        texture.set_size(size);
        texture.set_format(COLOR);
        texture.set_render_target();
        texture.set_mipmapped(true);
        texture.set_sampler(SamplerState {
            filter: TextureFilterType::Linear,
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            max_anisotropy: MAX_ANISOTROPY,
            mipmapped: true,
        });
    })
}

#[cfg(test)]
mod tests {
    use mln_map::transform::Transform;
    use mln_map_types::{CameraOptions, ConstrainMode, ViewportMode};
    use mln_style_types::ProjectionDefinition;
    use mln_tile_id::CanonicalTileID;
    use mln_util::geo::LatLng;

    use super::*;
    use crate::terrain::{ElevationRanges, TerrainTile};

    /// On the globe, a drape draws its layers as on Mercator: its view isn't
    /// the globe's, and its tile projects through the Mercator matrix onto
    /// its texture, as GL JS draws its drapes.
    #[test]
    fn a_globe_drape_draws_on_mercator() {
        let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
        transform.resize(Size::new(512, 512)).unwrap();
        transform.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        let center = LatLng::new(30.0, 20.0).unwrap();
        transform.jump_to(&CameraOptions { center: Some(center), zoom: Some(3.0), ..Default::default() });
        let view = transform.view().clone();
        assert!(view.is_globe());
        let id = OverscaledTileID::new(3, 0, CanonicalTileID::new(3, 4, 3));
        let cover = TerrainCover {
            source_id: "dem".to_owned(),
            tile_size: 512,
            tiles: vec![TerrainTile { id, dem: None, dem_loading: false }],
            center: None,
            dem_tiles: Vec::new(),
            standing_dems: Vec::new(),
            ranges: ElevationRanges::default(),
        };
        let mut drapes = Drapes::default();
        drapes.begin(&view, &cover, 1, 4096);
        let transform = &drapes.drapes[0].transform;
        assert!(!transform.view.is_globe());
        let tile = id.to_unwrapped();
        let projection = transform.projection_data_for_tile(&tile, false);
        let mercator = TransformParameters::for_drape(&view.mercator(), &tile);
        assert_eq!(mercator.proj_matrix * view.mercator().tile_matrix(&tile), projection.main_matrix);
        assert_eq!(0.0, projection.projection_transition);
    }
}
