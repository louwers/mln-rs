//! The terrain's draws: one per terrain tile and stack of draped layers, of
//! the shared grids, each with the tile's projection, where it reads its
//! DEM and its drape's light in entries of its stack's storage buffers,
//! raised by the DEM's texture and sampling the tile's drape of the stack,
//! which they shade with the stack's light, if it has one, and cover with
//! the frame's fog; one per terrain tile of the surface's depth, where 3D
//! layers draw before the surface; and one per terrain tile of the ground's
//! depth into its texture, where layers hide behind the ground.

use std::mem::size_of;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::{ClipDepthRange, Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::{AttributeDataType, DepthMaskType, DrawMode};
use mln_map_types::View;
use mln_style::tile_id::CanonicalTileID;

use super::mesh::{TerrainMesh, TerrainVertex};
use super::{DemBindings, DrapeStacks, Drapes, Surface, TerrainCover};
use crate::draw::{DrawGeometry, DrawObjects, Textures};
use crate::frame_projection::zero_to_one_clip_z;
use crate::gfx::context::Context;
use crate::gfx::indexes::Indexes;
use crate::gfx::segment::SegmentBase;
use crate::gfx::store::SharedIndexes;
use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::gfx::vertices::SharedVertices;
use crate::layer_drawing::{DrawDepth, DrawList, FrameDraw, KeptDraw, LayerDraws, TileMap, finish_tiles, tile_objects};
use crate::layers::render_hillshade_layer::lat_range;
use crate::log_target;
use crate::paint_parameters::PaintParameters;
use crate::terrain::programs;
use crate::terrain::ubo::{HillshadeTilePropsUBO, TerrainDrawableUBO, TerrainFog, TerrainSurfaceUBO};
use crate::tile_matrix::to_projection_ubo;

/// The ids of the buffers that the draws bind.
const DRAWABLE_ID: usize = programs::TERRAIN.buffer_id("drawableVector");
const LIGHT_TILE_ID: usize = programs::TERRAIN.buffer_id("lightVector");
const SURFACE_ID: usize = programs::TERRAIN.buffer_id("terrain");
const LIGHT_ID: usize = programs::TERRAIN.buffer_id("props");

/// The slots of the programs' textures that the draws bind.
const GROUND_DEM_SLOT: usize = programs::TERRAIN.texture_slot("ground_dem");
const DRAPE_SLOT: usize = programs::TERRAIN.texture_slot("drape");

/// A grid, in the context's stores.
struct SharedMesh {
    /// Whether it has skirts.
    skirts: bool,
    /// Whether it has a cap to the north and to the south pole.
    poles: [bool; 2],
    vertices: SharedVertices,
    vertex_count: usize,
    indexes: SharedIndexes,
    index_count: usize,
}

/// What a tile's draw reads of its own: where it reads its DEM, its
/// projection, and its drape's light.
#[derive(Clone, Copy)]
struct TileUniforms {
    drawable: TerrainDrawableUBO,
    projection: ProjectionUBO,
    light: HillshadeTilePropsUBO,
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
/// translucent pass, a set of the surface's depth and one of the ground's
/// depth, where the frame draws them, nearest the center first,
/// testing and writing the 3D depth range. The sets share the grids, the
/// DEM textures and each tile's projection and DEM coordinates.
#[derive(Default)]
pub(crate) struct TerrainDraws {
    /// The draws of each of the frame's stacks.
    stacks: Vec<StackDraws>,
    /// The draws of the surface's depth, transparent, which draw before the
    /// frame's layers ([`TerrainDraws::render_depth`]): the same draws as
    /// the stacks', so the same depth.
    depth: Option<StackDraws>,
    /// The draws of the ground's depth into its texture
    /// ([`TerrainDraws::render_ground`]).
    ground: Option<StackDraws>,
    /// The surface's shader and the ground depth's, with the variant they
    /// are of.
    shaders: Option<(ProjectionVariant, [ShaderKey; 2])>,
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
    /// tile's drape of their stack, shade it with the stack's light of
    /// `stacks` if it has one, cover it with `fog`, and draw as `surface`
    /// says, and, if `depth_first`, one of the surface's depth, and, if
    /// `ground`, one of the ground's depth; or drops the draws and what they
    /// share for `None`, a frame without terrain, or where there is nothing
    /// to draw. Each tile projects as the frame's view does, onto the globe
    /// on a globe.
    pub(crate) fn update(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        cover: Option<(&TerrainCover, &DemBindings)>,
        (surface, fog): (Surface, TerrainFog),
        (drapes, lights): (&Drapes, &DrapeStacks),
        (depth_first, ground): (bool, bool),
    ) {
        let stacks = drapes.stacks();
        let draws = stacks > 0 || ground;
        let shaders = cover.filter(|_| draws).and_then(|_| self.shaders(gpu, parameters.context, parameters.view));
        let (Some((cover, dems)), Some([shader, depth_shader])) = (cover, shaders) else {
            self.stacks.clear();
            self.depth = None;
            self.ground = None;
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
            self.ground = None;
            self.meshes.clear();
            self.built_for = Some(built_for);
        }

        // A backend whose clip depth runs from -1 writes the depth of one
        // whose runs from 0, as fill extrusions do. The globe projects in its
        // shaders.
        let remap_clip_z = context.caps().clip_depth == ClipDepthRange::NegativeOneToOne && !view.is_globe();
        self.stacks.resize_with(stacks, StackDraws::default);
        for (set, wanted) in [(&mut self.depth, depth_first), (&mut self.ground, ground)] {
            if !wanted {
                *set = None;
            } else if set.is_none() {
                *set = Some(StackDraws::default());
            }
        }
        for stack in self.stacks.iter_mut().chain(&mut self.depth).chain(&mut self.ground) {
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
                light: HillshadeTilePropsUBO::default(),
            };
            let latrange = lat_range(&tile.id.to_unwrapped());
            // The ground's depth is the surface's without the skirts, which
            // fill the cracks between tiles: a skirt that the camera sees
            // edge-on draws the depth of no ground, as a rasterizer makes it.
            let poles = poles(view, tile.id.canonical);
            let surface_mesh = mesh(&mut self.meshes, context, surface.skirts, poles);
            let ground_mesh = self.ground.is_some().then(|| mesh(&mut self.meshes, context, false, poles));
            let meshes = &self.meshes;
            for (set, draws) in sets(&mut self.stacks, &mut self.depth, &mut self.ground) {
                let Some(entry) = draws.tiles.entry(&tile.id) else {
                    continue;
                };
                // Where a stack draws nothing in the tile, it has no drape
                // and no draw, but the first stack's draw writes the depth
                // the stacks above test, so it draws transparent there. The
                // surface depth's draws draw so everywhere.
                let draw = if let Set::Ground = set {
                    let Some(mesh) = ground_mesh.map(|index| &meshes[index]) else {
                        continue;
                    };
                    let objects = || mesh_objects(&depth_shader, ColorMode::unblended(), mesh, cull_face_mode);
                    entry.value.get_or_insert_with(|| KeptDraw::new(objects()))
                } else {
                    let drape = match set {
                        Set::Stack(stack) => {
                            drapes.texture(stack, &tile.id).or_else(|| (stack == 0).then(|| dems.flat()))
                        }
                        Set::Depth | Set::Ground => Some(dems.flat()),
                    };
                    let Some(drape) = drape.cloned() else {
                        *entry.value = None;
                        continue;
                    };
                    let mesh = &meshes[surface_mesh];
                    let objects = || mesh_objects(&shader, ColorMode::alpha_blended(), mesh, cull_face_mode);
                    let draw = entry.value.get_or_insert_with(|| KeptDraw::new(objects()));
                    draw.objects.set_texture(DRAPE_SLOT, Some(&drape));
                    draw
                };
                draw.objects.set_texture(GROUND_DEM_SLOT, Some(dem_texture));
                let frame_draw = FrameDraw {
                    priority: index as i64,
                    depth: DrawDepth::Volume { mask: DepthMaskType::ReadWrite },
                    ..draw.frame_draw(Some(tile.id))
                };
                // A stack without a light, and the surface depth's draws,
                // draw their drapes' colors.
                let light = set
                    .stack()
                    .and_then(|stack| lights.light(stack))
                    .map(|light| HillshadeTilePropsUBO { latrange, ..light.tile });
                draws.draw_list.push(frame_draw, TileUniforms { light: light.unwrap_or_default(), ..uniforms });
            }
        }
        let surface_ubo = TerrainSurfaceUBO::new(&surface.props, &fog);
        for (set, StackDraws { tiles, frame, draw_list }) in sets(&mut self.stacks, &mut self.depth, &mut self.ground) {
            let light = set.stack().and_then(|stack| lights.light(stack)).map(|light| light.props).unwrap_or_default();
            finish_tiles(draw_list, frame, tiles, gpu, context, |writer| {
                writer.storage(DRAWABLE_ID, |uniforms| uniforms.drawable);
                writer.storage(PROJECTION_ID, |uniforms| uniforms.projection);
                writer.storage(LIGHT_TILE_ID, |uniforms| uniforms.light);
                writer.uniform(SURFACE_ID, &surface_ubo);
                writer.uniform(LIGHT_ID, &light);
            });
        }
    }

    /// Whether the backend failed to make the surface's shader, which then
    /// draws nothing until the shaders are made again.
    pub(crate) fn failed(&self) -> bool {
        self.failed
    }

    /// The surface's shader and the ground depth's in `view`'s projection,
    /// made on `gpu` in `context` the first time; `None`, logged once, where
    /// the backend has either not.
    pub(crate) fn shaders(&mut self, gpu: &mut dyn Gpu, context: &mut Context, view: &View) -> Option<[ShaderKey; 2]> {
        let variant = variant(view);
        if let Some((made_for, shaders)) = &self.shaders
            && *made_for == variant
        {
            return Some(shaders.clone());
        }
        if self.failed {
            return None;
        }
        let shaders = [&programs::TERRAIN, &programs::TERRAIN_DEPTH]
            .map(|program| context.shader_key(program, Uniforms::default(), variant));
        if let Err(error) = shaders.iter().try_for_each(|shader| context.shader(gpu, shader).map(|_| ())) {
            self.failed = true;
            log::warn!(target: log_target::RENDER, "Terrain draws nothing on this backend: {error}");
            return None;
        }
        Some(self.shaders.insert((variant, shaders)).1.clone())
    }

    /// Uploads the GPU objects of the frame's draws.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        for StackDraws { tiles, frame, .. } in self.stacks.iter_mut().chain(&mut self.depth).chain(&mut self.ground) {
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

    /// Whether the frame's draws draw the ground's depth.
    pub(crate) fn draws_ground(&self) -> bool {
        self.ground.is_some()
    }

    /// Draws the frame's draws of the surface's depth, if it made them, into
    /// the open pass of `gpu`, the context's: the frame's pass, before its
    /// layers.
    pub(crate) fn render_depth(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if let Some(StackDraws { tiles, frame, .. }) = &mut self.depth {
            frame.render(tiles, tile_objects, gpu, parameters);
        }
    }

    /// Draws the frame's draws of the ground's depth, if it made them, into
    /// the open pass of `gpu`, the context's: the ground's pass.
    pub(crate) fn render_ground(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) {
        if let Some(StackDraws { tiles, frame, .. }) = &mut self.ground {
            frame.render(tiles, tile_objects, gpu, parameters);
        }
    }
}

/// A set of the terrain's draws.
#[derive(Clone, Copy)]
enum Set {
    /// A stack of draped layers' surface.
    Stack(usize),
    /// The surface's depth, in the frame's pass.
    Depth,
    /// The ground's depth, in its own pass.
    Ground,
}

impl Set {
    /// The stack, for a stack's set.
    fn stack(self) -> Option<usize> {
        match self {
            Set::Stack(stack) => Some(stack),
            Set::Depth | Set::Ground => None,
        }
    }
}

/// The sets of draws: each of `stacks`, then `depth` and `ground` where the
/// frame has them.
fn sets<'a>(
    stacks: &'a mut [StackDraws],
    depth: &'a mut Option<StackDraws>,
    ground: &'a mut Option<StackDraws>,
) -> impl Iterator<Item = (Set, &'a mut StackDraws)> {
    let stacks = stacks.iter_mut().enumerate().map(|(stack, draws)| (Set::Stack(stack), draws));
    stacks
        .chain(depth.iter_mut().map(|draws| (Set::Depth, draws)))
        .chain(ground.iter_mut().map(|draws| (Set::Ground, draws)))
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

/// The index in `meshes` of the grid with `skirts` and the caps of `poles`,
/// made in the stores of `context` the first time.
fn mesh(meshes: &mut Vec<SharedMesh>, context: &mut Context, skirts: bool, poles: [bool; 2]) -> usize {
    if let Some(index) = meshes.iter().position(|mesh| mesh.skirts == skirts && mesh.poles == poles) {
        return index;
    }
    let TerrainMesh { vertices, indices } = TerrainMesh::new(skirts, poles);
    meshes.push(SharedMesh {
        skirts,
        poles,
        vertex_count: vertices.len(),
        vertices: context.add_vertices(vertices),
        index_count: indices.len(),
        indexes: context.add_indexes(Indexes::new(indices)),
    });
    meshes.len() - 1
}

/// The GPU objects of a tile's draw of `mesh` with `shader`, in
/// `color_mode`, culling as `cull_face_mode` says.
fn mesh_objects(
    shader: &ShaderKey,
    color_mode: ColorMode,
    mesh: &SharedMesh,
    cull_face_mode: CullFaceMode,
) -> DrawObjects {
    let mut vertices = VertexAttributeArray::new();
    vertices.set_shared(
        "pos",
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
