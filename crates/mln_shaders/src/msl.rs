//! A program as MSL for Metal.
//!
//! Each buffer is in the buffer slot of its id, as the renderer's buffers
//! bind; each texture is in the texture slot of its id, and its sampler in
//! the sampler slot of the texture's id. The vertex buffers bind after the
//! buffers ([`Program::first_vertex_buffer`]), and each attribute is at the
//! index of its id.
//!
//! naga's MSL takes the byte sizes of the buffers that end in an array of any
//! length from a buffer at [`SIZES_BUFFER_SLOT`], to bound their indexes and
//! for `arrayLength`. The programs index unchecked and don't ask for array
//! lengths, so that buffer's contents are never read, but a draw binds one.

use naga::back::msl;

use crate::{Program, ResourceKind, Stage};

/// The buffer slot of the buffer sizes naga's MSL takes: the last one, past
/// the buffers' and the vertex buffers'.
pub const SIZES_BUFFER_SLOT: u8 = 30;

/// The most bytes the buffer sizes of a program take: a `uint` for each
/// buffer that ends in an array of any length.
pub const SIZES_BUFFER_SIZE: usize = 64;

/// A program written as MSL: one library with both entry points.
#[derive(Clone, Debug)]
pub struct Msl {
    pub source: String,
    /// The names of the vertex and fragment functions.
    pub entry_points: [String; 2],
}

impl Msl {
    /// The name of `stage`'s function.
    #[must_use]
    pub fn entry_point(&self, stage: Stage) -> &str {
        &self.entry_points[stage as usize]
    }
}

impl Program {
    /// A Metal program as MSL 2.4, written once. Its clip space is the
    /// program's.
    pub fn msl(&self) -> Result<&Msl, String> {
        self.written.msl.get_or_init(|| write(self)).as_ref().map_err(Clone::clone)
    }

    /// The buffer slot of a Metal program's first vertex buffer: the one
    /// after its buffers' slots. The others follow it, below
    /// [`SIZES_BUFFER_SLOT`].
    #[must_use]
    pub fn first_vertex_buffer(&self) -> usize {
        let buffers = self
            .resources
            .iter()
            .filter(|resource| matches!(resource.kind, ResourceKind::UniformBuffer | ResourceKind::StorageBuffer));
        buffers.map(|resource| resource.id as usize + 1).max().unwrap_or(0)
    }
}

fn write(program: &Program) -> Result<Msl, String> {
    let fail = |error: String| format!("{}: {error}", program.name);
    let arrays = program.resources.iter().filter(|resource| resource.kind == ResourceKind::StorageBuffer).count();
    if arrays * size_of::<u32>() > SIZES_BUFFER_SIZE {
        return Err(fail(format!("{arrays} storage buffers, more than the buffer sizes hold")));
    }
    let mut resources = msl::BindingMap::new();
    for resource in &program.resources {
        let slot = u8::try_from(resource.id).map_err(|_| fail(format!("{} has no slot", resource.name)))?;
        let target = match resource.kind {
            ResourceKind::UniformBuffer | ResourceKind::StorageBuffer => {
                msl::BindTarget { buffer: Some(slot), ..msl::BindTarget::default() }
            }
            ResourceKind::Texture { .. } => msl::BindTarget { texture: Some(slot), ..msl::BindTarget::default() },
            ResourceKind::Sampler => {
                msl::BindTarget { sampler: Some(msl::BindSamplerTarget::Resource(slot)), ..msl::BindTarget::default() }
            }
        };
        resources.insert(resource.binding, target);
    }
    let entry = msl::EntryPointResources { resources, immediates_buffer: None, sizes_buffer: Some(SIZES_BUFFER_SLOT) };
    let options = msl::Options {
        lang_version: (2, 4),
        per_entry_point_map: Stage::ALL
            .into_iter()
            .map(|stage| (stage.entry_point().to_owned(), entry.clone()))
            .collect(),
        fake_missing_bindings: false,
        // No checks added around loops, indexes and integer divisions.
        bounds_check_policies: naga::proc::BoundsCheckPolicies::default(),
        zero_initialize_workgroup_memory: false,
        force_loop_bounding: false,
        emit_int_div_checks: false,
        ..msl::Options::default()
    };
    let pipeline = msl::PipelineOptions::default();
    let (source, info) = msl::write_string(&program.module, &program.info, &options, &pipeline)
        .map_err(|error| fail(error.to_string()))?;
    let name = |stage: Stage| -> Result<String, String> {
        let index = program.entry_point_index(stage);
        info.entry_point_names[index].clone().map_err(|error| fail(format!("{stage:?}: {error}")))
    };
    Ok(Msl { source, entry_points: [name(Stage::Vertex)?, name(Stage::Fragment)?] })
}
