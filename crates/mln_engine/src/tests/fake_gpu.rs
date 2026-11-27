//! A [`Gpu`] that draws nothing: it keeps what it makes and checks each call
//! as a backend does, so that the engine's tests see the lifecycle of what
//! the engine makes, without a device.
//!
//! A call that names a resource it doesn't have fails with
//! [`GpuError::UnknownId`], destroying a resource that live pipelines,
//! bindings or geometry were made of fails with [`GpuError::InUse`] (in
//! debug builds, as every backend's), and a pass, a draw or a flush out of
//! order fails with [`GpuError::Invalid`]. A readback reads zeros.

use std::any::Any;

use mln_gfx::gpu::validation::{self, BufferInfo, Dependents, TextureInfo};
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BindingsId, BufferDesc, BufferId, Caps, ClipDepthRange, Draw, GeometryDesc,
    GeometryId, Gpu, GpuError, PassDesc, PipelineDesc, PipelineId, ReadbackId, ResourceId, SamplerId, ShaderId,
    ShaderKey, SlotMap, TextureDesc, TextureFormat, TextureId, TextureRegion,
};
use mln_gfx::texture2d::SamplerState;

/// The fake's capabilities: a WebGPU device's, which draws into render
/// targets with a depth attachment and clears the stencil by drawing.
const CAPS: Caps = Caps {
    buffer_offset_alignment: 256,
    max_texture_size: 8192,
    half_float_render_targets: true,
    clip_depth: ClipDepthRange::ZeroToOne,
    render_target_depth: Some(TextureFormat::Depth32Float),
    clear_stencil_with_pass: false,
    cpu_rasterizer: false,
    max_line_width: 1.0,
    binds_vertex_buffers: false,
};

#[derive(Default)]
pub(crate) struct FakeGpu {
    buffers: SlotMap<BufferId, BufferInfo>,
    textures: SlotMap<TextureId, TextureInfo>,
    samplers: SlotMap<SamplerId, ()>,
    shaders: SlotMap<ShaderId, ()>,
    pipelines: SlotMap<PipelineId, ()>,
    bindings: SlotMap<BindingsId, ()>,
    geometry: SlotMap<GeometryId, ()>,
    readbacks: SlotMap<ReadbackId, usize>,
    dependents: Dependents,
    in_pass: bool,
}

impl FakeGpu {
    /// How many resources are live: made and not destroyed, readbacks
    /// included.
    pub(crate) fn live(&self) -> usize {
        self.buffers.len()
            + self.textures.len()
            + self.samplers.len()
            + self.shaders.len()
            + self.pipelines.len()
            + self.bindings.len()
            + self.geometry.len()
            + self.readbacks.len()
    }

    fn buffer(&self, buffer: BufferId) -> Result<BufferInfo, GpuError> {
        self.buffers.get(buffer).copied().ok_or(GpuError::UnknownId("buffer"))
    }

    fn texture(&self, texture: TextureId) -> Result<TextureInfo, GpuError> {
        self.textures.get(texture).copied().ok_or(GpuError::UnknownId("texture"))
    }

    fn shader(&self, shader: ShaderId) -> Result<(), GpuError> {
        self.shaders.get(shader).copied().ok_or(GpuError::UnknownId("shader"))
    }

    fn outside_pass(&self, call: &str) -> Result<(), GpuError> {
        if self.in_pass {
            return Err(GpuError::Invalid(format!("{call} while a pass is open")));
        }
        Ok(())
    }
}

impl Gpu for FakeGpu {
    fn caps(&self) -> &Caps {
        &CAPS
    }

    fn create_buffer(&mut self, desc: &BufferDesc<'_>) -> Result<BufferId, GpuError> {
        Ok(self.buffers.insert(BufferInfo { usage: desc.usage, size: desc.size.next_multiple_of(4) }))
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<(), GpuError> {
        validation::check_write(offset, data.len(), self.buffer(buffer)?.size)
    }

    fn create_texture(&mut self, desc: &TextureDesc<'_>) -> Result<TextureId, GpuError> {
        validation::check_mips(desc)?;
        Ok(self.textures.insert(TextureInfo { format: Some(desc.format), size: desc.size, usage: desc.usage }))
    }

    fn write_texture(&mut self, texture: TextureId, region: TextureRegion, data: &[u8]) -> Result<(), GpuError> {
        let info = self.texture(texture)?;
        let format = info.format.expect("a texture of a format");
        let expected = region.bytes_per_row(info.size, format)? * region.size.height as usize;
        if data.len() != expected {
            return Err(GpuError::Invalid(format!("{} bytes for a region of {expected}", data.len())));
        }
        Ok(())
    }

    fn generate_mipmaps(&mut self, texture: TextureId) -> Result<(), GpuError> {
        self.outside_pass("generate_mipmaps")?;
        self.texture(texture).map(drop)
    }

    fn create_sampler(&mut self, _state: &SamplerState) -> Result<SamplerId, GpuError> {
        Ok(self.samplers.insert(()))
    }

    fn create_shader(&mut self, _key: &ShaderKey) -> Result<ShaderId, GpuError> {
        Ok(self.shaders.insert(()))
    }

    fn create_pipeline(&mut self, desc: &PipelineDesc<'_>) -> Result<PipelineId, GpuError> {
        self.shader(desc.shader)?;
        let pipeline = self.pipelines.insert(());
        self.dependents.pipeline(pipeline, desc.shader);
        Ok(pipeline)
    }

    fn create_bindings(&mut self, desc: &BindingsDesc<'_>) -> Result<BindingsId, GpuError> {
        self.shader(desc.shader)?;
        for entry in desc.entries {
            match *entry {
                BindGroupEntry::Buffer { buffer, .. } => drop(self.buffer(buffer)?),
                BindGroupEntry::Texture { texture, sampler, .. } => {
                    self.texture(texture)?;
                    self.samplers.get(sampler).ok_or(GpuError::UnknownId("sampler"))?;
                }
            }
        }
        let bindings = self.bindings.insert(());
        self.dependents.bindings(bindings, desc);
        Ok(bindings)
    }

    fn create_geometry(&mut self, desc: &GeometryDesc<'_>) -> Result<GeometryId, GpuError> {
        self.shader(desc.shader)?;
        for &(buffer, _) in desc.vertex_buffers {
            self.buffer(buffer)?;
        }
        self.buffer(desc.index_buffer)?;
        let geometry = self.geometry.insert(());
        self.dependents.geometry(geometry, desc);
        Ok(geometry)
    }

    fn destroy(&mut self, resource: ResourceId) -> Result<(), GpuError> {
        self.dependents.destroy(resource)?;
        match resource {
            ResourceId::Buffer(id) => drop(self.buffers.remove(id)),
            ResourceId::Texture(id) => drop(self.textures.remove(id)),
            ResourceId::Sampler(id) => drop(self.samplers.remove(id)),
            ResourceId::Shader(id) => drop(self.shaders.remove(id)),
            ResourceId::Pipeline(id) => drop(self.pipelines.remove(id)),
            ResourceId::Bindings(id) => drop(self.bindings.remove(id)),
            ResourceId::Geometry(id) => drop(self.geometry.remove(id)),
            ResourceId::Readback(id) => drop(self.readbacks.remove(id)),
        }
        Ok(())
    }

    fn begin_pass(&mut self, desc: &PassDesc<'_>) -> Result<(), GpuError> {
        self.outside_pass("begin_pass")?;
        let color = desc.color.map(|color| self.texture(color.texture)).transpose()?;
        let depth = desc.depth_stencil.map(|depth| self.texture(depth.texture)).transpose()?;
        if color.is_none() && depth.is_none() {
            return Err(GpuError::Invalid("a pass without attachments".to_owned()));
        }
        self.in_pass = true;
        Ok(())
    }

    fn draw(&mut self, draw: &Draw) -> Result<(), GpuError> {
        if !self.in_pass {
            return Err(GpuError::Invalid("a draw outside a pass".to_owned()));
        }
        self.pipelines.get(draw.pipeline).ok_or(GpuError::UnknownId("pipeline"))?;
        self.bindings.get(draw.bindings).ok_or(GpuError::UnknownId("bindings"))?;
        self.geometry.get(draw.geometry).ok_or(GpuError::UnknownId("geometry"))?;
        Ok(())
    }

    fn lend(&mut self, f: &mut dyn FnMut(&mut dyn Any)) -> Result<(), GpuError> {
        f(&mut ());
        Ok(())
    }

    fn end_pass(&mut self) -> Result<(), GpuError> {
        if !self.in_pass {
            return Err(GpuError::Invalid("end_pass without a pass".to_owned()));
        }
        self.in_pass = false;
        Ok(())
    }

    fn flush(&mut self, _wait: bool) -> Result<(), GpuError> {
        self.outside_pass("flush")
    }

    fn read_texture(&mut self, texture: TextureId, region: TextureRegion) -> Result<ReadbackId, GpuError> {
        self.outside_pass("read_texture")?;
        let readback = validation::check_readback(self.textures.get(texture).copied(), region)?;
        Ok(self.readbacks.insert(readback.len()))
    }

    fn take_readback(&mut self, readback: ReadbackId, _wait: bool) -> Result<Option<Vec<u8>>, GpuError> {
        let len = self.readbacks.remove(readback).ok_or(GpuError::UnknownId("readback"))?;
        Ok(Some(vec![0; len]))
    }
}
