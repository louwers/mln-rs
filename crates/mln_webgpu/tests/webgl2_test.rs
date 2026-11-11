// The shader programs as a device without storage buffers gets them
// (src/webgl2.rs): every extracted WGSL source rewritten, and built on a
// device with WebGL2's limits.

use std::collections::HashMap;

use mln_gfx::backend_scope::{BackendScope, ScopeType};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::renderer_backend::{ContextMode, RendererBackend};
use mln_gfx::shader_group::GLOBE_DEFINE;
use mln_gfx::texture2d::Texture2D as _;
use mln_gfx::types::{TextureChannelDataType, TexturePixelType};
use mln_util::size::Size;
use mln_webgpu::HeadlessBackend;
use mln_webgpu::shaders::shader_program::ShaderProgram;
use mln_webgpu::shaders::shader_source::{WgslShaderSource, prelude};
use mln_webgpu::shaders::wgsl_preprocessor::preprocess_wgsl;
use mln_webgpu::shaders::wgsl_sources::ALL;
use mln_webgpu::webgl2::{self, MAX_ELEMENT_SIZE};

/// Every symbol tested by `#ifdef`/`#ifndef` in the source.
fn conditional_symbols(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let rest = line.strip_prefix("#ifdef").or_else(|| line.strip_prefix("#ifndef"))?;
            rest.split_whitespace().next().map(str::to_owned)
        })
        .collect()
}

/// The vertex and fragment sources of every program as the shader group
/// assembles them, in both projections and with every conditional defined.
fn programs() -> Vec<(&'static WgslShaderSource, String, String, String)> {
    let mut programs = Vec::new();
    for source in ALL {
        let (Some(vertex), Some(fragment)) = (source.vertex, source.fragment) else {
            continue; // Prelude
        };
        let assemble = |stage: &str| format!("{}\n{}{stage}", prelude(), source.prelude.unwrap_or(""));
        let (vertex, fragment) = (assemble(vertex), assemble(fragment));
        let base: HashMap<String, bool> = [("DEVICE_PIXEL_RATIO".to_owned(), true)].into_iter().collect();
        let mut globe = base.clone();
        globe.insert(GLOBE_DEFINE.to_owned(), true);
        let mut all = base.clone();
        for symbol in conditional_symbols(&vertex).into_iter().chain(conditional_symbols(&fragment)) {
            all.insert(symbol, true);
        }
        for (variant, defines) in [
            (format!("{:?}", ProjectionVariant::Mercator), base),
            (format!("{:?}", ProjectionVariant::Globe), globe),
            ("all conditionals".to_owned(), all),
        ] {
            programs.push((*source, variant, preprocess_wgsl(&vertex, &defines), preprocess_wgsl(&fragment, &defines)));
        }
    }
    programs
}

#[test]
fn every_program_is_rewritten_without_storage_buffers() {
    let mut windows = 0;
    for (source, variant, vertex, fragment) in programs() {
        for stage in [vertex, fragment] {
            let rewritten = webgl2::rewrite(&stage);
            let name = format!("{} ({variant})", source.built_in);
            assert!(!rewritten.source.contains("var<storage"), "{name} keeps a storage buffer");
            assert_eq!(
                rewritten.windows.len(),
                stage.matches("var<storage").count(),
                "{name}: a window for every storage buffer"
            );
            for window in &rewritten.windows {
                assert!(window.element_size.is_multiple_of(16) && window.element_size <= MAX_ELEMENT_SIZE);
            }
            windows += rewritten.windows.len();
            for ty in webgl2::uniform_types(&rewritten.source) {
                let size = webgl2::struct_size(&rewritten.source, ty);
                assert!(size.is_some_and(|size| size.is_multiple_of(16)), "{name}: uniform {ty} is {size:?} bytes");
            }
        }
    }
    assert!(windows > 0);
}

/// A device with WebGL2's limits on the adapter the tests render with.
fn webgl2_device() -> (wgpu::Device, wgpu::Queue) {
    let backends = wgpu::Backends::from_env().unwrap_or(if cfg!(target_os = "linux") {
        wgpu::Backends::VULKAN
    } else {
        wgpu::Backends::all()
    });
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("adapter");
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("WebGL2 limits"),
        required_limits: wgpu::Limits::downlevel_webgl2_defaults().or_worse_values_from(&adapter.limits()),
        ..Default::default()
    }))
    .expect("device")
}

/// Whether each program has both shader modules, and the validation errors
/// that making the programs raised.
fn build(backend: &HeadlessBackend, device: &wgpu::Device) -> (Vec<bool>, Option<wgpu::Error>) {
    let _scope = BackendScope::new(backend, ScopeType::Explicit);
    let context = backend.get_context().as_any().downcast_ref::<mln_webgpu::Context>().unwrap();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let built = programs()
        .iter()
        .map(|(source, _, vertex, fragment)| {
            ShaderProgram::new(context, vertex, fragment, &[], &[], source.name).has_shader_modules()
        })
        .collect();
    (built, pollster::block_on(scope.pop()))
}

#[test]
fn rewritten_programs_build_on_a_device_with_webgl2_limits() {
    let size = Size { width: 4, height: 4 };
    let (device, queue) = HeadlessBackend::request_device(None).expect("device");
    let full = HeadlessBackend::with_device(device.clone(), queue, size, ContextMode::Unique);
    let (expected, _) = build(&full, &device);

    let (device, queue) = webgl2_device();
    assert_eq!(device.limits().max_storage_buffers_per_shader_stage, 0);
    let limited = HeadlessBackend::with_device(device.clone(), queue, size, ContextMode::Unique);
    let (built, error) = build(&limited, &device);

    let names: Vec<String> =
        programs().iter().map(|(source, variant, ..)| format!("{} ({variant})", source.built_in)).collect();
    for ((name, expected), built) in names.iter().zip(expected).zip(built) {
        assert_eq!(built, expected, "{name}");
    }
    assert!(error.is_none(), "{}", error.unwrap());
}

/// The format and usage of a texture of `channel` that its owner uses as `usage`.
fn texture(
    backend: &HeadlessBackend,
    channel: TextureChannelDataType,
    usage: wgpu::TextureUsages,
) -> (wgpu::TextureFormat, wgpu::TextureUsages) {
    let _scope = BackendScope::new(backend, ScopeType::Explicit);
    let context = backend.get_context().as_any().downcast_ref::<mln_webgpu::Context>().unwrap();
    let texture = context.create_webgpu_texture_2d();
    let mut texture = texture.borrow_mut();
    texture.set_size(Size { width: 4, height: 4 });
    texture.set_format(TexturePixelType::RGBA, channel);
    texture.set_usage(usage);
    texture.create();
    let created = texture.get_texture().expect("texture");
    (created.format(), created.usage())
}

#[test]
fn float_textures_are_render_targets_only_where_webgl2_can_render_to_them() {
    use wgpu::{TextureFormat as F, TextureUsages as U};
    let size = Size { width: 4, height: 4 };
    let sampled = U::TEXTURE_BINDING | U::COPY_DST;
    let target = sampled | U::RENDER_ATTACHMENT;

    let (device, queue) = HeadlessBackend::request_device(None).expect("device");
    let full = HeadlessBackend::with_device(device, queue, size, ContextMode::Unique);
    assert_eq!(texture(&full, TextureChannelDataType::Float, sampled), (F::Rgba32Float, target));

    let (device, queue) = webgl2_device();
    let limited = HeadlessBackend::with_device(device, queue, size, ContextMode::Unique);
    assert_eq!(texture(&limited, TextureChannelDataType::Float, sampled), (F::Rgba32Float, sampled));
    assert_eq!(texture(&limited, TextureChannelDataType::UnsignedByte, sampled), (F::Rgba8Unorm, target));
    assert_eq!(texture(&limited, TextureChannelDataType::HalfFloat, target), (F::Rgba16Float, target));
    limited.webgpu_backend().backend_impl().set_half_float_renderable(false);
    assert_eq!(texture(&limited, TextureChannelDataType::HalfFloat, target), (F::Rgba8Unorm, target));
}
