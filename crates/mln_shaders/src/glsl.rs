//! A program's stages as GLSL for OpenGL ES 3.0 and WebGL2, or desktop
//! OpenGL 3.3.
//!
//! GLSL ES 3.00 and GLSL 3.30 have no binding qualifiers: a backend binds
//! each uniform block to the binding point of its buffer's id and each
//! sampler to the texture unit of its texture's id by the names
//! [`GlslStage`] reports. naga combines a texture and the sampler that
//! samples it into one sampler uniform, so a texture is sampled with the
//! texture unit's sampler state.
//!
//! A fragment stage declares the uniform blocks it shares with the vertex
//! stage first, in the vertex stage's order: SwiftShader, which Chrome's
//! WebGL falls back to without a GPU, misreads them otherwise
//! (maplibre/maplibre-native#4625).

use naga::back::glsl;

use crate::{Program, Stage};

/// A stage written as GLSL, and the names its uniforms and inputs have.
#[derive(Clone, Debug)]
pub struct GlslStage {
    pub source: String,
    /// Each uniform block's name, and the index of its resource in
    /// [`Program::resources`].
    pub blocks: Vec<(String, usize)>,
    /// Each sampler uniform's name, and the id of the texture it samples.
    pub samplers: Vec<(String, u32)>,
    /// Each vertex input's name, and its location: its attribute's id.
    pub inputs: Vec<(String, u32)>,
}

impl Program {
    /// `stage` of an OpenGL program as GLSL ES 3.00, or GLSL 3.30 if
    /// `desktop`, written once. Its clip space is the program's.
    pub fn glsl(&self, stage: Stage, desktop: bool) -> Result<&GlslStage, String> {
        self.written.glsl[stage as usize][usize::from(desktop)]
            .get_or_init(|| write(self, stage, desktop))
            .as_ref()
            .map_err(Clone::clone)
    }
}

fn write(program: &Program, stage: Stage, desktop: bool) -> Result<GlslStage, String> {
    let options = glsl::Options {
        version: if desktop { glsl::Version::Desktop(330) } else { glsl::Version::new_gles(300) },
        writer_flags: glsl::WriterFlags::empty(),
        binding_map: glsl::BindingMap::default(),
        zero_initialize_workgroup_memory: false,
    };
    let pipeline = glsl::PipelineOptions {
        shader_stage: stage.naga(),
        entry_point: stage.entry_point().to_owned(),
        multiview: None,
    };
    let fail = |error: glsl::Error| format!("{} {stage:?}: {error}", program.name);
    let mut source = String::new();
    let mut writer = glsl::Writer::new(
        &mut source,
        &program.module,
        &program.info,
        &options,
        &pipeline,
        naga::proc::BoundsCheckPolicies::default(),
    )
    .map_err(fail)?;
    let reflection = writer.write().map_err(fail)?;

    if stage == Stage::Fragment {
        source = shared_blocks_first(program, &source);
    }
    let mut blocks = Vec::new();
    for (handle, name) in reflection.uniforms {
        let index = program
            .resources
            .iter()
            .position(|resource| resource.handle == handle)
            .ok_or_else(|| format!("{}: uniform {name} is no resource", program.name))?;
        blocks.push((name, index));
    }
    blocks.sort();
    let mut samplers = Vec::new();
    for (name, mapping) in reflection.texture_mapping {
        let texture = program
            .resource(mapping.texture)
            .ok_or_else(|| format!("{}: sampler {name} samples no texture", program.name))?;
        samplers.push((name, texture.id));
    }
    samplers.sort();
    // naga names the vertex input of location N `_p2vs_locationN`.
    let inputs = match stage {
        Stage::Vertex => {
            program.inputs.iter().map(|input| (format!("_p2vs_location{}", input.location), input.location)).collect()
        }
        Stage::Fragment => Vec::new(),
    };
    Ok(GlslStage { source, blocks, samplers, inputs })
}

/// `source`, a fragment stage, with the uniform blocks of the resources the
/// vertex stage uses too moved before the others. naga writes each block on
/// a line of its own, in the order of the module's globals, which both
/// stages share.
fn shared_blocks_first(program: &Program, source: &str) -> String {
    const BLOCK: &str = "layout(std140) uniform ";
    let shared = |line: &str| {
        program.resources.iter().any(|resource| {
            let suffix = format!("_group_{}_binding_{}_fs;", resource.binding.group, resource.binding.binding);
            resource.vertex && line.contains(&suffix)
        })
    };
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let Some(first) = lines.iter().position(|line| line.starts_with(BLOCK)) else {
        return source.to_owned();
    };
    let blocks: Vec<&str> = lines[first..].iter().copied().filter(|line| line.starts_with(BLOCK)).collect();
    let (mut ordered, others): (Vec<&str>, Vec<&str>) = blocks.into_iter().partition(|line| shared(line));
    ordered.extend(others);
    let mut ordered = ordered.into_iter();
    lines
        .iter()
        .enumerate()
        .map(
            |(index, line)| {
                if index >= first && line.starts_with(BLOCK) { ordered.next().unwrap_or(line) } else { line }
            },
        )
        .collect()
}
