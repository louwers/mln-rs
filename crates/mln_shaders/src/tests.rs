use std::collections::BTreeSet;

use crate::{PROGRAMS, ResourceKind, SOURCES, Stage, Target, link};

/// More flags than this, and a program's variants are the flags none, all,
/// each alone and all but each, not every subset.
const MAX_EXHAUSTIVE_FLAGS: usize = 10;

/// The module `path` names, the longest prefix of an import's path that is a
/// module.
fn imported_module(path: &str) -> Option<&'static str> {
    let parts: Vec<&str> = path.split("::").collect();
    (1..=parts.len()).rev().find_map(|len| {
        let name = parts[..len].join("::");
        SOURCES.iter().find(|&&(module, _)| module == name).map(|&(module, _)| module)
    })
}

/// The feature flags that `module` and the modules it imports test, other
/// than the targets'.
fn flags_of(module: &str) -> BTreeSet<String> {
    let mut flags = BTreeSet::new();
    let mut pending = vec![module.to_owned()];
    let mut seen = BTreeSet::new();
    while let Some(module) = pending.pop() {
        if !seen.insert(module.clone()) {
            continue;
        }
        let source = SOURCES.iter().find(|&&(name, _)| name == module).map(|&(_, source)| source).expect("a module");
        for line in source.lines() {
            if let Some(path) = line.trim_start().strip_prefix("import package::") {
                let path = path.split(['{', ';']).next().unwrap_or_default().trim_end_matches("::");
                if let Some(imported) = imported_module(path) {
                    pending.push(imported.to_owned());
                }
            }
        }
        let conditions = ["@if(", "@elif("].into_iter().flat_map(|attribute| source.split(attribute).skip(1));
        for condition in conditions {
            let condition = condition.split(')').next().unwrap_or_default();
            for word in condition.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                if !word.is_empty() && !matches!(word, "true" | "false") {
                    flags.insert(word.to_owned());
                }
            }
        }
    }
    for target in Target::ALL {
        flags.remove(target.feature());
    }
    flags
}

/// The sets of flags a program of `flags` is tested with.
fn variants(flags: &BTreeSet<String>) -> Vec<Vec<String>> {
    let flags: Vec<&String> = flags.iter().collect();
    let subset = |mask: u64| -> Vec<String> {
        flags.iter().enumerate().filter(|&(i, _)| mask & (1 << i) != 0).map(|(_, flag)| (*flag).clone()).collect()
    };
    let all = (1u64 << flags.len()) - 1;
    if flags.len() <= MAX_EXHAUSTIVE_FLAGS {
        return (0..=all).map(subset).collect();
    }
    let mut masks = vec![0, all];
    for i in 0..flags.len() {
        masks.extend([1 << i, all & !(1 << i)]);
    }
    masks.into_iter().map(subset).collect()
}

/// Writes `target`'s language for each stage of `module` with `flags`.
fn write(module: &'static str, target: Target, flags: &[String]) -> Result<(), String> {
    let program = link(module, target, flags)?;
    match target {
        Target::Vulkan => {
            for stage in Stage::ALL {
                program.spirv(stage)?;
            }
        }
        Target::D3d12 => {
            for stage in Stage::ALL {
                program.hlsl(stage)?;
            }
        }
        Target::Gl => {
            for stage in Stage::ALL {
                for desktop in [false, true] {
                    let glsl = program.glsl(stage, desktop)?;
                    for (index, resource) in program.resources.iter().enumerate().filter(|(_, r)| r.used_by(stage)) {
                        let named = match resource.kind {
                            ResourceKind::UniformBuffer | ResourceKind::StorageBuffer => {
                                glsl.blocks.iter().any(|&(_, block)| block == index)
                            }
                            ResourceKind::Texture { .. } => glsl.samplers.iter().any(|&(_, id)| id == resource.id),
                            ResourceKind::Sampler => true,
                        };
                        if !named {
                            return Err(format!("{stage:?} has no GLSL name for {}", resource.name));
                        }
                    }
                    if stage == Stage::Fragment {
                        shared_blocks_come_first(&program, glsl)?;
                    }
                }
            }
        }
        Target::Metal => {
            program.msl()?;
        }
        // The WGSL is the linked source, which `link` validated.
        Target::WebGpu => {
            if program.resources.iter().any(|resource| resource.binding.group > 3) {
                return Err(format!("{module}: more than WebGPU's four groups"));
            }
        }
    }
    Ok(())
}

/// Whether the fragment stage `glsl` declares the uniform blocks it shares
/// with the vertex stage before the others.
fn shared_blocks_come_first(program: &crate::Program, glsl: &crate::glsl::GlslStage) -> Result<(), String> {
    let shared: Vec<bool> = glsl
        .source
        .lines()
        .filter_map(|line| line.strip_prefix("layout(std140) uniform "))
        .map(|line| {
            let name = line.split(' ').next().unwrap_or_default();
            let &(_, index) = glsl.blocks.iter().find(|(block, _)| block == name).expect("a reported block");
            program.resources[index].vertex
        })
        .collect();
    if shared.windows(2).any(|pair| !pair[0] && pair[1]) {
        return Err(format!("the fragment stage declares a block of its own before a shared one: {shared:?}"));
    }
    Ok(())
}

/// Every variant of every program links and validates for every target,
/// and naga writes each target's language from it.
#[test]
fn every_variant_of_every_program_translates() {
    let failures: Vec<String> = std::thread::scope(|scope| {
        let threads: Vec<_> = Target::ALL
            .into_iter()
            .map(|target| {
                scope.spawn(move || {
                    let mut failures = Vec::new();
                    for &(_, module) in PROGRAMS {
                        for flags in variants(&flags_of(module)) {
                            if let Err(error) = write(module, target, &flags) {
                                failures.push(format!("{module} {target:?} {flags:?}: {error}"));
                            }
                        }
                    }
                    failures
                })
            })
            .collect();
        threads.into_iter().flat_map(|thread| thread.join().expect("no panic")).collect()
    });
    assert!(failures.is_empty(), "{} variants fail:\n{}", failures.len(), failures.join("\n\n"));
}

#[test]
fn circle_has_its_flags() {
    let flags = flags_of("programs::CircleShader");
    for flag in ["HAS_UNIFORM_u_color", "PROJECTION_GLOBE", "OVERDRAW_INSPECTOR"] {
        assert!(flags.contains(flag), "{flags:?}");
    }
    assert!(!flags.contains("gl") && !flags.contains("vulkan"), "{flags:?}");
}

/// A variant of a program, for reading: `T=<target> D=<flag>,<flag>
/// P=<BuiltIn> cargo test -p mln-rs-shaders print_variant -- --ignored
/// --nocapture`, with `T` one of vulkan, d3d12, gl, metal, webgpu.
#[test]
#[ignore = "prints a variant for reading"]
fn print_variant() {
    let target = std::env::var("T").unwrap_or_default();
    let target = Target::ALL.into_iter().find(|t| t.feature() == target).unwrap_or(Target::WebGpu);
    let flags: Vec<String> =
        std::env::var("D").unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect();
    let name = std::env::var("P").unwrap_or_else(|_| "CircleShader".to_owned());
    let &(_, module) = PROGRAMS.iter().find(|(built_in, _)| format!("{built_in:?}") == name).expect("a program");
    let program = link(module, target, &flags).expect("links");
    println!("{}", program.wgsl);
    println!("--- resources\n{:#?}\n--- inputs\n{:?}", program.resources, program.inputs);
    match target {
        Target::Gl => {
            for stage in Stage::ALL {
                println!("--- {stage:?}\n{:#?}", program.glsl(stage, false).expect("writes"));
            }
        }
        Target::Metal => println!("--- MSL\n{}", program.msl().expect("writes").source),
        Target::D3d12 => {
            for stage in Stage::ALL {
                println!("--- {stage:?}\n{}", program.hlsl(stage).expect("writes").source);
            }
        }
        Target::Vulkan | Target::WebGpu => {}
    }
}
