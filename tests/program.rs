//! A program of a crate outside the engine draws through each backend's
//! `Gpu`, with nothing of the built-in layer types: a uniform block of its
//! own, an instance attribute in a buffer it rewrites in part, and a texture.

#![cfg(not(target_family = "wasm"))]

#[path = "program/quad.rs"]
mod quad;

use std::fmt::Display;

use mln_rs::mln_gfx::RendererBackend;
use mln_rs::mln_gfx::color_mode::{BlendFunction, Mask};
use mln_rs::mln_gfx::cull_face_mode::CullFaceMode;
use mln_rs::mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_rs::mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BufferDesc, BufferId, BufferUsage, ColorAttachment, DepthState, Draw, DynamicState,
    FixedState, GeometryDesc, Gpu, GpuError, LoadOp, PassDesc, PipelineDesc, Scissor, ShaderKey, StencilState,
    TargetFormats, TextureDesc, TextureFormat, TextureId, TextureRegion, TextureUsage, VertexAttribute,
    VertexBufferLayout, VertexStep, read_texels,
};
use mln_rs::mln_gfx::texture2d::SamplerState;
use mln_rs::mln_util::Color;
use mln_rs::mln_util::size::Size;
use quad::{ID_CORNER, ID_OFFSET, PROPS, QUAD, QuadUBO};

const SIZE: Size = Size { width: 32, height: 32 };
const BLACK: [u8; 4] = [0, 0, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
/// The image's blue, tinted.
const TINTED_BLUE: [u8; 4] = [0, 0, 153, 255];

/// A buffer of `usage` holding `data`.
fn buffer(gpu: &mut impl Gpu, usage: BufferUsage, data: &[u8]) -> Result<BufferId, GpuError> {
    let buffer = gpu.create_buffer(&BufferDesc { label: "quads", usage, size: data.len() as u64 })?;
    gpu.write_buffer(buffer, 0, data)?;
    Ok(buffer)
}

/// The texel at `x`, `y` of a target of [`SIZE`], counting rows from the top.
fn texel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE.width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Checks the texel at each of `expected`'s points, to within 1.
fn check(pixels: &[u8], expected: &[((u32, u32), [u8; 4])]) {
    for &((x, y), color) in expected {
        let actual = texel(pixels, x, y);
        let near = actual.iter().zip(color).all(|(&a, b)| a.abs_diff(b) <= 1);
        assert!(near, "({x}, {y}) is {actual:?}, not {color:?}");
    }
}

/// Draws `draw` over opaque black in `target`, and reads it back.
fn frame(gpu: &mut impl Gpu, target: TextureId, draw: &Draw) -> Result<Vec<u8>, GpuError> {
    let color = ColorAttachment { texture: target, load: LoadOp::Clear(Color::black()) };
    gpu.begin_pass(&PassDesc { label: "quads", color: Some(color), depth_stencil: None })?;
    gpu.draw(draw)?;
    gpu.end_pass()?;
    read_texels(gpu, target, TextureRegion::whole(SIZE))
}

/// Draws two quads over opaque black, each of the image's red left half
/// and its blue right half, tinted; then moves the second quad with a write
/// of its offset alone and draws again.
fn quads(gpu: &mut impl Gpu) -> Result<(), GpuError> {
    let target = gpu.create_texture(&TextureDesc {
        label: "target",
        size: SIZE,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() },
    })?;
    let image_size = Size { width: 2, height: 1 };
    let image = gpu.create_texture(&TextureDesc {
        label: "image",
        size: image_size,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsage { sampled: true, ..TextureUsage::default() },
    })?;
    gpu.write_texture(image, TextureRegion::whole(image_size), &[255, 0, 0, 255, 0, 0, 255, 255])?;
    let sampler = gpu.create_sampler(&SamplerState::default())?;

    let corners: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
    let corners = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&corners))?;
    let indices = buffer(gpu, BufferUsage::Index, bytemuck::cast_slice(&[0u16, 1, 2, 1, 3, 2]))?;
    // The bottom left and the top right quarters of clip space.
    let offsets: [[f32; 2]; 2] = [[-1.0, -1.0], [0.0, 0.0]];
    let offsets = buffer(gpu, BufferUsage::Vertex, bytemuck::cast_slice(&offsets))?;
    let block = QuadUBO { size: [1.0, 1.0], tint: [1.0, 1.0, 0.6, 1.0], ..QuadUBO::default() };
    let block = bytemuck::bytes_of(&block);
    let block_buffer = buffer(gpu, BufferUsage::Uniform, block)?;

    let shader = gpu.create_shader(&ShaderKey::new(&QUAD, []))?;
    let layout = |id, step| VertexBufferLayout {
        stride: 8,
        step,
        attributes: vec![VertexAttribute { id, format: AttributeDataType::Float2, offset: 0 }],
    };
    let layouts = [layout(ID_CORNER as u32, VertexStep::Vertex), layout(ID_OFFSET as u32, VertexStep::Instance)];
    let targets = TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None };
    let pipeline = gpu.create_pipeline(&PipelineDesc {
        shader,
        vertex_buffers: &layouts,
        state: FixedState {
            primitive: DrawModeType::Triangles,
            cull: CullFaceMode::disabled(),
            blend: BlendFunction::Replace,
            color_mask: Mask { r: true, g: true, b: true, a: true },
            depth: DepthState::DISABLED,
            stencil: StencilState::DISABLED,
        },
        targets,
    })?;
    let whole = |id, buffer, data: &[u8]| BindGroupEntry::Buffer { id, buffer, offset: 0, size: data.len() as u64 };
    let entries =
        [whole(PROPS.id as u32, block_buffer, block), BindGroupEntry::Texture { id: 0, texture: image, sampler }];
    let bindings = gpu.create_bindings(&BindingsDesc { label: "quads", shader, entries: &entries })?;
    let geometry = gpu.create_geometry(&GeometryDesc {
        shader,
        layouts: &layouts,
        vertex_buffers: &[(corners, 0), (offsets, 0)],
        index_buffer: indices,
    })?;
    let draw = Draw {
        pipeline,
        bindings,
        geometry,
        first_index: 0,
        index_count: 6,
        base_vertex: 0,
        first_instance: 0,
        instance_count: 2,
        global_index: 0,
        dynamic: DynamicState {
            depth_range: (0.0, 1.0),
            scissor: Scissor { x: 0, y: 0, width: SIZE.width, height: SIZE.height },
            stencil_reference: 0,
            blend_constant: Color::default(),
            line_width: 1.0,
        },
    };

    let (bottom, top) = (24, 8);
    let pixels = frame(gpu, target, &draw)?;
    check(&pixels, &[((4, bottom), RED), ((12, bottom), TINTED_BLUE), ((20, top), RED), ((28, top), TINTED_BLUE)]);
    check(&pixels, &[((4, top), BLACK), ((28, bottom), BLACK)]);

    // The second quad to the top left quarter.
    gpu.write_buffer(offsets, 8, bytemuck::cast_slice(&[-1.0f32, 0.0]))?;
    let pixels = frame(gpu, target, &draw)?;
    check(&pixels, &[((4, top), RED), ((12, top), TINTED_BLUE), ((4, bottom), RED), ((20, top), BLACK)]);
    Ok(())
}

/// Whether `MLN_RENDER_BACKEND` names `backend`, whose test then fails
/// rather than skips where it has no device.
fn named(backend: &str) -> bool {
    std::env::var("MLN_RENDER_BACKEND").is_ok_and(|named| named.eq_ignore_ascii_case(backend))
}

/// Draws the quads on `backend`, or skips where it has no device.
fn draw_on<B: RendererBackend>(name: &str, backend: Result<B, impl Display>) {
    match backend {
        Ok(mut backend) => quads(&mut *backend.gpu()).expect("the frames"),
        Err(error) if named(name) => panic!("no {name} device: {error}"),
        Err(error) => eprintln!("skipped: no {name} device: {error}"),
    }
}

#[cfg(feature = "webgpu")]
#[test]
fn a_program_of_its_own_draws_on_webgpu() {
    use mln_rs::mln_webgpu::{Device, RendererBackend};
    draw_on("webgpu", Device::new(None).map(|device| RendererBackend::new(&device)));
}

#[cfg(all(feature = "metal", target_vendor = "apple"))]
#[test]
fn a_program_of_its_own_draws_on_metal() {
    use mln_rs::mln_mtl::{Device, RendererBackend};
    draw_on("metal", Device::new(None).map(|device| RendererBackend::new(&device)));
}

#[cfg(all(feature = "vulkan", not(target_vendor = "apple")))]
#[test]
fn a_program_of_its_own_draws_on_vulkan() {
    use mln_rs::mln_vulkan::{Device, RendererBackend};
    draw_on("vulkan", Device::new(None).and_then(|device| RendererBackend::new(&device)));
}

#[cfg(all(feature = "d3d12", windows))]
#[test]
fn a_program_of_its_own_draws_on_d3d12() {
    use mln_rs::mln_d3d12::{Device, RendererBackend};
    let backend = Device::new(None).map_err(|error| error.to_string());
    draw_on("d3d12", backend.and_then(|device| RendererBackend::new(&device).map_err(|error| error.to_string())));
}

#[cfg(feature = "gl")]
#[test]
fn a_program_of_its_own_draws_on_gl() {
    use mln_rs::mln_gl::{Api, Device, DeviceOptions, RendererBackend};
    let (name, api) = if named("gl-desktop") { ("gl-desktop", Api::Gl) } else { ("gl", Api::default()) };
    let device = Device::with_options(DeviceOptions { api, ..DeviceOptions::default() });
    draw_on(name, device.and_then(|device| RendererBackend::new(&device)));
}
