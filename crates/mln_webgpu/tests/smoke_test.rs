// Smoke test for the headless wgpu setup that src/mln/webgpu/headless_backend.cpp
// relies on: a device on the platform's backend (Vulkan/lavapipe on Linux,
// Metal on macOS; `WGPU_BACKEND` overrides, as in HeadlessBackend), an
// RGBA8Unorm offscreen texture with RenderAttachment | CopySrc, a clear, and
// a texture-to-buffer readback.

#[test]
fn clear_offscreen_texture_and_read_back() {
    let backends = wgpu::Backends::from_env().unwrap_or(if cfg!(target_os = "linux") {
        wgpu::Backends::VULKAN
    } else {
        wgpu::Backends::all()
    });
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("no adapter (on Linux check LD_LIBRARY_PATH and VK_DRIVER_FILES)");
    let info = adapter.get_info();
    eprintln!("adapter: {} ({:?}, {:?})", info.name, info.backend, info.device_type);

    let (device, queue) = pollster::block_on(
        adapter.request_device(&wgpu::DeviceDescriptor { label: Some("smoke test device"), ..Default::default() }),
    )
    .expect("request_device failed");

    let (width, height) = (64u32, 32u32);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("smoke color"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    // Rows must be padded to COPY_BYTES_PER_ROW_ALIGNMENT (256) for the copy.
    let unpadded = width * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("smoke readback"),
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("smoke clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);

    let (tx, rx) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |r| tx.send(r).unwrap());
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("poll failed");
    rx.recv().unwrap().expect("map_async failed");

    let mapped = readback.get_mapped_range(..).expect("get_mapped_range failed");
    for y in 0..height as usize {
        let row = &mapped[y * padded as usize..][..unpadded as usize];
        for px in row.as_chunks::<4>().0 {
            assert_eq!(px, &[255, 0, 0, 255], "row {y}");
        }
    }
}
