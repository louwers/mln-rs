//! A tile's layout, in two stages. [`discover`] evaluates what the tile's
//! layers need from glyphs and images; [`build`], given those, makes every
//! bucket and the feature index. Both read only their arguments, so a
//! layout can stop after `discover` to fetch what it needs and run
//! `discover` again later instead of keeping its result.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_expression::image_expression::UnavailableImages;
use mln_map_types::MapMode;
use mln_style::expression::{EvaluationContext, GlobalStateMap};
use mln_style::layer::{LayerTypeInfo, layer_type_info};
use mln_style::layer_properties::LayerProperties;
use mln_style_types::image::{ImageDependencies, ImageMap, ImagePositions, ImageType};
use mln_text::glyph::{FontFaces, GlyphDependencies, GlyphMap, GlyphPositions};
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_tile_id::geometry_tile_data::GeometryTileFeature;
use mln_util::error::Error;
use mln_util::hash::HashMap;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use crate::bucket::BucketPtr;
use crate::bucket_parameters::BucketParameters;
use crate::geometry::feature_index::FeatureIndex;
use crate::group_by_layout::group_layers;
use crate::layermanager;
use crate::layout::filter_groups::FilterGroups;
use crate::layout::layout::{Layout, LayoutParameters, SourceLayer};
use crate::layout::pattern_layout::PatternLayerMap;
use crate::render_layer::LayerRenderData;
use crate::text::glyph_manager::HBShapeResults;

/// How a tile is laid out, apart from its style and data.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LayoutParams {
    pub(crate) mode: MapMode,
    pub(crate) pixel_ratio: f32,
    pub(crate) show_collision_boxes: bool,
    pub(crate) subdivision_granularity: SubdivisionGranularitySetting,
    /// Whether the tile has had no layout yet.
    pub(crate) first_load: bool,
    /// Whether buckets keep their features by id.
    pub(crate) capture_rendered_features: bool,
}

/// What a tile's layout reads, apart from its dependencies.
pub(crate) struct LayoutInput<'a> {
    pub(crate) id: OverscaledTileID,
    /// `None` when the tile has no data.
    pub(crate) data: Option<&'a TileData>,
    /// The source's layers, in style order.
    pub(crate) layers: &'a [Arc<dyn LayerProperties>],
    pub(crate) global_state: Option<&'a Arc<GlobalStateMap>>,
    pub(crate) font_faces: Option<&'a Arc<FontFaces>>,
    /// The images that exist, which `image` expressions branch on.
    pub(crate) available_images: &'a VecSet<String>,
    pub(crate) params: LayoutParams,
}

impl LayoutInput<'_> {
    fn bucket_parameters(&self, layer_type: &'static LayerTypeInfo) -> BucketParameters {
        BucketParameters {
            tile_id: self.id,
            mode: self.params.mode,
            pixel_ratio: self.params.pixel_ratio,
            layer_type,
            global_state: self.global_state.cloned(),
            retain_features_by_id: self.params.capture_rendered_features,
            subdivision_granularity: self.params.subdivision_granularity,
        }
    }
}

/// What a tile's layout needs from glyphs and images.
#[derive(Clone, Debug, Default)]
pub(crate) struct Dependencies {
    /// The glyph ids per font stack, and the text that font faces shape.
    pub(crate) glyphs: GlyphDependencies,
    /// The icons and patterns. The images that `image` expressions found
    /// unavailable are icons here too, so that adding one lays the tile out
    /// again.
    pub(crate) images: ImageDependencies,
}

/// What the dependencies of a layout resolved to.
pub(crate) struct ResolvedDependencies<'a> {
    pub(crate) glyphs: &'a GlyphMap,
    pub(crate) glyph_positions: &'a GlyphPositions,
    pub(crate) icons: &'a ImageMap,
    pub(crate) icon_positions: &'a ImagePositions,
    pub(crate) pattern_positions: &'a ImagePositions,
    /// The shaping of the text that font faces draw.
    pub(crate) shaping: &'a HBShapeResults,
}

/// A tile's layer groups, as [`discover`] left them for [`build`].
pub(crate) struct Discovered {
    /// The groups whose source layer the data has, in group order.
    groups: Vec<DiscoveredGroup>,
    /// The filter selections the layouts made, which `build` reuses.
    filter_groups: FilterGroups,
    pub(crate) dependencies: Dependencies,
}

struct DiscoveredGroup {
    layers: Vec<Arc<dyn LayerProperties>>,
    /// The layout of a group that can need glyphs or images, with its
    /// features selected and evaluated; `None` for a group `build` lays out
    /// alone.
    layout: Option<Box<dyn Layout>>,
}

impl Discovered {
    /// Whether a layout uses glyphs or images, so that `build` finishes it
    /// after the other groups.
    pub(crate) fn has_waiting_layouts(&self) -> bool {
        self.groups.iter().any(|group| group.layout.as_ref().is_some_and(|layout| layout.has_dependencies()))
    }
}

/// A tile's buckets and feature index.
pub(crate) struct Built {
    /// Each layer's bucket, by layer id.
    pub(crate) render_data: HashMap<String, LayerRenderData>,
    pub(crate) feature_index: FeatureIndex,
}

/// Groups the layers, builds the layouts of the groups that can need glyphs
/// or images (symbol layers, and fill, line and fill-extrusion layers with a
/// pattern), and records what they need. It builds no bucket. `None` if
/// `obsolete` was set.
pub(crate) fn discover(input: &LayoutInput<'_>, obsolete: &AtomicBool) -> Result<Option<Discovered>, Error> {
    let mut discovered = Discovered {
        groups: Vec::new(),
        filter_groups: FilterGroups::default(),
        dependencies: Dependencies::default(),
    };
    let Some(data) = input.data else {
        return Ok(Some(discovered));
    };
    let unavailable_images = UnavailableImages::new();

    for (_, layers) in group_layers(input.layers) {
        if obsolete.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let leader_impl = &layers[0].base_impl;
        if !data.has_layer(&leader_impl.source_layer)? {
            continue;
        }
        let layer_type = leader_impl.get_type_info();
        let layout = if layer_type.layout == layer_type_info::Layout::Required
            && layermanager::layout_can_need_dependencies(layer_type, &layers)
        {
            layermanager::create_layout(
                &mut LayoutParameters {
                    bucket_parameters: &input.bucket_parameters(layer_type),
                    font_faces: input.font_faces.cloned(),
                    glyph_dependencies: &mut discovered.dependencies.glyphs,
                    image_dependencies: &mut discovered.dependencies.images,
                    available_images: input.available_images,
                    unavailable_images: &unavailable_images,
                    filter_groups: &mut discovered.filter_groups,
                },
                SourceLayer::new(data.clone(), leader_impl.source_layer.clone()),
                &layers,
            )?
        } else {
            None
        };
        discovered.groups.push(DiscoveredGroup { layers, layout });
    }

    for id in unavailable_images.into_inner() {
        discovered.dependencies.images.entry(id).or_insert(ImageType::Icon);
    }
    Ok(Some(discovered))
}

/// Builds every bucket and the feature index, from what [`discover`] made of
/// the same `input` and what its dependencies resolved to. The feature index
/// gets the groups that use no glyphs or images first, then the others,
/// each in group order. `None` if `obsolete` was set.
pub(crate) fn build(
    discovered: Discovered,
    input: &LayoutInput<'_>,
    resolved: &ResolvedDependencies<'_>,
    obsolete: &AtomicBool,
) -> Result<Option<Built>, Error> {
    // Avoid small reallocations for populated cells. If we had a total
    // feature count, this could be based on that and the cell count.
    const ESTIMATED_ELEMENTS_PER_CELL: usize = 8;

    let Discovered { groups, mut filter_groups, dependencies: _ } = discovered;
    let mut built = Built { render_data: HashMap::default(), feature_index: FeatureIndex::new(input.data.cloned()) };
    built.feature_index.reserve(ESTIMATED_ELEMENTS_PER_CELL);
    let Some(data) = input.data else {
        return Ok(Some(built));
    };
    let canonical = &input.id.canonical;
    let LayoutParams { first_load, show_collision_boxes, .. } = input.params;

    let mut waiting = Vec::new();
    for DiscoveredGroup { layers, layout } in groups {
        if obsolete.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let leader_impl = &layers[0].base_impl;
        let layer_ids: Vec<String> = layers.iter().map(|layer| layer.base_impl.id.clone()).collect();
        built.feature_index.set_bucket_layer_ids(&leader_impl.id, &layer_ids);

        match layout {
            Some(layout) if layout.has_dependencies() => waiting.push(layout),
            Some(mut layout) => layout.create_bucket(
                &ImagePositions::default(),
                &mut built.feature_index,
                &mut built.render_data,
                first_load,
                show_collision_boxes,
                canonical,
            )?,
            None => {
                if !build_group(&layers, data, input, &mut filter_groups, &mut built, obsolete)? {
                    return Ok(None);
                }
            }
        }
    }

    for mut layout in waiting {
        if obsolete.load(Ordering::SeqCst) {
            return Ok(None);
        }
        if layout.need_finalize_symbols() {
            layout.finalize_symbols(resolved.shaping);
        }
        layout.prepare_symbols(resolved.glyphs, resolved.glyph_positions, resolved.icons, resolved.icon_positions);
        if !layout.has_symbol_instances() {
            continue;
        }
        layout.create_bucket(
            resolved.pattern_positions,
            &mut built.feature_index,
            &mut built.render_data,
            first_load,
            show_collision_boxes,
            canonical,
        )?;
    }
    Ok(Some(built))
}

/// Builds the bucket of a group that needs no glyphs or images. `false` if
/// `obsolete` was set.
fn build_group(
    layers: &[Arc<dyn LayerProperties>],
    data: &TileData,
    input: &LayoutInput<'_>,
    filter_groups: &mut FilterGroups,
    built: &mut Built,
    obsolete: &AtomicBool,
) -> Result<bool, Error> {
    let leader_impl = &layers[0].base_impl;
    let layer_type = leader_impl.get_type_info();
    let parameters = input.bucket_parameters(layer_type);
    let canonical = &input.id.canonical;

    if layer_type.layout == layer_type_info::Layout::Required {
        // Layouts of groups that need no glyphs or images record nothing.
        let layout = layermanager::create_layout(
            &mut LayoutParameters {
                bucket_parameters: &parameters,
                font_faces: input.font_faces.cloned(),
                glyph_dependencies: &mut GlyphDependencies::default(),
                image_dependencies: &mut ImageDependencies::default(),
                available_images: input.available_images,
                unavailable_images: &UnavailableImages::new(),
                filter_groups,
            },
            SourceLayer::new(data.clone(), leader_impl.source_layer.clone()),
            layers,
        )?;
        if let Some(mut layout) = layout {
            debug_assert!(!layout.has_dependencies());
            layout.create_bucket(
                &ImagePositions::default(),
                &mut built.feature_index,
                &mut built.render_data,
                input.params.first_load,
                input.params.show_collision_boxes,
                canonical,
            )?;
        }
        return Ok(true);
    }

    let filter = &leader_impl.filter;
    let source_layer_id = &leader_impl.source_layer;
    let Some(mut bucket) = layermanager::create_bucket(&parameters, layers) else {
        return Ok(true);
    };
    let Some(geometry_layer) = data.layer(source_layer_id)? else {
        return Ok(true);
    };

    bucket.set_retain_features_by_id(input.params.capture_rendered_features);
    bucket.reserve_features(geometry_layer.len());

    let effective_z = f32::from(input.id.overscaled_z);
    let context = EvaluationContext::new(effective_z)
        .with_canonical_tile_id(Some(canonical))
        .with_global_state(input.global_state.map(|state| &**state));
    let passes =
        |feature: &dyn GeometryTileFeature| filter.evaluate(&EvaluationContext { feature: Some(feature), ..context });
    let selected = filter_groups.select(source_layer_id, &geometry_layer, filter, &context, passes);

    for i in 0..geometry_layer.len() {
        if obsolete.load(Ordering::SeqCst) {
            return Ok(false);
        }
        if selected.as_ref().is_some_and(|selected| !selected[i]) {
            continue;
        }
        let feature = geometry_layer.feature(i)?;
        if selected.is_none() && !passes(&feature) {
            continue;
        }

        let geometries = feature.geometry()?;
        bucket.add_feature(
            &feature,
            &geometries,
            &ImagePositions::default(),
            &PatternLayerMap::default(),
            i,
            canonical,
        )?;
        built.feature_index.insert(&geometries, i, source_layer_id, &leader_impl.id);
    }

    if bucket.has_data() {
        let bucket: BucketPtr = Arc::from(bucket);
        for layer in layers {
            // `emplace` keeps an existing entry.
            built.render_data.entry(layer.base_impl.id.clone()).or_insert_with(|| LayerRenderData {
                bucket: Arc::clone(&bucket),
                layer_properties: Arc::clone(layer),
            });
        }
    }
    Ok(true)
}
