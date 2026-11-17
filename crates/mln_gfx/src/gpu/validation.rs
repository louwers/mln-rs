//! The checks of [`Gpu`](super::Gpu) calls that every backend makes alike.
//!
//! What a draw reads is checked where it is made: a backend runs
//! [`check_bindings`] in `create_bindings` and [`check_geometry`] in
//! `create_geometry`, with what it knows of the resources they name, and
//! keeps what they return. A draw then needs only [`check_draw`]'s
//! comparisons of what the pipeline, bindings and geometry it names were
//! made for. [`check_pass`] checks a pass's attachments in `begin_pass`,
//! [`check_readback`] a readback in `read_texture`, and [`check_write`] a
//! write in `write_buffer`.

use mln_util::hash::HashMap;
use mln_util::size::Size;

use super::{
    BindGroupEntry, BindingsDesc, BufferId, BufferUsage, Draw, GeometryDesc, GpuError, PassDesc, SamplerId, ShaderId,
    TargetFormats, TextureFormat, TextureId, TextureRegion, TextureUsage, VertexBufferLayout, VertexStep,
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

/// A resource that a shader reads from its bindings.
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

/// A backend's resources, as the checks of bindings and geometry see them:
/// `None` or `false` for an id the backend doesn't have.
pub struct Lookup<'a> {
    pub buffer: &'a dyn Fn(BufferId) -> Option<BufferInfo>,
    pub texture: &'a dyn Fn(TextureId) -> Option<TextureInfo>,
    pub sampler: &'a dyn Fn(SamplerId) -> bool,
}

/// Checks `desc`, whose shader reads `reads`: its entries name live
/// resources, buffers of [`Uniform`](BufferUsage::Uniform) or
/// [`Storage`](BufferUsage::Storage) usage bound from an offset that
/// `alignment` aligns, over a range inside the buffer that isn't empty, and
/// [`sampled`](TextureUsage::sampled) textures; no two entries bind one id;
/// and an entry binds each of `reads`, a buffer range of at least the bytes
/// the shader reads. Returns how many elements of the storage buffers the
/// shader reads that the entries bind, the global indices a draw may have:
/// `u32::MAX` where it reads none.
pub fn check_bindings(
    desc: &BindingsDesc<'_>,
    reads: &[ShaderBinding],
    alignment: u32,
    lookup: &Lookup<'_>,
) -> Result<u32, GpuError> {
    let label = desc.label;
    for (index, entry) in desc.entries.iter().enumerate() {
        if desc.entries[..index].iter().any(|earlier| same_id(earlier, entry)) {
            return Err(GpuError::Invalid(format!("{label}: two entries bind {entry:?}'s id")));
        }
        match *entry {
            BindGroupEntry::Buffer { id, buffer, offset, size } => {
                let info = (lookup.buffer)(buffer).ok_or(GpuError::UnknownId("buffer"))?;
                let invalid = |problem: String| GpuError::Invalid(format!("{label}: buffer id {id}: {problem}"));
                if !matches!(info.usage, BufferUsage::Uniform | BufferUsage::Storage) {
                    return Err(invalid(format!("a {:?} buffer", info.usage)));
                }
                if size == 0 {
                    return Err(invalid("an empty range".to_owned()));
                }
                if !offset.is_multiple_of(u64::from(alignment.max(1))) {
                    return Err(invalid(format!("an offset of {offset}, which {alignment} doesn't align")));
                }
                if offset.checked_add(size).is_none_or(|end| end > info.size) {
                    return Err(invalid(format!("{size} bytes at {offset} of a buffer of {}", info.size)));
                }
            }
            BindGroupEntry::Texture { id, texture, sampler } => {
                let info = (lookup.texture)(texture).ok_or(GpuError::UnknownId("texture"))?;
                if !info.usage.sampled {
                    return Err(GpuError::Invalid(format!("{label}: texture id {id}: a texture that isn't sampled")));
                }
                if !(lookup.sampler)(sampler) {
                    return Err(GpuError::UnknownId("sampler"));
                }
            }
        }
    }

    let mut elements = u32::MAX;
    for &binding in reads {
        let entry = desc.entries.iter().find(|entry| match (binding, entry) {
            (ShaderBinding::Buffer { id, .. }, BindGroupEntry::Buffer { id: entry, .. })
            | (ShaderBinding::Texture { id }, BindGroupEntry::Texture { id: entry, .. }) => id == *entry,
            _ => false,
        });
        let Some(&BindGroupEntry::Buffer { buffer, size, .. }) = entry else {
            if entry.is_none() {
                return Err(GpuError::Invalid(format!("{label}: the shader reads {binding:?}, which no entry binds")));
            }
            continue;
        };
        let ShaderBinding::Buffer { id, min_size } = binding else { continue };
        if size < min_size {
            return Err(GpuError::Invalid(format!(
                "{label}: the shader reads {min_size} bytes of buffer id {id}, which binds {size}"
            )));
        }
        let usage = (lookup.buffer)(buffer).ok_or(GpuError::UnknownId("buffer"))?.usage;
        if usage == BufferUsage::Storage && min_size > 0 {
            elements = elements.min(u32::try_from(size / min_size).unwrap_or(u32::MAX));
        }
    }
    Ok(elements)
}

/// Whether two entries bind the same id.
fn same_id(a: &BindGroupEntry, b: &BindGroupEntry) -> bool {
    match (a, b) {
        (BindGroupEntry::Buffer { id: a, .. }, BindGroupEntry::Buffer { id: b, .. })
        | (BindGroupEntry::Texture { id: a, .. }, BindGroupEntry::Texture { id: b, .. }) => a == b,
        _ => false,
    }
}

/// Checks a [`write_buffer`](super::Gpu::write_buffer) of `len` bytes at
/// `offset` into a buffer of `size` bytes: that `offset` and `len` are
/// multiples of 4, and that the bytes end inside the buffer.
pub fn check_write(offset: u64, len: usize, size: u64) -> Result<(), GpuError> {
    let len = len as u64;
    if !offset.is_multiple_of(4) || !len.is_multiple_of(4) {
        return Err(GpuError::Invalid(format!("a write of {len} bytes at {offset}, not multiples of 4")));
    }
    if offset.checked_add(len).is_none_or(|end| end > size) {
        return Err(GpuError::Invalid(format!("a write of {len} bytes at {offset} to a buffer of {size}")));
    }
    Ok(())
}

/// How much of its buffers geometry holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeometryRanges {
    /// The indices its index buffer holds.
    pub indices: u32,
    /// The instances its per-instance buffers hold, `u32::MAX` where it
    /// has none.
    pub instances: u32,
}

/// Checks `desc`: a buffer for each layout, of
/// [`Vertex`](BufferUsage::Vertex) usage from an offset inside it, and an
/// index buffer of [`Index`](BufferUsage::Index) usage. Returns what the
/// buffers hold.
pub fn check_geometry(desc: &GeometryDesc<'_>, lookup: &Lookup<'_>) -> Result<GeometryRanges, GpuError> {
    if desc.vertex_buffers.len() != desc.layouts.len() {
        return Err(GpuError::Invalid(format!(
            "{} vertex buffers for {} layouts",
            desc.vertex_buffers.len(),
            desc.layouts.len()
        )));
    }
    let mut instances = u32::MAX;
    for (slot, (&(buffer, offset), layout)) in desc.vertex_buffers.iter().zip(desc.layouts).enumerate() {
        let info = (lookup.buffer)(buffer).ok_or(GpuError::UnknownId("buffer"))?;
        if info.usage != BufferUsage::Vertex {
            return Err(GpuError::Invalid(format!("a {:?} buffer in vertex buffer slot {slot}", info.usage)));
        }
        if offset >= info.size {
            return Err(GpuError::Invalid(format!("vertex buffer slot {slot} from {offset} of {} bytes", info.size)));
        }
        if layout.step == VertexStep::Instance {
            instances = instances.min(elements(layout, info.size - offset));
        }
    }
    let index = (lookup.buffer)(desc.index_buffer).ok_or(GpuError::UnknownId("buffer"))?;
    if index.usage != BufferUsage::Index {
        return Err(GpuError::Invalid(format!("indices in a {:?} buffer", index.usage)));
    }
    Ok(GeometryRanges { indices: u32::try_from(index.size / 2).unwrap_or(u32::MAX), instances })
}

/// How many elements of `layout` the `bytes` of a buffer hold.
fn elements(layout: &VertexBufferLayout, bytes: u64) -> u32 {
    let end = layout
        .attributes
        .iter()
        .map(|attribute| u64::from(attribute.offset) + VertexAttribute::get_stride_of(attribute.format) as u64)
        .max()
        .unwrap_or(0);
    if end == 0 {
        return u32::MAX;
    }
    if bytes < end {
        return 0;
    }
    let count = match layout.stride {
        0 => u64::from(u32::MAX),
        stride => (bytes - end) / u64::from(stride) + 1,
    };
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// A number for each pair of a shader and the vertex buffer layouts its
/// pipelines read, so that geometry and the pipelines that draw it can
/// tell theirs equal with one comparison.
#[derive(Default)]
pub struct LayoutTags {
    /// The tag of each set of layouts, by shader.
    tags: HashMap<ShaderId, Vec<(Vec<VertexBufferLayout>, u32)>>,
    next: u32,
}

impl LayoutTags {
    /// The tag of `shader` reading `layouts`, the same for every equal
    /// pair.
    pub fn tag(&mut self, shader: ShaderId, layouts: &[VertexBufferLayout]) -> u32 {
        let tags = self.tags.entry(shader).or_default();
        if let Some(&(_, tag)) = tags.iter().find(|(made, _)| made == layouts) {
            return tag;
        }
        let tag = self.next;
        self.next += 1;
        tags.push((layouts.to_vec(), tag));
        tag
    }
}

/// What a draw's checks read of its pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipelineTags {
    pub shader: ShaderId,
    /// Its [`LayoutTags`] tag.
    pub layout: u32,
    pub targets: TargetFormats,
}

/// What a draw's checks read of its bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingsTags {
    pub shader: ShaderId,
    /// What [`check_bindings`] returned.
    pub elements: u32,
}

/// What a draw's checks read of its geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeometryTags {
    /// Its [`LayoutTags`] tag.
    pub layout: u32,
    pub ranges: GeometryRanges,
}

/// Checks `draw`, into a pass over attachments of `targets` (`None` where
/// one has a format with no [`TextureFormat`]), of a pipeline, bindings and
/// geometry of these tags: they were made for one shader and layout, the
/// pipeline for the attachments, and the draw's indices, instances and
/// global index are inside them.
pub fn check_draw(
    draw: &Draw,
    targets: Option<TargetFormats>,
    pipeline: &PipelineTags,
    bindings: &BindingsTags,
    geometry: &GeometryTags,
) -> Result<(), GpuError> {
    if targets != Some(pipeline.targets) {
        return Err(GpuError::Invalid(format!("a pipeline for {:?} in a pass over {targets:?}", pipeline.targets)));
    }
    if bindings.shader != pipeline.shader {
        return Err(GpuError::Invalid("bindings of another shader than the pipeline's".to_owned()));
    }
    if geometry.layout != pipeline.layout {
        return Err(GpuError::Invalid("geometry of other vertex layouts than the pipeline's".to_owned()));
    }
    let past = |first: u32, count: u32, limit: u32| u64::from(first) + u64::from(count) > u64::from(limit);
    if draw.index_count > 0 && past(draw.first_index, draw.index_count, geometry.ranges.indices) {
        return Err(GpuError::Invalid(format!(
            "{} indices from {} of {}",
            draw.index_count, draw.first_index, geometry.ranges.indices
        )));
    }
    if draw.instance_count > 0 && past(draw.first_instance, draw.instance_count, geometry.ranges.instances) {
        return Err(GpuError::Invalid(format!(
            "{} instances from {} of {}",
            draw.instance_count, draw.first_instance, geometry.ranges.instances
        )));
    }
    if draw.global_index >= bindings.elements {
        return Err(GpuError::Invalid(format!(
            "global index {} of {} bound elements",
            draw.global_index, bindings.elements
        )));
    }
    Ok(())
}

/// What a pass draws into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassTarget {
    pub size: Size,
    /// The formats of its attachments, `None` where one has a format with
    /// no [`TextureFormat`], which no pipeline draws into.
    pub targets: Option<TargetFormats>,
}

/// Checks `desc`: its attachments are render targets of one size, a color
/// texture and a depth one, and at least one is given.
pub fn check_pass(
    desc: &PassDesc<'_>,
    texture: &dyn Fn(TextureId) -> Option<TextureInfo>,
) -> Result<PassTarget, GpuError> {
    let label = desc.label;
    let mut size = None;
    let mut known = true;
    let mut targets = TargetFormats { color: None, depth_stencil: None };
    if let Some(attachment) = desc.color {
        let info = texture(attachment.texture).ok_or(GpuError::UnknownId("texture"))?;
        let color = info.format.is_none_or(|format| format.bytes_per_texel().is_some());
        if !info.usage.render_target || !color {
            return Err(GpuError::Invalid(format!("{label}: the color attachment isn't a color render target")));
        }
        size = Some(info.size);
        known = info.format.is_some();
        targets.color = info.format;
    }
    if let Some(attachment) = desc.depth_stencil {
        let info = texture(attachment.texture).ok_or(GpuError::UnknownId("texture"))?;
        let format = info.format.filter(|format| format.bytes_per_texel().is_none());
        let Some(format) = format.filter(|_| info.usage.render_target) else {
            return Err(GpuError::Invalid(format!("{label}: the depth attachment isn't a depth render target")));
        };
        if size.is_some_and(|size| size != info.size) {
            return Err(GpuError::Invalid(format!("{label}: attachments of different sizes")));
        }
        size = Some(info.size);
        targets.depth_stencil = Some(format);
    }
    let size = size.ok_or_else(|| GpuError::Invalid(format!("{label}: a pass without attachments")))?;
    Ok(PassTarget { size, targets: known.then_some(targets) })
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

/// Checks that `info`, the texture's, is of a live
/// [`copy_src`](TextureUsage::copy_src) texture of a color format, and that
/// `region` is inside it and not empty.
pub fn check_readback(info: Option<TextureInfo>, region: TextureRegion) -> Result<Readback, GpuError> {
    let info = info.ok_or(GpuError::UnknownId("texture"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx_types::AttributeDataType;
    use crate::gpu::{ColorAttachment, LoadOp, SlotMap, VertexAttribute as Attribute};

    fn lookup(buffers: &SlotMap<BufferId, BufferInfo>) -> impl Fn(BufferId) -> Option<BufferInfo> + '_ {
        |id| buffers.get(id).copied()
    }

    #[test]
    fn bindings_bind_uniform_ranges_inside_their_buffers() {
        let mut buffers = SlotMap::with_key();
        let uniform = buffers.insert(BufferInfo { usage: BufferUsage::Uniform, size: 512 });
        let storage = buffers.insert(BufferInfo { usage: BufferUsage::Storage, size: 512 });
        let vertex = buffers.insert(BufferInfo { usage: BufferUsage::Vertex, size: 512 });
        let shader = SlotMap::<ShaderId, ()>::with_key().insert(());
        let check = |buffers: &SlotMap<BufferId, BufferInfo>, entries: &[BindGroupEntry], reads: &[ShaderBinding]| {
            let buffer = lookup(buffers);
            let lookup = Lookup { buffer: &buffer, texture: &|_| None, sampler: &|_| false };
            check_bindings(&BindingsDesc { label: "test", shader, entries }, reads, 256, &lookup)
        };
        let entry = |buffer, offset, size| BindGroupEntry::Buffer { id: 2, buffer, offset, size };
        assert_eq!(check(&buffers, &[entry(uniform, 256, 256)], &[]).ok(), Some(u32::MAX));
        assert!(matches!(check(&buffers, &[entry(uniform, 256, 257)], &[]), Err(GpuError::Invalid(_))), "past");
        assert!(matches!(check(&buffers, &[entry(uniform, 256, 0)], &[]), Err(GpuError::Invalid(_))), "empty");
        assert!(matches!(check(&buffers, &[entry(uniform, 16, 16)], &[]), Err(GpuError::Invalid(_))), "unaligned");
        let overflow = check(&buffers, &[entry(uniform, u64::MAX - 255, 512)], &[]);
        assert!(matches!(overflow, Err(GpuError::Invalid(_))), "overflows");
        assert!(matches!(check(&buffers, &[entry(vertex, 0, 16)], &[]), Err(GpuError::Invalid(_))), "vertex");
        let twice = [entry(uniform, 0, 16), entry(uniform, 256, 16)];
        assert!(matches!(check(&buffers, &twice, &[]), Err(GpuError::Invalid(_))), "one id twice");

        let reads = [ShaderBinding::Buffer { id: 2, min_size: 64 }];
        assert!(matches!(check(&buffers, &[], &reads), Err(GpuError::Invalid(_))), "unbound");
        assert!(matches!(check(&buffers, &[entry(uniform, 0, 32)], &reads), Err(GpuError::Invalid(_))), "short");
        assert_eq!(check(&buffers, &[entry(storage, 0, 512)], &reads).ok(), Some(8), "elements");
        buffers.remove(uniform);
        assert!(matches!(check(&buffers, &[entry(uniform, 0, 16)], &[]), Err(GpuError::UnknownId("buffer"))));
    }

    #[test]
    fn geometry_holds_its_instances() {
        let mut buffers = SlotMap::with_key();
        let vertices = buffers.insert(BufferInfo { usage: BufferUsage::Vertex, size: 16 });
        let indices = buffers.insert(BufferInfo { usage: BufferUsage::Index, size: 12 });
        let shader = SlotMap::<ShaderId, ()>::with_key().insert(());
        let layout = |step| VertexBufferLayout {
            stride: 8,
            step,
            attributes: vec![Attribute { id: 0, format: AttributeDataType::Short2, offset: 4 }],
        };
        let check = |layouts: &[VertexBufferLayout], vertex_buffers: &[(BufferId, u64)], index_buffer| {
            let buffer = lookup(&buffers);
            let lookup = Lookup { buffer: &buffer, texture: &|_| None, sampler: &|_| false };
            check_geometry(&GeometryDesc { shader, layouts, vertex_buffers, index_buffer }, &lookup)
        };
        let ranges = check(&[layout(VertexStep::Instance)], &[(vertices, 0)], indices).unwrap();
        assert_eq!(ranges, GeometryRanges { indices: 6, instances: 2 });
        let ranges = check(&[layout(VertexStep::Instance)], &[(vertices, 8)], indices).unwrap();
        assert_eq!(ranges.instances, 1);
        assert_eq!(check(&[layout(VertexStep::Vertex)], &[(vertices, 0)], indices).unwrap().instances, u32::MAX);
        assert!(check(&[layout(VertexStep::Vertex)], &[], indices).is_err(), "no buffer for the layout");
        assert!(check(&[layout(VertexStep::Vertex)], &[(vertices, 16)], indices).is_err(), "from the end");
        assert!(check(&[layout(VertexStep::Vertex)], &[(indices, 0)], indices).is_err(), "indices as vertices");
        assert!(check(&[], &[], vertices).is_err(), "vertices as indices");
    }

    #[test]
    fn draws_stay_inside_what_they_name() {
        let shader = SlotMap::<ShaderId, ()>::with_key().insert(());
        let targets = TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None };
        let pipeline = PipelineTags { shader, layout: 0, targets };
        let bindings = BindingsTags { shader, elements: 2 };
        let geometry = GeometryTags { layout: 0, ranges: GeometryRanges { indices: 6, instances: u32::MAX } };
        let draw = Draw {
            pipeline: crate::gpu::PipelineId::default(),
            bindings: crate::gpu::BindingsId::default(),
            geometry: crate::gpu::GeometryId::default(),
            first_index: 0,
            index_count: 6,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
            global_index: 1,
            dynamic: crate::gpu::DynamicState {
                depth_range: (0.0, 1.0),
                scissor: crate::gpu::Scissor { x: 0, y: 0, width: 1, height: 1 },
                stencil_reference: 0,
                blend_constant: mln_util::Color::black(),
                line_width: 1.0,
            },
        };
        let check =
            |draw: Draw, geometry: GeometryTags| check_draw(&draw, Some(targets), &pipeline, &bindings, &geometry);
        assert!(check(draw, geometry).is_ok());
        assert!(check(Draw { first_index: 1, ..draw }, geometry).is_err(), "past the indices");
        assert!(check(Draw { global_index: 2, ..draw }, geometry).is_err(), "past the elements");
        assert!(check(draw, GeometryTags { layout: 1, ..geometry }).is_err(), "another layout");
        let srgb = TargetFormats { color: Some(TextureFormat::Rgba8UnormSrgb), ..targets };
        assert!(check_draw(&draw, Some(srgb), &pipeline, &bindings, &geometry).is_err(), "other targets");
        assert!(check_draw(&draw, None, &pipeline, &bindings, &geometry).is_err(), "unknown targets");
    }

    #[test]
    fn passes_draw_into_render_targets_of_one_size() {
        let mut textures = SlotMap::with_key();
        let usage = TextureUsage { render_target: true, ..TextureUsage::default() };
        let size = Size { width: 4, height: 4 };
        let color = textures.insert(TextureInfo { format: Some(TextureFormat::Rgba8Unorm), size, usage });
        let small = textures.insert(TextureInfo {
            format: Some(TextureFormat::Depth32Float),
            size: Size { width: 2, height: 2 },
            usage,
        });
        let texture = |id| textures.get(id).copied();
        let pass = |color, depth_stencil| PassDesc { label: "test", color, depth_stencil };
        let attachment = |texture| ColorAttachment { texture, load: LoadOp::Load };
        let depth =
            |texture| crate::gpu::DepthStencilAttachment { texture, depth: LoadOp::Load, stencil: LoadOp::Load };
        let target = check_pass(&pass(Some(attachment(color)), None), &texture).unwrap();
        assert_eq!(target.targets, Some(TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None }));
        assert!(check_pass(&pass(None, None), &texture).is_err(), "no attachments");
        assert!(check_pass(&pass(Some(attachment(color)), Some(depth(small))), &texture).is_err(), "sizes");
        assert!(check_pass(&pass(Some(attachment(small)), None), &texture).is_err(), "depth as color");
    }

    #[test]
    fn writes_reach_whole_words_inside_their_buffer() {
        assert!(check_write(4, 8, 12).is_ok());
        assert!(check_write(12, 0, 12).is_ok(), "empty, at the end");
        assert!(matches!(check_write(2, 8, 12), Err(GpuError::Invalid(_))), "an unaligned offset");
        assert!(matches!(check_write(0, 6, 12), Err(GpuError::Invalid(_))), "an unaligned length");
        assert!(matches!(check_write(8, 8, 12), Err(GpuError::Invalid(_))), "past the end");
        assert!(matches!(check_write(u64::MAX - 3, 4, 12), Err(GpuError::Invalid(_))), "past u64::MAX");
    }

    #[test]
    fn readbacks_copy_a_region_of_a_color_texture() {
        let size = Size { width: 4, height: 2 };
        let copy_src = TextureUsage { render_target: true, copy_src: true, ..TextureUsage::default() };
        let color = Some(TextureInfo { format: Some(TextureFormat::Rgba8Unorm), size, usage: copy_src });
        let depth = Some(TextureInfo { format: Some(TextureFormat::Depth32Float), size, usage: copy_src });
        let usage = TextureUsage { render_target: true, ..TextureUsage::default() };
        let target = Some(TextureInfo { format: Some(TextureFormat::Rgba8Unorm), size, usage });
        let region = TextureRegion { x: 1, y: 1, size: Size { width: 3, height: 1 } };
        let readback = check_readback(color, region).unwrap();
        assert_eq!((readback.bytes_per_row, readback.len()), (12, 12));
        let past = TextureRegion { x: 2, ..region };
        assert!(matches!(check_readback(color, past), Err(GpuError::Invalid(_))), "past the edge");
        let empty = TextureRegion { size: Size { width: 0, height: 1 }, ..region };
        assert!(matches!(check_readback(color, empty), Err(GpuError::Invalid(_))), "empty");
        assert!(matches!(check_readback(depth, region), Err(GpuError::Invalid(_))), "depth");
        assert!(matches!(check_readback(target, region), Err(GpuError::Invalid(_))), "not copy_src");
        assert!(matches!(check_readback(None, region), Err(GpuError::UnknownId("texture"))));
    }
}
