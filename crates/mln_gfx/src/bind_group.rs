use std::rc::Rc;

use crate::context::Context;
use crate::gpu::{BindGroupDesc, BindGroupEntry, BindGroupId, GpuError};
use crate::released::Released;
use crate::uniform_buffer::UniformBufferArray;

/// A bind group of the context's [`Gpu`](crate::gpu::Gpu), made again when
/// what it binds changes. The replaced group, and the last one when this
/// drops, go to the context's release queue.
#[derive(Default)]
pub struct BindGroup {
    entries: Vec<BindGroupEntry>,
    group: Option<(BindGroupId, Rc<Released>)>,
}

impl BindGroup {
    /// The group that binds `entries`.
    pub fn get<C: Context + ?Sized>(
        &mut self,
        context: &C,
        label: &str,
        entries: Vec<BindGroupEntry>,
    ) -> Result<BindGroupId, GpuError> {
        if let Some((group, _)) = &self.group
            && self.entries == entries
        {
            return Ok(*group);
        }
        self.release();
        let group = context.gpu().create_bind_group(&BindGroupDesc { label, entries: &entries })?;
        self.group = Some((group, Rc::clone(&context.context_base().released)));
        self.entries = entries;
        Ok(group)
    }

    fn release(&mut self) {
        if let Some((group, released)) = self.group.take() {
            released.bind_group(group);
        }
    }
}

impl Drop for BindGroup {
    fn drop(&mut self) {
        self.release();
    }
}

impl UniformBufferArray {
    /// An entry for each of the buffers, binding all of it at its id.
    #[must_use]
    pub fn entries(&self) -> Vec<BindGroupEntry> {
        (0..self.allocated_size())
            .filter_map(|id| {
                let buffer = self.get(id)?;
                Some(BindGroupEntry::Buffer {
                    id: id as u32,
                    buffer: buffer.id(),
                    offset: 0,
                    size: buffer.size() as u64,
                })
            })
            .collect()
    }
}
