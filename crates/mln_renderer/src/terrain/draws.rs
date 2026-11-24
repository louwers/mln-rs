//! The terrain's draws: one per terrain tile and stack of draped layers, of
//! the shared grids, each with the tile's projection and where it reads its
//! DEM in entries of its stack's storage buffers, raised by the DEM's
//! texture, which the frame's ground made, and sampling the tile's drape of
//! the stack; and one per terrain tile of the ground's depth.

use std::mem::size_of;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::hillshade_layer_ubo::{HillshadeEvaluatedPropsUBO, HillshadeTilePropsUBO};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_PROJECTION_UBO, ID_TERRAIN_DEM_TEXTURE, ID_TERRAIN_DRAPE_TEXTURE, ID_TERRAIN_DRAWABLE_UBO, ID_TERRAIN_FOG_UBO,
    ID_TERRAIN_LIGHT_UBO, ID_TERRAIN_POS_VERTEX_ATTRIBUTE, ID_TERRAIN_PROPS_UBO,
};
use mln_gfx::shaders::terrain_ubo::{TerrainDrawableUBO, TerrainLightUBO};
use mln_gfx::{AttributeDataType, DepthMaskType, DrawMode};
use mln_map_types::View;
use mln_tile_id::CanonicalTileID;
use mln_util::clip_depth::ClipDepthRange;
use mln_util::logging::{Event, Log};

use super::fog::terrain_fog;
use super::mesh::{TerrainMesh, TerrainVertex};
use super::{Drapes, GroundBindings, Surface, TerrainCover};
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::frame_projection::zero_to_one_clip_z;
use crate::gfx::context::Context;
use crate::gfx::indexes::Indexes;
use crate::gfx::store::SharedIndexes;
use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::gfx::vertices::SharedVertices;
use crate::layer_drawing::{DrawDepth, DrawList, FrameDraw, KeptDraw, LayerDraws, TileMap, finish_tiles, tile_objects};
use crate::layers::render_hillshade_layer::lat_range;
use crate::paint_parameters::PaintParameters;
use crate::render_sky::EvaluatedSky;
use crate::tile_matrix::to_projection_ubo;

/// A grid, in the context's stores.
struct SharedMesh {
    /// Whether it has a cap to the north and to the south pole.
    poles: [bool; 2],
    vertices: SharedVertices,
    vertex_count: usize,
    indexes: SharedIndexes,
    index_count: usize,
}

/// What the surface lights a stack's drapes with: the drapes are of a
/// hillshade's slopes, which the stack's draws light as the hillshade's own
/// draws would, with its lights of the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SurfaceLight {
    /// The hillshade's lights, turned to the frame's bearing.
    pub(crate) props: HillshadeEvaluatedPropsUBO,
    pub(crate) exaggeration: f32,
    /// The hillshade's method, as its shader numbers them.
    pub(crate) method: i32,
    pub(crate) num_lights: i32,
}

/// What a tile's draw reads of its own: where it reads its DEM, its
/// projection, and, where its stack is lit, its latitudes with the lights.
#[derive(Clone, Copy)]
struct TileUniforms {
    drawable: TerrainDrawableUBO,
    projection: ProjectionUBO,
    light: TerrainLightUBO,
}

/// The draws of the terrain's tiles for one stack of draped layers.
#[derive(Default)]
struct StackDraws {
    /// Whether the draws light their drapes, with the program that does.
    lit: bool,
    /// The GPU objects of each tile's draw.
    tiles: TileMap<KeptDraw>,
    /// The frame's draws of them.
    frame: LayerDraws,
    /// The frame's draws as the update makes them, with their uniforms.
    draw_list: DrawList<TileUniforms>,
}

/// How the draws were made: the projection variant of their shaders,
/// whether their grids have skirts, and the triangles they cull, those
/// facing away, whose winding a flipped viewport turns round.
type BuiltFor = (ProjectionVariant, bool, CullFaceMode);

/// The draws of the terrain's tiles, a set for each stack, which draw in the
/// translucent pass, nearest the center first, testing and writing the 3D
/// depth range, and a set that draws the ground's depth. The sets share the
/// grids, the DEM textures and each tile's projection and DEM coordinates.
#[derive(Default)]
pub(crate) struct TerrainDraws {
    /// The draws of each of the frame's stacks.
    stacks: Vec<StackDraws>,
    /// The draws of the ground's depth, in a frame that draws it.
    depth: Option<StackDraws>,
    /// The surface's shader, the shader of a surface that lights its
    /// drapes and the ground depth's, each with the variant it is of.
    shader: Option<(ProjectionVariant, ShaderKey)>,
    lit_shader: Option<(ProjectionVariant, ShaderKey)>,
    depth_shader: Option<(ProjectionVariant, ShaderKey)>,
    /// The grids the draws hold: on the globe, the tiles of the first and
    /// last rows draw grids with caps to the poles.
    meshes: Vec<SharedMesh>,
    /// How the draws and the grids were made.
    built_for: Option<BuiltFor>,
    /// Whether the backend has no shader of the surface, which draws
    /// nothing then.
    unsupported: bool,
    /// Whether the backend lacks a shader that lighting a drape takes: no
    /// stack is lit then.
    unlit: bool,
}

impl TerrainDraws {
    /// Makes the frame's draws of the tiles of `cover`, one for each of the
    /// stacks of `drapes`, which read the DEM textures of `ground`, sample
    /// the tile's drape of their stack, light it with the stack's of
    /// `lights` if it has one, draw as `surface` says and fog as
    /// `sky` says, and, with `depth`, one of the ground's depth
    /// ([`render_depth`](Self::render_depth)), unless its shader fails,
    /// which fails the frame; or drops the draws and what they share for
    /// `None`, a frame without terrain. Each tile projects as the frame's
    /// view does, onto the globe on a globe.
    #[expect(clippy::too_many_arguments, reason = "the frame's terrain")]
    pub(crate) fn update(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        cover: Option<(&TerrainCover, &GroundBindings)>,
        surface: Surface,
        sky: Option<&EvaluatedSky>,
        (drapes, lights): (&Drapes, &[Option<SurfaceLight>]),
        depth: bool,
    ) {
        let shader = cover.filter(|_| drapes.stacks() > 0).and_then(|_| self.shader(gpu, parameters));
        let lit_shader = (shader.as_ref().filter(|_| lights.iter().any(Option::is_some)))
            .and_then(|_| self.lit_shader(gpu, parameters));
        let depth_shader = cover.filter(|_| depth).and_then(|_| self.depth_shader(gpu, parameters));
        if shader.is_none() {
            self.stacks.clear();
        }
        if depth_shader.is_none() {
            self.depth = None;
        }
        let Some((cover, ground)) = cover.filter(|_| shader.is_some() || depth_shader.is_some()) else {
            self.meshes.clear();
            self.built_for = None;
            return;
        };

        let cull_face_mode = parameters.cull_back_faces();
        let (view, proj_matrix) = (parameters.view, &parameters.transform_params.proj_matrix);
        let context = &mut *parameters.context;
        let built_for = (variant(view), surface.skirts, cull_face_mode);
        if self.built_for != Some(built_for) {
            // The draws hold the other variant's shaders, the other grids,
            // or cull the other way.
            self.stacks.clear();
            self.depth = None;
            self.meshes.clear();
            self.built_for = Some(built_for);
        }

        // A backend whose clip depth runs from -1 writes the depth of one
        // whose runs from 0, as fill extrusions do. The globe projects in its
        // shaders.
        let remap_clip_z = context.caps().clip_depth == ClipDepthRange::NegativeOneToOne && !view.is_globe();
        let stacks = if shader.is_some() { drapes.stacks() } else { 0 };
        self.stacks.resize_with(stacks, StackDraws::default);
        if depth_shader.is_some() {
            self.depth.get_or_insert_with(StackDraws::default);
        }
        for (stack, draws) in self.stacks.iter_mut().enumerate() {
            // The draws of the other program go.
            let lit = lit_shader.is_some() && lights.get(stack).is_some_and(Option::is_some);
            if draws.lit != lit {
                *draws = StackDraws { lit, ..StackDraws::default() };
            }
        }
        for stack in self.stacks.iter_mut().chain(&mut self.depth) {
            stack.tiles.set_tiles(cover.tiles.iter().map(|tile| tile.id));
        }
        for (index, tile) in cover.tiles.iter().enumerate() {
            let bound = tile.dem.and_then(|(id, coords)| Some((ground.dem(&id)?, coords)));
            let (dem_texture, dem_coords) = match bound {
                Some(((texture, dim), coords)) => {
                    (texture, [coords.scale, coords.offset[0], coords.offset[1], dim as f32])
                }
                None => (ground.flat(), [0.0; 4]),
            };
            let mut projection = view.projection_data_with(&tile.id.to_unwrapped(), proj_matrix);
            if remap_clip_z {
                zero_to_one_clip_z(&mut projection.main_matrix);
                zero_to_one_clip_z(&mut projection.fallback_matrix);
            }
            let uniforms = TileUniforms {
                drawable: TerrainDrawableUBO { dem_coords },
                projection: to_projection_ubo(&projection),
                light: TerrainLightUBO::default(),
            };
            let latrange = lat_range(&tile.id.to_unwrapped());
            let mesh = mesh(&mut self.meshes, context, surface.skirts, poles(view, tile.id.canonical));
            if let (Some(draws), Some(shader)) = (&mut self.depth, &depth_shader)
                && let Some(entry) = draws.tiles.entry(&tile.id)
            {
                let draw =
                    entry.value.get_or_insert_with(|| KeptDraw::new(depth_objects(shader, mesh, cull_face_mode)));
                draw.objects.set_texture(ID_TERRAIN_DEM_TEXTURE, Some(dem_texture));
                let frame_draw = FrameDraw {
                    priority: index as i64,
                    depth: DrawDepth::Volume { mask: DepthMaskType::ReadWrite },
                    ..draw.frame_draw(Some(tile.id))
                };
                draws.draw_list.push(frame_draw, uniforms);
            }
            let Some(shader) = &shader else { continue };
            for (stack, draws) in self.stacks.iter_mut().enumerate() {
                let Some(entry) = draws.tiles.entry(&tile.id) else {
                    continue;
                };
                // Where a stack draws nothing in the tile, it has no drape
                // and no draw, but the first stack's draw writes the depth
                // the stacks above test, so it draws transparent there.
                let drape = match drapes.texture(stack, index, &tile.id) {
                    Some(drape) => drape.clone(),
                    None if stack == 0 => ground.flat().clone(),
                    None => {
                        *entry.value = None;
                        continue;
                    }
                };
                let light = lights.get(stack).copied().flatten().filter(|_| draws.lit);
                let shader = if draws.lit { lit_shader.as_ref().unwrap_or(shader) } else { shader };
                let draw = entry.value.get_or_insert_with(|| KeptDraw::new(mesh_objects(shader, mesh, cull_face_mode)));
                draw.objects.set_texture(ID_TERRAIN_DEM_TEXTURE, Some(dem_texture));
                draw.objects.set_texture(ID_TERRAIN_DRAPE_TEXTURE, Some(&drape));
                let frame_draw = FrameDraw {
                    priority: index as i64,
                    depth: DrawDepth::Volume { mask: DepthMaskType::ReadWrite },
                    ..draw.frame_draw(Some(tile.id))
                };
                let light = light.map_or_else(TerrainLightUBO::default, |light| TerrainLightUBO {
                    tile: HillshadeTilePropsUBO {
                        latrange,
                        exaggeration: light.exaggeration,
                        method: light.method,
                        num_lights: light.num_lights,
                        ..HillshadeTilePropsUBO::default()
                    },
                    props: light.props,
                });
                draws.draw_list.push(frame_draw, TileUniforms { light, ..uniforms });
            }
        }
        let fog = terrain_fog(sky, view);
        for StackDraws { lit, tiles, frame, draw_list } in &mut self.stacks {
            finish_tiles(draw_list, frame, tiles, gpu, context, |writer| {
                writer.storage(ID_TERRAIN_DRAWABLE_UBO, |uniforms| uniforms.drawable);
                if *lit {
                    writer.storage(ID_TERRAIN_LIGHT_UBO, |uniforms| uniforms.light);
                }
                writer.storage(ID_PROJECTION_UBO, |uniforms| uniforms.projection);
                writer.uniform(ID_TERRAIN_PROPS_UBO, &surface.props);
                writer.uniform(ID_TERRAIN_FOG_UBO, &fog);
            });
        }
        if let Some(StackDraws { tiles, frame, draw_list, .. }) = &mut self.depth {
            finish_tiles(draw_list, frame, tiles, gpu, context, |writer| {
                writer.storage(ID_TERRAIN_DRAWABLE_UBO, |uniforms| uniforms.drawable);
                writer.storage(ID_PROJECTION_UBO, |uniforms| uniforms.projection);
                writer.uniform(ID_TERRAIN_PROPS_UBO, &surface.props);
            });
        }
    }

    /// Whether it made draws of the ground's depth for the frame.
    pub(crate) fn draws_depth(&self) -> bool {
        self.depth.is_some()
    }

    /// The program of the ground's depth in the frame's projection, made on
    /// `gpu` the first time it makes; `None`, the frame's failure, in a frame
    /// where it fails.
    fn depth_shader(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> Option<ShaderKey> {
        let variant = variant(parameters.view);
        if let Some((made_for, shader)) = &self.depth_shader
            && *made_for == variant
        {
            return Some(shader.clone());
        }
        let context = &mut *parameters.context;
        let shader = context.shader_key(&programs::TERRAIN_DEPTH, Uniforms::default(), variant);
        if let Err(error) = context.shader(gpu, &shader) {
            context.fail(format!("Making the ground's depth shader failed: {error}"));
            return None;
        }
        Some(self.depth_shader.insert((variant, shader)).1.clone())
    }

    /// Whether the backend has the terrain's shader, which this makes on
    /// `gpu` the first time.
    pub(crate) fn supported(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> bool {
        self.shader(gpu, parameters).is_some()
    }

    /// The surface's shader in the frame's projection, made on `gpu` the
    /// first time; `None`, logged once, where the backend has none.
    fn shader(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> Option<ShaderKey> {
        let variant = variant(parameters.view);
        if let Some((made_for, shader)) = &self.shader
            && *made_for == variant
        {
            return Some(shader.clone());
        }
        if self.unsupported {
            return None;
        }
        let context = &mut *parameters.context;
        let shader = context.shader_key(&programs::TERRAIN, Uniforms::default(), variant);
        if let Err(error) = context.shader(gpu, &shader) {
            self.unsupported = true;
            Log::warning(Event::Render, format!("Terrain draws nothing on this backend: {error}"));
            return None;
        }
        Some(self.shader.insert((variant, shader)).1.clone())
    }

    /// Whether the backend has the shaders that lighting a drape on the
    /// surface takes, which this makes on `gpu` the first time: the
    /// surface's that lights a drape of slopes, in the frame's projection,
    /// and the hillshade's that drapes them.
    pub(crate) fn lights(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> bool {
        self.lit_shader(gpu, parameters).is_some()
    }

    /// The shader of a surface that lights its drapes, in the frame's
    /// projection, made on `gpu` the first time; `None` where the backend
    /// lacks it or the shader that drapes the slopes.
    fn lit_shader(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> Option<ShaderKey> {
        let variant = variant(parameters.view);
        if let Some((made_for, shader)) = &self.lit_shader
            && *made_for == variant
        {
            return Some(shader.clone());
        }
        if self.unlit {
            return None;
        }
        let context = &mut *parameters.context;
        let shader = context.shader_key(&programs::TERRAIN_HILLSHADE, Uniforms::default(), variant);
        // The drapes draw as on Mercator.
        let slopes = context.shader_key(&programs::HILLSHADE_SLOPES, Uniforms::default(), ProjectionVariant::Mercator);
        if let Err(error) = context.shader(gpu, &shader).and_then(|_| context.shader(gpu, &slopes)) {
            Log::warning(
                Event::Render,
                format!("The terrain's surface can't light a hillshade on this backend: {error}"),
            );
            self.unlit = true;
            return None;
        }
        Some(self.lit_shader.insert((variant, shader)).1.clone())
    }

    /// Whether it made the terrain's shader.
    #[cfg(test)]
    pub(crate) fn has_shader(&self) -> bool {
        self.shader.is_some()
    }

    /// Whether it holds draws of the surface, or the grids while it draws no
    /// ground's depth, which shares them.
    #[cfg(test)]
    pub(crate) fn holds_draws(&self) -> bool {
        !self.stacks.is_empty() || (!self.meshes.is_empty() && self.depth.is_none())
    }

    /// Uploads the GPU objects of the frame's draws.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        for StackDraws { tiles, frame, .. } in self.stacks.iter_mut().chain(&mut self.depth) {
            frame.upload(gpu, tiles, tile_objects, context);
        }
    }

    /// Draws the frame's draws of the ground's depth into the open pass of
    /// `gpu`, the context's.
    pub(crate) fn render_depth(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if let Some(StackDraws { tiles, frame, .. }) = &mut self.depth {
            frame.render(tiles, tile_objects, gpu, parameters);
        }
    }

    /// Draws the frame's draws of `stack` into the open pass of `gpu`, the
    /// context's.
    pub(crate) fn render(&mut self, stack: usize, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if let Some(StackDraws { tiles, frame, .. }) = self.stacks.get_mut(stack) {
            frame.render(tiles, tile_objects, gpu, parameters);
        }
    }
}

/// The variant of the surface's shaders in `view`: the globe's while it
/// draws the globe, through its handover to Mercator too.
fn variant(view: &View) -> ProjectionVariant {
    if view.is_globe() { ProjectionVariant::Globe } else { ProjectionVariant::Mercator }
}

/// Whether the grid of `tile` in `view` closes the north and the south
/// pole: on the globe, for a tile of the first or the last row.
fn poles(view: &View, tile: CanonicalTileID) -> [bool; 2] {
    let last = (1u32 << tile.z) - 1;
    [view.is_globe() && tile.y == 0, view.is_globe() && tile.y == last]
}

/// The grid of `meshes` with `skirts` and the caps of `poles`, made in the
/// stores of `context` the first time.
fn mesh<'a>(meshes: &'a mut Vec<SharedMesh>, context: &mut Context, skirts: bool, poles: [bool; 2]) -> &'a SharedMesh {
    if let Some(index) = meshes.iter().position(|mesh| mesh.poles == poles) {
        return &meshes[index];
    }
    let TerrainMesh { vertices, indices } = TerrainMesh::new(skirts, poles);
    meshes.push(SharedMesh {
        poles,
        vertex_count: vertices.len(),
        vertices: context.add_vertices(vertices),
        index_count: indices.len(),
        indexes: context.add_indexes(Indexes::new(indices)),
    });
    &meshes[meshes.len() - 1]
}

/// The GPU objects of a tile's draw of the ground's depth, of `mesh` with
/// `shader`: unblended, culling as `cull_face_mode` says.
fn depth_objects(shader: &ShaderKey, mesh: &SharedMesh, cull_face_mode: CullFaceMode) -> DrawObjects {
    grid_objects(shader, mesh, ColorMode::unblended(), cull_face_mode)
}

/// The GPU objects of a tile's draw of `mesh` with `shader`: alpha-blended,
/// culling as `cull_face_mode` says.
fn mesh_objects(shader: &ShaderKey, mesh: &SharedMesh, cull_face_mode: CullFaceMode) -> DrawObjects {
    grid_objects(shader, mesh, ColorMode::alpha_blended(), cull_face_mode)
}

/// The GPU objects of a tile's draw of `mesh` with `shader`, in
/// `color_mode`, culling as `cull_face_mode` says.
fn grid_objects(
    shader: &ShaderKey,
    mesh: &SharedMesh,
    color_mode: ColorMode,
    cull_face_mode: CullFaceMode,
) -> DrawObjects {
    let mut vertices = VertexAttributeArray::new();
    vertices.set_shared(
        ID_TERRAIN_POS_VERTEX_ATTRIBUTE,
        SharedAttribute::new(mesh.vertices.clone(), AttributeDataType::Short4, 0, size_of::<TerrainVertex>() as u32),
    );
    let geometry = DrawGeometry {
        vertices,
        vertex_count: mesh.vertex_count,
        mode: DrawMode::Triangles(),
        indexes: mesh.indexes.clone().into(),
        segments: vec![SegmentBase::with_lengths(0, 0, mesh.vertex_count, mesh.index_count)],
        instances: None,
    };
    DrawObjects::new(shader.clone(), color_mode, cull_face_mode, geometry, Textures::default())
}
