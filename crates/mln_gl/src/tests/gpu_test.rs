use glow::HasContext as _;
use mln_builtins::conformance::{
    clip_mask_and_background, custom_layers, depth_only, drawn_vertex_texels, foreign_ids, imported_textures,
    instanced_walls, invalid_calls, mipmapped_targets, pulled_walls, readbacks, resources_in_use, terrain_heights,
    topologies, transparent_replace, vertex_layouts, wall_quads, written_vertex_texels,
};
use mln_builtins::layers::background::programs::BACKGROUND;
use mln_builtins::layers::background::ubo::{BackgroundDrawableUnionUBO, BackgroundPropsUBO};
use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferDesc, BufferId, BufferUsage, ColorAttachment, DepthState,
    DepthStencilAttachment, Draw, DynamicState, FixedState, GeometryDesc, GeometryId, Gpu, GpuError, LoadOp, PassDesc,
    PipelineDesc, PipelineId, ResourceId, Scissor, ShaderId, ShaderKey, StencilState, TargetFormats, TextureDesc,
    TextureFormat, TextureId, TextureRegion, TextureUsage, VertexAttribute, VertexBufferLayout, VertexStep,
    read_texels,
};
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::stencil_mode::StencilMode;
use mln_gfx::types::DepthFunctionType;
use mln_gfx::{Rgba, Size};

use crate::gpu::GlGpu;
use crate::{Api, Device, DeviceOptions};

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
    with_device(GlGpu::new)
}

/// What `make` makes on a device of its own, or none where there is no EGL
/// to make a context of [`api`] with, as for [`gpu`].
pub(super) fn with_device<T>(make: impl FnOnce(&Device) -> Result<T, mln_gfx::Error>) -> Option<T> {
    let made =
        Device::with_options(DeviceOptions { api: api(), ..DeviceOptions::default() }).and_then(|device| make(&device));
    match made {
        Ok(made) => Some(made),
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
fn written_vertex_texels_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    written_vertex_texels(&mut gpu).expect("the frame");
}

#[test]
fn drawn_vertex_texels_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    drawn_vertex_texels(&mut gpu).expect("the frames");
}

#[test]
fn terrain_heights_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    terrain_heights(&mut gpu).expect("the frames");
}

#[test]
fn mipmapped_targets_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    mipmapped_targets(&mut gpu).expect("the frames");
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
fn resources_in_use_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    resources_in_use(&mut gpu).expect("the frames");
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

#[test]
fn wall_quads_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    wall_quads(&mut gpu).expect("the frame");
}

#[test]
fn pulled_walls_through_gpu() {
    let Some(mut gpu) = gpu() else {
        return;
    };
    pulled_walls(&mut gpu).expect("the bindings");
}

fn buffer(gpu: &mut GlGpu, usage: BufferUsage, data: &[u8]) -> BufferId {
    let buffer = gpu.create_buffer(&BufferDesc { label: "test", usage, size: data.len() as u64 }).unwrap();
    gpu.write_buffer(buffer, 0, data).unwrap();
    buffer
}

fn whole(id: usize, buffer: BufferId, data: &[u8]) -> BindGroupEntry {
    BindGroupEntry::Buffer { id: id as u32, buffer, offset: 0, size: data.len() as u64 }
}

/// The background shader's vertex buffer layout.
fn layouts() -> [VertexBufferLayout; 1] {
    [VertexBufferLayout {
        stride: 4,
        step: VertexStep::Vertex,
        attributes: vec![VertexAttribute {
            location: const { BACKGROUND.location("pos") as u32 },
            format: AttributeDataType::Short2,
            offset: 0,
        }],
    }]
}

/// The state of the background shader, unblended, drawing triangles.
fn background_state() -> FixedState {
    FixedState {
        primitive: DrawModeType::Triangles,
        cull: CullFaceMode::disabled(),
        blend: ColorMode::unblended().blend_function,
        color_mask: ColorMode::unblended().mask,
        depth: DepthState::from(&DepthMode::disabled()),
        stencil: StencilState::from(&StencilMode::disabled()),
    }
}

/// A quad over the whole target, and what the background shader reads to
/// draw it in `props`' color, at global index 0 of one element.
struct Background {
    shader: ShaderId,
    props_buffer: BufferId,
    bindings: BindingsId,
    geometry: GeometryId,
}

impl Background {
    /// A draw of the quad with `pipeline`.
    fn draw(&self, pipeline: PipelineId, size: Size) -> Draw {
        Draw {
            pipeline,
            bindings: self.bindings,
            geometry: self.geometry,
            first_index: 0,
            index_count: 6,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
            global_index: 0,
            dynamic: DynamicState {
                depth_range: (0.0, 1.0),
                scissor: Scissor { x: 0, y: 0, width: size.width, height: size.height },
                stencil_reference: 0,
                blend_constant: Rgba::new(0.0, 0.0, 0.0, 1.0),
                line_width: 1.0,
            },
        }
    }
}

fn background(gpu: &mut GlGpu, props: &BackgroundPropsUBO) -> Background {
    let shader = gpu.create_shader(&ShaderKey::new(&BACKGROUND, [])).unwrap();
    let quad = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&[[-1i16, -1], [1, -1], [-1, 1], [1, 1]]));
    let indices = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&[0u16, 1, 2, 1, 2, 3]));
    let projection = [ProjectionUBO {
        matrix: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        ..ProjectionUBO::default()
    }];
    let drawable = [<BackgroundDrawableUnionUBO as bytemuck::Zeroable>::zeroed()];
    let projection = bytemuck::cast_slice(&projection);
    let drawable = bytemuck::cast_slice(&drawable);
    let projection_buffer = buffer(gpu, BufferUsage::Storage, projection);
    let drawable_buffer = buffer(gpu, BufferUsage::Storage, drawable);
    let props_buffer = buffer(gpu, BufferUsage::Uniform, bytemuck::bytes_of(props));
    let bindings = gpu
        .create_bindings(&BindingsDesc {
            label: "background layer",
            shader,
            entries: &[
                whole(PROJECTION_ID, projection_buffer, projection),
                whole(const { BACKGROUND.buffer_id("drawableVector") }, drawable_buffer, drawable),
                whole(const { BACKGROUND.buffer_id("props") }, props_buffer, bytemuck::bytes_of(props)),
            ],
        })
        .unwrap();
    let geometry = gpu
        .create_geometry(&GeometryDesc {
            shader,
            layouts: &layouts(),
            vertex_buffers: &[(quad, 0)],
            index_buffer: indices,
        })
        .unwrap();
    Background { shader, props_buffer, bindings, geometry }
}

// A storage buffer is a uniform buffer of its elements, and a draw's base
// vertex moves its attribute pointers, so a draw at a global index past the
// bound elements, or from a vertex before the buffer, fails. A write after a
// flush reaches the next draws.
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
            mipmapped: false,
        })
        .unwrap();
    let red = BackgroundPropsUBO { color: [1.0, 0.0, 0.0, 1.0], opacity: 1.0, ..BackgroundPropsUBO::default() };
    let background = background(&mut gpu, &red);
    let targets = TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None };
    let layouts = layouts();
    let desc = PipelineDesc { shader: background.shader, vertex_buffers: &layouts, state: background_state(), targets };
    let pipeline = gpu.create_pipeline(&desc).unwrap();
    // A pipeline without its shader's attributes isn't made.
    assert!(matches!(gpu.create_pipeline(&PipelineDesc { vertex_buffers: &[], ..desc }), Err(GpuError::Invalid(_))));

    let pass = PassDesc {
        label: "frame",
        color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Rgba::new(0.0, 0.0, 0.0, 1.0)) }),
        depth_stencil: None,
    };
    let frame = |gpu: &mut GlGpu, draw: Draw| -> Result<(), GpuError> {
        gpu.begin_pass(&pass)?;
        let drawn = gpu.draw(&draw);
        gpu.end_pass()?;
        drawn
    };
    // Clamped to the widths the context draws, which a triangle ignores.
    let mut draw = background.draw(pipeline, size);
    draw.dynamic.line_width = 1000.0;
    frame(&mut gpu, draw).unwrap();
    assert_eq!(read_texels(&mut gpu, color, TextureRegion::whole(size)).unwrap()[..4], [255, 0, 0, 255]);

    let green = BackgroundPropsUBO { color: [0.0, 1.0, 0.0, 1.0], ..red };
    gpu.write_buffer(background.props_buffer, 0, bytemuck::bytes_of(&green)).unwrap();
    frame(&mut gpu, draw).unwrap();
    assert_eq!(read_texels(&mut gpu, color, TextureRegion::whole(size)).unwrap()[..4], [0, 255, 0, 255]);

    assert!(matches!(frame(&mut gpu, Draw { global_index: 1, ..draw }), Err(GpuError::Invalid(_))));
    assert!(matches!(frame(&mut gpu, Draw { base_vertex: -1, ..draw }), Err(GpuError::Invalid(_))));
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
    let red = Rgba::new(1.0, 0.0, 0.0, 1.0);
    let pass = |color: Option<TextureId>, depth_stencil: Option<TextureId>| PassDesc {
        label: "imported",
        color: color.map(|texture| ColorAttachment { texture, load: LoadOp::Clear(red) }),
        depth_stencil: depth_stencil.map(|texture| DepthStencilAttachment {
            texture,
            depth: LoadOp::Clear(1.0),
            stencil: LoadOp::Clear(0),
        }),
    };
    // A green quad into the depth alone, with a pipeline that would write
    // color: the framebuffer's color keeps the clear.
    let green = BackgroundPropsUBO { color: [0.0, 1.0, 0.0, 1.0], opacity: 1.0, ..BackgroundPropsUBO::default() };
    let background = background(&mut gpu, &green);
    let state =
        FixedState { depth: DepthState { compare: DepthFunctionType::Always, write: true }, ..background_state() };
    let targets = TargetFormats { color: None, depth_stencil: Some(TextureFormat::Depth24PlusStencil8) };
    let layouts = layouts();
    let depth_only = gpu
        .create_pipeline(&PipelineDesc { shader: background.shader, vertex_buffers: &layouts, state, targets })
        .unwrap();
    gpu.begin_pass(&pass(Some(color_id), Some(depth_id))).unwrap();
    gpu.end_pass().unwrap();
    gpu.begin_pass(&pass(None, Some(depth_id))).unwrap();
    gpu.draw(&background.draw(depth_only, size)).unwrap();
    gpu.end_pass().unwrap();
    assert_eq!(read_texels(&mut gpu, color_id, TextureRegion::whole(size)).unwrap(), [255, 0, 0, 255].repeat(16));
    let own_depth = gpu
        .create_texture(&TextureDesc {
            label: "depth",
            size,
            format: TextureFormat::Depth24PlusStencil8,
            usage: TextureUsage { render_target: true, ..TextureUsage::default() },
            mipmapped: false,
        })
        .unwrap();
    assert!(matches!(gpu.begin_pass(&pass(Some(color_id), Some(own_depth))), Err(GpuError::Invalid(_))));
    gpu.destroy(ResourceId::Texture(color_id)).expect("the destroy");
    gpu.destroy(ResourceId::Texture(depth_id)).expect("the destroy");
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
    gpu.flush(true).unwrap();
}
