// Ports include/mln/vulkan/renderable_resource.hpp and src/mln/vulkan/renderable_resource.cpp
//
// `gfx::Renderable` stores its resource as `Rc<dyn gfx::RenderableResource>`.
// The Vulkan resources are wrapped in `RenderableResourceBox`, the one
// concrete type the backend downcasts to (`as_vulkan`), standing in for the
// C++ `getResource<vulkan::RenderableResource>()` static_cast.
//
// C++'s `SurfaceRenderableResource` is a base class the platforms derive
// from, to make their surface and, headless, to read every frame back. Here
// it is one type: the platform's part is a `SurfaceProvider`, and reading
// every frame back an option.
//
// Differences from C++:
// - The render pass is the context's for the resource's formats
//   (`Context::get_render_pass`).
// - Android's surface pre-rotation (`hasSurfaceTransformSupport`,
//   `getRotation`, the polling for a new transform) is not ported: the
//   swapchain is made with the identity transform.
// - `getAcquireTimeout` is not ported: acquiring an image waits.
// - A swapchain's images can be copied from when the surface allows it, so
//   a queued read needs no new swapchain.
// - A resource without a platform surface counts as an offscreen frame
//   buffer in the rendering stats, as an offscreen texture does.

use std::any::Any;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use ash::vk;
use gpu_allocator::MemoryLocation;
use mln_gfx::context::Context as _;
use mln_gfx::renderable;
use mln_gfx::texture2d::Texture2D as _;
use mln_gfx::types::{TextureChannelDataType, TexturePixelType};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::context::{Context, RenderPassKey};
use crate::renderer_backend::{BackendImpl, SurfaceProvider};
use crate::texture2d::{ImageAllocation, Texture2D, Texture2DUsage, transition_image};

pub trait RenderableResource {
    /// Prepares the resource for a pass.
    fn bind(&self);
    /// Renderable resources that require a swap function to be called
    /// explicitly can override this method.
    fn swap(&self) {}
    fn get_extent(&self) -> vk::Extent2D;
    fn get_render_pass(&self) -> vk::RenderPass;
    fn get_framebuffer(&self) -> vk::Framebuffer;
    fn as_any(&self) -> &dyn Any;
}

/// The `gfx::RenderableResource` of every Vulkan renderable.
pub struct RenderableResourceBox(pub Rc<dyn RenderableResource>);

impl renderable::RenderableResource for RenderableResourceBox {
    fn bind(&self) {
        self.0.bind();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl RenderableResourceBox {
    pub fn new_rc(resource: Rc<dyn RenderableResource>) -> Rc<dyn renderable::RenderableResource> {
        Rc::new(RenderableResourceBox(resource))
    }
}

/// `renderable.getResource<vulkan::RenderableResource>()`
pub fn as_vulkan(resource: &dyn renderable::RenderableResource) -> Option<&dyn RenderableResource> {
    resource.as_any().downcast_ref::<RenderableResourceBox>().map(|b| b.0.as_ref())
}

/// `renderable.getResource<T>()` for a concrete Vulkan resource type.
pub fn downcast_resource<T: 'static>(resource: &dyn renderable::RenderableResource) -> Option<&T> {
    as_vulkan(resource)?.as_any().downcast_ref::<T>()
}

struct SurfaceState {
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    present_mode: vk::PresentModeKHR,
    extent: vk::Extent2D,
    render_pass: vk::RenderPass,
    acquired_image_index: u32,

    // optional color textures when no surface is available
    color_allocations: Vec<ImageAllocation>,

    swapchain_images: Vec<vk::Image>,
    swapchain_image_views: Vec<vk::ImageView>,
    swapchain_framebuffers: Vec<vk::Framebuffer>,
    acquire_semaphores: Vec<vk::Semaphore>,
    present_semaphores: Vec<vk::Semaphore>,
    color_format: vk::Format,

    depth_allocation: Option<ImageAllocation>,
    depth_format: vk::Format,

    surface_read: bool,
    read_texture: Option<Texture2D>,
}

/// `SurfaceRenderableResource`: the default renderable's images, a
/// swapchain's when there is a platform surface and its own otherwise, with
/// a depth and stencil attachment.
pub struct SurfaceRenderableResource {
    backend: Rc<BackendImpl>,
    context: Weak<Context>,
    provider: Option<Rc<dyn SurfaceProvider>>,
    /// Whether every frame is copied for `read_image` (the headless
    /// backend's `swap`).
    read_every_frame: bool,
    state: RefCell<SurfaceState>,
}

impl SurfaceRenderableResource {
    /// A resource of `size`, on `surface` if there is one. `provider` makes
    /// the surface again when it is lost, and `present_mode` is used when
    /// the surface has it, FIFO otherwise.
    pub fn new(
        context: &Context,
        size: Size,
        surface: Option<vk::SurfaceKHR>,
        provider: Option<Rc<dyn SurfaceProvider>>,
        present_mode: vk::PresentModeKHR,
        read_every_frame: bool,
    ) -> Rc<Self> {
        let resource = Rc::new(SurfaceRenderableResource {
            backend: context.get_backend().clone(),
            context: context.weak(),
            provider,
            read_every_frame,
            state: RefCell::new(SurfaceState {
                surface: surface.unwrap_or_default(),
                swapchain: vk::SwapchainKHR::null(),
                present_mode,
                extent: vk::Extent2D::default(),
                render_pass: vk::RenderPass::null(),
                acquired_image_index: 0,
                color_allocations: Vec::new(),
                swapchain_images: Vec::new(),
                swapchain_image_views: Vec::new(),
                swapchain_framebuffers: Vec::new(),
                acquire_semaphores: Vec::new(),
                present_semaphores: Vec::new(),
                color_format: vk::Format::UNDEFINED,
                depth_allocation: None,
                depth_format: vk::Format::UNDEFINED,
                surface_read: false,
                read_texture: None,
            }),
        });
        resource.init(context, size.width, size.height);
        if surface.is_none() {
            context.rendering_stats().num_frame_buffers += 1;
        }
        resource
    }

    /// `getPlatformSurface()`: whether the resource presents to a surface.
    pub fn has_platform_surface(&self) -> bool {
        self.state.borrow().surface != vk::SurfaceKHR::null()
    }

    pub fn get_swapchain(&self) -> vk::SwapchainKHR {
        self.state.borrow().swapchain
    }

    pub fn get_color_format(&self) -> vk::Format {
        self.state.borrow().color_format
    }

    pub fn get_image_count(&self) -> u32 {
        self.state.borrow().swapchain_framebuffers.len() as u32
    }

    pub fn get_acquired_image_index(&self) -> u32 {
        self.state.borrow().acquired_image_index
    }

    pub fn set_acquired_image_index(&self, index: u32) {
        self.state.borrow_mut().acquired_image_index = index;
    }

    /// `getAcquiredImage()`
    pub fn get_acquired_image(&self) -> vk::Image {
        let state = self.state.borrow();
        state.swapchain_images.get(state.acquired_image_index as usize).copied().unwrap_or_default()
    }

    /// `getAcquireSemaphore()`: the semaphore the frame `frame_index`
    /// acquires its image with.
    pub fn get_acquire_semaphore(&self, frame_index: u8) -> vk::Semaphore {
        self.state.borrow().acquire_semaphores.get(usize::from(frame_index)).copied().unwrap_or_default()
    }

    /// `getPresentSemaphore()`: the semaphore the acquired image is
    /// presented with.
    pub fn get_present_semaphore(&self) -> vk::Semaphore {
        let state = self.state.borrow();
        state.present_semaphores.get(state.acquired_image_index as usize).copied().unwrap_or_default()
    }

    /// `initColor(w, h)`: the color images of a resource without a surface.
    fn init_color(&self, state: &mut SurfaceState, w: u32, h: u32) {
        state.color_format = vk::Format::R8G8B8A8_UNORM;
        state.extent = vk::Extent2D { width: w, height: h };

        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(state.color_format)
            .extent(vk::Extent3D { width: w, height: h, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        for _ in 0..self.backend.get_max_frames() {
            match ImageAllocation::create(&self.backend, &image_info, MemoryLocation::GpuOnly, "color") {
                Some(allocation) => {
                    state.swapchain_images.push(allocation.image());
                    state.color_allocations.push(allocation);
                }
                None => Log::error(Event::Render, "Vulkan color texture allocation failed"),
            }
        }
    }

    /// `initSwapchain(w, h)`
    fn init_swapchain(&self, state: &mut SurfaceState, w: u32, h: u32) -> Result<(), String> {
        let physical_device = self.backend.get_physical_device();
        let device = self.backend.get_device();
        let surface_loader = self.backend.get_surface_loader();
        let swapchain_loader = self.backend.get_swapchain_loader().ok_or("The device has no swapchains")?;
        let surface = state.surface;

        // SAFETY: the surface is the instance's, and the device can present
        // to it.
        let (formats, present_modes, capabilities) = unsafe {
            (
                surface_loader
                    .get_physical_device_surface_formats(physical_device, surface)
                    .map_err(|e| format!("Surface formats: {e}"))?,
                surface_loader
                    .get_physical_device_surface_present_modes(physical_device, surface)
                    .map_err(|e| format!("Surface present modes: {e}"))?,
                surface_loader
                    .get_physical_device_surface_capabilities(physical_device, surface)
                    .map_err(|e| format!("Surface capabilities: {e}"))?,
            )
        };
        let format = formats
            .iter()
            .find(|format| {
                (format.format == vk::Format::B8G8R8A8_UNORM || format.format == vk::Format::R8G8B8A8_UNORM)
                    && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
            .ok_or("No suitable swapchain format found")?;

        // only vk::PresentModeKHR::eFifo (vsync on) is guaranteed
        if state.present_mode != vk::PresentModeKHR::FIFO && !present_modes.contains(&state.present_mode) {
            Log::error(
                Event::Render,
                format!("Requested PresentModeKHR not available ({})", state.present_mode.as_raw()),
            );
            state.present_mode = vk::PresentModeKHR::FIFO;
        }

        // When reading from the swapchain enable source transfer usage.
        let mut image_usage = vk::ImageUsageFlags::COLOR_ATTACHMENT;
        if capabilities.supported_usage_flags.contains(vk::ImageUsageFlags::TRANSFER_SRC) {
            image_usage |= vk::ImageUsageFlags::TRANSFER_SRC;
        }

        // pick surface size
        let extent = if capabilities.current_extent.width != u32::MAX {
            capabilities.current_extent
        } else {
            // update values based on surface limits
            vk::Extent2D {
                width: w.max(capabilities.min_image_extent.width).min(capabilities.max_image_extent.width),
                height: h.max(capabilities.min_image_extent.height).min(capabilities.max_image_extent.height),
            }
        };

        let mut swapchain_image_count = capabilities.min_image_count + 1;
        // check surface limits (0 is unlimited)
        if capabilities.max_image_count > 0 {
            swapchain_image_count = swapchain_image_count.min(capabilities.max_image_count);
        }

        let graphics_queue_index = self.backend.get_graphics_queue_index();
        let present_queue_index = self.backend.get_present_queue_index().unwrap_or(graphics_queue_index);
        let queue_indices = [graphics_queue_index, present_queue_index];
        let composite_alpha = if capabilities.supported_composite_alpha.contains(vk::CompositeAlphaFlagsKHR::INHERIT) {
            vk::CompositeAlphaFlagsKHR::INHERIT
        } else {
            vk::CompositeAlphaFlagsKHR::OPAQUE
        };
        let mut create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(swapchain_image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .present_mode(state.present_mode)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(image_usage)
            .pre_transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
            .clipped(true)
            .composite_alpha(composite_alpha)
            // update this when recreating
            .old_swapchain(state.swapchain);
        create_info = if graphics_queue_index != present_queue_index {
            // TODO if this scenario is widespread and performance is a
            // problem (shouldn't be the case on most hardware) rework to
            // vk::SharingMode::eExclusive + queue ownership
            create_info.image_sharing_mode(vk::SharingMode::CONCURRENT).queue_family_indices(&queue_indices)
        } else {
            create_info.image_sharing_mode(vk::SharingMode::EXCLUSIVE)
        };

        // SAFETY: the create info is valid for the surface, and the old
        // swapchain's images aren't in use (the device is idle when a
        // swapchain is replaced).
        unsafe {
            let swapchain = swapchain_loader
                .create_swapchain(&create_info, None)
                .map_err(|e| format!("Failed to create a swapchain: {e}"))?;
            if state.swapchain != vk::SwapchainKHR::null() {
                swapchain_loader.destroy_swapchain(state.swapchain, None);
            }
            state.swapchain = swapchain;
            state.swapchain_images =
                swapchain_loader.get_swapchain_images(swapchain).map_err(|e| format!("Swapchain images: {e}"))?;
        }
        state.color_format = format.format;
        state.extent = extent;

        // SAFETY: the create infos are valid for the device.
        unsafe {
            for index in 0..self.backend.get_max_frames() {
                let semaphore = device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                    .map_err(|e| format!("Failed to create a semaphore: {e}"))?;
                self.backend.set_debug_name(semaphore, &format!("AcquireSemaphore_{index}"));
                state.acquire_semaphores.push(semaphore);
            }
            for index in 0..state.swapchain_images.len() {
                let semaphore = device
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                    .map_err(|e| format!("Failed to create a semaphore: {e}"))?;
                self.backend.set_debug_name(semaphore, &format!("PresentSemaphore_{index}"));
                state.present_semaphores.push(semaphore);
            }
        }
        Ok(())
    }

    /// `initDepthStencil()`
    fn init_depth_stencil(&self, state: &mut SurfaceState) {
        // check for depth format support
        let formats = [vk::Format::D24_UNORM_S8_UINT, vk::Format::D32_SFLOAT_S8_UINT, vk::Format::D16_UNORM_S8_UINT];
        let format = formats.into_iter().find(|&format| {
            // SAFETY: a plain query of the physical device.
            let properties = unsafe {
                self.backend
                    .get_instance()
                    .get_physical_device_format_properties(self.backend.get_physical_device(), format)
            };
            properties.optimal_tiling_features.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
        });
        let Some(format) = format else {
            Log::error(Event::Render, "Depth/Stencil format not available");
            return;
        };
        state.depth_format = format;

        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width: state.extent.width, height: state.extent.height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let Some(mut allocation) =
            ImageAllocation::create(&self.backend, &image_info, MemoryLocation::GpuOnly, "depth")
        else {
            Log::error(Event::Render, "Vulkan depth texture allocation failed");
            return;
        };
        if !allocation.create_view(
            format,
            vk::ComponentMapping::default(),
            vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL,
            1,
        ) {
            Log::error(Event::Render, "Vulkan depth texture view creation failed");
            return;
        }
        self.backend.set_debug_name(allocation.image(), "SwapchainDepthImage");
        self.backend.set_debug_name(allocation.image_view(), "SwapchainDepthImageView");
        state.depth_allocation = Some(allocation);
    }

    /// `init(w, h)`: the images, their views, the depth and stencil
    /// attachment, the render pass and the framebuffers.
    fn init(&self, context: &Context, w: u32, h: u32) -> bool {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let has_surface = state.surface != vk::SurfaceKHR::null();
        if has_surface {
            if let Err(error) = self.init_swapchain(state, w, h) {
                Log::error(Event::Render, error);
                return false;
            }
        } else {
            self.init_color(state, w, h);
        }

        let device = self.backend.get_device();

        // create swapchain image views
        for (index, &image) in state.swapchain_images.iter().enumerate() {
            let create_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(state.color_format)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            // SAFETY: the image is live and of the color format.
            let Ok(view) = (unsafe { device.create_image_view(&create_info, None) }) else {
                Log::error(Event::Render, "Vulkan color view creation failed");
                return false;
            };
            self.backend.set_debug_name(image, &format!("SwapchainImage_{index}"));
            self.backend.set_debug_name(view, &format!("SwapchainImageView_{index}"));
            state.swapchain_image_views.push(view);
        }

        // depth resources
        self.init_depth_stencil(state);
        let Some(depth_view) = state.depth_allocation.as_ref().map(ImageAllocation::image_view) else {
            return false;
        };

        // create render pass
        state.render_pass = context.get_render_pass(RenderPassKey {
            color_format: state.color_format,
            depth_format: Some(state.depth_format),
            color_final_layout: if has_surface {
                vk::ImageLayout::PRESENT_SRC_KHR
            } else {
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL
            },
        });

        // create swapchain framebuffers
        for &image_view in &state.swapchain_image_views {
            let attachments = [image_view, depth_view];
            let create_info = vk::FramebufferCreateInfo::default()
                .render_pass(state.render_pass)
                .attachments(&attachments)
                .width(state.extent.width)
                .height(state.extent.height)
                .layers(1);
            // SAFETY: the views are live and of the render pass's formats.
            let Ok(framebuffer) = (unsafe { device.create_framebuffer(&create_info, None) }) else {
                Log::error(Event::Render, "Vulkan framebuffer creation failed");
                return false;
            };
            state.swapchain_framebuffers.push(framebuffer);
        }
        true
    }

    /// Destroys what `init` made, but the swapchain and the surface.
    fn destroy_images(&self, state: &mut SurfaceState) {
        let device = self.backend.get_device();
        // SAFETY: the device is idle, and these objects are this
        // resource's.
        unsafe {
            for framebuffer in state.swapchain_framebuffers.drain(..) {
                device.destroy_framebuffer(framebuffer, None);
            }
            for view in state.swapchain_image_views.drain(..) {
                device.destroy_image_view(view, None);
            }
            for semaphore in state.acquire_semaphores.drain(..).chain(state.present_semaphores.drain(..)) {
                device.destroy_semaphore(semaphore, None);
            }
        }
        state.swapchain_images.clear();
        state.depth_allocation = None;
        state.color_allocations.clear();
        state.read_texture = None;
    }

    /// `recreateSwapchain(recreateSurface)`: makes the swapchain, and the
    /// surface if it was lost, again. Returns whether it did.
    pub fn recreate_swapchain(&self, recreate_surface: bool) -> bool {
        let Some(context) = self.context.upgrade() else {
            return false;
        };
        let (width, height) = {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            // without a surface there is no swapchain, unless the surface
            // itself is being recreated
            if state.surface == vk::SurfaceKHR::null() && !recreate_surface {
                return true;
            }
            self.backend.wait_idle("swapchain recreation");
            self.destroy_images(state);

            if recreate_surface {
                // SAFETY: the device is idle, and the swapchain and the
                // surface are this resource's.
                unsafe {
                    if state.swapchain != vk::SwapchainKHR::null()
                        && let Some(loader) = self.backend.get_swapchain_loader()
                    {
                        loader.destroy_swapchain(state.swapchain, None);
                    }
                    if state.surface != vk::SurfaceKHR::null() {
                        self.backend.get_surface_loader().destroy_surface(state.surface, None);
                    }
                }
                state.swapchain = vk::SwapchainKHR::null();
                state.surface = vk::SurfaceKHR::null();
                let Some(provider) = &self.provider else {
                    return false;
                };
                match provider.create_surface(self.backend.entry(), self.backend.get_instance()) {
                    Ok(surface) => state.surface = surface,
                    Err(error) => {
                        Log::error(Event::Render, error);
                        return false;
                    }
                }
            }
            (state.extent.width, state.extent.height)
        };
        self.init(&context, width, height)
    }

    /// `setSize`: the size the swapchain is made with when the surface
    /// doesn't give one.
    pub fn set_extent(&self, size: Size) {
        self.state.borrow_mut().extent = vk::Extent2D { width: size.width, height: size.height };
    }

    /// `queueSurfaceRead()`: has the frame being recorded copied for
    /// `read_image`.
    pub fn queue_surface_read(&self) {
        self.state.borrow_mut().surface_read = true;
    }

    /// Whether the surface's images can be blitted into the read texture,
    /// which also converts their format.
    fn use_blit(&self, state: &SurfaceState) -> bool {
        if state.surface == vk::SurfaceKHR::null() {
            return false;
        }
        let instance = self.backend.get_instance();
        let physical_device = self.backend.get_physical_device();
        // SAFETY: plain queries of the physical device.
        unsafe {
            instance
                .get_physical_device_format_properties(physical_device, state.color_format)
                .optimal_tiling_features
                .contains(vk::FormatFeatureFlags::BLIT_SRC)
                && instance
                    .get_physical_device_format_properties(physical_device, vk::Format::R8G8B8A8_UNORM)
                    .linear_tiling_features
                    .contains(vk::FormatFeatureFlags::BLIT_DST)
        }
    }

    /// `copySurfaceToReadTexture()`: records a copy of the acquired image
    /// into the read texture, at the end of the frame.
    fn copy_surface_to_read_texture(&self, context: &Context) {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let use_blit = self.use_blit(state);
        let has_surface = state.surface != vk::SurfaceKHR::null();
        let Some(&swapchain_image) = state.swapchain_images.get(state.acquired_image_index as usize) else {
            return;
        };
        let size = Size::new(state.extent.width, state.extent.height);
        let read_texture = state.read_texture.get_or_insert_with(|| {
            let mut texture = Texture2D::new(context);
            texture.set_format(TexturePixelType::RGBA, TextureChannelDataType::UnsignedByte);
            texture.set_usage(Texture2DUsage::Read);
            texture
        });
        read_texture.set_size(size);

        let device = self.backend.get_device();
        let command_buffer = context.get_command_buffer();
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let old_layout =
            if has_surface { vk::ImageLayout::PRESENT_SRC_KHR } else { vk::ImageLayout::TRANSFER_SRC_OPTIMAL };
        transition_image(
            device,
            command_buffer,
            swapchain_image,
            range,
            (old_layout, vk::ImageLayout::TRANSFER_SRC_OPTIMAL),
            (vk::AccessFlags::COLOR_ATTACHMENT_WRITE, vk::AccessFlags::TRANSFER_READ),
            (vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT, vk::PipelineStageFlags::TRANSFER),
        );
        if use_blit {
            read_texture.blit_image(swapchain_image, size, 0, 0, command_buffer);
        } else {
            read_texture.copy_image(swapchain_image, size, 0, 0, command_buffer);
        }
        if has_surface {
            transition_image(
                device,
                command_buffer,
                swapchain_image,
                range,
                (vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::ImageLayout::PRESENT_SRC_KHR),
                (vk::AccessFlags::TRANSFER_READ, vk::AccessFlags::empty()),
                (vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::BOTTOM_OF_PIPE),
            );
        }
    }

    /// `readImage()`: the last frame copied for reading, if any.
    pub fn read_image(&self) -> Option<PremultipliedImage> {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let use_blit = self.use_blit(state);
        let mut image = state.read_texture.as_mut()?.read_image()?;
        // swizzle pixels to RGBA
        if !use_blit && state.color_format == vk::Format::B8G8R8A8_UNORM {
            for pixel in image.data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        Some(image)
    }
}

impl RenderableResource for SurfaceRenderableResource {
    fn bind(&self) {}

    /// `swap()`: submits the frame, copying it for `read_image` first if a
    /// read is queued.
    fn swap(&self) {
        let Some(context) = self.context.upgrade() else {
            return;
        };
        if self.read_every_frame {
            self.queue_surface_read();
        }
        let surface_read = self.state.borrow().surface_read;
        if surface_read {
            self.copy_surface_to_read_texture(&context);
        }
        context.submit_frame();
        if surface_read {
            context.wait_frame();
            self.state.borrow_mut().surface_read = false;
        }
    }

    fn get_extent(&self) -> vk::Extent2D {
        self.state.borrow().extent
    }

    fn get_render_pass(&self) -> vk::RenderPass {
        self.state.borrow().render_pass
    }

    fn get_framebuffer(&self) -> vk::Framebuffer {
        let state = self.state.borrow();
        state.swapchain_framebuffers.get(state.acquired_image_index as usize).copied().unwrap_or_default()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Drop for SurfaceRenderableResource {
    /// `destroyResources()`
    fn drop(&mut self) {
        self.backend.wait_idle("surface deletion");
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        if state.surface == vk::SurfaceKHR::null()
            && let Some(context) = self.context.upgrade()
        {
            context.rendering_stats().num_frame_buffers -= 1;
        }
        // specific order
        self.destroy_images(state);
        // SAFETY: the device is idle, and the swapchain and the surface are
        // this resource's.
        unsafe {
            if state.swapchain != vk::SwapchainKHR::null()
                && let Some(loader) = self.backend.get_swapchain_loader()
            {
                loader.destroy_swapchain(state.swapchain, None);
            }
            if state.surface != vk::SurfaceKHR::null() {
                self.backend.get_surface_loader().destroy_surface(state.surface, None);
            }
        }
    }
}
