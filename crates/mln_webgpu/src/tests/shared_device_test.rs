// Backends on a device they share: with other backends, with the host that
// made it, and on an adapter picked by index, as maplibre-native-ffi's
// offscreen device (tests/native/abi/render/offscreen.c) uses it.

use std::rc::Rc;

use mln_gfx::backend_scope::{BackendScope, ScopeType};
use mln_gfx::frame::FrameTarget as _;
use mln_gfx::render_pass::RenderPassDescriptor;
use mln_gfx::renderer_backend::RendererBackend as _;
use mln_util::Color;
use mln_util::size::Size;

use crate::{Device, OffscreenTarget, RendererBackend};

const SIZE: Size = Size { width: 16, height: 8 };

/// A backend on `device`, and an offscreen target on it.
fn headless(device: &Device) -> OffscreenTarget {
    OffscreenTarget::new(Rc::new(RendererBackend::new(device)), SIZE)
}

/// Clears the target to `color` and reads it back.
fn clear_and_read(target: &OffscreenTarget, color: Color) -> Vec<[u8; 4]> {
    let backend = target.backend();
    let _scope = BackendScope::new(&**backend, ScopeType::Explicit);
    let context = backend.get_context();
    let image = target.begin_frame().expect("a frame");
    context.begin_frame();
    let encoder = context.create_command_encoder();
    let pass = encoder.create_render_pass(
        "main buffer",
        &RenderPassDescriptor {
            target: &*image,
            clear_color: Some(color),
            clear_depth: Some(1.0),
            clear_stencil: Some(0),
        },
    );
    drop(pass);
    target.end_frame(&*encoder);
    context.end_frame();
    let image = target.read_image().expect("a frame");
    assert_eq!(image.size, SIZE);
    image.data.as_chunks::<4>().0.to_vec()
}

#[track_caller]
fn assert_all(pixels: &[[u8; 4]], expected: [u8; 4]) {
    assert!(pixels.iter().all(|px| *px == expected), "expected every pixel {expected:?}, got {:?}", pixels[0]);
}

const RED: Color = Color::red();
const BLUE: Color = Color::blue();

#[test]
fn backends_on_one_device_render_in_turn() {
    let device = Device::new(None).expect("device");
    let first = headless(&device);
    let second = headless(&device);
    assert_eq!(first.backend().device().device(), device.device());
    assert_eq!(second.backend().device().device(), device.device());

    assert_all(&clear_and_read(&first, RED), [255, 0, 0, 255]);
    assert_all(&clear_and_read(&second, BLUE), [0, 0, 255, 255]);
    // Each keeps its own target.
    assert_all(&clear_and_read(&first, RED), [255, 0, 0, 255]);
}

#[test]
fn a_backend_renders_after_the_host_drops_its_device() {
    let device = Device::new(None).expect("device");
    let target = headless(&device);
    drop(device);
    assert_all(&clear_and_read(&target, RED), [255, 0, 0, 255]);
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

    let target = headless(&Device::from_wgpu(Some(instance), device.clone(), queue.clone(), None));
    assert_eq!(target.backend().device().adapter_info(), adapter.get_info());
    assert_all(&clear_and_read(&target, BLUE), [0, 0, 255, 255]);

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
    let target = headless(&Device::new(Some(0)).expect("adapter 0"));
    assert_all(&clear_and_read(&target, RED), [255, 0, 0, 255]);
}

#[test]
fn requesting_a_missing_adapter_fails() {
    let error = Device::new(Some(usize::MAX)).expect_err("no such adapter");
    assert_eq!(error.kind, mln_util::ErrorKind::NotFound, "{error}");
    assert!(error.message.contains("past the"), "{error}");
}
