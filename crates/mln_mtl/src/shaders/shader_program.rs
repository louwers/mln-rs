// Ports include/mln/shaders/mtl/shader_program.hpp and
// src/mln/shaders/mtl/shader_program.cpp
//
// Differences from C++:
// - `mtl::VertexAttribute` adds a buffer index to the gfx attribute, which
//   the port's gfx attribute array can't hold; the program keeps the buffer
//   index of each attribute index instead (`get_vertex_buffer_index`).
// - The pipeline state cache is also keyed by the attachment formats it was
//   made for: C++ keys it by the caller's hash alone, which would hand a
//   state made for one renderable to a pass on another.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use mln_gfx::color_mode::{BlendFunction, ColorMode};
use mln_gfx::shader::Shader;
use mln_gfx::shaders::shader_defines::MAX_TEXTURE_COUNT_PER_SHADER;
use mln_gfx::shaders::shader_program_base::ShaderProgramBase;
use mln_gfx::types::{ColorBlendEquationType, ColorBlendFactorType};
use mln_gfx::vertex_attribute::VertexAttributeArray;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLColorWriteMask, MTLDevice, MTLRenderPipelineDescriptor, MTLVertexDescriptor,
};

use super::shader_source::{AttributeInfo, TextureInfo};
use crate::mtl_fwd::{MTLFunctionPtr, MTLRenderPipelineStatePtr};
use crate::render_pass::AttachmentFormats;
use crate::renderer_backend::BackendImpl;

fn metal_blend_operation(equation: ColorBlendEquationType) -> MTLBlendOperation {
    match equation {
        ColorBlendEquationType::Add => MTLBlendOperation::Add,
        ColorBlendEquationType::Subtract => MTLBlendOperation::Subtract,
        ColorBlendEquationType::ReverseSubtract => MTLBlendOperation::ReverseSubtract,
    }
}

fn metal_blend_factor(factor: ColorBlendFactorType) -> MTLBlendFactor {
    match factor {
        ColorBlendFactorType::Zero => MTLBlendFactor::Zero,
        ColorBlendFactorType::One => MTLBlendFactor::One,
        ColorBlendFactorType::SrcColor => MTLBlendFactor::SourceColor,
        ColorBlendFactorType::OneMinusSrcColor => MTLBlendFactor::OneMinusSourceColor,
        ColorBlendFactorType::SrcAlpha => MTLBlendFactor::SourceAlpha,
        ColorBlendFactorType::OneMinusSrcAlpha => MTLBlendFactor::OneMinusSourceAlpha,
        ColorBlendFactorType::DstAlpha => MTLBlendFactor::DestinationAlpha,
        ColorBlendFactorType::OneMinusDstAlpha => MTLBlendFactor::OneMinusDestinationAlpha,
        ColorBlendFactorType::DstColor => MTLBlendFactor::DestinationColor,
        ColorBlendFactorType::OneMinusDstColor => MTLBlendFactor::OneMinusDestinationColor,
        ColorBlendFactorType::SrcAlphaSaturate => MTLBlendFactor::SourceAlphaSaturated,
        ColorBlendFactorType::ConstantColor => MTLBlendFactor::BlendColor,
        ColorBlendFactorType::OneMinusConstantColor => MTLBlendFactor::OneMinusBlendColor,
        ColorBlendFactorType::ConstantAlpha => MTLBlendFactor::BlendAlpha,
        ColorBlendFactorType::OneMinusConstantAlpha => MTLBlendFactor::OneMinusBlendAlpha,
    }
}

/// `mtl::ShaderProgram`
pub struct ShaderProgram {
    shader_program_id: SimpleIdentity,
    shader_name: String,
    backend: Rc<BackendImpl>,
    vertex_function: MTLFunctionPtr,
    fragment_function: Option<MTLFunctionPtr>,
    vertex_attributes: VertexAttributeArray,
    instance_attributes: VertexAttributeArray,
    /// The buffer index of each vertex attribute, by attribute index.
    vertex_buffer_indices: HashMap<usize, usize>,
    /// The buffer index of each instance attribute, by attribute index.
    instance_buffer_indices: HashMap<usize, usize>,
    texture_bindings: [Option<usize>; MAX_TEXTURE_COUNT_PER_SHADER],
    render_pipeline_state_cache: RefCell<HashMap<(u64, AttachmentFormats), MTLRenderPipelineStatePtr>>,
}

impl ShaderProgram {
    pub const NAME: &'static str = "GenericMTLShader";

    #[must_use]
    pub fn new(
        name: String,
        backend: Rc<BackendImpl>,
        vertex_function: MTLFunctionPtr,
        fragment_function: Option<MTLFunctionPtr>,
    ) -> Self {
        ShaderProgram {
            shader_program_id: SimpleIdentity::new(),
            shader_name: name,
            backend,
            vertex_function,
            fragment_function,
            vertex_attributes: VertexAttributeArray::new(),
            instance_attributes: VertexAttributeArray::new(),
            vertex_buffer_indices: HashMap::new(),
            instance_buffer_indices: HashMap::new(),
            texture_bindings: [None; MAX_TEXTURE_COUNT_PER_SHADER],
            render_pipeline_state_cache: RefCell::new(HashMap::new()),
        }
    }

    pub fn get_shader_name(&self) -> &str {
        &self.shader_name
    }

    /// `getRenderPipelineState(renderable, vertexDescriptor, colorMode,
    /// reuseHash)`: the state for a pass with `formats`, reused from earlier
    /// calls with the same `reuse_hash`.
    pub fn get_render_pipeline_state(
        &self,
        formats: AttachmentFormats,
        vertex_descriptor: &MTLVertexDescriptor,
        color_mode: &ColorMode,
        reuse_hash: Option<u64>,
    ) -> Option<MTLRenderPipelineStatePtr> {
        if let Some(hash) = reuse_hash
            && let Some(state) = self.render_pipeline_state_cache.borrow().get(&(hash, formats))
        {
            return Some(state.clone());
        }

        let descriptor = MTLRenderPipelineDescriptor::new();
        descriptor.setLabel(Some(&NSString::from_str(&self.shader_name)));
        descriptor.setVertexFunction(Some(&self.vertex_function));
        descriptor.setFragmentFunction(self.fragment_function.as_deref());
        descriptor.setVertexDescriptor(Some(vertex_descriptor));

        // SAFETY: index 0 is in the attachment array's bounds.
        let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
        let blend_enabled = !matches!(color_mode.blend_function, BlendFunction::Replace);
        let (operation, src_factor, dst_factor) = if blend_enabled {
            let f = &color_mode.blend_function;
            (
                metal_blend_operation(f.equation()),
                metal_blend_factor(f.src_factor()),
                metal_blend_factor(f.dst_factor()),
            )
        } else {
            (MTLBlendOperation::Add, MTLBlendFactor::One, MTLBlendFactor::One)
        };
        color.setPixelFormat(formats.color);
        color.setBlendingEnabled(blend_enabled);
        color.setRgbBlendOperation(operation);
        color.setAlphaBlendOperation(operation);
        color.setSourceRGBBlendFactor(src_factor);
        color.setSourceAlphaBlendFactor(src_factor);
        color.setDestinationRGBBlendFactor(dst_factor);
        color.setDestinationAlphaBlendFactor(dst_factor);
        let mask = color_mode.mask;
        let mut write_mask = MTLColorWriteMask::None;
        for (enabled, bit) in [
            (mask.r, MTLColorWriteMask::Red),
            (mask.g, MTLColorWriteMask::Green),
            (mask.b, MTLColorWriteMask::Blue),
            (mask.a, MTLColorWriteMask::Alpha),
        ] {
            if enabled {
                write_mask |= bit;
            }
        }
        color.setWriteMask(write_mask);
        if let Some(depth) = formats.depth {
            descriptor.setDepthAttachmentPixelFormat(depth);
        }
        if let Some(stencil) = formats.stencil {
            descriptor.setStencilAttachmentPixelFormat(stencil);
        }

        let state = match self.backend.get_device().newRenderPipelineStateWithDescriptor_error(&descriptor) {
            Ok(state) => state,
            Err(error) => {
                Log::error(
                    Event::Shader,
                    format!("{} newRenderPipelineState failed: {}", self.shader_name, error.localizedDescription()),
                );
                debug_assert!(false, "newRenderPipelineState failed");
                return None;
            }
        };
        if let Some(hash) = reuse_hash {
            self.render_pipeline_state_cache.borrow_mut().insert((hash, formats), state.clone());
        }
        Some(state)
    }

    /// The buffer index the vertex attribute at `index` reads from.
    pub fn get_vertex_buffer_index(&self, index: usize) -> Option<usize> {
        self.vertex_buffer_indices.get(&index).copied()
    }

    /// The buffer index the instance attribute at `index` reads from.
    pub fn get_instance_buffer_index(&self, index: usize) -> Option<usize> {
        self.instance_buffer_indices.get(&index).copied()
    }

    pub fn init_vertex_attribute(&mut self, info: &AttributeInfo) {
        // Indexes must be unique, if there's a conflict check the
        // `attributes` array in the shader.
        debug_assert!(!self.vertex_buffer_indices.contains_key(&info.index));
        self.vertex_attributes.set(info.id, info.index as i32, info.data_type, 1);
        self.vertex_buffer_indices.insert(info.index, info.buffer_index);
    }

    pub fn init_instance_attribute(&mut self, info: &AttributeInfo) {
        // Index is the block index of the instance attribute; attributes
        // that share a block share it.
        self.instance_attributes.set(info.id, info.index as i32, info.data_type, 1);
        self.instance_buffer_indices.insert(info.index, info.buffer_index);
    }

    pub fn init_texture(&mut self, info: &TextureInfo) {
        debug_assert!(info.id < self.texture_bindings.len());
        if let Some(binding) = self.texture_bindings.get_mut(info.id) {
            *binding = Some(info.index);
        }
    }
}

impl Shader for ShaderProgram {
    fn type_name(&self) -> &'static str {
        Self::NAME
    }
}

impl ShaderProgramBase for ShaderProgram {
    fn get_id(&self) -> SimpleIdentity {
        self.shader_program_id
    }
    fn get_sampler_location(&self, id: usize) -> Option<usize> {
        self.texture_bindings.get(id).copied().flatten()
    }
    fn get_vertex_attributes(&self) -> &VertexAttributeArray {
        &self.vertex_attributes
    }
    fn get_instance_attributes(&self) -> &VertexAttributeArray {
        &self.instance_attributes
    }
}

/// Downcast a shader handle to the Metal program (C++ `static_cast`).
pub fn as_mtl_shader(shader: &Rc<dyn ShaderProgramBase>) -> Option<&ShaderProgram> {
    shader.downcast_ref::<ShaderProgram>()
}
