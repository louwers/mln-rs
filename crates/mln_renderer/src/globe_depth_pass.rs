// Ports src/mln/renderer/globe_depth_pass.hpp and src/mln/renderer/globe_depth_pass.cpp.
//
// The `MLN_UBO_CONSOLIDATION` path, as for the WebGPU backend: the projection
// blocks live in one layer-level array, so the `GlobeDepthTweaker` the other
// backends use is not ported.

use std::collections::{BTreeMap, BTreeSet};

use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::renderer::layer_group::LayerGroupBasePtr;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE, ID_PROJECTION_UBO};
use mln_gfx::uniform_buffer::UniformBufferPtr;
use mln_gfx::{AttributeDataType, Context, DepthMaskType, DrawMode, ShaderProgramBasePtr, ShaderRegistry};
use mln_map_types::TransformState;
use mln_tile_id::OverscaledTileID;
use mln_util::range::Range;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use crate::globe_tile_mesh::{RawGlobeTileMesh, raw_globe_tile_mesh};
use crate::layer_tweaker::to_projection_ubo;
use crate::render_pass::RenderPass;
use crate::update_parameters::UpdateParameters;
use crate::util::tile_cover::{TileCoverParameters, tile_cover};

const GLOBE_DEPTH_SHADER_NAME: &str = "GlobeDepthShader";

/// Writes the planet into the depth buffer ahead of the translucent pass, so
/// that 3D geometry behind the horizon is hidden by the globe the way the 2D
/// layers are by their clip Z.
#[derive(Default)]
pub struct GlobeDepthPass {
    layer_group: Option<LayerGroupBasePtr>,
    shader: Option<ShaderProgramBasePtr>,
    projection_uniform_buffer: Option<UniformBufferPtr>,
    meshes: BTreeMap<(u8, bool, bool), RawGlobeTileMesh>,
}

impl GlobeDepthPass {
    /// `needed` is whether any layer draws 3D geometry this frame; without
    /// one the planet's depth is never read.
    pub fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &dyn Context,
        state: &TransformState,
        update_parameters: &UpdateParameters,
        needed: bool,
    ) {
        if !state.is_globe_rendering() || !needed {
            if let Some(layer_group) = &self.layer_group {
                layer_group.borrow_mut().clear_drawables();
            }
            if !state.is_globe_rendering() {
                self.meshes.clear();
            }
            return;
        }

        if self.shader.is_none() {
            self.shader = context.get_generic_shader(shaders, GLOBE_DEPTH_SHADER_NAME, ProjectionVariant::Globe);
        }
        if self.shader.is_none() {
            return;
        }
        if self.layer_group.is_none() {
            const INITIAL_CAPACITY: usize = 64;
            self.layer_group = context.create_tile_layer_group(0, INITIAL_CAPACITY, "globe-depth".to_owned());
        }
        let Some(layer_group) = self.layer_group.clone() else {
            return;
        };

        let zoom = state.get_integer_zoom();
        let tile_cover = tile_cover(
            &TileCoverParameters {
                transform_state: state.clone(),
                tile_lod_min_radius: update_parameters.tile_lod_min_radius,
                tile_lod_scale: update_parameters.tile_lod_scale,
                tile_lod_pitch_threshold: update_parameters.tile_lod_pitch_threshold,
                tile_lod_mode: update_parameters.tile_lod_mode,
                requested_zoom: None,
                round_zoom: false,
            },
            zoom,
            Range::new(0u8, zoom),
            None,
        );

        let mut group = layer_group.borrow_mut();
        let Some(tile_layer_group) = group.as_tile_layer_group_mut() else {
            return;
        };

        let covered: BTreeSet<OverscaledTileID> = tile_cover.iter().copied().collect();
        tile_layer_group
            .remove_drawables_if(|drawable| drawable.get_tile_id().is_some_and(|tile_id| !covered.contains(&tile_id)));

        let mut builder: Option<mln_gfx::UniqueDrawableBuilder> = None;
        for tile_id in &tile_cover {
            if tile_layer_group.get_drawable_count_for(RenderPass::Translucent, tile_id) > 0 {
                continue;
            }
            let builder = builder.get_or_insert_with(|| {
                let mut builder = context.create_drawable_builder("globe-depth".to_owned());
                builder.set_shader(self.shader.clone());
                builder.set_render_pass(RenderPass::Translucent);
                builder.set_is_3d(true);
                builder.set_enable_color(false);
                builder.set_enable_depth(true);
                builder.set_depth_type(DepthMaskType::ReadWrite);
                // The far hemisphere is clockwise on screen on every backend,
                // Vulkan included, since its shaders flip Y.
                builder.set_cull_face_mode(&CullFaceMode::back_ccw());
                builder.set_vertex_attr_id(ID_GLOBE_DEPTH_POS_VERTEX_ATTRIBUTE);
                builder
            });

            // The grid depends only on the zoom and whether the tile touches
            // a pole.
            let canonical = &tile_id.canonical;
            let key = (canonical.z, canonical.y == 0, canonical.y == (1u32 << canonical.z) - 1);
            let mesh = self
                .meshes
                .entry(key)
                .or_insert_with(|| raw_globe_tile_mesh(canonical, true, &SubdivisionGranularitySetting::globe().tile));
            builder.set_raw_vertices(mesh.vertices.clone(), mesh.vertex_count, AttributeDataType::Short2);
            builder.set_segments(DrawMode::Triangles(), mesh.indices.clone(), &mesh.segments);
            builder.flush(context);

            for mut drawable in builder.clear_drawables() {
                drawable.set_tile_id(*tile_id);
                tile_layer_group.add_drawable(RenderPass::Translucent, tile_id, drawable);
            }
        }

        // The projection blocks live in one layer-level array, indexed like
        // the other consolidated UBOs.
        let mut projection_ubos: Vec<ProjectionUBO> = Vec::with_capacity(tile_layer_group.get_drawable_count());
        tile_layer_group.visit_drawables(|drawable| {
            let Some(tile_id) = drawable.get_tile_id() else {
                return;
            };
            drawable.set_ubo_index(projection_ubos.len() as u32);
            projection_ubos.push(to_projection_ubo(&state.get_projection_data(&tile_id.to_unwrapped())));
        });
        if !projection_ubos.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&projection_ubos);
            match &self.projection_uniform_buffer {
                Some(buffer) if buffer.get_size() >= bytes.len() => buffer.update(bytes),
                _ => self.projection_uniform_buffer = Some(context.create_uniform_buffer(bytes, false, true)),
            }
            group.mutable_uniform_buffers().set(ID_PROJECTION_UBO, self.projection_uniform_buffer.clone());
        }
    }

    pub fn get_layer_group(&self) -> Option<&LayerGroupBasePtr> {
        self.layer_group.as_ref()
    }
}
