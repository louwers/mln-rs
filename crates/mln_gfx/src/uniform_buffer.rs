use std::any::Any;
use std::rc::Rc;

use crate::context::Context;
use crate::shaders::shader_defines::MAX_UBO_COUNT_PER_SHADER;

/// A GPU uniform buffer. Created only by backends
/// (`Context::create_uniform_buffer`). `update` takes `&self`: the buffer is
/// shared (`UniformBufferPtr`) and GPU writes go through the queue.
pub trait UniformBuffer: Any {
    fn update(&self, data: &[u8]);
    fn get_size(&self) -> usize;
}

impl dyn UniformBuffer {
    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_ref<T: UniformBuffer>(&self) -> Option<&T> {
        let any: &dyn Any = self;
        any.downcast_ref()
    }
}

pub type UniformBufferPtr = Rc<dyn UniformBuffer>;

/// A collection of uniform buffers by id.
///
/// The C++ class is abstract only for `bind(RenderPass&)` and `copy()`; the
/// WebGPU overrides are no-ops (bind groups are built per drawable in
/// `Drawable::draw`), so this is a plain struct.
#[derive(Clone, Default)]
pub struct UniformBufferArray {
    uniform_buffer_vector: [Option<UniformBufferPtr>; MAX_UBO_COUNT_PER_SHADER],
}

impl UniformBufferArray {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of maximum allocated elements
    #[must_use]
    pub fn allocated_size(&self) -> usize {
        self.uniform_buffer_vector.len()
    }

    /// Get an uniform buffer element, or `None` if it doesn't exist.
    pub fn get(&self, id: usize) -> Option<&UniformBufferPtr> {
        self.uniform_buffer_vector.get(id).and_then(Option::as_ref)
    }

    /// Set a new uniform buffer element or replace the existing one.
    pub fn set(&mut self, id: usize, uniform_buffer: Option<UniformBufferPtr>) -> Option<&UniformBufferPtr> {
        debug_assert!(id < self.uniform_buffer_vector.len());
        if id >= self.uniform_buffer_vector.len() {
            return None;
        }
        self.uniform_buffer_vector[id] = uniform_buffer;
        self.uniform_buffer_vector[id].as_ref()
    }

    /// Create and add a new buffer or update an existing one.
    pub(crate) fn create_or_update(&mut self, id: usize, data: &[u8], context: &dyn Context, persistent: bool) {
        if let Some(ubo) = self.get(id)
            && ubo.get_size() == data.len()
        {
            ubo.update(data);
        } else {
            self.uniform_buffer_vector[id] = Some(context.create_uniform_buffer(data, persistent, false));
        }
    }

    /// `create_or_update` with the bytes of a `Pod` UBO struct.
    pub fn create_or_update_pod<T: bytemuck::Pod>(&mut self, id: usize, data: &T, context: &dyn Context) {
        self.create_or_update(id, bytemuck::bytes_of(data), context, false);
    }

    /// The C++ `operator=(const UniformBufferArray&)`: copy every slot.
    pub fn assign_from(&mut self, other: &UniformBufferArray) {
        self.uniform_buffer_vector.clone_from(&other.uniform_buffer_vector);
    }
}
