// wgpu vs. Dawn/wgpu-native: the readback maps the staging buffer and blocks
// with `Device::poll(PollType::Wait)` (C++: `wgpuInstanceWaitAny` on Dawn, a
// 1000 x 1 ms `wgpuDevicePoll` loop on wgpu-native).
//
// Port-only: the staging buffer is kept between reads, and after a read the
// next frame's own submission ends with the copy, which C++ submits
// separately on each read.
//
// There is no size-changed callback (`handleResize` / `setSizeListener`)
// that resizes the textures when someone calls `setSize` on the color
// texture.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use mln_gfx::command_encoder::CommandEncoder as GfxCommandEncoder;
use mln_gfx::texture2d::{SamplerState, Texture2D as _, Texture2DPtr};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_gfx::{frame, offscreen_texture};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::time::Instant;

use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::draw_target::{DrawTarget, TargetResource};
use crate::texture2d::Texture2D;

const BYTES_PER_ROW_ALIGNMENT: u32 = 256;

/// How long a readback waits again after waits that reported a timeout.
const WAIT_AGAIN_LIMIT: Duration = Duration::from_secs(1);

fn linear_clamp() -> SamplerState {
    SamplerState {
        filter: TextureFilterType::Linear,
        wrap_u: TextureWrapType::Clamp,
        wrap_v: TextureWrapType::Clamp,
        ..SamplerState::default()
    }
}

pub struct OffscreenTextureResource {
    context: Rc<Context>,
    size: Size,
    type_: TextureChannelDataType,
    color_texture: RefCell<Option<Rc<RefCell<Texture2D>>>>,
    depth_texture: Option<Rc<RefCell<Texture2D>>>,
    /// The buffer the color texture is copied into to be read back, kept
    /// for the next read.
    staging_buffer: RefCell<Option<wgpu::Buffer>>,
    /// The color texture was read back: the next frame copies it out.
    read_back: Cell<bool>,
    /// The submission (`Context::submissions`) whose copy the readback
    /// buffer holds.
    copied_at: Cell<Option<u64>>,
}

impl OffscreenTextureResource {
    pub fn new(context: Rc<Context>, size: Size, type_: TextureChannelDataType, depth: bool, stencil: bool) -> Self {
        debug_assert!(size.width != 0 && size.height != 0);
        let mut res = OffscreenTextureResource {
            context,
            size,
            type_,
            color_texture: RefCell::new(None),
            depth_texture: None,
            staging_buffer: RefCell::default(),
            read_back: Cell::default(),
            copied_at: Cell::default(),
        };
        // Port-only: the C++ WebGPU backend only decrements this count.
        res.context.rendering_stats_mut().num_frame_buffers += 1;
        // Create color texture
        *res.color_texture.borrow_mut() = Some(res.create_color_texture());

        // Create depth/stencil texture if needed. WebGPU requires a combined
        // depth-stencil texture when both are needed.
        if depth || stencil {
            let depth_texture = res.context.create_webgpu_texture_2d();
            {
                let mut t = depth_texture.borrow_mut();
                t.set_size(size);
                // Use combined depth-stencil format when both are needed
                if depth && stencil {
                    t.set_format(TexturePixelType::Stencil, TextureChannelDataType::UnsignedByte);
                } else if depth {
                    t.set_format(TexturePixelType::Depth, TextureChannelDataType::Float);
                } else {
                    t.set_format(TexturePixelType::Stencil, TextureChannelDataType::UnsignedByte);
                }
                t.set_usage(wgpu::TextureUsages::RENDER_ATTACHMENT);
                t.set_sampler_configuration(linear_clamp());
            }
            res.depth_texture = Some(depth_texture);
        }
        res
    }

    fn create_color_texture(&self) -> Rc<RefCell<Texture2D>> {
        let texture = self.context.create_webgpu_texture_2d();
        {
            let mut t = texture.borrow_mut();
            t.set_size(self.size);
            t.set_format(TexturePixelType::RGBA, self.type_);
            t.set_usage(
                wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            t.set_sampler_configuration(linear_clamp());
        }
        texture
    }

    fn color(&self) -> Option<Rc<RefCell<Texture2D>>> {
        self.color_texture.borrow().clone()
    }

    fn row_strides(&self) -> (u32, u32) {
        let row_stride = self.size.width * 4; // RGBA8 output
        (row_stride, row_stride.next_multiple_of(BYTES_PER_ROW_ALIGNMENT))
    }

    /// Records a copy of the color texture into the readback buffer, if
    /// the texture exists.
    fn encode_copy(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) -> bool {
        let color = self.color();
        let Some(texture) = color.as_ref().and_then(|c| c.borrow().get_texture().cloned()) else {
            return false;
        };
        let size = self.size;
        let (_, aligned_row_stride) = self.row_strides();
        let mut staging = self.staging_buffer.borrow_mut();
        let staging = staging.get_or_insert_with(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Readback Buffer"),
                size: u64::from(aligned_row_stride) * u64::from(size.height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(aligned_row_stride),
                    rows_per_image: Some(size.height),
                },
            },
            wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
        );
        true
    }

    /// Ends a frame with the copy a read asked for: a host reading frames
    /// back reads the next one too, and the copy then costs no submission
    /// of its own.
    pub(crate) fn encode_requested_copy(&self, encoder: &mut wgpu::CommandEncoder) {
        let device = self.context.get_backend().device();
        if self.read_back.replace(false) && self.encode_copy(device, encoder) {
            self.copied_at.set(Some(self.context.submissions()));
        }
    }

    /// The color texture's pixels. The copy is the one the last submission
    /// made (`encode_requested_copy`), or else one submitted now.
    pub fn read_still_image(&self) -> PremultipliedImage {
        let size = self.size;
        let (row_stride, aligned_row_stride) = self.row_strides();
        let mut data = vec![0u8; row_stride as usize * size.height as usize];
        self.read_back.set(true);

        let backend = self.context.get_backend();
        let (device, queue) = (backend.device(), backend.queue());

        // Any later submission may have drawn the texture again.
        if self.copied_at.take() != Some(self.context.submissions()) {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            if !self.encode_copy(device, &mut encoder) {
                eprintln!("WebGPU: Color texture is null");
                return PremultipliedImage::from_vec(size, data);
            }
            queue.submit([encoder.finish()]);
        }
        let staging = self.staging_buffer.borrow();
        let Some(staging_buffer) = staging.as_ref() else {
            return PremultipliedImage::from_vec(size, data);
        };

        // Map buffer and read data (blocking)
        let (tx, rx) = std::sync::mpsc::channel();
        staging_buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        // A wait without a timeout can still report one: PowerVR's Vulkan
        // driver on Android ends the wait shortly before the fence value
        // that wgpu reads afterwards shows the submission finished. Waiting
        // again returns at once then, so this asks until the value caught up.
        let wait = || device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let start = Instant::now();
        let mut waited = wait();
        while matches!(waited, Err(wgpu::PollError::Timeout)) && start.elapsed() < WAIT_AGAIN_LIMIT {
            waited = wait();
        }
        if let Err(e) = waited {
            eprintln!("WebGPU: Buffer mapping wait failed: {e}");
        }
        match rx.try_recv() {
            Ok(Ok(())) => match staging_buffer.get_mapped_range(..) {
                Ok(mapped) => {
                    if aligned_row_stride == row_stride {
                        let n = data.len();
                        data.copy_from_slice(&mapped[..n]);
                    } else {
                        let rs = row_stride as usize;
                        let ars = aligned_row_stride as usize;
                        for row in 0..size.height as usize {
                            data[row * rs..(row + 1) * rs].copy_from_slice(&mapped[row * ars..row * ars + rs]);
                        }
                    }
                }
                Err(e) => eprintln!("WebGPU: Failed to get mapped range: {e}"),
            },
            Ok(Err(e)) => eprintln!("WebGPU: Buffer mapping failed with status: {e}"),
            Err(_) => eprintln!("WebGPU: Buffer mapping timeout after polling"),
        }
        staging_buffer.unmap();
        PremultipliedImage::from_vec(size, data)
    }

    pub fn get_texture(&self) -> Texture2DPtr {
        self.color().expect("offscreen texture without color texture")
    }
}

impl Drop for OffscreenTextureResource {
    fn drop(&mut self) {
        self.context.rendering_stats_mut().num_frame_buffers -= 1;
    }
}

impl TargetResource for OffscreenTextureResource {
    fn bind(&self) {
        // Create textures if not already created
        if let Some(color) = self.color() {
            color.borrow_mut().create();
        }
        if let Some(depth) = &self.depth_texture {
            depth.borrow_mut().create();
        }
    }

    fn finish_frame(&self, encoder: &mut wgpu::CommandEncoder) {
        self.encode_requested_copy(encoder);
    }

    fn get_color_texture_view(&self) -> Option<wgpu::TextureView> {
        let color = self.color()?;
        let mut texture = color.borrow_mut();
        texture.create();
        texture.get_texture_view().cloned()
    }

    fn get_color_texture_format(&self) -> Option<wgpu::TextureFormat> {
        self.color()?.borrow().get_native_format()
    }

    fn get_depth_stencil_texture_view(&self) -> Option<wgpu::TextureView> {
        let depth = self.depth_texture.as_ref()?;
        let mut texture = depth.borrow_mut();
        texture.create();
        texture.get_texture_view().cloned()
    }

    fn get_depth_stencil_texture_format(&self) -> Option<wgpu::TextureFormat> {
        self.depth_texture.as_ref()?.borrow().get_native_format()
    }
}

pub struct OffscreenTexture {
    target: DrawTarget,
    resource: Rc<OffscreenTextureResource>,
}

impl OffscreenTexture {
    pub fn new(context: Rc<Context>, size: Size, type_: TextureChannelDataType, depth: bool, stencil: bool) -> Self {
        let resource = Rc::new(OffscreenTextureResource::new(context, size, type_, depth, stencil));
        OffscreenTexture { target: DrawTarget::new(size, resource.clone()), resource }
    }

    fn resource(&self) -> &OffscreenTextureResource {
        &self.resource
    }
}

impl offscreen_texture::OffscreenTexture for OffscreenTexture {
    fn draw_target(&self) -> &dyn frame::DrawTarget {
        &self.target
    }

    /// Check if the texture resource is valid and can be rendered to
    fn is_renderable(&self) -> bool {
        self.resource().color().is_some()
    }

    fn read_still_image(&self) -> PremultipliedImage {
        self.resource().read_still_image()
    }

    fn get_texture(&self) -> Texture2DPtr {
        self.resource().get_texture()
    }

    fn submit(&self, encoder: &dyn GfxCommandEncoder) {
        if let Some(encoder) = encoder.downcast_ref::<CommandEncoder>() {
            encoder.submit_frame(Some(self.target.resource()));
        }
    }
}
