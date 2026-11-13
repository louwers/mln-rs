// Differences from C++:
// - C++ records every upload into the frame's command buffer. The port does
//   too between a frame's passes; an upload outside a frame or inside a
//   render pass, where Vulkan allows no copies, goes through a command
//   buffer of its own, submitted at once (`Context::record_upload`).
// - Reading an attachment back leaves it in the layout it is sampled in;
//   C++ leaves it in the transfer layout.
// - A sampler configuration is new when any of its fields changes; C++
//   compares the filter and the wrap modes.
// - The memory comes from gpu-allocator, as the buffers' does
//   (buffer_resource.rs).

use std::rc::{Rc, Weak};

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use mln_gfx::context::Context as _;
use mln_gfx::texture2d::{self, SamplerState};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::buffer_resource::{BufferAllocation, unique_id};
use crate::context::Context;
use crate::renderer_backend::BackendImpl;

/// An image, its memory and its view.
pub(crate) struct ImageAllocation {
    id: u64,
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
            id: unique_id(),
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
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    #[must_use]
    pub(crate) fn image(&self) -> vk::Image {
        self.image
    }

    #[must_use]
    pub(crate) fn image_view(&self) -> vk::ImageView {
        self.image_view
    }

    /// The image's memory, if the CPU can read it.
    #[must_use]
    pub(crate) fn mapped_ptr(&self) -> Option<*const u8> {
        self.allocation.as_ref()?.mapped_ptr().map(|p| p.as_ptr().cast::<u8>().cast_const())
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Texture2DUsage {
    ShaderInput,
    Attachment,
    Read,
    Blit,
}

pub struct Texture2D {
    context: Weak<Context>,
    backend: Rc<BackendImpl>,
    size: Size,
    pixel_format: TexturePixelType,
    channel_type: TextureChannelDataType,
    sampler_state: SamplerState,
    image_data: Option<Rc<PremultipliedImage>>,
    texture_dirty: bool,
    sampler_state_dirty: bool,
    image_allocation: Option<Rc<ImageAllocation>>,
    image_layout: vk::ImageLayout,
    sampler: vk::Sampler,
    /// The id of `sampler` (`unique_id`).
    sampler_id: u64,
    texture_usage: Texture2DUsage,
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
#[allow(clippy::too_many_arguments)]
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
            image_data: None,
            texture_dirty: true,
            sampler_state_dirty: true,
            image_allocation: None,
            image_layout: vk::ImageLayout::UNDEFINED,
            sampler: vk::Sampler::null(),
            sampler_id: 0,
            texture_usage: Texture2DUsage::ShaderInput,
        }
    }

    pub(crate) fn set_usage(&mut self, value: Texture2DUsage) {
        self.texture_usage = value;
        self.texture_dirty = true;
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

    /// The ids of the image and of the sampler (`unique_id`), 0 without one.
    #[must_use]
    pub(crate) fn get_ids(&self) -> (u64, u64) {
        (self.image_allocation.as_ref().map_or(0, |allocation| allocation.id()), self.sampler_id)
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
        let (mut image_usage, image_tiling) = match self.texture_usage {
            Texture2DUsage::ShaderInput => {
                (vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED, vk::ImageTiling::OPTIMAL)
            }
            Texture2DUsage::Attachment => (
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC,
                vk::ImageTiling::OPTIMAL,
            ),
            Texture2DUsage::Read => (vk::ImageUsageFlags::TRANSFER_DST, vk::ImageTiling::LINEAR),
            Texture2DUsage::Blit => (
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC,
                vk::ImageTiling::LINEAR,
            ),
        };
        if self.texture_usage == Texture2DUsage::ShaderInput && self.sampler_state.mipmapped {
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
            .tiling(image_tiling)
            .usage(image_usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let location =
            if self.texture_usage == Texture2DUsage::Read { MemoryLocation::GpuToCpu } else { MemoryLocation::GpuOnly };
        let Some(mut allocation) = ImageAllocation::create(&self.backend, &image_info, location, "texture") else {
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
        if self.texture_usage != Texture2DUsage::Read
            && !allocation.create_view(format, image_swizzle, vk::ImageAspectFlags::COLOR, mip_levels)
        {
            Log::error(Event::Render, "Vulkan texture view creation failed");
            return;
        }
        self.image_allocation = Some(Rc::new(allocation));

        // If the image is used as an attachment, its layout is managed by
        // the render pass.
        self.image_layout = if self.texture_usage == Texture2DUsage::Attachment {
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
        } else {
            vk::ImageLayout::UNDEFINED
        };

        if let Some(context) = self.context.upgrade() {
            let mut stats = context.rendering_stats();
            stats.num_created_textures += 1;
            stats.num_active_textures += 1;
            stats.mem_textures += texture2d::Texture2D::get_data_size(self) as i32;
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
                self.sampler_id = unique_id();
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
                stats.mem_textures -= texture2d::Texture2D::get_data_size(self) as i32;
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

    fn transition_to_general_layout(&mut self, command_buffer: vk::CommandBuffer) {
        let Some(allocation) = &self.image_allocation else {
            return;
        };
        debug_assert_eq!(self.image_layout, vk::ImageLayout::TRANSFER_DST_OPTIMAL);
        transition_image(
            self.backend.get_device(),
            command_buffer,
            allocation.image(),
            color_range(0, 1),
            (self.image_layout, vk::ImageLayout::GENERAL),
            (vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::MEMORY_READ),
            (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER),
        );
        self.image_layout = vk::ImageLayout::GENERAL;
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

        if self.sampler_state.mipmapped && self.texture_usage == Texture2DUsage::ShaderInput {
            self.generate_mips(command_buffer);
        } else if self.texture_usage == Texture2DUsage::Blit {
            self.transition_to_transfer_read_layout(command_buffer);
        } else {
            self.transition_to_shader_read_layout(command_buffer);
        }
    }

    /// Records a copy of `image`, in the transfer source layout, into this
    /// texture.
    pub(crate) fn copy_image(
        &mut self,
        image: vk::Image,
        image_size: Size,
        x_offset: u16,
        y_offset: u16,
        command_buffer: vk::CommandBuffer,
    ) {
        if image == vk::Image::null() {
            return;
        }
        texture2d::Texture2D::create(self);
        let Some(target) = self.image_allocation.as_ref().map(|allocation| allocation.image()) else {
            return;
        };
        let copy_info = vk::ImageCopy::default()
            .src_subresource(COLOR_LAYER)
            .dst_subresource(COLOR_LAYER)
            .extent(vk::Extent3D { width: image_size.width, height: image_size.height, depth: 1 })
            .dst_offset(vk::Offset3D { x: i32::from(x_offset), y: i32::from(y_offset), z: 0 });
        self.transition_to_transfer_write_layout(command_buffer);
        // SAFETY: both images are in the layouts named, and the region is
        // within both.
        unsafe {
            self.backend.get_device().cmd_copy_image(
                command_buffer,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                target,
                self.image_layout,
                &[copy_info],
            );
        }
        self.transition_to_general_layout(command_buffer);
    }

    /// `copy_image` with a blit, which converts between the images' formats.
    pub(crate) fn blit_image(
        &mut self,
        image: vk::Image,
        image_size: Size,
        x_offset: u16,
        y_offset: u16,
        command_buffer: vk::CommandBuffer,
    ) {
        if image == vk::Image::null() {
            return;
        }
        texture2d::Texture2D::create(self);
        let Some(target) = self.image_allocation.as_ref().map(|allocation| allocation.image()) else {
            return;
        };
        let offsets = [
            vk::Offset3D { x: i32::from(x_offset), y: i32::from(y_offset), z: 0 },
            vk::Offset3D { x: image_size.width as i32, y: image_size.height as i32, z: 1 },
        ];
        let blit_info = vk::ImageBlit::default()
            .src_offsets(offsets)
            .dst_offsets(offsets)
            .src_subresource(COLOR_LAYER)
            .dst_subresource(COLOR_LAYER);
        self.transition_to_transfer_write_layout(command_buffer);
        // SAFETY: as in `copy_image`.
        unsafe {
            self.backend.get_device().cmd_blit_image(
                command_buffer,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                target,
                self.image_layout,
                &[blit_info],
                vk::Filter::NEAREST,
            );
        }
        self.transition_to_general_layout(command_buffer);
    }

    /// Reads the pixels of a texture made to be read (`Texture2DUsage::Read`)
    /// or rendered to (`Attachment`).
    pub(crate) fn read_image(&mut self) -> Option<PremultipliedImage> {
        let allocation = self.image_allocation.clone()?;
        let image_size = texture2d::Texture2D::get_data_size(self);
        let row_size = self.size.width as usize * texture2d::Texture2D::get_pixel_stride(self);
        let mut data = vec![0u8; image_size];

        match self.texture_usage {
            Texture2DUsage::Attachment => {
                // For optimal tiling, we need to copy to a staging buffer
                // first.
                let context = self.context.upgrade()?;
                let Some(staging) = BufferAllocation::create(
                    &self.backend,
                    image_size,
                    vk::BufferUsageFlags::TRANSFER_DST,
                    MemoryLocation::GpuToCpu,
                    "texture read",
                )
                .filter(|staging| !staging.mapped_buffer().is_null()) else {
                    Log::error(Event::Render, "Vulkan readImage staging buffer allocation failed");
                    return None;
                };
                let device = self.backend.get_device();
                let layout = self.image_layout;
                context.submit_one_time_command(|command_buffer| {
                    // Transition image layout for reading.
                    transition_image(
                        device,
                        command_buffer,
                        allocation.image(),
                        color_range(0, 1),
                        (layout, vk::ImageLayout::TRANSFER_SRC_OPTIMAL),
                        (vk::AccessFlags::COLOR_ATTACHMENT_WRITE, vk::AccessFlags::TRANSFER_READ),
                        (vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, vk::PipelineStageFlags::TRANSFER),
                    );
                    let region = vk::BufferImageCopy::default()
                        .image_subresource(COLOR_LAYER)
                        .image_extent(vk::Extent3D { width: self.size.width, height: self.size.height, depth: 1 });
                    // SAFETY: the image is in the transfer source layout and
                    // the buffer holds its pixels.
                    unsafe {
                        device.cmd_copy_image_to_buffer(
                            command_buffer,
                            allocation.image(),
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            staging.buffer(),
                            &[region],
                        );
                    }
                    transition_image(
                        device,
                        command_buffer,
                        allocation.image(),
                        color_range(0, 1),
                        (vk::ImageLayout::TRANSFER_SRC_OPTIMAL, layout),
                        (vk::AccessFlags::TRANSFER_READ, vk::AccessFlags::SHADER_READ),
                        (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER),
                    );
                });
                // SAFETY: the staging buffer is `image_size` bytes of mapped
                // memory, which the finished copy wrote.
                data.copy_from_slice(unsafe { std::slice::from_raw_parts(staging.mapped_buffer(), image_size) });
            }
            Texture2DUsage::Read => {
                // For linear tiling (Read usage), use direct memory mapping.
                let mapped = allocation.mapped_ptr()?;
                // SAFETY: the image is live and has the color aspect.
                let layout = unsafe {
                    self.backend.get_device().get_image_subresource_layout(
                        allocation.image(),
                        vk::ImageSubresource { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, array_layer: 0 },
                    )
                };
                for (i, row) in data.chunks_exact_mut(row_size.max(1)).enumerate() {
                    // SAFETY: the layout gives where each row of the image
                    // starts in its mapped memory, `row_size` bytes long.
                    row.copy_from_slice(unsafe {
                        std::slice::from_raw_parts(
                            mapped.add(layout.offset as usize + layout.row_pitch as usize * i),
                            row_size,
                        )
                    });
                }
            }
            _ => {
                // not readable
                debug_assert!(false, "texture is not readable");
                return None;
            }
        }
        Some(PremultipliedImage::from_vec(self.size, data))
    }
}

impl Drop for Texture2D {
    /// `destroy(deferred = true)`
    fn drop(&mut self) {
        self.destroy_texture();
        self.destroy_sampler();
    }
}

impl texture2d::Texture2D for Texture2D {
    fn set_sampler_configuration(&mut self, sampler_state: SamplerState) {
        if self.sampler_state == sampler_state {
            return;
        }
        self.sampler_state = sampler_state;
        self.sampler_state_dirty = true;
    }

    fn set_format(&mut self, pixel_format: TexturePixelType, channel_type: TextureChannelDataType) {
        if pixel_format == self.pixel_format && channel_type == self.channel_type {
            return;
        }
        self.destroy_texture();
        self.pixel_format = pixel_format;
        self.channel_type = channel_type;
        self.texture_dirty = true;
    }

    fn set_size(&mut self, size: Size) {
        if size == self.size {
            return;
        }
        self.destroy_texture();
        self.size = size;
        self.texture_dirty = true;
    }

    fn set_image(&mut self, image: Option<Rc<PremultipliedImage>>) {
        self.image_data = image;
    }

    fn get_format(&self) -> TexturePixelType {
        self.pixel_format
    }

    fn get_size(&self) -> Size {
        self.size
    }

    fn get_data_size(&self) -> usize {
        self.get_pixel_stride() * self.size.width as usize * self.size.height as usize
    }

    fn get_pixel_stride(&self) -> usize {
        match self.channel_type {
            TextureChannelDataType::UnsignedByte => self.num_channels(),
            TextureChannelDataType::HalfFloat => 2 * self.num_channels(),
            TextureChannelDataType::Float => 4 * self.num_channels(),
        }
    }

    fn num_channels(&self) -> usize {
        match self.pixel_format {
            TexturePixelType::RGBA => 4,
            TexturePixelType::Alpha | TexturePixelType::Stencil | TexturePixelType::Depth => 1,
            TexturePixelType::Luminance => 0,
        }
    }

    fn create(&mut self) {
        if self.texture_dirty {
            self.create_texture();
        }
    }

    fn upload_data(&mut self, pixel_data: Option<&[u8]>, size: Size) {
        self.set_size(size);
        self.upload_sub_region(pixel_data, size, 0, 0);
    }

    fn upload_sub_region(&mut self, pixel_data: Option<&[u8]>, size: Size, x_offset: u16, y_offset: u16) {
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

    fn upload(&mut self) {
        if let Some(image) = self.image_data.take()
            && image.valid()
        {
            self.set_format(TexturePixelType::RGBA, TextureChannelDataType::UnsignedByte);
            self.upload_data(Some(&image.data), image.size);
        }
    }

    fn needs_upload(&self) -> bool {
        self.image_data.is_some()
    }
}
