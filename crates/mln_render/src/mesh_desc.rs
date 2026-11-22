//! What a mesh is made of ([`MeshDesc`]), and how the resources make one.

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::VertexStep;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::MAX_TEXTURES;
use mln_gfx::shaders::block::DrawBlocks;
use mln_gfx::shaders::program::{Attribute, BufferKind, Program};
use smallvec::SmallVec;

use crate::draw_objects::{DrawGeometry, DrawObjects, Mesh, Textures};
use crate::gfx::buffer_store::SharedBuffer;
use crate::gfx::context::Resources;
use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::handles::{Buffer, IndexBuffer, Sampler, Texture, Vertex};
use crate::paint_property_binder::{MeshPaint, PaintBinders, PaintVertices};

/// What a [`Mesh`] is made of: a program, the indexes of its primitives and
/// the buffers its attributes read, per vertex or per instance, the paint
/// values it reads, its textures, and the state it draws in. The resources make the mesh
/// ([`Resources::mesh`]), which then holds what it was made of.
///
/// A mesh draws triangles, blended as premultiplied colors over the frame
/// (or as the overdraw inspector shows them, where it's on), with no
/// culling, in the frame's projection, unless the description says
/// otherwise.
pub struct MeshDesc<'a> {
    pub(crate) program: &'static Program,
    /// The buffers of the vertex attributes, in the program's order.
    pub(crate) vertices: SmallVec<[Input<'a>; 2]>,
    /// The buffers of the instance attributes, in the program's order.
    pub(crate) instances: SmallVec<[Input<'a>; 2]>,
    /// The buffers of the program's vertex buffers that it reads as storage
    /// buffers, in the program's order.
    pub(crate) pulled: SmallVec<[&'a SharedBuffer; 2]>,
    pub(crate) indexes: &'a IndexBuffer,
    /// The paint values' attributes, by step and attribute index, and the
    /// properties, by index, read from their uniforms instead.
    pub(crate) paint: Vec<(VertexStep, usize, SharedAttribute)>,
    pub(crate) uniforms: Uniforms,
    /// What the engine writes of the paint values into the draws' blocks.
    pub(crate) mesh_paint: Option<MeshPaint>,
    pub(crate) textures: SmallVec<[(&'a Texture, Sampler); 2]>,
    pub(crate) primitive: DrawMode,
    /// The blend, `None` for the default.
    pub(crate) blend: Option<ColorMode>,
    pub(crate) cull: CullFaceMode,
    /// The projection the mesh draws in, `None` for the frame's.
    pub(crate) projection: Option<ProjectionVariant>,
}

/// A buffer that a mesh's attributes read from its vertex `first` on, with
/// the formats of its vertex.
#[derive(Clone, Copy)]
pub(crate) struct Input<'a> {
    pub(crate) entry: &'a SharedBuffer,
    pub(crate) first: u32,
    pub(crate) attributes: &'static [AttributeDataType],
    /// The names of the program's attributes they bind to; empty to bind
    /// them in order.
    pub(crate) names: &'static [&'static str],
    pub(crate) stride: u32,
}

impl<'a> Input<'a> {
    fn of<V: Vertex>(buffer: &'a Buffer<V>, first: u32) -> Input<'a> {
        debug_assert_eq!(
            V::ATTRIBUTES.iter().map(|format| format.stride()).sum::<usize>(),
            size_of::<V>(),
            "a vertex whose attributes' formats don't add up to its size"
        );
        debug_assert!(
            V::NAMES.is_empty() || V::NAMES.len() == V::ATTRIBUTES.len(),
            "a vertex with names for some attributes"
        );
        Input { entry: &buffer.entry, first, attributes: V::ATTRIBUTES, names: V::NAMES, stride: size_of::<V>() as u32 }
    }

    /// The vertices the input reads.
    fn count(&self) -> usize {
        self.entry.count().saturating_sub(self.first as usize)
    }
}

impl<'a> MeshDesc<'a> {
    /// A mesh of `program` that draws the primitives of `indexes`, each of
    /// their segments a draw of a run of them, with nothing else bound yet.
    #[must_use]
    pub fn new(program: &'static Program, indexes: &'a IndexBuffer) -> Self {
        MeshDesc {
            program,
            vertices: SmallVec::new(),
            instances: SmallVec::new(),
            pulled: SmallVec::new(),
            indexes,
            paint: Vec::new(),
            uniforms: Uniforms::default(),
            mesh_paint: None,
            textures: SmallVec::new(),
            primitive: DrawMode::Triangles,
            blend: None,
            cull: CullFaceMode::disabled(),
            projection: None,
        }
    }

    /// Binds `buffer`'s vertices to the program's vertex attributes, as
    /// [`Vertex`] says.
    pub fn vertices<V: Vertex>(&mut self, buffer: &'a Buffer<V>) -> &mut Self {
        self.vertices.push(Input::of(buffer, 0));
        self
    }

    /// Binds `buffer`'s vertices to the program's instance attributes, as
    /// [`vertices`](Self::vertices) does to its vertex attributes: the mesh
    /// draws an instance of its primitives per vertex of the buffer, as
    /// many as the instance attributes' buffers have, the fewest of them.
    pub fn instances<V: Vertex>(&mut self, buffer: &'a Buffer<V>) -> &mut Self {
        self.instances_from(buffer, 0)
    }

    /// Binds `buffer`'s vertices from the `first` on to the program's
    /// instance attributes, as [`instances`](Self::instances) does from
    /// the first: an instance reads the vertex `first` places after its
    /// own, as of a neighbor. It binds the buffer at an offset, which every
    /// backend can.
    pub fn instances_from<V: Vertex>(&mut self, buffer: &'a Buffer<V>, first: u32) -> &mut Self {
        self.instances.push(Input::of(buffer, first));
        self
    }

    /// Binds `buffer` to the program's next vertex buffer that its vertex
    /// stage reads as a storage buffer
    /// ([`Buffer::vertices`](mln_gfx::shaders::program::Buffer::vertices)),
    /// in the program's order, to read any of its vertices by index. A
    /// program reads such buffers only where the backend binds them
    /// ([`Caps::binds_vertex_buffers`](mln_gfx::gpu::Caps::binds_vertex_buffers)).
    pub fn pulled<V: bytemuck::Pod>(&mut self, buffer: &'a Buffer<V>) -> &mut Self {
        self.pulled.push(&buffer.entry);
        self
    }

    /// Binds the values of the data-driven paint properties that `paint`
    /// holds to the program's attributes of the properties' names, of a
    /// vertex or of an instance, and reads those that `values`, the layer's
    /// evaluated paint values, has the same for every feature from their
    /// uniforms instead. A program without an attribute of a property's
    /// name reads its values itself, from the paint's buffer
    /// ([`PaintVertices::buffer`], [`pulled`](Self::pulled)). The engine
    /// then writes what the program's blocks hold of the properties into
    /// each draw of the mesh ([`Group::push`](crate::draw::Group::push)).
    ///
    /// A property whose attribute neither `paint` nor a buffer of the mesh
    /// binds ([`Vertex::NAMES`]) reads its uniform.
    pub fn paint<P: PaintBinders>(&mut self, paint: PaintVertices<'a, P>, values: &P::Paint) -> &mut Self {
        let MeshDesc { program, paint: attributes, uniforms, mesh_paint, .. } = self;
        *mesh_paint = Some(paint.bind(program, values, attributes, uniforms));
        self
    }

    /// Binds `texture` to the program's next texture, in the program's
    /// order, sampled as `sampler` says in each frame.
    pub fn texture(&mut self, texture: &'a Texture, sampler: impl Into<Sampler>) -> &mut Self {
        debug_assert!(self.textures.len() < MAX_TEXTURES, "more textures than a mesh binds");
        self.textures.push((texture, sampler.into()));
        self
    }

    /// Draws `mode`'s primitives instead of triangles.
    pub fn primitive(&mut self, mode: DrawMode) -> &mut Self {
        self.primitive = mode;
        self
    }

    /// Blends exactly as `mode` says, the overdraw inspector or not.
    pub fn blend(&mut self, mode: ColorMode) -> &mut Self {
        self.blend = Some(mode);
        self
    }

    /// Culls as `mode` says.
    pub fn cull(&mut self, mode: CullFaceMode) -> &mut Self {
        self.cull = mode;
        self
    }

    /// Draws in Mercator's projection on the globe too, as geometry of the
    /// viewport rather than the map does
    /// ([`TileTransform::screen_space`](crate::layer::TileTransform::screen_space)).
    pub fn screen_space(&mut self) -> &mut Self {
        self.projection(ProjectionVariant::Mercator)
    }

    /// Draws in `variant`'s projection, whichever the frame's is.
    pub(crate) fn projection(&mut self, variant: ProjectionVariant) -> &mut Self {
        self.projection = Some(variant);
        self
    }
}

impl Resources {
    /// The mesh of `desc`, whose draws give the values `B` of its program's
    /// blocks of each draw ([`BufferKind::Storage`] and
    /// [`BufferKind::DrawUniform`]). Panics where `B` isn't those blocks,
    /// in order. A mesh of a program with blocks of a draw's own, or a
    /// projection of its own
    /// ([`Projection::Own`](mln_gfx::shaders::program::Projection::Own)), draws once a frame:
    /// its draw writes them to the mesh's buffers.
    pub fn mesh<B: DrawBlocks>(&mut self, desc: &MeshDesc<'_>) -> Mesh<B> {
        let program = desc.program;
        assert!(
            program
                .buffers
                .iter()
                .filter_map(|buffer| match buffer.kind {
                    BufferKind::Storage { block, .. } | BufferKind::DrawUniform(block) =>
                        Some((block.name, block.size)),
                    _ => None,
                })
                .eq(B::BLOCKS.iter().map(|block| (block.name, block.size))),
            "a mesh of {} whose draws give other blocks than its program's of each draw",
            program.name
        );
        let mut vertices = VertexAttributeArray::new();
        let mut instances = (!desc.instances.is_empty()).then(VertexAttributeArray::new);
        for (step, id, attribute) in &desc.paint {
            match (step, &mut instances) {
                (VertexStep::Vertex, _) => vertices.set_shared(*id, attribute.clone()),
                (VertexStep::Instance, Some(instances)) => instances.set_shared(*id, attribute.clone()),
                (VertexStep::Instance, None) => debug_assert!(false, "per-instance paint of a mesh without instances"),
            }
        }
        let mut vertex_count = usize::MAX;
        for input in &desc.vertices {
            vertex_count = vertex_count.min(input.count());
        }
        bind_inputs(program, program.attributes, &mut vertices, &desc.vertices);
        if let Some(instances) = &mut instances {
            bind_inputs(program, program.instance_attributes, instances, &desc.instances);
        }
        let mut uniforms = desc.uniforms;
        for (index, property) in program.properties.iter().enumerate() {
            let bound = match program.property_attribute(property) {
                Some((VertexStep::Vertex, attribute)) => vertices.get(attribute).is_some(),
                Some((VertexStep::Instance, attribute)) => {
                    instances.as_ref().is_some_and(|i| i.get(attribute).is_some())
                }
                None => true,
            };
            if !bound {
                uniforms.insert(index);
            }
        }
        let mut textures = Textures::default();
        for (slot, (texture, sampler)) in textures.iter_mut().zip(&desc.textures) {
            *slot = Some((texture.0.clone(), sampler.clone()));
        }
        let blend = desc.blend.unwrap_or_else(|| self.color_mode(ColorMode::alpha_blended()));
        let geometry = DrawGeometry {
            vertices,
            vertex_count: if vertex_count == usize::MAX { 0 } else { vertex_count },
            mode: desc.primitive,
            indexes: desc.indexes.buffer.entry.clone(),
            segments: desc.indexes.segments.clone(),
            instances,
        };
        let mut objects = DrawObjects::new(program, uniforms, desc.projection, blend, desc.cull, geometry, textures);
        let mut pulled = program.buffers.iter().filter(|buffer| matches!(buffer.kind, BufferKind::Vertices { .. }));
        for entry in &desc.pulled {
            let Some(buffer) = pulled.next() else {
                debug_assert!(false, "a mesh of {} that pulls more buffers than its program reads", program.name);
                break;
            };
            objects.pull(buffer.id, (*entry).clone());
        }
        self.add_mesh(objects, desc.mesh_paint.clone())
    }
}

/// Binds `inputs` to `attributes`, the program's attributes of one step:
/// those of an input's vertex's names, or else, in order, those that read
/// no paint property.
fn bind_inputs(program: &Program, attributes: &[Attribute], array: &mut VertexAttributeArray, inputs: &[Input<'_>]) {
    let mut unnamed = attributes
        .iter()
        .enumerate()
        .filter(|(_, attribute)| program.properties.iter().all(|property| property.name != attribute.name))
        .map(|(index, _)| index);
    for input in inputs {
        if input.names.is_empty() {
            bind(array, &mut unnamed, input);
        } else {
            let mut named = input
                .names
                .iter()
                .map(|name| attributes.iter().position(|attribute| attribute.name == *name).unwrap_or(usize::MAX));
            bind(array, &mut named, input);
        }
    }
}

/// Binds `input`'s attributes, one per format of its vertex, to the next of
/// `attributes`, the indexes of the program's attributes.
fn bind(array: &mut VertexAttributeArray, attributes: &mut impl Iterator<Item = usize>, input: &Input<'_>) {
    let mut offset = 0;
    for &format in input.attributes {
        let Some(attribute) = attributes.next() else {
            debug_assert!(false, "a vertex with more attributes than its program");
            return;
        };
        if attribute != usize::MAX {
            let shared = SharedAttribute::new(input.entry.clone(), format, offset, input.stride);
            array.set_shared(attribute, shared.starting_at(input.first));
        }
        offset += format.stride() as u32;
    }
}
