mln_gfx::uniform_block! {
    /// Evaluated properties that do not depend on the tile.
    /// `color` is the premultiplied `mln::Color` as r, g, b, a floats.
    pub struct BackgroundPropsUBO {
        pub color: [f32; 4],
        pub opacity: f32,
        #[padding]
        pub pad1: f32,
        #[padding]
        pub pad2: f32,
        #[padding]
        pub pad3: f32,
    }
}

mln_gfx::uniform_block! {
    /// A pattern draw's block: where its tile is in the world's pixels,
    /// which the pattern repeats from.
    pub struct BackgroundPatternDrawableUBO {
        pub pixel_coord_upper: [f32; 2],
        pub pixel_coord_lower: [f32; 2],
        pub tile_units_to_pixels: f32,
        #[padding]
        pub pad1: f32,
        #[padding]
        pub pad2: f32,
        #[padding]
        pub pad3: f32,
    }
}

mln_gfx::uniform_block! {
    /// Evaluated properties that do not depend on the tile, and the size of the
    /// pattern atlas in pixels.
    pub struct BackgroundPatternPropsUBO {
        pub pattern_tl_a: [f32; 2],
        pub pattern_br_a: [f32; 2],
        pub pattern_tl_b: [f32; 2],
        pub pattern_br_b: [f32; 2],
        pub pattern_size_a: [f32; 2],
        pub pattern_size_b: [f32; 2],
        pub scale_a: f32,
        pub scale_b: f32,
        pub mix: f32,
        pub opacity: f32,
        pub texsize: [f32; 2],
        #[padding]
        pub pad1: f32,
        #[padding]
        pub pad2: f32,
    }
}
