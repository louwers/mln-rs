use glow::HasContext as _;
use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::conformance::{
    clip_mask_and_background, custom_layers, depth_only, foreign_ids, imported_textures, instanced_walls,
    invalid_calls, readbacks, topologies, transparent_replace, vertex_layouts,
};
use mln_gfx::gpu::{
    BindGroupDesc, BindGroupEntry, BindGroupId, BufferDesc, BufferId, BufferUsage, ColorAttachment, DepthState,
    DepthStencilAttachment, Gpu, GpuError, LoadOp, Pass, PipelineKey, PipelineState, RenderCommand, ShaderKey,
    StencilState, TargetFormats, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage, VertexAttribute,
    VertexBufferLayout, VertexStep,
};
use mln_gfx::shaders::background_layer_ubo::{BackgroundDrawableUnionUBO, BackgroundPropsUBO};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{
    ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, ID_BACKGROUND_PROPS_UBO, ID_PROJECTION_UBO,
};
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::DepthFunctionType;
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
    clip_mask_and_background(&mut gpu).expect("the frame");
}

#[test]
fn depth_only_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    depth_only(&mut gpu).expect("the frame");
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
    transparent_replace(&mut gpu).expect("the frame");
}

#[test]
fn topologies_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    topologies(&mut gpu).expect("the frames");
}

#[test]
fn readbacks_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    readbacks(&mut gpu).expect("the readbacks");
}

#[test]
fn vertex_layouts_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    vertex_layouts(&mut gpu).expect("the frames");
}

#[test]
fn instanced_walls_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    instanced_walls(&mut gpu).expect("the frame");
}

fn buffer(gpu: &mut GlGpu, usage: BufferUsage, data: &[u8]) -> BufferId {
    let buffer = gpu.create_buffer(&BufferDesc { label: "test", usage, size: data.len() as u64 }).unwrap();
    gpu.write_buffer(buffer, 0, data).unwrap();
    buffer
}

fn whole(id: usize, buffer: BufferId, data: &[u8]) -> BindGroupEntry {
    BindGroupEntry::Buffer { id: id as u32, buffer, offset: 0, size: data.len() as u64 }
}

/// The background shader, unblended, drawing triangles into `targets`.
fn background_key(targets: TargetFormats) -> PipelineKey {
    PipelineKey {
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
        targets,
    }
}

/// A quad over the whole target, and what the background shader reads to
/// draw it in `props`' color, at global index 0 of one element.
struct Background {
    quad: BufferId,
    indices: BufferId,
    props_buffer: BufferId,
    group: BindGroupId,
}

fn background(gpu: &mut GlGpu, props: &BackgroundPropsUBO) -> Background {
    let quad = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&[[-1i16, -1], [1, -1], [-1, 1], [1, 1]]));
    let indices = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&[0u16, 1, 2, 1, 2, 3]));
    let projection = [ProjectionUBO {
        matrix: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        ..ProjectionUBO::default()
    }];
    let drawable = [BackgroundDrawableUnionUBO::default()];
    let projection = bytemuck::cast_slice(&projection);
    let drawable = bytemuck::cast_slice(&drawable);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection);
    let drawable_buffer = buffer(gpu, BufferUsage::Storage, drawable);
    let props_buffer = buffer(gpu, BufferUsage::Uniform, bytemuck::bytes_of(props));
    let group = gpu
        .create_bind_group(&BindGroupDesc {
            label: "background layer",
            entries: &[
                whole(ID_PROJECTION_UBO, projection_buffer, projection),
                whole(ID_BACKGROUND_DRAWABLE_UBO, drawable_buffer, drawable),
                whole(ID_BACKGROUND_PROPS_UBO, props_buffer, bytemuck::bytes_of(props)),
            ],
        })
        .unwrap();
    Background { quad, indices, props_buffer, group }
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
    let mut key = background_key(TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None });
    let pipeline = gpu.create_pipeline(&key).unwrap();
    // A pipeline without its shader's attributes isn't made.
    key.state.vertex_buffers.clear();
    assert!(matches!(gpu.create_pipeline(&key), Err(GpuError::Invalid(_))));
    let red = BackgroundPropsUBO { color: [1.0, 0.0, 0.0, 1.0], opacity: 1.0, ..BackgroundPropsUBO::default() };
    let Background { quad, indices, props_buffer, group } = background(&mut gpu, &red);

    let draw_quad = RenderCommand::DrawIndexed { indices: 0..6, base_vertex: 0, instances: 0..1 };
    let pass = Pass {
        label: "frame".to_owned(),
        color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Color::black()) }),
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
    assert_eq!(gpu.read_texels(color, TextureRegion::whole(size)).unwrap()[..4], [255, 0, 0, 255]);

    let green = BackgroundPropsUBO { color: [0.0, 1.0, 0.0, 1.0], ..red };
    gpu.write_buffer(props_buffer, 0, bytemuck::bytes_of(&green)).unwrap();
    gpu.submit(std::slice::from_ref(&pass)).unwrap();
    assert_eq!(gpu.read_texels(color, TextureRegion::whole(size)).unwrap()[..4], [0, 255, 0, 255]);

    let mut past_the_end = pass.clone();
    past_the_end.commands.insert(5, RenderCommand::SetGlobalIndex(1));
    assert!(matches!(gpu.submit(&[past_the_end]), Err(GpuError::Invalid(_))));
    let mut before_the_start = pass.clone();
    before_the_start.commands.push(RenderCommand::DrawIndexed { indices: 0..6, base_vertex: -1, instances: 0..1 });
    assert!(matches!(gpu.submit(&[before_the_start]), Err(GpuError::Invalid(_))));
}

#[test]
fn custom_layers_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    custom_layers(
        &mut gpu,
        |lent| assert!(lent.is::<crate::CustomLayerEncoder>()),
        |lent| {
            let lent = lent.downcast_mut::<crate::CustomLayerPass>().expect("a GL pass");
            assert!(lent.framebuffer.is_some());
            // SAFETY: the lent context is current, and the query writes one
            // integer.
            let vertex_array = unsafe { glow::HasContext::get_parameter_i32(lent.gl(), glow::VERTEX_ARRAY_BINDING) };
            assert_eq!(vertex_array, 0, "the host starts with vertex array 0");
        },
    )
    .expect("the frames");
}

#[test]
fn imported_textures_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    imported_textures(&mut gpu, |gpu, texture| gpu.import_again(texture).ok_or(GpuError::UnknownId("texture")))
        .expect("the frames");
}

// A framebuffer made elsewhere is drawn into as it is, or one attachment of
// it alone, which leaves the other alone, read back, and left alive when its
// textures go; its color with another framebuffer's
// depth makes no framebuffer.
#[test]
fn imported_framebuffers_draw_as_they_are() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    let size = Size { width: 4, height: 4 };
    // SAFETY: the objects are new, of a size GL ES 3.0 takes.
    let (framebuffer, color, depth_stencil) = gpu.with_gl(|gl| unsafe {
        let color = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(color));
        gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, 4, 4);
        let depth_stencil = gl.create_renderbuffer().unwrap();
        gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth_stencil));
        gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH24_STENCIL8, 4, 4);
        let framebuffer = gl.create_framebuffer().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(color), 0);
        let depth = glow::DEPTH_STENCIL_ATTACHMENT;
        gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, depth, glow::RENDERBUFFER, Some(depth_stencil));
        assert_eq!(gl.check_framebuffer_status(glow::FRAMEBUFFER), glow::FRAMEBUFFER_COMPLETE);
        (framebuffer, color, depth_stencil)
    });
    let imported = gpu.import_framebuffer(
        Some(framebuffer),
        size,
        Some(TextureFormat::Rgba8Unorm),
        Some(TextureFormat::Depth24PlusStencil8),
    );
    let (color_id, depth_id) = (imported.color.unwrap(), imported.depth_stencil.unwrap());
    let red = Color::red();
    let pass = |color: Option<TextureId>, depth_stencil: Option<TextureId>| Pass {
        label: "imported".to_owned(),
        color: color.map(|texture| ColorAttachment { texture, load: LoadOp::Clear(red) }),
        depth_stencil: depth_stencil.map(|texture| DepthStencilAttachment {
            texture,
            depth: LoadOp::Clear(1.0),
            stencil: LoadOp::Clear(0),
        }),
        commands: Vec::new(),
    };
    // A green quad into the depth alone, with a pipeline that would write
    // color: the framebuffer's color keeps the clear.
    let green = BackgroundPropsUBO { color: [0.0, 1.0, 0.0, 1.0], opacity: 1.0, ..BackgroundPropsUBO::default() };
    let Background { quad, indices, group, .. } = background(&mut gpu, &green);
    let mut key =
        background_key(TargetFormats { color: None, depth_stencil: Some(TextureFormat::Depth24PlusStencil8) });
    key.state.depth = DepthState { compare: DepthFunctionType::Always, write: true };
    let depth_only = gpu.create_pipeline(&key).unwrap();
    let mut depth_pass = pass(None, Some(depth_id));
    depth_pass.commands = vec![
        RenderCommand::SetVertexBuffer { slot: 0, buffer: quad, offset: 0 },
        RenderCommand::SetIndexBuffer { buffer: indices, offset: 0 },
        RenderCommand::SetBindGroup { slot: 0, group },
        RenderCommand::SetPipeline(depth_only),
        RenderCommand::DrawIndexed { indices: 0..6, base_vertex: 0, instances: 0..1 },
    ];
    gpu.submit(&[pass(Some(color_id), Some(depth_id)), depth_pass]).unwrap();
    assert_eq!(gpu.read_texels(color_id, TextureRegion::whole(size)).unwrap(), [255, 0, 0, 255].repeat(16));
    let own_depth = gpu
        .create_texture(&TextureDesc {
            label: "depth",
            size,
            format: TextureFormat::Depth24PlusStencil8,
            usage: TextureUsage { render_target: true, ..TextureUsage::default() },
        })
        .unwrap();
    assert!(matches!(gpu.submit(&[pass(Some(color_id), Some(own_depth))]), Err(GpuError::Invalid(_))));
    gpu.destroy_texture(color_id);
    gpu.destroy_texture(depth_id);
    // SAFETY: the objects are the test's, which the backend forgot.
    gpu.with_gl(|gl| unsafe {
        assert!(gl.is_framebuffer(framebuffer), "the framebuffer stays");
        gl.delete_framebuffer(framebuffer);
        gl.delete_renderbuffer(depth_stencil);
        gl.delete_texture(color);
    });
}

// A fence that the backend makes signals once the commands before it are
// done, and a wait for one is the GPU's.
#[test]
fn fences_order_the_share_groups_commands() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    let fence = gpu.fence_sync().unwrap();
    // SAFETY: the fence is alive.
    let status =
        gpu.with_gl(|gl| unsafe { gl.client_wait_sync(fence, glow::SYNC_FLUSH_COMMANDS_BIT, u64::MAX as i32) });
    assert!(matches!(status, glow::ALREADY_SIGNALED | glow::CONDITION_SATISFIED));
    gpu.delete_sync(fence);
    let host = gpu.fence_sync().unwrap();
    gpu.wait_sync(host);
    gpu.finish().unwrap();
}
