//! The terrain's draws: one per terrain tile and stack of draped layers, of
//! the shared grid, each with the tile's matrix and where it reads its DEM
//! in an entry of its stack's storage buffer, raised by the DEM's texture,
//! which the frame's ground made, and sampling the tile's drape of the
//! stack; and one per terrain tile of the ground's depth.

use std::mem::size_of;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_PROJECTION_UBO, ID_TERRAIN_DEM_TEXTURE, ID_TERRAIN_DRAPE_TEXTURE, ID_TERRAIN_DRAWABLE_UBO, ID_TERRAIN_FOG_UBO,
    ID_TERRAIN_POS_VERTEX_ATTRIBUTE, ID_TERRAIN_PROPS_UBO,
};
use mln_gfx::shaders::terrain_ubo::TerrainDrawableUBO;
use mln_gfx::{AttributeDataType, DepthMaskType, DrawMode};
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
use crate::layers::cast_matrix;
use crate::paint_parameters::PaintParameters;
use crate::render_sky::EvaluatedSky;

/// The grid, in the context's stores.
struct SharedMesh {
    /// Whether it has skirts.
    skirts: bool,
    /// The triangles its draws cull: those facing away, whose winding a
    /// flipped viewport turns round.
    cull_face_mode: CullFaceMode,
    vertices: SharedVertices,
    vertex_count: usize,
    indexes: SharedIndexes,
    index_count: usize,
}

/// The draws of the terrain's tiles for one stack of draped layers.
#[derive(Default)]
struct StackDraws {
    /// The GPU objects of each tile's draw.
    tiles: TileMap<KeptDraw>,
    /// The frame's draws of them.
    frame: LayerDraws,
    /// The frame's draws as the update makes them, with their uniforms.
    draw_list: DrawList<TerrainDrawableUBO>,
}

/// The draws of the terrain's tiles, a set for each stack, which draw in the
/// translucent pass, nearest the center first, testing and writing the 3D
/// depth range, and a set that draws the ground's depth. The sets share the
/// grid, the DEM textures and each tile's matrix and DEM coordinates.
#[derive(Default)]
pub(crate) struct TerrainDraws {
    /// The draws of each of the frame's stacks.
    stacks: Vec<StackDraws>,
    /// The draws of the ground's depth, in a frame that draws it.
    depth: Option<StackDraws>,
    shader: Option<ShaderKey>,
    depth_shader: Option<ShaderKey>,
    mesh: Option<SharedMesh>,
    /// Whether the backend has no shader of the surface, which draws
    /// nothing then.
    unsupported: bool,
}

impl TerrainDraws {
    /// Makes the frame's draws of the tiles of `cover`, one for each of the
    /// stacks of `drapes`, which read the DEM textures of `ground`, sample
    /// the tile's drape of their stack, draw as `surface` says and fog as
    /// `sky` says, and, with `depth`, one of the ground's depth
    /// ([`render_depth`](Self::render_depth)), unless its shader fails,
    /// which fails the frame; or drops the draws and what they share for
    /// `None`, a frame without terrain.
    #[expect(clippy::too_many_arguments, reason = "the frame's terrain")]
    pub(crate) fn update(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        cover: Option<(&TerrainCover, &GroundBindings)>,
        surface: Surface,
        sky: Option<&EvaluatedSky>,
        drapes: &Drapes,
        depth: bool,
    ) {
        let shader = cover.filter(|_| drapes.stacks() > 0).and_then(|_| self.shader(gpu, parameters));
        let depth_shader = cover.filter(|_| depth).and_then(|_| self.depth_shader(gpu, parameters));
        if shader.is_none() {
            self.stacks.clear();
        }
        if depth_shader.is_none() {
            self.depth = None;
        }
        let Some((cover, ground)) = cover.filter(|_| shader.is_some() || depth_shader.is_some()) else {
            self.mesh = None;
            return;
        };

        let cull_face_mode = parameters.cull_back_faces();
        let context = &mut *parameters.context;
        let built_for = (surface.skirts, cull_face_mode);
        if self.mesh.as_ref().is_some_and(|mesh| (mesh.skirts, mesh.cull_face_mode) != built_for) {
            // The draws hold the other grid, or cull the other way.
            self.stacks.clear();
            self.depth = None;
            self.mesh = None;
        }
        let mesh = self.mesh.get_or_insert_with(|| {
            let TerrainMesh { vertices, indices } = TerrainMesh::new(surface.skirts);
            SharedMesh {
                skirts: surface.skirts,
                cull_face_mode,
                vertex_count: vertices.len(),
                vertices: context.add_vertices(vertices),
                index_count: indices.len(),
                indexes: context.add_indexes(Indexes::new(indices)),
            }
        });

        // A backend whose clip depth runs from -1 writes the depth of one
        // whose runs from 0, as fill extrusions do.
        let remap_clip_z = context.caps().clip_depth == ClipDepthRange::NegativeOneToOne;
        let (view, proj_matrix) = (parameters.view, &parameters.transform_params.proj_matrix);
        let stacks = if shader.is_some() { drapes.stacks() } else { 0 };
        self.stacks.resize_with(stacks, StackDraws::default);
        if depth_shader.is_some() {
            self.depth.get_or_insert_with(StackDraws::default);
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
            let mut matrix = *proj_matrix * view.tile_matrix(&tile.id.to_unwrapped());
            if remap_clip_z {
                zero_to_one_clip_z(&mut matrix);
            }
            let drawable = TerrainDrawableUBO { matrix: cast_matrix(&matrix), dem_coords };
            if let (Some(draws), Some(shader)) = (&mut self.depth, &depth_shader)
                && let Some(entry) = draws.tiles.entry(&tile.id)
            {
                let draw = entry.value.get_or_insert_with(|| KeptDraw::new(depth_objects(shader, mesh)));
                draw.objects.set_texture(ID_TERRAIN_DEM_TEXTURE, Some(dem_texture));
                let frame_draw = FrameDraw {
                    priority: index as i64,
                    depth: DrawDepth::Volume { mask: DepthMaskType::ReadWrite },
                    ..draw.frame_draw(Some(tile.id))
                };
                draws.draw_list.push(frame_draw, drawable);
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
                let draw = entry.value.get_or_insert_with(|| KeptDraw::new(mesh_objects(shader, mesh)));
                draw.objects.set_texture(ID_TERRAIN_DEM_TEXTURE, Some(dem_texture));
                draw.objects.set_texture(ID_TERRAIN_DRAPE_TEXTURE, Some(&drape));
                let frame_draw = FrameDraw {
                    priority: index as i64,
                    depth: DrawDepth::Volume { mask: DepthMaskType::ReadWrite },
                    ..draw.frame_draw(Some(tile.id))
                };
                draws.draw_list.push(frame_draw, drawable);
            }
        }
        let fog = terrain_fog(sky, view);
        for StackDraws { tiles, frame, draw_list } in &mut self.stacks {
            finish_tiles(draw_list, frame, tiles, gpu, context, |writer| {
                writer.storage(ID_TERRAIN_DRAWABLE_UBO, |drawable| *drawable);
                // GL's program declares the projection block whether it
                // reads it or not, so every draw binds one of zeros.
                writer.storage(ID_PROJECTION_UBO, |_| ProjectionUBO::default());
                writer.uniform(ID_TERRAIN_PROPS_UBO, &surface.props);
                writer.uniform(ID_TERRAIN_FOG_UBO, &fog);
            });
        }
        if let Some(StackDraws { tiles, frame, draw_list }) = &mut self.depth {
            finish_tiles(draw_list, frame, tiles, gpu, context, |writer| {
                writer.storage(ID_TERRAIN_DRAWABLE_UBO, |drawable| *drawable);
                writer.storage(ID_PROJECTION_UBO, |_| ProjectionUBO::default());
                writer.uniform(ID_TERRAIN_PROPS_UBO, &surface.props);
            });
        }
    }

    /// Whether it made draws of the ground's depth for the frame.
    pub(crate) fn draws_depth(&self) -> bool {
        self.depth.is_some()
    }

    /// The program of the ground's depth, made on `gpu` the first time it
    /// makes; `None`, the frame's failure, in a frame where it fails.
    fn depth_shader(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> Option<ShaderKey> {
        if let Some(shader) = &self.depth_shader {
            return Some(shader.clone());
        }
        let context = &mut *parameters.context;
        let shader = context.shader_key(&programs::TERRAIN_DEPTH, Uniforms::default(), ProjectionVariant::Mercator);
        if let Err(error) = context.shader(gpu, &shader) {
            context.fail(format!("Making the ground's depth shader failed: {error}"));
            return None;
        }
        Some(self.depth_shader.insert(shader).clone())
    }

    /// Whether the backend has the terrain's shader, which this makes on
    /// `gpu` the first time.
    pub(crate) fn supported(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> bool {
        self.shader(gpu, parameters).is_some()
    }

    /// The surface's shader, made on `gpu` the first time; `None`, logged
    /// once, where the backend has none.
    fn shader(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> Option<ShaderKey> {
        if let Some(shader) = &self.shader {
            return Some(shader.clone());
        }
        if self.unsupported {
            return None;
        }
        let context = &mut *parameters.context;
        let shader = context.shader_key(&programs::TERRAIN, Uniforms::default(), ProjectionVariant::Mercator);
        if let Err(error) = context.shader(gpu, &shader) {
            self.unsupported = true;
            Log::warning(Event::Render, format!("Terrain draws nothing on this backend: {error}"));
            return None;
        }
        Some(self.shader.insert(shader).clone())
    }

    /// Whether it made the terrain's shader.
    #[cfg(test)]
    pub(crate) fn has_shader(&self) -> bool {
        self.shader.is_some()
    }

    /// Whether it holds draws of the surface, or the grid while it draws no
    /// ground's depth, which shares it.
    #[cfg(test)]
    pub(crate) fn holds_draws(&self) -> bool {
        !self.stacks.is_empty() || (self.mesh.is_some() && self.depth.is_none())
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

/// The GPU objects of a tile's draw of the ground's depth, of `mesh` with
/// `shader`: unblended, without the triangles that face away.
fn depth_objects(shader: &ShaderKey, mesh: &SharedMesh) -> DrawObjects {
    grid_objects(shader, mesh, ColorMode::unblended())
}

/// The GPU objects of a tile's draw of `mesh` with `shader`: alpha-blended,
/// without the triangles that face away.
fn mesh_objects(shader: &ShaderKey, mesh: &SharedMesh) -> DrawObjects {
    grid_objects(shader, mesh, ColorMode::alpha_blended())
}

/// The GPU objects of a tile's draw of `mesh` with `shader`, in
/// `color_mode`, without the triangles that face away.
fn grid_objects(shader: &ShaderKey, mesh: &SharedMesh, color_mode: ColorMode) -> DrawObjects {
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
    DrawObjects::new(shader.clone(), color_mode, mesh.cull_face_mode, geometry, Textures::default())
}
