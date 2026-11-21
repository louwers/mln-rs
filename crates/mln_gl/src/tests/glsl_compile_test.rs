#![cfg(not(target_arch = "wasm32"))]
//! Compiles and links every built-in program's GLSL on an OpenGL ES 3.0
//! context and on a desktop OpenGL one, through `GlGpu`, with the keys of
//! the renderer's variants: in both projection variants, with every property
//! the program declares read per vertex and with every one read from its
//! uniform. Also checks each program against its declaration: every uniform
//! block is a buffer of the program's or the engine's, as large as the
//! declaration says, and every sampler it declares is in the shader.

use std::collections::BTreeSet;

use mln_gfx::RendererBackend as _;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::program::{Buffer, PROJECTION_BUFFER, Program, VIEW_BUFFER};
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::source::glsl_es::{self, Glsl};
use mln_util::hash::HashSet;

use crate::shaders::glsl_sources::stage_sources;
use crate::{Api, Device, DeviceOptions, RendererBackend};

/// A backend on a device of `api` that compiles the built-in layer types'
/// programs too, or none where there is no EGL to make a
/// context of it with, as on macOS: the test is skipped there, unless
/// `MLN_RENDER_BACKEND` asks for this backend with this API (`gl` for
/// OpenGL ES, `gl-desktop` for desktop OpenGL).
fn headless_backend(api: Api) -> Option<RendererBackend> {
    let options = DeviceOptions { api, ..DeviceOptions::default() };
    let knob = match api {
        Api::Gles => "gl",
        Api::Gl => "gl-desktop",
    };
    match Device::with_options(options).and_then(|device| RendererBackend::new(&device)) {
        Ok(backend) => Some(backend),
        Err(error) if std::env::var("MLN_RENDER_BACKEND").is_ok_and(|backend| backend.eq_ignore_ascii_case(knob)) => {
            panic!("{error}")
        }
        Err(error) => {
            eprintln!("skipped: {error}");
            None
        }
    }
}

/// Each program with GLSL: the engine's and the built-in layer types'.
fn programs() -> Vec<(&'static Program, &'static Glsl)> {
    let programs = mln_render::PROGRAMS.iter().chain(mln_layers::PROGRAMS);
    programs.filter_map(|&program| Some((program, program.sources.glsl_es?))).collect()
}

/// The names after `HAS_UNIFORM_u_` in a shader's GLSL.
fn uniforms(source: &Glsl) -> BTreeSet<String> {
    const PREFIX: &str = "HAS_UNIFORM_u_";
    let mut names = BTreeSet::new();
    for glsl in [source.vertex, source.fragment] {
        for (start, _) in glsl.match_indices(PREFIX) {
            let rest = &glsl[start + PREFIX.len()..];
            let end = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(rest.len());
            names.insert(rest[..end].to_owned());
        }
    }
    names
}

/// A variant's defines read a property from its uniform only where the
/// program declares it.
#[test]
fn every_program_declares_the_properties_its_glsl_reads_from_uniforms() {
    for (program, source) in programs() {
        let declared: BTreeSet<String> = program.properties.iter().map(|property| property.name.to_owned()).collect();
        assert_eq!(uniforms(source), declared, "{}", program.name);
    }
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
    for (program, source) in programs() {
        if !not_layers.contains(&program.name) {
            assert!(
                [source.vertex, source.fragment].iter().any(|text| text.contains("OVERDRAW_INSPECTOR")),
                "{}",
                program.name
            );
        }
    }
}

/// The names of the uniform blocks `glsl` declares, in declaration order.
fn declared_blocks_in_order(glsl: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (start, _) in glsl.match_indices("uniform ") {
        let rest = &glsl[start + "uniform ".len()..];
        let end = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(rest.len());
        if rest[end..].trim_start().starts_with('{') {
            names.push(rest[..end].to_owned());
        }
    }
    names
}

/// The names of the uniform blocks `glsl` declares.
fn declared_blocks(glsl: &str) -> BTreeSet<String> {
    declared_blocks_in_order(glsl).into_iter().collect()
}

/// The vertex and fragment shaders of each variant of each program that
/// the renderer compiles on OpenGL ES, as the backend composes them.
fn compiled_stages() -> Vec<(&'static Program, [String; 2])> {
    let parameters = ProgramParameters::new(1.0, false);
    let mut stages = Vec::new();
    for (program, _) in programs() {
        for variant in [ProjectionVariant::Mercator, ProjectionVariant::Globe] {
            for uniforms in [&[][..], program.properties] {
                let key = program.variant_key(&parameters, uniforms, variant);
                stages.push((program, stage_sources(&key, false, false).expect("GLSL ES")));
            }
        }
    }
    stages
}

/// WebGL2 rejects a draw whose program has a uniform block without a buffer,
/// and whether a block the shader doesn't read counts differs between
/// compilers. So every block a shader declares needs an id to bind at.
#[test]
fn every_declared_uniform_block_is_a_buffer() {
    for (program, stages) in compiled_stages() {
        for glsl in &stages {
            for block in declared_blocks(glsl) {
                assert!(
                    program.buffer_named(&block).is_some(),
                    "{}: uniform block {block} is no buffer of the program's",
                    program.name
                );
            }
        }
    }
}

/// SwiftShader, which Chrome's WebGL falls back to without a GPU, misreads a
/// fragment shader whose blocks shared with the vertex shader don't come
/// first, in the vertex shader's order (maplibre/maplibre-native#4625).
#[test]
fn fragment_shaders_declare_shared_uniform_blocks_first() {
    for (program, [vertex, fragment]) in compiled_stages() {
        let vertex = declared_blocks_in_order(&vertex);
        let fragment = declared_blocks_in_order(&fragment);
        let shared: Vec<&String> = vertex.iter().filter(|block| fragment.contains(block)).collect();
        let leading: Vec<&String> = fragment.iter().take(shared.len()).collect();
        assert_eq!(leading, shared, "{}: fragment shader's leading uniform blocks", program.name);
    }
}

#[test]
fn every_program_links_and_matches_its_declaration_on_open_gl_es() {
    if let Some(mut backend) = headless_backend(Api::Gles) {
        assert!(glow::HasContext::version(backend.context().gl()).is_embedded);
        every_program_links_and_matches_its_declaration(&mut backend);
    }
}

/// The programs compile as GLSL 3.30 there.
#[test]
fn every_program_links_and_matches_its_declaration_on_desktop_open_gl() {
    if let Some(mut backend) = headless_backend(Api::Gl) {
        assert!(!glow::HasContext::version(backend.context().gl()).is_embedded);
        every_program_links_and_matches_its_declaration(&mut backend);
    }
}

fn every_program_links_and_matches_its_declaration(backend: &mut RendererBackend) {
    let version = glow::HasContext::version(backend.context().gl());
    println!("GL_RENDERER: {}, GL_VERSION: {}", backend.get_renderer_name(), version.vendor_info);
    let parameters = ProgramParameters::new(1.0, false);

    let mut compiled = 0;
    for (program, _) in programs() {
        let declared = program;
        let mut property_sets = vec![&[][..]];
        if !program.properties.is_empty() {
            property_sets.push(program.properties);
        }
        for variant in [ProjectionVariant::Mercator, ProjectionVariant::Globe] {
            for (i, &property_set) in property_sets.iter().enumerate() {
                let what = format!("{} ({variant:?}, {})", program.name, ["attributes", "uniforms"][i]);
                let key = program.variant_key(&parameters, property_set, variant);
                // Fragment shaders default to mediump on a GPU and to highp
                // on a CPU rasterizer, so both must link wherever this runs.
                let mut compile = |highp_fragment| {
                    backend.gpu().program(&key, highp_fragment).unwrap_or_else(|error| {
                        panic!("{what} failed to compile or link with highp_fragment {highp_fragment}: {error}")
                    })
                };
                compile(true);
                let program = compile(false);
                compiled += 1;

                for block in &program.blocks {
                    let reserved = [VIEW_BUFFER, PROJECTION_BUFFER];
                    let info =
                        declared.buffers.iter().chain(&reserved).find(|info| info.id == block.id).unwrap_or_else(
                            || panic!("{what}: block at {} that isn't a buffer of the program's", block.id),
                        );
                    let declared = info.block().expect("a block");
                    assert_eq!(
                        block.size.next_multiple_of(16),
                        declared.size,
                        "{what}: the size of {} differs from its UBO's",
                        declared.name
                    );
                }

                // With every property an attribute, the program has every
                // attribute of its table but the ones its GLSL leaves unused.
                let mut locations = HashSet::default();
                for &(_, location) in &program.vertex_locations {
                    assert!(locations.insert(location), "{what}: two attributes at location {location}");
                }
                if i == 0 {
                    assert!(!locations.is_empty(), "{what}: no vertex attributes");
                }
                for (slot, info) in declared.textures.iter().enumerate() {
                    assert!(program.textures.contains(&slot), "{what}: no sampler {}", info.name);
                }
            }
        }
    }
    assert!(compiled >= 2 * programs().len());
}

/// `text` without its comments, its whitespace runs one space.
fn normalized(text: &str) -> String {
    let mut code = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("//") {
            rest = after.find('\n').map_or("", |end| &after[end..]);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.find("*/").map_or("", |end| &after[end + 2..]);
            code.push(' ');
        } else {
            let c = rest.chars().next().unwrap_or(' ');
            code.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    code.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The stages declare every block they declare as `uniform_block!` lays it
/// out.
#[test]
fn every_block_is_declared_as_its_layout_says() {
    for (program, stages) in compiled_stages() {
        let buffers = program.buffers.iter().chain([&VIEW_BUFFER, &PROJECTION_BUFFER]);
        for block in buffers.filter_map(Buffer::block) {
            for stage in &stages {
                let stage = normalized(stage);
                if stage.contains(&format!("uniform {} {{", block.name)) {
                    let declaration = normalized(&glsl_es::declaration(block));
                    assert!(stage.contains(&declaration), "{}: {declaration}", program.name);
                }
            }
        }
    }
}
