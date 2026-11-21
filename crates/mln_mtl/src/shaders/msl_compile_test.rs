//! Compiles every built-in program's MSL with the system's
//! Metal compiler, the way `MetalGpu` does: its `library_source`,
//! MSL 2.4, fast math, the `ProgramParameters` defines, a
//! `HAS_UNIFORM_u_<name>` define per property passed as a uniform, and
//! `PROJECTION_GLOBE` for the globe variant. Also checks that the shaders'
//! pipelines build with the vertex layouts their programs declare, in both
//! projection variants, and that what they bind is what their programs
//! declare.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::shaders::program::{Buffer, GLOBE_DEFINE, PROJECTION_BUFFER, Program, VIEW_BUFFER};
use mln_gfx::shaders::shader_defines::ID_GLOBAL_UBO_INDEX;
use mln_gfx::shaders::source::msl::{self, Msl};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSDictionary, NSObject, NSString};
use objc2_metal::{
    MTLBinding, MTLBindingType, MTLBufferBinding, MTLCompileOptions, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLFunction, MTLLanguageVersion, MTLLibrary, MTLPipelineOption, MTLPixelFormat, MTLRenderPipelineDescriptor,
    MTLVertexDescriptor, MTLVertexFormat, MTLVertexStepFunction,
};

use super::msl_sources::library_source;
use crate::Id;

/// Each program with MSL: the engine's and the built-in layer types'.
fn sources() -> Vec<(&'static Program, &'static Msl)> {
    let programs = mln_render::PROGRAMS.iter().chain(mln_layers::PROGRAMS);
    programs.filter_map(|&program| Some((program, program.sources.msl?))).collect()
}

/// The projection variants of a program: its name, and whether it defines
/// `PROJECTION_GLOBE`.
const PROJECTIONS: [(&str, bool); 2] = [("Mercator", false), ("globe", true)];

fn device() -> Id<dyn MTLDevice> {
    MTLCreateSystemDefaultDevice().expect("no Metal device")
}

/// The library of `source` with `defines`, or the compiler's error.
fn compile(
    device: &ProtocolObject<dyn MTLDevice>,
    source: &str,
    defines: &[(String, String)],
) -> Result<Id<dyn MTLLibrary>, String> {
    let keys: Vec<Retained<NSString>> = defines.iter().map(|(key, _)| NSString::from_str(key)).collect();
    let values: Vec<Retained<NSString>> = defines.iter().map(|(_, value)| NSString::from_str(value)).collect();
    let keys: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
    let values: Vec<&NSObject> = values.iter().map(|value| &***value).collect();
    let macros = NSDictionary::<NSString, NSObject>::from_slices(&keys, &values);

    let options = MTLCompileOptions::new();
    // SAFETY: the macros map NSString names to NSString values.
    unsafe { options.setPreprocessorMacros(Some(&macros)) };
    // Fast math is Metal's default.
    options.setLanguageVersion(MTLLanguageVersion::Version2_4);
    options.setPreserveInvariance(true);
    device
        .newLibraryWithSource_options_error(&NSString::from_str(source), Some(&options))
        .map_err(|error| error.localizedDescription().to_string())
}

fn full_source(program: &Program, msl: &Msl) -> String {
    library_source(program, msl).expect("expands")
}

/// The `<name>`s of the `HAS_UNIFORM_u_<name>` the shader tests.
fn uniform_properties(text: &str) -> BTreeSet<String> {
    const PREFIX: &str = "HAS_UNIFORM_u_";
    let mut names = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find(PREFIX) {
        rest = &rest[start + PREFIX.len()..];
        let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
        names.insert(rest[..end].to_owned());
        rest = &rest[end..];
    }
    names
}

/// ProgramParameters(1.0, overdraw), the globe define if `globe`, and a
/// uniform define per name.
fn defines<'a>(overdraw: bool, globe: bool, uniforms: impl IntoIterator<Item = &'a String>) -> Vec<(String, String)> {
    let mut defines = vec![("DEVICE_PIXEL_RATIO".to_owned(), "1.0".to_owned())];
    if overdraw {
        defines.push(("OVERDRAW_INSPECTOR".to_owned(), String::new()));
    }
    if globe {
        defines.push((GLOBE_DEFINE.to_owned(), String::new()));
    }
    defines.extend(uniforms.into_iter().map(|name| (format!("HAS_UNIFORM_u_{name}"), String::new())));
    defines
}

/// Every program that draws a layer outputs white for the overdraw
/// inspector, which counts its draws.
#[test]
fn every_layer_program_shows_the_overdraw_inspector() {
    let not_layers = [
        "ClippingMaskProgram",
        "CollisionBoxShader",
        "CollisionCircleShader",
        "GlobeDepthShader",
        "SkyShader",
        "AtmosphereShader",
    ];
    for (program, source) in sources() {
        if !not_layers.contains(&program.name) {
            assert!(full_source(program, source).contains("OVERDRAW_INSPECTOR"), "{}", program.name);
        }
    }
}

#[test]
fn every_shader_source_compiles() {
    let device = device();
    let mut failures = Vec::new();
    let mut compiled = 0;
    for (program, source) in sources() {
        let text = full_source(program, source);
        let properties: BTreeSet<String> = program.properties.iter().map(|property| property.name.to_owned()).collect();
        assert_eq!(
            uniform_properties(&text),
            properties,
            "{}: the properties its MSL reads from uniforms",
            program.name
        );
        for (projection, globe) in PROJECTIONS {
            // Every property per vertex, every property as a uniform, each one
            // alone as a uniform, and the overdraw inspector.
            let mut variants = vec![
                ("no uniforms".to_owned(), defines(false, globe, [])),
                ("all uniforms".to_owned(), defines(false, globe, &properties)),
                ("overdraw inspector".to_owned(), defines(true, globe, [])),
                ("overdraw inspector, all uniforms".to_owned(), defines(true, globe, &properties)),
            ];
            variants.extend(properties.iter().map(|name| (format!("uniform {name}"), defines(false, globe, [name]))));
            for (variant, defines) in variants {
                match compile(&device, &text, &defines) {
                    Ok(library) => {
                        compiled += 1;
                        for function in [source.vertex_main_function, source.fragment_main_function] {
                            if library.newFunctionWithName(&NSString::from_str(function)).is_none() {
                                failures.push(format!(
                                    "{} ({projection}, {variant}): no function {function}",
                                    program.name
                                ));
                            }
                        }
                    }
                    Err(error) => failures.push(format!("{} ({projection}, {variant}):\n{error}", program.name)),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} of {} variants failed:\n{}", failures.len(), compiled + failures.len(), {
        failures.join("\n\n")
    });
}

/// The vertex format of `data_type` and its size.
fn vertex_format(data_type: AttributeDataType) -> (MTLVertexFormat, usize) {
    match data_type {
        AttributeDataType::UByte4 => (MTLVertexFormat::UChar4, 4),
        AttributeDataType::Short2 => (MTLVertexFormat::Short2, 4),
        AttributeDataType::Short4 => (MTLVertexFormat::Short4, 8),
        AttributeDataType::UShort2 => (MTLVertexFormat::UShort2, 4),
        AttributeDataType::UShort4 => (MTLVertexFormat::UShort4, 8),
        AttributeDataType::Float => (MTLVertexFormat::Float, 4),
        AttributeDataType::Float2 => (MTLVertexFormat::Float2, 8),
        AttributeDataType::Float3 => (MTLVertexFormat::Float3, 12),
        AttributeDataType::Float4 => (MTLVertexFormat::Float4, 16),
        other => panic!("no vertex format for {other:?}"),
    }
}

/// A vertex descriptor with every attribute of `program`, at its location
/// and in its format through `format`, the vertex attributes one after the
/// other in the buffer of the first vertex buffer index and the instance
/// attributes in the next one, which steps per instance.
fn vertex_descriptor(
    program: &Program,
    format: impl Fn(AttributeDataType) -> AttributeDataType,
) -> Retained<MTLVertexDescriptor> {
    let descriptor = MTLVertexDescriptor::new();
    let first = crate::gpu::first_vertex_buffer(program).unwrap_or(0);
    let steps = [(program.attributes, 0), (program.instance_attributes, program.attributes.len())];
    for (buffer, (attributes, first_location)) in steps.into_iter().enumerate() {
        let buffer_index = first + buffer;
        let mut stride = 0;
        for (location, attr) in attributes.iter().enumerate() {
            let (vertex_format, size) = vertex_format(format(attr.format));
            // SAFETY: the locations and buffer indexes are below 31.
            unsafe {
                let attribute = descriptor.attributes().objectAtIndexedSubscript(first_location + location);
                attribute.setFormat(vertex_format);
                attribute.setOffset(stride);
                attribute.setBufferIndex(buffer_index);
            }
            stride += size;
        }
        if stride > 0 {
            // SAFETY: the buffer index is below 31.
            unsafe {
                let layout = descriptor.layouts().objectAtIndexedSubscript(buffer_index);
                layout.setStride(stride);
                if buffer == 1 {
                    layout.setStepFunction(MTLVertexStepFunction::PerInstance);
                }
            }
        }
    }
    descriptor
}

/// What is wrong with the buffers and textures a pipeline of `program`
/// binds, by its reflection's `bindings` of a stage: each buffer one of the
/// program's or the engine's, as large as it declares, and each texture one
/// of its slots.
fn check_bindings(program: &Program, bindings: &[Retained<ProtocolObject<dyn MTLBinding>>]) -> Vec<String> {
    let first_vertex_buffer = crate::gpu::first_vertex_buffer(program);
    let mut failures = Vec::new();
    for binding in bindings.iter().filter(|binding| binding.isUsed()) {
        let index = binding.index();
        match binding.r#type() {
            MTLBindingType::Buffer if index == ID_GLOBAL_UBO_INDEX => {}
            MTLBindingType::Buffer
                if !binding.isArgument() || first_vertex_buffer.is_some_and(|first| index >= first) => {}
            MTLBindingType::Buffer => {
                let reserved = [VIEW_BUFFER, PROJECTION_BUFFER];
                let Some(buffer) = program.buffers.iter().chain(&reserved).find(|buffer| buffer.id == index) else {
                    failures.push(format!("buffer {index} isn't the program's"));
                    continue;
                };
                // SAFETY: Metal reflects a buffer binding as an
                // MTLBufferBinding, which ProtocolObject wraps alike.
                let reflected =
                    unsafe { &*std::ptr::from_ref(&**binding).cast::<ProtocolObject<dyn MTLBufferBinding>>() };
                let size = buffer.size();
                if reflected.bufferDataSize() != size {
                    failures.push(format!("buffer {index} is {} bytes, not {size}", reflected.bufferDataSize()));
                }
            }
            MTLBindingType::Texture | MTLBindingType::Sampler if index >= program.textures.len() => {
                failures.push(format!("texture {index} isn't the program's"));
            }
            _ => {}
        }
    }
    failures
}

/// Builds each shader's render pipeline with the vertex layout its program
/// declares: the vertex attributes the MSL reads are all declared, in
/// formats Metal can convert to the MSL's types, and the buffers and
/// textures it binds are the program's. The data-driven attributes also
/// come as one value instead of two (source functions instead of composite
/// functions), which leaves the MSL's second component 0.
#[test]
fn pipelines_build_with_the_programs_vertex_layout() {
    let device = device();
    let mut failures = Vec::new();
    for (program, source) in sources() {
        let text = full_source(program, source);
        let properties: BTreeSet<String> = program.properties.iter().map(|property| property.name.to_owned()).collect();
        let narrow = |format| match format {
            AttributeDataType::Float2 => AttributeDataType::Float,
            AttributeDataType::Float4 => AttributeDataType::Float2,
            other => other,
        };
        let layouts: [(&str, Retained<MTLVertexDescriptor>); 2] = [
            ("declared formats", vertex_descriptor(program, |format| format)),
            ("single-value formats", vertex_descriptor(program, narrow)),
        ];
        let variants = PROJECTIONS.iter().flat_map(|&(projection, globe)| {
            [
                (format!("{projection}, no uniforms"), defines(false, globe, [])),
                (format!("{projection}, all uniforms"), defines(false, globe, &properties)),
            ]
        });
        let locations = program.attributes.len() + program.instance_attributes.len();
        for (variant, defines) in variants {
            let library = compile(&device, &text, &defines)
                .unwrap_or_else(|error| panic!("{} ({variant}): {error}", program.name));
            let function = |name: &str| library.newFunctionWithName(&NSString::from_str(name)).unwrap();
            let vertex = function(source.vertex_main_function);
            let fragment = function(source.fragment_main_function);
            // A vertex function without `[[stage_in]]` has no attributes.
            let attributes = vertex.vertexAttributes().map(|attributes| attributes.to_vec()).unwrap_or_default();
            for attribute in attributes.iter().filter(|attribute| attribute.isActive()) {
                if attribute.attributeIndex() >= locations {
                    failures.push(format!(
                        "{} ({variant}): [[attribute({})]] {} isn't declared",
                        program.name,
                        attribute.attributeIndex(),
                        attribute.name()
                    ));
                }
            }
            for (layout, vertex_descriptor) in &layouts {
                let descriptor = MTLRenderPipelineDescriptor::new();
                descriptor.setVertexFunction(Some(&vertex));
                descriptor.setFragmentFunction(Some(&fragment));
                descriptor.setVertexDescriptor(Some(vertex_descriptor));
                // SAFETY: index 0 of the 8 color attachments.
                unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) }
                    .setPixelFormat(MTLPixelFormat::BGRA8Unorm);
                descriptor.setDepthAttachmentPixelFormat(MTLPixelFormat::Depth32Float_Stencil8);
                descriptor.setStencilAttachmentPixelFormat(MTLPixelFormat::Depth32Float_Stencil8);
                let mut reflection = None;
                let built = device.newRenderPipelineStateWithDescriptor_options_reflection_error(
                    &descriptor,
                    MTLPipelineOption::BindingInfo,
                    Some(&mut reflection),
                );
                if let Err(error) = built {
                    failures.push(format!("{} ({variant}, {layout}): {}", program.name, error.localizedDescription()));
                    continue;
                }
                let reflection = reflection.expect("a pipeline's reflection");
                for stage in [reflection.vertexBindings(), reflection.fragmentBindings()] {
                    for failure in check_bindings(program, &stage.to_vec()) {
                        failures.push(format!("{} ({variant}, {layout}): {failure}", program.name));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// No source declares a block of the engine's or its program's: the
/// backend declares them, from their layouts.
#[test]
fn no_source_declares_a_block() {
    let engine = [VIEW_BUFFER, PROJECTION_BUFFER];
    for (program, source) in sources() {
        let text = [source.prelude, source.source].concat();
        for block in program.buffers.iter().chain(&engine).filter_map(Buffer::block) {
            assert!(!text.contains(&format!(" {} {{", block.name)), "{} declares {}", program.name, block.name);
        }
    }
}

/// MSL lays out every block's struct, padded to its buffer's stride, as its
/// Rust struct is laid out: a library of each with a `static_assert` of
/// each member's offset compiles.
#[test]
fn every_block_has_its_layout() {
    let device = device();
    let engine = [VIEW_BUFFER, PROJECTION_BUFFER];
    let mut library = String::from("#include <metal_stdlib>\nusing namespace metal;\n");
    let mut declared = BTreeSet::new();
    for (program, _) in sources() {
        for buffer in program.buffers.iter().chain(&engine) {
            let Some(block) = buffer.block() else { continue };
            if !declared.insert((block.name, buffer.size())) {
                continue;
            }
            // One namespace a block and its stride, as a library declares
            // each once.
            _ = writeln!(library, "namespace {}_{} {{", block.name, buffer.size());
            library += &msl::declaration(block, buffer.size());
            for member in block.members {
                let (name, offset) = (member.name, member.offset);
                _ = writeln!(
                    library,
                    "static_assert(__builtin_offsetof({}, {name}) == {offset}, \"{name}\");",
                    block.name
                );
            }
            library += "}\n";
        }
    }
    if let Err(error) = compile(&device, &library, &[]) {
        panic!("{error}");
    }
}
