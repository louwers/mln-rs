// Ports src/mln/renderer/sources/render_tile_source.hpp and
// src/mln/renderer/sources/render_tile_source.cpp (RenderTileSource and
// TileSourceRenderItem).
//
// TODO(port): RenderTileSetSource (vector/raster sources, M4), queries and
// feature state.
//
// layers: debug: TileSourceRenderItem::updateDebugDrawables with the
// polyline tile borders (MLN_ENABLE_POLYLINE_DRAWABLES, set for WebGPU),
// PolylineLayerImpl/Properties/Tweaker. TODO(port): the DebugShader text
// drawables for MapDebugOptions::Timestamps / ParseStatus (the DebugBucket
// has no text yet, see buckets/debug_bucket.rs).

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use geometry::Point;
use mln_actor::TaggedScheduler;
use mln_gfx::polyline_generator::PolylineGeneratorOptions;
use mln_gfx::renderer::layer_group::visit_layer_group_drawables;
use mln_gfx::shader_group::StringIDSetsPair;
use mln_gfx::shaders::line_layer_ubo::{
    LineDrawableUBO, LineDrawableUnionUBO, LineEvaluatedPropsUBO, line_expression_mask,
};
use mln_gfx::shaders::shader_defines::{
    ID_LINE_BLUR_VERTEX_ATTRIBUTE, ID_LINE_COLOR_VERTEX_ATTRIBUTE, ID_LINE_DRAWABLE_UBO, ID_LINE_EVALUATED_PROPS_UBO,
    ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, ID_LINE_OFFSET_VERTEX_ATTRIBUTE, ID_LINE_OPACITY_VERTEX_ATTRIBUTE,
    ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_LINE_WIDTH_VERTEX_ATTRIBUTE,
};
use mln_gfx::shaders::shader_program_base::ShaderProgramBasePtr;
use mln_gfx::{
    ColorMode, CullFaceMode, LayerGroupBase, LayerGroupBasePtr, UniformBufferPtr, UniqueDrawableBuilder, UploadPass,
};
use mln_map_types::MapDebugOptions;
use mln_style::expression::Dependency;
use mln_style::layer::{LayerTypeInfo, layer_type_info};
use mln_style::layer_properties::{LayerProperties, LayerPropertiesBase};
use mln_style::{layer, source};
use mln_style_types::TranslateAnchorType;
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::Color;
use mln_util::constants::EXTENT;
use mln_util::geometry::FeatureType;
use mln_util::immutable::Immutable;

// layers: debug
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, get_tile_matrix};
use crate::layers::cast_matrix;
use crate::paint_parameters::PaintParameters;
use crate::render_pass::RenderPass;
use crate::render_source::{RenderSourceBase, RenderTiles, SourcePrepareParameters};
use crate::render_tile::RenderTile;
use crate::render_tree::{DebugLayerGroupMap, DebugLayerTweakers, DebugType, RenderItem};
use crate::tile::tile::Tile;
use crate::tile_pyramid::TilePyramid;

/// `TileSourceRenderItem`.
pub struct TileSourceRenderItem {
    render_tiles: Rc<Vec<RenderTile>>,
    name: String,
    /// `mutable std::shared_ptr<PolylineLayerTweaker> layerTweaker` (layers:
    /// debug).
    layer_tweaker: RefCell<Option<Rc<RefCell<PolylineLayerTweaker>>>>,
}

impl TileSourceRenderItem {
    pub fn new(render_tiles: Rc<Vec<RenderTile>>, name: String) -> Self {
        TileSourceRenderItem { render_tiles, name, layer_tweaker: RefCell::new(None) }
    }
}

impl RenderItem for TileSourceRenderItem {
    fn upload(&self, upload_pass: &dyn UploadPass) {
        for tile in self.render_tiles.iter() {
            tile.upload(upload_pass);
        }
    }

    fn has_render_pass(&self, _pass: RenderPass) -> bool {
        false
    }

    fn get_name(&self) -> &str {
        &self.name
    }

    // layers: debug
    fn update_debug_drawables(
        &self,
        debug_layer_groups: &mut DebugLayerGroupMap,
        parameters: &PaintParameters<'_>,
        tweakers: &mut DebugLayerTweakers,
    ) {
        if !parameters
            .debug_options
            .contains(MapDebugOptions::TIMESTAMPS | MapDebugOptions::PARSE_STATUS | MapDebugOptions::TILE_BORDERS)
        {
            debug_layer_groups.clear();
            return;
        }

        let context = parameters.context;
        let render_pass = RenderPass::Translucent;
        let shaders = &*parameters.static_data.shaders;

        // initialize debug builder
        const DEBUG_SHADER_NAME: &str = "DebugShader";
        if context.get_generic_shader(shaders, DEBUG_SHADER_NAME).is_none() {
            return;
        }

        let drawable_name = format!("debug-{}", self.name);
        // TODO(port): the DebugShader builder ("debug-builder") of the
        // Timestamps / ParseStatus drawables.

        // initialize polyline builder
        let mut polyline_shader: Option<ShaderProgramBasePtr> = None;
        let create_polyline_shader = || -> Option<ShaderProgramBasePtr> {
            let shader_group = shaders.get_shader_group("LineShader")?;
            let properties_as_uniforms: StringIDSetsPair = (
                ["a_color", "a_blur", "a_opacity", "a_gapwidth", "a_offset", "a_width"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                [
                    ID_LINE_COLOR_VERTEX_ATTRIBUTE,
                    ID_LINE_BLUR_VERTEX_ATTRIBUTE,
                    ID_LINE_OPACITY_VERTEX_ATTRIBUTE,
                    ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE,
                    ID_LINE_OFFSET_VERTEX_ATTRIBUTE,
                    ID_LINE_WIDTH_VERTEX_ATTRIBUTE,
                ]
                .into_iter()
                .collect(),
            );
            shader_group.get_or_create_shader(context, &properties_as_uniforms, "a_pos")
        };

        let mut polyline_builder: Option<UniqueDrawableBuilder> = None;
        let create_polyline_builder = |shader: Option<ShaderProgramBasePtr>| -> UniqueDrawableBuilder {
            let mut builder = context.create_drawable_builder("debug-polyline-builder".to_owned());
            builder.set_shader(shader);
            builder.set_render_pass(render_pass);
            builder.set_enable_depth(false);
            builder.set_color_mode(&ColorMode::alpha_blended());
            builder.set_cull_face_mode(&CullFaceMode::disabled());
            builder.set_vertex_attr_id(ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE);
            builder.set_drawable_name(drawable_name.clone());
            builder
        };

        // add or get the layer group for a debug type
        let add_or_get_layer_group_for_type = |debug_layer_groups: &mut DebugLayerGroupMap,
                                               type_: DebugType,
                                               layer_name: &str|
         -> Option<LayerGroupBasePtr> {
            if let Some(layer_group) = debug_layer_groups.get(&type_) {
                return Some(layer_group.clone());
            }
            let layer_group =
                context.create_tile_layer_group(type_ as i32, /*initial_capacity=*/ 64, layer_name.to_owned())?;
            debug_layer_groups.insert(type_, layer_group.clone());
            Some(layer_group)
        };

        // build a set of tiles to cover
        let new_tiles: HashSet<OverscaledTileID> =
            self.render_tiles.iter().map(|tile| *tile.get_overscaled_tile_id()).collect();

        // TODO(port): the overlay texture and updateDrawables/addDrawable of
        // the DebugShader drawables.

        let line_properties_ubo = LineEvaluatedPropsUBO {
            color: Color::red().to_float_array(),
            blur: 0.0,
            opacity: 1.0,
            gapwidth: 0.0,
            offset: 0.0,
            width: 4.0,
            floorwidth: 0.0,
            expression_mask: line_expression_mask::NONE,
            pad1: 0.0,
        };

        // Timestamps or Parse Status
        if parameters.debug_options.contains(MapDebugOptions::TIMESTAMPS | MapDebugOptions::PARSE_STATUS) {
            for (type_, layer_name) in [(DebugType::TextOutline, "debug-text-outline"), (DebugType::Text, "debug-text")]
            {
                let Some(layer_group) = add_or_get_layer_group_for_type(debug_layer_groups, type_, layer_name) else {
                    continue;
                };
                // erase drawables that are not in the current tile set
                let mut group = layer_group.borrow_mut();
                if let Some(tile_layer_group) = group.as_tile_layer_group_mut() {
                    tile_layer_group.remove_drawables_if(|drawable| {
                        drawable.get_name() == drawable_name
                            && !drawable.get_tile_id().is_some_and(|id| new_tiles.contains(&id))
                    });
                }
            }
            // TODO(port): add new text drawables and update existing ones
            // (DebugShader, see the file comment).
        } else {
            // tile texts are not required, erase layer groups
            debug_layer_groups.remove(&DebugType::TextOutline);
            debug_layer_groups.remove(&DebugType::Text);
        }

        // Tile Borders
        if parameters.debug_options.contains(MapDebugOptions::TILE_BORDERS) {
            let Some(layer_group) =
                add_or_get_layer_group_for_type(debug_layer_groups, DebugType::Border, "debug-border")
            else {
                return;
            };

            {
                let mut group = layer_group.borrow_mut();
                let tile_layer_group = group.as_tile_layer_group_mut().expect("debug layer group is a TileLayerGroup");

                // erase drawables that are not in the current tile set
                tile_layer_group.remove_drawables_if(|drawable| {
                    drawable.get_name() == drawable_name
                        && !drawable.get_tile_id().is_some_and(|id| new_tiles.contains(&id))
                });

                // add new drawables and update existing ones
                for tile in self.render_tiles.iter() {
                    let tile_id = *tile.get_overscaled_tile_id();
                    if tile.debug_bucket.is_none() {
                        continue;
                    }

                    // MLN_ENABLE_POLYLINE_DRAWABLES
                    if tile_layer_group.get_drawable_count_for(render_pass, &tile_id) == 0 && tile.get_needs_rendering()
                    {
                        // addPolylineDrawable(tileLayerGroup, tile)
                        let extent = EXTENT as i16;
                        let coords: GeometryCoordinates = vec![
                            Point::new(0, 0),
                            Point::new(extent, 0),
                            Point::new(extent, extent),
                            Point::new(0, extent),
                            Point::new(0, 0),
                        ];
                        let options = PolylineGeneratorOptions { type_: FeatureType::Polygon, ..Default::default() };

                        if polyline_shader.is_none() {
                            polyline_shader = create_polyline_shader();
                        }
                        let builder =
                            polyline_builder.get_or_insert_with(|| create_polyline_builder(polyline_shader.clone()));
                        builder.add_polyline(&coords, &options);

                        // finish
                        builder.flush(context);
                        for mut drawable in builder.clear_drawables() {
                            drawable.set_tile_id(tile_id);
                            tile_layer_group.add_drawable(render_pass, &tile_id, drawable);
                        }

                        let mut layer_tweaker = self.layer_tweaker.borrow_mut();
                        if layer_tweaker.is_none() {
                            let tweaker = Rc::new(RefCell::new(PolylineLayerTweaker::new(line_properties_ubo)));
                            let weak: Weak<RefCell<dyn LayerTweaker>> = Rc::downgrade(&tweaker) as _;
                            tweakers.push((layer_group.clone(), weak));
                            *layer_tweaker = Some(tweaker);
                        }
                    }
                }
            }
        } else {
            // if tile borders are not required, erase layer group
            debug_layer_groups.remove(&DebugType::Border);
        }
    }
}

// layers: debug --------------------------------------------------------------

/// `PolylineLayerImpl`.
#[derive(Debug)]
struct PolylineLayerImpl {
    base: layer::ImplBase,
}

static POLYLINE_LAYER_TYPE_INFO: LayerTypeInfo = LayerTypeInfo {
    type_: "debugPolyline",
    source: layer_type_info::Source::NotRequired,
    pass3d: layer_type_info::Pass3D::NotRequired,
    layout: layer_type_info::Layout::NotRequired,
    fading_tiles: layer_type_info::FadingTiles::NotRequired,
    cross_tile_index: layer_type_info::CrossTileIndex::NotRequired,
    tile_kind: layer_type_info::TileKind::NotRequired,
};

impl layer::Impl for PolylineLayerImpl {
    fn base(&self) -> &layer::ImplBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut layer::ImplBase {
        &mut self.base
    }
    fn has_layout_difference(&self, _other: &dyn layer::Impl) -> bool {
        false
    }
    fn stringify_layout(&self, _writer: &mut String) {}
    fn get_type_info(&self) -> &'static LayerTypeInfo {
        &POLYLINE_LAYER_TYPE_INFO
    }
}

/// `PolylineLayerProperties`.
#[derive(Debug)]
struct PolylineLayerProperties {
    base: LayerPropertiesBase,
}

impl LayerProperties for PolylineLayerProperties {
    fn base(&self) -> &LayerPropertiesBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut LayerPropertiesBase {
        &mut self.base
    }
    fn get_dependencies(&self) -> Dependency {
        Dependency::NONE
    }
}

/// `PolylineLayerTweaker`: the uniforms of the tile-border polylines.
pub struct PolylineLayerTweaker {
    base: LayerTweakerBase,
    props_ubo: LineEvaluatedPropsUBO,
    /// MLN_UBO_CONSOLIDATION
    drawable_uniform_buffer: Option<UniformBufferPtr>,
}

impl PolylineLayerTweaker {
    pub fn new(properties: LineEvaluatedPropsUBO) -> Self {
        let layer_impl: Immutable<dyn layer::Impl> =
            Arc::new(PolylineLayerImpl { base: layer::ImplBase::new(String::new(), String::new()) });
        let layer_properties: Immutable<dyn LayerProperties> =
            Arc::new(PolylineLayerProperties { base: LayerPropertiesBase::new(layer_impl) });
        PolylineLayerTweaker {
            base: LayerTweakerBase::new("debug-polyline".to_owned(), layer_properties),
            props_ubo: properties,
            drawable_uniform_buffer: None,
        }
    }
}

impl LayerTweaker for PolylineLayerTweaker {
    fn base(&self) -> &LayerTweakerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut LayerTweakerBase {
        &mut self.base
    }

    fn execute(&mut self, layer_group: &mut dyn LayerGroupBase, parameters: &PaintParameters<'_>) {
        if layer_group.empty() {
            return;
        }

        let context = parameters.context;
        layer_group.mutable_uniform_buffers().create_or_update_pod(
            ID_LINE_EVALUATED_PROPS_UBO,
            &self.props_ubo,
            context,
        );

        // We would need to set up `idLineExpressionUBO` if the expression
        // mask isn't empty
        debug_assert!(self.props_ubo.expression_mask == line_expression_mask::NONE);
        // C++ also stores a LineExpressionUBO of null GPU expressions; those
        // are Metal-only and the WebGPU line shaders do not bind it (see
        // mln_gfx::shaders::line_layer_ubo).

        let mut i: u32 = 0;
        let mut drawable_ubo_vector = vec![LineDrawableUnionUBO::default(); layer_group.get_drawable_count()];
        visit_layer_group_drawables(layer_group, |drawable| {
            let Some(tile_id) = drawable.get_tile_id() else {
                return;
            };

            let tile_id = tile_id.to_unwrapped();
            let zoom = parameters.state.get_zoom();
            // C++ also computes an unused `tileMatrix` (state.matrixFor).

            let matrix = get_tile_matrix(
                &tile_id,
                parameters,
                &[0.0, 0.0],
                TranslateAnchorType::Viewport,
                false,
                false,
                drawable,
                false,
            );

            drawable_ubo_vector[i as usize].set(&LineDrawableUBO {
                matrix: cast_matrix(&matrix),
                ratio: 1.0 / tile_id.pixels_to_tile_units(1.0, zoom as f32),
                color_t: 0.0,
                blur_t: 0.0,
                opacity_t: 0.0,
                gapwidth_t: 0.0,
                offset_t: 0.0,
                width_t: 0.0,
                pad1: 0.0,
            });

            drawable.set_ubo_index(i);
            i += 1;
        });

        let bytes: &[u8] = bytemuck::cast_slice(&drawable_ubo_vector);
        match &self.drawable_uniform_buffer {
            Some(buffer) if buffer.get_size() >= bytes.len() => buffer.update(bytes),
            _ => self.drawable_uniform_buffer = Some(context.create_uniform_buffer(bytes, false, true)),
        }
        layer_group.mutable_uniform_buffers().set(ID_LINE_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
    }
}

/// `util::rotate(Point<float>, float)`.
fn rotate(a: Point<f32>, angle: f32) -> Point<f32> {
    let cos = angle.cos();
    let sin = angle.sin();
    Point::new(cos * a.x - sin * a.y, sin * a.x + cos * a.y)
}

/// `RenderTileSource`: base class for render sources that provide render
/// tiles.
pub struct RenderTileSource {
    pub base: RenderSourceBase,
    pub tile_pyramid: TilePyramid,
    pub render_tiles: Rc<Vec<RenderTile>>,
    filtered_render_tiles: RefCell<Option<RenderTiles>>,
    render_tiles_sorted_by_y: RefCell<Option<RenderTiles>>,

    bearing: f32,
    // layers: feature state
    pub feature_state: crate::source_state::SourceFeatureState,
}

impl RenderTileSource {
    /// `RenderTileSource(Immutable<style::Source::Impl>, const TaggedScheduler&)`.
    pub fn new(impl_: Immutable<dyn source::Impl>, thread_pool: &TaggedScheduler) -> Self {
        let base = RenderSourceBase::new(impl_);
        let mut tile_pyramid = TilePyramid::new(thread_pool.clone());
        tile_pyramid.set_observer(base.tile_observer());
        RenderTileSource {
            base,
            tile_pyramid,
            render_tiles: Rc::new(Vec::new()),
            filtered_render_tiles: RefCell::new(None),
            render_tiles_sorted_by_y: RefCell::new(None),
            bearing: 0.0,
            feature_state: crate::source_state::SourceFeatureState::new(),
        }
    }

    // layers: feature state
    /// `setFeatureState(sourceLayerID, featureID, state)`.
    pub fn set_feature_state(
        &mut self,
        source_layer_id: Option<&str>,
        feature_id: &str,
        state: &mln_style::expression::expression::FeatureState,
    ) {
        self.feature_state.update_state(source_layer_id, feature_id, state);
    }

    /// `getFeatureState(state, sourceLayerID, featureID)`.
    pub fn get_feature_state(
        &self,
        source_layer_id: Option<&str>,
        feature_id: &str,
    ) -> mln_style::expression::expression::FeatureState {
        self.feature_state.get_state(source_layer_id, feature_id)
    }

    /// `removeFeatureState(sourceLayerID, featureID, stateKey)`.
    pub fn remove_feature_state(
        &mut self,
        source_layer_id: Option<&str>,
        feature_id: Option<&str>,
        state_key: Option<&str>,
    ) {
        self.feature_state.remove_state(source_layer_id, feature_id, state_key);
    }

    // query
    /// `queryRenderedFeatures(geometry, transformState, layers, options,
    /// globalState, projMatrix)`.
    pub fn query_rendered_features(
        &self,
        geometry: &[mln_util::geo::ScreenCoordinate],
        transform_state: &mln_map_types::TransformState,
        layers: &crate::geometry::feature_index::QueryLayers<'_>,
        options: &crate::query::RenderedQueryOptions,
        global_state: Option<&mln_style::expression::GlobalStateMap>,
        proj_matrix: &mln_util::mat4::Mat4,
    ) -> Result<crate::geometry::feature_index::QueryResults, mln_util::exception::ExceptionPtr> {
        self.tile_pyramid.query_rendered_features(
            geometry,
            transform_state,
            layers,
            options,
            global_state,
            proj_matrix,
            &self.feature_state,
        )
    }

    /// `querySourceFeatures(options, globalState)`.
    pub fn query_source_features(
        &self,
        options: &crate::query::SourceQueryOptions,
        global_state: Option<&mln_style::expression::GlobalStateMap>,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::exception::ExceptionPtr> {
        self.tile_pyramid.query_source_features(options, global_state)
    }

    pub fn is_loaded(&self) -> bool {
        self.tile_pyramid.is_loaded()
    }

    pub fn create_render_item(&mut self) -> Box<dyn RenderItem> {
        Box::new(TileSourceRenderItem::new(self.render_tiles.clone(), self.base.base_impl.id.clone()))
    }

    pub fn prepare(&mut self, parameters: &SourcePrepareParameters<'_>) {
        self.bearing = parameters.transform.state.get_bearing() as f32;
        *self.filtered_render_tiles.borrow_mut() = None;
        *self.render_tiles_sorted_by_y.borrow_mut() = None;
        let mut tiles: Vec<RenderTile> = Vec::with_capacity(self.tile_pyramid.rendered_tile_count());
        for (id, tile) in self.tile_pyramid.get_rendered_tiles() {
            let mut render_tile = RenderTile::new(*id, tile);
            render_tile.prepare(tile, parameters);
            tiles.push(render_tile);
        }
        // layers: feature state
        // featureState.coalesceChanges(*tiles): RenderTile::setFeatureState
        // is tile.setFeatureState, applied to the pyramid's tiles directly.
        self.feature_state.coalesce_changes(self.tile_pyramid.get_rendered_tiles().map(|(_, tile)| tile));
        self.render_tiles = Rc::new(tiles);
    }

    pub fn update_fading_tiles(&mut self) {
        self.tile_pyramid.update_fading_tiles();
    }

    pub fn has_fading_tiles(&self) -> bool {
        self.tile_pyramid.has_fading_tiles()
    }

    pub fn get_render_tiles(&self) -> Option<RenderTiles> {
        let mut filtered = self.filtered_render_tiles.borrow_mut();
        if filtered.is_none() {
            let indices: Vec<usize> = self
                .render_tiles
                .iter()
                .enumerate()
                .filter(|(_, render_tile)| !render_tile.hold_for_fade())
                .map(|(i, _)| i)
                .collect();
            *filtered = Some(RenderTiles::new(self.render_tiles.clone(), indices));
        }
        filtered.clone()
    }

    pub fn get_render_tiles_sorted_by_y_position(&self) -> Option<RenderTiles> {
        let mut sorted = self.render_tiles_sorted_by_y.borrow_mut();
        if sorted.is_none() {
            let source_bearing = self.bearing;
            let tiles = &self.render_tiles;
            let comp = |a: &usize, b: &usize| -> bool {
                let a = &tiles[*a];
                let b = &tiles[*b];
                let pa = Point::new(a.id.canonical.x as f32, a.id.canonical.y as f32);
                let pb = Point::new(b.id.canonical.x as f32, b.id.canonical.y as f32);

                let par = rotate(pa, source_bearing);
                let pbr = rotate(pb, source_bearing);

                // std::tie(b.z, par.y, par.x) < std::tie(a.z, pbr.y, pbr.x)
                let (l0, l1, l2) = (b.id.canonical.z, par.y, par.x);
                let (r0, r1, r2) = (a.id.canonical.z, pbr.y, pbr.x);
                if l0 != r0 {
                    return l0 < r0;
                }
                if l1 < r1 {
                    return true;
                }
                if r1 < l1 {
                    return false;
                }
                l2 < r2
            };
            let mut indices: Vec<usize> = (0..tiles.len()).collect();
            libstdcxx::sort_by(&mut indices, comp);
            *sorted = Some(RenderTiles::new(self.render_tiles.clone(), indices));
        }
        sorted.clone()
    }

    pub fn get_rendered_tile(&self, tile_id: &UnwrappedTileID) -> Option<&dyn Tile> {
        self.tile_pyramid.get_rendered_tile(tile_id)
    }

    pub fn get_raw_render_tiles(&self) -> Rc<Vec<RenderTile>> {
        self.render_tiles.clone()
    }

    pub fn set_cache_enabled(&mut self, enable: bool) {
        self.tile_pyramid.set_cache_enabled(enable);
    }

    pub fn reduce_memory_use(&mut self) {
        self.tile_pyramid.reduce_memory_use();
    }

    pub fn retry_failed_tiles(&self) {
        self.tile_pyramid.retry_failed_tiles();
    }

    pub fn dump_debug_logs(&self) {
        self.tile_pyramid.dump_debug_logs();
    }
}
