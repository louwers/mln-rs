//! What a mesh is made of ([`MeshDesc`]), and how the resources make one.

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::MAX_TEXTURES;
use mln_gfx::shaders::block::DrawBlocks;
use mln_gfx::shaders::program::{Attribute, BufferKind, Program, Projection};
use mln_gfx::texture2d::SamplerState;
use smallvec::SmallVec;

use crate::draw_objects::{DrawGeometry, DrawObjects, Mesh, Textures};
use crate::gfx::buffer_store::SharedBuffer;
use crate::gfx::context::Resources;
use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};
use crate::handles::{Buffer, IndexBuffer, Texture, Vertex};
use crate::paint_property_binder::{MeshPaint, PaintBinders, PaintVertices};

/// What a [`Mesh`] is made of: a program, the indexes of its primitives and
/// the buffers its attributes read, the paint values it reads per vertex,
/// its textures, and the state it draws in. The resources make the mesh
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
    pub(crate) instances: SmallVec<[Input<'a>; 1]>,
    pub(crate) indexes: &'a IndexBuffer,
    /// The paint values' attributes, by attribute id, and the properties
    /// read from their uniforms instead.
    pub(crate) paint: Vec<(usize, SharedAttribute)>,
    pub(crate) uniforms: Uniforms,
    /// What the engine writes of the paint values into the draws' blocks.
    pub(crate) mesh_paint: Option<MeshPaint>,
    pub(crate) textures: SmallVec<[(&'a Texture, SamplerState); 2]>,
    pub(crate) primitive: DrawMode,
    /// The blend, `None` for the default.
    pub(crate) blend: Option<ColorMode>,
    pub(crate) cull: CullFaceMode,
    /// The projection the mesh draws in, `None` for the frame's.
    pub(crate) projection: Option<ProjectionVariant>,
}

/// A buffer that a mesh's attributes read, with the formats of its vertex.
#[derive(Clone, Copy)]
pub(crate) struct Input<'a> {
    pub(crate) entry: &'a SharedBuffer,
    pub(crate) attributes: &'static [AttributeDataType],
    pub(crate) stride: u32,
}

impl<'a> Input<'a> {
    fn of<V: Vertex>(buffer: &'a Buffer<V>) -> Input<'a> {
        debug_assert_eq!(
            V::ATTRIBUTES.iter().map(|format| format.stride()).sum::<usize>(),
            size_of::<V>(),
            "a vertex whose attributes' formats don't add up to its size"
        );
        Input { entry: &buffer.entry, attributes: V::ATTRIBUTES, stride: size_of::<V>() as u32 }
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

    /// Binds `buffer`'s vertices to the program's vertex attributes that
    /// read no paint property, one per attribute of `V`, from the first
    /// that no earlier call bound.
    pub fn vertices<V: Vertex>(&mut self, buffer: &'a Buffer<V>) -> &mut Self {
        self.vertices.push(Input::of(buffer));
        self
    }

    /// Binds `buffer`'s vertices to the program's instance attributes, as
    /// [`vertices`](Self::vertices) does to its vertex attributes: the mesh
    /// draws an instance of its primitives per vertex of the buffer.
    pub(crate) fn instances<V: Vertex>(&mut self, buffer: &'a Buffer<V>) -> &mut Self {
        self.instances.push(Input::of(buffer));
        self
    }

    /// Binds the values of the data-driven paint properties that `paint`
    /// holds to the program's properties, and reads those that `values`,
    /// the layer's evaluated paint values, has the same for every feature
    /// from their uniforms instead. The engine then writes what the
    /// program's blocks hold of the properties into each draw of the mesh
    /// ([`Group::push`](crate::draw::Group::push)).
    pub fn paint<P: PaintBinders>(&mut self, paint: PaintVertices<'a, P>, values: &P::Paint) -> &mut Self {
        let MeshDesc { program, paint: attributes, uniforms, mesh_paint, .. } = self;
        *mesh_paint = Some(paint.bind(program, values, attributes, uniforms));
        self
    }

    /// Binds `texture` to the program's next texture, in the program's
    /// order, sampled as `sampler` says.
    pub fn texture(&mut self, texture: &'a Texture, sampler: SamplerState) -> &mut Self {
        debug_assert!(self.textures.len() < MAX_TEXTURES, "more textures than a mesh binds");
        self.textures.push((texture, sampler));
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

    /// Draws in `variant`'s projection, whichever the frame's is.
    pub(crate) fn projection(&mut self, variant: ProjectionVariant) -> &mut Self {
        self.projection = Some(variant);
        self
    }
}

impl Resources {
    /// The mesh of `desc`, whose draws give the values `B` of its program's
    /// blocks of each draw. Panics where `B` isn't those blocks, in order,
    /// or where the program reads blocks of each draw that only a draw's
    /// own buffer holds ([`BufferKind::DrawUniform`],
    /// [`Projection::Own`]), which draws don't give.
    pub fn mesh<B: DrawBlocks>(&mut self, desc: &MeshDesc<'_>) -> Mesh<B> {
        let program = desc.program;
        assert!(
            program.projection != Projection::Own
                && !program.buffers.iter().any(|buffer| matches!(buffer.kind, BufferKind::DrawUniform(_))),
            "a mesh of {}, whose program reads a block of a draw's own",
            program.name
        );
        assert!(
            program
                .buffers
                .iter()
                .filter_map(|buffer| match buffer.kind {
                    BufferKind::Storage { block, .. } => Some((block.name, block.size)),
                    _ => None,
                })
                .eq(B::BLOCKS.iter().map(|block| (block.name, block.size))),
            "a mesh of {} whose draws give other blocks than its program's of each draw",
            program.name
        );
        let mut vertices = VertexAttributeArray::new();
        for (id, attribute) in &desc.paint {
            vertices.set_shared(*id, attribute.clone());
        }
        let mut vertex_count = usize::MAX;
        let mut attributes = program
            .attributes
            .iter()
            .filter(|attribute| program.properties.iter().all(|property| property.attribute != attribute.id));
        for input in &desc.vertices {
            vertex_count = vertex_count.min(input.entry.count());
            bind(&mut vertices, &mut attributes, input);
        }
        let instances = (!desc.instances.is_empty()).then(|| {
            let mut instances = VertexAttributeArray::new();
            let mut attributes = program.instance_attributes.iter();
            for input in &desc.instances {
                bind(&mut instances, &mut attributes, input);
            }
            instances
        });
        let mut textures = Textures::default();
        for (slot, (texture, sampler)) in textures.iter_mut().zip(&desc.textures) {
            *slot = Some((texture.0.clone(), *sampler));
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
        let objects = DrawObjects::new(program, desc.uniforms, desc.projection, blend, desc.cull, geometry, textures);
        self.add_mesh(objects, desc.mesh_paint.clone())
    }
}

/// Binds `input`'s attributes, one per format of its vertex, to the next of
/// `attributes`.
fn bind<'p>(array: &mut VertexAttributeArray, attributes: &mut impl Iterator<Item = &'p Attribute>, input: &Input<'_>) {
    let mut offset = 0;
    for &format in input.attributes {
        let Some(attribute) = attributes.next() else {
            debug_assert!(false, "a vertex with more attributes than its program");
            return;
        };
        array.set_shared(attribute.id, SharedAttribute::new(input.entry.clone(), format, offset, input.stride));
        offset += format.stride() as u32;
    }
}
