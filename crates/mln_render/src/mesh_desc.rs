//! What a mesh is made of ([`MeshDesc`]), and how the resources make one.

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::MAX_TEXTURES;
use mln_gfx::shaders::block::DrawBlocks;
use mln_gfx::shaders::program::{BufferKind, Program};
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
    /// The paint values' attributes, by attribute index, and the
    /// properties, by index, read from their uniforms instead.
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
    /// The names of the program's attributes they bind to; empty to bind
    /// them in order.
    pub(crate) names: &'static [&'static str],
    pub(crate) stride: u32,
}

impl<'a> Input<'a> {
    fn of<V: Vertex>(buffer: &'a Buffer<V>) -> Input<'a> {
        debug_assert_eq!(
            V::ATTRIBUTES.iter().map(|format| format.stride()).sum::<usize>(),
            size_of::<V>(),
            "a vertex whose attributes' formats don't add up to its size"
        );
        debug_assert!(
            V::NAMES.is_empty() || V::NAMES.len() == V::ATTRIBUTES.len(),
            "a vertex with names for some attributes"
        );
        Input { entry: &buffer.entry, attributes: V::ATTRIBUTES, names: V::NAMES, stride: size_of::<V>() as u32 }
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

    /// Binds `buffer`'s vertices to the program's vertex attributes, as
    /// [`Vertex`] says.
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
        let (objects, paint) = self.mesh_objects::<B>(desc);
        self.add_mesh(objects, paint)
    }

    /// Makes `mesh` the mesh of `desc`, as [`mesh`](Self::mesh) makes one,
    /// in its place among the meshes' ages: its draws order among others as
    /// before.
    pub fn set_mesh<B: DrawBlocks>(&mut self, mesh: &mut Mesh<B>, desc: &MeshDesc<'_>) {
        let (objects, paint) = self.mesh_objects::<B>(desc);
        mesh.replace(objects.program(), paint);
        drop(self.take_mesh(mesh.key()));
        self.restore_mesh(mesh.key(), Box::new(objects));
    }

    /// Samples `mesh`'s texture at `slot` of its program's as `sampler`
    /// says, from its next draw on.
    pub fn set_sampler<B>(&mut self, mesh: &Mesh<B>, slot: usize, sampler: SamplerState) {
        if let Some(mut objects) = self.take_mesh(mesh.key()) {
            objects.set_sampler(slot, sampler);
            self.restore_mesh(mesh.key(), objects);
        }
    }

    /// The GPU objects of a mesh of `desc`, with what the engine writes of
    /// its paint values, as [`mesh`](Self::mesh) checks them.
    fn mesh_objects<B: DrawBlocks>(&mut self, desc: &MeshDesc<'_>) -> (DrawObjects, Option<MeshPaint>) {
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
        for (id, attribute) in &desc.paint {
            vertices.set_shared(*id, attribute.clone());
        }
        let mut vertex_count = usize::MAX;
        let mut attributes = program
            .attributes
            .iter()
            .enumerate()
            .filter(|(_, attribute)| program.properties.iter().all(|property| property.name != attribute.name))
            .map(|(index, _)| index);
        for input in &desc.vertices {
            vertex_count = vertex_count.min(input.entry.count());
            if input.names.is_empty() {
                bind(&mut vertices, &mut attributes, input);
            } else {
                let mut named = input.names.iter().map(|name| {
                    program.attributes.iter().position(|attribute| attribute.name == *name).unwrap_or(usize::MAX)
                });
                bind(&mut vertices, &mut named, input);
            }
        }
        let instances = (!desc.instances.is_empty()).then(|| {
            let mut instances = VertexAttributeArray::new();
            let mut attributes = 0..program.instance_attributes.len();
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
        (objects, desc.mesh_paint.clone())
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
            array.set_shared(attribute, SharedAttribute::new(input.entry.clone(), format, offset, input.stride));
        }
        offset += format.stride() as u32;
    }
}
