//! The terrain's blocks, and the lights of a hillshade's slopes, which the
//! terrain's surface shades a drape of a hillshade's slopes with, as the
//! hillshade does.

use mln_gfx::shaders::block::uniform_block;

uniform_block! {
    /// A terrain tile's draw block, an entry of the storage buffer that the
    /// terrain's draws index by `globalIndex`. The tile projects through its
    /// entry of the programs' [`PROJECTION`](mln_gfx::shaders::program::PROJECTION)
    /// buffer.
    pub struct TerrainDrawableUBO {
        /// Where the tile reads its DEM: the scale and offset that map the
        /// tile's uv to the DEM's (`uv * scale + offset`), then the DEM's width
        /// in pixels. A width of 0 is flat ground.
        pub dem_coords: [f32; 4],
    }
}

/// How the terrain's surface stands: how its DEM's texels decode to
/// heights, as [`crate::tile::dem::DemUnpack`] does, the exaggeration, and how
/// far the surface's skirts reach down. The first members of a
/// [`TerrainSurfaceUBO`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TerrainProps {
    /// The weights of a texel's red, green and blue channels, from 0 to
    /// 255, then the base added to their sum.
    pub unpack: [f32; 4],
    /// What the sum is multiplied by, to meters.
    pub unpack_scale: f32,
    pub exaggeration: f32,
    /// In meters.
    pub skirt_length: f32,
}

/// The fog over the terrain's surface, from the style's `sky`, the last
/// members of a [`TerrainSurfaceUBO`]. It covers a fragment of the surface
/// by how far along the view it is: from none at `ground_blend` of the depth
/// range to all at its far end, in `fog_color` that turns to `horizon_color`
/// past `horizon_blend`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TerrainFog {
    /// Linear, through a gamma of 2.2, and premultiplied.
    pub fog_color: [f32; 4],
    /// Linear and premultiplied, as `fog_color`.
    pub horizon_color: [f32; 4],
    /// The distances along the view, in pixels, at which the fog's depth is
    /// 0 and 1, the second beyond the first where there is fog. The depth
    /// between them is a perspective depth, which grows fastest near the
    /// first.
    pub depth_range: [f32; 2],
    /// Where in the depth the fog starts, from 0 to 1. At 1 there is no fog.
    pub ground_blend: f32,
    /// Where in the depth the fog starts to turn to the horizon's color,
    /// from 0 to 1.
    pub horizon_blend: f32,
}

impl TerrainFog {
    /// No fog.
    pub const NONE: TerrainFog = TerrainFog {
        fog_color: [0.0; 4],
        horizon_color: [0.0; 4],
        depth_range: [0.0; 2],
        ground_blend: 1.0,
        horizon_blend: 1.0,
    };
}

uniform_block! {
    /// The uniform block of the terrain's programs: how the surface stands
    /// ([`TerrainProps`]), which its vertex stage reads, and the fog over it
    /// ([`TerrainFog`]), which its fragment stage reads.
    pub struct TerrainSurfaceUBO {
        pub unpack: [f32; 4],
        pub unpack_scale: f32,
        pub exaggeration: f32,
        pub skirt_length: f32,
        pub pad0: f32,
        pub fog_color: [f32; 4],
        pub horizon_color: [f32; 4],
        pub depth_range: [f32; 2],
        pub ground_blend: f32,
        pub horizon_blend: f32,
    }
}

impl TerrainSurfaceUBO {
    /// The block of a surface that stands as `props` say, under `fog`.
    #[must_use]
    pub const fn new(props: &TerrainProps, fog: &TerrainFog) -> TerrainSurfaceUBO {
        TerrainSurfaceUBO {
            unpack: props.unpack,
            unpack_scale: props.unpack_scale,
            exaggeration: props.exaggeration,
            skirt_length: props.skirt_length,
            pad0: 0.0,
            fog_color: fog.fog_color,
            horizon_color: fog.horizon_color,
            depth_range: fog.depth_range,
            ground_blend: fog.ground_blend,
            horizon_blend: fog.horizon_blend,
        }
    }
}

/// Maximum number of illumination sources supported
pub const MAX_ILLUMINATION_SOURCES: usize = 4;

uniform_block! {
    /// The hillshade shader's per-tile uniform block.
    pub struct HillshadeTilePropsUBO {
        pub latrange: [f32; 2],
        pub exaggeration: f32,
        pub method: i32,
        pub num_lights: i32,
        pub pad0: f32,
        pub pad1: f32,
        pub pad2: f32,
    }
}

uniform_block! {
    /// Evaluated properties that do not depend on the tile. The
    /// colors are the premultiplied `mln::Color` as r, g, b, a floats.
    pub struct HillshadeEvaluatedPropsUBO {
        pub accent: [f32; 4],
        pub altitudes: [f32; 4],
        pub azimuths: [f32; 4],
        pub shadows: [[f32; 4]; MAX_ILLUMINATION_SOURCES],
        pub highlights: [[f32; 4]; MAX_ILLUMINATION_SOURCES],
    }
}
