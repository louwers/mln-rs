// Ports include/mln/shaders/vulkan/shader_program.hpp and
// src/mln/shaders/vulkan/shader_program.cpp
//
// Differences from C++:
// - The stages are compiled by the context (`Context::create_program`,
//   with `shaders::compiler` in place of glslang), which hands the program
//   its modules.
// - `vulkan::VertexAttribute`'s storage buffer (`initVertexAttribute`'s
//   `ubo`) is not ported: instance attributes are vertex inputs.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use ash::vk;
use mln_gfx::shader::Shader;
use mln_gfx::shaders::shader_defines::MAX_TEXTURE_COUNT_PER_SHADER;
use mln_gfx::shaders::shader_program_base::ShaderProgramBase;
use mln_gfx::vertex_attribute::VertexAttributeArray;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};

use super::shader_source::{AttributeInfo, TextureInfo};
use crate::context::Context;
use crate::pipeline::PipelineInfo;
use crate::renderer_backend::BackendImpl;

/// `vulkan::ShaderProgram`
pub struct ShaderProgram {
    shader_program_id: SimpleIdentity,
    shader_name: String,
    backend: Rc<BackendImpl>,
    context: Weak<Context>,
    vertex_shader: vk::ShaderModule,
    fragment_shader: vk::ShaderModule,
    /// The pipelines made so far, by `PipelineInfo::hash`.
    pipelines: RefCell<HashMap<u64, vk::Pipeline>>,
    vertex_attributes: VertexAttributeArray,
    instance_attributes: VertexAttributeArray,
    texture_bindings: [Option<usize>; MAX_TEXTURE_COUNT_PER_SHADER],
}

impl ShaderProgram {
    pub const NAME: &'static str = "GenericVulkanShader";

    /// A program of two compiled stages, which it owns.
    pub fn new(
        name: String,
        context: &Context,
        vertex_shader: vk::ShaderModule,
        fragment_shader: vk::ShaderModule,
    ) -> Self {
        ShaderProgram {
            shader_program_id: SimpleIdentity::new(),
            shader_name: name,
            backend: context.get_backend().clone(),
            context: context.weak(),
            vertex_shader,
            fragment_shader,
            pipelines: RefCell::new(HashMap::new()),
            vertex_attributes: VertexAttributeArray::new(),
            instance_attributes: VertexAttributeArray::new(),
            texture_bindings: [None; MAX_TEXTURE_COUNT_PER_SHADER],
        }
    }

    pub fn get_shader_name(&self) -> &str {
        &self.shader_name
    }

    /// `getPipeline(pipelineInfo)`: the pipeline of the program with the
    /// state of `pipeline_info`, made when first asked for.
    pub fn get_pipeline(&self, pipeline_info: &PipelineInfo) -> Option<vk::Pipeline> {
        let hash = pipeline_info.hash();
        if let Some(&pipeline) = self.pipelines.borrow().get(&hash) {
            return Some(pipeline);
        }
        let context = self.context.upgrade()?;

        let vertex_input_state = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&pipeline_info.input_bindings)
            .vertex_attribute_descriptions(&pipeline_info.input_attributes);

        let input_assembly_state = vk::PipelineInputAssemblyStateCreateInfo::default().topology(pipeline_info.topology);

        let renderable_extent = pipeline_info.view_extent;
        let viewport_extent = [vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: renderable_extent.width as f32,
            height: renderable_extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        }];
        let scissor_rect = [vk::Rect2D { offset: vk::Offset2D::default(), extent: renderable_extent }];
        let viewport_state =
            vk::PipelineViewportStateCreateInfo::default().viewports(&viewport_extent).scissors(&scissor_rect);

        let raster_state = vk::PipelineRasterizationStateCreateInfo::default()
            .cull_mode(pipeline_info.cull_mode)
            .front_face(pipeline_info.front_face)
            .polygon_mode(pipeline_info.polygon_mode)
            .line_width(1.0);

        let multisample_state =
            vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);

        let stencil_state = vk::StencilOpState::default()
            .compare_op(pipeline_info.stencil_function)
            .pass_op(pipeline_info.stencil_pass)
            .fail_op(pipeline_info.stencil_fail)
            .depth_fail_op(pipeline_info.stencil_depth_fail);
        let depth_stencil_state = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(pipeline_info.depth_test)
            .depth_write_enable(pipeline_info.depth_write)
            .depth_bounds_test_enable(false)
            .min_depth_bounds(0.0)
            .max_depth_bounds(1.0)
            .depth_compare_op(pipeline_info.depth_function)
            .stencil_test_enable(pipeline_info.stencil_test)
            .front(stencil_state)
            .back(stencil_state);

        let color_blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(pipeline_info.color_blend)
            .color_blend_op(pipeline_info.color_blend_function)
            .src_color_blend_factor(pipeline_info.src_blend_factor)
            .dst_color_blend_factor(pipeline_info.dst_blend_factor)
            .alpha_blend_op(pipeline_info.color_blend_function)
            .src_alpha_blend_factor(pipeline_info.src_blend_factor)
            .dst_alpha_blend_factor(pipeline_info.dst_blend_factor)
            .color_write_mask(pipeline_info.color_mask)];
        let color_blend_state = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(&color_blend_attachments)
            .logic_op_enable(false)
            .logic_op(vk::LogicOp::COPY);

        // values available for core 1.0
        // vk::DynamicState::eViewport,
        // vk::DynamicState::eScissor,
        // vk::DynamicState::eLineWidth,
        // vk::DynamicState::eStencilCompareMask,
        // vk::DynamicState::eStencilWriteMask,
        // vk::DynamicState::eStencilReference,
        // vk::DynamicState::eBlendConstants,
        // vk::DynamicState::eDepthBias,
        // vk::DynamicState::eDepthBounds,
        let dynamic_values = pipeline_info.get_dynamic_states(&self.backend);
        let dynamic_state = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_values);

        let pipeline_layout = if pipeline_info.use_push_constants {
            context.get_push_constant_pipeline_layout()
        } else {
            context.get_general_pipeline_layout()
        };

        let shader_stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(self.vertex_shader)
                .name(c"main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(self.fragment_shader)
                .name(c"main"),
        ];

        let create_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input_state)
            .input_assembly_state(&input_assembly_state)
            .viewport_state(&viewport_state)
            .rasterization_state(&raster_state)
            .multisample_state(&multisample_state)
            .depth_stencil_state(&depth_stencil_state)
            .color_blend_state(&color_blend_state)
            .dynamic_state(&dynamic_state)
            .layout(pipeline_layout)
            .render_pass(pipeline_info.render_pass);

        // SAFETY: the create info points at live state, the modules are the
        // program's and the render pass is the context's.
        let result = unsafe {
            self.backend.get_device().create_graphics_pipelines(vk::PipelineCache::null(), &[create_info], None)
        };
        let pipeline = match result {
            Ok(pipelines) => pipelines[0],
            Err((_, error)) => {
                Log::error(Event::Shader, format!("{} pipeline creation failed: {error}", self.shader_name));
                debug_assert!(false, "pipeline creation failed");
                return None;
            }
        };
        self.backend.set_debug_name(pipeline, &format!("{}_pipeline", self.shader_name));
        self.pipelines.borrow_mut().insert(hash, pipeline);
        Some(pipeline)
    }

    pub fn has_textures(&self) -> bool {
        self.texture_bindings.iter().any(Option::is_some)
    }

    pub fn init_vertex_attribute(&mut self, info: &AttributeInfo) {
        // Indexes must be unique, if there's a conflict check the
        // `attributes` array in the shader.
        #[cfg(debug_assertions)]
        self.vertex_attributes.visit_attributes(|attrib| debug_assert_ne!(attrib.get_index(), info.index as i32));
        self.vertex_attributes.set(info.id, info.index as i32, info.data_type, 1);
    }

    pub fn init_instance_attribute(&mut self, info: &AttributeInfo) {
        // Index is the input location of the instance attribute. Indexes
        // must not be reused by regular attributes.
        self.instance_attributes.set(info.id, info.index as i32, info.data_type, 1);
    }

    pub fn init_texture(&mut self, info: &TextureInfo) {
        debug_assert!(info.id < self.texture_bindings.len());
        if let Some(binding) = self.texture_bindings.get_mut(info.id) {
            *binding = Some(info.index);
        }
    }
}

impl Drop for ShaderProgram {
    fn drop(&mut self) {
        // SAFETY: pipelines don't need their modules once made.
        unsafe {
            self.backend.get_device().destroy_shader_module(self.vertex_shader, None);
            self.backend.get_device().destroy_shader_module(self.fragment_shader, None);
        }
        let pipelines: Vec<vk::Pipeline> = self.pipelines.borrow_mut().drain().map(|(_, pipeline)| pipeline).collect();
        if pipelines.is_empty() {
            return;
        }
        let backend = self.backend.clone();
        // SAFETY: the commands that use the pipelines have been waited for
        // when the deletion queue runs, or the context is gone.
        let destroy = move || unsafe {
            for pipeline in pipelines {
                backend.get_device().destroy_pipeline(pipeline, None);
            }
        };
        match self.context.upgrade() {
            Some(context) => context.enqueue_deletion(Box::new(move |_| destroy())),
            None => destroy(),
        }
    }
}

impl Shader for ShaderProgram {
    fn type_name(&self) -> &'static str {
        Self::NAME
    }
    fn as_any(&self) -> &dyn Any {
        self
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

/// Downcast a shader handle to the Vulkan program (C++ `static_cast`).
pub fn as_vulkan_shader(shader: &Rc<dyn ShaderProgramBase>) -> Option<&ShaderProgram> {
    shader.as_any().downcast_ref::<ShaderProgram>()
}
