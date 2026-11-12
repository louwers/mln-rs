// Ports include/mln/gfx/globe_clip_mask.hpp.

use mln_tile_id::CanonicalTileID;

use crate::shaders::layer_ubo::ProjectionUBO;

/// A tile clipping mask on the globe: the tile's projection block, drawn over
/// its pole-capped mesh.
#[derive(Clone, Copy, Debug)]
pub struct GlobeClipMask {
    pub projection: ProjectionUBO,
    pub stencil_ref: u32,
    pub tile: CanonicalTileID,
}

/// Makes the pole-capped mesh of a globe tile, with or without its borders:
/// returns the mesh's raw `Short2` vertices and its indices.
pub type GlobeClipMeshFn<'a> = dyn Fn(&CanonicalTileID, bool) -> (Vec<u8>, Vec<u16>) + 'a;
