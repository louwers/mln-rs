// Port-only: after a read, the frame after it is copied out as soon as it
// is submitted, so that a host reading frames back finds the next one's
// copy started (C++ submits the copy at each read).
//
// There is no size-changed callback (`handleResize` / `setSizeListener`):
// an offscreen texture keeps its size, and a render target of another size
// makes a new one.

use std::cell::{Cell, RefMut};
use std::rc::Rc;

use mln_gfx::command_encoder::CommandEncoder as GfxCommandEncoder;
use mln_gfx::context::Context as _;
use mln_gfx::gpu::{Gpu as _, GpuError, ReadbackId, TextureRegion};
use mln_gfx::texture2d::{SamplerState, Texture2DPtr, texture_format};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_gfx::{frame, offscreen_texture};
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::WebGpu;
use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::draw_target::{DrawTarget, TargetResource};

fn linear_clamp() -> SamplerState {
    SamplerState {
        filter: TextureFilterType::Linear,
        wrap_u: TextureWrapType::Clamp,
        wrap_v: TextureWrapType::Clamp,
        ..SamplerState::default()
    }
}

pub(crate) struct OffscreenTextureResource {
    context: Rc<Context>,
    size: Size,
    /// A texture of the backend's `Gpu`, which the renderer samples.
    color_texture: Texture2DPtr,
    /// The depth or depth-stencil attachment, and its view.
    depth_texture: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// The color texture was read back: the next frame copies it out.
    read_back: Cell<bool>,
    /// The copy of the color texture that a submission
    /// (`Context::submissions`) ended with.
    copy: Cell<Option<(u64, ReadbackId)>>,
}

impl OffscreenTextureResource {
    pub(crate) fn new(
        context: Rc<Context>,
        size: Size,
        type_: TextureChannelDataType,
        depth: bool,
        stencil: bool,
    ) -> Self {
        debug_assert!(size.width != 0 && size.height != 0);
        let color_texture = context.create_texture_2d();
        {
            let mut texture = color_texture.borrow_mut();
            texture.set_size(size);
            texture.set_format(texture_format(TexturePixelType::RGBA, type_));
            texture.set_render_target();
            texture.set_sampler(linear_clamp());
        }
        // WebGPU requires a combined depth-stencil texture when both are
        // needed.
        let depth_texture = (depth || stencil).then(|| {
            let format = if depth && !stencil {
                wgpu::TextureFormat::Depth32Float
            } else {
                wgpu::TextureFormat::Depth24PlusStencil8
            };
            let texture = context.get_backend().device().create_texture(&wgpu::TextureDescriptor {
                label: Some("Offscreen depth"),
                size: wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            (texture, view)
        });
        // Port-only: the C++ WebGPU backend only decrements this count.
        context.rendering_stats_mut().num_frame_buffers += 1;
        OffscreenTextureResource {
            context,
            size,
            color_texture,
            depth_texture,
            read_back: Cell::default(),
            copy: Cell::default(),
        }
    }

    /// The color texture's wgpu texture, made on first use.
    fn color(&self) -> Option<wgpu::Texture> {
        let created = self.color_texture.borrow_mut().create(&*self.context);
        created.ok().and_then(|texture| self.context.get_backend().native_texture(texture)).map(|(texture, _)| texture)
    }

    fn gpu(&self) -> RefMut<'_, WebGpu> {
        self.context.get_backend().gpu().borrow_mut()
    }

    /// Starts copying the color texture out, if it exists.
    fn copy(&self) -> Result<ReadbackId, GpuError> {
        let texture = self.color_texture.borrow_mut().create(&*self.context)?;
        self.gpu().read_texture(texture, TextureRegion::whole(self.size))
    }

    /// Copies out the frame just submitted if a read asked for it: a host
    /// reading frames back reads the next one too.
    pub(crate) fn copy_requested(&self) {
        if !self.read_back.replace(false) {
            return;
        }
        match self.copy() {
            Ok(copy) => self.replace_copy(Some((self.context.submissions(), copy))),
            Err(error) => Log::error(Event::Render, format!("WebGPU: copying the frame out failed: {error}")),
        }
    }

    fn replace_copy(&self, copy: Option<(u64, ReadbackId)>) {
        if let Some((_, stale)) = self.copy.replace(copy) {
            self.gpu().destroy_readback(stale);
        }
    }

    /// The color texture's pixels: the copy the last submission ended with
    /// (`copy_requested`), or else one made now.
    pub(crate) fn read_still_image(&self) -> PremultipliedImage {
        self.read_back.set(true);
        let submissions = self.context.submissions();
        let read = || {
            // Any later submission may have drawn the texture again.
            let copy = match self.copy.take() {
                Some((at, copy)) if at == submissions => copy,
                stale => {
                    self.replace_copy(stale);
                    self.copy()?
                }
            };
            let texels = self.gpu().take_readback(copy, true)?;
            texels.ok_or_else(|| {
                self.gpu().destroy_readback(copy);
                GpuError::Unsupported("waiting for a readback".to_owned())
            })
        };
        match read() {
            Ok(texels) => PremultipliedImage::from_vec(self.size, texels),
            Err(error) => {
                Log::error(Event::Render, format!("WebGPU: reading the color texture back failed: {error}"));
                PremultipliedImage::new(self.size)
            }
        }
    }

    pub(crate) fn get_texture(&self) -> Texture2DPtr {
        Rc::clone(&self.color_texture)
    }
}

impl Drop for OffscreenTextureResource {
    fn drop(&mut self) {
        self.replace_copy(None);
        self.context.rendering_stats_mut().num_frame_buffers -= 1;
    }
}

impl TargetResource for OffscreenTextureResource {
    fn frame_submitted(&self) {
        self.copy_requested();
    }

    fn get_color_texture_view(&self) -> Option<wgpu::TextureView> {
        let created = self.color_texture.borrow_mut().create(&*self.context);
        created.ok().and_then(|texture| self.context.get_backend().native_texture(texture)).map(|(_, view)| view)
    }

    fn get_color_texture_format(&self) -> Option<wgpu::TextureFormat> {
        self.color().map(|texture| texture.format())
    }

    fn get_depth_stencil_texture_view(&self) -> Option<wgpu::TextureView> {
        self.depth_texture.as_ref().map(|(_, view)| view.clone())
    }

    fn get_depth_stencil_texture_format(&self) -> Option<wgpu::TextureFormat> {
        self.depth_texture.as_ref().map(|(texture, _)| texture.format())
    }
}

pub(crate) struct OffscreenTexture {
    target: DrawTarget,
    resource: Rc<OffscreenTextureResource>,
}

impl OffscreenTexture {
    pub(crate) fn new(
        context: Rc<Context>,
        size: Size,
        type_: TextureChannelDataType,
        depth: bool,
        stencil: bool,
    ) -> Self {
        let resource = Rc::new(OffscreenTextureResource::new(context, size, type_, depth, stencil));
        OffscreenTexture { target: DrawTarget::new(size, Rc::<OffscreenTextureResource>::clone(&resource)), resource }
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
        self.resource.read_still_image()
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
