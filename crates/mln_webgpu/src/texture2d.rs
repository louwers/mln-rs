// Ports include/mln/webgpu/texture2d.hpp and src/mln/webgpu/texture2d.cpp
//
// Port-only: the rendering stats count the textures and their memory, as the
// OpenGL backend's do; the C++ WebGPU backend doesn't.

use std::any::Any;
use std::rc::Rc;

use mln_gfx::texture2d::{self, SamplerState};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::context::Context;

const BYTES_PER_ROW_ALIGNMENT: usize = 256;

struct UploadCopyData {
    data: Vec<u8>,
    bytes_per_row: u32,
}

/// `prepareCopyData`: pad rows to 256 bytes when there is more than one row.
fn prepare_copy_data(pixel_data: &[u8], region_size: Size, pixel_stride: usize) -> Option<UploadCopyData> {
    if region_size.width == 0 || region_size.height == 0 {
        return None;
    }
    let row_stride = region_size.width as usize * pixel_stride;
    let data_size = row_stride * region_size.height as usize;
    if region_size.height > 1 && row_stride % BYTES_PER_ROW_ALIGNMENT != 0 {
        let aligned = row_stride.div_ceil(BYTES_PER_ROW_ALIGNMENT) * BYTES_PER_ROW_ALIGNMENT;
        let mut staging = vec![0u8; aligned * region_size.height as usize];
        for row in 0..region_size.height as usize {
            staging[row * aligned..row * aligned + row_stride]
                .copy_from_slice(&pixel_data[row * row_stride..(row + 1) * row_stride]);
        }
        return Some(UploadCopyData { data: staging, bytes_per_row: aligned as u32 });
    }
    Some(UploadCopyData { data: pixel_data[..data_size].to_vec(), bytes_per_row: row_stride as u32 })
}

pub struct Texture2D {
    context: Rc<Context>,
    texture: Option<wgpu::Texture>,
    texture_view: Option<wgpu::TextureView>,
    sampler: Option<wgpu::Sampler>,
    usage_flags: wgpu::TextureUsages,
    native_format: Option<wgpu::TextureFormat>,
    size: Size,
    pixel_format: TexturePixelType,
    channel_type: TextureChannelDataType,
    sampler_state: SamplerState,
    image: Option<Rc<PremultipliedImage>>,
    dirty: bool,
    size_changed_callback: Option<Box<dyn Fn(Size)>>,
    /// The bytes of `texture` in the rendering stats' `mem_textures`.
    counted_bytes: usize,
}

impl Texture2D {
    pub fn new(context: Rc<Context>) -> Self {
        Texture2D {
            context,
            texture: None,
            texture_view: None,
            sampler: None,
            usage_flags: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            native_format: None,
            size: Size { width: 0, height: 0 },
            pixel_format: TexturePixelType::RGBA,
            channel_type: TextureChannelDataType::UnsignedByte,
            sampler_state: SamplerState::default(),
            image: None,
            dirty: false,
            size_changed_callback: None,
            counted_bytes: 0,
        }
    }

    /// Drops the texture and takes it out of the rendering stats.
    fn release_texture(&mut self) {
        if self.texture.take().is_some() {
            let mut stats = self.context.rendering_stats_mut();
            stats.num_active_textures -= 1;
            stats.mem_textures -= self.counted_bytes as i32;
            self.counted_bytes = 0;
        }
    }

    pub fn set_usage(&mut self, usage_flags: wgpu::TextureUsages) {
        self.usage_flags = usage_flags;
    }

    pub fn set_size_changed_callback(&mut self, callback: Option<Box<dyn Fn(Size)>>) {
        self.size_changed_callback = callback;
    }

    pub fn get_native_format(&self) -> Option<wgpu::TextureFormat> {
        self.native_format
    }
    pub fn get_texture(&self) -> Option<&wgpu::Texture> {
        self.texture.as_ref()
    }
    pub fn get_texture_view(&self) -> Option<&wgpu::TextureView> {
        self.texture_view.as_ref()
    }
    pub fn get_sampler(&self) -> Option<&wgpu::Sampler> {
        self.sampler.as_ref()
    }
    pub fn get_sampler_state(&self) -> &SamplerState {
        &self.sampler_state
    }

    fn write(&self, texture: &wgpu::Texture, copy: &UploadCopyData, origin: wgpu::Origin3d, region: Size) {
        let Some(queue) = self.context.get_backend().get_queue() else {
            return;
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin, aspect: wgpu::TextureAspect::All },
            &copy.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(copy.bytes_per_row),
                rows_per_image: Some(region.height),
            },
            wgpu::Extent3d { width: region.width, height: region.height, depth_or_array_layers: 1 },
        );
    }
}

impl Drop for Texture2D {
    fn drop(&mut self) {
        self.release_texture();
    }
}

impl texture2d::Texture2D for Texture2D {
    fn set_sampler_configuration(&mut self, sampler_state: SamplerState) {
        let mut safe_state = sampler_state;
        if safe_state.max_anisotropy < 1 {
            safe_state.max_anisotropy = 1;
        }
        // Port-only: keep the sampler when the state is unchanged. Tweakers
        // set the atlases' configuration on every frame, and a new sampler
        // each time is costly in the browser (a JavaScript object per call).
        if self.sampler.is_some() && self.sampler_state == safe_state {
            return;
        }
        self.sampler_state = safe_state;
        self.sampler = None;

        let filter = if self.sampler_state.filter == TextureFilterType::Linear {
            wgpu::FilterMode::Linear
        } else {
            wgpu::FilterMode::Nearest
        };
        let map_wrap_mode = |wrap: TextureWrapType| match wrap {
            TextureWrapType::Repeat => wgpu::AddressMode::Repeat,
            TextureWrapType::Clamp => wgpu::AddressMode::ClampToEdge,
        };
        if let Some(device) = self.context.get_backend().get_device() {
            self.sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Texture Sampler"),
                address_mode_u: map_wrap_mode(self.sampler_state.wrap_u),
                address_mode_v: map_wrap_mode(self.sampler_state.wrap_v),
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: if self.sampler_state.mipmapped {
                    wgpu::MipmapFilterMode::Linear
                } else {
                    wgpu::MipmapFilterMode::Nearest
                },
                anisotropy_clamp: u16::from(self.sampler_state.max_anisotropy),
                ..Default::default()
            }));
        }
    }

    fn set_format(&mut self, pixel_format: TexturePixelType, channel_type: TextureChannelDataType) {
        self.pixel_format = pixel_format;
        self.channel_type = channel_type;
    }

    fn set_size(&mut self, size: Size) {
        if self.size == size {
            return;
        }
        self.size = size;
        self.dirty = true;
        if let Some(callback) = &self.size_changed_callback {
            callback(size);
        }
    }

    fn set_image(&mut self, image: Option<Rc<PremultipliedImage>>) {
        if let Some(img) = &image {
            self.size = img.size;
            self.dirty = true;
        }
        self.image = image;
    }

    fn get_format(&self) -> TexturePixelType {
        self.pixel_format
    }

    fn get_size(&self) -> Size {
        self.size
    }

    fn get_data_size(&self) -> usize {
        self.size.width as usize * self.size.height as usize * self.get_pixel_stride()
    }

    fn get_pixel_stride(&self) -> usize {
        match self.pixel_format {
            TexturePixelType::Alpha | TexturePixelType::Luminance => match self.channel_type {
                TextureChannelDataType::UnsignedByte => 1,
                TextureChannelDataType::HalfFloat => 2,
                TextureChannelDataType::Float => 4,
            },
            TexturePixelType::RGBA => match self.channel_type {
                TextureChannelDataType::UnsignedByte => 4,
                TextureChannelDataType::HalfFloat => 8,
                TextureChannelDataType::Float => 16,
            },
            _ => 4,
        }
    }

    fn num_channels(&self) -> usize {
        match self.pixel_format {
            TexturePixelType::Alpha | TexturePixelType::Luminance => 1,
            _ => 4,
        }
    }

    fn create(&mut self) {
        // If texture already exists, don't recreate it (quirk ported: a later
        // setSize does not resize an existing texture).
        if self.texture.is_some() && self.texture_view.is_some() {
            return;
        }
        self.texture_view = None;
        self.release_texture();

        let Some(device) = self.context.get_backend().get_device() else {
            return;
        };
        if self.size.width == 0 || self.size.height == 0 {
            return;
        }

        // Determine WebGPU texture format based on pixel format
        let mut usage = self.usage_flags;
        let format = match self.pixel_format {
            TexturePixelType::Alpha | TexturePixelType::Luminance => match self.channel_type {
                TextureChannelDataType::UnsignedByte => wgpu::TextureFormat::R8Unorm,
                TextureChannelDataType::HalfFloat => {
                    usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
                    wgpu::TextureFormat::R16Float
                }
                TextureChannelDataType::Float => {
                    usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
                    wgpu::TextureFormat::R32Float
                }
            },
            TexturePixelType::Depth => {
                usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
                wgpu::TextureFormat::Depth32Float
            }
            TexturePixelType::Stencil => {
                // C++ `#ifdef WGPUTextureFormat_Stencil8` is never true (an
                // enum value, not a macro), so Stencil maps to Depth24PlusStencil8.
                usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
                wgpu::TextureFormat::Depth24PlusStencil8
            }
            TexturePixelType::RGBA => match self.channel_type {
                TextureChannelDataType::UnsignedByte => wgpu::TextureFormat::Rgba8Unorm,
                TextureChannelDataType::HalfFloat => {
                    usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
                    wgpu::TextureFormat::Rgba16Float
                }
                TextureChannelDataType::Float => {
                    usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
                    wgpu::TextureFormat::Rgba32Float
                }
            },
        };
        self.native_format = Some(format);

        // Ensure color targets are renderable when requested via usage flags
        if self.pixel_format == TexturePixelType::RGBA {
            usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Texture2D"),
            size: wgpu::Extent3d { width: self.size.width, height: self.size.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        self.texture_view = Some(texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Texture2D View"),
            format: Some(format),
            dimension: Some(wgpu::TextureViewDimension::D2),
            usage: None,
            aspect: wgpu::TextureAspect::All,
            base_mip_level: 0,
            mip_level_count: Some(1),
            base_array_layer: 0,
            array_layer_count: Some(1),
        }));
        self.texture = Some(texture);
        self.counted_bytes = self.get_data_size();
        {
            let mut stats = self.context.rendering_stats_mut();
            stats.num_created_textures += 1;
            stats.num_active_textures += 1;
            stats.mem_textures += self.counted_bytes as i32;
        }

        // Create default sampler if not already created
        if self.sampler.is_none() {
            let state = self.sampler_state;
            self.set_sampler_configuration(state);
        }
    }

    fn upload_data(&mut self, pixel_data: Option<&[u8]>, size: Size) {
        self.size = size;
        // Create texture if it doesn't exist
        if self.texture.is_none() {
            self.create();
        }
        let (Some(texture), Some(pixel_data)) = (self.texture.clone(), pixel_data) else {
            return;
        };
        let Some(copy) = prepare_copy_data(pixel_data, size, self.get_pixel_stride()) else {
            return;
        };
        self.write(&texture, &copy, wgpu::Origin3d::ZERO, size);
    }

    fn upload_sub_region(&mut self, pixel_data: Option<&[u8]>, region_size: Size, x_offset: u16, y_offset: u16) {
        let (Some(texture), Some(pixel_data)) = (self.texture.clone(), pixel_data) else {
            return;
        };
        let Some(copy) = prepare_copy_data(pixel_data, region_size, self.get_pixel_stride()) else {
            return;
        };
        let origin = wgpu::Origin3d { x: u32::from(x_offset), y: u32::from(y_offset), z: 0 };
        self.write(&texture, &copy, origin, region_size);
    }

    fn upload(&mut self) {
        if let Some(image) = self.image.clone() {
            self.upload_data(Some(&image.data), image.size);
            self.dirty = false;
        }
    }

    fn needs_upload(&self) -> bool {
        self.dirty
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
