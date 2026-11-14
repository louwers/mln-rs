// A frame drawn through `Gpu` alone: a clipping mask written to the stencil
// buffer as an ordinary draw, then the background shader tested against it.
// The mask covers the left half, so the background colors only that half.

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::gpu::{
    BindGroupDesc, BindGroupEntry, BufferDesc, BufferId, BufferUsage, ColorAttachment, DepthState,
    DepthStencilAttachment, Gpu, GpuError, LoadOp, Pass, PipelineKey, PipelineState, RenderCommand, ShaderKey,
    StencilState, TargetFormats, TextureDesc, TextureFormat, TextureUsage, VertexAttribute, VertexBufferLayout,
    VertexStep,
};
use mln_gfx::shaders::background_layer_ubo::{BackgroundDrawableUnionUBO, BackgroundPropsUBO};
use mln_gfx::shaders::clipping_mask_ubo::ClipUBO;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{
    ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE, ID_BACKGROUND_PROPS_UBO,
    ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE, ID_CLIPPING_MASK_UBO, ID_PROJECTION_UBO,
};
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_gfx::stencil_mode::{StencilMode, Test};
use mln_gfx::types::StencilOpType;
use mln_util::Color;
use mln_util::size::Size;

use crate::{Device, WebGpu};

const SIZE: Size = Size { width: 64, height: 32 };
const EXTENT: f32 = 8192.0;

/// A column-major matrix that maps tile x `0..EXTENT` to clip x
/// `-1..x_max` and tile y `0..EXTENT` to clip y `-1..1`.
fn tile_matrix(x_max: f32) -> [f32; 16] {
    let sx = (x_max + 1.0) / EXTENT;
    let sy = 2.0 / EXTENT;
    [sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0]
}

fn buffer(gpu: &mut WebGpu, usage: BufferUsage, data: &[u8]) -> BufferId {
    let buffer = gpu.create_buffer(&BufferDesc { label: "test", usage, size: data.len() as u64 }).unwrap();
    gpu.write_buffer(buffer, 0, data).unwrap();
    buffer
}

fn whole(id: usize, buffer: BufferId, data: &[u8]) -> BindGroupEntry {
    BindGroupEntry::Buffer { id: id as u32, buffer, offset: 0, size: data.len() as u64 }
}

/// The RGBA texels of `texture`.
fn read_back(device: &Device, texture: &wgpu::Texture) -> Vec<u8> {
    let (device, queue) = (device.device(), device.queue());
    let row = SIZE.width * 4;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded * SIZE.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(SIZE.height),
            },
        },
        wgpu::Extent3d { width: SIZE.width, height: SIZE.height, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    staging.slice(..).map_async(wgpu::MapMode::Read, |result| result.expect("mapped"));
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("polled");
    let mapped = staging.slice(..).get_mapped_range().expect("mapped range");
    mapped.chunks(padded as usize).flat_map(|line| &line[..row as usize]).copied().collect()
}

fn position_layout(id: usize) -> VertexBufferLayout {
    VertexBufferLayout {
        stride: 4,
        step: VertexStep::Vertex,
        attributes: vec![VertexAttribute { id: id as u32, format: AttributeDataType::Short2, offset: 0 }],
    }
}

#[test]
fn clip_mask_and_background_through_gpu() {
    let device = Device::new(None).expect("WebGPU device");
    let mut gpu = WebGpu::new(&device);

    let color_texture = device.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("color"),
        size: wgpu::Extent3d { width: SIZE.width, height: SIZE.height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color = gpu.import_texture(&color_texture);
    let depth_stencil = gpu
        .create_texture(&TextureDesc {
            label: "depth-stencil",
            size: SIZE,
            format: TextureFormat::Depth24PlusStencil8,
            usage: TextureUsage { render_target: true, ..TextureUsage::default() },
        })
        .unwrap();
    let targets =
        TargetFormats { color: TextureFormat::Rgba8Unorm, depth_stencil: Some(TextureFormat::Depth24PlusStencil8) };

    let e = EXTENT as i16;
    let quad: [[i16; 2]; 4] = [[0, 0], [e, 0], [0, e], [e, e]];
    let quad = buffer(&mut gpu, BufferUsage::Vertex, bytemuck::cast_slice(&quad));
    let indices = buffer(&mut gpu, BufferUsage::Index, bytemuck::cast_slice(&[0u16, 1, 2, 1, 2, 3]));

    // The mask: stencil 1 over the left half, no color.
    let disabled = ColorMode::disabled();
    let mask_pipeline = gpu
        .create_pipeline(&PipelineKey {
            shader: ShaderKey::new(BuiltIn::ClippingMaskProgram, []),
            state: PipelineState {
                vertex_buffers: vec![position_layout(ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE)],
                primitive: DrawModeType::Triangles,
                cull: CullFaceMode::disabled(),
                blend: disabled.blend_function,
                color_mask: disabled.mask,
                depth: DepthState::from(&DepthMode::disabled()),
                stencil: StencilState {
                    test: Test::Always,
                    write_mask: 0xFF,
                    fail: StencilOpType::Keep,
                    depth_fail: StencilOpType::Keep,
                    pass: StencilOpType::Replace,
                },
            },
            targets,
        })
        .unwrap();
    let clip = ClipUBO { matrix: tile_matrix(0.0), stencil_ref: 1, ..ClipUBO::default() };
    let clip_buffer = buffer(&mut gpu, BufferUsage::Uniform, bytemuck::bytes_of(&clip));
    let mask_group = gpu
        .create_bind_group(&BindGroupDesc {
            label: "clip",
            entries: &[whole(ID_CLIPPING_MASK_UBO, clip_buffer, bytemuck::bytes_of(&clip))],
        })
        .unwrap();

    // The background, where the stencil is 1. Its layer's entries are at
    // global index 1.
    let unblended = ColorMode::unblended();
    let background_pipeline = gpu
        .create_pipeline(&PipelineKey {
            shader: ShaderKey::new(BuiltIn::BackgroundShader, []),
            state: PipelineState {
                vertex_buffers: vec![position_layout(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE)],
                primitive: DrawModeType::Triangles,
                cull: CullFaceMode::disabled(),
                blend: unblended.blend_function,
                color_mask: unblended.mask,
                depth: DepthState::from(&DepthMode::disabled()),
                stencil: StencilState::from(&StencilMode {
                    test: Test::Equal { mask: 0xFF },
                    ref_: 0,
                    mask: 0,
                    fail: StencilOpType::Keep,
                    depth_fail: StencilOpType::Keep,
                    pass: StencilOpType::Keep,
                }),
            },
            targets,
        })
        .unwrap();
    let projections =
        [ProjectionUBO::default(), ProjectionUBO { matrix: tile_matrix(1.0), ..ProjectionUBO::default() }];
    let drawables = [BackgroundDrawableUnionUBO::default(); 2];
    let props = BackgroundPropsUBO { color: [1.0, 0.0, 0.0, 1.0], opacity: 1.0, ..BackgroundPropsUBO::default() };
    let projections = bytemuck::cast_slice(&projections);
    let drawables = bytemuck::cast_slice(&drawables);
    let props = bytemuck::bytes_of(&props);
    let projection_buffer = buffer(&mut gpu, BufferUsage::Storage, projections);
    let drawable_buffer = buffer(&mut gpu, BufferUsage::Storage, drawables);
    let props_buffer = buffer(&mut gpu, BufferUsage::Uniform, props);
    let layer_group = gpu
        .create_bind_group(&BindGroupDesc {
            label: "background layer",
            entries: &[
                whole(ID_PROJECTION_UBO, projection_buffer, projections),
                whole(ID_BACKGROUND_DRAWABLE_UBO, drawable_buffer, drawables),
                whole(ID_BACKGROUND_PROPS_UBO, props_buffer, props),
            ],
        })
        .unwrap();

    let draw_quad = RenderCommand::DrawIndexed { indices: 0..6, base_vertex: 0, instances: 0..1 };
    let pass = Pass {
        label: "frame".to_owned(),
        color: Some(ColorAttachment { texture: color, load: LoadOp::Clear(Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }) }),
        depth_stencil: Some(DepthStencilAttachment {
            texture: depth_stencil,
            depth: LoadOp::Clear(1.0),
            stencil: LoadOp::Clear(0),
        }),
        commands: vec![
            RenderCommand::SetVertexBuffer { slot: 0, buffer: quad, offset: 0 },
            RenderCommand::SetIndexBuffer { buffer: indices, offset: 0 },
            RenderCommand::SetPipeline(mask_pipeline),
            RenderCommand::SetBindGroup { slot: 0, group: mask_group },
            RenderCommand::SetStencilReference(1),
            draw_quad.clone(),
            RenderCommand::SetPipeline(background_pipeline),
            RenderCommand::SetBindGroup { slot: 0, group: layer_group },
            RenderCommand::SetGlobalIndex(1),
            draw_quad.clone(),
        ],
    };
    gpu.submit(std::slice::from_ref(&pass)).unwrap();

    let pixels = read_back(&device, &color_texture);
    let pixel = |x: u32, y: u32| {
        let i = ((y * SIZE.width + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    for y in [0, SIZE.height / 2, SIZE.height - 1] {
        assert_eq!(pixel(0, y), [255, 0, 0, 255], "the mask's half has the background");
        assert_eq!(pixel(SIZE.width / 2 - 1, y), [255, 0, 0, 255]);
        assert_eq!(pixel(SIZE.width / 2, y), [0, 0, 0, 255], "the other half keeps the clear color");
        assert_eq!(pixel(SIZE.width - 1, y), [0, 0, 0, 255]);
    }

    // A draw without its shader's buffers, and a pass naming a destroyed
    // buffer, submit nothing.
    let mut unbound = pass.clone();
    unbound
        .commands
        .retain(|command| !matches!(command, RenderCommand::SetBindGroup { group, .. } if *group == layer_group));
    assert!(matches!(gpu.submit(&[unbound]), Err(GpuError::Invalid(_))));
    gpu.destroy_buffer(quad);
    assert!(matches!(gpu.submit(&[pass]), Err(GpuError::UnknownId("buffer"))));
    assert!(matches!(gpu.write_buffer(quad, 0, &[0; 4]), Err(GpuError::UnknownId("buffer"))));
}
