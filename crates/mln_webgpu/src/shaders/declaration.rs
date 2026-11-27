//! A variant's WGSL declarations, written from its program: the structs of
//! the blocks it reads, its vertex input, and the variables of its buffers,
//! textures and samplers. Its bind group layouts follow from the same
//! bindings.

use std::fmt::Write as _;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{GpuError, ShaderKey};
use mln_gfx::shaders::bindings::GLOBAL_INDEX_ID;
use mln_gfx::shaders::block::{Block, MemberType, UniformBlock};
use mln_gfx::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use mln_gfx::shaders::program::{
    Attribute, BufferKind, PAINT_PARAMS, Program, ProgramBuffer, ProgramTexture, Stages, TERRAIN_DEFINE, Variants,
    VertexWord,
};

use crate::shaders::shader_program::{BindingInfo, BindingType};

/// The group of the buffers, each at the binding of its id.
const BUFFER_GROUP: u32 = 0;
/// The group of the textures: of the texture at slot `s`, its sampler at
/// binding `2s` and the texture at `2s + 1`.
const TEXTURE_GROUP: u32 = 1;

/// The slot of the texture whose texture, or whose sampler, binds at
/// `binding` of the texture group.
pub(crate) fn texture_slot(binding: u32) -> usize {
    binding as usize / 2
}

/// The struct of the draw's index, `globalIndex`, a `u32` padded to the
/// index table's entries (`index_table.rs`).
const GLOBAL_INDEX_STRUCT: &str = "struct GlobalIndexUBO {\n    value: u32,\n    pad0: vec3<u32>,\n};\n";

/// What a variant of a program declares: the WGSL of its stages'
/// declarations, and the bindings its layouts have.
pub(crate) struct Declaration {
    /// The structs and the variables, which both stages declare.
    pub(crate) declarations: String,
    /// The vertex stage's input, `VertexInput`; empty for a program
    /// without attributes.
    pub(crate) vertex_input: String,
    /// The bindings, by group and binding.
    pub(crate) bindings: Vec<BindingInfo>,
    /// The group of `globalIndex`, the index of the draw's elements in the
    /// storage buffers it reads, if it reads any: the group after the
    /// others, which holds only the index.
    pub(crate) index_group: Option<u32>,
}

impl Declaration {
    /// The declaration of the variant of `key`.
    ///
    /// A variant reads each of the program's buffers, textures and
    /// attributes that its [`Variants`] include, and of an attribute of a
    /// property, only where it reads the property per vertex, without its
    /// `HAS_UNIFORM_u_<name>`. It reads the paint parameters where the
    /// program does, its projection as the program's
    /// [`projection`](Program::projection) says, and `globalIndex` where it
    /// reads a storage buffer. Fails where a block's struct would differ
    /// between two of its buffers.
    pub(crate) fn new(key: &ShaderKey) -> Result<Declaration, GpuError> {
        let program = key.program();
        let variant = Variant { program, key };
        let paint_params = program.paint_params.map(|stages| ProgramBuffer { stages, ..PAINT_PARAMS });
        let buffers: Vec<ProgramBuffer> = paint_params
            .into_iter()
            .chain(program.projection_buffer().copied())
            .chain(program.buffers.iter().copied())
            .filter(|buffer| variant.includes(buffer.variants))
            .collect();
        let textures: Vec<(usize, &ProgramTexture)> = program
            .textures
            .iter()
            .enumerate()
            .filter(|(_, texture)| !texture.is_unused() && variant.includes(texture.variants))
            .collect();

        let mut structs = Structs::default();
        structs.add(GlobalPaintParamsUBO::BLOCK, None)?;
        // The prelude's functions take a projection.
        structs.add(ProjectionUBO::BLOCK, None)?;
        let mut declarations = String::new();
        let mut bindings = Vec::new();
        for buffer in &buffers {
            let (type_, space, ty, size) = match buffer.kind {
                BufferKind::Uniform(block) | BufferKind::DrawUniform(block) => {
                    structs.add(block, None)?;
                    (BindingType::UniformBuffer, "uniform", block.name.to_owned(), block.size)
                }
                BufferKind::Storage { block, stride } => {
                    structs.add(block, Some(stride))?;
                    (BindingType::ReadOnlyStorageBuffer, "storage, read", format!("array<{}>", block.name), stride)
                }
                BufferKind::Vertices(word) => {
                    let word = match word {
                        VertexWord::Uint => "u32",
                        VertexWord::Float => "f32",
                    };
                    (BindingType::ReadOnlyStorageBuffer, "storage, read", format!("array<{word}>"), 4)
                }
            };
            let binding = buffer.id as u32;
            _ = writeln!(
                declarations,
                "@group({BUFFER_GROUP}) @binding({binding}) var<{space}> {}: {ty};",
                buffer.name
            );
            bindings.push(BindingInfo {
                group: BUFFER_GROUP,
                binding,
                type_,
                visibility: visibility(buffer.stages),
                min_size: size as u64,
            });
        }
        for &(slot, texture) in &textures {
            let visibility = visibility(texture.stages);
            let binding = 2 * slot as u32;
            if texture.sampled() {
                _ = writeln!(
                    declarations,
                    "@group({TEXTURE_GROUP}) @binding({binding}) var {}_sampler: sampler;",
                    texture.name
                );
                let type_ = if texture.filterable() { BindingType::Sampler } else { BindingType::NonFilteringSampler };
                bindings.push(BindingInfo { group: TEXTURE_GROUP, binding, type_, visibility, min_size: 0 });
            }
            let binding = binding + 1;
            _ = writeln!(
                declarations,
                "@group({TEXTURE_GROUP}) @binding({binding}) var {}: texture_2d<f32>;",
                texture.name
            );
            let type_ = if texture.filterable() { BindingType::Texture } else { BindingType::UnfilterableTexture };
            bindings.push(BindingInfo { group: TEXTURE_GROUP, binding, type_, visibility, min_size: 0 });
        }
        let index_group = buffers.iter().any(|buffer| matches!(buffer.kind, BufferKind::Storage { .. })).then(|| {
            let group = if textures.is_empty() { TEXTURE_GROUP } else { TEXTURE_GROUP + 1 };
            _ = writeln!(
                declarations,
                "@group({group}) @binding({GLOBAL_INDEX_ID}) var<uniform> globalIndex: GlobalIndexUBO;"
            );
            group
        });

        let mut vertex_input = String::new();
        let mut inputs = program.locations().filter(|(_, _, attribute)| variant.reads(attribute)).peekable();
        if inputs.peek().is_some() {
            vertex_input.push_str("struct VertexInput {\n");
            for (location, _, attribute) in inputs {
                let ty = input_type(attribute, variant.is_property(attribute));
                _ = writeln!(vertex_input, "    @location({location}) {}: {ty},", attribute.name);
            }
            vertex_input.push_str("};\n");
        }

        let mut text = structs.text;
        if index_group.is_some() {
            text.push_str(GLOBAL_INDEX_STRUCT);
        }
        text.push_str(&declarations);
        Ok(Declaration { declarations: text, vertex_input, bindings, index_group })
    }
}

/// A variant of a program, by its key's defines.
struct Variant<'a> {
    program: &'static Program,
    key: &'a ShaderKey,
}

impl Variant<'_> {
    fn defines(&self, name: &str) -> bool {
        self.key.define_values().any(|(define, _)| define == name)
    }

    /// Whether the variant reads the property `name` from its uniform.
    fn reads_uniform(&self, name: &str) -> bool {
        self.key.define_values().any(|(define, _)| define.strip_prefix("HAS_UNIFORM_u_") == Some(name))
    }

    fn includes(&self, variants: Variants) -> bool {
        match variants {
            Variants::All => true,
            Variants::Standing => self.defines(TERRAIN_DEFINE),
            Variants::PerVertex => self.program.properties.iter().any(|property| !self.reads_uniform(property.name)),
        }
    }

    fn is_property(&self, attribute: &Attribute) -> bool {
        self.program.properties.iter().any(|property| property.name == attribute.name)
    }

    /// Whether the variant reads `attribute`.
    fn reads(&self, attribute: &Attribute) -> bool {
        self.includes(attribute.variants) && !(self.is_property(attribute) && self.reads_uniform(attribute.name))
    }
}

/// The stages a binding is visible to.
fn visibility(stages: Stages) -> wgpu::ShaderStages {
    match stages {
        Stages::Vertex => wgpu::ShaderStages::VERTEX,
        Stages::Fragment => wgpu::ShaderStages::FRAGMENT,
        Stages::Both => wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
    }
}

/// The WGSL type that the vertex stage reads `attribute` as: of its
/// format, but a property's float values as a pair, to mix by zoom.
fn input_type(attribute: &Attribute, property: bool) -> String {
    use AttributeDataType as A;
    let (scalar, count) = match attribute.format {
        A::Byte | A::Short | A::Int => ("i32", 1),
        A::Byte2 | A::Short2 | A::Int2 => ("i32", 2),
        A::Byte3 | A::Short3 | A::Int3 => ("i32", 3),
        A::Byte4 | A::Short4 | A::Int4 => ("i32", 4),
        A::UByte | A::UShort | A::UInt => ("u32", 1),
        A::UByte2 | A::UShort2 | A::UInt2 => ("u32", 2),
        A::UByte3 | A::UShort3 | A::UInt3 => ("u32", 3),
        A::UByte4 | A::UShort4 | A::UInt4 | A::UShort8 => ("u32", 4),
        A::Float if property => ("f32", 2),
        A::Float | A::Invalid => ("f32", 1),
        A::Float2 => ("f32", 2),
        A::Float3 => ("f32", 3),
        A::Float4 | A::UByte4Normalized => ("f32", 4),
    };
    if count == 1 { scalar.to_owned() } else { format!("vec{count}<{scalar}>") }
}

/// The structs of the blocks a variant reads, each once.
#[derive(Default)]
struct Structs {
    /// Each block's name and size in WGSL.
    written: Vec<(&'static str, usize)>,
    text: String,
}

impl Structs {
    /// Adds the struct of `block`, the element of an array `stride` bytes
    /// apart if `stride` is given: one larger than the block ends in
    /// padding, its last member's `@size`. Fails if the struct is written
    /// already with another size.
    fn add(&mut self, block: &'static Block, stride: Option<usize>) -> Result<(), GpuError> {
        let size = stride.unwrap_or(block.size);
        if let Some(&(_, written)) = self.written.iter().find(|(name, _)| *name == block.name) {
            return if written == size {
                Ok(())
            } else {
                Err(GpuError::Invalid(format!("{} is read with strides of {written} and {size}", block.name)))
            };
        }
        self.written.push((block.name, size));
        _ = writeln!(self.text, "struct {} {{", block.name);
        for (index, member) in block.members.iter().enumerate() {
            let ty = match member.ty {
                MemberType::Float => "f32".to_owned(),
                MemberType::Int => "i32".to_owned(),
                MemberType::Uint => "u32".to_owned(),
                MemberType::Vec2 => "vec2<f32>".to_owned(),
                MemberType::Vec3 => "vec3<f32>".to_owned(),
                MemberType::Vec4 => "vec4<f32>".to_owned(),
                MemberType::Mat4 => "mat4x4<f32>".to_owned(),
                MemberType::Vec4Array(n) => format!("array<vec4<f32>, {n}>"),
            };
            let padding = size - block.size;
            if index + 1 == block.members.len() && padding > 0 {
                let member_size = block.size - member.offset;
                _ = writeln!(self.text, "    @size({}) {}: {ty},", member_size + padding, member.name);
            } else {
                _ = writeln!(self.text, "    {}: {ty},", member.name);
            }
        }
        self.text.push_str("};\n");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use mln_builtins::layers::circle::programs::CIRCLE;
    use mln_builtins::layers::fill::ubo::FillDrawableUBO;
    use mln_builtins::layers::fill_extrusion::programs::FILL_EXTRUSION_PULLED;
    use mln_engine::shaders::programs::CLIPPING_MASK;
    use mln_gfx::projection_variant::ProjectionVariant;
    use mln_gfx::shaders::program::Property;
    use mln_gfx::shaders::program_parameters::ProgramParameters;

    use super::*;

    fn declaration(program: &'static Program, uniforms: &[Property], variant: ProjectionVariant) -> Declaration {
        let key = program.variant_key(&ProgramParameters::new(1.0), uniforms, variant);
        Declaration::new(&key).unwrap()
    }

    /// The lines of `text` that declare a variable or a vertex input.
    fn variables(text: &str) -> Vec<&str> {
        text.lines().filter(|line| line.trim_start().starts_with('@')).collect()
    }

    #[test]
    fn a_union_stride_pads_the_block_to_it() {
        let mut structs = Structs::default();
        structs.add(FillDrawableUBO::BLOCK, Some(112)).unwrap();
        assert!(structs.text.ends_with("    pad1: f32,\n    @size(36) pad2: f32,\n};\n"), "{}", structs.text);
        assert!(structs.add(FillDrawableUBO::BLOCK, Some(112)).is_ok());
        assert!(structs.add(FillDrawableUBO::BLOCK, None).is_err());
    }

    #[test]
    fn a_program_declares_its_buffers_at_their_ids() {
        let declaration = declaration(&CLIPPING_MASK, &[], ProjectionVariant::Mercator);
        assert_eq!(
            variables(&declaration.declarations),
            ["@group(0) @binding(4) var<uniform> projection: ProjectionUBO;"]
        );
        assert_eq!(variables(&declaration.vertex_input), ["    @location(0) pos: vec2<i32>,"]);
        assert_eq!(declaration.index_group, None);
    }

    #[test]
    fn a_variant_declares_what_it_reads() {
        let plain = declaration(&CIRCLE, &[], ProjectionVariant::Mercator);
        assert_eq!(
            variables(&plain.declarations),
            [
                "@group(0) @binding(0) var<uniform> paintParams: GlobalPaintParamsUBO;",
                "@group(0) @binding(4) var<storage, read> projectionVector: array<ProjectionUBO>;",
                "@group(0) @binding(2) var<storage, read> drawableVector: array<CircleDrawableUBO>;",
                "@group(0) @binding(5) var<uniform> props: CircleEvaluatedPropsUBO;",
                "@group(1) @binding(1) var<uniform> globalIndex: GlobalIndexUBO;",
            ]
        );
        assert!(plain.vertex_input.contains("    @location(1) color: vec4<f32>,\n"));
        assert!(!plain.vertex_input.contains("ground_height"));

        // Over terrain, with its color from its uniform.
        let standing = declaration(&CIRCLE, &[Property::new("color")], ProjectionVariant::Terrain);
        assert!(!standing.vertex_input.contains(" color:"));
        assert!(standing.vertex_input.contains("    @location(8) ground_height: f32,\n"));
        assert!(standing.declarations.contains("@group(1) @binding(3) var ground_depth: texture_2d<f32>;\n"));
        assert!(!standing.declarations.contains("ground_depth_sampler"));
        assert_eq!(standing.index_group, Some(2));
    }

    #[test]
    fn a_pulled_program_reads_its_vertices_as_words() {
        let declaration = declaration(&FILL_EXTRUSION_PULLED, &[], ProjectionVariant::Mercator);
        let vertices = "@group(0) @binding(7) var<storage, read> wallVertices: array<u32>;\n";
        assert!(declaration.declarations.contains(vertices));
        assert!(declaration.declarations.contains("var<storage, read> wallPaint: array<f32>;\n"));
        assert!(!declaration.declarations.contains("wallHeights"));
        assert_eq!(declaration.vertex_input, "");
    }
}
