//! The oracle of the embedded programs: a variant linked directly by WESL,
//! with every flag decided, and naga's reflection of it.

use mln_gfx::shaders::shader_defines::ID_GLOBAL_UBO_INDEX;
use mln_util::hash::HashMap;
use wesl::resolver::VirtualResolver;
use wesl::syntax::{
    AddressSpace, Attribute, DeclarationKind, Expression, ExpressionNode, GlobalDeclaration, LiteralExpression,
    TemplateArg, TranslationUnit,
};
use wesl::{CompileOptions, Compiler, Feature, Features, ManglerKind};

use crate::bindings::{Declared, DeclaredKind, vulkan_binding};
use crate::layout::{DescriptorSetType, IMAGE_SAMPLER_START};
use crate::{ResourceBinding, ResourceKind, SOURCES, ScalarKind, Stage, Target, VertexInput};

/// A program linked for a target: its WGSL, what each resource declaration
/// in it is, by its name, and, for WebGPU, the group of the draw's index.
struct Linked {
    wgsl: String,
    declared: HashMap<String, Declared>,
    index_group: Option<u32>,
}

/// Links the program `module` for `target` with the feature flags `flags`
/// on, the target's own among them.
fn link_wesl(module: &str, target: Target, flags: &[String]) -> Result<Linked, String> {
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
            Target::Vulkan | Target::D3d12 => {
                Some(vulkan_binding(resource).map(|b| (b.group, b.binding)).ok_or_else(|| {
                    format!("{name}: no binding of {:?} {} in set {:?}", resource.kind, resource.id, resource.set)
                })?)
            }
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

/// A resource as the program before this crate's tables reported it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resource {
    pub(crate) name: String,
    pub(crate) handle: naga::Handle<naga::GlobalVariable>,
    pub(crate) kind: ResourceKind,
    pub(crate) set: DescriptorSetType,
    pub(crate) id: u32,
    pub(crate) binding: ResourceBinding,
    pub(crate) size: u64,
    pub(crate) vertex: bool,
    pub(crate) fragment: bool,
}

/// A variant linked directly, validated, and reflected by naga.
pub(crate) struct Direct {
    pub(crate) module: naga::Module,
    pub(crate) resources: Vec<Resource>,
    pub(crate) inputs: Vec<VertexInput>,
    pub(crate) index_group: Option<u32>,
}

/// Links `module`'s variant of `flags` for `target` with WESL, and parses,
/// validates and reflects it with naga.
pub(crate) fn link(module: &'static str, target: Target, flags: &[String]) -> Result<Direct, String> {
    use naga::valid::{Capabilities, ValidationFlags, Validator};

    let linked = link_wesl(module, target, flags)?;
    let wgsl = linked.wgsl;
    let parsed = naga::front::wgsl::parse_str(&wgsl)
        .map_err(|error| format!("{module} ({target:?}): {}", error.emit_to_string(&wgsl)))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&parsed)
        .map_err(|error| format!("{module} ({target:?}): {}", error.emit_to_string(&wgsl)))?;
    let resources = resources(&parsed, &info, &linked.declared)?;
    let inputs = inputs(&parsed)?;
    Ok(Direct { module: parsed, resources, inputs, index_group: linked.index_group })
}

/// The resources of `module` that either entry point uses.
fn resources(
    module: &naga::Module,
    info: &naga::valid::ModuleInfo,
    declared: &HashMap<String, Declared>,
) -> Result<Vec<Resource>, String> {
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).map_err(|error| format!("{error:?}"))?;
    let used_by = |stage: Stage, handle: naga::Handle<naga::GlobalVariable>| {
        module
            .entry_points
            .iter()
            .position(|entry| entry.stage == stage.naga())
            .is_some_and(|index| !info.get_entry_point(index)[handle].is_empty())
    };
    let (sampled, loaded) = texture_uses(module);
    let mut resources = Vec::new();
    for (handle, global) in module.global_variables.iter() {
        let Some(binding) = global.binding else { continue };
        let name = global.name.clone().unwrap_or_default();
        let resource =
            *declared.get(&name).ok_or_else(|| format!("{name} has a binding the program doesn't declare"))?;
        let (vertex, fragment) = (used_by(Stage::Vertex, handle), used_by(Stage::Fragment, handle));
        if !vertex && !fragment {
            continue;
        }
        // The kind declared: OpenGL's storage buffers are uniform blocks.
        let kind = match resource.kind {
            DeclaredKind::Texture => {
                ResourceKind::Texture { filterable: sampled.contains(&handle) || !loaded.contains(&handle) }
            }
            kind => kind.into(),
        };
        let size = match kind {
            ResourceKind::UniformBuffer | ResourceKind::StorageBuffer => u64::from(layouter[global.ty].size),
            _ => 0,
        };
        resources.push(Resource {
            name,
            handle,
            kind,
            set: resource.set,
            id: resource.id,
            binding: ResourceBinding { group: binding.group, binding: binding.binding },
            size,
            vertex,
            fragment,
        });
    }
    Ok(resources)
}

/// The textures that some function of `module` samples, and those it loads
/// texels of, where it names the global itself.
fn texture_uses(
    module: &naga::Module,
) -> (Vec<naga::Handle<naga::GlobalVariable>>, Vec<naga::Handle<naga::GlobalVariable>>) {
    let (mut sampled, mut loaded) = (Vec::new(), Vec::new());
    let functions = module.functions.iter().map(|(_, function)| function);
    for function in functions.chain(module.entry_points.iter().map(|entry| &entry.function)) {
        for (_, expression) in function.expressions.iter() {
            let (image, uses) = match *expression {
                naga::Expression::ImageSample { image, .. } => (image, &mut sampled),
                naga::Expression::ImageLoad { image, .. } => (image, &mut loaded),
                _ => continue,
            };
            if let naga::Expression::GlobalVariable(global) = function.expressions[image] {
                uses.push(global);
            }
        }
    }
    (sampled, loaded)
}

/// The inputs of `module`'s vertex entry point.
fn inputs(module: &naga::Module) -> Result<Vec<VertexInput>, String> {
    let entry = module
        .entry_points
        .iter()
        .find(|entry| entry.stage == naga::ShaderStage::Vertex)
        .ok_or("no vertex entry point")?;
    let mut inputs = Vec::new();
    let mut add = |binding: &Option<naga::Binding>, ty: naga::Handle<naga::Type>| {
        if let Some(naga::Binding::Location { location, .. }) = *binding {
            let inner = &module.types[ty].inner;
            let scalar = match inner.scalar().map(|scalar| scalar.kind) {
                Some(naga::ScalarKind::Sint) => ScalarKind::Sint,
                Some(naga::ScalarKind::Uint) => ScalarKind::Uint,
                _ => ScalarKind::Float,
            };
            let components = match *inner {
                naga::TypeInner::Vector { size, .. } => size as u32,
                _ => 1,
            };
            inputs.push(VertexInput { location, scalar, components });
        }
    };
    for argument in &entry.function.arguments {
        match &module.types[argument.ty].inner {
            naga::TypeInner::Struct { members, .. } => {
                for member in members {
                    add(&member.binding, member.ty);
                }
            }
            _ => add(&argument.binding, argument.ty),
        }
    }
    inputs.sort_by_key(|input| input.location);
    Ok(inputs)
}
