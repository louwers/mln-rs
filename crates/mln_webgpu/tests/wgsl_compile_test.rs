// Compiles every WGSL source of `wgsl_sources` the way webgpu::ShaderGroup +
// ShaderProgram do (Prelude, then the struct prelude if any, through the
// preprocessor with the default ProgramParameters defines, in both projection
// variants) and reports which ones wgpu's WGSL front end (naga) rejects. C++
// compiles them with Dawn's Tint, so a failure here is a wgpu-vs-Dawn
// difference.
//
// The shaders on the M1 path must compile; the rest are reported only.

use std::collections::HashMap;

use mln_gfx::backend_scope::{BackendScope, ScopeType};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::renderer_backend::RendererBackend;
use mln_gfx::shader_group::GLOBE_DEFINE;
use mln_webgpu::Device;
use mln_webgpu::shaders::shader_program::ShaderProgram;
use mln_webgpu::shaders::shader_source::{WgslShaderSource, prelude};
use mln_webgpu::shaders::wgsl_preprocessor::preprocess_wgsl;
use mln_webgpu::shaders::wgsl_sources::ALL;

const REQUIRED: &[&str] = &[
    "BackgroundShader",
    "BackgroundPatternShader",
    "ClippingMaskProgram",
    "GlobeDepthShader",
    "SkyShader",
    "AtmosphereShader",
];

/// Prelude, the struct prelude and the stage's source, before preprocessing.
fn assemble(source: &WgslShaderSource, stage: &str) -> String {
    format!("{}\n{}{stage}", prelude(), source.prelude.unwrap_or(""))
}

#[test]
fn all_extracted_wgsl_compiles() {
    let backend = mln_webgpu::RendererBackend::new(&Device::new(None).expect("device"));
    let _scope = BackendScope::new(&backend, ScopeType::Explicit);
    let context = backend.get_context().as_any().downcast_ref::<mln_webgpu::Context>().unwrap();

    for variant in [ProjectionVariant::Mercator, ProjectionVariant::Globe] {
        // ProgramParameters(pixelRatio, overdraw=false): only DEVICE_PIXEL_RATIO.
        let mut defines: HashMap<String, bool> = [("DEVICE_PIXEL_RATIO".to_owned(), true)].into_iter().collect();
        if variant == ProjectionVariant::Globe {
            defines.insert(GLOBE_DEFINE.to_owned(), true);
        }
        let mut failed = Vec::new();
        let mut compiled = 0;
        for source in ALL {
            let (Some(vertex), Some(fragment)) = (source.vertex, source.fragment) else {
                continue; // Prelude
            };
            let vertex = preprocess_wgsl(&assemble(source, vertex), &defines);
            let fragment = preprocess_wgsl(&assemble(source, fragment), &defines);
            let program = ShaderProgram::new(context, &vertex, &fragment, &[], &[], source.name);
            assert_eq!(
                program.get_index_group().is_some(),
                vertex.contains("globalIndex") || fragment.contains("globalIndex"),
                "{} binds globalIndex in a group of its own",
                source.built_in
            );
            if program.has_shader_modules() {
                compiled += 1;
            } else {
                failed.push(source.built_in);
            }
        }
        eprintln!("{variant:?}: compiled {compiled} WGSL shader sources; rejected by naga: {failed:?}");
        for required in REQUIRED {
            assert!(!failed.contains(required), "{required} must compile ({variant:?})");
        }
    }
}

/// Every symbol tested by `#ifdef`/`#ifndef` in the source.
fn conditional_symbols(source: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for line in source.lines() {
        let t = line.trim_start();
        for directive in ["#ifdef", "#ifndef"] {
            if let Some(rest) = t.strip_prefix(directive)
                && let Some(sym) = rest.split_whitespace().next()
            {
                symbols.push(sym.to_owned());
            }
        }
    }
    symbols
}

/// The variant with every conditional defined (e.g. all HAS_UNIFORM_u_*, as
/// when all data-driven properties are constant). Reported, not asserted.
#[test]
fn all_extracted_wgsl_compiles_with_all_conditionals_defined() {
    let backend = mln_webgpu::RendererBackend::new(&Device::new(None).expect("device"));
    let _scope = BackendScope::new(&backend, ScopeType::Explicit);
    let context = backend.get_context().as_any().downcast_ref::<mln_webgpu::Context>().unwrap();
    let mut failed = Vec::new();
    let mut compiled = 0;
    for source in ALL {
        let (Some(vertex), Some(fragment)) = (source.vertex, source.fragment) else {
            continue;
        };
        let vertex = assemble(source, vertex);
        let fragment = assemble(source, fragment);
        let mut defines: HashMap<String, bool> = [("DEVICE_PIXEL_RATIO".to_owned(), true)].into_iter().collect();
        for sym in conditional_symbols(&vertex).into_iter().chain(conditional_symbols(&fragment)) {
            defines.insert(sym, true);
        }
        let program = ShaderProgram::new(
            context,
            &preprocess_wgsl(&vertex, &defines),
            &preprocess_wgsl(&fragment, &defines),
            &[],
            &[],
            source.name,
        );
        if program.has_shader_modules() {
            compiled += 1;
        } else {
            failed.push(source.built_in);
        }
    }
    eprintln!("all-defined variant: compiled {compiled}; rejected by naga: {failed:?}");
}
