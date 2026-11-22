//! The handles of the renderer's resources that a layer keeps: buffers,
//! textures, the targets it draws into and samplers.

use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::texture2d::SamplerState;
use mln_util::Color;

use crate::gfx::buffer_store::SharedBuffer;
use crate::gfx::store::SharedTexture;

/// A buffer of `V`s in the renderer's [`Resources`](crate::draw::Resources):
/// vertices that a mesh reads ([`MeshDesc::vertices`](crate::draw::MeshDesc::vertices)),
/// or, of `u16`s, the indexes of its primitives
/// ([`MeshDesc::indexes`](crate::draw::MeshDesc::indexes)).
pub struct Buffer<V> {
    pub(crate) entry: SharedBuffer,
    element: PhantomData<fn() -> V>,
}

impl<V> Buffer<V> {
    pub(crate) fn new(entry: SharedBuffer) -> Buffer<V> {
        Buffer { entry, element: PhantomData }
    }

    /// The number of `V`s in the buffer.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entry.count()
    }

    /// Whether the buffer holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<V> PartialEq for Buffer<V> {
    /// Whether the handles are of one buffer.
    fn eq(&self, other: &Self) -> bool {
        self.entry == other.entry
    }
}

impl<V> Clone for Buffer<V> {
    fn clone(&self) -> Self {
        Buffer { entry: self.entry.clone(), element: PhantomData }
    }
}

impl<V> std::fmt::Debug for Buffer<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffer").field("len", &self.len()).finish_non_exhaustive()
    }
}

/// A type of vertex that a mesh reads: the formats of its attributes, each
/// right after the last in the vertex, and the program's attributes they
/// bind to: those of their [`NAMES`](Vertex::NAMES), or else as many of
/// the program's attributes that read no paint property, in order, after
/// those that the mesh's earlier vertices without names bound.
pub trait Vertex: bytemuck::Pod {
    /// The formats of the vertex's attributes, whose sizes add up to the
    /// vertex's.
    const ATTRIBUTES: &'static [AttributeDataType];

    /// The names of the program's attributes that the vertex's attributes
    /// bind to, one for each: an attribute whose program has none of its
    /// name binds to none, and one of a paint property's name gives the
    /// property its values per vertex. Empty for a vertex whose attributes
    /// bind in order.
    const NAMES: &'static [&'static str] = &[];
}

macro_rules! vertices {
    ($($ty:ty => $format:ident,)*) => {$(
        impl Vertex for $ty {
            const ATTRIBUTES: &'static [AttributeDataType] = &[AttributeDataType::$format];
        }
    )*};
}

vertices! {
    [i16; 2] => Short2,
    [i16; 4] => Short4,
    [u16; 2] => UShort2,
    [u16; 4] => UShort4,
    [u8; 4] => UByte4,
    f32 => Float,
    [f32; 2] => Float2,
    [f32; 3] => Float3,
    [f32; 4] => Float4,
}

/// A vertex of a surface that an image drapes over
/// ([`TileSurface`]): where it is, in its tile's units, and where it samples
/// the image, in tile units from the image's top left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct ImageVertex {
    /// Where the vertex is.
    pub position: [i16; 2],
    /// Where it samples the image.
    pub texture: [u16; 2],
}

impl ImageVertex {
    /// The vertex at `p` that samples the image at `t`.
    #[must_use]
    pub(crate) fn new(p: geometry::Point<i16>, t: geometry::Point<u16>) -> ImageVertex {
        ImageVertex { position: [p.x, p.y], texture: [t.x, t.y] }
    }
}

impl Vertex for ImageVertex {
    const ATTRIBUTES: &'static [AttributeDataType] = &[AttributeDataType::Short2, AttributeDataType::Short2];
}

/// A buffer of `u16` indexes in the renderer's
/// [`Resources`](crate::draw::Resources), each relative to the first vertex
/// of its segment, with the segments that draw them: a mesh's primitives
/// ([`MeshDesc::indexes`](crate::draw::MeshDesc::indexes)).
#[derive(Clone, Debug)]
pub struct IndexBuffer {
    pub(crate) buffer: Buffer<u16>,
    pub(crate) segments: Vec<SegmentBase>,
}

impl IndexBuffer {
    /// The same indexes, of which a mesh draws only the segment at `index`,
    /// or none where there is no such segment.
    #[must_use]
    pub fn segment(&self, index: usize) -> IndexBuffer {
        IndexBuffer { buffer: self.buffer.clone(), segments: self.segments.get(index).copied().into_iter().collect() }
    }
}

/// A texture in the renderer's [`Resources`](crate::draw::Resources), which
/// a mesh samples ([`MeshDesc::texture`](crate::draw::MeshDesc::texture)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture(pub(crate) SharedTexture);

/// A texture in the renderer's [`Resources`](crate::draw::Resources) that
/// a layer's draws draw into before the frame's passes
/// ([`Draws::target`](crate::draw::Draws::target)), cleared to a color
/// first, and which its meshes then sample ([`texture`](Self::texture)).
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub(crate) texture: Texture,
    /// What the target is cleared to before its draws.
    pub(crate) clear: Color,
}

impl Target {
    /// The texture the target's draws draw into, to sample. It stays the
    /// same texture when the target takes another size
    /// ([`Resources::resize`](crate::draw::Resources::resize)).
    #[must_use]
    pub fn texture(&self) -> &Texture {
        &self.texture
    }
}

/// How meshes sample a texture ([`MeshDesc::texture`](crate::draw::MeshDesc::texture)),
/// which a layer may change from frame to frame: each mesh bound with the
/// sampler, or a clone of it, samples as it last said, from its next draw
/// on. A [`SamplerState`] converts into a sampler that nothing else changes.
#[derive(Clone, Debug)]
pub struct Sampler(Rc<Cell<SamplerState>>);

impl Sampler {
    /// A sampler that samples as `state` says.
    #[must_use]
    pub fn new(state: SamplerState) -> Sampler {
        Sampler(Rc::new(Cell::new(state)))
    }

    /// How the sampler samples.
    #[must_use]
    pub fn get(&self) -> SamplerState {
        self.0.get()
    }

    /// Samples as `state` says from now on.
    pub fn set(&self, state: SamplerState) {
        self.0.set(state);
    }
}

impl From<SamplerState> for Sampler {
    fn from(state: SamplerState) -> Sampler {
        Sampler::new(state)
    }
}

impl From<&Sampler> for Sampler {
    fn from(sampler: &Sampler) -> Sampler {
        sampler.clone()
    }
}

/// The surface that a tile's geometry covers in a frame's projection, in
/// the tile's units: its extent's corners on Mercator, and on the globe a
/// grid that bends with the planet and reaches a pole where the tile
/// touches one, with the triangles that cover it
/// ([`Resources::tile_surface`](crate::draw::Resources::tile_surface)). The
/// surface of a tile's image has vertices of the image's
/// ([`ImageVertex`], [`DemTile::surface`](crate::layer::DemTile::surface)).
#[derive(Clone, Debug)]
pub struct TileSurface<V = [i16; 2]> {
    /// The vertices.
    pub vertices: Buffer<V>,
    /// Their triangles.
    pub triangles: IndexBuffer,
}

#[cfg(test)]
mod tests {
    use mln_gfx::types::TextureFilterType;

    use super::*;

    #[test]
    fn a_samplers_clones_sample_as_it_last_said_and_a_states_alone() {
        let nearest = SamplerState::default();
        let linear = SamplerState { filter: TextureFilterType::Linear, ..nearest };
        let sampler = Sampler::new(nearest);
        let (clone, own) = (Sampler::from(&sampler), Sampler::from(nearest));
        sampler.set(linear);
        assert_eq!((linear, nearest), (clone.get(), own.get()));
    }
}
