//! The terrain's draws: one per terrain tile and stack of draped layers, of
//! the shared grid, each with the tile's matrix and where it reads its DEM
//! in an entry of its stack's storage buffer, raised by the DEM's texture
//! and sampling the tile's drape of the stack.

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
use mln_gfx::types::CullFaceWindingType;
use mln_gfx::{AttributeDataType, DepthMaskType, DrawMode};
use mln_map_types::ViewportMode;
use mln_util::clip_depth::ClipDepthRange;
use mln_util::logging::{Event, Log};

use super::cover::dem_bucket;
use super::dem_textures::DemTextures;
use super::mesh::{TerrainMesh, TerrainVertex};
use super::{Drapes, Surface, TerrainCover};
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
use crate::tile::source_tiles::SourceTiles;

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
/// depth range. The stacks share the grid, the DEM textures and each tile's
/// matrix and DEM coordinates.
#[derive(Default)]
pub(crate) struct TerrainDraws {
    /// The draws of each of the frame's stacks.
    stacks: Vec<StackDraws>,
    shader: Option<ShaderKey>,
    mesh: Option<SharedMesh>,
    /// The textures of the DEM tiles the tiles read.
    dem_textures: DemTextures,
    /// Whether the backend has no terrain shader, which draws nothing then.
    unsupported: bool,
}

impl TerrainDraws {
    /// Makes the frame's draws of `cover`'s tiles, one for each of the
    /// stacks of `drapes`, which read their DEM from the tiles of `dem`,
    /// sample the tile's drape of their stack and draw as `surface` says, or
    /// drops the draws and what they share for `None`, a frame without
    /// terrain.
    pub(crate) fn update(
        &mut self,
        gpu: &mut dyn Gpu,
        parameters: &mut PaintParameters<'_>,
        cover: Option<&TerrainCover>,
        dem: Option<&SourceTiles>,
        surface: Surface,
        drapes: &Drapes,
    ) {
        let shader = cover.and_then(|_| self.shader(gpu, parameters));
        let (Some(cover), Some(shader)) = (cover, shader) else {
            self.stacks.clear();
            self.mesh = None;
            self.dem_textures = DemTextures::default();
            return;
        };

        let winding = match parameters.view.viewport_mode() {
            ViewportMode::Default => CullFaceWindingType::CounterClockwise,
            ViewportMode::FlippedY => CullFaceWindingType::Clockwise,
        };
        let cull_face_mode = CullFaceMode { winding, ..CullFaceMode::back_ccw() };
        let context = &mut *parameters.context;
        let built_for = (surface.skirts, cull_face_mode);
        if self.mesh.as_ref().is_some_and(|mesh| (mesh.skirts, mesh.cull_face_mode) != built_for) {
            // The draws hold the other grid, or cull the other way.
            self.stacks.clear();
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
        self.stacks.resize_with(drapes.stacks(), StackDraws::default);
        for stack in &mut self.stacks {
            stack.tiles.set_tiles(cover.tiles.iter().map(|tile| tile.id));
        }
        for (index, tile) in cover.tiles.iter().enumerate() {
            let bound = tile.dem.and_then(|(id, coords)| Some((dem_bucket(dem?.tile(&id)?)?, coords)));
            let (dem_texture, dem_coords) = match bound {
                Some((bucket, coords)) => {
                    let dim = bucket.dem_data().dim as f32;
                    (
                        self.dem_textures.texture(context, bucket),
                        [coords.scale, coords.offset[0], coords.offset[1], dim],
                    )
                }
                None => (self.dem_textures.flat(context), [0.0; 4]),
            };
            let mut matrix = *proj_matrix * view.tile_matrix(&tile.id.to_unwrapped());
            if remap_clip_z {
                zero_to_one_clip_z(&mut matrix);
            }
            let drawable = TerrainDrawableUBO { matrix: cast_matrix(&matrix), dem_coords };
            for (stack, draws) in self.stacks.iter_mut().enumerate() {
                let Some(entry) = draws.tiles.entry(&tile.id) else {
                    continue;
                };
                let draw = entry.value.get_or_insert_with(|| KeptDraw::new(mesh_objects(&shader, mesh)));
                draw.objects.set_texture(ID_TERRAIN_DEM_TEXTURE, Some(&dem_texture));
                draw.objects.set_texture(ID_TERRAIN_DRAPE_TEXTURE, drapes.texture(stack, index, &tile.id));
                let frame_draw = FrameDraw {
                    priority: index as i64,
                    depth: DrawDepth::Volume { mask: DepthMaskType::ReadWrite },
                    ..draw.frame_draw(Some(tile.id))
                };
                draws.draw_list.push(frame_draw, drawable);
            }
        }
        self.dem_textures.finish_frame();
        for StackDraws { tiles, frame, draw_list } in &mut self.stacks {
            finish_tiles(draw_list, frame, tiles, gpu, context, |writer| {
                writer.storage(ID_TERRAIN_DRAWABLE_UBO, |drawable| *drawable);
                // GL's program declares the projection block whether it
                // reads it or not, so every draw binds one of zeros.
                writer.storage(ID_PROJECTION_UBO, |_| ProjectionUBO::default());
                writer.uniform(ID_TERRAIN_PROPS_UBO, &surface.props);
            });
        }
    }

    /// Whether the backend has the terrain's shader, which this makes on
    /// `gpu` the first time.
    pub(crate) fn supported(&mut self, gpu: &mut dyn Gpu, parameters: &mut PaintParameters<'_>) -> bool {
        self.shader(gpu, parameters).is_some()
    }

    /// The terrain's shader, made on `gpu` the first time; `None`, logged
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

    /// Uploads the GPU objects of the frame's draws.
    pub(crate) fn upload(&mut self, gpu: &mut dyn Gpu, context: &mut Context) {
        for StackDraws { tiles, frame, .. } in &mut self.stacks {
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
}

/// The GPU objects of a tile's draw of `mesh` with `shader`: alpha-blended,
/// without the triangles that face away.
fn mesh_objects(shader: &ShaderKey, mesh: &SharedMesh) -> DrawObjects {
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
    DrawObjects::new(shader.clone(), ColorMode::alpha_blended(), mesh.cull_face_mode, geometry, Textures::default())
}
