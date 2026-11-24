//! A program: what its shaders bind and read, the same on every backend,
//! and its sources in each shading language.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::{fmt, ptr};

use crate::gfx_types::AttributeDataType;
use crate::gpu::{Caps, GpuError, ShaderKey, VertexStep};
use crate::projection_variant::ProjectionVariant;
use crate::shaders::bindings::{
    FIRST_UNIFORM_ID, FIRST_VERTEX_BUFFER_ID, FRAGMENT_STORAGE_ID, MAX_ATTRIBUTES, MAX_TEXTURES, MAX_VERTEX_BUFFERS,
    PROJECTION_BUFFER_ID, VERTEX_STORAGE_ID, VIEW_BUFFER_ID,
};
use crate::shaders::block::{Block, UniformBlock};
use crate::shaders::layer_ubo::{ProjectionUBO, ViewUBO};
use crate::shaders::source::{self, Stage, glsl, glsl_es, msl, wgsl};

/// A program: the buffers, vertex attributes and textures its shaders bind,
/// the paint properties a variant may read from a uniform, and its sources.
///
/// A program is the `static` that declares it: two programs are equal only
/// if they are the same `static`. A backend compiles a program from its
/// source in the backend's shading language and binds what a draw gives it
/// where this declaration says:
///
/// - A buffer at the binding of its id, which a stage that may read it and
///   names it declares ([`bindings`](Program::bindings)), as the backend
///   writes it from the [`ProgramBuffer`]: a source declares no buffer itself.
///   The engine's buffers ([`ENGINE_BUFFERS`]) and the draw's index into
///   the storage buffers it reads ([`DRAW_INDEX`]), which the backend binds
///   itself, are any program's to read, and it lists none of them in
///   [`buffers`](Program::buffers). A program's own buffers take the ids
///   that [`ProgramBuffer`]'s constructors give them.
/// - A vertex attribute at the shader location of its index in
///   [`attributes`](Program::attributes), and an instance attribute at the
///   location after the vertex attributes' of its index in
///   [`instance_attributes`](Program::instance_attributes). A draw names an
///   attribute by its index among those of its step, and a
///   [`Property`] by its name.
/// - A texture and its sampler at the slot of its index in
///   [`textures`](Program::textures), which a draw names it by, and which
///   a fragment stage that names them declares, as the backend writes them
///   from the [`ProgramTexture`]: a source declares no texture itself. A
///   vertex stage that names a texture declares it, without its sampler,
///   only where the texture says that the vertex stage reads it
///   ([`ProgramTexture::vertex`]).
///
/// A program has at most [`MAX_ATTRIBUTES`] attributes and [`MAX_TEXTURES`]
/// textures.
///
/// [`MAX_ATTRIBUTES`]: crate::shaders::bindings::MAX_ATTRIBUTES
/// [`MAX_TEXTURES`]: crate::shaders::bindings::MAX_TEXTURES
pub struct Program {
    /// Its name, for logs and labels.
    pub name: &'static str,
    /// The buffers of its own that its shaders read.
    pub buffers: &'static [ProgramBuffer],
    /// The attributes that step once a vertex.
    pub attributes: &'static [Attribute],
    /// The attributes that step once an instance.
    pub instance_attributes: &'static [Attribute],
    /// The textures it samples.
    pub textures: &'static [ProgramTexture],
    /// The paint properties it reads, each from its uniform or per vertex
    /// as a variant says ([`source`](crate::shaders::source#properties)):
    /// those whose `HAS_UNIFORM_<name>` its sources test once written
    /// out, and no others, since a variant defines only these.
    pub properties: &'static [Property],
    /// Which of the engine's projection blocks its stages read.
    pub projection: Projection,
    /// Its source in each shading language it has one in.
    pub sources: Sources,
}

/// Which of the engine's projection blocks a [`Program`]'s stages read, as
/// its vertex stage does to project: a program's stages read one at most.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    /// None: its geometry is in clip space or the view's, as a quad over
    /// the whole frame. A draw binds a projection block of zeros all the
    /// same, since GLSL declares one in every program.
    None,
    /// [`PROJECTION_BUFFER`], whose element at the draw's index the engine
    /// writes from the tile projection the draw was pushed with.
    PerDraw,
    /// [`DRAW_PROJECTION_BUFFER`], a block of each draw's own, which the
    /// draw's maker writes.
    Own,
}

/// A buffer a program reads, by its name in the program's shaders.
///
/// A program's buffers take the ids that the constructors give them, and
/// the stages that may read them:
///
/// - a uniform block, the id of its slot, which both stages may read
///   ([`uniform`](ProgramBuffer::uniform));
/// - the values of each draw, in a storage buffer of an element per draw
///   ([`storage`](ProgramBuffer::storage)) or in a uniform block of the draw's own
///   ([`draw_uniform`](ProgramBuffer::draw_uniform)), the id of the [`Stages`]
///   that may read them;
/// - a vertex buffer that the vertex stage reads as a storage buffer, the
///   id of its slot ([`vertices`](ProgramBuffer::vertices)).
///
/// A stage that names a buffer it may not read has no declaration of it,
/// and fails to compile. [`Program::check`] fails for a program with two
/// buffers at one id.
#[derive(Clone, Copy, Debug)]
pub struct ProgramBuffer {
    /// The id a draw binds it by.
    pub id: usize,
    /// The name its shaders read it by: the variable of a block, or of an
    /// array of its elements or of the vertices. GLSL ES has no variable: a
    /// stage reads a block's members by their own names.
    pub name: &'static str,
    pub kind: BufferKind,
}

/// How a program reads a [`ProgramBuffer`].
#[derive(Clone, Copy, Debug)]
pub enum BufferKind {
    /// One block at a uniform slot's id.
    Uniform(&'static Block),
    /// One block of a draw's values, at the id of the [`Stages`] that may
    /// read it. It differs from a [`Uniform`](BufferKind::Uniform) only in
    /// the descriptor set its GLSL declares it in, a draw's own rather than
    /// a layer's, and the Vulkan and D3D12 backends bind either by its id.
    DrawUniform(&'static Block),
    /// An array of `block`s, which a shader indexes: by its draw's
    /// [`global_index`](crate::gpu::Draw::global_index), as for an element
    /// per draw of a layer, or by an index of its own, as for a value per
    /// feature. GLSL ES has no storage buffers: a draw binds its element as
    /// a uniform block, so a shader reads only that one where
    /// [`Caps::indexes_storage_buffers`](crate::gpu::Caps::indexes_storage_buffers)
    /// is false.
    ///
    /// A shader indexes inside the buffer the draw binds, here and in
    /// [`Vertices`](BufferKind::Vertices). A read past it reads zero or a
    /// value of the buffer on WebGPU, and on Vulkan where the device has
    /// `robustBufferAccess` on, as `mln_vulkan`'s own device does where the
    /// GPU offers it; on Metal, D3D12 and a Vulkan device without it, what
    /// it reads is undefined, as the GPU's own rules say.
    Storage { block: &'static Block },
    /// A vertex buffer of vertices of `format` that the vertex stage reads
    /// as a storage buffer, where the backend binds them
    /// ([`Caps::binds_vertex_buffers`](crate::gpu::Caps::binds_vertex_buffers)):
    /// in MSL as an array of the vertices, in WGSL and GLSL as one of 32-bit
    /// words, floats for a format of floats, and otherwise integers that
    /// hold two 16-bit or four 8-bit values each. Its format is one that
    /// every language reads whole words of a vertex of: 1, 2 or 4 `Float`s,
    /// `Int`s or `UInt`s, 2 or 4 `Short`s or `UShort`s, or 4 `Byte`s or
    /// `UByte`s.
    Vertices { format: AttributeDataType },
}

/// The stages of a program that may read a buffer of each draw's values,
/// which give the buffer its id. A program has one such buffer for each,
/// and the buffer that both stages may read takes the id of uniform slot 0
/// ([`ProgramBuffer::uniform`]), so a program with one has its uniform block at
/// slot 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stages {
    /// Only the vertex stage.
    Vertex,
    /// Only the fragment stage.
    Fragment,
    /// Both stages.
    Both,
}

impl Stages {
    /// The id of a buffer of each draw's values that these stages may read.
    const fn id(self) -> usize {
        match self {
            Stages::Vertex => VERTEX_STORAGE_ID,
            Stages::Fragment => FRAGMENT_STORAGE_ID,
            Stages::Both => FIRST_UNIFORM_ID,
        }
    }

    /// Whether these stages are `stage` or include it.
    const fn include(self, stage: Stage) -> bool {
        matches!(
            (self, stage),
            (Stages::Both, _) | (Stages::Vertex, Stage::Vertex) | (Stages::Fragment, Stage::Fragment)
        )
    }
}

impl ProgramBuffer {
    /// The uniform block `name` of a `T`, which all the draws of a layer
    /// share, at uniform block slot `slot`, 0 or 1.
    #[must_use]
    pub const fn uniform<T: UniformBlock>(name: &'static str, slot: usize) -> ProgramBuffer {
        assert!(slot < UNIFORM_SLOTS, "a program has two uniform block slots");
        ProgramBuffer { id: FIRST_UNIFORM_ID + slot, name, kind: BufferKind::Uniform(T::BLOCK) }
    }

    /// The uniform block `name` of a `T` of a draw's own values, which each
    /// draw binds a buffer of its own for, and which `stages` read.
    #[must_use]
    pub const fn draw_uniform<T: UniformBlock>(name: &'static str, stages: Stages) -> ProgramBuffer {
        ProgramBuffer { id: stages.id(), name, kind: BufferKind::DrawUniform(T::BLOCK) }
    }

    /// The storage buffer `name` of a `T` per draw, which `stages` read.
    #[must_use]
    pub const fn storage<T: UniformBlock>(name: &'static str, stages: Stages) -> ProgramBuffer {
        assert!(size_of::<T>().is_multiple_of(16), "a block's size is a multiple of 16");
        ProgramBuffer { id: stages.id(), name, kind: BufferKind::Storage { block: T::BLOCK } }
    }

    /// The vertex buffer `name` of vertices of `format`, which the vertex
    /// stage reads as a storage buffer, at vertex buffer slot `slot`, 0, 1
    /// or 2.
    #[must_use]
    pub const fn vertices(name: &'static str, slot: usize, format: AttributeDataType) -> ProgramBuffer {
        assert!(slot < MAX_VERTEX_BUFFERS, "a program has three vertex buffer slots");
        ProgramBuffer { id: FIRST_VERTEX_BUFFER_ID + slot, name, kind: BufferKind::Vertices { format } }
    }

    /// The block the shader reads of it, or of each of its elements; `None`
    /// for a vertex buffer.
    #[must_use]
    pub const fn block(&self) -> Option<&'static Block> {
        match self.kind {
            BufferKind::Uniform(block) | BufferKind::DrawUniform(block) | BufferKind::Storage { block, .. } => {
                Some(block)
            }
            BufferKind::Vertices { .. } => None,
        }
    }

    /// The bytes the shader reads of it: its block, a storage buffer's
    /// element, or a vertex.
    #[must_use]
    pub const fn size(&self) -> usize {
        match self.kind {
            BufferKind::Uniform(block) | BufferKind::DrawUniform(block) | BufferKind::Storage { block } => block.size,
            BufferKind::Vertices { format } => format.stride(),
        }
    }

    /// The stages that may read it, as [`ProgramBuffer`] says: both for an
    /// engine's buffer.
    const fn stages(&self) -> Stages {
        match self.kind {
            BufferKind::Uniform(_) => Stages::Both,
            BufferKind::DrawUniform(_) | BufferKind::Storage { .. } => match self.id {
                VERTEX_STORAGE_ID => Stages::Vertex,
                FRAGMENT_STORAGE_ID => Stages::Fragment,
                _ => Stages::Both,
            },
            BufferKind::Vertices { .. } => Stages::Vertex,
        }
    }
}

/// How many uniform blocks that all of a layer's draws share a program can
/// read.
const UNIFORM_SLOTS: usize = FIRST_VERTEX_BUFFER_ID - FIRST_UNIFORM_ID;

/// The view's block, which the engine binds for every draw.
pub const VIEW_BUFFER: ProgramBuffer =
    ProgramBuffer { id: VIEW_BUFFER_ID, name: "view", kind: BufferKind::Uniform(ViewUBO::BLOCK) };

/// The projection block of each draw, a storage buffer that a layer binds
/// for the draws of a program whose vertex stage projects with it.
pub const PROJECTION_BUFFER: ProgramBuffer = ProgramBuffer {
    id: PROJECTION_BUFFER_ID,
    name: "projectionVector",
    kind: BufferKind::Storage { block: ProjectionUBO::BLOCK },
};

/// The projection block of a draw's own, at the projection block's id, for
/// a program whose draws each bind a buffer of their own for it.
pub const DRAW_PROJECTION_BUFFER: ProgramBuffer = ProgramBuffer {
    id: PROJECTION_BUFFER_ID,
    name: "drawProjection",
    kind: BufferKind::DrawUniform(ProjectionUBO::BLOCK),
};

/// The engine's buffers, which any program's stages may read without the
/// program listing them: the view's block, and the projection block as
/// [`PROJECTION_BUFFER`] or [`DRAW_PROJECTION_BUFFER`].
pub static ENGINE_BUFFERS: [ProgramBuffer; 3] = [VIEW_BUFFER, PROJECTION_BUFFER, DRAW_PROJECTION_BUFFER];

/// The name a stage reads the draw's index into the storage buffers it
/// reads by, the draw's [`global_index`](crate::gpu::Draw::global_index),
/// which the backend binds.
pub const DRAW_INDEX: &str = "drawIndex";

/// What a stage of a program may read, which a backend declares for a stage
/// that names it ([`Program::bindings`]).
#[derive(Clone, Copy, Debug)]
pub enum Binding<'a> {
    /// The draw's index, [`DRAW_INDEX`].
    DrawIndex,
    /// One of the engine's buffers or the program's.
    Buffer(&'a ProgramBuffer),
    /// The texture at a slot of the program's textures.
    Texture(usize, &'a ProgramTexture),
    /// The sampler of the texture at a slot of the program's textures.
    Sampler(usize, &'a ProgramTexture),
}

impl Binding<'_> {
    /// The name a stage reads it by.
    #[must_use]
    pub fn name(&self) -> Cow<'static, str> {
        match *self {
            Binding::DrawIndex => Cow::Borrowed(DRAW_INDEX),
            Binding::Buffer(buffer) => Cow::Borrowed(buffer.name),
            Binding::Texture(_, texture) => Cow::Borrowed(texture.name),
            Binding::Sampler(_, texture) => Cow::Owned(texture.sampler_name()),
        }
    }
}

/// A vertex or instance attribute of a program: of a paint property's
/// values where it has the property's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attribute {
    /// Its name: a [`Property`]'s binds the property's values.
    pub name: &'static str,
    /// The format a draw without data for it binds.
    pub format: AttributeDataType,
}

impl Attribute {
    /// The attribute `name` of `format`.
    #[must_use]
    pub const fn new(name: &'static str, format: AttributeDataType) -> Attribute {
        Attribute { name, format }
    }
}

/// A texture a program samples, by its name in the program's shaders.
///
/// A fragment stage reads the texture as `<name>` and the sampler that a
/// draw binds with it as `<name>_sampler`, in WGSL, MSL and GLSL; it may
/// sample a texture with another texture's sampler. GLSL ES reads both as
/// the combined sampler `u_<name>`. A vertex stage samples no texture: it
/// reads the texels of one whose [`vertex`](Self::vertex) says so, by their
/// coordinates, without a sampler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgramTexture {
    /// The name its shaders read it by.
    pub name: &'static str,
    /// Whether its shaders may filter it, with a filtering sampler: not a
    /// texture of a format that can't be filtered, as `Rgba32Float`, or one
    /// whose shaders only read texels of it.
    pub filterable: bool,
    /// Whether its shaders need its texels at full precision, as numbers to
    /// decode rather than colors: GLSL ES then samples it with a `highp`
    /// sampler, where a sampler is `lowp` by default and a driver may read
    /// its bytes too coarsely. The other languages sample every texture at
    /// full precision.
    pub precise: bool,
    /// Whether the vertex stage reads it too, texel by texel and without a
    /// sampler: by `texelFetch` in GLSL and GLSL ES, `textureLoad` in WGSL
    /// and `read` in MSL. The backends then declare it for the vertex stage
    /// as well, and its sampler for the fragment stage alone.
    pub vertex: bool,
}

impl ProgramTexture {
    /// The filterable texture `name`.
    #[must_use]
    pub const fn new(name: &'static str) -> ProgramTexture {
        ProgramTexture { name, filterable: true, precise: false, vertex: false }
    }

    /// The unfilterable texture `name`.
    #[must_use]
    pub const fn unfilterable(name: &'static str) -> ProgramTexture {
        ProgramTexture { name, filterable: false, precise: false, vertex: false }
    }

    /// The texture, sampled at full precision ([`precise`](Self::precise)).
    #[must_use]
    pub const fn precise(self) -> ProgramTexture {
        ProgramTexture { precise: true, ..self }
    }

    /// The texture, read by the vertex stage too ([`vertex`](Self::vertex)).
    #[must_use]
    pub const fn vertex(self) -> ProgramTexture {
        ProgramTexture { vertex: true, ..self }
    }

    /// The name its shaders read its sampler by, `<name>_sampler`.
    #[must_use]
    pub fn sampler_name(&self) -> String {
        format!("{}_sampler", self.name)
    }
}

/// A paint property a program reads: from its uniform in a variant that
/// reads it from there, otherwise from the values a layer binds to the
/// program's attribute of its name, which a source reads through the
/// directives that [`source`](crate::shaders::source#properties)
/// describes, or, in a program without one that reads vertex buffers as
/// storage buffers ([`ProgramBuffer::vertices`]), from such a buffer, as its
/// source reads it.
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
    /// The name of the uniform, `u_<name>`, and of its attribute.
    pub name: &'static str,
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
    /// The float `u_<name>`.
    #[must_use]
    pub const fn float(name: &'static str) -> Property {
        Property { name, ty: PropertyType::Float }
    }

    /// The color `u_<name>`.
    #[must_use]
    pub const fn color(name: &'static str) -> Property {
        Property { name, ty: PropertyType::Color }
    }

    /// The unmixed `vec4` `u_<name>`.
    #[must_use]
    pub const fn vec4(name: &'static str) -> Property {
        Property { name, ty: PropertyType::Vec4 }
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
    /// test as `OVERDRAW_INSPECTOR`
    /// ([`source`](crate::shaders::source#the-overdraw-inspector)).
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
    /// [`defines`](ProgramParameters::defines), a `HAS_UNIFORM_<name>` define for
    /// each of `uniforms`, and the globe's define for its variant.
    #[must_use]
    pub fn variant_key<'a>(
        &'static self,
        parameters: &ProgramParameters,
        uniforms: impl IntoIterator<Item = &'a Property>,
        variant: ProjectionVariant,
    ) -> ShaderKey {
        let parameters = parameters.defines();
        let uniforms = uniforms.into_iter().map(|property| ["HAS_UNIFORM_", property.name].concat());
        let globe = (variant == ProjectionVariant::Globe).then(|| GLOBE_DEFINE.to_owned());
        ShaderKey::new(self, parameters.chain(uniforms).chain(globe))
    }

    /// The shader location of the attribute at `index` among those that
    /// step by `step`.
    #[must_use]
    pub fn location(&self, step: VertexStep, index: usize) -> Option<usize> {
        let (attributes, first) = match step {
            VertexStep::Vertex => (self.attributes, 0),
            VertexStep::Instance => (self.instance_attributes, self.attributes.len()),
        };
        (index < attributes.len()).then_some(first + index)
    }

    /// The step and index of the attribute of `property`'s values, the
    /// attribute of its name.
    #[must_use]
    pub fn property_attribute(&self, property: &Property) -> Option<(VertexStep, usize)> {
        let find = |attributes: &[Attribute]| attributes.iter().position(|attribute| attribute.name == property.name);
        find(self.attributes)
            .map(|index| (VertexStep::Vertex, index))
            .or_else(|| find(self.instance_attributes).map(|index| (VertexStep::Instance, index)))
    }

    /// Checks the rules this declaration's doc states, which a backend checks
    /// before it compiles the program: distinct attribute names, an
    /// attribute of each property's name unless the program reads vertex
    /// buffers as storage buffers, and so a property's values itself,
    /// buffers at ids of their kinds, as [`ProgramBuffer`]'s constructors
    /// give them, each id once, vertex buffers of the formats
    /// [`BufferKind::Vertices`] names, no more attributes and textures than
    /// a program can have, and distinct names of the buffers, the textures
    /// and their samplers, none of them a name of the engine's buffers or
    /// [`DRAW_INDEX`]. Also that the names of the engine's blocks and the
    /// program's are distinct, since a backend declares a block of a name
    /// once, and the GLSL ES names of their members and of the textures,
    /// since GLSL ES has them in one scope.
    ///
    /// # Errors
    ///
    /// [`GpuError::Invalid`], naming the rule the program breaks.
    pub fn check(&self) -> Result<(), GpuError> {
        let invalid = |problem: String| Err(GpuError::Invalid(format!("program {}: {problem}", self.name)));
        let attributes: Vec<&Attribute> = self.attributes.iter().chain(self.instance_attributes).collect();
        for (index, attribute) in attributes.iter().enumerate() {
            if attributes[..index].iter().any(|earlier| earlier.name == attribute.name) {
                return invalid(format!("two attributes are named {}", attribute.name));
            }
        }
        let pulls = self.buffers.iter().any(|buffer| matches!(buffer.kind, BufferKind::Vertices { .. }));
        if let Some(property) =
            self.properties.iter().find(|property| !pulls && self.property_attribute(property).is_none())
        {
            return invalid(format!("no attribute of property {}", property.name));
        }
        let attributes = self.attributes.len() + self.instance_attributes.len();
        if attributes > MAX_ATTRIBUTES {
            return invalid(format!("{attributes} attributes, more than {MAX_ATTRIBUTES}"));
        }
        if self.textures.len() > MAX_TEXTURES {
            return invalid(format!("{} textures, more than {MAX_TEXTURES}", self.textures.len()));
        }
        let per_draw = [Stages::Vertex.id(), Stages::Fragment.id(), Stages::Both.id()];
        for (index, buffer) in self.buffers.iter().enumerate() {
            let fits = match buffer.kind {
                BufferKind::Uniform(_) => (FIRST_UNIFORM_ID..FIRST_VERTEX_BUFFER_ID).contains(&buffer.id),
                BufferKind::DrawUniform(_) | BufferKind::Storage { .. } => per_draw.contains(&buffer.id),
                BufferKind::Vertices { .. } => {
                    (FIRST_VERTEX_BUFFER_ID..FIRST_VERTEX_BUFFER_ID + MAX_VERTEX_BUFFERS).contains(&buffer.id)
                }
            };
            if !fits {
                return invalid(format!("buffer {} has the id {}, which its kind can't have", buffer.name, buffer.id));
            }
            if let BufferKind::Vertices { format } = buffer.kind
                && !reads_whole_words(format)
            {
                return invalid(format!(
                    "buffer {} has vertices of {format:?}, which not every language reads",
                    buffer.name
                ));
            }
            if let Some(earlier) = self.buffers[..index].iter().find(|earlier| earlier.id == buffer.id) {
                let slot_0 = |buffer: &ProgramBuffer| matches!(buffer.kind, BufferKind::Uniform(_));
                if slot_0(earlier) || slot_0(buffer) {
                    return invalid(format!(
                        "buffers {} and {} have the id {}: uniform slot 0 has the id of the buffer of each draw's \
                         values that both stages read, so take slot 1",
                        earlier.name, buffer.name, buffer.id
                    ));
                }
                return invalid(format!("buffers {} and {} have the id {}", earlier.name, buffer.name, buffer.id));
            }
        }
        let mut names: Vec<Cow<'_, str>> = ENGINE_BUFFERS.iter().map(|buffer| Cow::Borrowed(buffer.name)).collect();
        names.push(Cow::Borrowed(DRAW_INDEX));
        names.extend(self.buffers.iter().map(|buffer| Cow::Borrowed(buffer.name)));
        for (slot, texture) in self.textures.iter().enumerate() {
            names.extend([Binding::Texture(slot, texture).name(), Binding::Sampler(slot, texture).name()]);
        }
        if let Some(name) = repeated(names) {
            return invalid(format!("two buffers, textures, samplers or the engine's bindings are named {name}"));
        }
        // The engine's projection buffers are one block.
        let mut blocks: Vec<&Block> = Vec::new();
        for block in ENGINE_BUFFERS.iter().filter_map(ProgramBuffer::block) {
            if !blocks.iter().any(|earlier| earlier.name == block.name) {
                blocks.push(block);
            }
        }
        blocks.extend(self.buffers.iter().filter_map(ProgramBuffer::block));
        if let Some(name) = repeated(blocks.iter().map(|block| block.name).collect()) {
            return invalid(format!("two blocks are named {name}"));
        }
        let mut names: Vec<String> = blocks
            .iter()
            .flat_map(|block| block.members.iter().map(|member| glsl_es::member_name(block, member)))
            .collect();
        names.extend(self.textures.iter().map(glsl_es::texture_name));
        if let Some(name) = repeated(names) {
            return invalid(format!("two block members or textures are {name} in GLSL ES"));
        }
        self.check_projection()
    }

    /// Checks that the projection block that the program's stages and
    /// prelude read in WGSL, MSL and GLSL is the one its
    /// [`projection`](Program::projection) names: a draw of a program that
    /// reads another gets a block of zeros, and one stage can't read one
    /// block while the other reads the other at its id. GLSL ES isn't
    /// checked: its backend's prelude reads the projection's members itself.
    fn check_projection(&self) -> Result<(), GpuError> {
        let Sources { wgsl, msl, glsl, .. } = self.sources;
        let stages = [
            ("WGSL", wgsl.map(|source| [source.prelude, source.vertex, source.fragment])),
            ("MSL", msl.map(|source| [source.prelude, source.source, ""])),
            ("GLSL", glsl.map(|source| [source.prelude, source.vertex, source.fragment])),
        ];
        for (language, texts) in stages {
            let Some(texts) = texts else { continue };
            let reads = |name: &str| texts.iter().any(|text| source::identifiers(text).any(|word| word == name));
            let read = match (reads(PROJECTION_BUFFER.name), reads(DRAW_PROJECTION_BUFFER.name)) {
                (false, false) => Projection::None,
                (true, false) => Projection::PerDraw,
                (false, true) => Projection::Own,
                (true, true) => {
                    return Err(GpuError::Invalid(format!(
                        "program {}: its {language} reads both projections",
                        self.name
                    )));
                }
            };
            if read != self.projection {
                return Err(GpuError::Invalid(format!(
                    "program {}: its {language} reads the projection {read:?}, but it declares {:?}",
                    self.name, self.projection
                )));
            }
        }
        Ok(())
    }

    /// [`check`](Self::check)s the program, and that a backend of `caps`
    /// binds what `texts`, its source in the backend's language, reads: no
    /// vertex buffer that its vertex stage reads as a storage buffer
    /// ([`BufferKind::Vertices`]) and that `texts` name outside their
    /// comments, where [`Caps::binds_vertex_buffers`] is false. So one
    /// program can pull vertices in one language and read attributes in
    /// another. A backend checks this before it compiles the program.
    ///
    /// # Errors
    ///
    /// As [`check`](Self::check), and [`GpuError::Unsupported`] for a
    /// program that reads vertex buffers on a backend that binds none.
    pub fn check_on(&self, caps: &Caps, texts: &[&str]) -> Result<(), GpuError> {
        self.check()?;
        let named = |name: &str| texts.iter().any(|text| source::identifiers(text).any(|word| word == name));
        if !caps.binds_vertex_buffers
            && let Some(buffer) = self
                .buffers
                .iter()
                .find(|buffer| matches!(buffer.kind, BufferKind::Vertices { .. }) && named(buffer.name))
        {
            return Err(GpuError::Unsupported(format!(
                "program {}: it reads vertex buffer {}, which this backend's bindings don't bind",
                self.name, buffer.name
            )));
        }
        Ok(())
    }

    /// What `stage` may read, in the order a backend declares it in for a
    /// stage that names it: the draw's index, the engine's buffers, the
    /// program's buffers that the stage may read, as [`ProgramBuffer`] says,
    /// and the program's textures, in a fragment stage each followed by its
    /// sampler, and in a vertex stage only those it reads
    /// ([`ProgramTexture::vertex`]), without their samplers.
    pub fn bindings(&self, stage: Stage) -> impl Iterator<Item = Binding<'_>> {
        let buffers = ENGINE_BUFFERS.iter().chain(self.buffers).filter(move |buffer| buffer.stages().include(stage));
        let textures = self.textures.iter().enumerate().filter(move |(_, texture)| match stage {
            Stage::Vertex => texture.vertex,
            Stage::Fragment => true,
        });
        let samplers = move |slot, texture| (stage == Stage::Fragment).then_some(Binding::Sampler(slot, texture));
        std::iter::once(Binding::DrawIndex).chain(buffers.map(Binding::Buffer)).chain(textures.flat_map(
            move |(slot, texture)| std::iter::once(Binding::Texture(slot, texture)).chain(samplers(slot, texture)),
        ))
    }

    /// The buffer whose block is `name`: one of the program's, or the
    /// engine's, of which [`PROJECTION_BUFFER`] for the projection block.
    #[must_use]
    pub fn buffer_named(&self, name: &str) -> Option<&'static ProgramBuffer> {
        self.buffers.iter().chain(&ENGINE_BUFFERS).find(|buffer| buffer.block().is_some_and(|block| block.name == name))
    }
}

/// Whether every language reads vertices of `format` from a storage buffer
/// as whole words: the formats of [`BufferKind::Vertices`].
const fn reads_whole_words(format: AttributeDataType) -> bool {
    use AttributeDataType::{
        Byte4, Float, Float2, Float4, Int, Int2, Int4, Short2, Short4, UByte4, UInt, UInt2, UInt4, UShort2, UShort4,
    };
    matches!(
        format,
        Float
            | Float2
            | Float4
            | Int
            | Int2
            | Int4
            | UInt
            | UInt2
            | UInt4
            | Short2
            | Short4
            | UShort2
            | UShort4
            | Byte4
            | UByte4
    )
}

/// A name that `names` has twice, if any.
fn repeated<T: Ord>(mut names: Vec<T>) -> Option<T> {
    names.sort_unstable();
    let at = names.windows(2).position(|pair| pair[0] == pair[1])?;
    Some(names.swap_remove(at))
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
    use crate::gfx_types::AttributeDataType::{Float3, Short2};
    use crate::shaders::block::{Member, MemberType, Precision};

    static EMPTY: Program = Program {
        name: "empty",
        buffers: &[],
        attributes: &[],
        instance_attributes: &[],
        textures: &[],
        properties: &[],
        projection: Projection::None,
        sources: Sources::NONE,
    };

    #[test]
    fn a_program_breaking_a_rule_of_its_declaration_fails_its_check() {
        const POSITION: Attribute = Attribute::new("position", Short2);
        // A block of the name of the view's, whose members GLSL ES names
        // apart from the view's.
        static OTHER_VIEW: Block = Block {
            name: "ViewUBO",
            size: 16,
            members: &[Member {
                name: "x",
                ty: MemberType::Vec4,
                offset: 0,
                precision: Precision::High,
                padding: false,
            }],
            glsl_es_prefix: "other_",
        };
        static OTHER: [ProgramBuffer; 1] =
            [ProgramBuffer { id: FIRST_UNIFORM_ID, name: "props", kind: BufferKind::Uniform(&OTHER_VIEW) }];
        assert!(EMPTY.check().is_ok());
        let fails = |program: Program| assert!(program.check().is_err(), "{:?}", program.buffers);
        fails(Program { attributes: &[POSITION, POSITION], ..EMPTY });
        fails(Program { properties: const { &[Property::color("color")] }, ..EMPTY });
        let pulling = Program {
            buffers: const { &[ProgramBuffer::vertices("vertices", 0, Short2)] },
            properties: const { &[Property::color("color")] },
            ..EMPTY
        };
        assert!(pulling.check().is_ok(), "a program that pulls its properties' values");
        fails(Program {
            buffers: const {
                &[ProgramBuffer::uniform::<ViewUBO>("props", 0), ProgramBuffer::uniform::<ViewUBO>("other", 0)]
            },
            ..EMPTY
        });
        fails(Program {
            buffers: const {
                &[
                    ProgramBuffer::uniform::<ViewUBO>("props", 0),
                    ProgramBuffer::storage::<ViewUBO>("drawable", Stages::Both),
                ]
            },
            ..EMPTY
        });
        fails(Program {
            buffers: const {
                &[ProgramBuffer {
                    id: PROJECTION_BUFFER_ID,
                    ..ProgramBuffer::storage::<ViewUBO>("drawable", Stages::Vertex)
                }]
            },
            ..EMPTY
        });
        fails(Program {
            buffers: const { &[ProgramBuffer { id: 2, ..ProgramBuffer::vertices("vertices", 0, Short2) }] },
            ..EMPTY
        });
        fails(Program {
            buffers: const {
                &[ProgramBuffer::vertices("vertices", 0, Short2), ProgramBuffer::vertices("vertices", 1, Short2)]
            },
            ..EMPTY
        });
        // The view's members again, in one GLSL ES scope with the view's.
        fails(Program { buffers: const { &[ProgramBuffer::uniform::<ViewUBO>("props", 0)] }, ..EMPTY });
        // Vertices that MSL reads 16 bytes apart, and WGSL and GLSL 12.
        fails(Program { buffers: const { &[ProgramBuffer::vertices("vertices", 0, Float3)] }, ..EMPTY });
        // The names of the engine's bindings, and a sampler's name again.
        fails(Program { buffers: const { &[ProgramBuffer::vertices("view", 0, Short2)] }, ..EMPTY });
        fails(Program { textures: const { &[ProgramTexture::new("drawIndex")] }, ..EMPTY });
        fails(Program {
            buffers: const { &[ProgramBuffer::vertices("image_sampler", 0, Short2)] },
            textures: const { &[ProgramTexture::new("image")] },
            ..EMPTY
        });
        // A block of the name of the view's.
        fails(Program { buffers: &OTHER, ..EMPTY });
    }

    #[test]
    #[cfg(feature = "wgsl")]
    fn a_program_declares_the_projection_its_vertex_stage_reads() {
        static READS: wgsl::Wgsl = wgsl::Wgsl {
            vertex: "let clip = projectTile(pos, projectionVector[drawIndex]);",
            fragment: "",
            prelude: "",
        };
        // One stage reads one projection block and the other the other.
        static BOTH: wgsl::Wgsl = wgsl::Wgsl { fragment: "let z = drawProjection.x;", ..READS };
        let reading = Program { sources: Sources::NONE.wgsl(&READS), ..EMPTY };
        assert!(reading.check().is_err(), "a program that reads the projection and declares none");
        assert!(Program { projection: Projection::PerDraw, ..reading }.check().is_ok());
        assert!(Program { projection: Projection::Own, ..reading }.check().is_err());
        assert!(Program { projection: Projection::PerDraw, ..EMPTY }.check().is_ok(), "no source to check");
        let both = Program { projection: Projection::PerDraw, sources: Sources::NONE.wgsl(&BOTH), ..EMPTY };
        assert!(both.check().is_err(), "a fragment stage that reads the other projection");
    }

    #[test]
    fn a_stage_may_read_what_its_declaration_says() {
        static PROGRAM: Program = Program {
            buffers: const {
                &[
                    ProgramBuffer::storage::<ViewUBO>("vertexOnly", Stages::Vertex),
                    ProgramBuffer::draw_uniform::<ViewUBO>("fragmentOnly", Stages::Fragment),
                    ProgramBuffer::storage::<ViewUBO>("both", Stages::Both),
                    ProgramBuffer::uniform::<ViewUBO>("props", 1),
                    ProgramBuffer::vertices("vertices", 0, Short2),
                ]
            },
            textures: const { &[ProgramTexture::new("image"), ProgramTexture::new("heights").vertex()] },
            ..EMPTY
        };
        let names = |stage| PROGRAM.bindings(stage).map(|binding| binding.name().into_owned()).collect::<Vec<_>>();
        let engine = ["drawIndex", "view", "projectionVector", "drawProjection"];
        assert_eq!(
            names(Stage::Vertex),
            [&engine[..], &["vertexOnly", "both", "props", "vertices", "heights"]].concat(),
            "a vertex stage reads only the textures that say so, without samplers"
        );
        assert_eq!(
            names(Stage::Fragment),
            [&engine[..], &["fragmentOnly", "both", "props", "image", "image_sampler", "heights", "heights_sampler"]]
                .concat()
        );
    }

    #[test]
    fn a_buffer_takes_the_id_of_what_it_is() {
        let uniforms = [0, 1].map(|slot| ProgramBuffer::uniform::<ViewUBO>("props", slot).id);
        assert_eq!(uniforms, [5, 6]);
        let per_draw = [Stages::Vertex, Stages::Fragment, Stages::Both];
        assert_eq!(per_draw.map(|stages| ProgramBuffer::storage::<ViewUBO>("values", stages).id), [2, 3, 5]);
        assert_eq!(per_draw.map(|stages| ProgramBuffer::draw_uniform::<ViewUBO>("values", stages).id), [2, 3, 5]);
        assert_eq!([0, 1, 2].map(|slot| ProgramBuffer::vertices("vertices", slot, Short2).id), [7, 8, 9]);
        assert_eq!(ProgramBuffer::vertices("vertices", 0, Short2).size(), 4);
    }

    #[test]
    fn parameters_define_the_pixel_ratio_and_the_overdraw_inspector() {
        let defines = |pixel_ratio, overdraw| ProgramParameters { pixel_ratio, overdraw }.defines().collect::<Vec<_>>();
        assert_eq!(defines(1.0, false), ["DEVICE_PIXEL_RATIO=1.0"]);
        assert_eq!(defines(1.5, true), ["DEVICE_PIXEL_RATIO=1.5", "OVERDRAW_INSPECTOR"]);
    }

    #[test]
    fn a_program_binds_vertex_buffers_only_where_its_backend_does() {
        let pulling = Program { buffers: const { &[ProgramBuffer::vertices("vertices", 0, Short2)] }, ..EMPTY };
        let caps = |binds_vertex_buffers| Caps {
            buffer_offset_alignment: 4,
            max_texture_size: 4096,
            half_float_render_targets: false,
            clip_depth: crate::gpu::ClipDepthRange::ZeroToOne,
            render_target_depth: None,
            clear_stencil_with_pass: false,
            cpu_rasterizer: false,
            max_line_width: 1.0,
            binds_vertex_buffers,
            indexes_storage_buffers: true,
        };
        let reads = ["vec2 at = vertices[gl_VertexIndex];"];
        assert!(pulling.check_on(&caps(true), &reads).is_ok());
        assert!(matches!(pulling.check_on(&caps(false), &reads), Err(GpuError::Unsupported(_))));
        // A source that reads attributes instead, as GLSL ES does.
        assert!(pulling.check_on(&caps(false), &["// vertices\nin vec2 a_pos;"]).is_ok());
        assert!(EMPTY.check_on(&caps(false), &reads).is_ok());
    }
}
