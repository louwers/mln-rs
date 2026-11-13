// Differences in mechanics, not in output:
// - C++ re-applies the vertex attributes and creates new DEM textures for
//   existing tile drawables every frame; the port rebuilds a tile's
//   drawables only when the bucket's masked geometry changed, and keeps the
//   DEM texture per bucket id.
// - The elevation stops texture is created anew when the ramp changes (C++
//   re-uploads into the same texture; its size may change with the ramp).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::color_relief_layer_ubo::ColorReliefTilePropsUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE, ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE, ID_COLOR_RELIEF_IMAGE_TEXTURE,
    ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE,
};
use mln_gfx::texture2d::{SamplerState, Texture2DPtr};
use mln_gfx::types::{TextureChannelDataType, TextureFilterType, TexturePixelType, TextureWrapType};
use mln_gfx::vertex_attribute::VertexAttributeArrayPtr;
use mln_gfx::vertex_vector::VertexVectorBasePtr;
use mln_gfx::{AttributeDataType, ColorMode, Context, CullFaceMode, DrawMode, ShaderProgramBasePtr, ShaderRegistry};
use mln_map_types::TransformState;
use mln_style::expression::{
    Dependency, EvaluationContext, GlobalStateMap, Interpolate, downcast, from_expression_value,
    global_state_refs_intersect,
};
use mln_style::layer::downcast_arc;
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::ColorReliefLayer;
use mln_style::layers::color_relief_layer_impl::Impl as ColorReliefLayerImpl;
use mln_style::layers::color_relief_layer_properties::{
    ColorReliefColor, ColorReliefLayerProperties, ColorReliefOpacity, ColorReliefPaintProperties, Resampling,
};
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_style_types::ResamplingType;
use mln_util::Color;
use mln_util::identity::SimpleIdentity;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_util::time::Instant;

use super::color_relief_layer_tweaker::{ColorReliefDrawableData, ColorReliefLayerTweaker};
use super::{TileDrawables, drawable_has_layer_tweaker, shared_vertices};
use crate::buckets::hillshade_bucket::{HillshadeBucket, HillshadeLayoutVertex};
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::globe_tile_mesh::GlobeTileMeshCache;
use crate::layer_tweaker::{LayerTweakerPtr, layer_tweaker_slot};
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{LayerPrepareParameters, RenderLayer, RenderLayerBase};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tree::RenderTree;
use crate::update_parameters::UpdateParameters;

fn impl_cast(impl_: &Arc<dyn layer::Impl>) -> &ColorReliefLayerImpl {
    debug_assert!(std::ptr::eq(impl_.get_type_info(), ColorReliefLayerImpl::static_type_info()));
    impl_.downcast_ref::<ColorReliefLayerImpl>().expect("RenderColorReliefLayer: not a color-relief layer impl")
}

const COLOR_RELIEF_SHADER_GROUP_NAME: &str = "ColorReliefShader";

pub(crate) struct RenderColorReliefLayer {
    base: RenderLayerBase,
    // Paint properties
    unevaluated: ColorReliefPaintProperties::Unevaluated,

    // Color ramp data
    color_ramp_size: u32,
    color_ramp_changed: bool,
    /// The global-state snapshot used to generate the cached color ramp and
    /// detect when it needs to be rebuilt.
    color_ramp_global_state: Option<Arc<GlobalStateMap>>,

    /// Elevation values for each stop (RGBA floats, elevation in R).
    elevation_stops_data: Vec<f32>,
    /// RGB colors for each stop
    color_stops: PremultipliedImage,

    // GPU textures
    elevation_stops_texture: Option<Texture2DPtr>,
    color_stops_texture: Option<Texture2DPtr>,

    color_relief_shader: Option<ShaderProgramBasePtr>,

    static_data_shared_vertices: Option<VertexVectorBasePtr>,
    globe_meshes: GlobeTileMeshCache<HillshadeLayoutVertex>,

    /// The DEM texture of each bucket, with the bucket's prepare generation
    /// it was made at (C++ makes one per drawable and frame from the
    /// current, possibly backfilled, DEM image).
    dem_textures: HashMap<SimpleIdentity, (Texture2DPtr, u64)>,
}

impl RenderColorReliefLayer {
    pub(crate) fn new(impl_: Arc<ColorReliefLayerImpl>) -> Self {
        let unevaluated = impl_.paint.untransitioned();
        let properties: Arc<dyn LayerProperties> = Arc::new(ColorReliefLayerProperties::new(impl_));
        let mut base = RenderLayerBase::new(properties);
        base.style_dependencies = unevaluated.get_dependencies();

        // Initialize color ramp data with RGBA format (4 floats per stop)
        let color_ramp_size = 256;
        let mut layer = RenderColorReliefLayer {
            base,
            unevaluated,
            color_ramp_size,
            color_ramp_changed: true,
            color_ramp_global_state: None,
            elevation_stops_data: vec![0.0; color_ramp_size as usize * 4],
            color_stops: PremultipliedImage::new(Size::new(color_ramp_size, 1)),
            elevation_stops_texture: None,
            color_stops_texture: None,
            color_relief_shader: None,
            static_data_shared_vertices: None,
            globe_meshes: GlobeTileMeshCache::new(),
            dem_textures: HashMap::new(),
        };

        // Initialize with default color ramp immediately to avoid
        // uninitialized state
        layer.update_color_ramp();
        layer
    }

    fn update_color_ramp(&mut self) {
        // Get the color property value, using default if undefined
        let mut color_value = self.unevaluated.get::<ColorReliefColor>().get_value().clone();
        if color_value.is_undefined() {
            color_value = ColorReliefLayer::get_default_color_relief_color();
            if color_value.is_undefined() {
                // If even the default is undefined, we can't proceed
                return;
            }
        }

        let mut elevation_stops_vector: Vec<f32> = Vec::new();
        let mut color_stops_vector: Vec<Color> = Vec::new();

        let expr = color_value.get_expression();

        if let Some(interpolate) = downcast::<Interpolate>(expr) {
            let stop_count = interpolate.get_stop_count();

            elevation_stops_vector.reserve(stop_count);
            color_stops_vector.reserve(stop_count);

            // Extract elevation values from stops
            interpolate.each_stop(|elevation, _output_expr| {
                elevation_stops_vector.push(elevation as f32);
            });

            // Evaluate expression at each elevation to get colors
            for &elevation in &elevation_stops_vector {
                // Create evaluation context with elevation as color ramp
                // parameter
                let context = EvaluationContext::with_color_ramp(None, None, Some(f64::from(elevation)))
                    .with_global_state(self.color_ramp_global_state.as_deref());

                // Default to transparent black
                let color = match expr.evaluate(&context) {
                    Ok(result) => from_expression_value::<Color>(&result).unwrap_or(Color::new(0.0, 0.0, 0.0, 0.0)),
                    Err(_) => Color::new(0.0, 0.0, 0.0, 0.0),
                };

                color_stops_vector.push(color);
            }
        } else {
            // Fallback: Sample the color ramp uniformly
            const NUM_SAMPLES: u32 = 256;
            const MIN_ELEVATION: f32 = -500.0;
            const MAX_ELEVATION: f32 = 9000.0;

            elevation_stops_vector.reserve(NUM_SAMPLES as usize);
            color_stops_vector.reserve(NUM_SAMPLES as usize);

            for i in 0..NUM_SAMPLES {
                let t = i as f32 / (NUM_SAMPLES - 1) as f32;
                let elevation = MIN_ELEVATION + t * (MAX_ELEVATION - MIN_ELEVATION);
                elevation_stops_vector.push(elevation);

                let color = color_value.evaluate_ramp(f64::from(elevation), self.color_ramp_global_state.as_deref());
                color_stops_vector.push(color);
            }
        }

        let ramp_size = elevation_stops_vector.len() as u32;
        if ramp_size == 0 {
            return;
        }

        // Resize and prepare structures
        self.elevation_stops_data.resize(ramp_size as usize * 4, 0.0); // RGBA float for compatibility
        // `colorStops->resize({rampSize, 1})`
        let mut color_stops = PremultipliedImage::new(Size::new(ramp_size, 1));
        let keep = color_stops.data.len().min(self.color_stops.data.len());
        color_stops.data[..keep].copy_from_slice(&self.color_stops.data[..keep]);
        self.color_ramp_size = ramp_size;

        for i in 0..ramp_size as usize {
            // Store elevation in the R channel of an RGBA float vector
            self.elevation_stops_data[i * 4] = elevation_stops_vector[i]; // R = elevation
            self.elevation_stops_data[i * 4 + 1] = 0.0; // G = unused
            self.elevation_stops_data[i * 4 + 2] = 0.0; // B = unused
            self.elevation_stops_data[i * 4 + 3] = 1.0; // A = unused

            // Store colors without premultiplication for proper
            // interpolation
            let color = color_stops_vector[i];
            color_stops.data[i * 4] = (color.r * 255.0) as u8;
            color_stops.data[i * 4 + 1] = (color.g * 255.0) as u8;
            color_stops.data[i * 4 + 2] = (color.b * 255.0) as u8;
            color_stops.data[i * 4 + 3] = (color.a * 255.0) as u8;
        }
        self.color_stops = color_stops;

        self.color_ramp_changed = true;
    }
}

/// The vertex attributes of a color-relief drawable over `vertices`.
fn color_relief_vertex_attributes(context: &dyn Context, vertices: &VertexVectorBasePtr) -> VertexAttributeArrayPtr {
    let vertex_attrs = context.create_vertex_attribute_array();
    {
        let mut attrs = vertex_attrs.borrow_mut();
        let stride = size_of::<HillshadeLayoutVertex>() as u32;
        if let Some(attr) = attrs.set(ID_COLOR_RELIEF_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(
                Some(vertices.clone()),
                0, // offsetof(HillshadeLayoutVertex, a1)
                0,
                stride,
                AttributeDataType::Short2,
            );
        }
        if let Some(attr) = attrs.set(ID_COLOR_RELIEF_TEXTURE_POS_VERTEX_ATTRIBUTE, -1, AttributeDataType::Invalid, 0) {
            attr.set_shared_raw_data(
                Some(vertices.clone()),
                4, // offsetof(HillshadeLayoutVertex, a2)
                0,
                stride,
                AttributeDataType::Short2,
            );
        }
    }
    vertex_attrs
}

impl RenderLayer for RenderColorReliefLayer {
    fn base(&self) -> &RenderLayerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        &mut self.base
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.unevaluated);
        self.unevaluated = impl_cast(&self.base.base_impl).paint.transitioned(parameters, prior);
        self.base.style_dependencies = self.unevaluated.get_dependencies();
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        // Build the ramp here rather than in transition(), which has no global
        // state. A new or changed layer always needs one; otherwise only a
        // change to a state key that the ramp reads does.
        let global_state_differs = match (&self.color_ramp_global_state, &parameters.global_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        };
        let ramp_value = self.unevaluated.get::<ColorReliefColor>().get_value();
        let ramp_affected = parameters.layer_changed
            || (global_state_differs
                && ramp_value.get_dependencies().intersects(Dependency::GLOBAL_STATE)
                && global_state_refs_intersect(
                    ramp_value.get_global_state_refs(),
                    parameters.changed_global_state_keys.as_deref(),
                ));
        self.color_ramp_global_state = parameters.global_state.clone();
        if ramp_affected {
            self.update_color_ramp();
        }

        let previous_evaluated = get_evaluated::<ColorReliefLayerProperties>(&self.base.evaluated_properties).clone();
        let mut properties = ColorReliefLayerProperties::with_evaluated(
            downcast_arc::<ColorReliefLayerImpl>(&self.base.base_impl),
            self.unevaluated.evaluate_with_previous(parameters, &previous_evaluated),
        );

        self.base.passes = if *properties.evaluated.get::<ColorReliefOpacity>() > 0.0 {
            RenderPass::Translucent
        } else {
            RenderPass::None
        };
        properties.base.render_passes = self.base.passes.underlying_type();
        self.base.evaluated_properties = Arc::new(properties);

        if let Some(layer_tweaker) = &self.base.layer_tweaker {
            layer_tweaker.borrow_mut().update_properties(self.base.evaluated_properties.clone());
        }
    }

    fn has_transition(&self) -> bool {
        self.unevaluated.has_transition()
    }

    fn has_crossfade(&self) -> bool {
        false
    }

    fn release_render_data(&mut self) {
        crate::render_layer::release_render_data_base(self);
        self.dem_textures.clear();
    }

    fn prepare(&mut self, params: &mut LayerPrepareParameters<'_>) {
        match params.source {
            Some(source) => {
                self.base.render_tiles = source.get_render_tiles();
                self.base.render_tiles_owner = source.get_raw_render_tiles();
            }
            None => {
                self.base.render_tiles = None;
            }
        }
        self.update_render_tile_ids();
    }

    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &dyn Context,
        state: &TransformState,
        _update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        _render_tree: &RenderTree,
        changes: &mut UniqueChangeRequestVec,
    ) {
        if self.update_projection_variant(state) {
            self.color_relief_shader = None;
            if self.base.projection_variant == ProjectionVariant::Mercator {
                self.globe_meshes.clear();
            }
        }
        let Some(render_tiles) = self.base.render_tiles.clone().filter(|t| !t.is_empty()) else {
            self.remove_all_drawables();
            return;
        };

        // Set up layer group
        if self.base.layer_group.is_none() {
            match context.create_tile_layer_group(
                self.base.layer_index,
                /*initial_capacity=*/ 64,
                self.get_id().to_owned(),
            ) {
                Some(layer_group) => self.set_layer_group(Some(layer_group), changes),
                None => return,
            }
        }
        let layer_group = self.base.layer_group.clone().expect("layer group");

        if self.base.layer_tweaker.is_none() {
            let tweaker: LayerTweakerPtr = Rc::new(RefCell::new(ColorReliefLayerTweaker::new(
                self.get_id().to_owned(),
                self.base.evaluated_properties.clone(),
            )));
            self.base.layer_tweaker = Some(tweaker.clone());
            // layerGroup->addLayerTweaker(layerTweaker)
            changes.push(ChangeRequest::AddLayerTweaker(layer_group.clone(), Rc::downgrade(&tweaker)));
        }
        let layer_tweaker = self.base.layer_tweaker.clone();

        if self.color_relief_shader.is_none() {
            self.color_relief_shader =
                context.get_generic_shader(shaders, COLOR_RELIEF_SHADER_GROUP_NAME, self.base.projection_variant);
        }
        let Some(color_relief_shader) = self.color_relief_shader.clone() else {
            Log::error(Event::Render, "ColorRelief shader failed to load");
            self.remove_all_drawables();
            return;
        };

        let render_pass = RenderPass::Translucent;
        if (render_pass.underlying_type() & self.base.evaluated_properties.render_passes) == 0 {
            return;
        }

        {
            let mut group = layer_group.borrow_mut();
            let tile_layer_group =
                group.as_tile_layer_group_mut().expect("color-relief layer group is a TileLayerGroup");
            let base = &self.base;
            tile_layer_group.remove_drawables_if(|drawable| {
                drawable.get_tile_id().is_some_and(|id| !base.render_tile_ids.contains_key(&id))
            });
        }

        if self.static_data_shared_vertices.is_none() {
            self.static_data_shared_vertices = Some(shared_vertices(&RenderStaticData::raster_vertices()));
        }
        let static_data_shared_vertices = self.static_data_shared_vertices.clone().expect("static vertices");
        let static_data_indices: IndexVectorBasePtr =
            Rc::new(RefCell::new(IndexVectorBase::new(RenderStaticData::quad_triangle_indices())));
        let static_data_segments = RenderStaticData::raster_segments();

        // Update color ramp textures if changed
        if self.color_ramp_changed {
            let ramp_changed_texture = context.create_texture_2d();
            {
                let mut t = ramp_changed_texture.borrow_mut();
                // Use RGBA32F instead of R32F for llvmpipe compatibility
                t.set_format(TexturePixelType::RGBA, TextureChannelDataType::Float);
                t.upload_data(
                    Some(bytemuck::cast_slice(&self.elevation_stops_data)),
                    Size::new(self.color_ramp_size, 1),
                );
                t.set_sampler_configuration(SamplerState {
                    filter: TextureFilterType::Nearest,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            }
            self.elevation_stops_texture = Some(ramp_changed_texture);

            let color_stops_texture =
                self.color_stops_texture.get_or_insert_with(|| context.create_texture_2d()).clone();
            {
                let mut t = color_stops_texture.borrow_mut();
                t.set_image(Some(Rc::new(self.color_stops.clone())));
                t.set_sampler_configuration(SamplerState {
                    filter: TextureFilterType::Linear,
                    wrap_u: TextureWrapType::Clamp,
                    wrap_v: TextureWrapType::Clamp,
                    ..Default::default()
                });
            }

            self.color_ramp_changed = false;
        }

        // Skip rendering if color ramp textures aren't ready
        let (Some(elevation_stops_texture), Some(color_stops_texture)) =
            (self.elevation_stops_texture.clone(), self.color_stops_texture.clone())
        else {
            return;
        };

        // Drop the DEM textures of buckets that are no longer rendered.
        let live_buckets: HashSet<SimpleIdentity> =
            render_tiles.iter().filter_map(|tile| tile.get_bucket(&*self.base.base_impl).map(|b| b.get_id())).collect();
        self.dem_textures.retain(|id, _| live_buckets.contains(id));

        let evaluated = get_evaluated::<ColorReliefLayerProperties>(&self.base.evaluated_properties);
        let dem_sampler = SamplerState {
            filter: match *evaluated.get::<Resampling>() {
                ResamplingType::Linear => TextureFilterType::Linear,
                ResamplingType::Nearest => TextureFilterType::Nearest,
            },
            wrap_u: TextureWrapType::Clamp,
            wrap_v: TextureWrapType::Clamp,
            ..Default::default()
        };

        for tile in render_tiles.iter() {
            let tile_id = *tile.get_overscaled_tile_id();

            let base_impl = self.base.base_impl.clone();
            let bucket_ptr = tile.get_bucket(&*base_impl).filter(|b| b.has_data()).cloned();
            let Some(bucket_ptr) = bucket_ptr else {
                self.remove_tile(render_pass, &tile_id);
                continue;
            };
            let Some(bucket) = (*bucket_ptr).downcast_ref::<HillshadeBucket>() else {
                self.remove_tile(render_pass, &tile_id);
                continue;
            };
            let bucket_id = bucket.base.bucket_id;

            let prev_bucket_id = self.get_render_tile_bucket_id(&tile_id);
            if prev_bucket_id != SimpleIdentity::EMPTY && prev_bucket_id != bucket_id {
                self.remove_tile(render_pass, &tile_id);
            }
            self.set_render_tile_bucket_id(&tile_id, bucket_id);

            // Set up tile drawable
            let (vertices, vertex_count, indices, segments, geometry_time): (
                VertexVectorBasePtr,
                usize,
                IndexVectorBasePtr,
                Vec<SegmentBase>,
                Instant,
            ) = {
                let geometry = bucket.geometry();
                if self.base.projection_variant == ProjectionVariant::Globe {
                    let globe_mesh =
                        self.globe_meshes.get(&tile_id.canonical, &geometry.mask, HillshadeBucket::layout_vertex);
                    let vertices: VertexVectorBasePtr = globe_mesh.vertices.clone();
                    (
                        vertices,
                        globe_mesh.vertices.elements(),
                        globe_mesh.indices.clone(),
                        globe_mesh.segments.iter().map(SegmentBase::copy).collect(),
                        geometry.last_modified,
                    )
                } else if !geometry.vertices.is_empty() && !geometry.indices.is_empty() && !geometry.segments.is_empty()
                {
                    (
                        shared_vertices(&geometry.vertices),
                        geometry.vertices.len(),
                        Rc::new(RefCell::new(IndexVectorBase::new(geometry.indices.clone()))),
                        geometry.segments.iter().map(SegmentBase::copy).collect(),
                        geometry.last_modified,
                    )
                } else {
                    (
                        static_data_shared_vertices.clone(),
                        4,
                        static_data_indices.clone(),
                        static_data_segments.iter().map(SegmentBase::copy).collect(),
                        geometry.last_modified,
                    )
                }
            };

            // The DEM texture and tile properties
            let (dem_image, tile_props_ubo) = {
                let dem_data = bucket.get_dem_data();
                if !dem_data.get_image().valid() {
                    Log::warning(Event::Render, "ColorRelief: DEM image not valid for tile");
                    continue; // Skip this tile if DEM data is not ready
                }
                let unpack_vector = dem_data.get_unpack_vector();
                (
                    dem_data.get_image().clone(),
                    ColorReliefTilePropsUBO {
                        unpack: unpack_vector,
                        // Use stride (dim + 2) for dimension, as the
                        // texture includes a 1-pixel border
                        dimension: [dem_data.stride as f32, dem_data.stride as f32],
                        color_ramp_size: self.color_ramp_size as i32,
                        pad_tile0: 0.0,
                    },
                )
            };
            let generation = bucket.render_target_generation();
            let dem_texture = match self.dem_textures.get(&bucket_id) {
                Some((texture, made_at)) if *made_at == generation => texture.clone(),
                _ => {
                    let texture = context.create_texture_2d();
                    texture.borrow_mut().set_image(Some(Rc::new(dem_image)));
                    self.dem_textures.insert(bucket_id, (texture.clone(), generation));
                    texture
                }
            };
            dem_texture.borrow_mut().set_sampler_configuration(dem_sampler);

            let update_existing = |drawable: &mut dyn mln_gfx::Drawable| {
                if !drawable_has_layer_tweaker(drawable, layer_tweaker.as_ref()) {
                    return false;
                }
                // The masked geometry changed: rebuild (see the file comment).
                if drawable.base().create_time < geometry_time {
                    return false;
                }

                // Update textures
                drawable.set_texture(Some(dem_texture.clone()), ID_COLOR_RELIEF_IMAGE_TEXTURE);
                drawable.set_texture(Some(elevation_stops_texture.clone()), ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE);
                drawable.set_texture(Some(color_stops_texture.clone()), ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE);

                true
            };

            if self.update_tile(render_pass, &tile_id, update_existing) {
                continue;
            }

            let mut builder = context.create_drawable_builder();
            builder.set_shader(Some(color_relief_shader.clone()));
            builder.set_enable_depth(false);
            builder.set_color_mode(&ColorMode::alpha_blended());
            builder.set_cull_face_mode(&CullFaceMode::disabled());
            builder.set_render_pass(render_pass);
            builder.set_vertex_attributes(Some(color_relief_vertex_attributes(context, &vertices)));
            builder.set_raw_vertices(Vec::new(), vertex_count, AttributeDataType::Short2);
            builder.set_segments_shared(DrawMode::Triangles(), Some(indices.clone()), &segments);

            builder.set_texture(Some(dem_texture), ID_COLOR_RELIEF_IMAGE_TEXTURE);

            // Bind color ramp textures
            builder.set_texture(Some(elevation_stops_texture.clone()), ID_COLOR_RELIEF_ELEVATION_STOPS_TEXTURE);
            builder.set_texture(Some(color_stops_texture.clone()), ID_COLOR_RELIEF_COLOR_STOPS_TEXTURE);

            builder.flush(context);

            let mut group = layer_group.borrow_mut();
            let tile_layer_group = group.as_tile_layer_group_mut().expect("TileLayerGroup");
            for mut drawable in builder.clear_drawables() {
                drawable.set_tile_id(tile_id);
                drawable.set_layer_tweaker(layer_tweaker.as_ref().and_then(layer_tweaker_slot));

                // Store tile props in drawable data for use by the tweaker
                // (for consolidated backends)
                drawable.set_data(Some(Box::new(ColorReliefDrawableData { tile_props: tile_props_ubo })));

                tile_layer_group.add_drawable(render_pass, &tile_id, drawable);
            }
        }
    }
}
