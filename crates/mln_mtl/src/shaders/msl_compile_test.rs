//! Compiles every built-in program's MSL with the system's
//! Metal compiler, the way `MetalGpu` does: as `compose` writes it, MSL
//! 2.4, fast math unless the program's MSL turns it off,
//! the `ProgramParameters` defines, a
//! `HAS_UNIFORM_u_<name>` define per property passed as a uniform, and
//! `PROJECTION_GLOBE` for the globe variant. Also checks that the shaders'
//! pipelines build with vertex buffers of the attributes their programs
//! declare, in both projection variants.

use std::collections::BTreeSet;

use mln_builtins::shaders::{every_program, projection_variants};
use mln_gfx::gfx_types::AttributeDataType;
use mln_gfx::gpu::VertexStep;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::program::{GLOBE_DEFINE, Program, TERRAIN_DEFINE};
use mln_gfx::shaders::source::MslSource;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSDictionary, NSObject, NSString};
use objc2_metal::{
    MTLCompileOptions, MTLCreateSystemDefaultDevice, MTLDevice, MTLFunction, MTLLanguageVersion, MTLLibrary,
    MTLPixelFormat, MTLRenderPipelineDescriptor, MTLVertexDescriptor, MTLVertexFormat, MTLVertexStepFunction,
};

use super::shader_source::{compose, first_vertex_buffer};
use crate::Id;

/// Every program the renderer draws with that has MSL, with its MSL.
fn sources() -> impl Iterator<Item = (&'static Program, &'static MslSource)> {
    every_program().filter_map(|program| Some((program, program.sources.msl?)))
}

fn device() -> Id<dyn MTLDevice> {
    MTLCreateSystemDefaultDevice().expect("no Metal device")
}

/// The library of `source` with `defines`, and with fast math if
/// `fast_math`, or the compiler's error.
fn compile(
    device: &ProtocolObject<dyn MTLDevice>,
    source: &str,
    defines: &[(String, String)],
    fast_math: bool,
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
    if !fast_math {
        #[expect(deprecated, reason = "its replacement, mathMode, needs macOS 15 and iOS 18")]
        options.setFastMathEnabled(false);
    }
    options.setLanguageVersion(MTLLanguageVersion::Version2_4);
    options.setPreserveInvariance(true);
    device
        .newLibraryWithSource_options_error(&NSString::from_str(source), Some(&options))
        .map_err(|error| error.localizedDescription().to_string())
}

fn full_source(program: &Program, source: &MslSource) -> String {
    compose(source, program).unwrap_or_else(|error| panic!("{error}"))
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

/// ProgramParameters(1.0), the projection defines, and a uniform define per
/// name.
fn defines<'a>(projection: ProjectionVariant, uniforms: impl IntoIterator<Item = &'a String>) -> Vec<(String, String)> {
    let mut defines = vec![("DEVICE_PIXEL_RATIO".to_owned(), "1.0".to_owned())];
    if projection.is_globe() {
        defines.push((GLOBE_DEFINE.to_owned(), String::new()));
    }
    if projection.stands() {
        defines.push((TERRAIN_DEFINE.to_owned(), String::new()));
    }
    defines.extend(uniforms.into_iter().map(|name| (format!("HAS_UNIFORM_u_{name}"), String::new())));
    defines
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
        for &projection in projection_variants(program) {
            // Every property per vertex, every property as a uniform, and each
            // one alone as a uniform.
            let mut variants = vec![
                ("no uniforms".to_owned(), defines(projection, [])),
                ("all uniforms".to_owned(), defines(projection, &properties)),
            ];
            variants.extend(properties.iter().map(|name| (format!("uniform {name}"), defines(projection, [name]))));
            for (variant, defines) in variants {
                match compile(&device, &text, &defines, source.fast_math) {
                    Ok(library) => {
                        compiled += 1;
                        for function in [source.vertex_main_function, source.fragment_main_function] {
                            if library.newFunctionWithName(&NSString::from_str(function)).is_none() {
                                failures.push(format!(
                                    "{} ({projection:?}, {variant}): no function {function}",
                                    program.name
                                ));
                            }
                        }
                    }
                    Err(error) => failures.push(format!("{} ({projection:?}, {variant}):\n{error}", program.name)),
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

/// A vertex descriptor with every attribute of `program` at its location,
/// each in a buffer of its own, from the shader's first vertex buffer on, the
/// instance attributes' buffers stepping per instance.
fn vertex_descriptor(
    program: &Program,
    data_type: impl Fn(AttributeDataType) -> AttributeDataType,
) -> Retained<MTLVertexDescriptor> {
    let descriptor = MTLVertexDescriptor::new();
    let first = first_vertex_buffer(program).unwrap_or(0);
    for (location, step, attribute) in program.locations() {
        let (format, size) = vertex_format(data_type(attribute.format));
        let buffer_index = first + location;
        // SAFETY: the attribute and buffer indices are below 31.
        unsafe {
            let descriptor_attribute = descriptor.attributes().objectAtIndexedSubscript(location);
            descriptor_attribute.setFormat(format);
            descriptor_attribute.setOffset(0);
            descriptor_attribute.setBufferIndex(buffer_index);
            let layout = descriptor.layouts().objectAtIndexedSubscript(buffer_index);
            layout.setStride(size);
            if step == VertexStep::Instance {
                layout.setStepFunction(MTLVertexStepFunction::PerInstance);
            }
        }
    }
    descriptor
}

/// Builds each shader's render pipeline with a buffer of each attribute its
/// program declares: the vertex attributes the MSL reads are all the
/// program's, at their locations, in formats Metal can convert to the MSL's
/// types. The data-driven attributes also come as one value instead of two
/// (source functions instead of composite functions), which leaves the MSL's
/// second component 0.
#[test]
fn pipelines_build_with_the_programs_attributes() {
    let device = device();
    let mut failures = Vec::new();
    for (program, source) in sources() {
        let text = full_source(program, source);
        let properties: BTreeSet<String> = program.properties.iter().map(|property| property.name.to_owned()).collect();
        assert_eq!(
            uniform_properties(&text),
            properties,
            "{}: the properties its MSL reads from uniforms",
            program.name
        );
        let narrow = |data_type| match data_type {
            AttributeDataType::Float2 => AttributeDataType::Float,
            AttributeDataType::Float4 => AttributeDataType::Float2,
            other => other,
        };
        let layouts: [(&str, Retained<MTLVertexDescriptor>); 2] = [
            ("declared formats", vertex_descriptor(program, |data_type| data_type)),
            ("single-value formats", vertex_descriptor(program, narrow)),
        ];
        let variants = projection_variants(program).iter().flat_map(|&projection| {
            [
                (format!("{projection:?}, no uniforms"), defines(projection, [])),
                (format!("{projection:?}, all uniforms"), defines(projection, &properties)),
            ]
        });
        for (variant, defines) in variants {
            let library = compile(&device, &text, &defines, source.fast_math)
                .unwrap_or_else(|error| panic!("{} ({variant}): {error}", program.name));
            let function = |name: &str| library.newFunctionWithName(&NSString::from_str(name)).unwrap();
            let vertex = function(source.vertex_main_function);
            let fragment = function(source.fragment_main_function);
            // A vertex function without `[[stage_in]]` has no attributes.
            let attributes = vertex.vertexAttributes().map(|attributes| attributes.to_vec()).unwrap_or_default();
            for attribute in attributes.iter().filter(|attribute| attribute.isActive()) {
                if program.attribute_at(attribute.attributeIndex()).is_none() {
                    failures.push(format!(
                        "{} ({variant}): [[attribute({})]] {} isn't the program's",
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
                if let Err(error) = device.newRenderPipelineStateWithDescriptor_error(&descriptor) {
                    failures.push(format!("{} ({variant}, {layout}): {}", program.name, error.localizedDescription()));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_programs_fit_the_argument_tables() {
    // Metal's argument tables have 31 buffers, 31 vertex attributes and 16
    // samplers.
    for (program, _) in sources() {
        let attributes = program.locations().count();
        assert!(attributes < 31, "{}", program.name);
        assert_eq!(first_vertex_buffer(program).is_some(), attributes > 0, "{}", program.name);
        assert!(first_vertex_buffer(program).is_none_or(|first| first + attributes <= 31), "{}", program.name);
        assert!(program.textures.len() <= 16, "{}", program.name);
    }
}
