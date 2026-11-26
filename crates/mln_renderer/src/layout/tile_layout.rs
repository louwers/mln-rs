//! A tile's layout, in two stages. [`discover`] evaluates what the tile's
//! layers need from glyphs and images; [`build`], given those, makes every
//! bucket and the feature index. Both read only their arguments, so a
//! layout can stop after `discover` to fetch what it needs and run
//! `discover` again later instead of keeping its result.

use std::sync::Arc;

use mln_map_types::MapMode;
use mln_style::error::Error;
use mln_style::expression::{GlobalStateMap, UnavailableImages};
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_text::atlas::ImagePositions;
use mln_text::glyph::{FontFaces, GlyphDependencies, GlyphPositions};
use mln_text::tagged_string::font_stack_hash;
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_vec_map::VecSet;
use mln_wire::sorted;

use crate::bucket_parameters::BucketParameters;
use crate::geometry::feature_index::FeatureIndexBuilder;
use crate::group_by_layout::group_layers;
use crate::image_store::{ImageDependencies, ImageType};
use crate::layer_type::{ErasedEvaluatedLayer, ErasedLayout};
use crate::layout::filter_groups::FilterGroups;
use crate::layout::layout::{BuildParams, LayoutParameters, SourceLayer};
use crate::text::glyph_store::ShapeResults;
use crate::tile::contents::LayoutBuckets;

/// How a tile is laid out, apart from its style and data.
#[derive(Clone, Copy, Debug, mln_wire::Wire)]
pub(crate) struct LayoutParams {
    pub(crate) mode: MapMode,
    pub(crate) pixel_ratio: f32,
    pub(crate) subdivision_granularity: SubdivisionGranularitySetting,
    /// Whether the tile has had no layout yet.
    pub(crate) first_load: bool,
}

/// What a tile's layout reads, apart from its dependencies.
pub(crate) struct LayoutInput<'a> {
    pub(crate) id: OverscaledTileID,
    /// `None` when the tile has no data.
    pub(crate) data: Option<&'a TileData>,
    /// The source's layers, in style order.
    pub(crate) layers: &'a [Arc<dyn ErasedEvaluatedLayer>],
    pub(crate) global_state: Option<&'a Arc<GlobalStateMap>>,
    pub(crate) font_faces: Option<&'a Arc<FontFaces>>,
    /// Whether the style has a `glyphs` URL.
    pub(crate) glyph_pbfs: bool,
    /// The images that exist, which `image` expressions branch on.
    pub(crate) available_images: &'a VecSet<String>,
    pub(crate) params: LayoutParams,
}

impl LayoutInput<'_> {
    fn bucket_parameters(&self) -> BucketParameters {
        BucketParameters {
            tile_id: self.id,
            mode: self.params.mode,
            pixel_ratio: self.params.pixel_ratio,
            global_state: self.global_state.cloned(),
            subdivision_granularity: self.params.subdivision_granularity,
        }
    }
}

/// What a tile's layout needs from glyphs and images.
#[derive(Clone, Debug, Default, mln_wire::Wire)]
pub(crate) struct Dependencies {
    /// The glyph ids per font stack, and the text that font faces shape.
    pub(crate) glyphs: GlyphDependencies,
    /// The icons and patterns. The images that `image` expressions found
    /// unavailable are icons here too, so that adding one lays the tile out
    /// again.
    #[wire(with = "sorted::map")]
    pub(crate) images: ImageDependencies,
    /// The images that `image` expressions found unavailable, which the
    /// layout branched on.
    pub(crate) unavailable_images: VecSet<String>,
}

impl Dependencies {
    /// Whether `resolved` answers every one of these: each glyph has a
    /// position or is known missing, each text its shaping, and each image a
    /// position of its type or is known missing.
    pub(crate) fn satisfied_by(&self, resolved: &Resolved) -> bool {
        let glyphs = self.glyphs.glyphs.iter().all(|(font_stack, ids)| {
            resolved
                .glyphs
                .get(&font_stack_hash(font_stack))
                .is_some_and(|positions| ids.iter().all(|id| positions.contains_key(id)))
        });
        let shapes = self.glyphs.shapes.iter().all(|(font_stack, types)| {
            let shaped = resolved.shaping.get(font_stack);
            types.iter().all(|(type_, texts)| {
                let shaped = shaped.and_then(|shaped| shaped.get(type_));
                texts.iter().all(|text| shaped.is_some_and(|shaped| shaped.contains_key(text)))
            })
        });
        let images = self.images.iter().all(|(id, type_)| {
            let positions = match type_ {
                ImageType::Icon => &resolved.icons,
                ImageType::Pattern => &resolved.patterns,
            };
            positions.contains_key(id) || resolved.missing_images.contains(id)
        });
        glyphs && shapes && images
    }
}

/// What a layout's dependencies resolved to: where its glyphs and images are
/// in the atlas pages the render thread packed them into, and the shaping of
/// the text that font faces draw. No bitmaps or pixels.
#[derive(Debug, Default, mln_wire::Wire)]
pub(crate) struct Resolved {
    /// `None` for a glyph that no font has, or whose range failed.
    pub(crate) glyphs: GlyphPositions,
    pub(crate) shaping: ShapeResults,
    #[wire(with = "sorted::map")]
    pub(crate) icons: ImagePositions,
    #[wire(with = "sorted::map")]
    pub(crate) patterns: ImagePositions,
    /// The requested images that are missing, which a layout lays out
    /// without.
    pub(crate) missing_images: VecSet<String>,
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
    layers: Vec<Arc<dyn ErasedEvaluatedLayer>>,
    /// The layout of a group that can need glyphs or images, with its
    /// features selected and evaluated; `None` for a group that `build`
    /// lays out.
    layout: Option<Box<dyn ErasedLayout>>,
}

/// A tile's buckets and feature index.
pub(crate) struct Built {
    /// The buckets, and each layer's.
    pub(crate) buckets: LayoutBuckets,
    pub(crate) feature_index: FeatureIndexBuilder,
}

/// Groups the layers, builds the layouts of the groups that can need glyphs
/// or images (symbol layers, and fill, line and fill-extrusion layers with a
/// pattern), and records what they need. It builds no bucket.
pub(crate) fn discover(input: &LayoutInput<'_>) -> Result<Discovered, Error> {
    let mut discovered = Discovered {
        groups: Vec::new(),
        filter_groups: FilterGroups::default(),
        dependencies: Dependencies::default(),
    };
    let Some(data) = input.data else {
        return Ok(discovered);
    };
    let unavailable_images = UnavailableImages::new();
    let bucket_parameters = input.bucket_parameters();

    for (_, layers) in group_layers(input.layers) {
        let source_layer = &layers[0].style().base().source_layer;
        if !data.has_layer(source_layer)? {
            continue;
        }
        let layer_type = layers[0].entry();
        let layout = if layer_type.layout_can_need_dependencies(&layers) {
            layer_type.create_layout(
                &layers,
                &mut LayoutParameters {
                    bucket_parameters: &bucket_parameters,
                    font_faces: input.font_faces.cloned(),
                    glyph_pbfs: input.glyph_pbfs,
                    glyph_dependencies: &mut discovered.dependencies.glyphs,
                    image_dependencies: &mut discovered.dependencies.images,
                    available_images: input.available_images,
                    unavailable_images: &unavailable_images,
                    filter_groups: &mut discovered.filter_groups,
                },
                SourceLayer::new(data.clone(), source_layer.clone()),
            )?
        } else {
            None
        };
        discovered.groups.push(DiscoveredGroup { layers, layout });
    }

    for id in unavailable_images.into_inner() {
        discovered.dependencies.images.entry(id.clone()).or_insert(ImageType::Icon);
        discovered.dependencies.unavailable_images.insert(id);
    }
    Ok(discovered)
}

/// Builds every bucket and the feature index, from what [`discover`] made of
/// the same `input` and what its dependencies resolved to. The feature index
/// gets the groups that use no glyphs or images first, then the others,
/// each in group order.
pub(crate) fn build(discovered: Discovered, input: &LayoutInput<'_>, resolved: &Resolved) -> Result<Built, Error> {
    let Discovered { groups, mut filter_groups, dependencies: _ } = discovered;
    let mut built =
        Built { buckets: LayoutBuckets::default(), feature_index: FeatureIndexBuilder::new(input.data.cloned()) };
    let Some(data) = input.data else {
        return Ok(built);
    };
    // Room for every layer of the groups, of which those with data keep it.
    built.buckets = LayoutBuckets::with_capacity(groups.iter().map(|group| group.layers.len()).sum());
    let LayoutParams { first_load, .. } = input.params;
    let build = BuildParams { first_load, canonical: input.id.canonical };
    let bucket_parameters = input.bucket_parameters();

    let mut waiting = Vec::new();
    for DiscoveredGroup { layers, layout } in groups {
        let leader = layers[0].style();
        let layer_ids: Vec<String> = layers.iter().map(|layer| layer.style().base().id.clone()).collect();
        built.feature_index.set_bucket_layer_ids(&leader.base().id, &layer_ids);

        let layout = match layout {
            Some(layout) => Some(layout),
            // Layouts of groups that need no glyphs or images record nothing.
            None => layers[0].entry().create_layout(
                &layers,
                &mut LayoutParameters {
                    bucket_parameters: &bucket_parameters,
                    font_faces: input.font_faces.cloned(),
                    glyph_pbfs: input.glyph_pbfs,
                    glyph_dependencies: &mut GlyphDependencies::default(),
                    image_dependencies: &mut ImageDependencies::default(),
                    available_images: input.available_images,
                    unavailable_images: &UnavailableImages::new(),
                    filter_groups: &mut filter_groups,
                },
                SourceLayer::new(data.clone(), leader.base().source_layer.clone()),
            )?,
        };
        match layout {
            Some(layout) if layout.has_dependencies() => waiting.push(layout),
            Some(mut layout) => layout.create_bucket(None, &mut built, &build)?,
            None => {}
        }
    }

    for mut layout in waiting {
        layout.create_bucket(Some(resolved), &mut built, &build)?;
    }
    built.buckets.shrink_to_fit();
    Ok(built)
}
