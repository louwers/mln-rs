use bytemuck::{Pod, Zeroable};

//
// Line

mln_gfx::uniform_block! {
    /// The line shader's per-drawable uniform block.
    pub struct LineDrawableUBO {
        pub matrix: [f32; 4 * 4],
        #[glsl_es(mediump)]
        pub ratio: f32,
        // Interpolations
        #[glsl_es(lowp)]
        pub color_t: f32,
        #[glsl_es(lowp)]
        pub blur_t: f32,
        #[glsl_es(lowp)]
        pub opacity_t: f32,
        #[glsl_es(lowp)]
        pub gapwidth_t: f32,
        #[glsl_es(lowp)]
        pub offset_t: f32,
        #[glsl_es(lowp)]
        pub width_t: f32,
        #[padding]
        pub pad1: f32,
    }
}

//
// Line gradient

mln_gfx::uniform_block! {
    /// The line gradient shader's per-drawable uniform block.
    pub struct LineGradientDrawableUBO {
        pub matrix: [f32; 4 * 4],
        #[glsl_es(mediump)]
        pub ratio: f32,
        // Interpolations
        #[glsl_es(lowp)]
        pub blur_t: f32,
        #[glsl_es(lowp)]
        pub opacity_t: f32,
        #[glsl_es(lowp)]
        pub gapwidth_t: f32,
        #[glsl_es(lowp)]
        pub offset_t: f32,
        #[glsl_es(lowp)]
        pub width_t: f32,
        #[padding]
        pub pad1: f32,
        #[padding]
        pub pad2: f32,
    }
}

//
// Line pattern

mln_gfx::uniform_block! {
    /// The line pattern shader's per-drawable uniform block.
    pub struct LinePatternDrawableUBO {
        pub matrix: [f32; 4 * 4],
        #[glsl_es(mediump)]
        pub ratio: f32,
        // Interpolations
        #[glsl_es(lowp)]
        pub blur_t: f32,
        #[glsl_es(lowp)]
        pub opacity_t: f32,
        #[glsl_es(lowp)]
        pub gapwidth_t: f32,
        #[glsl_es(lowp)]
        pub offset_t: f32,
        #[glsl_es(lowp)]
        pub width_t: f32,
        #[glsl_es(lowp)]
        pub pattern_from_t: f32,
        #[glsl_es(lowp)]
        pub pattern_to_t: f32,
        #[glsl_es(lowp)]
        pub floorwidth_t: f32,
        #[padding]
        pub pad1: f32,
        #[padding]
        pub pad2: f32,
        #[padding]
        pub pad3: f32,
    }
}

mln_gfx::uniform_block! {
    /// The line pattern shader's per-tile uniform block.
    pub struct LinePatternTilePropsUBO {
        pub pattern_from: [f32; 4],
        pub pattern_to: [f32; 4],
        #[glsl_es(mediump)]
        pub scale: [f32; 4],
        pub texsize: [f32; 2],
        pub fade: f32,
        #[padding]
        pub pad1: f32,
    }
}

//
// Line SDF

mln_gfx::uniform_block! {
    /// The line SDF shader's per-drawable uniform block.
    pub struct LineSDFDrawableUBO {
        pub matrix: [f32; 4 * 4],
        pub patternscale_a: [f32; 2],
        pub patternscale_b: [f32; 2],
        pub tex_y_a: f32,
        pub tex_y_b: f32,
        #[glsl_es(mediump)]
        pub ratio: f32,
        // Interpolations
        #[glsl_es(lowp)]
        pub color_t: f32,
        #[glsl_es(lowp)]
        pub blur_t: f32,
        #[glsl_es(lowp)]
        pub opacity_t: f32,
        #[glsl_es(lowp)]
        pub gapwidth_t: f32,
        #[glsl_es(lowp)]
        pub offset_t: f32,
        #[glsl_es(lowp)]
        pub width_t: f32,
        #[glsl_es(lowp)]
        pub floorwidth_t: f32,
        /// The height of the bucket's dash atlas, where the dash array differs
        /// by feature: the shaders then read the patterns from an attribute.
        pub atlas_height: f32,
        #[padding]
        pub pad2: f32,
    }
}

mln_gfx::uniform_block! {
    /// The line SDF shader's per-tile uniform block.
    pub struct LineSDFTilePropsUBO {
        pub sdfgamma: f32,
        pub mix: f32,
        #[padding]
        pub pad1: f32,
        #[padding]
        pub pad2: f32,
    }
}

mln_gfx::uniform_block! {
    /// Evaluated properties that do not depend on the tile.
    pub struct LineEvaluatedPropsUBO {
        pub color: [f32; 4],
        #[glsl_es(lowp)]
        pub blur: f32,
        #[glsl_es(lowp)]
        pub opacity: f32,
        #[glsl_es(mediump)]
        pub gapwidth: f32,
        #[glsl_es(lowp)]
        pub offset: f32,
        #[glsl_es(mediump)]
        pub width: f32,
        #[glsl_es(lowp)]
        pub floorwidth: f32,
        #[padding]
        pub pad0: f32,
        #[padding]
        pub pad1: f32,
    }
}

/// Any line per-drawable uniform block, for consolidated UBOs: 128 bytes,
/// the size of `LineSDFDrawableUBO`.
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LineDrawableUnionUBO {
    bytes: [u8; 128],
}

impl Default for LineDrawableUnionUBO {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl LineDrawableUnionUBO {
    /// Assigns one union member (`drawableUBOVector[i].lineDrawableUBO = ...`).
    pub fn set<T: Pod>(&mut self, member: &T) {
        let src = bytemuck::bytes_of(member);
        self.bytes[..src.len()].copy_from_slice(src);
    }
}

/// Any line per-tile uniform block, for consolidated UBOs: 64 bytes, the
/// size of `LinePatternTilePropsUBO`.
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
#[repr(C, align(16))]
pub struct LineTilePropsUnionUBO {
    bytes: [u8; 64],
}

impl Default for LineTilePropsUnionUBO {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl LineTilePropsUnionUBO {
    /// Assigns one union member.
    pub fn set<T: Pod>(&mut self, member: &T) {
        let src = bytemuck::bytes_of(member);
        self.bytes[..src.len()].copy_from_slice(src);
    }
}
