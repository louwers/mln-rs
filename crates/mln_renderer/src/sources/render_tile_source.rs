// layers: debug: TileSourceRenderItem::updateDebugDrawables, with the tile
// borders and the parse status and timestamp text as polylines
// (MLN_ENABLE_POLYLINE_DRAWABLES, set for WebGPU), and
// PolylineLayerImpl/Properties/Tweaker.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use geometry::Point;
use glam::Vec2;
use mln_gfx::polyline_generator::PolylineGeneratorOptions;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shader_group::StringIDSetsPair;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::line_layer_ubo::{
    LINE_EXPRESSION_UBO_SIZE, LineDrawableUBO, LineDrawableUnionUBO, LineEvaluatedPropsUBO, line_expression_mask,
};
use mln_gfx::shaders::shader_defines::{
    ID_LINE_BLUR_VERTEX_ATTRIBUTE, ID_LINE_COLOR_VERTEX_ATTRIBUTE, ID_LINE_DRAWABLE_UBO, ID_LINE_EVALUATED_PROPS_UBO,
    ID_LINE_EXPRESSION_UBO, ID_LINE_GAP_WIDTH_VERTEX_ATTRIBUTE, ID_LINE_OFFSET_VERTEX_ATTRIBUTE,
    ID_LINE_OPACITY_VERTEX_ATTRIBUTE, ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE, ID_LINE_WIDTH_VERTEX_ATTRIBUTE,
};
use mln_gfx::{ColorMode, CullFaceMode};
use mln_map_types::{MapDebugOptions, MapMode};
use mln_storage::FileSource;
use mln_style::expression::Dependency;
use mln_style::layer::{LayerTypeInfo, SourceKind};
use mln_style::layer_properties::{LayerProperties, LayerPropertiesBase};
use mln_style::layer_type::{LayerStyle, NoProperties, StyleLayerType};
use mln_style::property_table::{PropertyTable, PropertyVisitor};
use mln_style::source::downcast_arc;
use mln_style::sources::custom_vector_source::{CustomVectorTileData, TileDataFormat};
use mln_style::sources::geojson_source::{GeoJSONData, TileFeatures};
use mln_style::sources::{custom_geometry_source, custom_vector_source, geojson_source_impl, tile_source};
use mln_style::{layer, source};
use mln_style_types::{LineCapType, LineJoinType, SourceType, TranslateAnchorType};
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_util::constants::EXTENT;
use mln_util::error::Error;
use mln_util::geometry::FeatureType;
use mln_util::hash::HashSet;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_util::tileset::{RasterEncoding, Tileset, VectorEncoding};
use mln_util::{Color, constants};

use crate::drawable::Drawable;
use crate::drawable_builder::{DrawableBuilder, UniqueDrawableBuilder};
use crate::gfx::context::Context;
use crate::gfx::uniform_buffer::UniformBufferPtr;
use crate::layer_group::{LayerGroup, LayerGroupPtr};
// layers: debug
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, get_projection_data, to_projection_ubo};
use crate::layers::cast_matrix;
use crate::paint_parameters::PaintParameters;
use crate::query::FeatureExtensionValue;
use crate::render_pass::RenderPass;
use crate::render_source::{RenderSource, RenderSourceBase, RenderTiles, SourcePrepareParameters};
use crate::render_tile::RenderTile;
use crate::render_tree::{DebugLayerGroupMap, DebugType, RenderItem};
use crate::tile::job::SourceKey;
use crate::tile::loading::{CustomLoader, CustomLoaderOf, DataOrigin, VectorTileEncoding};
use crate::tile::source_tiles::{CoverUpdate, SourceTiles};
use crate::tile::store::TileStore;
use crate::tile::tile::TileKind;
use crate::tile_parameters::TileParameters;
use crate::util::subdivision::subdivide_vertex_line;

pub(crate) struct TileSourceRenderItem {
    render_tiles: Rc<Vec<RenderTile>>,
    name: String,
}

impl TileSourceRenderItem {
    #[must_use]
    pub(crate) fn new(render_tiles: Rc<Vec<RenderTile>>, name: String) -> Self {
        TileSourceRenderItem { render_tiles, name }
    }
}

/// The line properties of a debug layer group's polylines: red tile
/// borders, and black text over a white outline (layers: debug).
#[must_use]
pub(crate) fn debug_line_properties(type_: DebugType) -> LineEvaluatedPropsUBO {
    let (color, width) = match type_ {
        DebugType::TextOutline => (Color::white(), 4.0),
        DebugType::Text => (Color::black(), 2.0),
        DebugType::Border => (Color::red(), 4.0),
    };
    LineEvaluatedPropsUBO {
        color: color.to_float_array(),
        blur: 0.0,
        opacity: 1.0,
        gapwidth: 0.0,
        offset: 0.0,
        width,
        floorwidth: 0.0,
        expression_mask: line_expression_mask::NONE,
        pad1: 0.0,
    }
}

impl RenderItem for TileSourceRenderItem {
    fn upload(&self, context: &Context) {
        for tile in self.render_tiles.iter() {
            tile.upload(context);
        }
    }

    // layers: debug
    //
    // C++ draws the text with the DebugShader as line primitives of 2 and 4
    // pixels. WebGPU draws line primitives one pixel wide, so the text is
    // polylines, like the tile borders (MLN_ENABLE_POLYLINE_DRAWABLES).
    fn update_debug_drawables(&self, debug_layer_groups: &mut DebugLayerGroupMap, parameters: &PaintParameters<'_>) {
        if !parameters
            .debug_options
            .contains(MapDebugOptions::TIMESTAMPS | MapDebugOptions::PARSE_STATUS | MapDebugOptions::TILE_BORDERS)
        {
            debug_layer_groups.clear();
            return;
        }

        let render_pass = RenderPass::Translucent;
        let shaders = &*parameters.static_data.shaders;
        let drawable_name = format!("debug-{}", self.name);
        let globe = parameters.state.is_globe_rendering();
        let projection_variant = if globe { ProjectionVariant::Globe } else { ProjectionVariant::Mercator };

        // initialize polyline builder
        let Some(polyline_shader) = shaders.shader_group("LineShader").map(|shader_group| {
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
            shader_group.shader(&properties_as_uniforms, projection_variant)
        }) else {
            return;
        };

        let mut polyline_builder: Option<UniqueDrawableBuilder> = None;
        // Builds one drawable of `lines` for the tile and adds it to the group.
        let mut add_polyline_drawable =
            |tile_layer_group: &mut LayerGroup,
             tile_id: OverscaledTileID,
             lines: &[GeometryCoordinates],
             options: &PolylineGeneratorOptions,
             data: Option<crate::drawable::DrawableData>| {
                let builder = polyline_builder.get_or_insert_with(|| {
                    let mut builder = DrawableBuilder::boxed();
                    builder.set_shader(Some(Rc::clone(&polyline_shader)));
                    builder.set_render_pass(render_pass);
                    builder.set_enable_depth(false);
                    builder.set_color_mode(&ColorMode::alpha_blended());
                    builder.set_cull_face_mode(CullFaceMode::disabled());
                    builder.set_vertex_attr_id(ID_LINE_POS_NORMAL_VERTEX_ATTRIBUTE);
                    builder.set_drawable_name(drawable_name.clone());
                    builder
                });
                for line in lines {
                    builder.add_polyline(line, options);
                }

                // finish
                builder.flush();
                let mut data = data;
                for mut drawable in builder.clear_drawables() {
                    drawable.tile_id = Some(tile_id);
                    drawable.data = data.take();
                    tile_layer_group.add_tile_drawable(render_pass, &tile_id, drawable);
                }
            };

        // add or get the layer group for a debug type
        let add_or_get_layer_group_for_type =
            |debug_layer_groups: &mut DebugLayerGroupMap, type_: DebugType| -> Option<LayerGroupPtr> {
                if let Some(layer_group) = debug_layer_groups.get(&type_) {
                    return Some(Rc::clone(layer_group));
                }
                let layer_group = LayerGroup::tiles(type_ as i32);
                debug_layer_groups.insert(type_, Rc::clone(&layer_group));
                Some(layer_group)
            };

        // build a set of tiles to cover
        let new_tiles: HashSet<OverscaledTileID> =
            self.render_tiles.iter().map(|tile| *tile.get_overscaled_tile_id()).collect();
        // erase drawables that are not in the current tile set, and those
        // built for the other projection, which keep its shader
        let remove_stale = |tile_layer_group: &mut LayerGroup| {
            tile_layer_group.remove_drawables_if(|drawable| {
                let same_shader = drawable
                    .shader()
                    .is_some_and(|shader| std::ptr::addr_eq(Rc::as_ptr(shader), Rc::as_ptr(&polyline_shader)));
                drawable.name == drawable_name
                    && (!drawable.tile_id.is_some_and(|id| new_tiles.contains(&id)) || !same_shader)
            });
        };

        // Timestamps or Parse Status
        if parameters.debug_options.contains(MapDebugOptions::TIMESTAMPS | MapDebugOptions::PARSE_STATUS) {
            let text_options = PolylineGeneratorOptions {
                join_type: LineJoinType::Round,
                begin_cap: LineCapType::Round,
                end_cap: LineCapType::Round,
                ..Default::default()
            };
            for type_ in [DebugType::TextOutline, DebugType::Text] {
                let Some(layer_group) = add_or_get_layer_group_for_type(debug_layer_groups, type_) else {
                    continue;
                };
                let mut tile_layer_group = layer_group.borrow_mut();
                remove_stale(&mut tile_layer_group);

                // add new drawables and replace those whose text changed
                for tile in self.render_tiles.iter() {
                    let tile_id = *tile.get_overscaled_tile_id();
                    let Some(debug_bucket) = &tile.debug_bucket else {
                        continue;
                    };
                    let shows_text = |drawable: &mut Drawable| matches!(&drawable.data, Some(crate::drawable::DrawableData::DebugText(text)) if *text == debug_bucket.text);
                    let mut current = false;
                    tile_layer_group.visit_drawables_for(render_pass, &tile_id, |drawable| {
                        current |= drawable.name == drawable_name && shows_text(drawable);
                    });
                    if current {
                        continue;
                    }
                    tile_layer_group.remove_drawables_if(|drawable| {
                        drawable.name == drawable_name && drawable.tile_id == Some(tile_id)
                    });
                    if tile.get_needs_rendering() && !debug_bucket.strokes.is_empty() {
                        add_polyline_drawable(
                            &mut tile_layer_group,
                            tile_id,
                            &debug_bucket.strokes,
                            &text_options,
                            Some(crate::drawable::DrawableData::DebugText(debug_bucket.text.clone())),
                        );
                    }
                }
            }
        } else {
            // tile texts are not required, erase layer groups
            debug_layer_groups.remove(&DebugType::TextOutline);
            debug_layer_groups.remove(&DebugType::Text);
        }

        // Tile Borders
        if parameters.debug_options.contains(MapDebugOptions::TILE_BORDERS) {
            let Some(layer_group) = add_or_get_layer_group_for_type(debug_layer_groups, DebugType::Border) else {
                return;
            };
            let mut tile_layer_group = layer_group.borrow_mut();
            remove_stale(&mut tile_layer_group);

            // add new drawables
            let extent = EXTENT as i16;
            let tile_border: GeometryCoordinates = vec![
                Point::new(0, 0),
                Point::new(extent, 0),
                Point::new(extent, extent),
                Point::new(0, extent),
                Point::new(0, 0),
            ];
            // On the globe the edges bend with the sphere: the border
            // subdivided at the tile mesh's granularity, one per zoom.
            let mut globe_borders: BTreeMap<u8, GeometryCoordinates> = BTreeMap::new();
            let border_options = PolylineGeneratorOptions { type_: FeatureType::Polygon, ..Default::default() };
            for tile in self.render_tiles.iter() {
                let tile_id = *tile.get_overscaled_tile_id();
                if tile.debug_bucket.is_none() || !tile.get_needs_rendering() {
                    continue;
                }
                let mut drawn = false;
                tile_layer_group.visit_drawables_for(render_pass, &tile_id, |drawable| {
                    drawn |= drawable.name == drawable_name;
                });
                if !drawn {
                    let border = if globe {
                        let z = tile_id.canonical.z;
                        globe_borders.entry(z).or_insert_with(|| {
                            let steps = SubdivisionGranularitySetting::globe().tile.get_granularity_for_zoom_level(z);
                            subdivide_vertex_line(&tile_border, steps, false)
                        })
                    } else {
                        &tile_border
                    };
                    add_polyline_drawable(
                        &mut tile_layer_group,
                        tile_id,
                        std::slice::from_ref(border),
                        &border_options,
                        None,
                    );
                }
            }
        } else {
            // if tile borders are not required, erase layer group
            debug_layer_groups.remove(&DebugType::Border);
        }
    }
}

// layers: debug --------------------------------------------------------------

/// The layer type of the debug polylines, which no registry holds.
struct Polyline;

static POLYLINE_PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

impl StyleLayerType for Polyline {
    const INFO: LayerTypeInfo = LayerTypeInfo {
        name: "debugPolyline",
        source: SourceKind::None,
        pass_3d: false,
        fading_tiles: false,
        cross_tile_index: false,
        from_json: false,
    };
    const PROPERTIES: &'static PropertyTable = &POLYLINE_PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }

    fn layout_difference(_a: &LayerStyle<Self>, _b: &LayerStyle<Self>) -> bool {
        false
    }
}

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

/// Sets the uniforms of a debug layer group's polylines.
pub(crate) struct PolylineLayerTweaker {
    base: LayerTweakerBase,
    props_ubo: LineEvaluatedPropsUBO,
    /// `MLN_UBO_CONSOLIDATION`
    drawable_uniform_buffer: Option<UniformBufferPtr>,
    /// Port-only: the Metal line shaders' `LineExpressionUBO`.
    expression_uniform_buffer: Option<UniformBufferPtr>,
}

impl PolylineLayerTweaker {
    #[must_use]
    pub(crate) fn new(properties: LineEvaluatedPropsUBO) -> Self {
        let layer_impl: Arc<dyn layer::Impl> = Arc::new(LayerStyle::<Polyline>::new(String::new(), String::new()));
        let layer_properties: Arc<dyn LayerProperties> =
            Arc::new(PolylineLayerProperties { base: LayerPropertiesBase::new(layer_impl) });
        PolylineLayerTweaker {
            base: LayerTweakerBase::new("debug-polyline".to_owned(), layer_properties),
            props_ubo: properties,
            drawable_uniform_buffer: None,
            expression_uniform_buffer: None,
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

    fn execute(&mut self, layer_group: &mut LayerGroup, parameters: &PaintParameters<'_>) {
        let context = parameters.context;
        if layer_group.empty() {
            return;
        }

        layer_group.mutable_uniform_buffers().create_or_update_pod(
            ID_LINE_EVALUATED_PROPS_UBO,
            &self.props_ubo,
            context,
        );

        // We would need to set up `idLineExpressionUBO` if the expression
        // mask isn't empty
        debug_assert_eq!(self.props_ubo.expression_mask, line_expression_mask::NONE);
        // No GPU expressions; only line shaders of `Caps::line_expressions`
        // read it.
        if context.caps().line_expressions {
            if self.expression_uniform_buffer.is_none() {
                self.expression_uniform_buffer = context.create_uniform_buffer(&[0; LINE_EXPRESSION_UBO_SIZE]);
            }
            layer_group.mutable_uniform_buffers().set(ID_LINE_EXPRESSION_UBO, self.expression_uniform_buffer.clone());
        }

        let mut i: u32 = 0;
        let mut drawable_ubo_vector = vec![LineDrawableUnionUBO::default(); layer_group.drawable_count()];
        let mut projection_ubo_vector = vec![ProjectionUBO::default(); layer_group.drawable_count()];
        layer_group.visit_drawables(|drawable| {
            let Some(tile_id) = drawable.tile_id else {
                return;
            };

            let tile_id = tile_id.to_unwrapped();
            let zoom = parameters.state.get_zoom();
            // C++ also computes an unused `tileMatrix` (state.matrixFor).

            let projection = get_projection_data(
                &tile_id,
                parameters,
                [0.0, 0.0],
                TranslateAnchorType::Viewport,
                false,
                false,
                drawable,
                false,
            );
            let matrix = projection.fallback_matrix;
            projection_ubo_vector[i as usize] = to_projection_ubo(&projection);

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

            drawable.ubo_index = i;
            i += 1;
        });

        let bytes: &[u8] = bytemuck::cast_slice(&drawable_ubo_vector);
        context.write_storage_buffer(&mut self.drawable_uniform_buffer, bytes);
        layer_group.mutable_uniform_buffers().set(ID_LINE_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_group.mutable_uniform_buffers(), &projection_ubo_vector, context);
    }
}

/// Where a tile source's data comes from, as its style source has it.
enum Origin {
    /// A tileset's tiles: vector, raster or raster-dem.
    Tileset { tileset: Option<Tileset> },
    /// A GeoJSON source's data, and whether the source's tiles were made
    /// with synchronous updates.
    GeoJson { data: Option<Weak<dyn GeoJSONData>>, synchronous: bool },
    /// A custom geometry or custom vector source's loader.
    Custom,
}

/// The render source of a source with tiles: a view into the tile store,
/// which keeps the tiles, with the render tiles of the last `prepare`.
pub(crate) struct RenderTileSource {
    pub(crate) base: RenderSourceBase,
    key: SourceKey,
    origin: Origin,
    fast_pfor_enabled: bool,
    pub(crate) render_tiles: Rc<Vec<RenderTile>>,
    filtered_render_tiles: RefCell<Option<RenderTiles>>,
    render_tiles_sorted_by_y: RefCell<Option<RenderTiles>>,
    bearing: f32,
}

/// Whether `a` and `b` are both empty or both the same object.
fn same_data(a: Option<&Arc<dyn GeoJSONData>>, b: Option<&Arc<dyn GeoJSONData>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)),
        _ => false,
    }
}

/// The property `name`, if it is an unsigned integer.
fn get_uint_property(properties: &feature::PropertyMap, name: &str) -> Option<u64> {
    match properties.get(name) {
        Some(feature::Value::Uint(value)) => Some(*value),
        _ => None,
    }
}

/// The supercluster `extensionGetters`: the `extension_field` of cluster
/// `cluster_id`, or `None` for a field it doesn't have.
fn supercluster_extension(
    cluster_data: &dyn GeoJSONData,
    extension_field: &str,
    cluster_id: u32,
    args: Option<&feature::PropertyMap>,
) -> Option<Result<FeatureExtensionValue, Error>> {
    Some(match extension_field {
        "children" => cluster_data.get_children(cluster_id).map(FeatureExtensionValue::FeatureCollection),
        "leaves" => {
            // Offset cannot be set without limit.
            let (limit, offset) = match args.and_then(|args| Some((get_uint_property(args, "limit")?, args))) {
                Some((limit, args)) => {
                    (limit as u32, get_uint_property(args, "offset").map_or(0, |offset| offset as u32))
                }
                None => (10, 0),
            };
            cluster_data.get_leaves(cluster_id, limit, offset).map(FeatureExtensionValue::FeatureCollection)
        }
        "expansion-zoom" => cluster_data
            .get_cluster_expansion_zoom(cluster_id)
            .map(|zoom| FeatureExtensionValue::Value(feature::Value::Uint(u64::from(zoom)))),
        _ => return None,
    })
}

impl RenderTileSource {
    /// The render source of `impl_`, whose tiles go in `store`.
    pub(crate) fn new(impl_: Arc<dyn source::Impl>, store: &mut TileStore) -> Self {
        let (kind, single_layer, origin) = match impl_.type_ {
            SourceType::Raster => (TileKind::Raster, false, Origin::Tileset { tileset: None }),
            SourceType::RasterDEM => (TileKind::Dem(RasterEncoding::Mapbox), false, Origin::Tileset { tileset: None }),
            SourceType::GeoJSON => (TileKind::Geometry, true, Origin::GeoJson { data: None, synchronous: false }),
            SourceType::CustomVector => (TileKind::Geometry, true, Origin::Custom),
            SourceType::CustomMVTVector => (TileKind::Geometry, false, Origin::Custom),
            SourceType::Vector | SourceType::Image => (TileKind::Geometry, false, Origin::Tileset { tileset: None }),
        };
        let key = store.add_source(impl_.id.clone(), kind, single_layer);
        RenderTileSource {
            base: RenderSourceBase::new(impl_),
            key,
            origin,
            fast_pfor_enabled: false,
            render_tiles: Rc::new(Vec::new()),
            filtered_render_tiles: RefCell::new(None),
            render_tiles_sorted_by_y: RefCell::new(None),
            bearing: 0.0,
        }
    }

    /// Takes in a tileset source's `impl_`. Returns its tileset, if it has
    /// tiles; a new tileset clears the source's tiles.
    fn update_tileset(&mut self, store: &mut TileStore, parameters: &TileParameters<'_>) -> Option<Tileset> {
        let impl_ = downcast_arc::<tile_source::Impl>(&self.base.base_impl);
        let Origin::Tileset { tileset: cached } = &mut self.origin else {
            return None;
        };
        let impl_tileset = impl_.tileset.clone();
        // In Continuous mode, keep the existing tiles if the new tileset is
        // not yet available, thus providing smart style transitions without
        // flickering. In other modes, allow clearing the tiles first, before
        // the early return in order to avoid render tests being flaky.
        let can_update_tileset = impl_tileset.is_some() || parameters.mode != MapMode::Continuous;
        let (source, ctx) = store.source_mut(self.key)?;
        if can_update_tileset && *cached != impl_tileset {
            *cached = impl_tileset;
            // TODO: this removes existing buckets, and will cause flickering.
            // Should instead refresh tile data in place.
            source.clear_all(ctx);
            source.info.origin = None;
        }
        // A tileset without tiles makes no tiles, and the source counts as
        // loaded until its tiles are set.
        let tileset = cached.clone().filter(|tileset| !tileset.tiles.is_empty())?;
        let encoding = match self.base.base_impl.type_ {
            // The encoding is decided once, from the first tileset (`isMLT`).
            SourceType::Vector if tileset.vector_encoding == Some(VectorEncoding::MLT) => {
                VectorTileEncoding::Mlt { fast_pfor_enabled: self.fast_pfor_enabled }
            }
            _ => VectorTileEncoding::Mvt,
        };
        if let TileKind::Dem(_) = source.info.kind {
            source.info.kind = TileKind::Dem(tileset.raster_encoding.unwrap_or(RasterEncoding::Mapbox));
        }
        let file_source = parameters.file_source.clone();
        let current = matches!(
            &source.info.origin,
            Some(DataOrigin::Tileset { tileset: origin, encoding: current, file_source: current_file_source })
                if *origin == tileset
                    && *current == encoding
                    && same_file_source(current_file_source.as_ref(), file_source.as_ref())
        );
        if !current {
            source.info.origin = Some(DataOrigin::Tileset { tileset: tileset.clone(), encoding, file_source });
        }
        Some(tileset)
    }

    /// Takes in a GeoJSON source's `impl_`: new data loads into its tiles.
    /// Returns its data, if it has any.
    fn update_geojson(
        &mut self,
        store: &mut TileStore,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
    ) -> Option<Arc<dyn GeoJSONData>> {
        let impl_ = downcast_arc::<geojson_source_impl::Impl>(&self.base.base_impl);
        let Origin::GeoJson { data: current, synchronous } = &mut self.origin else {
            return None;
        };
        let (source, ctx) = store.source_mut(self.key)?;
        // A tile lays out in the mode it was created with, so a mode change
        // recreates the tiles.
        if parameters.is_update_synchronous != *synchronous {
            *synchronous = parameters.is_update_synchronous;
            source.clear_all(ctx);
        }
        let data = impl_.get_data().and_then(|weak| weak.upgrade());
        let previous = current.as_ref().and_then(Weak::upgrade);
        if !same_data(previous.as_ref(), data.as_ref()) {
            *current = data.as_ref().map(Arc::downgrade);
            source.info.origin = data.as_ref().map(|data| DataOrigin::GeoJson(Arc::clone(data)));
            source.info.synchronous = parameters.is_update_synchronous;
            if parameters.mode != MapMode::Continuous {
                // Clearing the tiles in order to avoid render tests being
                // flaky.
                source.clear_all(ctx);
            } else if let Some(data) = &data {
                source.reduce_memory_use(ctx);
                let max_z = impl_.get_zoom_range().max;
                let SourceTiles { info, tiles, .. } = source;
                for tile in tiles.values_mut().filter(|tile| !tile.cached && tile.id.canonical.z <= max_z) {
                    info.load_geojson(tile, ctx, data, needs_relayout);
                }
            }
        }
        data
    }

    /// Takes in a custom source's `impl_`, whose new options or loader clear
    /// the source's tiles. Returns whether it has a loader.
    fn update_custom(&mut self, store: &mut TileStore, base_impl: &Arc<dyn source::Impl>) -> bool {
        let previous = std::mem::replace(&mut self.base.base_impl, Arc::clone(base_impl));
        let Some((source, ctx)) = store.source_mut(self.key) else {
            return false;
        };
        let (differs, loader): (bool, Option<Arc<dyn CustomLoader>>) = if base_impl.type_ == SourceType::CustomVector {
            let impl_ = downcast_arc::<custom_geometry_source::Impl>(base_impl);
            let previous = downcast_arc::<custom_geometry_source::Impl>(&previous);
            let loader = impl_.get_tile_loader().map(|loader| -> Arc<dyn CustomLoader> {
                Arc::new(CustomLoaderOf {
                    loader: loader.clone(),
                    convert: Arc::new(|features: Arc<TileFeatures>| Some(TileData::Features(features))),
                })
            });
            (impl_.differs(&previous), loader)
        } else {
            let impl_ = downcast_arc::<custom_vector_source::Impl>(base_impl);
            let previous = downcast_arc::<custom_vector_source::Impl>(&previous);
            let fast_pfor_enabled = self.fast_pfor_enabled;
            let loader = impl_.get_tile_loader().map(|loader| -> Arc<dyn CustomLoader> {
                Arc::new(CustomLoaderOf {
                    loader: loader.clone(),
                    convert: Arc::new(move |(data, format): CustomVectorTileData| {
                        data.map(|data| match format {
                            TileDataFormat::Mvt => TileData::Mvt(mln_tile::MvtTile::new(data)),
                            TileDataFormat::Mlt => TileData::Mlt(mln_tile::MltTile::new(data, fast_pfor_enabled)),
                        })
                    }),
                })
            });
            (impl_.differs(&previous), loader)
        };
        // Clear the tiles only if the updated source has different tile
        // options, zoom range or initialization state for a custom tile
        // loader.
        if !Arc::ptr_eq(&previous, base_impl) && differs {
            source.clear_all(ctx);
        }
        let has_loader = loader.is_some();
        if source.info.origin.is_none() || !Arc::ptr_eq(&previous, base_impl) {
            source.info.origin = loader.map(DataOrigin::Custom);
        }
        has_loader
    }

    pub(crate) fn create_render_item(&mut self) -> Box<dyn RenderItem> {
        Box::new(TileSourceRenderItem::new(Rc::clone(&self.render_tiles), self.base.base_impl.id.clone()))
    }
}

/// Whether `a` and `b` are both absent or the same file source.
fn same_file_source(a: Option<&Arc<dyn FileSource>>, b: Option<&Arc<dyn FileSource>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b)),
        _ => false,
    }
}

impl RenderSource for RenderTileSource {
    fn base(&self) -> &RenderSourceBase {
        &self.base
    }

    fn is_loaded(&self, store: &TileStore) -> bool {
        store.source(self.key).is_none_or(SourceTiles::is_loaded)
    }

    fn update(
        &mut self,
        store: &mut TileStore,
        base_impl: Arc<dyn source::Impl>,
        layers: &[Arc<dyn LayerProperties>],
        needs_rendering: bool,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
    ) {
        let (tile_size, zoom_range, bounds) = match &self.origin {
            Origin::Tileset { .. } => {
                self.base.base_impl = base_impl;
                self.base.enabled = needs_rendering;
                let Some(tileset) = self.update_tileset(store, parameters) else {
                    return;
                };
                let impl_ = downcast_arc::<tile_source::Impl>(&self.base.base_impl);
                let tile_size = if self.base.base_impl.type_ == SourceType::Vector {
                    constants::TILE_SIZE_I
                } else {
                    impl_.get_tile_size()
                };
                (tile_size, tileset.zoom_range, tileset.bounds)
            }
            Origin::GeoJson { .. } => {
                self.base.base_impl = base_impl;
                self.base.enabled = needs_rendering;
                if self.update_geojson(store, needs_relayout, parameters).is_none() {
                    return;
                }
                let impl_ = downcast_arc::<geojson_source_impl::Impl>(&self.base.base_impl);
                (constants::TILE_SIZE_I, impl_.get_zoom_range(), None)
            }
            Origin::Custom => {
                let has_loader = self.update_custom(store, &base_impl);
                self.base.enabled = needs_rendering;
                if !has_loader {
                    return;
                }
                if base_impl.type_ == SourceType::CustomVector {
                    let impl_ = downcast_arc::<custom_geometry_source::Impl>(&base_impl);
                    (impl_.get_tile_options().tile_size, impl_.get_zoom_range(), None)
                } else {
                    let impl_ = downcast_arc::<custom_vector_source::Impl>(&base_impl);
                    (constants::TILE_SIZE_I, impl_.get_zoom_range(), None)
                }
            }
        };
        let Some((source, ctx)) = store.source_mut(self.key) else {
            return;
        };
        let update = CoverUpdate {
            layers,
            needs_rendering,
            needs_relayout,
            parameters,
            source_impl: &*self.base.base_impl,
            tile_size,
            zoom_range,
            bounds,
        };
        source.update_cover(&update, ctx);
    }

    fn create_render_item(&mut self) -> Box<dyn RenderItem> {
        RenderTileSource::create_render_item(self)
    }

    fn prepare(&mut self, store: &mut TileStore, parameters: &SourcePrepareParameters<'_>) {
        self.bearing = parameters.transform.state.get_bearing() as f32;
        *self.filtered_render_tiles.borrow_mut() = None;
        *self.render_tiles_sorted_by_y.borrow_mut() = None;
        let pool = store.pool().clone();
        let Some((source, _)) = store.source_mut(self.key) else {
            self.render_tiles = Rc::new(Vec::new());
            return;
        };
        // layers: feature state
        source.coalesce_feature_state();
        let mut tiles: Vec<RenderTile> = Vec::with_capacity(source.rendered_count());
        for (id, tile) in source.rendered() {
            let mut render_tile = RenderTile::new(*id, tile);
            render_tile.prepare(tile, &pool, parameters);
            tiles.push(render_tile);
        }
        self.render_tiles = Rc::new(tiles);
    }

    fn release_render_data(&mut self) {
        if !self.render_tiles.is_empty() {
            *self.filtered_render_tiles.borrow_mut() = None;
            *self.render_tiles_sorted_by_y.borrow_mut() = None;
            self.render_tiles = Rc::new(Vec::new());
        }
    }

    fn tiles(&self) -> Option<SourceKey> {
        Some(self.key)
    }

    fn get_render_tiles(&self) -> Option<RenderTiles> {
        let mut filtered = self.filtered_render_tiles.borrow_mut();
        if filtered.is_none() {
            let indices: Vec<usize> = self
                .render_tiles
                .iter()
                .enumerate()
                .filter(|(_, render_tile)| !render_tile.hold_for_fade())
                .map(|(i, _)| i)
                .collect();
            *filtered = Some(RenderTiles::new(Rc::clone(&self.render_tiles), indices));
        }
        filtered.clone()
    }

    fn get_render_tiles_sorted_by_y_position(&self) -> Option<RenderTiles> {
        let mut sorted = self.render_tiles_sorted_by_y.borrow_mut();
        if sorted.is_none() {
            let source_bearing = self.bearing;
            let tiles = &self.render_tiles;
            let mut indices: Vec<usize> = (0..tiles.len()).collect();
            indices.sort_by(|&a, &b| {
                let a = &tiles[a];
                let b = &tiles[b];
                let pa = Vec2::new(a.id.canonical.x as f32, a.id.canonical.y as f32);
                let pb = Vec2::new(b.id.canonical.x as f32, b.id.canonical.y as f32);

                let rotation = Vec2::from_angle(source_bearing);
                let par = rotation.rotate(pa);
                let pbr = rotation.rotate(pb);

                // std::tie(b.z, par.y, par.x) < std::tie(a.z, pbr.y, pbr.x)
                b.id.canonical.z.cmp(&a.id.canonical.z).then(par.y.total_cmp(&pbr.y)).then(par.x.total_cmp(&pbr.x))
            });
            *sorted = Some(RenderTiles::new(Rc::clone(&self.render_tiles), indices));
        }
        sorted.clone()
    }

    fn get_raw_render_tiles(&self) -> Rc<Vec<RenderTile>> {
        Rc::clone(&self.render_tiles)
    }

    /// The "supercluster" extension of a GeoJSON source: a cluster feature's
    /// "children", "leaves" (`args` "limit", default 10, and "offset",
    /// default 0) or "expansion-zoom". Anything else is a null value.
    fn query_feature_extensions(
        &self,
        feature: &mln_util::feature::Feature,
        extension: &str,
        extension_field: &str,
        args: Option<&feature::PropertyMap>,
    ) -> Result<FeatureExtensionValue, Error> {
        let Origin::GeoJson { data, .. } = &self.origin else {
            return Ok(FeatureExtensionValue::default());
        };
        if extension != "supercluster" {
            return Ok(FeatureExtensionValue::default());
        }
        let Some(cluster_id) = get_uint_property(&feature.properties, "cluster_id") else {
            return Ok(FeatureExtensionValue::default());
        };
        let Some(json_data) = data.as_ref().and_then(Weak::upgrade) else {
            return Ok(FeatureExtensionValue::default());
        };
        supercluster_extension(&*json_data, extension_field, cluster_id as u32, args)
            .unwrap_or_else(|| Ok(FeatureExtensionValue::default()))
    }

    fn set_fast_pfor_enabled(&mut self, enable: bool) {
        self.fast_pfor_enabled = enable;
    }

    fn dump_debug_logs(&self) {}

    fn get_max_zoom(&self) -> u8 {
        match &self.origin {
            Origin::Tileset { tileset: Some(tileset) } => tileset.zoom_range.max,
            _ => constants::TERRAIN_RGB_MAXZOOM,
        }
    }
}
