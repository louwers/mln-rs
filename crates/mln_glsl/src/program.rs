// A program's GLSL reads the program's attribute at location `l` as
// `layout(location = l)`, but where `ShaderSource::locations` says otherwise,
// and its texture at slot `s` and the texture's sampler at
// `layout::image_texture_binding(s)` and `layout::image_sampler_binding(s)`
// of the image set.

use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::programs;

use crate::front_end::Stage;
use crate::sources;

/// A program's GLSL.
///
/// A stage compiles from the stage's prelude (`sources::PRELUDE_VERTEX` or
/// `PRELUDE_FRAGMENT`), `prelude` and the stage's source
/// (`ShaderSource::compose`).
#[derive(Debug)]
pub(crate) struct GlslShaderSource {
    /// The shader's own prelude constant (its buffer bindings), "" if none.
    pub(crate) prelude: &'static str,
    pub(crate) vertex: &'static str,
    pub(crate) fragment: &'static str,
}

/// A program's GLSL.
#[derive(Debug)]
pub struct ShaderSource {
    pub(crate) glsl: &'static GlslShaderSource,
    /// The location the GLSL reads each of the program's attributes at, by
    /// the program's location of it, where they differ: the fill
    /// extrusions' ground height, at 10.
    pub(crate) locations: Option<&'static [u32]>,
}

impl ShaderSource {
    /// The location the GLSL reads the program's attribute at `location`
    /// ([`Program::location`]) at.
    #[must_use]
    pub fn location(&self, location: usize) -> u32 {
        self.locations.and_then(|locations| locations.get(location).copied()).unwrap_or(location as u32)
    }

    /// The source of `stage` for naga's GLSL front end, with `defines`
    /// (`#define` lines): the version, the defines, the stage's prelude, and
    /// the program's own prelude and stage.
    #[must_use]
    pub fn compose(&self, stage: Stage, defines: &str) -> String {
        let source = match stage {
            Stage::Vertex => self.glsl.vertex,
            Stage::Fragment => self.glsl.fragment,
        };
        crate::front_end::compose(stage, defines, self.glsl.prelude, source)
    }
}

macro_rules! shader_source {
    ($name:ident) => {
        static $name: ShaderSource = ShaderSource { glsl: &sources::$name, locations: None };
    };
    ($name:ident, $glsl:ident, $locations:expr) => {
        static $name: ShaderSource = ShaderSource { glsl: &sources::$glsl, locations: Some(&$locations) };
    };
}

shader_source!(BACKGROUND_SHADER);
shader_source!(BACKGROUND_PATTERN_SHADER);
shader_source!(CIRCLE_SHADER);
shader_source!(CLIPPING_MASK_PROGRAM);
shader_source!(COLLISION_BOX_SHADER);
shader_source!(COLLISION_CIRCLE_SHADER);
shader_source!(COLOR_RELIEF_SHADER);
shader_source!(FILL_SHADER);
shader_source!(FILL_OUTLINE_SHADER);
shader_source!(FILL_PATTERN_SHADER);
shader_source!(FILL_OUTLINE_PATTERN_SHADER);
shader_source!(FILL_OUTLINE_INSTANCED_SHADER);
shader_source!(FILL_OUTLINE_PATTERN_INSTANCED_SHADER);
shader_source!(FILL_EXTRUSION_SHADER, FILL_EXTRUSION_SHADER, [0, 1, 2, 3, 4, 10]);
shader_source!(FILL_EXTRUSION_INSTANCED_SHADER, FILL_EXTRUSION_INSTANCED_SHADER, [0, 1, 2, 3, 4, 5, 6, 7, 10]);
// The wall shaders run the instanced ones' GLSL with every attribute per
// vertex, for walls whose quads are in the vertex buffer.
shader_source!(FILL_EXTRUSION_WALL_SHADER, FILL_EXTRUSION_INSTANCED_SHADER, [0, 1, 2, 3, 4, 5, 6, 7, 10]);
shader_source!(FILL_EXTRUSION_PATTERN_SHADER, FILL_EXTRUSION_PATTERN_SHADER, [0, 1, 2, 3, 4, 5, 10]);
shader_source!(
    FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 10]
);
shader_source!(
    FILL_EXTRUSION_PATTERN_WALL_SHADER,
    FILL_EXTRUSION_PATTERN_INSTANCED_SHADER,
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 10]
);
// The pulled walls and outlines have no vertex inputs: they read the
// vertices from storage buffers.
shader_source!(FILL_EXTRUSION_PULLED_SHADER);
shader_source!(FILL_OUTLINE_PULLED_SHADER);
shader_source!(GLOBE_DEPTH_SHADER);
shader_source!(HEATMAP_SHADER);
shader_source!(HEATMAP_TEXTURE_SHADER);
shader_source!(LAYER_COMPOSITE_SHADER);
#[cfg(any(test, feature = "gpu-conformance"))]
shader_source!(VERTEX_TEXEL_SHADER);
shader_source!(TERRAIN_SHADER);
shader_source!(TERRAIN_DEPTH_SHADER);
#[cfg(any(test, feature = "gpu-conformance"))]
shader_source!(TERRAIN_PROBE_SHADER);
shader_source!(HILLSHADE_SHADER);
shader_source!(HILLSHADE_PREPARE_SHADER);
shader_source!(LINE_SHADER);
shader_source!(LINE_GRADIENT_SHADER);
shader_source!(LINE_PATTERN_SHADER);
shader_source!(LINE_SDF_SHADER);
shader_source!(LINE_GRADIENT_SDF_SHADER);
shader_source!(RASTER_SHADER);
shader_source!(SKY_SHADER);
shader_source!(ATMOSPHERE_SHADER);
shader_source!(SYMBOL_ICON_SHADER);
shader_source!(SYMBOL_SDF_SHADER);
shader_source!(SYMBOL_TEXT_AND_ICON_SHADER);

/// The source of each built-in program.
pub static BUILT_INS: &[(&Program, &ShaderSource)] = &[
    (&programs::BACKGROUND, &BACKGROUND_SHADER),
    (&programs::BACKGROUND_PATTERN, &BACKGROUND_PATTERN_SHADER),
    (&programs::CIRCLE, &CIRCLE_SHADER),
    (&programs::CLIPPING_MASK, &CLIPPING_MASK_PROGRAM),
    (&programs::COLLISION_BOX, &COLLISION_BOX_SHADER),
    (&programs::COLLISION_CIRCLE, &COLLISION_CIRCLE_SHADER),
    (&programs::COLOR_RELIEF, &COLOR_RELIEF_SHADER),
    (&programs::FILL, &FILL_SHADER),
    (&programs::FILL_OUTLINE, &FILL_OUTLINE_SHADER),
    (&programs::FILL_PATTERN, &FILL_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_PATTERN, &FILL_OUTLINE_PATTERN_SHADER),
    (&programs::FILL_OUTLINE_INSTANCED, &FILL_OUTLINE_INSTANCED_SHADER),
    (&programs::FILL_OUTLINE_PATTERN_INSTANCED, &FILL_OUTLINE_PATTERN_INSTANCED_SHADER),
    (&programs::FILL_OUTLINE_PULLED, &FILL_OUTLINE_PULLED_SHADER),
    (&programs::FILL_EXTRUSION, &FILL_EXTRUSION_SHADER),
    (&programs::FILL_EXTRUSION_INSTANCED, &FILL_EXTRUSION_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_WALL, &FILL_EXTRUSION_WALL_SHADER),
    (&programs::FILL_EXTRUSION_PULLED, &FILL_EXTRUSION_PULLED_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN, &FILL_EXTRUSION_PATTERN_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_INSTANCED, &FILL_EXTRUSION_PATTERN_INSTANCED_SHADER),
    (&programs::FILL_EXTRUSION_PATTERN_WALL, &FILL_EXTRUSION_PATTERN_WALL_SHADER),
    (&programs::HEATMAP, &HEATMAP_SHADER),
    (&programs::HEATMAP_TEXTURE, &HEATMAP_TEXTURE_SHADER),
    (&programs::HILLSHADE, &HILLSHADE_SHADER),
    (&programs::HILLSHADE_PREPARE, &HILLSHADE_PREPARE_SHADER),
    (&programs::LINE, &LINE_SHADER),
    (&programs::LINE_GRADIENT, &LINE_GRADIENT_SHADER),
    (&programs::LINE_PATTERN, &LINE_PATTERN_SHADER),
    (&programs::LINE_SDF, &LINE_SDF_SHADER),
    (&programs::LINE_GRADIENT_SDF, &LINE_GRADIENT_SDF_SHADER),
    (&programs::RASTER, &RASTER_SHADER),
    (&programs::SYMBOL_ICON, &SYMBOL_ICON_SHADER),
    (&programs::SYMBOL_SDF, &SYMBOL_SDF_SHADER),
    (&programs::SYMBOL_TEXT_AND_ICON, &SYMBOL_TEXT_AND_ICON_SHADER),
    (&programs::SKY, &SKY_SHADER),
    (&programs::ATMOSPHERE, &ATMOSPHERE_SHADER),
    (&programs::GLOBE_DEPTH, &GLOBE_DEPTH_SHADER),
    (&programs::LAYER_COMPOSITE, &LAYER_COMPOSITE_SHADER),
    (&programs::TERRAIN, &TERRAIN_SHADER),
    (&programs::TERRAIN_DEPTH, &TERRAIN_DEPTH_SHADER),
    #[cfg(any(test, feature = "gpu-conformance"))]
    (&programs::VERTEX_TEXEL, &VERTEX_TEXEL_SHADER),
    #[cfg(any(test, feature = "gpu-conformance"))]
    (&programs::TERRAIN_PROBE, &TERRAIN_PROBE_SHADER),
];
