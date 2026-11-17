use mln_gfx::shaders::shader_source::BuiltIn;
use mln_gfx::shaders::{
    background_layer_ubo as background, circle_layer_ubo as circle, color_relief_layer_ubo as color_relief,
    fill_extrusion_layer_ubo as fill_extrusion, fill_layer_ubo as fill, heatmap_layer_ubo as heatmap,
    heatmap_texture_layer_ubo as heatmap_texture, hillshade_layer_ubo as hillshade,
    hillshade_prepare_layer_ubo as hillshade_prepare, layer_composite_ubo, layer_ubo, line_layer_ubo as line,
    raster_layer_ubo as raster, sky_ubo, symbol_layer_ubo as symbol,
};

use crate::{MAX_EXHAUSTIVE_FLAGS, PROGRAMS, ResourceKind, Stage, Target, flags_of, link, validate, variants};

mod direct;

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
        Target::WebGpu => {
            validate(&program.wgsl)?;
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

/// Every variant of every program validates for every target, and naga
/// writes each target's language from it.
#[test]
fn every_variant_of_every_program_translates() {
    let failures: Vec<String> = std::thread::scope(|scope| {
        let threads: Vec<_> = Target::ALL
            .into_iter()
            .map(|target| {
                scope.spawn(move || {
                    let mut failures = Vec::new();
                    for &(program, module) in PROGRAMS {
                        for flags in variants(&flags_of(program), MAX_EXHAUSTIVE_FLAGS) {
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
    let flags = flags_of(BuiltIn::CircleShader);
    for flag in ["HAS_UNIFORM_u_color", "PROJECTION_GLOBE", "OVERDRAW_INSPECTOR"] {
        assert!(flags.iter().any(|f| f == flag), "{flags:?}");
    }
    assert!(!flags.iter().any(|f| f == "gl" || f == "vulkan"), "{flags:?}");
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

/// The renderer's UBO structs by name: the bytes of one, and of an element of
/// the layer's buffer that holds one for each drawable, where the renderer
/// writes such a buffer.
const UBOS: &[(&str, usize, usize)] = &[
    ("GlobalPaintParamsUBO", size_of::<layer_ubo::GlobalPaintParamsUBO>(), 0),
    ("ProjectionUBO", size_of::<layer_ubo::ProjectionUBO>(), size_of::<layer_ubo::ProjectionUBO>()),
    ("SkyPropsUBO", size_of::<sky_ubo::SkyPropsUBO>(), 0),
    ("AtmospherePropsUBO", size_of::<sky_ubo::AtmospherePropsUBO>(), 0),
    ("BackgroundPropsUBO", size_of::<background::BackgroundPropsUBO>(), 0),
    ("BackgroundPatternPropsUBO", size_of::<background::BackgroundPatternPropsUBO>(), 0),
    (
        "BackgroundPatternDrawableUBO",
        size_of::<background::BackgroundPatternDrawableUBO>(),
        size_of::<background::BackgroundDrawableUnionUBO>(),
    ),
    ("CircleDrawableUBO", size_of::<circle::CircleDrawableUBO>(), size_of::<circle::CircleDrawableUBO>()),
    ("CircleEvaluatedPropsUBO", size_of::<circle::CircleEvaluatedPropsUBO>(), 0),
    ("CollisionDrawableUBO", size_of::<symbol::CollisionDrawableUBO>(), 0),
    ("CollisionTilePropsUBO", size_of::<symbol::CollisionTilePropsUBO>(), 0),
    (
        "ColorReliefTilePropsUBO",
        size_of::<color_relief::ColorReliefTilePropsUBO>(),
        size_of::<color_relief::ColorReliefTilePropsUBO>(),
    ),
    ("ColorReliefEvaluatedPropsUBO", size_of::<color_relief::ColorReliefEvaluatedPropsUBO>(), 0),
    // The debug overlay's, which no layer draws.
    ("DebugUBO", 96, 0),
    ("FillDrawableUBO", size_of::<fill::FillDrawableUBO>(), size_of::<fill::FillDrawableUnionUBO>()),
    ("FillOutlineDrawableUBO", size_of::<fill::FillOutlineDrawableUBO>(), size_of::<fill::FillDrawableUnionUBO>()),
    ("FillPatternDrawableUBO", size_of::<fill::FillPatternDrawableUBO>(), size_of::<fill::FillDrawableUnionUBO>()),
    (
        "FillOutlineTriangulatedDrawableUBO",
        size_of::<fill::FillOutlineTriangulatedDrawableUBO>(),
        size_of::<fill::FillDrawableUnionUBO>(),
    ),
    ("FillPatternTilePropsUBO", size_of::<fill::FillPatternTilePropsUBO>(), size_of::<fill::FillTilePropsUnionUBO>()),
    ("FillEvaluatedPropsUBO", size_of::<fill::FillEvaluatedPropsUBO>(), 0),
    (
        "FillExtrusionDrawableUBO",
        size_of::<fill_extrusion::FillExtrusionDrawableUBO>(),
        size_of::<fill_extrusion::FillExtrusionDrawableUBO>(),
    ),
    (
        "FillExtrusionTilePropsUBO",
        size_of::<fill_extrusion::FillExtrusionTilePropsUBO>(),
        size_of::<fill_extrusion::FillExtrusionTilePropsUBO>(),
    ),
    ("FillExtrusionPropsUBO", size_of::<fill_extrusion::FillExtrusionPropsUBO>(), 0),
    ("HeatmapDrawableUBO", size_of::<heatmap::HeatmapDrawableUBO>(), size_of::<heatmap::HeatmapDrawableUBO>()),
    ("HeatmapEvaluatedPropsUBO", size_of::<heatmap::HeatmapEvaluatedPropsUBO>(), 0),
    ("HeatmapTexturePropsUBO", size_of::<heatmap_texture::HeatmapTexturePropsUBO>(), 0),
    ("LayerCompositeUBO", size_of::<layer_composite_ubo::LayerCompositeUBO>(), 0),
    (
        "HillshadeTilePropsUBO",
        size_of::<hillshade::HillshadeTilePropsUBO>(),
        size_of::<hillshade::HillshadeTilePropsUBO>(),
    ),
    ("HillshadeEvaluatedPropsUBO", size_of::<hillshade::HillshadeEvaluatedPropsUBO>(), 0),
    ("HillshadePrepareDrawableUBO", size_of::<hillshade_prepare::HillshadePrepareDrawableUBO>(), 0),
    ("HillshadePrepareTilePropsUBO", size_of::<hillshade_prepare::HillshadePrepareTilePropsUBO>(), 0),
    ("LineDrawableUBO", size_of::<line::LineDrawableUBO>(), size_of::<line::LineDrawableUnionUBO>()),
    ("LineGradientDrawableUBO", size_of::<line::LineGradientDrawableUBO>(), size_of::<line::LineDrawableUnionUBO>()),
    ("LinePatternDrawableUBO", size_of::<line::LinePatternDrawableUBO>(), size_of::<line::LineDrawableUnionUBO>()),
    ("LineSDFDrawableUBO", size_of::<line::LineSDFDrawableUBO>(), size_of::<line::LineDrawableUnionUBO>()),
    ("LinePatternTilePropsUBO", size_of::<line::LinePatternTilePropsUBO>(), size_of::<line::LineTilePropsUnionUBO>()),
    ("LineSDFTilePropsUBO", size_of::<line::LineSDFTilePropsUBO>(), size_of::<line::LineTilePropsUnionUBO>()),
    ("LineEvaluatedPropsUBO", size_of::<line::LineEvaluatedPropsUBO>(), 0),
    ("RasterEvaluatedPropsUBO", size_of::<raster::RasterEvaluatedPropsUBO>(), 0),
    ("SymbolDrawableUBO", size_of::<symbol::SymbolDrawableUBO>(), size_of::<symbol::SymbolDrawableUBO>()),
    ("SymbolTilePropsUBO", size_of::<symbol::SymbolTilePropsUBO>(), size_of::<symbol::SymbolTilePropsUBO>()),
    ("SymbolEvaluatedPropsUBO", size_of::<symbol::SymbolEvaluatedPropsUBO>(), 0),
];

/// Every buffer of every program is one of the renderer's UBOs: a uniform
/// buffer of its size, or a storage buffer of the layer's element size,
/// which a struct smaller than the element pads to.
#[test]
fn every_buffer_is_the_renderers() {
    let mut failures = Vec::new();
    for &(program, module) in PROGRAMS {
        let flags = flags_of(program);
        for target in [Target::WebGpu, Target::Gl] {
            for flags in [Vec::new(), flags.clone()] {
                let linked = link(module, target, &flags).expect("links");
                let validated = validate(&linked.wgsl).expect("validates");
                for resource in &linked.resources {
                    let storage = match resource.kind {
                        ResourceKind::UniformBuffer => false,
                        ResourceKind::StorageBuffer => true,
                        _ => continue,
                    };
                    let naga_module = &validated.module;
                    let (_, global) = naga_module
                        .global_variables
                        .iter()
                        .find(|(_, global)| global.name.as_deref() == Some(resource.name))
                        .expect("a global of each resource");
                    let mut ty = global.ty;
                    if let naga::TypeInner::Array { base, .. } = naga_module.types[ty].inner {
                        ty = base;
                    }
                    let name = naga_module.types[ty].name.as_deref().unwrap_or_default();
                    // WESL names a struct after its module's path and its own.
                    let name = name.rsplit('_').next().unwrap_or_default();
                    if name == "GlobalIndexUBO" {
                        continue;
                    }
                    let at = format!("{module} {target:?} {}", resource.name);
                    match UBOS.iter().find(|&&(ubo, _, _)| ubo == name) {
                        None => failures.push(format!("{at}: {name} is no UBO of the renderer's")),
                        Some(&(_, _, element)) if storage && resource.size != element as u64 => {
                            failures.push(format!("{at}: {name} elements of {} bytes, not {element}", resource.size));
                        }
                        Some(&(_, size, _)) if !storage && resource.size != size as u64 => {
                            failures.push(format!("{at}: {name} of {} bytes, not {size}", resource.size));
                        }
                        Some(_) => {}
                    }
                }
            }
        }
    }
    failures.dedup();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// naga's WGSL of `module`.
fn written_wgsl(module: &naga::Module) -> Result<String, String> {
    use naga::valid::{Capabilities, ValidationFlags, Validator};
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(module)
        .map_err(|error| format!("{error:?}"))?;
    naga::back::wgsl::write_string(module, &info, naga::back::wgsl::WriterFlags::empty())
        .map_err(|error| error.to_string())
}

/// How `module`'s variant of `flags` for `target`, from the embedded text
/// and tables, differs from WESL's direct link of the variant and naga's
/// reflection of it, if it does.
fn compare(module: &'static str, target: Target, flags: &[String]) -> Result<(), String> {
    let direct = direct::link(module, target, flags)?;
    let built = link(module, target, flags)?;
    let validated = validate(&built.wgsl)?;
    let mut ours: Vec<_> = built
        .resources
        .iter()
        .map(|r| (r.name.to_owned(), r.kind, r.set, r.id, r.binding, r.size, r.vertex, r.fragment))
        .collect();
    let mut theirs: Vec<_> = direct
        .resources
        .iter()
        .map(|r| (r.name.clone(), r.kind, r.set, r.id, r.binding, r.size, r.vertex, r.fragment))
        .collect();
    ours.sort_by(|a, b| a.0.cmp(&b.0));
    theirs.sort_by(|a, b| a.0.cmp(&b.0));
    if ours != theirs {
        return Err(format!("resources\n  tables: {ours:?}\n  naga:   {theirs:?}"));
    }
    if built.inputs != direct.inputs {
        return Err(format!("inputs\n  tables: {:?}\n  naga:   {:?}", built.inputs, direct.inputs));
    }
    if built.index_group != direct.index_group {
        return Err(format!("index group {:?}, not {:?}", built.index_group, direct.index_group));
    }
    let (ours, theirs) = (written_wgsl(&validated.module)?, written_wgsl(&direct.module)?);
    if ours != theirs {
        let line = ours.lines().zip(theirs.lines()).position(|(a, b)| a != b).unwrap_or(0);
        let context = |text: &str| text.lines().skip(line.saturating_sub(3)).take(8).collect::<Vec<_>>().join("\n");
        return Err(format!(
            "naga writes other WGSL, from line {line}:\n--- embedded\n{}\n--- direct\n{}",
            context(&ours),
            context(&theirs)
        ));
    }
    Ok(())
}

/// Every variant of every program for every target, from the embedded text
/// and tables, is what WESL links for the variant: both validate, naga
/// writes the same WGSL of them, and the tables give the resources and
/// vertex inputs that naga finds each stage uses.
#[test]
fn every_variant_is_what_wesl_links() {
    let failures: Vec<String> = std::thread::scope(|scope| {
        let threads: Vec<_> = Target::ALL
            .into_iter()
            .map(|target| {
                scope.spawn(move || {
                    let mut failures = Vec::new();
                    for &(program, module) in PROGRAMS {
                        for flags in variants(&flags_of(program), MAX_EXHAUSTIVE_FLAGS) {
                            if let Err(error) = compare(module, target, &flags) {
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
    let shown: Vec<&str> = failures.iter().take(10).map(String::as_str).collect();
    assert!(failures.is_empty(), "{} variants differ:\n{}", failures.len(), shown.join("\n\n"));
}
