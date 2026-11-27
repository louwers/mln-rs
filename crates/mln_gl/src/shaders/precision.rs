// The precision that GL ES reads each block member, attribute and sampler
// at, as the program's GL ES declarations say (`GlslEsDeclarations`): what
// they don't name is `highp`, but a pad, which no shader reads, is `lowp`,
// and a sampler has GL ES's default, `lowp`.

use mln_gfx::shaders::block::{Block, Member};
use mln_gfx::shaders::program::{Attribute, ProgramTexture};
use mln_gfx::shaders::source::GlslEsDeclarations;
use mln_gfx::shaders::source::Precision::{self, High, Low};

/// Whether `member` is a pad, which fills a gap in its block.
pub(crate) fn is_pad(member: &Member) -> bool {
    member.name.starts_with("pad")
}

/// The precision of `member` of `block`, of a program of `declarations`.
pub(crate) fn member(declarations: &GlslEsDeclarations, block: &Block, member: &Member) -> Precision {
    if is_pad(member) {
        return Low;
    }
    declarations
        .members
        .iter()
        .find(|(of, _, names)| of.name == block.name && names.contains(&member.name))
        .map_or(High, |&(_, precision, _)| precision)
}

/// The precision of `attribute`, of a program of `declarations`.
pub(crate) fn attribute(declarations: &GlslEsDeclarations, attribute: &Attribute) -> Precision {
    declarations
        .attributes
        .iter()
        .find(|(_, names)| names.contains(&attribute.name))
        .map_or(High, |&(precision, _)| precision)
}

/// The precision of the sampler of `texture`, of a program of
/// `declarations`, where it isn't GL ES's default.
pub(crate) fn sampler(declarations: &GlslEsDeclarations, texture: &ProgramTexture) -> Option<Precision> {
    declarations.highp_samplers.contains(&texture.name).then_some(High)
}

#[cfg(test)]
mod tests {
    use mln_builtins::shaders::every_program;
    use mln_gfx::shaders::program::{PAINT_PARAMS, ProgramBuffer};

    /// Every member, attribute and texture a program's declarations name is
    /// one of the program's, so that none of them goes stale.
    #[test]
    fn the_declarations_name_what_their_programs_have() {
        for program in every_program() {
            let Some(source) = program.sources.glsl_es else {
                continue;
            };
            let declarations = source.declarations;
            let blocks: Vec<_> =
                std::iter::once(&PAINT_PARAMS).chain(program.buffers).filter_map(ProgramBuffer::block).collect();
            for &(block, _, names) in declarations.members {
                assert!(blocks.contains(&block), "{}: {} is none of its blocks", program.name, block.name);
                for name in names {
                    assert!(block.members.iter().any(|member| member.name == *name), "{}::{name}", block.name);
                }
            }
            for &(_, names) in declarations.attributes {
                for name in names {
                    assert!(
                        program.locations().any(|(_, _, attribute)| attribute.name == *name),
                        "{}: {name}",
                        program.name
                    );
                }
            }
            for name in declarations.highp_samplers {
                assert!(program.textures.iter().any(|texture| texture.name == *name), "{}: {name}", program.name);
            }
        }
    }
}
