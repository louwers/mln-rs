//! Writes drawables' indexes and vertex attributes to buffers of the
//! backend's [`Gpu`](crate::gpu::Gpu).

use std::rc::Rc;

use crate::attribute::{AttributeBinding, AttributeBindingArray, AttributeDescriptor};
use crate::buffer::{Buffer, BufferPtr};
use crate::context::Context;
use crate::gpu::{BufferUsage, GpuError};
use crate::index_vector::IndexVectorBase;
use crate::vertex_attribute::{VertexAttribute, VertexAttributeArray};
use crate::vertex_vector::VertexVectorBasePtr;

/// Writes `indexes` to their buffer, where they have none or changed since
/// they were written, making one where they have none or don't fit in it.
pub fn upload_indexes(context: &dyn Context, indexes: &mut IndexVectorBase) -> Result<(), GpuError> {
    if indexes.get_buffer().is_some() && !indexes.get_dirty() {
        return Ok(());
    }
    #[cfg(target_family = "wasm")]
    if let Some(data) = indexes.host_data() {
        let buffer = Buffer::create_from_host(context, BufferUsage::Index, data)?;
        indexes.set_buffer(Some(buffer));
        indexes.set_dirty(false);
        return Ok(());
    }
    let data: &[u8] = bytemuck::cast_slice(indexes.data());
    match indexes.get_buffer() {
        Some(buffer) if data.len() <= buffer.size() => buffer.write(context, data)?,
        _ => {
            let buffer = Buffer::create(context, BufferUsage::Index, data)?;
            indexes.set_buffer(Some(buffer));
        }
    }
    indexes.set_dirty(false);
    Ok(())
}

/// The buffer of `vertices`, or `None` where there are none. Where the
/// vertices changed since their buffer was written, or `force`, it is written
/// again, or replaced where they don't fit in it.
pub fn vertex_buffer(
    context: &dyn Context,
    vertices: &VertexVectorBasePtr,
    force: bool,
) -> Result<Option<BufferPtr>, GpuError> {
    let size = vertices.get_raw_count() * vertices.get_raw_size();
    if let Some((buffer, holds)) = vertices.get_buffer()
        && size <= buffer.size()
    {
        // A freed vector's data is already in the buffer.
        if vertices.has_raw_data() && (force || vertices.is_modified_after(holds)) {
            buffer.write(context, &vertices.get_raw_data()[..size])?;
            vertices.set_buffer(Rc::clone(&buffer), vertices.get_last_modified());
        }
        return Ok(Some(buffer));
    }
    if size == 0 {
        return Ok(None);
    }
    debug_assert!(vertices.has_raw_data());
    #[cfg(target_family = "wasm")]
    let host = vertices.get_host_data().map(|data| Buffer::create_from_host(context, BufferUsage::Vertex, &data));
    #[cfg(target_family = "wasm")]
    if let Some(buffer) = host {
        let buffer = buffer?;
        vertices.set_buffer(Rc::clone(&buffer), vertices.get_last_modified());
        return Ok(Some(buffer));
    }
    let buffer = Buffer::create(context, BufferUsage::Vertex, &vertices.get_raw_data()[..size])?;
    vertices.set_buffer(Rc::clone(&buffer), vertices.get_last_modified());
    Ok(Some(buffer))
}

/// The buffer of an attribute's own data, made from it, which it then
/// frees, on first use.
fn attribute_buffer(context: &dyn Context, attribute: &mut VertexAttribute) -> Result<Option<BufferPtr>, GpuError> {
    if let Some(buffer) = attribute.get_buffer() {
        return Ok(Some(Rc::clone(buffer)));
    }
    if attribute.get_raw_data().is_empty() {
        debug_assert!(false, "vertex attribute without data");
        return Ok(None);
    }
    let buffer = Buffer::create(context, BufferUsage::Vertex, attribute.get_raw_data())?;
    attribute.set_buffer(Some(Rc::clone(&buffer)));
    attribute.set_raw_data(Vec::new());
    attribute.set_dirty(false);
    Ok(Some(buffer))
}

/// The binding of an attribute the drawable has, `None` where it has no
/// data.
fn attribute_binding(
    context: &dyn Context,
    attribute: &mut VertexAttribute,
    force: bool,
) -> Result<Option<AttributeBinding>, GpuError> {
    if let Some(shared) = attribute.get_shared_raw_data().cloned() {
        let Some(buffer) = vertex_buffer(context, &shared, force)? else {
            return Ok(None);
        };
        debug_assert!(
            (attribute.get_shared_stride() as usize) * (attribute.get_shared_vertex_offset() as usize)
                < shared.get_raw_size() * shared.get_raw_count()
        );
        return Ok(Some(AttributeBinding {
            attribute: AttributeDescriptor {
                data_type: attribute.get_shared_type(),
                offset: attribute.get_shared_offset(),
            },
            vertex_stride: attribute.get_shared_stride(),
            buffer: Some(buffer),
            vertex_offset: attribute.get_shared_vertex_offset(),
            buffer_index: 0,
        }));
    }
    debug_assert!(attribute.get_stride() > 0);
    Ok(attribute_buffer(context, attribute)?.map(|buffer| AttributeBinding {
        attribute: AttributeDescriptor { data_type: attribute.get_data_type(), offset: 0 },
        vertex_stride: attribute.get_stride() as u32,
        buffer: Some(buffer),
        vertex_offset: 0,
        buffer_index: 0,
    }))
}

/// The vertex bindings of a drawable, indexed by shader attribute index:
/// one for each attribute of the shader's `defaults`, read from the
/// drawable's attribute in `overrides` where it has one. An attribute the
/// drawable doesn't have, or has no data for, binds no buffer, at the
/// default's type and stride. `force` writes shared vertex data that
/// appears unchanged (see [`vertex_buffer`]).
pub fn build_attribute_bindings(
    context: &dyn Context,
    defaults: &VertexAttributeArray,
    overrides: &mut VertexAttributeArray,
    force: bool,
) -> Result<AttributeBindingArray, GpuError> {
    let mut bindings: AttributeBindingArray = vec![None; defaults.allocated_size()];
    let mut result = Ok(());
    let resolve = |_id: usize, default: &VertexAttribute, override_: Option<&mut VertexAttribute>| {
        if result.is_err() {
            return;
        }
        let binding = match override_ {
            Some(attribute) => attribute_binding(context, attribute, force),
            None => Ok(None),
        };
        let binding = match binding {
            Ok(binding) => binding.unwrap_or_else(|| AttributeBinding {
                attribute: AttributeDescriptor { data_type: default.get_data_type(), offset: 0 },
                vertex_stride: VertexAttribute::get_stride_of(default.get_data_type()) as u32,
                buffer: None,
                vertex_offset: 0,
                buffer_index: 0,
            }),
            Err(error) => {
                result = Err(error);
                return;
            }
        };
        let index = default.get_index() as usize;
        if bindings.len() <= index {
            bindings.resize(index + 1, None);
        }
        bindings[index] = Some(binding);
    };
    // An attribute the drawable has and the shader doesn't read.
    let unused = |_id: usize, attribute: &mut VertexAttribute| attribute.set_dirty(false);
    defaults.resolve(overrides, resolve, unused);
    result.map(|()| bindings)
}
