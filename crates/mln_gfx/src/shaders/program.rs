//! A program: what its shaders bind and read, the same on every backend,
//! and its sources in each shading language.

use std::hash::{Hash, Hasher};
use std::{fmt, ptr};

use crate::gfx_types::AttributeDataType;
use crate::gpu::{ShaderKey, VertexStep};
use crate::projection_variant::ProjectionVariant;
use crate::shaders::layer_ubo::{ProjectionUBO, ViewUBO};
use crate::shaders::program_parameters::ProgramParameters;
use crate::shaders::shader_defines::{ID_PROJECTION_UBO, ID_VIEW_UBO};
use crate::shaders::source::{glsl, glsl_es, msl, wgsl};

/// A program: the buffers, vertex attributes and textures its shaders bind,
/// the paint properties a variant may read from a uniform, and its sources.
///
/// A program is the `static` that declares it: two programs are equal only
/// if they are the same `static`. A backend compiles a program from its
/// source in the backend's shading language and binds what a draw gives it
/// where this declaration says:
///
/// - A buffer at the binding of its id. The engine reserves the view's
///   block at 0 ([`VIEW_BUFFER`]), the draw's index into the storage
///   buffers it reads at 1, which the backend binds itself, and the
///   projection block at 4 ([`PROJECTION_BUFFER`]); any program may read
///   them, and lists none of them in [`buffers`](Program::buffers): the
///   backends declare them for the stages that read them. A program's own
///   buffers take the other ids: at 2 a storage buffer that
///   only its vertex stage reads, at 3 one that only its fragment stage
///   reads, either a uniform block instead where a draw binds one of its
///   own; at 5 and 6 its uniform blocks, or at 5 a storage buffer that both
///   stages read; and at 7, 8 and 9 the vertex buffers its vertex stage reads
///   as storage buffers, where the backend binds them
///   ([`Caps::binds_vertex_buffers`](crate::gpu::Caps::binds_vertex_buffers)).
///   The Vulkan and D3D12 backends share one pipeline layout that these
///   ranges lay out.
/// - A vertex attribute at the shader location of its index in
///   [`attributes`](Program::attributes), and an instance attribute at the
///   location after the vertex attributes' of its index in
///   [`instance_attributes`](Program::instance_attributes). A draw names an
///   attribute by its [`id`](Attribute::id).
/// - A texture and its sampler at the slot of its index in
///   [`textures`](Program::textures), which a draw names it by. In WGSL the
///   slots are the program's textures in binding order, and a sampler is
///   the one of the texture at the binding after it.
///
/// A program has at most [`MAX_UBO_COUNT_PER_SHADER`] buffer ids before its
/// vertex buffers', [`MAX_ATTRIBUTE_COUNT_PER_SHADER`] attributes and
/// [`MAX_TEXTURE_COUNT_PER_SHADER`] textures.
///
/// [`MAX_UBO_COUNT_PER_SHADER`]: crate::shaders::shader_defines::MAX_UBO_COUNT_PER_SHADER
/// [`MAX_ATTRIBUTE_COUNT_PER_SHADER`]: crate::shaders::shader_defines::MAX_ATTRIBUTE_COUNT_PER_SHADER
/// [`MAX_TEXTURE_COUNT_PER_SHADER`]: crate::shaders::shader_defines::MAX_TEXTURE_COUNT_PER_SHADER
pub struct Program {
    /// Its name, for logs and labels.
    pub name: &'static str,
    /// The buffers of its own that its shaders read.
    pub buffers: &'static [Buffer],
    /// The attributes that step once a vertex.
    pub attributes: &'static [Attribute],
    /// The attributes that step once an instance.
    pub instance_attributes: &'static [Attribute],
    /// The textures it samples.
    pub textures: &'static [Texture],
    /// The paint properties it reads: those whose `HAS_UNIFORM_u_<name>`
    /// its source tests, and no others, since a variant defines only these.
    pub properties: &'static [Property],
    /// Its source in each shading language it has one in.
    pub sources: Sources,
}

/// A uniform or storage buffer a program reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buffer {
    /// The id a draw binds it by, which is also its binding.
    pub id: usize,
    /// The name of its block in the GLSL ES and GLSL.
    pub name: &'static str,
    /// The bytes the shader reads: its block, a storage buffer's element as
    /// the block declares it, or the vertex of a vertex buffer it reads as a
    /// storage buffer.
    pub size: usize,
    pub kind: BufferKind,
}

/// How a program reads a [`Buffer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferKind {
    /// One block.
    Uniform,
    /// An array of elements `stride` bytes apart, one for each draw of a
    /// layer, which a draw reads at its index. GLSL ES reads its element as
    /// a uniform block bound at the element's offset.
    Storage { stride: usize },
}

impl Buffer {
    /// The uniform buffer `id`, the block `name` of a `T`.
    #[must_use]
    pub const fn uniform<T>(id: usize, name: &'static str) -> Buffer {
        Buffer { id, name, size: size_of::<T>(), kind: BufferKind::Uniform }
    }

    /// The storage buffer `id` of `T`s, the block `name` of one.
    #[must_use]
    pub const fn storage<T>(id: usize, name: &'static str) -> Buffer {
        Buffer::storage_of::<T, T>(id, name)
    }

    /// The storage buffer `id` of `Element`s, of each of which the shader
    /// reads the block `name` of a `T`, as of a union of the blocks of a
    /// layer's programs.
    #[must_use]
    pub const fn storage_of<T, Element>(id: usize, name: &'static str) -> Buffer {
        assert!(size_of::<T>() <= size_of::<Element>());
        Buffer { id, name, size: size_of::<T>(), kind: BufferKind::Storage { stride: size_of::<Element>() } }
    }
}

/// The view's block, which the engine binds for every draw.
pub const VIEW_BUFFER: Buffer = Buffer::uniform::<ViewUBO>(ID_VIEW_UBO, "ViewUBO");

/// The projection block, which a layer binds for its draws.
pub const PROJECTION_BUFFER: Buffer = Buffer::storage::<ProjectionUBO>(ID_PROJECTION_UBO, "ProjectionUBO");

/// A vertex or instance attribute of a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attribute {
    /// The id a draw names it by, which a layer type chooses for its
    /// programs.
    pub id: usize,
    /// Its name in the GLSL ES.
    pub name: &'static str,
    /// The format a draw without data for it binds.
    pub format: AttributeDataType,
}

impl Attribute {
    /// The attribute `id`, `name` in the GLSL ES, of `format`.
    #[must_use]
    pub const fn new(id: usize, name: &'static str, format: AttributeDataType) -> Attribute {
        Attribute { id, name, format }
    }
}

/// A texture a program samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Texture {
    /// The name of its sampler in the GLSL ES.
    pub name: &'static str,
    /// Whether it binds with a filtering sampler, which a texture of
    /// `Rgba32Float` can't.
    pub filterable: bool,
}

impl Texture {
    /// The filterable texture `name`.
    #[must_use]
    pub const fn new(name: &'static str) -> Texture {
        Texture { name, filterable: true }
    }

    /// The unfilterable texture `name`.
    #[must_use]
    pub const fn unfilterable(name: &'static str) -> Texture {
        Texture { name, filterable: false }
    }
}

/// A paint property a program reads: from its uniform in a variant that
/// reads it from there, otherwise from the values a layer binds to its
/// attribute.
#[derive(Clone, Copy, Debug)]
pub struct Property {
    /// The name of the uniform, `u_<name>`.
    pub name: &'static str,
    /// The id of the attribute a layer binds the property's values to.
    pub attribute: usize,
}

impl Property {
    /// The property `u_<name>`, whose values a layer binds to attribute
    /// `attribute`.
    #[must_use]
    pub const fn new(name: &'static str, attribute: usize) -> Property {
        Property { name, attribute }
    }
}

/// A program's source in each shading language, `None` in one it has no
/// source in or that the build leaves out: [`wgsl`] for `mln_webgpu`,
/// [`msl`] for `mln_mtl`, [`glsl_es`] for `mln_gl`, and [`glsl`] for
/// `mln_vulkan` and `mln_d3d12`. A backend fails to compile a program
/// without a source in its language.
#[derive(Clone, Copy, Debug)]
pub struct Sources {
    pub wgsl: Option<&'static wgsl::Wgsl>,
    pub msl: Option<&'static msl::Msl>,
    pub glsl_es: Option<&'static glsl_es::Glsl>,
    pub glsl: Option<&'static glsl::Glsl>,
}

impl Sources {
    /// No source in any language, to fill in the languages a program
    /// lacks: `Sources { wgsl: Some(&MY_WGSL), ..Sources::NONE }`.
    pub const NONE: Sources = Sources { wgsl: None, msl: None, glsl_es: None, glsl: None };
}

/// The define that compiles a program's globe variant.
pub const GLOBE_DEFINE: &str = "PROJECTION_GLOBE";

impl Program {
    /// The key of the variant compiled with `parameters` that reads
    /// `uniforms`, properties of the program, from their uniforms, for
    /// `variant`'s projection. It has the parameters' defines
    /// (`DEVICE_PIXEL_RATIO=<ratio>`), a `HAS_UNIFORM_u_<name>` define for
    /// each of `uniforms`, and the globe's define for its variant.
    #[must_use]
    pub fn variant_key<'a>(
        &'static self,
        parameters: &ProgramParameters,
        uniforms: impl IntoIterator<Item = &'a Property>,
        variant: ProjectionVariant,
    ) -> ShaderKey {
        let parameters = parameters
            .defines()
            .iter()
            .map(|(name, value)| if value.is_empty() { name.clone() } else { format!("{name}={value}") });
        let uniforms = uniforms.into_iter().map(|property| format!("HAS_UNIFORM_u_{}", property.name));
        let globe = (variant == ProjectionVariant::Globe).then(|| GLOBE_DEFINE.to_owned());
        ShaderKey::new(self, parameters.chain(uniforms).chain(globe))
    }

    /// The shader location of the attribute `id` that steps by `step`.
    #[must_use]
    pub fn location(&self, step: VertexStep, id: usize) -> Option<usize> {
        let (attributes, first) = match step {
            VertexStep::Vertex => (self.attributes, 0),
            VertexStep::Instance => (self.instance_attributes, self.attributes.len()),
        };
        attributes.iter().position(|attribute| attribute.id == id).map(|index| first + index)
    }

    /// The buffer whose block is `name`: one of the program's, or the
    /// frame's or the projection block.
    #[must_use]
    pub fn buffer_named(&self, name: &str) -> Option<&'static Buffer> {
        static RESERVED: [Buffer; 2] = [VIEW_BUFFER, PROJECTION_BUFFER];
        self.buffers.iter().chain(&RESERVED).find(|buffer| buffer.name == name)
    }
}

impl PartialEq for Program {
    fn eq(&self, other: &Program) -> bool {
        ptr::eq(self, other)
    }
}

impl Eq for Program {}

impl Hash for Program {
    fn hash<H: Hasher>(&self, state: &mut H) {
        ptr::hash(self, state);
    }
}

impl fmt::Debug for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}
