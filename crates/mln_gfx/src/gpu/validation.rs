//! The checks of [`Gpu`](super::Gpu) calls that every backend makes alike.
//!
//! A backend tells the checks what it knows of its resources through
//! [`Resources`]. It runs [`check_bind_group`] in `create_bind_group`,
//! [`check_pass`] on each pass of a `submit` before it records any, and
//! [`check_readback`] in `read_texture`, so that what reaches its API names
//! live resources of the right usage, within their ranges, and every
//! binding a draw's shader reads.

use std::ops::Range;

use mln_util::size::Size;

use super::{
    BindGroupDesc, BindGroupEntry, BindGroupId, BufferId, BufferUsage, GpuError, Limits, Pass, PipelineId,
    RenderCommand, SamplerId, TargetFormats, TextureFormat, TextureId, TextureRegion, TextureUsage, VertexBufferLayout,
    VertexStep,
};
use crate::vertex_attribute::VertexAttribute;

/// What the checks read of a buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferInfo {
    pub usage: BufferUsage,
    /// The size the caller asked for, rounded up to a multiple of 4: the
    /// bytes that writes, bindings and draws may reach.
    pub size: u64,
}

/// What the checks read of a texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureInfo {
    /// `None` for a texture made elsewhere, of a format with no
    /// [`TextureFormat`]. Such a texture is a color attachment that no
    /// pipeline draws into.
    pub format: Option<TextureFormat>,
    pub size: Size,
    pub usage: TextureUsage,
}

/// A resource that a pipeline's shader reads from the bind groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShaderBinding {
    /// The buffer at uniform or storage buffer id `id`, of which the
    /// shader reads at least `min_size` bytes: of a storage buffer, the
    /// size of each of its elements, of which a draw reads the one at
    /// `globalIndex`. 0 where the backend can't tell.
    Buffer { id: u32, min_size: u64 },
    /// The texture at texture id `id`, and its sampler.
    Texture { id: u32 },
}

/// What the checks read of a pipeline.
#[derive(Clone, Copy, Debug)]
pub struct PipelineInfo<'a> {
    pub targets: TargetFormats,
    pub vertex_buffers: &'a [VertexBufferLayout],
    /// What its shader reads from the bind groups.
    pub bindings: &'a [ShaderBinding],
}

/// A backend's resources, as the checks see them: `None` or `false` for an
/// id the backend doesn't have.
pub trait Resources {
    fn buffer_info(&self, id: BufferId) -> Option<BufferInfo>;
    fn texture_info(&self, id: TextureId) -> Option<TextureInfo>;
    fn has_sampler(&self, id: SamplerId) -> bool;
    fn pipeline_info(&self, id: PipelineId) -> Option<PipelineInfo<'_>>;
    fn bind_group_entries(&self, id: BindGroupId) -> Option<&[BindGroupEntry]>;
}

/// Checks that `desc`'s entries name live resources: buffers of
/// [`Uniform`](BufferUsage::Uniform) or [`Storage`](BufferUsage::Storage)
/// usage, bound from an offset that `limits` aligns, over a range inside
/// the buffer that isn't empty, and [`sampled`](TextureUsage::sampled)
/// textures; and that no two entries bind one id.
pub fn check_bind_group(resources: &impl Resources, limits: Limits, desc: &BindGroupDesc<'_>) -> Result<(), GpuError> {
    let label = desc.label;
    for (index, entry) in desc.entries.iter().enumerate() {
        let repeated = desc.entries[..index].iter().any(|earlier| match *entry {
            BindGroupEntry::Buffer { id, .. } => earlier.binds_buffer(id),
            BindGroupEntry::Texture { id, .. } => earlier.binds_texture(id),
        });
        if repeated {
            return Err(GpuError::Invalid(format!("{label}: two entries bind {entry:?}'s id")));
        }
        match *entry {
            BindGroupEntry::Buffer { id, buffer, offset, size } => {
                let info = resources.buffer_info(buffer).ok_or(GpuError::UnknownId("buffer"))?;
                let invalid = |problem: String| GpuError::Invalid(format!("{label}: buffer id {id}: {problem}"));
                if !matches!(info.usage, BufferUsage::Uniform | BufferUsage::Storage) {
                    return Err(invalid(format!("a {:?} buffer", info.usage)));
                }
                if size == 0 {
                    return Err(invalid("an empty range".to_owned()));
                }
                if !offset.is_multiple_of(u64::from(limits.buffer_offset_alignment.max(1))) {
                    return Err(invalid(format!("an offset of {offset}, which {limits:?} doesn't align")));
                }
                if offset.checked_add(size).is_none_or(|end| end > info.size) {
                    return Err(invalid(format!("{size} bytes at {offset} of a buffer of {}", info.size)));
                }
            }
            BindGroupEntry::Texture { id, texture, sampler } => {
                let info = resources.texture_info(texture).ok_or(GpuError::UnknownId("texture"))?;
                if !info.usage.sampled {
                    return Err(GpuError::Invalid(format!("{label}: texture id {id}: a texture that isn't sampled")));
                }
                if !resources.has_sampler(sampler) {
                    return Err(GpuError::UnknownId("sampler"));
                }
            }
        }
    }
    Ok(())
}

/// A texture that a readback copies out of, and the region it copies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Readback {
    pub format: TextureFormat,
    /// The bytes in each row of the region.
    pub bytes_per_row: usize,
    pub region: TextureRegion,
}

impl Readback {
    /// The bytes of the region's texels.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes_per_row * self.region.size.height as usize
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Checks that `texture` is a live [`copy_src`](TextureUsage::copy_src)
/// texture of a color format, and that `region` is inside it and not empty.
pub fn check_readback(
    resources: &impl Resources,
    texture: TextureId,
    region: TextureRegion,
) -> Result<Readback, GpuError> {
    let info = resources.texture_info(texture).ok_or(GpuError::UnknownId("texture"))?;
    let format = info.format.ok_or_else(|| GpuError::Invalid("a readback of a texture of no known format".into()))?;
    if !info.usage.copy_src {
        return Err(GpuError::Invalid(format!("a readback of a {format:?} texture that isn't copy_src")));
    }
    if region.size.width == 0 || region.size.height == 0 {
        return Err(GpuError::Invalid(format!("a readback of an empty region, {region:?}")));
    }
    let bytes_per_row = region.bytes_per_row(info.size, format)?;
    Ok(Readback { format, bytes_per_row, region })
}

/// The entry that the bind groups in `bound` give `binding`: where two
/// groups bind its id, the one in the higher slot.
pub fn resolve(
    resources: &impl Resources,
    bound: &[Option<BindGroupId>],
    binding: ShaderBinding,
) -> Option<BindGroupEntry> {
    let entries = bound.iter().rev().flatten().filter_map(|&group| resources.bind_group_entries(group)).flatten();
    entries.copied().find(|entry| match binding {
        ShaderBinding::Buffer { id, .. } => entry.binds_buffer(id),
        ShaderBinding::Texture { id } => entry.binds_texture(id),
    })
}

/// The formats of a pass's attachments, as a pipeline's
/// [`TargetFormats`] would name them. A color attachment of a format with
/// no [`TextureFormat`] matches no pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PassFormats {
    has_color: bool,
    /// The color attachment's format, if it has one with a name.
    color: Option<TextureFormat>,
    depth_stencil: Option<TextureFormat>,
}

impl PassFormats {
    fn matches(self, targets: TargetFormats) -> bool {
        self.has_color == targets.color.is_some()
            && self.color == targets.color
            && self.depth_stencil == targets.depth_stencil
    }
}

/// Checks `pass`: its attachments are render targets of one size, a color
/// texture and a depth one, and at least one is given; and each command
/// names live resources. A pipeline draws into attachments of its
/// [`TargetFormats`], a vertex buffer has [`Vertex`](BufferUsage::Vertex)
/// usage and an index buffer [`Index`](BufferUsage::Index) usage, from an
/// offset inside it, even for indices. A draw follows a pipeline, has a
/// buffer in each of its vertex buffer slots, and an index buffer if
/// indexed; the vertices and instances it draws, and the indices, are
/// inside their buffers; and the bound groups give each binding its
/// shader reads, a buffer range of at least the bytes it reads, which of a
/// [`Storage`](BufferUsage::Storage) buffer holds the element at the draw's
/// global index.
pub fn check_pass(resources: &impl Resources, pass: &Pass) -> Result<(), GpuError> {
    let formats = check_attachments(resources, pass)?;
    check_commands(resources, &pass.label, formats, &pass.commands)
}

/// Checks `commands`, a pass's over attachments of `formats`.
fn check_commands(
    resources: &impl Resources,
    label: &str,
    formats: PassFormats,
    commands: &[RenderCommand],
) -> Result<(), GpuError> {
    let invalid = |problem: String| GpuError::Invalid(format!("{label}: {problem}"));
    let mut pipeline: Option<PipelineInfo<'_>> = None;
    let mut bound: Vec<Option<BindGroupId>> = Vec::new();
    let mut vertex_buffers: Vec<Option<(BufferInfo, u64)>> = Vec::new();
    let mut index_buffer: Option<(BufferInfo, u64)> = None;
    let mut global_index = 0;
    // Whether the bindings of the next draw are checked already.
    let mut bindings_checked = false;
    for command in commands {
        match *command {
            RenderCommand::SetPipeline(id) => {
                let info = resources.pipeline_info(id).ok_or(GpuError::UnknownId("pipeline"))?;
                if !formats.matches(info.targets) {
                    return Err(invalid(format!("a pipeline for {:?} in a pass over {formats:?}", info.targets)));
                }
                pipeline = Some(info);
                bindings_checked = false;
            }
            RenderCommand::SetBindGroup { slot, group } => {
                if resources.bind_group_entries(group).is_none() {
                    return Err(GpuError::UnknownId("bind group"));
                }
                set_slot(&mut bound, slot, Some(group));
                bindings_checked = false;
            }
            RenderCommand::ClearBindGroup { slot } => {
                set_slot(&mut bound, slot, None);
                bindings_checked = false;
            }
            RenderCommand::SetVertexBuffer { slot, buffer, offset } => {
                let info = resources.buffer_info(buffer).ok_or(GpuError::UnknownId("buffer"))?;
                if info.usage != BufferUsage::Vertex {
                    return Err(invalid(format!("a {:?} buffer in vertex buffer slot {slot}", info.usage)));
                }
                if offset >= info.size {
                    return Err(invalid(format!("vertex buffer slot {slot} from {offset} of {} bytes", info.size)));
                }
                set_slot(&mut vertex_buffers, slot, Some((info, offset)));
            }
            RenderCommand::SetIndexBuffer { buffer, offset } => {
                let info = resources.buffer_info(buffer).ok_or(GpuError::UnknownId("buffer"))?;
                if info.usage != BufferUsage::Index {
                    return Err(invalid(format!("indices in a {:?} buffer", info.usage)));
                }
                if !offset.is_multiple_of(2) || offset >= info.size {
                    return Err(invalid(format!("indices from {offset} of {} bytes", info.size)));
                }
                index_buffer = Some((info, offset));
            }
            RenderCommand::SetGlobalIndex(index) => {
                global_index = index;
                bindings_checked = false;
            }
            RenderCommand::SetViewport(_)
            | RenderCommand::SetScissor(_)
            | RenderCommand::SetStencilReference(_)
            | RenderCommand::SetBlendConstant(_)
            | RenderCommand::SetLineWidth(_) => {}
            RenderCommand::Draw { ref vertices, ref instances } => {
                let info = pipeline.ok_or_else(|| invalid("a draw before a pipeline".to_owned()))?;
                if !bindings_checked {
                    check_bindings(resources, info, &bound, global_index).map_err(|error| prefix(label, error))?;
                    bindings_checked = true;
                }
                check_vertex_buffers(info, &vertex_buffers, Some(vertices), instances).map_err(invalid)?;
            }
            RenderCommand::DrawIndexed { ref indices, ref instances, .. } => {
                let info = pipeline.ok_or_else(|| invalid("a draw before a pipeline".to_owned()))?;
                if !bindings_checked {
                    check_bindings(resources, info, &bound, global_index).map_err(|error| prefix(label, error))?;
                    bindings_checked = true;
                }
                check_vertex_buffers(info, &vertex_buffers, None, instances).map_err(invalid)?;
                let (buffer, offset) =
                    index_buffer.ok_or_else(|| invalid("an indexed draw without an index buffer".to_owned()))?;
                if !indices.is_empty() && offset + u64::from(indices.end) * 2 > buffer.size {
                    return Err(invalid(format!("indices {indices:?} from {offset} of {} bytes", buffer.size)));
                }
            }
        }
    }
    Ok(())
}

/// Puts `value` in slot `slot` of `slots`, growing it as needed.
fn set_slot<T: Clone>(slots: &mut Vec<Option<T>>, slot: u32, value: Option<T>) {
    let slot = slot as usize;
    if slots.len() <= slot {
        slots.resize(slot + 1, None);
    }
    slots[slot] = value;
}

/// `error` with the pass's label in front.
fn prefix(label: &str, error: GpuError) -> GpuError {
    match error {
        GpuError::Invalid(problem) => GpuError::Invalid(format!("{label}: {problem}")),
        error => error,
    }
}

/// The formats of `pass`'s attachments, which [`check_pass`] describes.
fn check_attachments(resources: &impl Resources, pass: &Pass) -> Result<PassFormats, GpuError> {
    let label = &pass.label;
    let mut size = None;
    let mut formats = PassFormats { has_color: false, color: None, depth_stencil: None };
    if let Some(attachment) = pass.color {
        let info = resources.texture_info(attachment.texture).ok_or(GpuError::UnknownId("texture"))?;
        let color = info.format.is_none_or(|format| format.bytes_per_texel().is_some());
        if !info.usage.render_target || !color {
            return Err(GpuError::Invalid(format!("{label}: the color attachment isn't a color render target")));
        }
        size = Some(info.size);
        formats.has_color = true;
        formats.color = info.format;
    }
    if let Some(attachment) = pass.depth_stencil {
        let info = resources.texture_info(attachment.texture).ok_or(GpuError::UnknownId("texture"))?;
        let format = info.format.filter(|format| format.bytes_per_texel().is_none());
        let Some(format) = format.filter(|_| info.usage.render_target) else {
            return Err(GpuError::Invalid(format!("{label}: the depth attachment isn't a depth render target")));
        };
        if size.is_some_and(|size| size != info.size) {
            return Err(GpuError::Invalid(format!("{label}: attachments of different sizes")));
        }
        size = Some(info.size);
        formats.depth_stencil = Some(format);
    }
    if size.is_none() {
        return Err(GpuError::Invalid(format!("{label}: a pass without attachments")));
    }
    Ok(formats)
}

/// Checks that the groups in `bound` give each binding that `pipeline`'s
/// shader reads, for a draw at `global_index`.
fn check_bindings(
    resources: &impl Resources,
    pipeline: PipelineInfo<'_>,
    bound: &[Option<BindGroupId>],
    global_index: u32,
) -> Result<(), GpuError> {
    for &binding in pipeline.bindings {
        match (binding, resolve(resources, bound, binding)) {
            (ShaderBinding::Buffer { id, min_size }, Some(BindGroupEntry::Buffer { buffer, size, .. })) => {
                let info = resources.buffer_info(buffer).ok_or(GpuError::UnknownId("buffer"))?;
                let elements = if info.usage == BufferUsage::Storage { u64::from(global_index) + 1 } else { 1 };
                if elements.checked_mul(min_size).is_none_or(|read| size < read) {
                    return Err(GpuError::Invalid(format!(
                        "the shader reads {elements} of {min_size} bytes of buffer id {id}, which binds {size}"
                    )));
                }
            }
            (ShaderBinding::Texture { .. }, Some(BindGroupEntry::Texture { texture, sampler, .. })) => {
                resources.texture_info(texture).ok_or(GpuError::UnknownId("texture"))?;
                if !resources.has_sampler(sampler) {
                    return Err(GpuError::UnknownId("sampler"));
                }
            }
            _ => return Err(GpuError::Invalid(format!("the shader reads {binding:?}, which no bind group binds"))),
        }
    }
    Ok(())
}

/// Checks that each of `pipeline`'s vertex buffer slots has a buffer, which
/// holds the `vertices` (of a draw that isn't indexed) and `instances` that
/// a draw reads of it.
fn check_vertex_buffers(
    pipeline: PipelineInfo<'_>,
    vertex_buffers: &[Option<(BufferInfo, u64)>],
    vertices: Option<&Range<u32>>,
    instances: &Range<u32>,
) -> Result<(), String> {
    for (slot, layout) in pipeline.vertex_buffers.iter().enumerate() {
        let (buffer, offset) = vertex_buffers
            .get(slot)
            .copied()
            .flatten()
            .ok_or_else(|| format!("the pipeline reads vertex buffer slot {slot}, which has none"))?;
        let elements = match layout.step {
            VertexStep::Vertex => vertices,
            VertexStep::Instance => Some(instances),
        };
        let Some(elements) = elements.filter(|elements| !elements.is_empty() && !instances.is_empty()) else {
            continue;
        };
        let attributes_end = layout
            .attributes
            .iter()
            .map(|attribute| u64::from(attribute.offset) + VertexAttribute::get_stride_of(attribute.format) as u64)
            .max()
            .unwrap_or(0);
        if attributes_end == 0 {
            continue;
        }
        let end = offset + u64::from(elements.end - 1) * u64::from(layout.stride) + attributes_end;
        if end > buffer.size {
            return Err(format!(
                "a draw of {elements:?} reads {end} bytes of vertex buffer slot {slot}, which has {}",
                buffer.size
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx_types::AttributeDataType;
    use crate::gpu::{ColorAttachment, LoadOp, VertexAttribute as Attribute};

    /// A backend of a few resources, with ids from a map of its own.
    struct Fake {
        buffers: crate::gpu::SlotMap<BufferId, BufferInfo>,
        textures: crate::gpu::SlotMap<TextureId, TextureInfo>,
        pipelines: crate::gpu::SlotMap<PipelineId, (TargetFormats, Vec<VertexBufferLayout>, Vec<ShaderBinding>)>,
        groups: crate::gpu::SlotMap<BindGroupId, Vec<BindGroupEntry>>,
    }

    impl Resources for Fake {
        fn buffer_info(&self, id: BufferId) -> Option<BufferInfo> {
            self.buffers.get(id).copied()
        }
        fn texture_info(&self, id: TextureId) -> Option<TextureInfo> {
            self.textures.get(id).copied()
        }
        fn has_sampler(&self, _: SamplerId) -> bool {
            false
        }
        fn pipeline_info(&self, id: PipelineId) -> Option<PipelineInfo<'_>> {
            self.pipelines.get(id).map(|(targets, vertex_buffers, bindings)| PipelineInfo {
                targets: *targets,
                vertex_buffers,
                bindings,
            })
        }
        fn bind_group_entries(&self, id: BindGroupId) -> Option<&[BindGroupEntry]> {
            self.groups.get(id).map(Vec::as_slice)
        }
    }

    const LIMITS: Limits =
        Limits { buffer_offset_alignment: 256, max_texture_size: 4096, half_float_render_targets: true };

    fn fake() -> Fake {
        Fake {
            buffers: crate::gpu::SlotMap::with_key(),
            textures: crate::gpu::SlotMap::with_key(),
            pipelines: crate::gpu::SlotMap::with_key(),
            groups: crate::gpu::SlotMap::with_key(),
        }
    }

    #[test]
    fn bind_groups_bind_uniform_ranges_inside_their_buffers() {
        let mut gpu = fake();
        let uniform = gpu.buffers.insert(BufferInfo { usage: BufferUsage::Uniform, size: 512 });
        let vertex = gpu.buffers.insert(BufferInfo { usage: BufferUsage::Vertex, size: 512 });
        let check = |gpu: &Fake, buffer, offset, size| {
            let entries = [BindGroupEntry::Buffer { id: 2, buffer, offset, size }];
            check_bind_group(gpu, LIMITS, &BindGroupDesc { label: "test", entries: &entries })
        };
        assert!(check(&gpu, uniform, 256, 256).is_ok());
        assert!(matches!(check(&gpu, uniform, 256, 257), Err(GpuError::Invalid(_))), "past the end");
        assert!(matches!(check(&gpu, uniform, 256, 0), Err(GpuError::Invalid(_))), "empty");
        assert!(matches!(check(&gpu, uniform, 16, 16), Err(GpuError::Invalid(_))), "unaligned");
        assert!(matches!(check(&gpu, uniform, u64::MAX - 255, 512), Err(GpuError::Invalid(_))), "overflows");
        assert!(matches!(check(&gpu, vertex, 0, 16), Err(GpuError::Invalid(_))), "a vertex buffer");
        gpu.buffers.remove(uniform);
        assert!(matches!(check(&gpu, uniform, 0, 16), Err(GpuError::UnknownId("buffer"))));
    }

    #[test]
    fn passes_draw_with_what_their_pipelines_read() {
        let mut gpu = fake();
        let size = Size { width: 4, height: 4 };
        let usage = TextureUsage { render_target: true, ..TextureUsage::default() };
        let color = gpu.textures.insert(TextureInfo { format: Some(TextureFormat::Rgba8Unorm), size, usage });
        let targets = TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None };
        let layout = VertexBufferLayout {
            stride: 4,
            step: VertexStep::Vertex,
            attributes: vec![Attribute { id: 0, format: AttributeDataType::Short2, offset: 0 }],
        };
        let binding = ShaderBinding::Buffer { id: 3, min_size: 64 };
        let pipeline = gpu.pipelines.insert((targets, vec![layout], vec![binding]));
        let vertices = gpu.buffers.insert(BufferInfo { usage: BufferUsage::Vertex, size: 16 });
        let uniform = gpu.buffers.insert(BufferInfo { usage: BufferUsage::Uniform, size: 64 });
        let group = gpu.groups.insert(vec![BindGroupEntry::Buffer { id: 3, buffer: uniform, offset: 0, size: 64 }]);
        let pass = |commands: Vec<RenderCommand>| Pass {
            label: "test".to_owned(),
            color: Some(ColorAttachment { texture: color, load: LoadOp::Load }),
            depth_stencil: None,
            commands,
        };
        let draw = |vertices: Range<u32>| RenderCommand::Draw { vertices, instances: 0..1 };
        let setup = vec![
            RenderCommand::SetPipeline(pipeline),
            RenderCommand::SetBindGroup { slot: 0, group },
            RenderCommand::SetVertexBuffer { slot: 0, buffer: vertices, offset: 0 },
        ];
        assert!(check_pass(&gpu, &pass([setup.clone(), vec![draw(0..4)]].concat())).is_ok());
        assert!(check_pass(&gpu, &pass([setup.clone(), vec![draw(0..5)]].concat())).is_err(), "past the vertices");
        assert!(check_pass(&gpu, &pass([&setup[..2], &[draw(0..4)]].concat())).is_err(), "no vertex buffer");
        assert!(check_pass(&gpu, &pass([&setup[1..], &[draw(0..4)]].concat())).is_err(), "no pipeline");
        let unbound = [&setup[..1], &setup[2..], &[draw(0..4)]].concat();
        assert!(check_pass(&gpu, &pass(unbound)).is_err(), "no uniform buffer");
        let wrong_use = vec![RenderCommand::SetVertexBuffer { slot: 0, buffer: uniform, offset: 0 }];
        assert!(check_pass(&gpu, &pass(wrong_use)).is_err(), "a uniform buffer as vertices");
        let indexed = RenderCommand::DrawIndexed { indices: 0..3, base_vertex: 0, instances: 0..1 };
        assert!(check_pass(&gpu, &pass([setup, vec![indexed]].concat())).is_err(), "no index buffer");
    }

    #[test]
    fn readbacks_copy_a_region_of_a_color_texture() {
        let mut gpu = fake();
        let size = Size { width: 4, height: 2 };
        let copy_src = TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() };
        let color = gpu.textures.insert(TextureInfo { format: Some(TextureFormat::Rgba8Unorm), size, usage: copy_src });
        let depth =
            gpu.textures.insert(TextureInfo { format: Some(TextureFormat::Depth32Float), size, usage: copy_src });
        let usage = TextureUsage { render_target: true, ..TextureUsage::default() };
        let target = gpu.textures.insert(TextureInfo { format: Some(TextureFormat::Rgba8Unorm), size, usage });
        let region = TextureRegion { x: 1, y: 1, size: Size { width: 3, height: 1 } };
        let readback = check_readback(&gpu, color, region).unwrap();
        assert_eq!((readback.bytes_per_row, readback.len()), (12, 12));
        let past = TextureRegion { x: 2, ..region };
        assert!(matches!(check_readback(&gpu, color, past), Err(GpuError::Invalid(_))), "past the edge");
        let empty = TextureRegion { size: Size { width: 0, height: 1 }, ..region };
        assert!(matches!(check_readback(&gpu, color, empty), Err(GpuError::Invalid(_))), "empty");
        assert!(matches!(check_readback(&gpu, depth, region), Err(GpuError::Invalid(_))), "depth");
        assert!(matches!(check_readback(&gpu, target, region), Err(GpuError::Invalid(_))), "not copy_src");
        gpu.textures.remove(color);
        assert!(matches!(check_readback(&gpu, color, region), Err(GpuError::UnknownId("texture"))));
    }
}
