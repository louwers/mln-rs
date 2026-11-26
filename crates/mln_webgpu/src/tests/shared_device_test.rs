// Backends on a device they share: with other backends, with the host that
// made it, and on an adapter picked by index.

use mln_gfx::frame::FrameTarget as _;
use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu as _, LoadOp, PassDesc};
use mln_gfx::{RendererBackend as _, Rgba, Size};

use crate::{Device, OffscreenTarget, RendererBackend};

const SIZE: Size = Size { width: 16, height: 8 };

/// A backend on `device`, and an offscreen target on it.
fn headless(device: &Device) -> (RendererBackend, OffscreenTarget) {
    (RendererBackend::new(device), OffscreenTarget::new(SIZE))
}

/// Clears the target to `color` and reads it back.
fn clear_and_read((backend, target): &mut (RendererBackend, OffscreenTarget), color: Rgba) -> Vec<[u8; 4]> {
    let gpu = backend.gpu();
    let image = target.begin_frame(gpu).expect("a frame");
    let pass = PassDesc {
        label: "main buffer",
        color: Some(ColorAttachment { texture: image.color, load: LoadOp::Clear(color) }),
        depth_stencil: image.depth_stencil.map(|texture| DepthStencilAttachment {
            texture,
            depth: LoadOp::Clear(1.0),
            stencil: LoadOp::Clear(0),
        }),
    };
    gpu.begin_pass(&pass).expect("a pass");
    gpu.end_pass().expect("the pass's end");
    gpu.flush(false).expect("a flush");
    target.end_frame(gpu);
    let image = target.read_image(gpu).expect("a frame");
    assert_eq!(image.size, SIZE);
    image.data.as_chunks::<4>().0.to_vec()
}

#[track_caller]
fn assert_all(pixels: &[[u8; 4]], expected: [u8; 4]) {
    assert!(pixels.iter().all(|px| *px == expected), "expected every pixel {expected:?}, got {:?}", pixels[0]);
}

const RED: Rgba = Rgba::new(1.0, 0.0, 0.0, 1.0);
const BLUE: Rgba = Rgba::new(0.0, 0.0, 1.0, 1.0);

#[test]
fn backends_on_one_device_render_in_turn() {
    let Some(device) = super::device() else {
        return;
    };
    let mut first = headless(&device);
    let mut second = headless(&device);
    assert_eq!(first.0.device().device(), device.device());
    assert_eq!(second.0.device().device(), device.device());

    assert_all(&clear_and_read(&mut first, RED), [255, 0, 0, 255]);
    assert_all(&clear_and_read(&mut second, BLUE), [0, 0, 255, 255]);
    // Each keeps its own target.
    assert_all(&clear_and_read(&mut first, RED), [255, 0, 0, 255]);
}

#[test]
fn a_backend_renders_after_the_host_drops_its_device() {
    let Some(device) = super::device() else {
        return;
    };
    let mut target = headless(&device);
    drop(device);
    assert_all(&clear_and_read(&mut target, RED), [255, 0, 0, 255]);
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
        ..wgpu::InstanceDescriptor::new_without_display_handle_from_env()
    });
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) {
        Ok(adapter) => adapter,
        Err(error) if !super::webgpu_requested() => {
            eprintln!("skipped: {error}");
            return;
        }
        Err(error) => panic!("adapter: {error}"),
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("host device"),
        required_limits: wgpu::Limits::default().or_worse_values_from(&adapter.limits()),
        ..Default::default()
    }))
    .expect("device");

    let mut target = headless(&Device::from_wgpu(Some(instance), device.clone(), queue.clone(), None));
    assert_eq!(target.0.device().adapter_info(), adapter.get_info());
    assert_all(&clear_and_read(&mut target, BLUE), [0, 0, 255, 255]);

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
    let device = match Device::new(Some(0)) {
        Ok(device) => device,
        Err(error)
            if matches!(error.kind, mln_gfx::ErrorKind::Unsupported | mln_gfx::ErrorKind::NotFound)
                && !super::webgpu_requested() =>
        {
            eprintln!("skipped: {error}");
            return;
        }
        Err(error) => panic!("adapter 0: {error}"),
    };
    let mut target = headless(&device);
    assert_all(&clear_and_read(&mut target, RED), [255, 0, 0, 255]);
}

#[test]
fn requesting_a_missing_adapter_fails() {
    let error = Device::new(Some(usize::MAX)).expect_err("no such adapter");
    assert_eq!(error.kind, mln_gfx::ErrorKind::NotFound, "{error}");
    assert!(error.message.contains("past the"), "{error}");
}
