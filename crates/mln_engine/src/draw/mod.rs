//! What a layer draws with: the other half of the [layer
//! contract](crate::layer).
//!
//! A layer makes its GPU objects through the [`Resources`] its hooks get and
//! keeps their handles, from vertices, indexes and textures in the
//! [`Context`]'s stores, the meshes the layers share
//! ([`RenderStaticData`], [`globe`]) and the quads of a tile's mask
//! ([`StoredQuads`]): [`Mesh`]es, which draw a program over geometry with
//! textures, and [`Target`]s, textures of the layer's own that its draws
//! draw into ahead of the frame and that its other draws sample. An object
//! goes once its last handle drops and the GPU is done with it.
//!
//! Each frame, a layer pushes its draws into its [`Draws`]: into its main
//! [`Group`], which draws at its place in the layers' order and depth, and
//! into the groups of its targets. A [`Draw`] of a mesh says where it is
//! among the group's draws, whether it may draw in the opaque pass, how it
//! tests depth ([`Depth`]) and whether it clips to its tile, and comes with
//! the values of its program's blocks of each draw and, for a draw of a
//! tile, the tile's projection. The engine writes the blocks once for the
//! group, uploads the meshes, and draws the group in the frame's passes.
//!
//! A layer sees its frame as a [`LayerFrame`]: the map's, or a drape's. A
//! layer that draws with the backend's own commands asks the frame to lend
//! them ([`Draws::lend`]), at a time in the frame ([`LendAt`]), and gets them
//! with the frame and where it draws the map ([`Lent`]). What it makes with
//! them, the engine doesn't know of: the layer frees it once the GPU is done
//! with it, through work it queues for after the GPU ([`AfterGpu`]).
//!
//! The objects and their handles belong to the renderer's thread.
//!
//! Inside the engine, the renderer's graphics state over its backend's `Gpu`
//! (`context`) keeps the stores of what draws share, and the queue that
//! releases it in order.

pub(crate) mod after_gpu;
pub(crate) mod atlas_binding;
pub(crate) mod buffer;
pub(crate) mod context;
pub(crate) mod draws;
pub(crate) mod dynamic_texture;
pub(crate) mod dynamic_texture_atlas;
pub(crate) mod identity;
pub(crate) mod indexes;
pub(crate) mod mesh;
pub(crate) mod objects;
pub(crate) mod owned;
pub(crate) mod quads;
pub(crate) mod release;
pub(crate) mod resources;
pub(crate) mod segment;
pub(crate) mod store;
pub(crate) mod target;
pub(crate) mod texture2d;
pub(crate) mod tile_render_data;
pub(crate) mod uniform_buffer;
pub(crate) mod upload;
pub(crate) mod vertex_attribute;
pub(crate) mod vertices;

/// `m` as the column-major `f32`s that a block's matrix holds.
#[must_use]
pub fn cast_matrix(m: &glam::DMat4) -> [f32; 16] {
    m.as_mat4().to_cols_array()
}

/// Copies again into `mesh`'s paint attributes, through `refresh`, the
/// values of its bucket's paint properties that `changes` says feature
/// state rewrote: a layer's [`prepare`](crate::layer::Layer::prepare) calls
/// it with what its tile's entry took (`TileEntry::take_changes`).
pub fn refresh_paint_attributes(
    mesh: &Mesh,
    context: &mut Context,
    changes: crate::layer::BucketChanges,
    refresh: impl FnOnce(&mut Context, &mut VertexAttributeArray),
) {
    if !changes.paint {
        return;
    }
    mesh.update(context, |objects, context| refresh(context, objects.paint_attributes_mut()));
}

/// The size of a texture of an image of `size`.
#[must_use]
pub fn texture_size(size: mln_style::size::Size) -> mln_gfx::Size {
    mln_gfx::Size::new(size.width, size.height)
}

pub use crate::draw::after_gpu::{AfterGpu, GpuEnd};
pub use crate::draw::atlas_binding::AtlasBinding;
pub use crate::draw::context::Context;
pub use crate::draw::draws::{Depth, Draw, Draws, Group, LayerFrame, LendAt, Lent};
pub use crate::draw::identity::SimpleIdentity;
pub use crate::draw::indexes::{DrawIndexes, Indexes};
pub use crate::draw::mesh::{Indexed, add_point_quads};
pub use crate::draw::objects::{DrawGeometry, Mesh, Textures, copy_shared_attributes, raw_vertices};
pub use crate::draw::quads::{RasterLayoutVertex, StoredQuads, mask_quads};
pub use crate::draw::resources::Resources;
pub use crate::draw::segment::{MAX_SEGMENT_VERTICES, SegmentBase};
pub use crate::draw::store::{SharedIndexes, SharedTexture};
pub use crate::draw::target::Target;
pub use crate::draw::texture2d::Texture2D;
pub use crate::draw::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
pub use crate::draw::vertices::{SharedVertices, TypedVertices, Vertices};
pub use crate::render::frame_projection::zero_to_one_clip_z;
pub use crate::render::layer_composite::Composite;
pub use crate::render::light::EvaluatedLight;
pub use crate::render::pass::RenderPass;
pub use crate::render::static_data::RenderStaticData;
pub use crate::render::tile::RenderTile;
pub use crate::render::tile_matrix::{globe_extrude_scale, projection_data, to_projection_ubo};
pub use crate::text::atlas::{ImagePosition, ImagePositions, RectU16};

/// The globe's tile meshes, which a layer that draws whole tiles draws on
/// the globe, and the rows of the poles' vertices.
pub mod globe {
    pub use crate::render::globe::globe_tile_mesh::{GlobeTileMeshCache, RawGlobeTileMesh, raw_globe_tile_mesh};
    pub use crate::render::globe::{NORTH_POLE_Y, SOUTH_POLE_Y};
}

/// What a program of a layer that stands on terrain or hides behind it
/// declares. A program of a layer whose type stands on terrain
/// ([`OnTerrain::Stands`](crate::layer::OnTerrain::Stands)) declares the
/// attribute [`GROUND_HEIGHT`](crate::draw::ground::GROUND_HEIGHT), which the engine fills, for a draw of its
/// terrain variant, with the height of the ground under each of the draw's
/// points; one whose type hides behind the ground in front of it declares the
/// texture [`GROUND_DEPTH`](crate::draw::ground::GROUND_DEPTH), which the engine binds to the depth of the
/// frame's ground.
pub mod ground {
    pub use crate::render::terrain::programs::{GROUND_DEM, GROUND_DEPTH, GROUND_HEIGHT, TERRAIN_SURFACE};
    pub use crate::render::terrain::{
        FrameGround, GroundHeights, StandingDraw, TerrainElevation, TileGround, height_attribute, point_center,
    };

    /// The GLSL snippets of the ground: `ground_height!()`, which a program
    /// that stands on terrain reads the ground's height with,
    /// `ground_visibility!()`, which a program that hides behind the ground
    /// declares, `hillshade_shading!()`, the hillshade's shading of its
    /// slopes, which the terrain's surface shades a lit hillshade's drapes
    /// with, and `terrain_fog!()` and `terrain_props!()`, the terrain
    /// surface's vertex prelude, for a program that reads the surface as the
    /// terrain's programs do.
    pub mod glsl {
        pub use crate::{
            terrain_glsl_ground_height as ground_height, terrain_glsl_ground_visibility as ground_visibility,
            terrain_glsl_hillshade_shading as hillshade_shading, terrain_glsl_terrain_fog as terrain_fog,
            terrain_glsl_terrain_props as terrain_props,
        };
    }

    /// [`glsl`]'s snippets in GLSL ES, but the fog, which is in its
    /// `terrain_props!()`, and the names of the terrain's blocks.
    pub mod glsl_es {
        pub use crate::render::terrain::glsl_es::TERRAIN_NAMES;
        pub use crate::{
            terrain_glsl_es_ground_height as ground_height, terrain_glsl_es_ground_visibility as ground_visibility,
            terrain_glsl_es_hillshade_shading as hillshade_shading, terrain_glsl_es_terrain_props as terrain_props,
        };
    }

    /// [`glsl`]'s snippets in MSL, but the surface's prelude.
    pub mod msl {
        pub use crate::{
            terrain_msl_ground_height as ground_height, terrain_msl_ground_visibility as ground_visibility,
            terrain_msl_hillshade_shading as hillshade_shading,
        };
    }

    /// [`glsl`]'s snippets in WGSL, with the surface's prelude as one text.
    pub mod wgsl {
        pub use crate::render::terrain::wgsl::TERRAIN_PRELUDE;
        pub use crate::{
            terrain_wgsl_ground_height as ground_height, terrain_wgsl_ground_visibility as ground_visibility,
            terrain_wgsl_hillshade_shading as hillshade_shading,
        };
    }
}
