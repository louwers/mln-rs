use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, ColorAttachment, Draw, DynamicState, GeometryDesc, GeometryId, GpuError,
    LoadOp, PassDesc, Scissor, ShaderKey, TextureDesc, TextureFormat, TextureRegion, TextureUsage, VertexAttribute,
    VertexBufferLayout, VertexStep,
};
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::{Rgba, Size};

use super::with_context;
use crate::draw::indexes::{DrawIndexes, Indexes};
use crate::draw::owned::Held;
use crate::shaders::programs;

// A texture that a resize replaced, and vertices and indexes of the
// context's stores that their owners let go of, stay while the bindings and
// geometry made of them do, and go after them, out of the stats too. A
// buffer dropped with the bindings made of it goes after them. A backend
// refuses, in debug builds, to destroy one before them, which fails the
// destroy.
#[test]
fn bindings_and_geometry_go_before_what_they_were_made_of() {
    with_context(|gpu, context| {
        let projection = size_of::<ProjectionUBO>();
        let mut ubo = None;
        context.device().write_uniforms(gpu, &mut ubo, &vec![0; projection], false);
        let ubo_id = ubo.as_ref().expect("a uniform buffer").id();
        let texture = context.add_texture_with(|texture| texture.set_size(Size::new(2, 2)));
        let (texture_id, sampler) = context.texture_binding(gpu, &texture).expect("the texture");
        let vertices = context.add_vertices(vec![[0i16; 2]; 4]);
        let vertices_id = context.upload_vertices(gpu, &vertices).expect("vertices").expect("a buffer");
        let indexes = context.add_indexes(Indexes::new(vec![0, 1, 2, 3]));
        context.upload_indexes(gpu, &indexes).expect("indexes");
        let indices_id = DrawIndexes::Shared(indexes.clone()).buffer(context).expect("a buffer");

        let shader = context.shader(gpu, &ShaderKey::new(&programs::CLIPPING_MASK, [])).expect("the shader");
        let entries = [
            BindGroupEntry::Buffer { id: PROJECTION_ID as u32, buffer: ubo_id, offset: 0, size: projection as u64 },
            BindGroupEntry::Texture { slot: 0, texture: texture_id, sampler },
        ];
        let bindings =
            gpu.create_bindings(&BindingsDesc { label: "test", shader, entries: &entries }).expect("the bindings");
        let layout = position_layout();
        let geometry = gpu
            .create_geometry(&GeometryDesc {
                shader,
                layouts: &[layout],
                vertex_buffers: &[(vertices_id, 0)],
                index_buffer: indices_id,
            })
            .expect("the geometry");
        let gpu_texture = context.gpu_texture(&texture).cloned().expect("the texture's");
        let bindings = context.own(bindings, vec![Held::Texture(gpu_texture)]);
        let geometry = context.own(geometry, vec![vertices.held(), Held::Indexes(indexes.clone())]);
        drop((vertices, indexes));
        context.texture_mut(&texture).expect("the texture").set_size(Size::new(4, 4));
        context.texture_binding(gpu, &texture).expect("the resized texture");

        let stats = context.stats();
        context.destroy_released(gpu);
        assert_eq!(context.stats(), stats, "nothing went");
        for buffer in [vertices_id, indices_id] {
            assert!(gpu.write_buffer(buffer, 0, &[0; 4]).is_ok(), "{buffer:?} stays");
        }
        let texel = TextureRegion { x: 0, y: 0, size: Size::new(1, 1) };
        assert!(gpu.write_texture(texture_id, texel, &[0; 4]).is_ok(), "the replaced texture stays");

        drop((ubo, bindings, geometry));
        context.destroy_released(gpu);
        for buffer in [ubo_id, vertices_id, indices_id] {
            assert!(matches!(gpu.write_buffer(buffer, 0, &[0; 4]), Err(GpuError::UnknownId("buffer"))));
        }
        assert!(matches!(gpu.write_texture(texture_id, texel, &[0; 4]), Err(GpuError::UnknownId("texture"))));
        let gone = context.stats();
        assert_eq!(gone.num_buffers, stats.num_buffers - 3);
        assert_eq!(gone.num_active_textures, stats.num_active_textures - 1);
        assert_eq!(gone.mem_textures, stats.mem_textures - 2 * 2 * 4);
    });
}

// The shaders and pipelines that a new pixel ratio replaces go with the
// next destroy, and the context makes new ones for their keys.
// Each frame binds a mesh with its own buffers, once while they and the
// mesh's textures stay. Its bindings of the mesh go with the mesh, and all
// of its bindings with a buffer it replaces, before that buffer goes.
#[test]
fn a_frames_bindings_of_a_mesh_go_with_the_mesh_and_with_its_buffers() {
    use mln_gfx::{ColorMode, CullFaceMode, DrawMode};

    use crate::draw::objects::{DrawBuffers, DrawGeometry, Mesh, Textures};
    use crate::draw::vertex_attribute::VertexAttributeArray;

    with_context(|gpu, context| {
        let geometry = DrawGeometry {
            vertices: VertexAttributeArray::new(),
            vertex_count: 0,
            mode: DrawMode::Triangles(),
            indexes: DrawIndexes::new(vec![0, 1, 2]),
            segments: Vec::new(),
            instances: None,
        };
        let shader = ShaderKey::new(&programs::CLIPPING_MASK, []);
        let mesh =
            Mesh::new(context, shader, ColorMode::disabled(), CullFaceMode::disabled(), geometry, Textures::default());
        let (mut frame, mut other) = (DrawBuffers::default(), DrawBuffers::default());
        for buffers in [&mut frame, &mut other] {
            buffers.write_pod(gpu, PROJECTION_ID, &ProjectionUBO::default(), context.device());
        }

        let bound = frame.bind(gpu, context, &mesh).expect("the frame's bindings");
        assert_eq!(frame.bind(gpu, context, &mesh).expect("the frame's bindings"), bound);
        assert_ne!(other.bind(gpu, context, &mesh).expect("the other frame's bindings"), bound);
        frame.write_pod(gpu, PROJECTION_ID, &ProjectionUBO::default(), context.device());
        assert_eq!(frame.bind(gpu, context, &mesh).expect("the frame's bindings"), bound, "a write that fits");

        // A larger value replaces the buffer, and the bindings made of it.
        frame.write_pod(gpu, PROJECTION_ID, &[ProjectionUBO::default(); 2], context.device());
        assert_eq!(frame.bound_meshes(), 0);
        frame.bind(gpu, context, &mesh).expect("the frame's new bindings");
        context.destroy_released(gpu);
        assert!(!context.device().failed(), "the replaced buffer went after its bindings");

        drop(mesh);
        frame.sweep();
        assert_eq!((frame.bound_meshes(), other.bound_meshes()), (0, 1));
        other.sweep();
        assert_eq!(other.bound_meshes(), 0);
        drop((frame, other));
        context.destroy_released(gpu);
        assert!(!context.device().failed());
    });
}

#[test]
fn released_shaders_and_pipelines_go() {
    with_context(|gpu, context| {
        let key = ShaderKey::new(&programs::CLIPPING_MASK, []);
        let shader = context.shader(gpu, &key).expect("the shader");
        let pipeline_key = clip_mask_pipeline(shader);
        let pipeline = context.pipeline(gpu, &pipeline_key).expect("the pipeline");
        context.release_shaders();
        context.destroy_released(gpu);

        let gone = gpu.create_pipeline(&pipeline_key.desc());
        assert!(matches!(gone, Err(GpuError::UnknownId("shader"))), "{gone:?}");
        let size = Size::new(1, 1);
        let usage = TextureUsage { render_target: true, ..TextureUsage::default() };
        let target = gpu
            .create_texture(&TextureDesc {
                label: "target",
                size,
                format: TextureFormat::Rgba8Unorm,
                usage,
                mipmapped: false,
            })
            .expect("a target");
        let color = Some(ColorAttachment { texture: target, load: LoadOp::Load });
        gpu.begin_pass(&PassDesc { label: "test", color, depth_stencil: None }).expect("the pass");
        let dynamic = DynamicState {
            depth_range: (0.0, 1.0),
            scissor: Scissor { x: 0, y: 0, width: 1, height: 1 },
            stencil_reference: 0,
            blend_constant: Rgba::new(0.0, 0.0, 0.0, 1.0),
            line_width: 1.0,
        };
        let draw = Draw {
            pipeline,
            bindings: BindingsId::default(),
            geometry: GeometryId::default(),
            first_index: 0,
            index_count: 3,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
            global_index: 0,
            dynamic,
        };
        assert!(matches!(gpu.draw(&draw), Err(GpuError::UnknownId("pipeline"))), "a draw of the pipeline");
        gpu.end_pass().expect("the pass's end");
        gpu.destroy(target.into()).expect("the target's destroy");
        let remade = context.shader(gpu, &key).expect("a new shader");
        assert_ne!(remade, shader);
        let remade = context.pipeline(gpu, &clip_mask_pipeline(remade)).expect("a new pipeline");
        assert_ne!(remade, pipeline);
    });
}

/// The key of a pipeline of the clipping mask program `shader`, drawing into
/// RGBA8 without depth or stencil.
fn clip_mask_pipeline(shader: mln_gfx::gpu::ShaderId) -> mln_gfx::gpu::PipelineKey {
    use mln_gfx::color_mode::ColorMode;
    use mln_gfx::cull_face_mode::CullFaceMode;
    use mln_gfx::depth_mode::DepthMode;
    use mln_gfx::gfx_types::DrawModeType;
    use mln_gfx::gpu::{DepthState, FixedState, PipelineKey, StencilState, TargetFormats};
    use mln_gfx::stencil_mode::StencilMode;

    let color = ColorMode::unblended();
    PipelineKey {
        shader,
        vertex_buffers: vec![position_layout()],
        state: FixedState {
            primitive: DrawModeType::Triangles,
            cull: CullFaceMode::disabled(),
            blend: color.blend_function,
            color_mask: color.mask,
            depth: DepthState::from(&DepthMode::disabled()),
            stencil: StencilState::from(&StencilMode::disabled()),
        },
        targets: TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None },
    }
}

/// The clipping mask program's vertex layout: a position of two shorts.
fn position_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        stride: 4,
        step: VertexStep::Vertex,
        attributes: vec![VertexAttribute {
            location: const { programs::CLIPPING_MASK.location("pos") as u32 },
            format: AttributeDataType::Short2,
            offset: 0,
        }],
    }
}
