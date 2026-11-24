//! The terrain's draws: one per terrain tile and stack of draped layers, of
//! the shared grids, each with the tile's projection and where it reads its
//! DEM in entries of its stack's storage buffers, raised by the DEM's
//! texture and sampling the tile's drape of the stack.

use std::mem::size_of;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::programs;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_PROJECTION_UBO, ID_TERRAIN_DEM_TEXTURE, ID_TERRAIN_DRAPE_TEXTURE, ID_TERRAIN_DRAWABLE_UBO,
    ID_TERRAIN_POS_VERTEX_ATTRIBUTE, ID_TERRAIN_PROPS_UBO,
};
use mln_gfx::shaders::terrain_ubo::TerrainDrawableUBO;
use mln_gfx::{AttributeDataType, DepthMaskType, DrawMode};
use mln_map_types::View;
use mln_tile_id::CanonicalTileID;
use mln_util::clip_depth::ClipDepthRange;
use mln_util::logging::{Event, Log};

use super::mesh::{TerrainMesh, TerrainVertex};
use super::{DemBindings, Drapes, Surface, TerrainCover};
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::frame_projection::zero_to_one_clip_z;
use crate::gfx::context::Context;
use crate::gfx::indexes::Indexes;
use crate::gfx::store::SharedIndexes;
use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::gfx::vertices::SharedVertices;
use crate::layer_drawing::{DrawDepth, DrawList, FrameDraw, KeptDraw, LayerDraws, TileMap, finish_tiles, tile_objects};
use crate::paint_parameters::PaintParameters;
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

/// What a tile's draw reads of its own: where it reads its DEM, and its
/// projection.
#[derive(Clone, Copy)]
struct TileUniforms {
    drawable: TerrainDrawableUBO,
    projection: ProjectionUBO,
}

/// The draws of the terrain's tiles for one stack of draped layers.
#[derive(Default)]
struct StackDraws {
    /// The GPU objects of each tile's draw.
    tiles: TileMap<KeptDraw>,
    /// The frame's draws of them.
    frame: LayerDraws,
    /// The frame's draws as the update makes them, with their uniforms.
    draw_list: DrawList<TileUniforms>,
}

/// How the draws were made: the projection variant of their shader,
/// whether their grids have skirts, and the triangles they cull, those
/// facing away, whose winding a flipped viewport turns round.
type BuiltFor = (ProjectionVariant, bool, CullFaceMode);

/// The draws of the terrain's tiles, a set for each stack, which draw in the
/// translucent pass, nearest the center first, testing and writing the 3D
/// depth range. The sets share the grids, the DEM textures and each tile's
/// projection and DEM coordinates.
#[derive(Default)]
pub(crate) struct TerrainDraws {
    /// The draws of each of the frame's stacks.
    stacks: Vec<StackDraws>,
    /// The draws of the surface's depth, transparent, which draw before the
    /// frame's layers, while the frame draws them.
    depth: Option<StackDraws>,
    /// The surface's shader, with the variant it is of.
    shader: Option<(ProjectionVariant, ShaderKey)>,
    /// The grids the draws hold: on the globe, the tiles of the first and
    /// last rows draw grids with caps to the poles.
    meshes: Vec<SharedMesh>,
    /// How the draws and the grids were made.
    built_for: Option<BuiltFor>,
    /// Whether the backend failed to make the surface's shader.
    failed: bool,
}

impl TerrainDraws {
    /// Makes the frame's draws of the tiles of `cover`, one for each of the
    /// stacks of `drapes`, which read the DEM textures of `dems`, sample the
    /// tile's drape of their stack and draw as `surface` says, and, if
    /// `depth_first`, one that draws only the surface's depth; or drops the
    /// draws and what they share for `None`, a frame without terrain. Each
    /// tile projects as the frame's view does, onto the globe on a globe.
    pub(crate) fn update(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        cover: Option<(&TerrainCover, &DemBindings)>,
        surface: Surface,
        drapes: &Drapes,
        depth_first: bool,
    ) {
        let stacks = drapes.stacks();
        let shader = cover.filter(|_| stacks > 0).and_then(|_| self.shader(gpu, parameters.context, parameters.view));
        let (Some((cover, dems)), Some(shader)) = (cover, shader) else {
            self.stacks.clear();
            self.depth = None;
            self.meshes.clear();
            self.built_for = None;
            return;
        };

        let cull_face_mode = parameters.cull_back_faces();
        let (view, proj_matrix) = (parameters.view, &parameters.transform_params.proj_matrix);
        let context = &mut *parameters.context;
        let built_for = (variant(view), surface.skirts, cull_face_mode);
        if self.built_for != Some(built_for) {
            // The draws hold the other variant's shader, the other grids, or
            // cull the other way.
            self.stacks.clear();
            self.depth = None;
            self.meshes.clear();
            self.built_for = Some(built_for);
        }

        // A backend whose clip depth runs from -1 writes the depth of one
        // whose runs from 0, as fill extrusions do. The globe projects in its
        // shaders.
        let remap_clip_z = context.caps().clip_depth == ClipDepthRange::NegativeOneToOne && !view.is_globe();
        self.stacks.resize_with(stacks, StackDraws::default);
        if !depth_first {
            self.depth = None;
        } else if self.depth.is_none() {
            self.depth = Some(StackDraws::default());
        }
        for stack in self.stacks.iter_mut().chain(&mut self.depth) {
            stack.tiles.set_tiles(cover.tiles.iter().map(|tile| tile.id));
        }
        for (index, tile) in cover.tiles.iter().enumerate() {
            let bound = tile.dem.and_then(|(id, coords)| Some((dems.dem(&id)?, coords)));
            let (dem_texture, dem_coords) = match bound {
                Some(((texture, dim), coords)) => {
                    (texture, [coords.scale, coords.offset[0], coords.offset[1], dim as f32])
                }
                None => (dems.flat(), [0.0; 4]),
            };
            let mut projection = view.projection_data_with(&tile.id.to_unwrapped(), proj_matrix);
            if remap_clip_z {
                zero_to_one_clip_z(&mut projection.main_matrix);
                zero_to_one_clip_z(&mut projection.fallback_matrix);
            }
            let uniforms = TileUniforms {
                drawable: TerrainDrawableUBO { dem_coords },
                projection: to_projection_ubo(&projection),
            };
            let mesh = mesh(&mut self.meshes, context, surface.skirts, poles(view, tile.id.canonical));
            let sets = self.stacks.iter_mut().enumerate().map(|(stack, draws)| (Some(stack), draws));
            for (stack, draws) in sets.chain(self.depth.iter_mut().map(|draws| (None, draws))) {
                let Some(entry) = draws.tiles.entry(&tile.id) else {
                    continue;
                };
                // Where a stack draws nothing in the tile, it has no drape
                // and no draw, but the first stack's draw writes the depth
                // the stacks above test, so it draws transparent there. The
                // depth's draws draw so everywhere: the same draw, so the
                // same depth.
                let drape = match stack {
                    Some(stack) => drapes.texture(stack, &tile.id).or_else(|| (stack == 0).then(|| dems.flat())),
                    None => Some(dems.flat()),
                };
                let Some(drape) = drape.cloned() else {
                    *entry.value = None;
                    continue;
                };
                let draw =
                    entry.value.get_or_insert_with(|| KeptDraw::new(mesh_objects(&shader, mesh, cull_face_mode)));
                draw.objects.set_texture(ID_TERRAIN_DEM_TEXTURE, Some(dem_texture));
                draw.objects.set_texture(ID_TERRAIN_DRAPE_TEXTURE, Some(&drape));
                let frame_draw = FrameDraw {
                    priority: index as i64,
                    depth: DrawDepth::Volume { mask: DepthMaskType::ReadWrite },
                    ..draw.frame_draw(Some(tile.id))
                };
                draws.draw_list.push(frame_draw, uniforms);
            }
        }
        for StackDraws { tiles, frame, draw_list } in self.stacks.iter_mut().chain(&mut self.depth) {
            finish_tiles(draw_list, frame, tiles, gpu, context, |writer| {
                writer.storage(ID_TERRAIN_DRAWABLE_UBO, |uniforms| uniforms.drawable);
                writer.storage(ID_PROJECTION_UBO, |uniforms| uniforms.projection);
                writer.uniform(ID_TERRAIN_PROPS_UBO, &surface.props);
            });
        }
    }

    /// Whether the backend failed to make the surface's shader, which then
    /// draws nothing until the shaders are made again.
    pub(crate) fn failed(&self) -> bool {
        self.failed
    }

    /// The surface's shader in `view`'s projection, made on `gpu` in
    /// `context` the first time; `None`, logged once, where the backend has
    /// none.
    pub(crate) fn shader(&mut self, gpu: &mut dyn Gpu, context: &mut Context, view: &View) -> Option<ShaderKey> {
        let variant = variant(view);
        if let Some((made_for, shader)) = &self.shader
            && *made_for == variant
        {
            return Some(shader.clone());
        }
        if self.failed {
            return None;
        }
        let shader = context.shader_key(&programs::TERRAIN, Uniforms::default(), variant);
        if let Err(error) = context.shader(gpu, &shader) {
            self.failed = true;
            Log::warning(Event::Render, format!("Terrain draws nothing on this backend: {error}"));
            return None;
        }
        Some(self.shader.insert((variant, shader)).1.clone())
    }

    /// Uploads the GPU objects of the frame's draws.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        for StackDraws { tiles, frame, .. } in self.stacks.iter_mut().chain(&mut self.depth) {
            frame.upload(gpu, tiles, tile_objects, context);
        }
    }

    /// Draws the frame's draws of `stack` into the open pass of `gpu`, the
    /// context's.
    pub(crate) fn render(&mut self, stack: usize, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if let Some(StackDraws { tiles, frame, .. }) = self.stacks.get_mut(stack) {
            frame.render(tiles, tile_objects, gpu, parameters);
        }
    }

    /// Draws the frame's draws of the surface's depth, if it made them, into
    /// the open pass of `gpu`, the context's.
    pub(crate) fn render_depth(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if let Some(StackDraws { tiles, frame, .. }) = &mut self.depth {
            frame.render(tiles, tile_objects, gpu, parameters);
        }
    }
}

/// The variant of the surface's shader in `view`: the globe's while it
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

/// The GPU objects of a tile's draw of `mesh` with `shader`: alpha-blended,
/// culling as `cull_face_mode` says.
fn mesh_objects(shader: &ShaderKey, mesh: &SharedMesh, cull_face_mode: CullFaceMode) -> DrawObjects {
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
    DrawObjects::new(shader.clone(), ColorMode::alpha_blended(), cull_face_mode, geometry, Textures::default())
}
