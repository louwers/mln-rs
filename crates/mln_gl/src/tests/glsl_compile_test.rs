#![cfg(not(target_arch = "wasm32"))]
//! Compiles and links every built-in program's GLSL on an OpenGL ES 3.0
//! context and on a desktop OpenGL one, through `GlGpu`, with the keys of
//! the renderer's variants: in each projection variant, with every property
//! the program declares read per vertex and with every one read from its
//! uniform. Also checks each program against its tables:
//! every uniform block is as large as the UBO the renderer writes for it,
//! and every attribute and sampler the program has is in them.

use std::collections::BTreeSet;

use mln_gfx::RendererBackend as _;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::programs::projection_variants;
use mln_gfx::shaders::shader_defines::{ID_GROUND_DEM_TEXTURE, ID_GROUND_DEPTH_TEXTURE};
use mln_util::hash::HashSet;

use crate::shaders::glsl_sources::PRELUDE;
use crate::shaders::shader_source::{BUILT_INS, ShaderSource};
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

/// The names after `HAS_UNIFORM_u_` in a shader's GLSL.
fn uniforms(source: &ShaderSource) -> BTreeSet<String> {
    const PREFIX: &str = "HAS_UNIFORM_u_";
    let mut names = BTreeSet::new();
    for glsl in [source.glsl.vertex, source.glsl.fragment] {
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
    for &(program, source) in BUILT_INS {
        let declared: BTreeSet<String> = program.properties.iter().map(|property| property.name.to_owned()).collect();
        assert_eq!(uniforms(source), declared, "{}", program.name);
    }
}

/// Every program that draws a layer outputs white for the overdraw
/// inspector, which counts its draws.
#[test]
fn every_layer_program_shows_the_overdraw_inspector() {
    use mln_gfx::shaders::programs::{
        ATMOSPHERE, CLIPPING_MASK, COLLISION_BOX, COLLISION_CIRCLE, GLOBE_DEPTH, SKY, TERRAIN, TERRAIN_DEPTH,
        TERRAIN_PROBE, VERTEX_TEXEL,
    };
    // The terrain shows its drapes, where the draped layers count their draws.
    let not_layers = [
        &CLIPPING_MASK,
        &COLLISION_BOX,
        &COLLISION_CIRCLE,
        &GLOBE_DEPTH,
        &SKY,
        &ATMOSPHERE,
        &TERRAIN,
        &TERRAIN_DEPTH,
        &TERRAIN_PROBE,
        &VERTEX_TEXEL,
    ];
    for &(program, source) in BUILT_INS {
        if !not_layers.contains(&program) {
            assert!(
                [source.glsl.vertex, source.glsl.fragment].iter().any(|text| text.contains("OVERDRAW_INSPECTOR")),
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

/// WebGL2 rejects a draw whose program has a uniform block without a buffer,
/// and whether a block the shader doesn't read counts differs between
/// compilers. So every block a shader declares needs an id to bind at.
#[test]
fn every_declared_uniform_block_has_a_table_entry() {
    for &(program, source) in BUILT_INS {
        for glsl in [PRELUDE.vertex, PRELUDE.fragment, source.glsl.vertex, source.glsl.fragment] {
            for block in declared_blocks(glsl) {
                assert!(
                    source.uniform_blocks.iter().any(|info| info.name == block),
                    "{}: uniform block {block} has no table entry",
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
    for &(program, source) in BUILT_INS {
        let vertex = declared_blocks_in_order(source.glsl.vertex);
        let fragment = declared_blocks_in_order(source.glsl.fragment);
        let shared: Vec<&String> = vertex.iter().filter(|block| fragment.contains(block)).collect();
        let leading: Vec<&String> = fragment.iter().take(shared.len()).collect();
        assert_eq!(leading, shared, "{}: fragment shader's leading uniform blocks", program.name);
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
    let parameters = ProgramParameters::new(1.0, false);

    let mut programs = 0;
    for &(program, source) in BUILT_INS {
        let mut property_sets = vec![&[][..]];
        if !program.properties.is_empty() {
            property_sets.push(program.properties);
        }
        let stands = projection_variants(program).contains(&ProjectionVariant::Terrain);
        for &variant in projection_variants(program) {
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
                programs += 1;

                for block in &program.blocks {
                    let info = source
                        .uniform_blocks
                        .iter()
                        .find(|info| info.id == block.id)
                        .unwrap_or_else(|| panic!("{what}: block at {} without a table entry", block.id))
                        .in_variant(variant.stands());
                    assert_eq!(
                        block.size.next_multiple_of(16),
                        info.size,
                        "{what}: the size of {} differs from its UBO's",
                        info.name
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
                // Only the terrain variants of a standing layer's program read
                // the ground's DEM and depth.
                let reads = |id: usize| {
                    variant.stands() || !stands || (id != ID_GROUND_DEM_TEXTURE && id != ID_GROUND_DEPTH_TEXTURE)
                };
                for info in source.textures.iter().filter(|info| reads(info.id)) {
                    assert!(program.textures.contains(&info.id), "{what}: no sampler {}", info.name);
                }
            }
        }
    }
    assert!(programs >= 2 * BUILT_INS.len());
}
