// HeadlessBackend on a device it shares: with other backends, with the host
// that made it, and on an adapter picked by index, as maplibre-native-ffi's
// offscreen device (tests/native/abi/render/offscreen.c) uses it.

use mln_gfx::backend_scope::{BackendScope, ScopeType};
use mln_gfx::headless_backend::HeadlessBackend as _;
use mln_gfx::render_pass::RenderPassDescriptor;
use mln_gfx::renderer_backend::{ContextMode, RendererBackend as _};
use mln_util::Color;
use mln_util::size::Size;
use mln_webgpu::HeadlessBackend;

const SIZE: Size = Size { width: 16, height: 8 };

/// Clears the backend's offscreen texture to `color` and reads it back.
fn clear_and_read(backend: &HeadlessBackend, color: Color) -> Vec<[u8; 4]> {
    let _scope = BackendScope::new(backend, ScopeType::Explicit);
    let context = backend.get_context();
    let renderable = backend.get_default_renderable();
    context.begin_frame();
    let encoder = context.create_command_encoder();
    let pass = encoder.create_render_pass(
        "main buffer",
        &RenderPassDescriptor { renderable, clear_color: Some(color), clear_depth: Some(1.0), clear_stencil: Some(0) },
    );
    drop(pass);
    encoder.present(renderable);
    context.end_frame();
    let image = backend.read_still_image();
    assert_eq!(image.size, SIZE);
    image.data.chunks_exact(4).map(|px| px.try_into().unwrap()).collect()
}

#[track_caller]
fn assert_all(pixels: &[[u8; 4]], expected: [u8; 4]) {
    assert!(pixels.iter().all(|px| *px == expected), "expected every pixel {expected:?}, got {:?}", pixels[0]);
}

const RED: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
const BLUE: Color = Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };

#[test]
fn backends_on_one_device_render_in_turn() {
    let (device, queue) = HeadlessBackend::request_device(None).expect("device");
    let first = HeadlessBackend::with_device(device.clone(), queue.clone(), SIZE, ContextMode::Unique);
    let second = HeadlessBackend::with_device(device.clone(), queue, SIZE, ContextMode::Unique);
    assert_eq!(first.get_device().as_ref(), Some(&device));
    assert_eq!(second.get_device().as_ref(), Some(&device));

    assert_all(&clear_and_read(&first, RED), [255, 0, 0, 255]);
    assert_all(&clear_and_read(&second, BLUE), [0, 0, 255, 255]);
    // Each keeps its own target.
    assert_all(&clear_and_read(&first, RED), [255, 0, 0, 255]);
}

#[test]
fn a_backend_renders_after_the_host_drops_its_device() {
    let (device, queue) = HeadlessBackend::request_device(None).expect("device");
    let backend = HeadlessBackend::with_device(device.clone(), queue.clone(), SIZE, ContextMode::Unique);
    drop((device, queue));
    assert_all(&clear_and_read(&backend, RED), [255, 0, 0, 255]);
}

#[test]
fn a_backend_renders_on_a_device_the_host_made() {
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
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("host device"),
        required_limits: wgpu::Limits::default().or_worse_values_from(&adapter.limits()),
        ..Default::default()
    }))
    .expect("device");

    let backend = HeadlessBackend::with_device(device.clone(), queue.clone(), SIZE, ContextMode::Unique);
    assert_eq!(backend.get_adapter_info(), Some(adapter.get_info()));
    assert_all(&clear_and_read(&backend, BLUE), [0, 0, 255, 255]);

    // The host's own work on the device still runs alongside.
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("host buffer"),
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, &[1, 2, 3, 4]);
    queue.submit([]);
    device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
}

#[test]
fn the_first_named_adapter_renders() {
    let (device, queue) = HeadlessBackend::request_device(Some(0)).expect("adapter 0");
    let backend = HeadlessBackend::with_device(device, queue, SIZE, ContextMode::Unique);
    assert_all(&clear_and_read(&backend, RED), [255, 0, 0, 255]);
}

#[test]
fn requesting_a_missing_adapter_fails() {
    let error = HeadlessBackend::request_device(Some(usize::MAX)).expect_err("no such adapter");
    assert!(error.contains("past the"), "{error}");
}
