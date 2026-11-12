// Ports src/mln/gfx/upload_pass.hpp
//
// TODO(port): the typed `createVertexBuffer<Vertex>` / `createIndexBuffer<DrawMode>`
// helpers of the legacy program path.

use std::any::Any;

use mln_util::time::Instant;

use crate::attribute::AttributeBindingArray;
use crate::context::Context;
use crate::debug_group::DebugGroupTarget;
use crate::gfx_types::AttributeDataType;
use crate::types::BufferUsageType;
use crate::vertex_attribute::VertexAttributeArray;
use crate::vertex_buffer::IndexBufferResource;

pub trait UploadPass: DebugGroupTarget + Any {
    fn get_context(&self) -> &dyn Context;

    /// Build the vertex bindings for a drawable: one binding per shader
    /// attribute index, from `overrides` (the drawable's attributes) or a
    /// dummy buffer for missing ones.
    #[allow(clippy::too_many_arguments)]
    fn build_attribute_bindings(
        &self,
        vertex_count: usize,
        vertex_type: AttributeDataType,
        vertex_attribute_index: usize,
        vertex_data: &[u8],
        defaults: &VertexAttributeArray,
        overrides: &mut VertexAttributeArray,
        usage: BufferUsageType,
        last_update: Option<Instant>,
    ) -> AttributeBindingArray;

    fn create_index_buffer_resource(
        &self,
        data: &[u8],
        usage: BufferUsageType,
        persistent: bool,
    ) -> Box<dyn IndexBufferResource>;

    fn as_any(&self) -> &dyn Any;
}
