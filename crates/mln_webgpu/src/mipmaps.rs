//! The mips of a mipmapped texture, each level made from the level before:
//! WebGPU has no command that makes them. One compute pass writes all the
//! levels of a texture whose format shaders can write, `Rgba8Unorm`, on a
//! device that [`computes`]; a render pass for each level draws the others,
//! which costs the pass's bookkeeping, and the driver's, for every level.

use mln_util::hash::HashMap;

/// A triangle over the target, and each texel of it sampled at its center
/// in the level before, between the four texels it covers there, which the
/// linear sampler averages.
const SHADER: &str = r"
@group(0) @binding(0) var level: texture_2d<f32>;
@group(0) @binding(1) var linear: sampler;

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Varyings {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return Varyings(vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0), uv);
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    return textureSampleLevel(level, linear, in.uv, 0.0);
}
";

/// Each texel of a level written from the level before, as [`SHADER`]
/// draws it, in a compute pass: one pass for all the levels of a texture
/// whose format a shader can write, where drawing them takes a render pass
/// for each.
const COMPUTE_SHADER: &str = r"
@group(0) @binding(0) var level: texture_2d<f32>;
@group(0) @binding(1) var linear: sampler;
@group(0) @binding(2) var next: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(next);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    textureStore(next, id.xy, textureSampleLevel(level, linear, uv, 0.0));
}
";

/// The side of [`COMPUTE_SHADER`]'s workgroups.
const WORKGROUP: u32 = 8;

/// Whether a device of `limits` runs [`COMPUTE_SHADER`]: one with compute
/// shaders and storage textures, which a device of WebGL 2's limits lacks.
pub(crate) fn computes(limits: &wgpu::Limits) -> bool {
    limits.max_storage_textures_per_shader_stage >= 1
        && limits.max_compute_workgroup_size_x >= WORKGROUP
        && limits.max_compute_workgroup_size_y >= WORKGROUP
        && limits.max_compute_invocations_per_workgroup >= WORKGROUP * WORKGROUP
}

/// What writes mips in a compute pass.
struct Compute {
    group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

/// What draws mips, made for the first texture whose mips are made.
pub(crate) struct Mipmapper {
    compute: Option<Compute>,
    module: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    group_layout: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    /// A pipeline for each format whose mips were made.
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl Mipmapper {
    pub(crate) fn new(device: &wgpu::Device) -> Mipmapper {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mipmaps"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mipmaps"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mipmaps"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mipmaps"),
            bind_group_layouts: &[Some(&group_layout)],
            immediate_size: 0,
        });
        Mipmapper { compute: None, module, sampler, group_layout, layout, pipelines: HashMap::default() }
    }

    /// Records into `encoder` one compute pass that writes each of `levels`
    /// after the first from the level before: single-level views of an
    /// `Rgba8Unorm` texture of `size` that shaders can write, from level 0.
    /// `groups` are the texture's, what each level's write binds, which the
    /// first call makes. In a benchmark's build, `queries` measure the pass
    /// as they do the renderer's.
    pub(crate) fn generate_in_compute(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        size: wgpu::Extent3d,
        levels: &[wgpu::TextureView],
        groups: &mut Vec<wgpu::BindGroup>,
        #[cfg(feature = "bench")] queries: Option<&mut crate::bench::Queries>,
    ) {
        let compute = self.compute.get_or_insert_with(|| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("mipmaps"),
                source: wgpu::ShaderSource::Wgsl(COMPUTE_SHADER.into()),
            });
            let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("mipmaps"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                ],
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("mipmaps"),
                bind_group_layouts: &[Some(&group_layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("mipmaps"),
                layout: Some(&layout),
                module: &module,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
            Compute { group_layout, pipeline }
        });
        if groups.is_empty() {
            groups.extend(levels.windows(2).map(|pair| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("mipmaps"),
                    layout: &compute.group_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&pair[0]) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&pair[1]) },
                    ],
                })
            }));
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("mipmaps"),
            #[cfg(feature = "bench")]
            timestamp_writes: queries.as_deref().and_then(crate::bench::Queries::compute_timestamp_writes),
            #[cfg(not(feature = "bench"))]
            timestamp_writes: None,
        });
        #[cfg(feature = "bench")]
        let counted = queries.is_some_and(|queries| queries.begin_compute(&mut pass));
        pass.set_pipeline(&compute.pipeline);
        for (level, group) in (1u32..).zip(&*groups) {
            let side = |side: u32| (side >> level).max(1).div_ceil(WORKGROUP);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(side(size.width), side(size.height), 1);
        }
        #[cfg(feature = "bench")]
        if counted {
            crate::bench::Queries::end_compute(&mut pass);
        }
    }

    /// Records into `encoder` a pass for each of `levels`, single-level
    /// views of a texture of `format` from level 0, after the first, which
    /// draws it from the level before. In a benchmark's build, `queries`
    /// measure each pass as they do the renderer's.
    pub(crate) fn generate(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        format: wgpu::TextureFormat,
        levels: &[wgpu::TextureView],
        #[cfg(feature = "bench")] mut queries: Option<&mut crate::bench::Queries>,
    ) {
        let pipeline = self.pipelines.entry(format).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mipmaps"),
                layout: Some(&self.layout),
                vertex: wgpu::VertexState {
                    module: &self.module,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.module,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        for pair in levels.windows(2) {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mipmaps"),
                layout: &self.group_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&pair[0]) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mipmaps"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &pair[1],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                #[cfg(feature = "bench")]
                timestamp_writes: queries.as_deref().and_then(crate::bench::Queries::timestamp_writes),
                #[cfg(not(feature = "bench"))]
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            #[cfg(feature = "bench")]
            let counted = queries.as_deref_mut().is_some_and(|queries| queries.begin(&mut pass));
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
            #[cfg(feature = "bench")]
            if counted {
                crate::bench::Queries::end(&mut pass);
            }
        }
    }
}
