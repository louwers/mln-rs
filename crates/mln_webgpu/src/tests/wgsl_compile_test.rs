// Compiles every built-in program's WGSL as `WebGpu::create_shader` does, in
// both projection variants, with and without the overdraw inspector, with
// every property it declares read per vertex and with every one read from
// its uniform.

use std::collections::BTreeSet;

use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::program::Program;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::source::wgsl;

use crate::Device;
use crate::index_table::IndexTable;
use crate::shaders::shader_program::compile;
use crate::shaders::shader_source::{BUILT_INS, prelude};

/// Each program with WGSL: the engine's and the built-in layer types'.
fn programs() -> Vec<(&'static Program, &'static wgsl::Source)> {
    let layers = mln_layers::shader_sources();
    BUILT_INS.iter().copied().chain(layers.of::<wgsl::Source>().iter()).collect()
}

/// The names after the `HAS_UNIFORM_u_` that `source`'s WGSL tests.
fn uniforms(source: &wgsl::Source) -> BTreeSet<String> {
    const PREFIX: &str = "HAS_UNIFORM_u_";
    let wgsl = source.wgsl;
    let mut names = BTreeSet::new();
    for text in [wgsl.prelude, wgsl.vertex, wgsl.fragment].into_iter().flatten() {
        for (start, _) in text.match_indices(PREFIX) {
            let rest = &text[start + PREFIX.len()..];
            let end = rest.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(rest.len());
            names.insert(rest[..end].to_owned());
        }
    }
    names
}

/// A variant's defines read a property from its uniform only where the
/// program declares it.
#[test]
fn every_program_declares_the_properties_its_wgsl_reads_from_uniforms() {
    for (program, source) in programs() {
        let declared: BTreeSet<String> = program.properties.iter().map(|property| property.name.to_owned()).collect();
        assert_eq!(uniforms(source), declared, "{}", program.name);
    }
}

/// Every program that draws a layer outputs white for the overdraw
/// inspector, which counts its draws.
#[test]
fn every_layer_program_shows_the_overdraw_inspector() {
    use mln_gfx::shaders::programs::{ATMOSPHERE, CLIPPING_MASK, COLLISION_BOX, COLLISION_CIRCLE, GLOBE_DEPTH, SKY};
    let not_layers = [&CLIPPING_MASK, &COLLISION_BOX, &COLLISION_CIRCLE, &GLOBE_DEPTH, &SKY, &ATMOSPHERE];
    for (program, source) in programs() {
        if !not_layers.contains(&program) {
            assert!(
                [source.wgsl.prelude, source.wgsl.vertex, source.wgsl.fragment]
                    .into_iter()
                    .flatten()
                    .any(|text| text.contains("OVERDRAW_INSPECTOR")),
                "{}",
                program.name
            );
        }
    }
}

#[test]
fn every_program_compiles() {
    let device = Device::new(None).expect("device");
    let device = device.device();
    let index_table = IndexTable::new(device);
    let mut failed = Vec::new();
    let parameters = [false, true].map(|overdraw| ProgramParameters::new(1.0, overdraw));
    for (program, source) in programs() {
        for parameters in &parameters {
            for variant in [ProjectionVariant::Mercator, ProjectionVariant::Globe] {
                for uniforms in [&[][..], program.properties] {
                    let key = program.variant_key(parameters, uniforms, variant);
                    match compile(device, index_table.layout(), &key, source) {
                        Ok(compiled) => {
                            let wgsl = source.wgsl;
                            let text = [Some(prelude()), wgsl.prelude, wgsl.vertex, wgsl.fragment];
                            assert_eq!(
                                compiled.index_group().is_some(),
                                text.into_iter().flatten().any(|text| text.contains("globalIndex")),
                                "{} binds globalIndex in a group of its own",
                                program.name
                            );
                        }
                        Err(error) => {
                            failed.push(format!("{} ({variant:?}, {:?}): {error}", program.name, key.defines()));
                        }
                    }
                }
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
