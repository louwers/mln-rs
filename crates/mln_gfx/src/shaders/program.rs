//! What a program reads, the same on every backend. A backend compiles a
//! program from the source registered for it.

use std::hash::{Hash, Hasher};
use std::{fmt, ptr};

use mln_util::hash::HashMap;

use crate::gfx_types::AttributeDataType;
use crate::gpu::{GpuError, ShaderKey, VertexStep};
use crate::projection_variant::ProjectionVariant;
use crate::shaders::bindings::{
    FIRST_UNIFORM_ID, FIRST_VERTICES_ID, FRAGMENT_DRAW_ID, PAINT_PARAMS_ID, PROJECTION_ID, UNIFORM_SLOTS,
    VERTEX_DRAW_ID, VERTICES_SLOTS,
};
use crate::shaders::block::{Block, UniformBlock};
use crate::shaders::layer_ubo::{GlobalPaintParamsUBO, ProjectionUBO};
use crate::shaders::program_parameters::ProgramParameters;

/// A program: the buffers, attributes, textures and paint properties its
/// shader reads.
///
/// A program is the `static` that declares it: two programs are equal only
/// if they are the same `static`, and a backend finds the source it compiles
/// a program from by it.
///
/// A buffer binds at the id of its [`ProgramBuffer`]
/// ([`buffer_id`](Program::buffer_id)). Any program may read the engine's
/// paint parameters ([`PAINT_PARAMS`]) and the draw's index, which it lists
/// none of, and reads its projection as its
/// [`projection`](Program::projection) says. An attribute's location is its
/// index in [`attributes`](Program::attributes), or, past the vertex
/// attributes', in [`instance_attributes`](Program::instance_attributes),
/// and a texture's slot its index in [`textures`](Program::textures): a draw
/// names them by these ([`location`](Program::location),
/// [`texture_slot`](Program::texture_slot)), and a backend binds each where
/// its shaders read it, which is there unless its sources number them
/// otherwise.
pub struct Program {
    /// Its name, for logs and labels.
    pub name: &'static str,
    /// The buffers of its own that its shaders read.
    pub buffers: &'static [ProgramBuffer],
    /// How its vertex stage reads the draw's projection.
    pub projection: Projection,
    /// The attributes that step once a vertex.
    pub attributes: &'static [Attribute],
    /// The attributes that step once an instance.
    pub instance_attributes: &'static [Attribute],
    /// The textures it samples.
    pub textures: &'static [ProgramTexture],
    /// The paint properties it reads: those whose `HAS_UNIFORM_u_<name>`
    /// its source tests, and no others, since a variant defines only these.
    pub properties: &'static [Property],
}

/// How a [`Program`]'s vertex stage reads the draw's projection block, a
/// [`ProjectionUBO`], at [`PROJECTION_ID`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    /// It reads none: its geometry is in clip space or the view's, as a quad
    /// over the whole frame. A draw binds a projection block all the same,
    /// since GLSL ES declares one in every program.
    None,
    /// From [`PROJECTION`], the element of each draw.
    PerDraw,
    /// From [`OWN_PROJECTION`], a block of the draw's own.
    Own,
}

/// A buffer a program reads, by its name in the program's shaders.
///
/// A program's buffers take the ids that the constructors give them
/// ([`bindings`](crate::shaders::bindings)):
///
/// - a uniform block that all of a layer's draws share, the id of its slot
///   ([`uniform`](ProgramBuffer::uniform));
/// - a block of each draw's values, in a storage buffer of an element per
///   draw ([`storage`](ProgramBuffer::storage)) or in a uniform block of the
///   draw's own ([`draw_uniform`](ProgramBuffer::draw_uniform)), the id of
///   the [`Stages`] that read it;
/// - a vertex buffer that the vertex stage reads as a storage buffer, the
///   id of its slot ([`vertices`](ProgramBuffer::vertices)).
#[derive(Clone, Copy, Debug)]
pub struct ProgramBuffer {
    /// The id a draw binds it at.
    pub id: usize,
    /// The name its shaders read it by: the variable of a block, or of an
    /// array of its elements or of the vertices.
    pub name: &'static str,
    pub kind: BufferKind,
}

/// How a program reads a [`ProgramBuffer`].
#[derive(Clone, Copy, Debug)]
pub enum BufferKind {
    /// A block that all of a layer's draws share.
    Uniform(&'static Block),
    /// A block of the draw's own.
    DrawUniform(&'static Block),
    /// An array of blocks, `stride` bytes apart, which a shader indexes by
    /// the draw's `globalIndex`
    /// ([`Draw::global_index`](crate::gpu::Draw::global_index)). A stride
    /// larger than the block's size is that of a union of the blocks of the
    /// layer's programs, whose draws write one buffer.
    Storage { block: &'static Block, stride: usize },
    /// A vertex buffer, which the vertex stage reads as a storage buffer, by
    /// vertex index, where the backend binds one
    /// ([`Caps::binds_vertex_buffers`](crate::gpu::Caps::binds_vertex_buffers)).
    Vertices,
}

/// The stages of a program that read a block of each draw's values, which
/// give the block its id. A program has one such block for each, and the
/// block that both stages read takes the id of uniform slot 0, so a program
/// with one has its uniform block at slot 1.
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
    /// The id of a block of each draw's values that these stages read.
    #[must_use]
    pub const fn id(self) -> usize {
        match self {
            Stages::Vertex => VERTEX_DRAW_ID,
            Stages::Fragment => FRAGMENT_DRAW_ID,
            Stages::Both => FIRST_UNIFORM_ID,
        }
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

    /// The uniform block `name` of a `T` of a draw's own values, which
    /// `stages` read.
    #[must_use]
    pub const fn draw_uniform<T: UniformBlock>(name: &'static str, stages: Stages) -> ProgramBuffer {
        ProgramBuffer { id: stages.id(), name, kind: BufferKind::DrawUniform(T::BLOCK) }
    }

    /// The storage buffer `name` of a `T` per draw, which `stages` read.
    #[must_use]
    pub const fn storage<T: UniformBlock>(name: &'static str, stages: Stages) -> ProgramBuffer {
        ProgramBuffer::union_storage::<T, T>(name, stages)
    }

    /// The storage buffer `name` of elements of a `U`, a union of a `T` and
    /// the blocks of the other programs whose draws write the buffer, which
    /// `stages` read.
    #[must_use]
    pub const fn union_storage<T: UniformBlock, U: bytemuck::Pod>(name: &'static str, stages: Stages) -> ProgramBuffer {
        assert!(size_of::<T>() <= size_of::<U>(), "a union is as large as its blocks");
        ProgramBuffer { id: stages.id(), name, kind: BufferKind::Storage { block: T::BLOCK, stride: size_of::<U>() } }
    }

    /// The vertex buffer `name`, which the vertex stage reads as a storage
    /// buffer, at vertex buffer slot `slot`, 0, 1 or 2.
    #[must_use]
    pub const fn vertices(name: &'static str, slot: usize) -> ProgramBuffer {
        assert!(slot < VERTICES_SLOTS, "a program has three vertex buffer slots");
        ProgramBuffer { id: FIRST_VERTICES_ID + slot, name, kind: BufferKind::Vertices }
    }

    /// The block the shaders read of it, or of each of its elements; `None`
    /// for a vertex buffer.
    #[must_use]
    pub const fn block(&self) -> Option<&'static Block> {
        match self.kind {
            BufferKind::Uniform(block) | BufferKind::DrawUniform(block) | BufferKind::Storage { block, .. } => {
                Some(block)
            }
            BufferKind::Vertices => None,
        }
    }

    /// The bytes between its elements; `None` for a buffer of one block or
    /// of vertices.
    #[must_use]
    pub const fn stride(&self) -> Option<usize> {
        match self.kind {
            BufferKind::Storage { stride, .. } => Some(stride),
            _ => None,
        }
    }
}

/// The paint parameters' block, which the renderer binds for every draw.
pub const PAINT_PARAMS: ProgramBuffer =
    ProgramBuffer { id: PAINT_PARAMS_ID, name: "paintParams", kind: BufferKind::Uniform(GlobalPaintParamsUBO::BLOCK) };

/// The projection of each draw, a storage buffer that a layer writes from
/// the tile projections of its draws.
pub const PROJECTION: ProgramBuffer = ProgramBuffer {
    id: PROJECTION_ID,
    name: "projectionVector",
    kind: BufferKind::Storage { block: ProjectionUBO::BLOCK, stride: size_of::<ProjectionUBO>() },
};

/// The projection of a draw's own.
pub const OWN_PROJECTION: ProgramBuffer =
    ProgramBuffer { id: PROJECTION_ID, name: "projection", kind: BufferKind::DrawUniform(ProjectionUBO::BLOCK) };

/// A vertex or instance attribute of a program, which GLSL ES reads as
/// `a_<name>`: of a paint property's values where it has the property's
/// name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub name: &'static str,
    /// The format a draw without data for it binds.
    pub format: AttributeDataType,
}

impl Attribute {
    #[must_use]
    pub const fn new(name: &'static str, format: AttributeDataType) -> Attribute {
        Attribute { name, format }
    }
}

/// A texture a program samples, which GLSL ES reads as the sampler
/// `u_<name>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgramTexture {
    pub name: &'static str,
    /// Whether a shader may filter it: not one whose shaders only read its
    /// texels, or of a format that can't be filtered, as `Rgba32Float`.
    pub filterable: bool,
}

impl ProgramTexture {
    /// A slot that the program's shaders don't read, which keeps the
    /// textures after it at the slots their sources name.
    pub const UNUSED: ProgramTexture = ProgramTexture { name: "", filterable: true };

    /// The filterable texture `name`.
    #[must_use]
    pub const fn new(name: &'static str) -> ProgramTexture {
        ProgramTexture { name, filterable: true }
    }

    /// The unfilterable texture `name`.
    #[must_use]
    pub const fn unfilterable(name: &'static str) -> ProgramTexture {
        ProgramTexture { name, filterable: false }
    }

    /// Whether it is [`UNUSED`](Self::UNUSED).
    #[must_use]
    pub const fn is_unused(&self) -> bool {
        self.name.is_empty()
    }
}

/// A paint property a program reads: from its uniform, `u_<name>`, in a
/// variant that reads it from there, otherwise from the values a layer binds
/// to the program's attribute of its name.
#[derive(Clone, Copy, Debug)]
pub struct Property {
    pub name: &'static str,
}

impl Property {
    #[must_use]
    pub const fn new(name: &'static str) -> Property {
        Property { name }
    }
}

/// The define that compiles a program's globe variants.
pub const GLOBE_DEFINE: &str = "PROJECTION_GLOBE";
/// The define that compiles a program's variants that stand on terrain.
pub const TERRAIN_DEFINE: &str = "TERRAIN";

impl Program {
    /// The location of the attribute `name`: its index in
    /// [`attributes`](Program::attributes), or in
    /// [`instance_attributes`](Program::instance_attributes) past the
    /// vertex attributes'. For a `const`, whose build fails where the
    /// program has no such attribute, as the call does.
    #[must_use]
    pub const fn location(&self, name: &str) -> usize {
        let location = match position(self.attributes, name) {
            Some(index) => Some(index),
            None => match position(self.instance_attributes, name) {
                Some(index) => Some(self.attributes.len() + index),
                None => None,
            },
        };
        assert!(location.is_some(), "the program has no attribute of the name");
        match location {
            Some(location) => location,
            None => 0,
        }
    }

    /// The attribute at `location` ([`location`](Program::location)), and
    /// the step of its buffer.
    #[must_use]
    pub fn attribute_at(&self, location: usize) -> Option<(VertexStep, &'static Attribute)> {
        match location.checked_sub(self.attributes.len()) {
            None => Some((VertexStep::Vertex, &self.attributes[location])),
            Some(index) => self.instance_attributes.get(index).map(|attribute| (VertexStep::Instance, attribute)),
        }
    }

    /// The attributes, each with its location and the step of its buffer.
    pub fn locations(&self) -> impl Iterator<Item = (usize, VertexStep, &'static Attribute)> + use<> {
        let vertex = self.attributes.iter().map(|attribute| (VertexStep::Vertex, attribute));
        let instance = self.instance_attributes.iter().map(|attribute| (VertexStep::Instance, attribute));
        vertex.chain(instance).enumerate().map(|(location, (step, attribute))| (location, step, attribute))
    }

    /// The slot of the texture `name`, its index in
    /// [`textures`](Program::textures). For a `const`, whose build fails
    /// where the program has no such texture, as the call does.
    #[must_use]
    pub const fn texture_slot(&self, name: &str) -> usize {
        let mut slot = 0;
        while slot < self.textures.len() && (self.textures[slot].is_unused() || !same(self.textures[slot].name, name)) {
            slot += 1;
        }
        assert!(slot < self.textures.len(), "the program has no texture of the name");
        slot
    }

    /// The id of the buffer `name` of [`buffers`](Program::buffers). For a
    /// `const`, whose build fails where the program has no such buffer, as
    /// the call does.
    #[must_use]
    pub const fn buffer_id(&self, name: &str) -> usize {
        let mut index = 0;
        while index < self.buffers.len() && !same(self.buffers[index].name, name) {
            index += 1;
        }
        assert!(index < self.buffers.len(), "the program has no buffer of the name");
        self.buffers[index].id
    }

    /// The engine's buffer that the program reads its projection from, as
    /// its [`projection`](Program::projection) says.
    #[must_use]
    pub const fn projection_buffer(&self) -> Option<&'static ProgramBuffer> {
        match self.projection {
            Projection::None => None,
            Projection::PerDraw => Some(&PROJECTION),
            Projection::Own => Some(&OWN_PROJECTION),
        }
    }

    /// The key of the variant compiled with `parameters` that reads
    /// `uniforms`, properties of the program, from their uniforms, for
    /// `variant`'s projection. It has the parameters' defines
    /// (`DEVICE_PIXEL_RATIO=<ratio>`), a `HAS_UNIFORM_u_<name>` define for
    /// each of `uniforms`, and the globe's and terrain's defines of its
    /// variant.
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
        let globe = variant.is_globe().then(|| GLOBE_DEFINE.to_owned());
        let terrain = variant.stands().then(|| TERRAIN_DEFINE.to_owned());
        ShaderKey::new(self, parameters.chain(uniforms).chain(globe).chain(terrain))
    }
}

/// The index of the attribute `name` in `attributes`.
const fn position(attributes: &[Attribute], name: &str) -> Option<usize> {
    let mut index = 0;
    while index < attributes.len() {
        if same(attributes[index].name, name) {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// Whether `a` and `b` are the same string, in a `const`.
const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
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

/// The source of type `S` that a backend compiles each program from.
pub struct Sources<S: 'static> {
    sources: HashMap<&'static Program, &'static S>,
}

impl<S> Sources<S> {
    /// The sources with each of `built_ins` registered.
    #[must_use]
    pub fn new(built_ins: &[(&'static Program, &'static S)]) -> Sources<S> {
        let mut sources = Sources { sources: HashMap::default() };
        for &(program, source) in built_ins {
            sources.register(program, source);
        }
        sources
    }

    /// Registers `source` as `program`'s, in place of any before.
    pub fn register(&mut self, program: &'static Program, source: &'static S) {
        self.sources.insert(program, source);
    }

    /// The source of `program`. Fails with [`GpuError::Unsupported`] if it
    /// has none.
    pub fn get(&self, program: &'static Program) -> Result<&'static S, GpuError> {
        self.sources
            .get(program)
            .copied()
            .ok_or_else(|| GpuError::Unsupported(format!("no source for {}", program.name)))
    }
}
