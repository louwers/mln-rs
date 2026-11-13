// Debug groups exist on Metal command encoders, which belong to the passes,
// so the encoder keeps a stack of names that each pass pushes when it is
// created. C++ also pushes a group onto the passes already open; the
// renderer opens its groups before its passes, so the port leaves that out.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use mln_gfx::command_encoder;
use mln_gfx::debug_group::DebugGroupTarget;
use mln_gfx::frame::DrawTarget;
use mln_gfx::render_pass::RenderPassDescriptor;

use crate::context::Context;
use crate::custom_layer::CustomLayerEncoder;
use crate::draw_target::as_mtl;
use crate::render_pass::RenderPass;
use crate::upload_pass::UploadPass;

pub struct CommandEncoder {
    context: Rc<Context>,
    debug_group_names: RefCell<Vec<String>>,
}

impl CommandEncoder {
    pub fn new(context: Rc<Context>) -> Self {
        CommandEncoder { context, debug_group_names: RefCell::new(Vec::new()) }
    }

    pub fn get_context(&self) -> &Rc<Context> {
        &self.context
    }

    pub fn visit_debug_groups(&self, mut f: impl FnMut(&str)) {
        for name in self.debug_group_names.borrow().iter() {
            f(name);
        }
    }
}

impl DebugGroupTarget for CommandEncoder {
    fn push_debug_group(&self, name: &str) {
        self.debug_group_names.borrow_mut().push(name.to_owned());
    }
    fn pop_debug_group(&self) {
        debug_assert!(!self.debug_group_names.borrow().is_empty());
        self.debug_group_names.borrow_mut().pop();
    }
}

impl command_encoder::CommandEncoder for CommandEncoder {
    fn create_upload_pass(&self, name: &str, target: &dyn DrawTarget) -> Box<dyn mln_gfx::upload_pass::UploadPass> {
        Box::new(UploadPass::new(target, self, name))
    }

    fn create_render_pass(
        &self,
        name: &str,
        descriptor: &RenderPassDescriptor<'_>,
    ) -> Box<dyn mln_gfx::render_pass::RenderPass> {
        Box::new(RenderPass::new(self, name, descriptor))
    }

    /// Lends the command buffer of `target`'s frame, which has no encoder
    /// open between the passes.
    fn lend_to_custom_layer(&self, target: &dyn DrawTarget, f: &mut dyn FnMut(&mut dyn Any)) -> bool {
        let Some(command_buffer) = as_mtl(target).and_then(|target| target.resource().get_command_buffer()) else {
            return false;
        };
        let mut encoder = CustomLayerEncoder { command_buffer };
        f(&mut encoder);
        true
    }
}
