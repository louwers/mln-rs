//! A program: what its shaders bind and read, the same on every backend,
//! and its sources in each shading language.

use std::hash::{Hash, Hasher};
use std::{fmt, ptr};

use crate::gfx_types::AttributeDataType;
use crate::gpu::{GpuError, ShaderKey, VertexStep};
use crate::projection_variant::ProjectionVariant;
use crate::shaders::bindings::{
    FIRST_PROGRAM_BUFFER_ID, FIRST_UNIFORM_ID, FIRST_VERTEX_BUFFER_ID, MAX_ATTRIBUTES, MAX_TEXTURES,
    MAX_VERTEX_BUFFERS, PROJECTION_BUFFER_ID, VIEW_BUFFER_ID,
};
use crate::shaders::block::{Block, UniformBlock};
use crate::shaders::layer_ubo::{ProjectionUBO, ViewUBO};
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
///   [`textures`](Program::textures), which a draw names it by. In WGSL,
///   where textures and samplers bind in group 1, the slots are the
///   textures that the program's WGSL declares, in any variant, in binding
///   order, and a sampler is the one of the texture at the binding after it.
///
/// A program has at most [`FIRST_VERTEX_BUFFER_ID`] buffer ids before its
/// vertex buffers', [`MAX_ATTRIBUTES`] attributes and
/// [`MAX_TEXTURES`] textures.
///
/// [`FIRST_VERTEX_BUFFER_ID`]: crate::shaders::bindings::FIRST_VERTEX_BUFFER_ID
/// [`MAX_ATTRIBUTES`]: crate::shaders::bindings::MAX_ATTRIBUTES
/// [`MAX_TEXTURES`]: crate::shaders::bindings::MAX_TEXTURES
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
    /// The paint properties it reads, each from its uniform or per vertex
    /// as a variant says ([`source`](crate::shaders::source#properties)):
    /// those whose `HAS_UNIFORM_u_<name>` its sources test once written
    /// out, and no others, since a variant defines only these.
    pub properties: &'static [Property],
    /// Its source in each shading language it has one in.
    pub sources: Sources,
}

/// A uniform or storage buffer a program reads.
#[derive(Clone, Copy, Debug)]
pub struct Buffer {
    /// The id a draw binds it by, which is also its binding.
    pub id: usize,
    pub kind: BufferKind,
}

/// How a program reads a [`Buffer`].
#[derive(Clone, Copy, Debug)]
pub enum BufferKind {
    /// One block.
    Uniform(&'static Block),
    /// An array of elements `stride` bytes apart, each read as `block`
    /// padded to the stride, which a shader indexes: by its draw's
    /// [`global_index`](crate::gpu::Draw::global_index), as for an element
    /// per draw of a layer, or by an index of its own, as for a value per
    /// feature. GLSL ES has no storage buffers: a draw binds its element as
    /// a uniform block, so a shader reads only that one where
    /// [`Caps::indexes_storage_buffers`](crate::gpu::Caps::indexes_storage_buffers)
    /// is false.
    Storage { block: &'static Block, stride: usize },
    /// A vertex buffer that the vertex stage reads as a storage buffer, of
    /// vertices `stride` bytes apart, where the backend binds them
    /// ([`Caps::binds_vertex_buffers`](crate::gpu::Caps::binds_vertex_buffers)).
    Vertices { stride: usize },
}

impl Buffer {
    /// The uniform buffer `id` of a `T`.
    #[must_use]
    pub const fn uniform<T: UniformBlock>(id: usize) -> Buffer {
        Buffer { id, kind: BufferKind::Uniform(T::BLOCK) }
    }

    /// The storage buffer `id` of `T`s.
    #[must_use]
    pub const fn storage<T: UniformBlock>(id: usize) -> Buffer {
        Buffer::storage_of::<T, T>(id)
    }

    /// The storage buffer `id` of `Element`s, each of which the shader reads
    /// as a `T`, as of a union of the blocks of a layer's programs.
    #[must_use]
    pub const fn storage_of<T: UniformBlock, Element>(id: usize) -> Buffer {
        assert!(size_of::<T>() <= size_of::<Element>(), "an element holds the block");
        assert!(size_of::<Element>().is_multiple_of(16), "an element's size is a multiple of 16");
        Buffer { id, kind: BufferKind::Storage { block: T::BLOCK, stride: size_of::<Element>() } }
    }

    /// The vertex buffer `id` of `V`s, which the vertex stage reads as a
    /// storage buffer.
    #[must_use]
    pub const fn vertices<V>(id: usize) -> Buffer {
        Buffer { id, kind: BufferKind::Vertices { stride: size_of::<V>() } }
    }

    /// The block the shader reads of it, or of each of its elements; `None`
    /// for a vertex buffer.
    #[must_use]
    pub const fn block(&self) -> Option<&'static Block> {
        match self.kind {
            BufferKind::Uniform(block) | BufferKind::Storage { block, .. } => Some(block),
            BufferKind::Vertices { .. } => None,
        }
    }

    /// The bytes the shader reads of it: its block, a storage buffer's
    /// element padded to the stride, or a vertex.
    #[must_use]
    pub const fn size(&self) -> usize {
        match self.kind {
            BufferKind::Uniform(block) => block.size,
            BufferKind::Storage { stride, .. } | BufferKind::Vertices { stride } => stride,
        }
    }
}

/// The view's block, which the engine binds for every draw.
pub const VIEW_BUFFER: Buffer = Buffer::uniform::<ViewUBO>(VIEW_BUFFER_ID);

/// The projection block, which a layer binds for the draws of a program
/// whose vertex stage projects with it.
pub const PROJECTION_BUFFER: Buffer = Buffer::storage::<ProjectionUBO>(PROJECTION_BUFFER_ID);

/// The engine's buffers that a program may read without listing them: the
/// view's block and the projection block.
pub static ENGINE_BUFFERS: [Buffer; 2] = [VIEW_BUFFER, PROJECTION_BUFFER];

/// A vertex or instance attribute of a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attribute {
    /// The id a draw names it by, which a layer type chooses for its
    /// programs.
    pub id: usize,
    /// The format a draw without data for it binds.
    pub format: AttributeDataType,
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
/// attribute, which a source reads through the directives that
/// [`source`](crate::shaders::source#properties) describes.
///
/// A vertex stage mixes the values of a [`Float`](PropertyType::Float) or a
/// [`Color`](PropertyType::Color) at two zooms by the member `<name>_t` of
/// one of the program's blocks, which a backend fails to compile a program
/// without. The stage reads it as `drawable.<name>_t`, from a block it names
/// `drawable`, in WGSL, MSL and GLSL, and as `u_<name>_t` in GLSL ES; an MSL
/// vertex stage reads its per-vertex values from its stage input,
/// `vertex_in`.
#[derive(Clone, Copy, Debug)]
pub struct Property {
    /// The name of the uniform, `u_<name>`.
    pub name: &'static str,
    /// The id of the attribute a layer binds the property's values to.
    pub attribute: usize,
    /// What its values per vertex are.
    pub ty: PropertyType,
}

/// What a [`Property`]'s values per vertex are, of the format of its
/// attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyType {
    /// A float at two zooms, `Float2`, which the vertex stage mixes by the
    /// member `<name>_t` of its draw's block.
    Float,
    /// A color at two zooms, `Float4`, each packed in two floats, which the
    /// vertex stage mixes by `<name>_t` too.
    Color,
    /// Four values that the vertex stage doesn't mix, as a pattern's
    /// rectangle in its atlas, `UShort4`.
    Vec4,
}

impl Property {
    /// The float `u_<name>`, whose values a layer binds to attribute
    /// `attribute`.
    #[must_use]
    pub const fn float(name: &'static str, attribute: usize) -> Property {
        Property { name, attribute, ty: PropertyType::Float }
    }

    /// The color `u_<name>`, whose values a layer binds to attribute
    /// `attribute`.
    #[must_use]
    pub const fn color(name: &'static str, attribute: usize) -> Property {
        Property { name, attribute, ty: PropertyType::Color }
    }

    /// The unmixed `vec4` `u_<name>`, whose values a layer binds to
    /// attribute `attribute`.
    #[must_use]
    pub const fn vec4(name: &'static str, attribute: usize) -> Property {
        Property { name, attribute, ty: PropertyType::Vec4 }
    }
}

/// A program's source in each shading language: [`wgsl`] for
/// `mln_webgpu`, [`msl`] for `mln_mtl`, [`glsl_es`] for `mln_gl`, and
/// [`glsl`] for `mln_vulkan` and `mln_d3d12`. A backend fails to compile a
/// program without a source in its language.
///
/// A program names its sources with the methods of each language, which
/// keep a source only where the build has a backend of its language, so a
/// build carries no other backend's shaders:
/// `Sources::NONE.wgsl(&MY_WGSL).msl(&MY_MSL)`.
#[derive(Clone, Copy, Debug)]
pub struct Sources {
    pub wgsl: Option<&'static wgsl::Wgsl>,
    pub msl: Option<&'static msl::Msl>,
    pub glsl_es: Option<&'static glsl_es::Glsl>,
    pub glsl: Option<&'static glsl::Glsl>,
}

impl Sources {
    /// No source in any language.
    pub const NONE: Sources = Sources { wgsl: None, msl: None, glsl_es: None, glsl: None };

    /// These sources and `source`, where the build has `mln_webgpu`.
    #[must_use]
    pub const fn wgsl(self, source: &'static wgsl::Wgsl) -> Sources {
        Sources { wgsl: if cfg!(feature = "wgsl") { Some(source) } else { None }, ..self }
    }

    /// These sources and `source`, where the build has `mln_mtl` on an
    /// Apple platform.
    #[must_use]
    pub const fn msl(self, source: &'static msl::Msl) -> Sources {
        Sources { msl: if cfg!(all(feature = "msl", target_vendor = "apple")) { Some(source) } else { None }, ..self }
    }

    /// These sources and `source`, where the build has `mln_gl`.
    #[must_use]
    pub const fn glsl_es(self, source: &'static glsl_es::Glsl) -> Sources {
        Sources { glsl_es: if cfg!(feature = "glsl-es") { Some(source) } else { None }, ..self }
    }

    /// These sources and `source`, where the build has `mln_glsl` outside
    /// Apple platforms and the browser.
    #[must_use]
    pub const fn glsl(self, source: &'static glsl::Glsl) -> Sources {
        let has = cfg!(all(feature = "glsl", not(any(target_vendor = "apple", target_family = "wasm"))));
        Sources { glsl: if has { Some(source) } else { None }, ..self }
    }
}

/// The define that compiles a program's globe variant.
pub const GLOBE_DEFINE: &str = "PROJECTION_GLOBE";

/// What the engine compiles every program with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgramParameters {
    /// The device pixels per CSS pixel, which shaders read as
    /// `DEVICE_PIXEL_RATIO`.
    pub pixel_ratio: f32,
    /// Whether draws show how often each pixel is drawn, which shaders
    /// test as `OVERDRAW_INSPECTOR`.
    pub overdraw: bool,
}

impl ProgramParameters {
    /// The defines of the parameters, as [`ShaderKey::new`] takes them:
    /// `DEVICE_PIXEL_RATIO=<ratio>`, the ratio with a `.0` if it is whole,
    /// and `OVERDRAW_INSPECTOR` if `overdraw`.
    pub fn defines(&self) -> impl Iterator<Item = String> + use<> {
        let ratio = f64::from(self.pixel_ratio);
        let ratio =
            if ratio.is_finite() && ratio == ratio.trunc() { format!("{ratio:.1}") } else { format!("{ratio}") };
        let overdraw = self.overdraw.then(|| "OVERDRAW_INSPECTOR".to_owned());
        std::iter::once(format!("DEVICE_PIXEL_RATIO={ratio}")).chain(overdraw)
    }
}

impl Program {
    /// The key of the variant compiled with `parameters` that reads
    /// `uniforms`, properties of the program, from their uniforms, for
    /// `variant`'s projection. It has the parameters'
    /// [`defines`](ProgramParameters::defines), a `HAS_UNIFORM_u_<name>` define for
    /// each of `uniforms`, and the globe's define for its variant.
    #[must_use]
    pub fn variant_key<'a>(
        &'static self,
        parameters: &ProgramParameters,
        uniforms: impl IntoIterator<Item = &'a Property>,
        variant: ProjectionVariant,
    ) -> ShaderKey {
        let parameters = parameters.defines();
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

    /// Checks the rules this declaration's doc states, which a backend checks
    /// before it compiles the program: distinct attribute ids within each
    /// step, distinct buffer ids in the ranges of their kinds and none of the
    /// engine's, and no more attributes and textures than a program can
    /// have. Also that the GLSL ES names of the members of the view's block,
    /// the projection block and the program's blocks are distinct, since
    /// GLSL ES has them in one scope.
    ///
    /// # Errors
    ///
    /// [`GpuError::Invalid`], naming the rule the program breaks.
    pub fn check(&self) -> Result<(), GpuError> {
        let invalid = |problem: String| Err(GpuError::Invalid(format!("program {}: {problem}", self.name)));
        for attributes in [self.attributes, self.instance_attributes] {
            for (index, attribute) in attributes.iter().enumerate() {
                if attributes[..index].iter().any(|earlier| earlier.id == attribute.id) {
                    return invalid(format!("two attributes have the id {}", attribute.id));
                }
            }
        }
        let attributes = self.attributes.len() + self.instance_attributes.len();
        if attributes > MAX_ATTRIBUTES {
            return invalid(format!("{attributes} attributes, more than {MAX_ATTRIBUTES}"));
        }
        if self.textures.len() > MAX_TEXTURES {
            return invalid(format!("{} textures, more than {MAX_TEXTURES}", self.textures.len()));
        }
        for (index, buffer) in self.buffers.iter().enumerate() {
            let ids = match buffer.kind {
                BufferKind::Uniform(_) => FIRST_PROGRAM_BUFFER_ID..FIRST_VERTEX_BUFFER_ID,
                BufferKind::Storage { .. } => FIRST_PROGRAM_BUFFER_ID..FIRST_UNIFORM_ID + 1,
                BufferKind::Vertices { .. } => FIRST_VERTEX_BUFFER_ID..FIRST_VERTEX_BUFFER_ID + MAX_VERTEX_BUFFERS,
            };
            if !ids.contains(&buffer.id) || buffer.id == PROJECTION_BUFFER_ID {
                return invalid(format!(
                    "buffer id {}, outside the ids of its kind ({ids:?}, but {PROJECTION_BUFFER_ID})",
                    buffer.id
                ));
            }
            if self.buffers[..index].iter().any(|earlier| earlier.id == buffer.id) {
                return invalid(format!("two buffers have the id {}", buffer.id));
            }
        }
        let blocks = || ENGINE_BUFFERS.iter().chain(self.buffers).filter_map(Buffer::block);
        let mut names: Vec<String> =
            blocks().flat_map(|block| block.members.iter().map(|member| glsl_es::member_name(block, member))).collect();
        names.sort_unstable();
        let repeated = names.windows(2).find_map(|pair| match pair {
            [name, next] if name == next => Some(name),
            _ => None,
        });
        if let Some(name) = repeated {
            return invalid(format!("two block members are {name} in GLSL ES"));
        }
        Ok(())
    }

    /// The buffer whose block is `name`: one of the program's, or the
    /// view's or the projection block.
    #[must_use]
    pub fn buffer_named(&self, name: &str) -> Option<&'static Buffer> {
        self.buffers.iter().chain(&ENGINE_BUFFERS).find(|buffer| buffer.block().is_some_and(|block| block.name == name))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx_types::AttributeDataType::Short2;

    static EMPTY: Program = Program {
        name: "empty",
        buffers: &[],
        attributes: &[],
        instance_attributes: &[],
        textures: &[],
        properties: &[],
        sources: Sources::NONE,
    };

    #[test]
    fn a_program_breaking_a_rule_of_its_declaration_fails_its_check() {
        const POSITION: Attribute = Attribute { id: 0, format: Short2 };
        assert!(EMPTY.check().is_ok());
        let fails = |program: Program| assert!(program.check().is_err(), "{:?}", program.buffers);
        fails(Program { attributes: &[POSITION, POSITION], ..EMPTY });
        fails(Program { buffers: const { &[Buffer::uniform::<ViewUBO>(5), Buffer::uniform::<ViewUBO>(5)] }, ..EMPTY });
        fails(Program { buffers: const { &[Buffer::storage::<ViewUBO>(PROJECTION_BUFFER_ID)] }, ..EMPTY });
        fails(Program { buffers: const { &[Buffer::storage::<ViewUBO>(6)] }, ..EMPTY });
        fails(Program { buffers: const { &[Buffer::vertices::<[f32; 4]>(2)] }, ..EMPTY });
        // The view's members again, in one GLSL ES scope with the view's.
        fails(Program { buffers: const { &[Buffer::uniform::<ViewUBO>(5)] }, ..EMPTY });
    }

    #[test]
    fn parameters_define_the_pixel_ratio_and_the_overdraw_inspector() {
        let defines = |pixel_ratio, overdraw| ProgramParameters { pixel_ratio, overdraw }.defines().collect::<Vec<_>>();
        assert_eq!(defines(1.0, false), ["DEVICE_PIXEL_RATIO=1.0"]);
        assert_eq!(defines(1.5, true), ["DEVICE_PIXEL_RATIO=1.5", "OVERDRAW_INSPECTOR"]);
    }
}
