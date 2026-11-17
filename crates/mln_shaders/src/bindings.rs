//! The resource declarations of the WESL, by its convention (see the crate's
//! doc comment), and their group and binding in each target's.

use mln_gfx::shaders::shader_defines::ID_GLOBAL_UBO_INDEX;

use crate::layout::{
    DescriptorSetType, IMAGE_SAMPLER_START, drawable_uniform_binding, global_uniform_binding, image_sampler_binding,
    image_texture_binding, layer_storage_binding, layer_uniform_binding,
};
use crate::{ResourceBinding, ResourceKind, Target};

/// What a resource declaration of the WESL is, by its convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Declared {
    /// Its `@group`.
    pub(crate) set: DescriptorSetType,
    pub(crate) kind: DeclaredKind,
    /// The buffer's or texture's id; a sampler's texture's.
    pub(crate) id: u32,
}

/// The kind of a resource declaration, before a target's rewriting.
#[cfg_attr(
    not(any(test, feature = "spirv", feature = "hlsl", feature = "glsl", feature = "msl", feature = "wgsl")),
    expect(dead_code, reason = "the crate embeds no target's programs")
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeclaredKind {
    Uniform,
    Storage,
    Texture,
    Sampler,
}

impl DeclaredKind {
    pub(crate) fn is_buffer(self) -> bool {
        matches!(self, DeclaredKind::Uniform | DeclaredKind::Storage)
    }
}

impl Declared {
    /// The declaration `name` of `kind` at `@group(group) @binding(binding)`.
    pub(crate) fn new(name: &str, kind: DeclaredKind, group: u32, binding: u32) -> Result<Declared, String> {
        let set =
            *DescriptorSetType::ALL.get(group as usize).ok_or_else(|| format!("{name} is in no group {group}"))?;
        let image = set == DescriptorSetType::DrawableImage;
        if image == kind.is_buffer() {
            return Err(format!("{name}: buffers go in groups 0 to 2, textures and samplers in group 3"));
        }
        let id = match kind {
            DeclaredKind::Sampler => binding
                .checked_sub(IMAGE_SAMPLER_START as u32)
                .ok_or_else(|| format!("{name}: a sampler binds at {IMAGE_SAMPLER_START} plus its texture's id"))?,
            _ => binding,
        };
        Ok(Declared { set, kind, id })
    }

    /// The group and binding the WESL declares it at.
    fn written(self) -> ResourceBinding {
        let binding = match self.kind {
            DeclaredKind::Sampler => self.id + IMAGE_SAMPLER_START as u32,
            _ => self.id,
        };
        ResourceBinding { group: self.set.set_index(), binding }
    }
}

/// The group and binding of each of `declared`, a variant's resources, in
/// `target`'s convention, and for WebGPU, the group of the draw's index.
pub(crate) fn bindings(declared: &[Declared], target: Target) -> Result<(Vec<ResourceBinding>, Option<u32>), String> {
    let index_group = match target {
        Target::WebGpu => webgpu_groups(declared)[2],
        _ => None,
    };
    let bindings = declared
        .iter()
        .map(|&resource| match target {
            Target::Vulkan | Target::D3d12 => vulkan_binding(resource)
                .ok_or_else(|| format!("no binding of {:?} {} in set {:?}", resource.kind, resource.id, resource.set)),
            Target::WebGpu => Ok(webgpu_binding(resource, declared)),
            Target::Gl | Target::Metal => Ok(resource.written()),
        })
        .collect::<Result<_, _>>()?;
    Ok((bindings, index_group))
}

/// The set and binding of `resource` in the Vulkan layout (`crate::layout`),
/// with each texture at the index of its id.
pub(crate) fn vulkan_binding(resource: Declared) -> Option<ResourceBinding> {
    let id = resource.id as usize;
    let binding = match (resource.set, resource.kind) {
        (DescriptorSetType::Global, DeclaredKind::Uniform) => global_uniform_binding(id)?,
        (DescriptorSetType::Layer, DeclaredKind::Storage) => layer_storage_binding(id)?,
        (DescriptorSetType::Layer, DeclaredKind::Uniform) => layer_uniform_binding(id)?,
        (DescriptorSetType::DrawableUniform, DeclaredKind::Uniform) => drawable_uniform_binding(id)?,
        (DescriptorSetType::DrawableImage, DeclaredKind::Texture) => image_texture_binding(id),
        (DescriptorSetType::DrawableImage, DeclaredKind::Sampler) => image_sampler_binding(id),
        _ => return None,
    };
    Some(ResourceBinding { group: resource.set.set_index(), binding })
}

/// Whether `resource` is the draw's index, which WebGPU binds in a group of
/// its own.
fn is_global_index(resource: Declared) -> bool {
    resource.set == DescriptorSetType::Global && resource.id == ID_GLOBAL_UBO_INDEX as u32
}

/// WebGPU's groups, in order, of those `declared` uses: the buffers, the
/// textures and samplers, and the draw's index.
fn webgpu_groups(declared: &[Declared]) -> [Option<u32>; 3] {
    let buffers = declared.iter().any(|resource| resource.kind.is_buffer() && !is_global_index(*resource));
    let images = declared.iter().any(|resource| !resource.kind.is_buffer());
    let index = declared.iter().any(|resource| is_global_index(*resource));
    let mut next = 0;
    [buffers, images, index].map(|used| {
        used.then(|| {
            next += 1;
            next - 1
        })
    })
}

/// The group and binding of `resource`, one of `declared`, for WebGPU: each
/// buffer at its id in the first group, the textures and samplers at their
/// bindings in the next, and the draw's index alone in the last.
fn webgpu_binding(resource: Declared, declared: &[Declared]) -> ResourceBinding {
    let [buffers, images, index] = webgpu_groups(declared);
    // The group of each of `declared` is there.
    let (group, binding) = match resource.kind {
        _ if is_global_index(resource) => (index, resource.id),
        DeclaredKind::Uniform | DeclaredKind::Storage => (buffers, resource.id),
        DeclaredKind::Texture => (images, resource.id),
        DeclaredKind::Sampler => (images, resource.id + IMAGE_SAMPLER_START as u32),
    };
    ResourceBinding { group: group.unwrap_or_default(), binding }
}

impl From<DeclaredKind> for ResourceKind {
    fn from(kind: DeclaredKind) -> ResourceKind {
        match kind {
            DeclaredKind::Uniform => ResourceKind::UniformBuffer,
            DeclaredKind::Storage => ResourceKind::StorageBuffer,
            DeclaredKind::Texture => ResourceKind::Texture { filterable: true },
            DeclaredKind::Sampler => ResourceKind::Sampler,
        }
    }
}
