use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::conformance::{
    clip_mask_and_background, depth_only, foreign_ids, invalid_calls, topologies, transparent_replace, vertex_layouts,
};
use mln_gfx::gpu::{
    BindGroupDesc, BindGroupEntry, BufferDesc, BufferId, BufferUsage, ColorAttachment, DepthState, Gpu, GpuError,
    LoadOp, Pass, PipelineKey, PipelineState, RenderCommand, ShaderKey, StencilState, TargetFormats, TextureDesc,
    TextureFormat, TextureUsage, VertexAttribute, VertexBufferLayout, VertexStep,
};
use mln_gfx::shaders::background_layer_ubo::{BackgroundDrawableUnionUBO, BackgroundPropsUBO};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{
    ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, ID_BACKGROUND_PROPS_UBO, ID_PROJECTION_UBO,
};
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_gfx::stencil_mode::StencilMode;
use mln_util::Color;
use mln_util::size::Size;

use crate::{Api, Device, DeviceOptions, GlGpu};

/// The API that `MLN_RENDER_BACKEND` asks for: desktop OpenGL for
/// `gl-desktop`, and otherwise the platform's.
fn api() -> Api {
    if knob().is_some_and(|backend| backend.eq_ignore_ascii_case("gl-desktop")) { Api::Gl } else { Api::default() }
}

fn knob() -> Option<String> {
    std::env::var("MLN_RENDER_BACKEND").ok()
}

/// A backend on a device of its own, or none where there is no EGL to make
/// a context of [`api`] with: the test is skipped there, unless
/// `MLN_RENDER_BACKEND` asks for this backend (`gl` or `gl-desktop`).
fn gpu() -> Option<GlGpu> {
    let gpu = Device::with_options(DeviceOptions { api: api(), ..DeviceOptions::default() })
        .and_then(|device| GlGpu::new(&device));
    match gpu {
        Ok(gpu) => Some(gpu),
        Err(error)
            if knob().is_some_and(|backend| {
                backend.eq_ignore_ascii_case("gl") || backend.eq_ignore_ascii_case("gl-desktop")
            }) =>
        {
            panic!("{error}")
        }
        Err(error) => {
            eprintln!("skipped: {error}");
            None
        }
    }
}

#[test]
fn clip_mask_and_background_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    clip_mask_and_background(&mut gpu, |gpu, texture| gpu.read_rgba8(texture).expect("readback")).expect("the frame");
}

#[test]
fn depth_only_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    depth_only(&mut gpu, |gpu, texture| gpu.read_rgba8(texture).expect("readback")).expect("the frame");
}

#[test]
fn foreign_ids_through_gpu() {
    let (Some(mut first), Some(mut second)) = (gpu(), gpu()) else {
        return;
    };
    foreign_ids(&mut first, &mut second).expect("the valid calls");
}

#[test]
fn invalid_calls_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    invalid_calls(&mut gpu).expect("the valid calls");
}

#[test]
fn transparent_replace_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    transparent_replace(&mut gpu, |gpu, texture| gpu.read_rgba8(texture).expect("readback")).expect("the frame");
}

#[test]
fn topologies_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    topologies(&mut gpu, |gpu, texture| gpu.read_rgba8(texture).expect("readback")).expect("the frames");
}

#[test]
fn vertex_layouts_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    vertex_layouts(&mut gpu, |gpu, texture| gpu.read_rgba8(texture).expect("readback")).expect("the frames");
}

fn buffer(gpu: &mut GlGpu, usage: BufferUsage, data: &[u8]) -> BufferId {
    let buffer = gpu.create_buffer(&BufferDesc { label: "test", usage, size: data.len() as u64 }).unwrap();
    gpu.write_buffer(buffer, 0, data).unwrap();
    buffer
}

fn whole(id: usize, buffer: BufferId, data: &[u8]) -> BindGroupEntry {
    BindGroupEntry::Buffer { id: id as u32, buffer, offset: 0, size: data.len() as u64 }
}

// A storage buffer is a uniform buffer of its elements, and a draw's base
// vertex moves its attribute pointers, so submit rejects a global index past
// the bound elements and a vertex before the buffer. A write after a submit
// reaches the next one.
#[test]
fn draws_read_only_what_is_bound() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    let size = Size { width: 4, height: 4 };
    let color = gpu
        .create_texture(&TextureDesc {
            label: "color",
            size,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() },
        })
        .unwrap();
    let mut key = PipelineKey {
        shader: ShaderKey::new(BuiltIn::BackgroundShader, []),
        state: PipelineState {
            vertex_buffers: vec![VertexBufferLayout {
                stride: 4,
                step: VertexStep::Vertex,
                attributes: vec![VertexAttribute {
                    id: ID_BACKGROUND_POS_VERTEX_ATTRIBUTE as u32,
                    format: AttributeDataType::Short2,
                    offset: 0,
                }],
            }],
            primitive: DrawModeType::Triangles,
            cull: CullFaceMode::disabled(),
            blend: ColorMode::unblended().blend_function,
            color_mask: ColorMode::unblended().mask,
            depth: DepthState::from(&DepthMode::disabled()),
            stencil: StencilState::from(&StencilMode::disabled()),
        },
        targets: TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None },
    };
    let pipeline = gpu.create_pipeline(&key).unwrap();
    // A pipeline without its shader's attributes isn't made.
    key.state.vertex_buffers.clear();
    assert!(matches!(gpu.create_pipeline(&key), Err(GpuError::Invalid(_))));

    // A quad over the whole target, at global index 0 of one element.
    let quad = buffer(&mut gpu, BufferUsage::Vertex, bytemuck::cast_slice(&[[-1i16, -1], [1, -1], [-1, 1], [1, 1]]));
    let indices = buffer(&mut gpu, BufferUsage::Index, bytemuck::cast_slice(&[0u16, 1, 2, 1, 2, 3]));
    let projection = [ProjectionUBO {
        matrix: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        ..ProjectionUBO::default()
    }];
    let drawable = [BackgroundDrawableUnionUBO::default()];
    let red = BackgroundPropsUBO { color: [1.0, 0.0, 0.0, 1.0], opacity: 1.0, ..BackgroundPropsUBO::default() };
    let projection = bytemuck::cast_slice(&projection);
    let drawable = bytemuck::cast_slice(&drawable);
    let projection_buffer = buffer(&mut gpu, BufferUsage::Storage, projection);
    let drawable_buffer = buffer(&mut gpu, BufferUsage::Storage, drawable);
    let props_buffer = buffer(&mut gpu, BufferUsage::Uniform, bytemuck::bytes_of(&red));
    let group = gpu
        .create_bind_group(&BindGroupDesc {
            label: "background layer",
            entries: &[
                whole(ID_PROJECTION_UBO, projection_buffer, projection),
                whole(ID_BACKGROUND_DRAWABLE_UBO, drawable_buffer, drawable),
                whole(ID_BACKGROUND_PROPS_UBO, props_buffer, bytemuck::bytes_of(&red)),
            ],
        })
        .unwrap();

    let draw_quad = RenderCommand::DrawIndexed { indices: 0..6, base_vertex: 0, instances: 0..1 };
    let pass = Pass {
        label: "frame".to_owned(),
        color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }) }),
        depth_stencil: None,
        commands: vec![
            RenderCommand::SetVertexBuffer { slot: 0, buffer: quad, offset: 0 },
            RenderCommand::SetIndexBuffer { buffer: indices, offset: 0 },
            RenderCommand::SetBindGroup { slot: 0, group },
            RenderCommand::SetPipeline(pipeline),
            // Clamped to the widths the context draws, which a triangle
            // ignores.
            RenderCommand::SetLineWidth(1000.0),
            draw_quad,
        ],
    };
    gpu.submit(std::slice::from_ref(&pass)).unwrap();
    assert_eq!(gpu.read_rgba8(color).unwrap()[..4], [255, 0, 0, 255]);

    let green = BackgroundPropsUBO { color: [0.0, 1.0, 0.0, 1.0], ..red };
    gpu.write_buffer(props_buffer, 0, bytemuck::bytes_of(&green)).unwrap();
    gpu.submit(std::slice::from_ref(&pass)).unwrap();
    assert_eq!(gpu.read_rgba8(color).unwrap()[..4], [0, 255, 0, 255]);

    let mut past_the_end = pass.clone();
    past_the_end.commands.insert(5, RenderCommand::SetGlobalIndex(1));
    assert!(matches!(gpu.submit(&[past_the_end]), Err(GpuError::Invalid(_))));
    let mut before_the_start = pass.clone();
    before_the_start.commands.push(RenderCommand::DrawIndexed { indices: 0..6, base_vertex: -1, instances: 0..1 });
    assert!(matches!(gpu.submit(&[before_the_start]), Err(GpuError::Invalid(_))));
}
