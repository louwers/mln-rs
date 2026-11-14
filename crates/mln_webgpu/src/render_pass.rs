use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use mln_gfx::context::Context as _;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::gpu::{GpuError, RenderCommand, TargetFormats};
use mln_gfx::render_pass::{self, RenderPassDescriptor};
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use crate::command_encoder::CommandEncoder;
use crate::context::Context;
use crate::custom_layer::CustomLayerPass;
use crate::draw_target::as_webgpu;
use crate::gpu::from_wgpu_format;

pub(crate) struct RenderPass {
    context: Rc<Context>,
    renderable_size: Size,
    encoder: RefCell<Option<wgpu::RenderPass<'static>>>,
    /// The formats of the pass's attachments.
    color_format: Option<wgpu::TextureFormat>,
    depth_stencil_format: Option<wgpu::TextureFormat>,
}

fn debug_groups_enabled() -> bool {
    // Debug groups on render pass encoders can cause issues in some Dawn
    // versions; enable with an environment variable if needed.
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("WEBGPU_ENABLE_DEBUG_GROUPS").is_some())
}

impl RenderPass {
    pub(crate) fn new(command_encoder: &CommandEncoder, name: &str, descriptor: &RenderPassDescriptor<'_>) -> Self {
        let context = Rc::clone(command_encoder.get_context());
        let mut pass = RenderPass {
            context: Rc::clone(&context),
            renderable_size: descriptor.target.size(),
            encoder: RefCell::new(None),
            color_format: None,
            depth_stencil_format: None,
        };

        let mut encoder_guard = command_encoder.get_encoder();
        let Some(command_encoder) = encoder_guard.as_mut() else {
            Log::error(Event::Render, "WebGPU: Command encoder unavailable");
            return pass;
        };

        // Access the draw target associated with this pass
        let Some(target) = as_webgpu(descriptor.target) else {
            Log::error(Event::Render, "WebGPU: The pass's draw target is not a WebGPU one");
            return pass;
        };
        let renderable_resource = target.resource();
        renderable_resource.bind();

        // Resolve the color attachment view.
        let Some(color_view) = renderable_resource.get_color_texture_view() else {
            Log::error(Event::Render, "WebGPU: Failed to acquire color attachment view");
            return pass;
        };
        pass.color_format = renderable_resource.get_color_texture_format();

        let color_ops = wgpu::Operations {
            load: match descriptor.clear_color {
                Some(c) => wgpu::LoadOp::Clear(wgpu::Color {
                    r: f64::from(c.r),
                    g: f64::from(c.g),
                    b: f64::from(c.b),
                    a: f64::from(c.a),
                }),
                None => wgpu::LoadOp::Load,
            },
            store: wgpu::StoreOp::Store,
        };

        let depth_view = renderable_resource.get_depth_stencil_texture_view();
        pass.depth_stencil_format = depth_view.as_ref().and(renderable_resource.get_depth_stencil_texture_format());
        let depth_stencil_attachment = depth_view.as_ref().map(|view| wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(wgpu::Operations {
                load: match descriptor.clear_depth {
                    Some(d) => wgpu::LoadOp::Clear(d),
                    None => wgpu::LoadOp::Load,
                },
                store: wgpu::StoreOp::Store,
            }),
            // Without a stencil clear the stencil aspect is read-only.
            stencil_ops: descriptor
                .clear_stencil
                .map(|s| wgpu::Operations { load: wgpu::LoadOp::Clear(s as u32), store: wgpu::StoreOp::Store }),
        });

        let encoder = command_encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(name),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: color_ops,
                })],
                depth_stencil_attachment,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        drop(encoder_guard);

        context.perform_cleanup();
        *pass.encoder.borrow_mut() = Some(encoder);
        pass
    }
}

impl DebugGroupTarget for RenderPass {
    fn push_debug_group(&self, name: &str) {
        if debug_groups_enabled()
            && let Some(encoder) = self.encoder.borrow_mut().as_mut()
        {
            encoder.push_debug_group(name);
        }
    }

    fn pop_debug_group(&self) {
        if debug_groups_enabled()
            && let Some(encoder) = self.encoder.borrow_mut().as_mut()
        {
            encoder.pop_debug_group();
        }
    }
}

impl render_pass::RenderPass for RenderPass {
    fn target(&self) -> Option<(Size, TargetFormats)> {
        let color = from_wgpu_format(self.color_format?)?;
        let depth_stencil = match self.depth_stencil_format {
            Some(format) => Some(from_wgpu_format(format)?),
            None => None,
        };
        Some((self.renderable_size, TargetFormats { color: Some(color), depth_stencil }))
    }

    fn encode(&self, commands: &[RenderCommand]) -> Result<(), GpuError> {
        let (size, targets) =
            self.target().ok_or_else(|| GpuError::Invalid("a render pass that didn't begin".into()))?;
        let mut encoder = self.encoder.borrow_mut();
        let pass = encoder.as_mut().ok_or_else(|| GpuError::Invalid("a render pass that didn't begin".into()))?;
        self.context.get_backend().gpu().borrow_mut().encode_into(pass, size, targets, commands)
    }

    /// Moves the pass to the host for the call and back.
    fn lend_to_custom_layer(&self, f: &mut dyn FnMut(&mut dyn Any)) -> bool {
        let backend = self.context.get_backend();
        let Some(color_format) = self.color_format else {
            return false;
        };
        let (device, queue) = (backend.device().clone(), backend.queue().clone());
        let mut encoder = self.encoder.borrow_mut();
        let Some(pass) = encoder.take() else {
            return false;
        };
        let mut lent = CustomLayerPass {
            device,
            queue,
            pass,
            color_format,
            depth_stencil_format: self.depth_stencil_format,
            sample_count: 1,
        };
        f(&mut lent);
        *encoder = Some(lent.pass);
        true
    }

    fn add_debug_signpost(&self, name: &str) {
        if let Some(encoder) = self.encoder.borrow_mut().as_mut() {
            encoder.insert_debug_marker(name);
        }
    }
}
