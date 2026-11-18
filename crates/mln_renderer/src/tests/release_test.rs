use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BufferUsage, GeometryDesc, GpuError, ShaderKey, TextureRegion, VertexAttribute,
    VertexBufferLayout, VertexStep,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO};
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_util::size::Size;

use super::with_context;
use crate::gfx::buffer::SharedBuffer;
use crate::gfx::owned::Held;

// A uniform buffer replaced because its data grew, a texture replaced
// because its size changed, and vertices and indexes their owners let go of
// stay while the bindings and geometry made of them do, and go after them,
// out of the stats too. A backend refuses, in debug builds, to destroy one
// before them, which fails the destroy.
#[test]
fn bindings_and_geometry_keep_what_they_were_made_of() {
    with_context(|context| {
        let projection = size_of::<ProjectionUBO>();
        let mut slot = None;
        context.device().write_uniform_buffer(&mut slot, &vec![0; projection]);
        let ubo = slot.clone().expect("a uniform buffer");
        let texture = context.add_texture_with(|texture| texture.set_size(Size::new(2, 2)));
        let (texture_id, sampler) = context.bind_texture(&texture).expect("the texture");
        let vertices = context.device().buffer(BufferUsage::Vertex, &[0; 16]).expect("vertices");
        let indices =
            context.device().buffer(BufferUsage::Index, bytemuck::cast_slice(&[0u16, 1, 2, 3])).expect("indices");

        let backend = std::rc::Rc::clone(context.backend());
        let mut gpu = backend.gpu();
        let shader = context.shader(&mut *gpu, &ShaderKey::new(BuiltIn::ClippingMaskProgram, [])).expect("the shader");
        let entries = [
            BindGroupEntry::Buffer {
                id: ID_PROJECTION_UBO as u32,
                buffer: ubo.id(),
                offset: 0,
                size: projection as u64,
            },
            BindGroupEntry::Texture { id: 0, texture: texture_id, sampler },
        ];
        let bindings =
            gpu.create_bindings(&BindingsDesc { label: "test", shader, entries: &entries }).expect("the bindings");
        let layout = VertexBufferLayout {
            stride: 4,
            step: VertexStep::Vertex,
            attributes: vec![VertexAttribute {
                id: ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE as u32,
                format: AttributeDataType::Short2,
                offset: 0,
            }],
        };
        let geometry = gpu
            .create_geometry(&GeometryDesc {
                shader,
                layouts: &[layout],
                vertex_buffers: &[(vertices.id(), 0)],
                index_buffer: indices.id(),
            })
            .expect("the geometry");
        drop(gpu);
        let gpu_texture = context.gpu_texture(&texture).cloned().expect("the texture's");
        let bindings = context.own(bindings, vec![Held::Buffer(ubo.clone()), Held::Texture(gpu_texture)]);
        let geometry = context.own(geometry, vec![Held::Buffer(vertices.clone()), Held::Buffer(indices.clone())]);
        let (ubo_id, vertices_id, indices_id) = (ubo.id(), vertices.id(), indices.id());
        drop((ubo, vertices, indices));
        context.device().write_uniform_buffer(&mut slot, &vec![0; 2 * projection]);
        assert_ne!(slot.as_ref().map(SharedBuffer::id), Some(ubo_id), "a buffer the data fits in");
        context.texture_mut(&texture).expect("the texture").set_size(Size::new(4, 4));
        context.bind_texture(&texture).expect("the resized texture");

        let stats = context.stats();
        context.destroy_released();
        assert_eq!(context.stats(), stats, "nothing went");
        let mut gpu = backend.gpu();
        for buffer in [ubo_id, vertices_id, indices_id] {
            assert!(gpu.write_buffer(buffer, 0, &[0; 4]).is_ok(), "{buffer:?} stays");
        }
        let texel = TextureRegion { x: 0, y: 0, size: Size::new(1, 1) };
        assert!(gpu.write_texture(texture_id, texel, &[0; 4]).is_ok(), "the replaced texture stays");
        drop(gpu);

        drop((bindings, geometry));
        context.destroy_released();
        let mut gpu = backend.gpu();
        for buffer in [ubo_id, vertices_id, indices_id] {
            assert!(matches!(gpu.write_buffer(buffer, 0, &[0; 4]), Err(GpuError::UnknownId("buffer"))));
        }
        assert!(matches!(gpu.write_texture(texture_id, texel, &[0; 4]), Err(GpuError::UnknownId("texture"))));
        drop(gpu);
        let gone = context.stats();
        assert_eq!(gone.num_buffers, stats.num_buffers - 3);
        assert_eq!(gone.num_active_textures, stats.num_active_textures - 1);
        assert_eq!(gone.mem_textures, stats.mem_textures - 2 * 2 * 4);
    });
}
