#![cfg(not(target_arch = "wasm32"))]
//! Compiles and links every built-in program's GLSL on an OpenGL ES 3.0
//! context and on a desktop OpenGL one, through `GlGpu`, with the keys of
//! the renderer's variants: in both projection variants, with every property
//! the program declares read per vertex and with every one read from its
//! uniform. Also checks each program against its tables:
//! every uniform block is as large as the UBO the renderer writes for it,
//! and every attribute and sampler the program has is in them.

use std::collections::BTreeSet;

use mln_builtins::shaders::{every_program, projection_variants, stands};
use mln_gfx::RendererBackend as _;
use mln_gfx::gpu::ShaderKey;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::source::GlslEsSource;
use mln_gfx::util_hash::HashSet;

use crate::shaders::declarations::Stage;
use crate::shaders::shader_source::{UniformBlockInfo, compose, uniform_block};
use crate::{Api, Device, DeviceOptions, RendererBackend};

/// A backend on a device of `api`, or none where there is no EGL to make a
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

/// Every program the renderer draws with that has GLSL ES, with its GLSL
/// ES.
fn programs() -> impl Iterator<Item = (&'static Program, &'static GlslEsSource)> {
    every_program().filter_map(|program| Some((program, program.sources.glsl_es?)))
}

/// The names after `HAS_UNIFORM_u_` in a shader's GLSL.
fn uniforms(source: &GlslEsSource) -> BTreeSet<String> {
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

/// A variant of a program that the tests compile.
struct Variant {
    /// The program and variant, for messages.
    what: String,
    key: ShaderKey,
    /// Whether it reads every property from its uniform, rather than every
    /// one per vertex.
    uniforms: bool,
    /// Whether it is a variant of a program that stands on terrain that
    /// doesn't.
    flat: bool,
}

/// The variants of `program` that the tests compile: in each projection
/// variant, with every property read per vertex and with every one read
/// from its uniform.
fn variants(program: &'static Program) -> Vec<Variant> {
    let parameters = ProgramParameters::new(1.0);
    let mut property_sets = vec![&[][..]];
    if !program.properties.is_empty() {
        property_sets.push(program.properties);
    }
    let mut variants = Vec::new();
    for &variant in projection_variants(program) {
        for (i, &property_set) in property_sets.iter().enumerate() {
            variants.push(Variant {
                what: format!("{} ({variant:?}, {})", program.name, ["attributes", "uniforms"][i]),
                key: program.variant_key(&parameters, property_set, variant),
                uniforms: i == 1,
                flat: !variant.stands() && stands(program),
            });
        }
    }
    variants
}

/// The vertex and fragment stages of `source` for `key`'s program, as GL ES.
fn stages(source: &GlslEsSource, key: &ShaderKey) -> [String; 2] {
    let defines: String = key.define_values().flat_map(|(name, value)| ["#define ", name, " ", value, "\n"]).collect();
    let header = ["#version 300 es\n", &defines].concat();
    [Stage::Vertex, Stage::Fragment].map(|stage| {
        compose(source, key.program(), stage, &header).unwrap_or_else(|error| panic!("{}: {error}", key.program().name))
    })
}

/// The names of the uniform blocks `glsl` declares, in declaration order.
fn declared_blocks(glsl: &str) -> Vec<String> {
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

/// The uniform blocks that the stages of `source` for `key` declare, by
/// their names, with where each binds.
fn uniform_blocks(source: &GlslEsSource, key: &ShaderKey) -> Vec<(String, Option<UniformBlockInfo>)> {
    let names: BTreeSet<String> = stages(source, key).iter().flat_map(|glsl| declared_blocks(glsl)).collect();
    names.into_iter().map(|name| (name.clone(), uniform_block(key.program(), &name))).collect()
}

/// WebGL2 rejects a draw whose program has a uniform block without a buffer,
/// and whether a block the shader doesn't read counts differs between
/// compilers. So every block a shader declares needs an id to bind at.
#[test]
fn every_declared_uniform_block_has_a_buffer() {
    for (program, source) in programs() {
        for variant in variants(program) {
            for (block, info) in uniform_blocks(source, &variant.key) {
                assert!(info.is_some(), "{}: uniform block {block} has no buffer", variant.what);
            }
        }
    }
}

/// SwiftShader, which Chrome's WebGL falls back to without a GPU, misreads a
/// fragment shader whose blocks shared with the vertex shader don't come
/// first, in the vertex shader's order (maplibre/maplibre-native#4625).
#[test]
fn fragment_shaders_declare_shared_uniform_blocks_first() {
    for (program, source) in programs() {
        for variant in variants(program) {
            let [vertex, fragment] = stages(source, &variant.key).map(|glsl| declared_blocks(&glsl));
            let shared: Vec<&String> = vertex.iter().filter(|block| fragment.contains(block)).collect();
            let leading: Vec<&String> = fragment.iter().take(shared.len()).collect();
            assert_eq!(leading, shared, "{}: fragment shader's leading uniform blocks", variant.what);
        }
    }
}

#[test]
fn every_program_links_and_matches_its_tables_on_open_gl_es() {
    if let Some(mut backend) = headless_backend(Api::Gles) {
        assert!(glow::HasContext::version(backend.context().gl()).is_embedded);
        every_program_links_and_matches_its_tables(&mut backend);
    }
}

/// The programs compile as GLSL 3.30 there.
#[test]
fn every_program_links_and_matches_its_tables_on_desktop_open_gl() {
    if let Some(mut backend) = headless_backend(Api::Gl) {
        assert!(!glow::HasContext::version(backend.context().gl()).is_embedded);
        every_program_links_and_matches_its_tables(&mut backend);
    }
}

fn every_program_links_and_matches_its_tables(backend: &mut RendererBackend) {
    let version = glow::HasContext::version(backend.context().gl());
    println!("GL_RENDERER: {}, GL_VERSION: {}", backend.get_renderer_name(), version.vendor_info);

    let mut linked = 0;
    for (program, source) in programs() {
        for Variant { what, key, uniforms, flat } in variants(program) {
            // Fragment shaders default to mediump on a GPU and to highp on a
            // CPU rasterizer, so both must link wherever this runs.
            let mut compile = |highp_fragment| {
                backend.gpu().program(&key, highp_fragment).unwrap_or_else(|error| {
                    panic!("{what} failed to compile or link with highp_fragment {highp_fragment}: {error}")
                })
            };
            compile(true);
            let compiled = compile(false);
            linked += 1;

            let blocks = uniform_blocks(source, &key);
            for block in &compiled.blocks {
                let (name, info) = blocks
                    .iter()
                    .find_map(|(name, info)| info.filter(|info| info.id == block.id).map(|info| (name, info)))
                    .unwrap_or_else(|| panic!("{what}: block at {} without a buffer", block.id));
                assert_eq!(
                    block.size.next_multiple_of(16),
                    info.size,
                    "{what}: the size of {name} differs from its UBO's"
                );
            }

            // With every property an attribute, the program has every
            // attribute it declares but the ones its GLSL leaves unused.
            let mut locations = HashSet::default();
            for &(_, location) in &compiled.vertex_locations {
                assert!(locations.insert(location), "{what}: two attributes at location {location}");
            }
            if !uniforms {
                assert!(!locations.is_empty(), "{what}: no vertex attributes");
            }
            // A variant that doesn't stand on terrain doesn't read the
            // ground's depth, whatever slot its program keeps for it.
            let textures = program.textures.iter().enumerate();
            for (slot, texture) in textures.filter(|(_, texture)| !(flat && texture.name == "ground_depth")) {
                if !texture.is_unused() {
                    assert!(compiled.textures.contains(&slot), "{what}: no sampler u_{}", texture.name);
                }
            }
        }
    }
    assert!(linked >= 2 * programs().count());
}
