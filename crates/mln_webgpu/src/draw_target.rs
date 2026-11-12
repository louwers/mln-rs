// The WebGPU side of `gfx::DrawTarget`: the attachments a render pass draws
// into, and what a frame records before it is submitted.

use std::any::Any;
use std::cell::Cell;
use std::rc::Rc;

use mln_gfx::frame;
use mln_util::size::Size;

/// What a WebGPU draw target is made of.
pub trait TargetResource {
    /// Makes the attachments before a pass draws into them.
    fn bind(&self) {}

    /// Records commands at the end of the frame that is about to be
    /// submitted, after its passes, into the same submission.
    fn finish_frame(&self, _encoder: &mut wgpu::CommandEncoder) {}

    /// The color attachment view for render passes targeting this resource.
    fn get_color_texture_view(&self) -> Option<wgpu::TextureView>;
    fn get_color_texture_format(&self) -> Option<wgpu::TextureFormat>;
    /// The depth/stencil view, if the resource manages one.
    fn get_depth_stencil_texture_view(&self) -> Option<wgpu::TextureView> {
        None
    }
    fn get_depth_stencil_texture_format(&self) -> Option<wgpu::TextureFormat> {
        None
    }

    fn as_any(&self) -> &dyn Any;
}

/// A resource that something else keeps a hold of too.
impl<T: TargetResource + ?Sized> TargetResource for Rc<T> {
    fn bind(&self) {
        (**self).bind();
    }
    fn finish_frame(&self, encoder: &mut wgpu::CommandEncoder) {
        (**self).finish_frame(encoder);
    }
    fn get_color_texture_view(&self) -> Option<wgpu::TextureView> {
        (**self).get_color_texture_view()
    }
    fn get_color_texture_format(&self) -> Option<wgpu::TextureFormat> {
        (**self).get_color_texture_format()
    }
    fn get_depth_stencil_texture_view(&self) -> Option<wgpu::TextureView> {
        (**self).get_depth_stencil_texture_view()
    }
    fn get_depth_stencil_texture_format(&self) -> Option<wgpu::TextureFormat> {
        (**self).get_depth_stencil_texture_format()
    }
    fn as_any(&self) -> &dyn Any {
        (**self).as_any()
    }
}

/// The `gfx::DrawTarget` of every WebGPU target.
pub struct DrawTarget {
    size: Cell<Size>,
    resource: Box<dyn TargetResource>,
}

impl DrawTarget {
    pub fn new(size: Size, resource: impl TargetResource + 'static) -> Self {
        DrawTarget { size: Cell::new(size), resource: Box::new(resource) }
    }

    pub fn resource(&self) -> &dyn TargetResource {
        &*self.resource
    }

    pub(crate) fn set_size(&self, size: Size) {
        self.size.set(size);
    }
}

impl frame::DrawTarget for DrawTarget {
    fn size(&self) -> Size {
        self.size.get()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `target` as a WebGPU draw target.
pub fn as_webgpu(target: &dyn frame::DrawTarget) -> Option<&DrawTarget> {
    target.as_any().downcast_ref::<DrawTarget>()
}

/// The resource of `target`, if it is a `T`.
pub fn downcast_resource<T: 'static>(target: &dyn frame::DrawTarget) -> Option<&T> {
    as_webgpu(target)?.resource().as_any().downcast_ref::<T>()
}
