// The precision that GL ES reads each block member, attribute and sampler
// at: MapLibre's, which keeps paint values that don't need more in `lowp`
// or `mediump` registers on phones' GPUs. Only GL ES has precision
// qualifiers, so the tables are here rather than in the program's
// declaration. What they don't name is `highp`, but a pad, which no shader
// reads, is `lowp`, and a sampler has GL ES's default, `lowp`.

use mln_gfx::shaders::block::{Block, Member, UniformBlock};
use mln_gfx::shaders::circle_layer_ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use mln_gfx::shaders::fill_layer_ubo::FillPatternTilePropsUBO;
use mln_gfx::shaders::heatmap_layer_ubo::HeatmapDrawableUBO;
use mln_gfx::shaders::line_layer_ubo::{
    LineDrawableUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO, LinePatternTilePropsUBO,
    LineSDFDrawableUBO,
};
use mln_gfx::shaders::program::{Attribute, Program, ProgramTexture};
use mln_gfx::shaders::programs;

/// A GL ES precision qualifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Precision {
    Low,
    Medium,
    High,
}

use Precision::{High, Low, Medium};

impl Precision {
    /// Its qualifier.
    pub(crate) fn qualifier(self) -> &'static str {
        match self {
            Low => "lowp",
            Medium => "mediump",
            High => "highp",
        }
    }
}

/// The members of blocks that GL ES reads at less than `highp`, but pads.
const MEMBERS: &[(&Block, Precision, &[&str])] = &[
    (
        CircleDrawableUBO::BLOCK,
        Low,
        &["color_t", "radius_t", "blur_t", "opacity_t", "stroke_color_t", "stroke_width_t", "stroke_opacity_t"],
    ),
    (CircleEvaluatedPropsUBO::BLOCK, Medium, &["radius", "stroke_width"]),
    (CircleEvaluatedPropsUBO::BLOCK, Low, &["blur", "opacity", "stroke_opacity"]),
    (FillPatternTilePropsUBO::BLOCK, Low, &["sdf"]),
    (HeatmapDrawableUBO::BLOCK, Low, &["weight_t", "radius_t"]),
    (LineDrawableUBO::BLOCK, Medium, &["ratio"]),
    (LineDrawableUBO::BLOCK, Low, &["color_t", "blur_t", "opacity_t", "gapwidth_t", "offset_t", "width_t"]),
    (LineEvaluatedPropsUBO::BLOCK, Medium, &["gapwidth", "width"]),
    (LineEvaluatedPropsUBO::BLOCK, Low, &["blur", "opacity", "offset", "floorwidth"]),
    (LineGradientDrawableUBO::BLOCK, Medium, &["ratio"]),
    (LineGradientDrawableUBO::BLOCK, Low, &["blur_t", "opacity_t", "gapwidth_t", "offset_t", "width_t"]),
    (LinePatternDrawableUBO::BLOCK, Medium, &["ratio"]),
    (
        LinePatternDrawableUBO::BLOCK,
        Low,
        &["blur_t", "opacity_t", "gapwidth_t", "offset_t", "width_t", "pattern_from_t", "pattern_to_t", "floorwidth_t"],
    ),
    (LinePatternTilePropsUBO::BLOCK, Medium, &["scale"]),
    (LineSDFDrawableUBO::BLOCK, Medium, &["ratio"]),
    (
        LineSDFDrawableUBO::BLOCK,
        Low,
        &["color_t", "blur_t", "opacity_t", "gapwidth_t", "offset_t", "width_t", "floorwidth_t"],
    ),
];

/// The attributes that GL ES reads at less than `highp`, by program.
const ATTRIBUTES: &[(&Program, Precision, &[&str])] = &[
    (&programs::CIRCLE, Medium, &["radius", "stroke_width"]),
    (&programs::CIRCLE, Low, &["blur", "opacity", "stroke_opacity"]),
    (&programs::FILL, Low, &["opacity"]),
    (&programs::FILL_OUTLINE, Low, &["opacity"]),
    (&programs::FILL_OUTLINE_INSTANCED, Low, &["opacity"]),
    (&programs::FILL_PATTERN, Low, &["opacity"]),
    (&programs::FILL_OUTLINE_PATTERN, Low, &["opacity"]),
    (&programs::FILL_OUTLINE_PATTERN_INSTANCED, Low, &["opacity"]),
    (&programs::HEATMAP, Medium, &["radius"]),
    (&programs::LINE, Medium, &["gapwidth", "width"]),
    (&programs::LINE, Low, &["blur", "offset", "opacity"]),
    (&programs::LINE_GRADIENT, Medium, &["gapwidth", "width"]),
    (&programs::LINE_GRADIENT, Low, &["blur", "offset", "opacity"]),
    (&programs::LINE_PATTERN, Medium, &["gapwidth", "width"]),
    (&programs::LINE_PATTERN, Low, &["blur", "offset", "opacity"]),
    (&programs::LINE_SDF, Medium, &["gapwidth", "width"]),
    (&programs::LINE_SDF, Low, &["blur", "floorwidth", "offset", "opacity"]),
    (&programs::LINE_GRADIENT_SDF, Medium, &["gapwidth", "offset", "width"]),
    (&programs::LINE_GRADIENT_SDF, Low, &["blur", "floorwidth", "opacity"]),
    (&programs::SYMBOL_ICON, Low, &["opacity"]),
    (&programs::SYMBOL_SDF, Low, &["halo_blur", "halo_width", "opacity"]),
    (&programs::SYMBOL_TEXT_AND_ICON, Low, &["halo_blur", "halo_width", "opacity"]),
];

/// The textures that GL ES samples at `highp`, by program: the DEMs and
/// the ground's depth, whose texels encode heights and depths in their
/// bytes, and the color relief's elevations.
const HIGHP_SAMPLERS: &[(&Program, &[&str])] = &[
    (&programs::CIRCLE, &["ground_depth"]),
    (&programs::COLOR_RELIEF, &["image", "elevation_stops"]),
    (&programs::HILLSHADE_PREPARE, &["image"]),
    (&programs::SYMBOL_ICON, &["ground_depth"]),
    (&programs::SYMBOL_SDF, &["ground_depth"]),
    (&programs::SYMBOL_TEXT_AND_ICON, &["ground_depth"]),
    (&programs::TERRAIN, &["ground_dem"]),
    (&programs::TERRAIN_DEPTH, &["ground_dem"]),
    #[cfg(test)]
    (&programs::TERRAIN_PROBE, &["ground_dem"]),
    #[cfg(test)]
    (&programs::VERTEX_TEXEL, &["image"]),
];

/// Whether `member` is a pad, which fills a gap in its block.
pub(crate) fn is_pad(member: &Member) -> bool {
    member.name.starts_with("pad")
}

/// The precision of `member` of `block`.
pub(crate) fn member(block: &Block, member: &Member) -> Precision {
    if is_pad(member) {
        return Low;
    }
    MEMBERS
        .iter()
        .find(|(of, _, names)| of.name == block.name && names.contains(&member.name))
        .map_or(High, |&(_, precision, _)| precision)
}

/// The precision of `program`'s `attribute`.
pub(crate) fn attribute(program: &Program, attribute: &Attribute) -> Precision {
    ATTRIBUTES
        .iter()
        .find(|(of, _, names)| *of == program && names.contains(&attribute.name))
        .map_or(High, |&(_, precision, _)| precision)
}

/// The precision of the sampler of `program`'s `texture`, where it isn't
/// GL ES's default.
pub(crate) fn sampler(program: &Program, texture: &ProgramTexture) -> Option<Precision> {
    HIGHP_SAMPLERS.iter().any(|(of, names)| *of == program && names.contains(&texture.name)).then_some(High)
}

#[cfg(test)]
mod tests {
    use mln_gfx::shaders::program::PAINT_PARAMS;

    use super::*;
    use crate::shaders::shader_source::BUILT_INS;

    /// Every member, attribute and texture the tables name is one of a
    /// program's, so that none of them goes stale.
    #[test]
    fn the_tables_name_what_the_programs_have() {
        let blocks: Vec<&Block> = BUILT_INS
            .iter()
            .flat_map(|(program, _)| std::iter::once(&PAINT_PARAMS).chain(program.buffers))
            .filter_map(mln_gfx::shaders::program::ProgramBuffer::block)
            .collect();
        for &(block, _, names) in MEMBERS {
            assert!(blocks.contains(&block), "{} is no program's", block.name);
            for name in names {
                assert!(block.members.iter().any(|member| member.name == *name), "{}::{name}", block.name);
            }
        }
        let has = |program: &Program| BUILT_INS.iter().any(|(of, _)| *of == program);
        for &(program, _, names) in ATTRIBUTES {
            assert!(has(program), "{}", program.name);
            for name in names {
                assert!(
                    program.locations().any(|(_, _, attribute)| attribute.name == *name),
                    "{}: {name}",
                    program.name
                );
            }
        }
        for &(program, names) in HIGHP_SAMPLERS {
            assert!(has(program), "{}", program.name);
            for name in names {
                assert!(program.textures.iter().any(|texture| texture.name == *name), "{}: {name}", program.name);
            }
        }
    }
}
