// Ports include/mln/webgpu/buffer_resource.hpp and src/mln/webgpu/buffer_resource.cpp

use std::rc::Rc;

use crate::context::Context;

pub type VersionType = u16;

pub struct BufferResource {
    context: Rc<Context>,
    buffer: Option<wgpu::Buffer>,
    size: usize,
    version: VersionType,
}

impl BufferResource {
    /// A buffer of `size` bytes for `usage`, holding `data` if it's given.
    pub fn new(context: Rc<Context>, data: Option<&[u8]>, size: usize, usage: wgpu::BufferUsages) -> Self {
        let mut res = BufferResource { context, buffer: None, size, version: 0 };
        if size == 0 {
            return res;
        }
        let backend = res.context.get_backend();
        let device = backend.device();
        let padded_size = (size + 3) & !3usize;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Buffer Resource"),
            size: padded_size as u64,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if let Some(data) = data {
            // Upload initial data via queue write. C++ falls back to mapping
            // when there is no queue; a device always has one here.
            if padded_size == size {
                backend.write_buffer(&buffer, 0, &data[..size]);
            } else {
                let mut padded = vec![0u8; padded_size];
                padded[..size].copy_from_slice(&data[..size]);
                backend.write_buffer(&buffer, 0, &padded);
            }
        }
        res.context.count_buffer_created(buffer.size());
        res.buffer = Some(buffer);
        res
    }

    /// Port-only: a buffer of the bytes in the JS array `data`, written to
    /// the GPU from there.
    #[cfg(target_family = "wasm")]
    pub fn new_from_host(
        context: Rc<Context>,
        data: &mln_util::host_bytes::HostBytes,
        usage: wgpu::BufferUsages,
    ) -> Self {
        let res = BufferResource::new(context, None, data.len(), usage);
        if let Some(buffer) = &res.buffer {
            res.context.get_backend().write_buffer_from_host(buffer, 0, data);
        }
        res
    }

    pub fn update(&mut self, data: &[u8], offset: usize) {
        let update_size = data.len();
        if update_size == 0 {
            return;
        }
        // Update GPU buffer if we have one
        if let Some(buffer) = &self.buffer {
            self.context.get_backend().write_buffer(buffer, offset as u64, data);
            self.context.count_buffer_update(update_size);
        }
        // Increment version to indicate change
        self.version = self.version.wrapping_add(1);
    }

    #[must_use]
    pub fn get_size_in_bytes(&self) -> usize {
        self.size
    }
    /// The bytes of the GPU buffer, padded from the data size; 0 without one.
    pub fn get_allocated_size(&self) -> u64 {
        self.buffer.as_ref().map_or(0, wgpu::Buffer::size)
    }
    #[must_use]
    pub fn get_context(&self) -> &Rc<Context> {
        &self.context
    }
    #[must_use]
    pub fn get_buffer(&self) -> Option<&wgpu::Buffer> {
        self.buffer.as_ref()
    }
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.buffer.is_some()
    }
    /// Used to detect whether buffer contents have changed
    #[must_use]
    pub fn get_version(&self) -> VersionType {
        self.version
    }
}

impl Drop for BufferResource {
    fn drop(&mut self) {
        if let Some(buffer) = &self.buffer {
            self.context.count_buffer_released(buffer.size());
        }
    }
}
