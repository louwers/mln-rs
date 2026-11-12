#![cfg(not(target_arch = "wasm32"))]
//! Compiles and links every GLSL program `shader_source()` returns on an
//! OpenGL ES 3.0 context and on a desktop OpenGL one, the way `gl::ShaderGroup` does: in both projection
//! variants, with every data-driven property as a vertex attribute and with
//! every one as a uniform. Also checks each program against its tables:
//! every uniform block is as large as the UBO the renderer writes for it,
//! and every attribute and sampler the program has is in them.

use std::collections::{BTreeSet, HashSet};

use mln_gfx::backend_scope::{BackendScope, ScopeType};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::renderer_backend::RendererBackend as _;
use mln_gfx::shader_group::StringIDSetsPair;
use mln_gfx::shader_registry::ShaderRegistry;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_program_base::ShaderProgramBase;
use mln_gl::shaders::glsl_sources::PRELUDE;
use mln_gl::shaders::shader_program::as_gl_shader;
use mln_gl::shaders::shader_source::{REGISTERED_SHADERS, ShaderSource, shader_source};
use mln_gl::{Api, Device, DeviceOptions, RendererBackend};

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

/// The data-driven properties of a shader: the names after `HAS_UNIFORM_u_`
/// in its GLSL.
fn properties(source: &ShaderSource) -> BTreeSet<String> {
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
    for &shader_id in REGISTERED_SHADERS {
        let source = shader_source(shader_id).expect("a registered shader has a source");
        for glsl in [PRELUDE.vertex, PRELUDE.fragment, source.glsl.vertex, source.glsl.fragment] {
            for block in declared_blocks(glsl) {
                assert!(
                    source.uniform_blocks.iter().any(|info| info.name == block),
                    "{}: uniform block {block} has no table entry",
                    source.name
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
    for &shader_id in REGISTERED_SHADERS {
        let source = shader_source(shader_id).expect("a registered shader has a source");
        let vertex = declared_blocks_in_order(source.glsl.vertex);
        let fragment = declared_blocks_in_order(source.glsl.fragment);
        let shared: Vec<&String> = vertex.iter().filter(|block| fragment.contains(block)).collect();
        let leading: Vec<&String> = fragment.iter().take(shared.len()).collect();
        assert_eq!(leading, shared, "{}: fragment shader's leading uniform blocks", source.name);
    }
}

#[test]
fn every_program_links_and_matches_its_tables_on_open_gl_es() {
    if let Some(backend) = headless_backend(Api::Gles) {
        assert!(glow::HasContext::version(backend.gl()).is_embedded);
        every_program_links_and_matches_its_tables(&backend);
    }
}

/// The programs compile as GLSL 3.30 there.
#[test]
fn every_program_links_and_matches_its_tables_on_desktop_open_gl() {
    if let Some(backend) = headless_backend(Api::Gl) {
        assert!(!glow::HasContext::version(backend.gl()).is_embedded);
        every_program_links_and_matches_its_tables(&backend);
    }
}

fn every_program_links_and_matches_its_tables(backend: &RendererBackend) {
    let _scope = BackendScope::new(backend, ScopeType::Explicit);
    let version = glow::HasContext::version(backend.gl());
    println!("GL_RENDERER: {}, GL_VERSION: {}", backend.get_renderer_name(), version.vendor_info);
    let registry = ShaderRegistry::new();
    backend.init_shaders(&registry, &ProgramParameters::new(1.0, false));
    let context = backend.get_context();

    let mut programs = 0;
    for &shader_id in REGISTERED_SHADERS {
        let source = shader_source(shader_id).expect("a registered shader has a source");
        let group = registry.get_shader_group(source.name).expect("a registered shader has a group");
        let properties = properties(source);
        let all_uniforms: StringIDSetsPair = (
            properties.iter().map(|name| format!("a_{name}")).collect::<HashSet<_>>(),
            (0..properties.len()).collect::<HashSet<_>>(),
        );
        let mut property_sets = vec![StringIDSetsPair::default()];
        if !properties.is_empty() {
            property_sets.push(all_uniforms);
        }
        for variant in [ProjectionVariant::Mercator, ProjectionVariant::Globe] {
            for (i, property_set) in property_sets.iter().enumerate() {
                let what = format!("{} ({variant:?}, {})", source.name, ["attributes", "uniforms"][i]);
                let shader = group
                    .get_or_create_shader(context, property_set, variant, "")
                    .unwrap_or_else(|| panic!("{what} failed to compile or link"));
                let program = as_gl_shader(&shader).expect("a GL program");
                programs += 1;

                for block in program.uniform_blocks() {
                    let info = source
                        .uniform_blocks
                        .iter()
                        .find(|info| info.id == block.id)
                        .unwrap_or_else(|| panic!("{what}: block at {} without a table entry", block.id));
                    assert_eq!(
                        block.size.next_multiple_of(16),
                        info.size,
                        "{what}: the size of {} differs from its UBO's",
                        info.name
                    );
                }

                // With every property an attribute, the program has every
                // attribute of its table but the ones its GLSL leaves unused.
                let attributes = program.get_vertex_attributes();
                let mut locations = HashSet::new();
                for info in source.attributes {
                    if let Some(attribute) = attributes.get(info.id) {
                        assert!(
                            locations.insert(attribute.get_index()),
                            "{what}: two attributes at location {}",
                            attribute.get_index()
                        );
                    }
                }
                if i == 0 {
                    assert!(!locations.is_empty(), "{what}: no vertex attributes");
                }
                for info in source.textures {
                    assert!(program.get_sampler_location(info.id).is_some(), "{what}: no sampler {}", info.name);
                }
            }
        }
    }
    assert!(programs >= 2 * REGISTERED_SHADERS.len());
}
