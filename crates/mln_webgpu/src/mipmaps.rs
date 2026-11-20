//! The mips of a mipmapped texture, drawn a level at a time from the level
//! before: WebGPU has no command that makes them.

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

/// What draws mips, made for the first texture whose mips are made.
pub(crate) struct Mipmapper {
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
        Mipmapper { module, sampler, group_layout, layout, pipelines: HashMap::default() }
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
