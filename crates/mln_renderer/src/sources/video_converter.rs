//! Turns a video frame's planes, in memory or on the GPU, into the RGBA
//! texture that a video source's raster layers sample, with one draw of the
//! video frame shader.

use mln_gfx::buffer::BufferPtr;
use mln_gfx::gpu::{
    BindGroupEntry, BindingsDesc, BufferUsage, ColorAttachment, DepthState, Draw, DynamicState, FixedState,
    GeometryDesc, GeometryId, Gpu, GpuError, LoadOp, PassDesc, PipelineId, PipelineKey, Scissor, ShaderId, ShaderKey,
    StencilState, TargetFormats, TextureFormat, TextureId, VertexAttribute, VertexBufferLayout, VertexStep,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{
    ID_PROJECTION_UBO, ID_VIDEO_FRAME_PLANE0_TEXTURE, ID_VIDEO_FRAME_PLANE1_TEXTURE, ID_VIDEO_FRAME_PLANE2_TEXTURE,
    ID_VIDEO_FRAME_POS_VERTEX_ATTRIBUTE, ID_VIDEO_FRAME_PROPS_UBO,
};
use mln_gfx::shaders::shader_source::BuiltIn;
use mln_gfx::shaders::video_frame_ubo::VideoFramePropsUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::video::{FrameKeepAlive, FramePixels, ImportedFrame, PlaneLayout};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMode, DrawModeType, StencilMode};
use mln_util::Color;
use mln_util::mat4::{Mat4, matrix};
use mln_util::size::Size;

use crate::gfx::context::Context;
use crate::gfx::owned::Owned;
use crate::gfx::texture2d::Texture2D;
use crate::gfx::uniform_buffer::UniformBufferPtr;
use crate::sources::video_texture::{chroma_size, tight_rows};

/// The textures a frame's planes are in for its draw.
enum Planes {
    /// Written from memory to the converter's textures.
    Uploaded(Vec<TextureId>),
    /// The backend's import of a GPU frame, which goes after the draw.
    Imported(Vec<Owned<TextureId>>),
}

impl Planes {
    fn ids(&self) -> Vec<TextureId> {
        match self {
            Planes::Uploaded(ids) => ids.clone(),
            Planes::Imported(owned) => owned.iter().map(Owned::id).collect(),
        }
    }
}

/// The program, pipeline and geometry of one layout's draws.
struct Program {
    layout: PlaneLayout,
    external: bool,
    shader: ShaderId,
    pipeline: PipelineId,
    geometry: Owned<GeometryId>,
}

/// What converts a video source's frames: the textures its frames in memory
/// are written to, and what draws them.
pub(crate) struct VideoConverter {
    /// The planes of frames in memory, made at the first frame of each size.
    planes: [Option<Texture2D>; 3],
    programs: Vec<Program>,
    /// The unit square the draws cover, and its indices.
    quad: Option<(BufferPtr, BufferPtr)>,
    props: Option<UniformBufferPtr>,
    /// A projection block for the backends whose prelude declares one that
    /// every program binds (OpenGL's), which the shader doesn't read.
    projection: Option<UniformBufferPtr>,
}

/// The vertex layout of the square: a position of two shorts.
fn layout() -> VertexBufferLayout {
    VertexBufferLayout {
        stride: 4,
        step: VertexStep::Vertex,
        attributes: vec![VertexAttribute {
            id: ID_VIDEO_FRAME_POS_VERTEX_ATTRIBUTE as u32,
            format: AttributeDataType::Short2,
            offset: 0,
        }],
    }
}

/// The matrix that maps the unit square onto a pass's whole target, its
/// top edge at clip space's top, so that the target's texels have the rows
/// of an uploaded image (OpenGL's shader flips them for its bottom-first
/// targets).
fn target_matrix() -> [f32; 16] {
    let mut ortho: Mat4 = [0.0; 16];
    matrix::ortho(&mut ortho, 0.0, 1.0, 1.0, 0.0, -1.0, 1.0);
    ortho.map(|value| value as f32)
}

impl VideoConverter {
    pub(crate) fn new() -> VideoConverter {
        VideoConverter { planes: [None, None, None], programs: Vec::new(), quad: None, props: None, projection: None }
    }

    /// Writes the frame of `size` with `pixels` to `target`, an RGBA
    /// texture that passes draw into, at the frame's size. The backend holds
    /// `keep_alive` while it reads a GPU frame. Fails with
    /// [`GpuError::Unsupported`] for a GPU frame the backend doesn't import.
    pub(crate) fn convert(
        &mut self,
        context: &Context,
        size: Size,
        pixels: &FramePixels,
        keep_alive: FrameKeepAlive,
        target: &mut Texture2D,
    ) -> Result<(), GpuError> {
        let (layout, color, external, planes) = match pixels {
            FramePixels::Nv12 { y, uv, color } => {
                let chroma = chroma_size(size);
                let planes = vec![
                    self.upload(
                        context,
                        0,
                        TextureFormat::R8Unorm,
                        size,
                        &tight_rows(y, size.width as usize, size.height as usize)?,
                    )?,
                    self.upload(
                        context,
                        1,
                        TextureFormat::Rg8Unorm,
                        chroma,
                        &tight_rows(uv, chroma.width as usize * 2, chroma.height as usize)?,
                    )?,
                ];
                (PlaneLayout::Nv12, *color, false, Planes::Uploaded(planes))
            }
            FramePixels::I420 { y, u, v, color } => {
                let chroma = chroma_size(size);
                let (width, height) = (chroma.width as usize, chroma.height as usize);
                let planes = vec![
                    self.upload(
                        context,
                        0,
                        TextureFormat::R8Unorm,
                        size,
                        &tight_rows(y, size.width as usize, size.height as usize)?,
                    )?,
                    self.upload(context, 1, TextureFormat::R8Unorm, chroma, &tight_rows(u, width, height)?)?,
                    self.upload(context, 2, TextureFormat::R8Unorm, chroma, &tight_rows(v, width, height)?)?,
                ];
                (PlaneLayout::I420, *color, false, Planes::Uploaded(planes))
            }
            FramePixels::Gpu(frame) => {
                let ImportedFrame { layout, planes, color, external } =
                    context.gpu().import_video_frame(frame, keep_alive)?;
                let owned = planes.into_iter().map(|id| Owned::new(id, context.released())).collect();
                (layout, color, external, Planes::Imported(owned))
            }
            FramePixels::Rgba(_) => return Err(GpuError::Invalid("an RGBA frame needs no conversion".to_owned())),
            _ => return Err(GpuError::Unsupported("this kind of video frame".to_owned())),
        };

        target.set_format(TextureFormat::Rgba8Unorm);
        target.set_size(size);
        let target_id = target.create(context)?;
        let [to_r, to_g, to_b] = color.to_rgb_rows();
        context.write_uniform_buffer_pod(
            &mut self.props,
            &VideoFramePropsUBO { matrix: target_matrix(), to_rgb: [to_r, to_g, to_b] },
        );
        if self.projection.is_none() {
            context.write_uniform_buffer_pod(&mut self.projection, &ProjectionUBO::default());
        }
        if self.quad.is_none() {
            let vertices: [i16; 8] = [0, 0, 1, 0, 0, 1, 1, 1];
            let indices: [u16; 6] = [0, 1, 2, 1, 3, 2];
            self.quad = Some((
                context.buffer(BufferUsage::Vertex, bytemuck::cast_slice(&vertices))?,
                context.buffer(BufferUsage::Index, bytemuck::cast_slice(&indices))?,
            ));
        }
        self.draw(context, size, layout, external, &planes.ids(), target_id)
    }

    /// Plane `index`'s texture of `format` and `size`, holding `data`.
    fn upload(
        &mut self,
        context: &Context,
        index: usize,
        format: TextureFormat,
        size: Size,
        data: &[u8],
    ) -> Result<TextureId, GpuError> {
        let plane = self.planes[index].get_or_insert_with(|| Texture2D::new(std::rc::Rc::clone(context.released())));
        plane.set_format(format);
        plane.upload_data(context, data, size)?;
        plane.create(context)
    }

    fn draw(
        &mut self,
        context: &Context,
        size: Size,
        layout: PlaneLayout,
        external: bool,
        planes: &[TextureId],
        target: TextureId,
    ) -> Result<(), GpuError> {
        let (Some(props), Some(projection), Some((vertices, indices))) = (&self.props, &self.projection, &self.quad)
        else {
            return Err(GpuError::Invalid("the video frame shader's buffers weren't made".to_owned()));
        };
        let mut gpu = context.gpu();
        let gpu = &mut *gpu;
        let program =
            if let Some(index) = self.programs.iter().position(|p| p.layout == layout && p.external == external) {
                &self.programs[index]
            } else {
                let program = Self::program(gpu, context, layout, external, vertices, indices)?;
                self.programs.push(program);
                self.programs.last().ok_or_else(|| GpuError::Invalid("no program".to_owned()))?
            };
        let sampler = context.sampler(
            gpu,
            SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                max_anisotropy: 1,
                mipmapped: false,
            },
        )?;
        // Every plane id binds a texture: the layouts with fewer planes bind
        // the first again where the shader reads none.
        let plane = |index: usize| planes.get(index).or(planes.first()).copied();
        let mut entries = vec![
            BindGroupEntry::Buffer {
                id: ID_VIDEO_FRAME_PROPS_UBO as u32,
                buffer: props.id(),
                offset: 0,
                size: size_of::<VideoFramePropsUBO>() as u64,
            },
            BindGroupEntry::Buffer {
                id: ID_PROJECTION_UBO as u32,
                buffer: projection.id(),
                offset: 0,
                size: size_of::<ProjectionUBO>() as u64,
            },
        ];
        for (index, id) in [ID_VIDEO_FRAME_PLANE0_TEXTURE, ID_VIDEO_FRAME_PLANE1_TEXTURE, ID_VIDEO_FRAME_PLANE2_TEXTURE]
            .into_iter()
            .enumerate()
        {
            let texture = plane(index).ok_or_else(|| GpuError::Invalid("a video frame without planes".to_owned()))?;
            entries.push(BindGroupEntry::Texture { id: id as u32, texture, sampler });
        }
        let bindings = Owned::new(
            gpu.create_bindings(&BindingsDesc { label: "video frame", shader: program.shader, entries: &entries })?,
            context.released(),
        );
        gpu.begin_pass(&PassDesc {
            label: "video frame",
            color: Some(ColorAttachment { texture: target, load: LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 1.0)) }),
            depth_stencil: None,
        })?;
        let disabled = ColorMode::disabled();
        let drawn = gpu.draw(&Draw {
            pipeline: program.pipeline,
            bindings: bindings.id(),
            geometry: program.geometry.id(),
            first_index: 0,
            index_count: 6,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
            global_index: 0,
            dynamic: DynamicState {
                depth_range: (0.0, 1.0),
                scissor: Scissor { x: 0, y: 0, width: size.width, height: size.height },
                stencil_reference: 0,
                blend_constant: disabled.blend_color,
                line_width: 1.0,
            },
        });
        let ended = gpu.end_pass();
        drawn.and(ended)?;
        let mut stats = context.rendering_stats();
        stats.num_draw_calls += 1;
        stats.total_draw_calls += 1;
        Ok(())
    }

    /// The shader, pipeline and geometry of the draws of `layout`.
    fn program(
        gpu: &mut dyn Gpu,
        context: &Context,
        layout: PlaneLayout,
        external: bool,
        vertices: &BufferPtr,
        indices: &BufferPtr,
    ) -> Result<Program, GpuError> {
        let defines: &[&str] = match layout {
            PlaneLayout::Rgba => &[],
            PlaneLayout::Nv12 => &["VIDEO_YUV", "VIDEO_NV12"],
            PlaneLayout::I420 => &["VIDEO_YUV"],
        };
        let mut defines: Vec<String> = defines.iter().map(|&define| define.to_owned()).collect();
        if external {
            defines.push("VIDEO_EXTERNAL".to_owned());
        }
        let shader = context.shader(gpu, &ShaderKey::new(BuiltIn::VideoFrameShader, defines))?;
        let key = PipelineKey {
            shader,
            vertex_buffers: vec![self::layout()],
            state: FixedState {
                primitive: DrawModeType::Triangles,
                cull: CullFaceMode::disabled(),
                blend: ColorMode::unblended().blend_function,
                color_mask: ColorMode::unblended().mask,
                depth: DepthState::from(&DepthMode::disabled()),
                stencil: StencilState::from(&StencilMode::disabled()),
            },
            targets: TargetFormats { color: Some(TextureFormat::Rgba8Unorm), depth_stencil: None },
        };
        let pipeline = context.pipeline(gpu, &key)?;
        let geometry = gpu.create_geometry(&GeometryDesc {
            shader,
            layouts: &[self::layout()],
            vertex_buffers: &[(vertices.id(), 0)],
            index_buffer: indices.id(),
        })?;
        Ok(Program { layout, external, shader, pipeline, geometry: Owned::new(geometry, context.released()) })
    }
}
