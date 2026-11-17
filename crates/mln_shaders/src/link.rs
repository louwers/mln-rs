//! Links a program's WESL into WGSL for a target, rewriting its resource
//! declarations into the target's binding convention (see the crate's doc
//! comment).

use mln_gfx::shaders::shader_defines::ID_GLOBAL_UBO_INDEX;
use mln_util::hash::HashMap;
use wesl::resolver::VirtualResolver;
use wesl::syntax::{
    AddressSpace, Attribute, DeclarationKind, Expression, ExpressionNode, GlobalDeclaration, LiteralExpression,
    TemplateArg, TranslationUnit,
};
use wesl::{CompileOptions, Compiler, Feature, Features, ManglerKind};

use crate::layout::{
    DescriptorSetType, IMAGE_SAMPLER_START, drawable_uniform_binding, global_uniform_binding, image_sampler_binding,
    image_texture_binding, layer_storage_binding, layer_uniform_binding,
};
use crate::{ResourceKind, SOURCES, Target};

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

/// A program linked for a target: its WGSL, what each resource declaration
/// in it is, by its name, and, for WebGPU, the group of the draw's index.
pub(crate) struct Linked {
    pub(crate) wgsl: String,
    pub(crate) declared: HashMap<String, Declared>,
    pub(crate) index_group: Option<u32>,
}

/// Links the program `module` for `target` with the feature flags `flags`
/// on, the target's own among them.
pub(crate) fn link(module: &str, target: Target, flags: &[String]) -> Result<Linked, String> {
    let mut resolver = VirtualResolver::new();
    for &(name, source) in SOURCES {
        let path = format!("package::{name}").parse().map_err(|error| format!("{name}: {error:?}"))?;
        resolver.add_module(path, source.into());
    }
    let mut features = Features { default: Feature::Disable, ..Features::default() };
    features.set(target.feature(), true);
    for flag in flags {
        features.set(flag, true);
    }
    let options = CompileOptions { features, mangler: ManglerKind::Escape, ..CompileOptions::default() };
    let main = format!("package::{module}").parse().map_err(|error| format!("{module}: {error:?}"))?;
    let mut result = Compiler::new_with_resolver(options, resolver)
        .compile_module(&main)
        .map_err(|error| format!("{module}: WESL: {error}"))?;
    let (declared, index_group) = rewrite(&mut result.syntax, target).map_err(|error| format!("{module}: {error}"))?;
    Ok(Linked { wgsl: result.to_string(), declared, index_group })
}

/// Reads what each resource declaration of `syntax` is, and rewrites them
/// into `target`'s convention.
fn rewrite(syntax: &mut TranslationUnit, target: Target) -> Result<(HashMap<String, Declared>, Option<u32>), String> {
    let mut declared = HashMap::default();
    for node in &mut syntax.global_declarations {
        let GlobalDeclaration::Declaration(declaration) = node.node_mut() else { continue };
        let DeclarationKind::Var(space) = declaration.kind else { continue };
        let name = declaration.ident.name().clone();
        let group = attribute(&declaration.attributes, |attribute| match attribute {
            Attribute::Group(expression) => Some(expression),
            _ => None,
        });
        let binding = attribute(&declaration.attributes, |attribute| match attribute {
            Attribute::Binding(expression) => Some(expression),
            _ => None,
        });
        let (group, binding) = match (group, binding) {
            (Some(group), Some(binding)) => (group?, binding?),
            (None, None) => continue,
            _ => return Err(format!("{name} has a @group or a @binding without the other")),
        };
        let type_name = declaration.ty.as_ref().map(|ty| ty.ident.name().clone()).unwrap_or_default();
        let kind = match space {
            Some((AddressSpace::Uniform, _)) => DeclaredKind::Uniform,
            Some((AddressSpace::Storage, _)) => DeclaredKind::Storage,
            None if type_name.starts_with("texture") => DeclaredKind::Texture,
            None if type_name.starts_with("sampler") => DeclaredKind::Sampler,
            _ => return Err(format!("{name} is no buffer, texture or sampler")),
        };
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
        declared.insert(name, Declared { set, kind, id });
    }

    let index_group = match target {
        Target::WebGpu => webgpu_index_group(&declared),
        _ => None,
    };
    for node in &mut syntax.global_declarations {
        let GlobalDeclaration::Declaration(declaration) = node.node_mut() else { continue };
        let Some(&resource) = declared.get(&*declaration.ident.name()) else { continue };
        let name = declaration.ident.name().clone();
        let at = match target {
            Target::Vulkan | Target::D3d12 => Some(vulkan_binding(resource).ok_or_else(|| {
                format!("{name}: no binding of {:?} {} in set {:?}", resource.kind, resource.id, resource.set)
            })?),
            Target::WebGpu => Some(webgpu_binding(resource, &declared)),
            Target::Gl | Target::Metal => None,
        };
        if let Some((group, binding)) = at {
            for attribute in &mut declaration.attributes {
                match attribute.node_mut() {
                    Attribute::Group(expression) => *expression = literal(group),
                    Attribute::Binding(expression) => *expression = literal(binding),
                    _ => {}
                }
            }
        }
        // OpenGL ES 3.0 has no storage buffers: a draw binds its element of
        // the layer's buffer as a uniform block of one element.
        if target == Target::Gl && resource.kind == DeclaredKind::Storage {
            declaration.kind = DeclarationKind::Var(Some((AddressSpace::Uniform, None)));
            let args = declaration
                .ty
                .as_mut()
                .filter(|ty| *ty.ident.name() == "array")
                .and_then(|ty| ty.template_args.as_mut())
                .filter(|args| args.len() == 1)
                .ok_or_else(|| format!("{name}: a storage buffer is an array of any length"))?;
            args.push(TemplateArg { expression: literal(1) });
        }
    }
    Ok((declared, index_group))
}

/// The value of the attribute that `pick` picks, if any: `Err` if it isn't
/// an integer literal.
fn attribute<'a>(
    attributes: &'a [wesl::syntax::AttributeNode],
    pick: impl Fn(&'a Attribute) -> Option<&'a ExpressionNode>,
) -> Option<Result<u32, String>> {
    attributes.iter().find_map(|attribute| pick(attribute)).map(|expression| {
        match expression.node() {
            Expression::Literal(LiteralExpression::AbstractInt(value)) => u32::try_from(*value).ok(),
            Expression::Literal(LiteralExpression::I32(value)) => u32::try_from(*value).ok(),
            Expression::Literal(LiteralExpression::U32(value)) => Some(*value),
            _ => None,
        }
        .ok_or_else(|| "@group and @binding take integer literals".to_owned())
    })
}

fn literal(value: u32) -> ExpressionNode {
    Expression::Literal(LiteralExpression::AbstractInt(i64::from(value))).into()
}

/// The set and binding of `resource` in the Vulkan layout (`crate::layout`),
/// with each texture at the index of its id.
pub(crate) fn vulkan_binding(resource: Declared) -> Option<(u32, u32)> {
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
    Some((resource.set.set_index(), binding))
}

/// Whether `resource` is the draw's index, which WebGPU binds in a group of
/// its own.
fn is_global_index(resource: Declared) -> bool {
    resource.set == DescriptorSetType::Global && resource.id == ID_GLOBAL_UBO_INDEX as u32
}

/// WebGPU's groups, in order, of those `declared` uses: the buffers, the
/// textures and samplers, and the draw's index.
fn webgpu_groups(declared: &HashMap<String, Declared>) -> [Option<u32>; 3] {
    let buffers = declared.values().any(|resource| resource.kind.is_buffer() && !is_global_index(*resource));
    let images = declared.values().any(|resource| !resource.kind.is_buffer());
    let index = declared.values().any(|resource| is_global_index(*resource));
    let mut next = 0;
    [buffers, images, index].map(|used| {
        used.then(|| {
            next += 1;
            next - 1
        })
    })
}

fn webgpu_index_group(declared: &HashMap<String, Declared>) -> Option<u32> {
    webgpu_groups(declared)[2]
}

/// The group and binding of `resource`, one of `declared`, for WebGPU: each
/// buffer at its id in the first group, the textures and samplers at their
/// bindings in the next, and the draw's index alone in the last.
fn webgpu_binding(resource: Declared, declared: &HashMap<String, Declared>) -> (u32, u32) {
    let [buffers, images, index] = webgpu_groups(declared);
    // The group of each of `declared` is there.
    let (group, binding) = match resource.kind {
        _ if is_global_index(resource) => (index, resource.id),
        DeclaredKind::Uniform | DeclaredKind::Storage => (buffers, resource.id),
        DeclaredKind::Texture => (images, resource.id),
        DeclaredKind::Sampler => (images, resource.id + IMAGE_SAMPLER_START as u32),
    };
    (group.unwrap_or_default(), binding)
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
