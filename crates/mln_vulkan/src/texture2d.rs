// Differences from C++:
// - C++ records every upload into the frame's command buffer. The port
//   records it into a command buffer of its own, which the context's `Gpu`
//   submits at once, after the work submitted before it
//   (`Context::record_upload`).
// - Only textures that shaders sample are made: the renderer's attachments
//   are `Gpu` textures.
// - A sampler configuration is new when any of its fields changes; C++
//   compares the filter and the wrap modes.
// - The memory comes from gpu-allocator, as the buffers' does
//   (buffer_resource.rs).

use std::rc::{Rc, Weak};

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use mln_gfx::context::Context as _;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::buffer_resource::BufferAllocation;
use crate::context::Context;
use crate::renderer_backend::BackendImpl;

/// An image, its memory and its view.
pub(crate) struct ImageAllocation {
    backend: Rc<BackendImpl>,
    image: vk::Image,
    allocation: Option<Allocation>,
    image_view: vk::ImageView,
}

impl ImageAllocation {
    /// Creates an image in device memory (`MemoryLocation::GpuOnly`) or in
    /// memory the CPU reads (`GpuToCpu`).
    pub(crate) fn create(
        backend: &Rc<BackendImpl>,
        image_info: &vk::ImageCreateInfo<'_>,
        location: MemoryLocation,
        name: &str,
    ) -> Option<ImageAllocation> {
        let device = backend.get_device();
        // SAFETY: the create info is valid for the device.
        let image = unsafe { device.create_image(image_info, None) }.ok()?;
        // SAFETY: `image` is an image of the device.
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let allocation = backend.with_allocator(|allocator| {
            allocator.allocate(&AllocationCreateDesc {
                name,
                requirements,
                location,
                linear: image_info.tiling == vk::ImageTiling::LINEAR,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
        });
        let allocation = match allocation {
            // SAFETY: the memory was allocated for the image's requirements,
            // and neither is bound yet.
            Ok(allocation)
                if unsafe { device.bind_image_memory(image, allocation.memory(), allocation.offset()).is_ok() } =>
            {
                allocation
            }
            result => {
                if let Ok(allocation) = result {
                    let _ = backend.with_allocator(|allocator| allocator.free(allocation));
                }
                // SAFETY: nothing uses the image yet.
                unsafe { device.destroy_image(image, None) };
                return None;
            }
        };
        Some(ImageAllocation {
            backend: Rc::clone(backend),
            image,
            allocation: Some(allocation),
            image_view: vk::ImageView::null(),
        })
    }

    /// Creates the image's view, of every mip level of `aspect`.
    pub(crate) fn create_view(
        &mut self,
        format: vk::Format,
        components: vk::ComponentMapping,
        aspect: vk::ImageAspectFlags,
        mip_levels: u32,
    ) -> bool {
        let create_info = vk::ImageViewCreateInfo::default()
            .image(self.image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .components(components)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: aspect,
                base_mip_level: 0,
                level_count: mip_levels,
                base_array_layer: 0,
                layer_count: 1,
            });
        // SAFETY: the image is live and of `format`.
        match unsafe { self.backend.get_device().create_image_view(&create_info, None) } {
            Ok(view) => {
                self.image_view = view;
                true
            }
            Err(_) => false,
        }
    }

    #[must_use]
    pub(crate) fn image(&self) -> vk::Image {
        self.image
    }

    #[must_use]
    pub(crate) fn image_view(&self) -> vk::ImageView {
        self.image_view
    }
}

impl Drop for ImageAllocation {
    fn drop(&mut self) {
        let device = self.backend.get_device();
        // SAFETY: the owner of the allocation waited for the commands that
        // use the image (the context's deletion queue), or never used it.
        unsafe {
            if self.image_view != vk::ImageView::null() {
                device.destroy_image_view(self.image_view, None);
            }
            device.destroy_image(self.image, None);
        }
        if let Some(allocation) = self.allocation.take() {
            let _ = self.backend.with_allocator(|allocator| allocator.free(allocation));
        }
    }
}

/// A texture of the context itself, outside its `Gpu`, that shaders sample:
/// an overlay's (`OverlayTarget`).
pub struct Texture2D {
    context: Weak<Context>,
    backend: Rc<BackendImpl>,
    size: Size,
    pixel_format: TexturePixelType,
    channel_type: TextureChannelDataType,
    sampler_state: SamplerState,
    texture_dirty: bool,
    sampler_state_dirty: bool,
    image_allocation: Option<Rc<ImageAllocation>>,
    image_layout: vk::ImageLayout,
    sampler: vk::Sampler,
}

const COLOR_LAYER: vk::ImageSubresourceLayers = vk::ImageSubresourceLayers {
    aspect_mask: vk::ImageAspectFlags::COLOR,
    mip_level: 0,
    base_array_layer: 0,
    layer_count: 1,
};

fn color_range(base_mip_level: u32, level_count: u32) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level,
        level_count,
        base_array_layer: 0,
        layer_count: 1,
    }
}

/// Records a layout transition of `image`'s color `range`.
pub(crate) fn transition_image(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    range: vk::ImageSubresourceRange,
    (old_layout, new_layout): (vk::ImageLayout, vk::ImageLayout),
    (src_access, dst_access): (vk::AccessFlags, vk::AccessFlags),
    (src_stage, dst_stage): (vk::PipelineStageFlags, vk::PipelineStageFlags),
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .image(image)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_access_mask(src_access)
        .dst_access_mask(dst_access)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .subresource_range(range);
    // SAFETY: `command_buffer` is recording outside a render pass, and
    // `image` is in `old_layout` when the command runs.
    unsafe {
        device.cmd_pipeline_barrier(
            command_buffer,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}

impl Texture2D {
    pub(crate) fn new(context: &Context) -> Self {
        Texture2D {
            context: context.weak(),
            backend: Rc::clone(context.get_backend()),
            size: Size::new(0, 0),
            pixel_format: TexturePixelType::RGBA,
            channel_type: TextureChannelDataType::UnsignedByte,
            sampler_state: SamplerState::default(),
            texture_dirty: true,
            sampler_state_dirty: true,
            image_allocation: None,
            image_layout: vk::ImageLayout::UNDEFINED,
            sampler: vk::Sampler::null(),
        }
    }

    #[must_use]
    pub(crate) fn get_vulkan_format(&self) -> vk::Format {
        Self::vulkan_format(self.pixel_format, self.channel_type)
    }

    #[must_use]
    pub fn get_vulkan_image_layout(&self) -> vk::ImageLayout {
        self.image_layout
    }

    /// The view of the image, null until the texture is created.
    #[must_use]
    pub fn get_vulkan_image_view(&self) -> vk::ImageView {
        self.image_allocation.as_ref().map_or(vk::ImageView::null(), |allocation| allocation.image_view())
    }

    /// The sampler of the current configuration.
    pub fn get_vulkan_sampler(&mut self) -> vk::Sampler {
        if self.sampler_state_dirty {
            self.create_sampler();
        }
        self.sampler
    }

    fn vulkan_format(pixel: TexturePixelType, channel: TextureChannelDataType) -> vk::Format {
        match pixel {
            // depth/stencil are packed formats
            TexturePixelType::Stencil => vk::Format::S8_UINT,
            TexturePixelType::Depth | TexturePixelType::Luminance => vk::Format::UNDEFINED,
            TexturePixelType::Alpha => match channel {
                TextureChannelDataType::UnsignedByte => vk::Format::R8_UNORM,
                TextureChannelDataType::HalfFloat => vk::Format::R16_SFLOAT,
                TextureChannelDataType::Float => vk::Format::R32_SFLOAT,
            },
            TexturePixelType::RGBA => match channel {
                TextureChannelDataType::UnsignedByte => vk::Format::R8G8B8A8_UNORM,
                TextureChannelDataType::HalfFloat => vk::Format::R16G16B16A16_SFLOAT,
                TextureChannelDataType::Float => vk::Format::R32G32B32A32_SFLOAT,
            },
        }
    }

    fn vulkan_filter(type_: TextureFilterType) -> vk::Filter {
        match type_ {
            TextureFilterType::Nearest => vk::Filter::NEAREST,
            TextureFilterType::Linear => vk::Filter::LINEAR,
        }
    }

    fn vulkan_address_mode(type_: TextureWrapType) -> vk::SamplerAddressMode {
        match type_ {
            TextureWrapType::Clamp => vk::SamplerAddressMode::CLAMP_TO_EDGE,
            TextureWrapType::Repeat => vk::SamplerAddressMode::REPEAT,
        }
    }

    fn get_mip_levels(&self) -> u32 {
        if self.sampler_state.mipmapped && self.size.width > 0 && self.size.height > 0 {
            self.size.width.max(self.size.height).ilog2() + 1
        } else {
            1
        }
    }

    fn create_texture(&mut self) {
        if self.size.width == 0 || self.size.height == 0 {
            return;
        }
        self.destroy_texture();

        let format = self.get_vulkan_format();
        let mut image_usage = vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED;
        if self.sampler_state.mipmapped {
            image_usage |= vk::ImageUsageFlags::TRANSFER_SRC;
        }
        let mip_levels = self.get_mip_levels();
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width: self.size.width, height: self.size.height, depth: 1 })
            .mip_levels(mip_levels)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(image_usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let Some(mut allocation) =
            ImageAllocation::create(&self.backend, &image_info, MemoryLocation::GpuOnly, "texture")
        else {
            Log::error(Event::Render, "Vulkan texture allocation failed");
            return;
        };

        // Alpha only format (eA8UnormKHR) is not part of core: use a R8
        // texture and map red channel to alpha.
        let image_swizzle = if self.pixel_format == TexturePixelType::Alpha {
            vk::ComponentMapping {
                r: vk::ComponentSwizzle::ZERO,
                g: vk::ComponentSwizzle::ZERO,
                b: vk::ComponentSwizzle::ZERO,
                a: vk::ComponentSwizzle::R,
            }
        } else {
            vk::ComponentMapping::default()
        };
        if !allocation.create_view(format, image_swizzle, vk::ImageAspectFlags::COLOR, mip_levels) {
            Log::error(Event::Render, "Vulkan texture view creation failed");
            return;
        }
        self.image_allocation = Some(Rc::new(allocation));

        self.image_layout = vk::ImageLayout::UNDEFINED;

        if let Some(context) = self.context.upgrade() {
            let mut stats = context.rendering_stats();
            stats.num_created_textures += 1;
            stats.num_active_textures += 1;
            stats.mem_textures += self.get_data_size() as i32;
        }
        self.texture_dirty = false;
    }

    fn create_sampler(&mut self) {
        self.destroy_sampler();

        let filter = Self::vulkan_filter(self.sampler_state.filter);
        let mut create_info = vk::SamplerCreateInfo::default()
            .min_filter(filter)
            .mag_filter(filter)
            .min_lod(0.0)
            .max_lod((self.get_mip_levels() - 1) as f32)
            .address_mode_u(Self::vulkan_address_mode(self.sampler_state.wrap_u))
            .address_mode_v(Self::vulkan_address_mode(self.sampler_state.wrap_v))
            .address_mode_w(vk::SamplerAddressMode::REPEAT);
        if self.sampler_state.mipmapped {
            create_info = create_info.mipmap_mode(vk::SamplerMipmapMode::LINEAR);
        }
        if self.sampler_state.max_anisotropy != 1 && self.backend.get_device_features().sampler_anisotropy == vk::TRUE {
            let device_max_anisotropy = self.backend.get_device_properties().limits.max_sampler_anisotropy;
            let max_anisotropy = device_max_anisotropy.min(f32::from(self.sampler_state.max_anisotropy));
            create_info = create_info.anisotropy_enable(true).max_anisotropy(max_anisotropy);
        }
        // SAFETY: the create info is valid for the device.
        match unsafe { self.backend.get_device().create_sampler(&create_info, None) } {
            Ok(sampler) => {
                self.sampler = sampler;
            }
            Err(_) => Log::error(Event::Render, "Vulkan sampler creation failed"),
        }
        self.sampler_state_dirty = false;
    }

    fn destroy_texture(&mut self) {
        let Some(allocation) = self.image_allocation.take() else {
            return;
        };
        self.image_layout = vk::ImageLayout::UNDEFINED;
        self.texture_dirty = true;
        if let Some(context) = self.context.upgrade() {
            {
                let mut stats = context.rendering_stats();
                stats.num_active_textures -= 1;
                stats.mem_textures -= self.get_data_size() as i32;
            }
            context.enqueue_deletion(Box::new(move |_| drop(allocation)));
        }
    }

    fn destroy_sampler(&mut self) {
        let sampler = std::mem::replace(&mut self.sampler, vk::Sampler::null());
        if sampler == vk::Sampler::null() {
            return;
        }
        let backend = Rc::clone(&self.backend);
        // SAFETY: the commands that use the sampler have been waited for
        // when the deletion queue runs, or the context is gone.
        let destroy = move || unsafe { backend.get_device().destroy_sampler(sampler, None) };
        match self.context.upgrade() {
            Some(context) => context.enqueue_deletion(Box::new(move |_| destroy())),
            None => destroy(),
        }
    }

    fn transition_to_transfer_write_layout(&mut self, command_buffer: vk::CommandBuffer) {
        let Some(allocation) = &self.image_allocation else {
            return;
        };
        let (src_access, src_stage) = match self.image_layout {
            vk::ImageLayout::UNDEFINED => (vk::AccessFlags::empty(), vk::PipelineStageFlags::TOP_OF_PIPE),
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL => {
                (vk::AccessFlags::SHADER_READ, vk::PipelineStageFlags::FRAGMENT_SHADER)
            }
            vk::ImageLayout::GENERAL => (vk::AccessFlags::TRANSFER_WRITE, vk::PipelineStageFlags::TRANSFER),
            _ => {
                debug_assert!(false, "unexpected texture layout");
                (vk::AccessFlags::empty(), vk::PipelineStageFlags::TOP_OF_PIPE)
            }
        };
        transition_image(
            self.backend.get_device(),
            command_buffer,
            allocation.image(),
            color_range(0, self.get_mip_levels()),
            (self.image_layout, vk::ImageLayout::TRANSFER_DST_OPTIMAL),
            (src_access, vk::AccessFlags::TRANSFER_WRITE),
            (src_stage, vk::PipelineStageFlags::TRANSFER),
        );
        self.image_layout = vk::ImageLayout::TRANSFER_DST_OPTIMAL;
    }

    fn transition_to_transfer_read_layout(&mut self, command_buffer: vk::CommandBuffer) {
        let Some(allocation) = &self.image_allocation else {
            return;
        };
        debug_assert_eq!(self.image_layout, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        transition_image(
            self.backend.get_device(),
            command_buffer,
            allocation.image(),
            color_range(0, 1),
            (self.image_layout, vk::ImageLayout::TRANSFER_SRC_OPTIMAL),
            (vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::TRANSFER_READ),
            (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER),
        );
        self.image_layout = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
    }

    fn transition_to_shader_read_layout(&mut self, command_buffer: vk::CommandBuffer) {
        let Some(allocation) = &self.image_allocation else {
            return;
        };
        debug_assert!(
            self.image_layout == vk::ImageLayout::TRANSFER_SRC_OPTIMAL
                || self.image_layout == vk::ImageLayout::TRANSFER_DST_OPTIMAL
        );
        let src_access = if self.image_layout == vk::ImageLayout::TRANSFER_SRC_OPTIMAL {
            vk::AccessFlags::TRANSFER_READ
        } else {
            vk::AccessFlags::TRANSFER_WRITE
        };
        transition_image(
            self.backend.get_device(),
            command_buffer,
            allocation.image(),
            color_range(0, self.get_mip_levels()),
            (self.image_layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
            (src_access, vk::AccessFlags::SHADER_READ),
            (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER),
        );
        self.image_layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
    }

    /// Blits each mip level from the one above.
    fn generate_mips(&mut self, command_buffer: vk::CommandBuffer) {
        let mip_levels = self.get_mip_levels();
        let Some(image) = self.image_allocation.as_ref().map(|allocation| allocation.image()) else {
            return;
        };
        if mip_levels <= 1 {
            return;
        }
        let device = self.backend.get_device().clone();

        // Transfer top level mip to read.
        self.transition_to_transfer_read_layout(command_buffer);

        let mut mip_width = self.size.width as i32;
        let mut mip_height = self.size.height as i32;
        for i in 1..mip_levels {
            let next_mip_width = (mip_width / 2).max(1);
            let next_mip_height = (mip_height / 2).max(1);
            let blit = vk::ImageBlit::default()
                .src_offsets([vk::Offset3D::default(), vk::Offset3D { x: mip_width, y: mip_height, z: 1 }])
                .dst_offsets([vk::Offset3D::default(), vk::Offset3D { x: next_mip_width, y: next_mip_height, z: 1 }])
                .src_subresource(vk::ImageSubresourceLayers { mip_level: i - 1, ..COLOR_LAYER })
                .dst_subresource(vk::ImageSubresourceLayers { mip_level: i, ..COLOR_LAYER });
            // SAFETY: level `i - 1` is in the transfer source layout and
            // level `i` in the transfer destination layout.
            unsafe {
                device.cmd_blit_image(
                    command_buffer,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[blit],
                    vk::Filter::LINEAR,
                );
            }
            // Prepare next loop read.
            transition_image(
                &device,
                command_buffer,
                image,
                color_range(i, 1),
                (vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL),
                (vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::TRANSFER_READ),
                (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER),
            );
            mip_width = next_mip_width;
            mip_height = next_mip_height;
        }

        // Transition image to fragment read.
        self.image_layout = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
        self.transition_to_shader_read_layout(command_buffer);
    }

    /// Records the copy of `staging` into the region of `size` at `x_offset`,
    /// `y_offset`.
    fn record_upload_sub_region(
        &mut self,
        command_buffer: vk::CommandBuffer,
        staging: &BufferAllocation,
        size: Size,
        x_offset: u16,
        y_offset: u16,
    ) {
        let Some(image) = self.image_allocation.as_ref().map(|allocation| allocation.image()) else {
            return;
        };
        self.transition_to_transfer_write_layout(command_buffer);

        let region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(size.width)
            .image_subresource(COLOR_LAYER)
            .image_offset(vk::Offset3D { x: i32::from(x_offset), y: i32::from(y_offset), z: 0 })
            .image_extent(vk::Extent3D { width: size.width, height: size.height, depth: 1 });
        // SAFETY: the staging buffer holds the region's pixels, and the
        // image is in the transfer destination layout.
        unsafe {
            self.backend.get_device().cmd_copy_buffer_to_image(
                command_buffer,
                staging.buffer(),
                image,
                self.image_layout,
                &[region],
            );
        }

        if self.sampler_state.mipmapped {
            self.generate_mips(command_buffer);
        } else {
            self.transition_to_shader_read_layout(command_buffer);
        }
    }
}

impl Drop for Texture2D {
    /// `destroy(deferred = true)`
    fn drop(&mut self) {
        self.destroy_texture();
        self.destroy_sampler();
    }
}

impl Texture2D {
    pub fn set_sampler_configuration(&mut self, sampler_state: SamplerState) {
        if self.sampler_state == sampler_state {
            return;
        }
        self.sampler_state = sampler_state;
        self.sampler_state_dirty = true;
    }

    pub fn set_format(&mut self, pixel_format: TexturePixelType, channel_type: TextureChannelDataType) {
        if pixel_format == self.pixel_format && channel_type == self.channel_type {
            return;
        }
        self.destroy_texture();
        self.pixel_format = pixel_format;
        self.channel_type = channel_type;
        self.texture_dirty = true;
    }

    pub fn set_size(&mut self, size: Size) {
        if size == self.size {
            return;
        }
        self.destroy_texture();
        self.size = size;
        self.texture_dirty = true;
    }

    #[must_use]
    pub fn get_format(&self) -> TexturePixelType {
        self.pixel_format
    }

    #[must_use]
    pub fn get_size(&self) -> Size {
        self.size
    }

    #[must_use]
    pub fn get_data_size(&self) -> usize {
        self.get_pixel_stride() * self.size.width as usize * self.size.height as usize
    }

    #[must_use]
    pub fn get_pixel_stride(&self) -> usize {
        match self.channel_type {
            TextureChannelDataType::UnsignedByte => self.num_channels(),
            TextureChannelDataType::HalfFloat => 2 * self.num_channels(),
            TextureChannelDataType::Float => 4 * self.num_channels(),
        }
    }

    #[must_use]
    pub fn num_channels(&self) -> usize {
        match self.pixel_format {
            TexturePixelType::RGBA => 4,
            TexturePixelType::Alpha | TexturePixelType::Stencil | TexturePixelType::Depth => 1,
            TexturePixelType::Luminance => 0,
        }
    }

    pub fn create(&mut self) {
        if self.texture_dirty {
            self.create_texture();
        }
    }

    pub fn upload_data(&mut self, pixel_data: Option<&[u8]>, size: Size) {
        self.set_size(size);
        self.upload_sub_region(pixel_data, size, 0, 0);
    }

    pub fn upload_sub_region(&mut self, pixel_data: Option<&[u8]>, size: Size, x_offset: u16, y_offset: u16) {
        let Some(pixel_data) = pixel_data else {
            return;
        };
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.create();
        let Some(context) = self.context.upgrade() else {
            return;
        };
        if self.image_allocation.is_none() {
            return;
        }

        let byte_size = size.width as usize * size.height as usize * self.get_pixel_stride();
        debug_assert!(pixel_data.len() >= byte_size);
        if pixel_data.len() < byte_size {
            return;
        }
        let Some(staging) = BufferAllocation::create(
            &self.backend,
            byte_size,
            vk::BufferUsageFlags::TRANSFER_SRC,
            MemoryLocation::CpuToGpu,
            "texture upload",
        )
        .filter(|staging| !staging.mapped_buffer().is_null()) else {
            Log::error(Event::Render, "Vulkan texture buffer allocation failed");
            return;
        };
        // SAFETY: the staging buffer is `byte_size` bytes of mapped memory.
        unsafe { std::ptr::copy_nonoverlapping(pixel_data.as_ptr(), staging.mapped_buffer(), byte_size) };

        let submitted = context.record_upload(|command_buffer| {
            self.record_upload_sub_region(command_buffer, &staging, size, x_offset, y_offset);
        });
        if !submitted {
            // The frame's command buffer copies from the buffer later.
            context.enqueue_deletion(Box::new(move |_| drop(staging)));
        }

        let mut stats = context.rendering_stats();
        stats.num_texture_updates += 1;
        stats.texture_update_bytes += byte_size;
    }
}
