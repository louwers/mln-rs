// Ports include/mln/vulkan/pipeline.hpp and src/mln/vulkan/pipeline.cpp
//
// Differences from C++:
// - `setRenderable` takes the render pass and extent, which the port's
//   render pass keeps (`RenderPass::get_vulkan_render_pass`).
// - The scissor rectangle is clipped to the attachments.
// - The viewport's depth range is the depth mode's: the port's renderer
//   orders its layers by it (`PaintParameters::depth_mode_for_sublayer`),
//   as C++'s does for OpenGL. C++'s other backends have the order in the
//   projection matrix, and a viewport of the whole depth range.

use ash::vk;
use mln_gfx::color_mode::{BlendFunction, ColorMode};
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::draw_mode::DrawMode;
use mln_gfx::gfx_types::{AttributeDataType, DrawModeType};
use mln_gfx::scissor_rect::ScissorRect;
use mln_gfx::stencil_mode::{StencilMode, Test};
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    DepthMaskType, StencilFunctionType, StencilOpType,
};
use mln_gfx::util_hash::{hash, hash_combine};

use crate::renderer_backend::BackendImpl;

/// Dynamic values (not part of the pipeline/ignored in hash).
#[derive(Clone, Copy, Debug)]
pub struct DynamicValues {
    pub blend_constants: Option<[f32; 4]>,
    pub stencil_write_mask: u32,
    pub stencil_compare_mask: u32,
    pub stencil_ref: u32,
    pub line_width: f32,
    /// The viewport's depth range.
    pub depth_range: (f32, f32),
}

#[derive(Clone, Debug)]
pub struct PipelineInfo {
    /// Used to select the pipeline layout.
    pub use_push_constants: bool,

    pub topology: vk::PrimitiveTopology,
    pub cull_mode: vk::CullModeFlags,
    pub front_face: vk::FrontFace,
    pub polygon_mode: vk::PolygonMode,

    pub color_blend: bool,
    pub color_blend_function: vk::BlendOp,
    pub src_blend_factor: vk::BlendFactor,
    pub dst_blend_factor: vk::BlendFactor,
    pub color_mask: vk::ColorComponentFlags,

    pub depth_test: bool,
    pub depth_write: bool,
    pub depth_function: vk::CompareOp,

    pub stencil_test: bool,
    pub stencil_function: vk::CompareOp,
    pub stencil_pass: vk::StencilOp,
    pub stencil_fail: vk::StencilOp,
    pub stencil_depth_fail: vk::StencilOp,

    pub wide_lines: bool,

    // external values (used in hash)
    pub render_pass: vk::RenderPass,
    pub view_extent: vk::Extent2D,
    pub scissor_rect: vk::Rect2D,

    pub dynamic_values: DynamicValues,

    pub input_bindings: Vec<vk::VertexInputBindingDescription>,
    pub input_attributes: Vec<vk::VertexInputAttributeDescription>,

    vertex_input_hash: u64,
}

impl Default for PipelineInfo {
    fn default() -> Self {
        PipelineInfo {
            use_push_constants: false,
            topology: vk::PrimitiveTopology::TRIANGLE_LIST,
            cull_mode: vk::CullModeFlags::NONE,
            front_face: vk::FrontFace::CLOCKWISE,
            polygon_mode: vk::PolygonMode::FILL,
            color_blend: true,
            color_blend_function: vk::BlendOp::ADD,
            src_blend_factor: vk::BlendFactor::ONE,
            dst_blend_factor: vk::BlendFactor::ZERO,
            color_mask: vk::ColorComponentFlags::RGBA,
            depth_test: false,
            depth_write: true,
            depth_function: vk::CompareOp::ALWAYS,
            stencil_test: false,
            stencil_function: vk::CompareOp::NEVER,
            stencil_pass: vk::StencilOp::KEEP,
            stencil_fail: vk::StencilOp::KEEP,
            stencil_depth_fail: vk::StencilOp::KEEP,
            wide_lines: false,
            render_pass: vk::RenderPass::null(),
            view_extent: vk::Extent2D::default(),
            scissor_rect: vk::Rect2D::default(),
            dynamic_values: DynamicValues {
                blend_constants: None,
                stencil_write_mask: 0,
                stencil_compare_mask: 0,
                stencil_ref: 0,
                line_width: 1.0,
                depth_range: (0.0, 1.0),
            },
            input_bindings: Vec::new(),
            input_attributes: Vec::new(),
            vertex_input_hash: 0,
        }
    }
}

impl PipelineInfo {
    pub fn vulkan_format(value: AttributeDataType) -> vk::Format {
        use AttributeDataType as A;
        match value {
            A::Byte => vk::Format::R8_SINT,
            A::Byte2 => vk::Format::R8G8_SINT,
            A::Byte3 => vk::Format::R8G8B8_SINT,
            A::Byte4 => vk::Format::R8G8B8A8_SINT,
            A::UByte => vk::Format::R8_UINT,
            A::UByte2 => vk::Format::R8G8_UINT,
            A::UByte3 => vk::Format::R8G8B8_UINT,
            A::UByte4 => vk::Format::R8G8B8A8_UINT,
            A::UByte4Normalized => vk::Format::R8G8B8A8_UNORM,
            A::Short => vk::Format::R16_SINT,
            A::Short2 => vk::Format::R16G16_SINT,
            A::Short3 => vk::Format::R16G16B16_SINT,
            A::Short4 => vk::Format::R16G16B16A16_SINT,
            A::UShort => vk::Format::R16_UINT,
            A::UShort2 => vk::Format::R16G16_UINT,
            A::UShort3 => vk::Format::R16G16B16_UINT,
            A::UShort4 => vk::Format::R16G16B16A16_UINT,
            A::Int => vk::Format::R32_SINT,
            A::Int2 => vk::Format::R32G32_SINT,
            A::Int3 => vk::Format::R32G32B32_SINT,
            A::Int4 => vk::Format::R32G32B32A32_SINT,
            A::UInt => vk::Format::R32_UINT,
            A::UInt2 => vk::Format::R32G32_UINT,
            A::UInt3 => vk::Format::R32G32B32_UINT,
            A::UInt4 => vk::Format::R32G32B32A32_UINT,
            A::Float => vk::Format::R32_SFLOAT,
            A::Float2 => vk::Format::R32G32_SFLOAT,
            A::Float3 => vk::Format::R32G32B32_SFLOAT,
            A::Float4 => vk::Format::R32G32B32A32_SFLOAT,
            _ => vk::Format::UNDEFINED,
        }
    }

    pub fn vulkan_primitive_topology(value: DrawModeType) -> vk::PrimitiveTopology {
        match value {
            DrawModeType::Points => vk::PrimitiveTopology::POINT_LIST,
            DrawModeType::Lines => vk::PrimitiveTopology::LINE_LIST,
            DrawModeType::LineLoop | DrawModeType::LineStrip => vk::PrimitiveTopology::LINE_STRIP,
            DrawModeType::Triangles => vk::PrimitiveTopology::TRIANGLE_LIST,
            DrawModeType::TriangleStrip => vk::PrimitiveTopology::TRIANGLE_STRIP,
            DrawModeType::TriangleFan => vk::PrimitiveTopology::TRIANGLE_FAN,
            #[allow(unreachable_patterns)]
            _ => vk::PrimitiveTopology::TRIANGLE_LIST,
        }
    }

    pub fn vulkan_cull_mode(value: CullFaceSideType) -> vk::CullModeFlags {
        match value {
            CullFaceSideType::Back => vk::CullModeFlags::BACK,
            CullFaceSideType::Front => vk::CullModeFlags::FRONT,
            CullFaceSideType::FrontAndBack => vk::CullModeFlags::FRONT_AND_BACK,
        }
    }

    /// The rectangle, with what lies outside `extent` cut off.
    pub fn vulkan_scissor_rect(value: &ScissorRect, extent: vk::Extent2D) -> vk::Rect2D {
        let x = value.x.clamp(0, extent.width as i32);
        let y = value.y.clamp(0, extent.height as i32);
        vk::Rect2D {
            offset: vk::Offset2D { x, y },
            extent: vk::Extent2D {
                width: value.width.min(extent.width - x as u32),
                height: value.height.min(extent.height - y as u32),
            },
        }
    }

    pub fn vulkan_front_face(value: CullFaceWindingType) -> vk::FrontFace {
        match value {
            CullFaceWindingType::Clockwise => vk::FrontFace::CLOCKWISE,
            CullFaceWindingType::CounterClockwise => vk::FrontFace::COUNTER_CLOCKWISE,
        }
    }

    pub fn vulkan_blend_op(value: ColorBlendEquationType) -> vk::BlendOp {
        match value {
            ColorBlendEquationType::Add => vk::BlendOp::ADD,
            ColorBlendEquationType::Subtract => vk::BlendOp::SUBTRACT,
            ColorBlendEquationType::ReverseSubtract => vk::BlendOp::REVERSE_SUBTRACT,
        }
    }

    pub fn vulkan_blend_factor(value: ColorBlendFactorType) -> vk::BlendFactor {
        use ColorBlendFactorType as F;
        match value {
            F::Zero => vk::BlendFactor::ZERO,
            F::One => vk::BlendFactor::ONE,
            F::SrcColor => vk::BlendFactor::SRC_COLOR,
            F::OneMinusSrcColor => vk::BlendFactor::ONE_MINUS_SRC_COLOR,
            F::SrcAlpha => vk::BlendFactor::SRC_ALPHA,
            F::OneMinusSrcAlpha => vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
            F::DstAlpha => vk::BlendFactor::DST_ALPHA,
            F::OneMinusDstAlpha => vk::BlendFactor::ONE_MINUS_DST_ALPHA,
            F::DstColor => vk::BlendFactor::DST_COLOR,
            F::OneMinusDstColor => vk::BlendFactor::ONE_MINUS_DST_COLOR,
            F::SrcAlphaSaturate => vk::BlendFactor::SRC_ALPHA_SATURATE,
            F::ConstantColor => vk::BlendFactor::CONSTANT_COLOR,
            F::OneMinusConstantColor => vk::BlendFactor::ONE_MINUS_CONSTANT_COLOR,
            F::ConstantAlpha => vk::BlendFactor::CONSTANT_ALPHA,
            F::OneMinusConstantAlpha => vk::BlendFactor::ONE_MINUS_CONSTANT_ALPHA,
        }
    }

    pub fn vulkan_depth_compare_op(value: DepthFunctionType) -> vk::CompareOp {
        match value {
            DepthFunctionType::Never => vk::CompareOp::NEVER,
            DepthFunctionType::Less => vk::CompareOp::LESS,
            DepthFunctionType::Equal => vk::CompareOp::EQUAL,
            DepthFunctionType::LessEqual => vk::CompareOp::LESS_OR_EQUAL,
            DepthFunctionType::Greater => vk::CompareOp::GREATER,
            DepthFunctionType::NotEqual => vk::CompareOp::NOT_EQUAL,
            DepthFunctionType::GreaterEqual => vk::CompareOp::GREATER_OR_EQUAL,
            DepthFunctionType::Always => vk::CompareOp::ALWAYS,
        }
    }

    pub fn vulkan_stencil_compare_op(value: StencilFunctionType) -> vk::CompareOp {
        match value {
            StencilFunctionType::Never => vk::CompareOp::NEVER,
            StencilFunctionType::Less => vk::CompareOp::LESS,
            StencilFunctionType::Equal => vk::CompareOp::EQUAL,
            StencilFunctionType::LessEqual => vk::CompareOp::LESS_OR_EQUAL,
            StencilFunctionType::Greater => vk::CompareOp::GREATER,
            StencilFunctionType::NotEqual => vk::CompareOp::NOT_EQUAL,
            StencilFunctionType::GreaterEqual => vk::CompareOp::GREATER_OR_EQUAL,
            StencilFunctionType::Always => vk::CompareOp::ALWAYS,
        }
    }

    pub fn vulkan_stencil_op(value: StencilOpType) -> vk::StencilOp {
        match value {
            StencilOpType::Zero => vk::StencilOp::ZERO,
            StencilOpType::Keep => vk::StencilOp::KEEP,
            StencilOpType::Replace => vk::StencilOp::REPLACE,
            StencilOpType::Increment => vk::StencilOp::INCREMENT_AND_CLAMP,
            StencilOpType::Decrement => vk::StencilOp::DECREMENT_AND_CLAMP,
            StencilOpType::Invert => vk::StencilOp::INVERT,
            StencilOpType::IncrementWrap => vk::StencilOp::INCREMENT_AND_WRAP,
            StencilOpType::DecrementWrap => vk::StencilOp::DECREMENT_AND_WRAP,
        }
    }

    pub fn set_cull_mode(&mut self, value: &CullFaceMode) {
        self.cull_mode = if value.enabled { Self::vulkan_cull_mode(value.side) } else { vk::CullModeFlags::NONE };
        self.front_face = Self::vulkan_front_face(value.winding);
    }

    /// `setScissorRect(value)`; call it after `set_render_target`, whose
    /// extent it is clipped to.
    pub fn set_scissor_rect(&mut self, value: &ScissorRect) {
        self.scissor_rect = Self::vulkan_scissor_rect(value, self.view_extent);
    }

    pub fn set_draw_mode(&mut self, value: &DrawMode) {
        self.topology = Self::vulkan_primitive_topology(value.type_);
        if matches!(value.type_, DrawModeType::Lines | DrawModeType::LineStrip | DrawModeType::LineLoop) {
            self.set_line_width(value.size);
        } else {
            self.set_line_width(1.0);
        }
    }

    pub fn set_color_blend(&mut self, value: &ColorMode) {
        self.set_color_blend_function(&value.blend_function);
        self.dynamic_values.blend_constants = self.uses_blend_constants().then(|| {
            let color = value.blend_color;
            [color.r, color.g, color.b, color.a]
        });

        self.color_mask = vk::ColorComponentFlags::empty();
        for (enabled, bit) in [
            (value.mask.r, vk::ColorComponentFlags::R),
            (value.mask.g, vk::ColorComponentFlags::G),
            (value.mask.b, vk::ColorComponentFlags::B),
            (value.mask.a, vk::ColorComponentFlags::A),
        ] {
            if enabled {
                self.color_mask |= bit;
            }
        }
    }

    pub fn set_color_blend_function(&mut self, value: &BlendFunction) {
        self.color_blend = !matches!(value, BlendFunction::Replace);
        self.color_blend_function = Self::vulkan_blend_op(value.equation());
        self.src_blend_factor = Self::vulkan_blend_factor(value.src_factor());
        self.dst_blend_factor = Self::vulkan_blend_factor(value.dst_factor());
    }

    pub fn set_depth_write(&mut self, value: DepthMaskType) {
        self.depth_write = value == DepthMaskType::ReadWrite;
    }

    pub fn set_depth_mode(&mut self, value: &DepthMode) {
        self.depth_function = Self::vulkan_depth_compare_op(value.func);
        self.set_depth_write(value.mask);
        self.dynamic_values.depth_range = (value.range.min, value.range.max);
    }

    pub fn set_stencil_mode(&mut self, value: &StencilMode) {
        if value.test == Test::Always {
            self.stencil_test = false;
            self.stencil_function = vk::CompareOp::NEVER;
            return;
        }
        self.stencil_test = true;
        if value.test == Test::Never {
            self.stencil_function = vk::CompareOp::NEVER;
            return;
        }

        self.stencil_function = Self::vulkan_stencil_compare_op(value.test.func());
        self.dynamic_values.stencil_compare_mask = value.test.masked().unwrap_or(0);
        self.dynamic_values.stencil_write_mask = value.mask;
        self.dynamic_values.stencil_ref = value.ref_ as u32;

        self.stencil_pass = Self::vulkan_stencil_op(value.pass);
        self.stencil_fail = Self::vulkan_stencil_op(value.fail);
        self.stencil_depth_fail = Self::vulkan_stencil_op(value.depth_fail);
    }

    /// `setRenderable(value)`: the render pass the pipeline draws in, and
    /// the extent of its attachments.
    pub fn set_render_target(&mut self, render_pass: vk::RenderPass, extent: vk::Extent2D) {
        self.render_pass = render_pass;
        self.view_extent = extent;
    }

    pub fn set_line_width(&mut self, value: f32) {
        self.wide_lines = value != 1.0;
        self.dynamic_values.line_width = value;
    }

    pub fn uses_blend_constants(&self) -> bool {
        let constant = |factor| {
            matches!(
                factor,
                vk::BlendFactor::CONSTANT_ALPHA
                    | vk::BlendFactor::CONSTANT_COLOR
                    | vk::BlendFactor::ONE_MINUS_CONSTANT_ALPHA
                    | vk::BlendFactor::ONE_MINUS_CONSTANT_COLOR
            )
        };
        constant(self.src_blend_factor) || constant(self.dst_blend_factor)
    }

    pub fn update_vertex_input_hash(&mut self) {
        let mut seed = 0;
        for value in &self.input_attributes {
            hash_combine(&mut seed, u64::from(value.binding));
            hash_combine(&mut seed, value.format.as_raw() as u64);
            hash_combine(&mut seed, u64::from(value.location));
            hash_combine(&mut seed, u64::from(value.offset));
        }
        for value in &self.input_bindings {
            hash_combine(&mut seed, u64::from(value.binding));
            hash_combine(&mut seed, u64::from(value.stride));
            hash_combine(&mut seed, value.input_rate.as_raw() as u64);
        }
        self.vertex_input_hash = seed;
    }

    pub fn hash(&self) -> u64 {
        use ash::vk::Handle;
        hash(&[
            u64::from(self.use_push_constants),
            self.topology.as_raw() as u64,
            u64::from(self.cull_mode.as_raw()),
            self.front_face.as_raw() as u64,
            self.polygon_mode.as_raw() as u64,
            u64::from(self.color_blend),
            self.color_blend_function.as_raw() as u64,
            self.src_blend_factor.as_raw() as u64,
            self.dst_blend_factor.as_raw() as u64,
            u64::from(self.color_mask.as_raw()),
            u64::from(self.depth_test),
            u64::from(self.depth_write),
            self.depth_function.as_raw() as u64,
            u64::from(self.stencil_test),
            self.stencil_function.as_raw() as u64,
            self.stencil_pass.as_raw() as u64,
            self.stencil_fail.as_raw() as u64,
            self.stencil_depth_fail.as_raw() as u64,
            u64::from(self.wide_lines),
            self.render_pass.as_raw(),
            self.vertex_input_hash,
        ])
    }

    /// Whether drawing with this pipeline sets the line width.
    fn dynamic_line_width(&self, backend: &BackendImpl) -> bool {
        backend.get_device_features().wide_lines == vk::TRUE && self.wide_lines
    }

    pub fn set_dynamic_values(&self, backend: &BackendImpl, buffer: vk::CommandBuffer) {
        let device = backend.get_device();
        // SAFETY: `buffer` is recording, and each value set is a dynamic
        // state of the pipelines made from this info.
        unsafe {
            if let Some(blend_constants) = &self.dynamic_values.blend_constants {
                device.cmd_set_blend_constants(buffer, blend_constants);
            }
            if self.stencil_test {
                let faces = vk::StencilFaceFlags::FRONT_AND_BACK;
                device.cmd_set_stencil_write_mask(buffer, faces, self.dynamic_values.stencil_write_mask);
                device.cmd_set_stencil_compare_mask(buffer, faces, self.dynamic_values.stencil_compare_mask);
                device.cmd_set_stencil_reference(buffer, faces, self.dynamic_values.stencil_ref);
            }
            if self.dynamic_line_width(backend) {
                device.cmd_set_line_width(buffer, self.dynamic_values.line_width);
            }
            let viewport = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: self.view_extent.width as f32,
                height: self.view_extent.height as f32,
                min_depth: self.dynamic_values.depth_range.0,
                max_depth: self.dynamic_values.depth_range.1,
            };
            device.cmd_set_viewport(buffer, 0, &[viewport]);
            device.cmd_set_scissor(buffer, 0, &[self.scissor_rect]);
        }
    }

    pub fn get_dynamic_states(&self, backend: &BackendImpl) -> Vec<vk::DynamicState> {
        let mut dynamic_states = Vec::new();
        if self.uses_blend_constants() {
            dynamic_states.push(vk::DynamicState::BLEND_CONSTANTS);
        }
        if self.stencil_test {
            dynamic_states.push(vk::DynamicState::STENCIL_COMPARE_MASK);
            dynamic_states.push(vk::DynamicState::STENCIL_WRITE_MASK);
            dynamic_states.push(vk::DynamicState::STENCIL_REFERENCE);
        }
        if self.dynamic_line_width(backend) {
            dynamic_states.push(vk::DynamicState::LINE_WIDTH);
        }
        dynamic_states.push(vk::DynamicState::VIEWPORT);
        dynamic_states.push(vk::DynamicState::SCISSOR);
        dynamic_states
    }
}
