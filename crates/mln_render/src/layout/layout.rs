// A `TileLayer` borrows its `TileData`, and the worker keeps layouts with
// pending dependencies across messages, so layouts keep a `SourceLayer`: the
// tile data and the layer name, from which their contexts borrow the layer
// again when they run.

use std::sync::Arc;

use mln_style::PropertyEvaluationParameters;
use mln_style::expression::EvaluationContext;
use mln_style::expression::image_expression::UnavailableImages;
use mln_style::layer_type::StyleLayerType;
use mln_style_types::image::ImageDependencies;
use mln_text::glyph::GlyphDependencies;
use mln_tile::{TileData, TileLayer};
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::GeometryTileFeature;
use mln_util::error::Error;
use mln_vec_map::VecSet;

use crate::bucket_parameters::BucketParameters;
use crate::layer::{BuildCx, EvaluatedLayer, Layout, LayoutCx};
use crate::layout::filter_groups::FilterGroups;
use crate::layout::tile_layout::{Built, Resolved};

/// The layer a layout reads: its tile data and its name.
#[derive(Clone)]
pub(crate) struct SourceLayer {
    pub(crate) data: TileData,
    pub(crate) name: String,
}

impl SourceLayer {
    /// The layer `name` of `data`.
    pub(crate) fn new(data: TileData, name: impl Into<String>) -> Self {
        SourceLayer { data, name: name.into() }
    }

    /// The layer. The worker only creates layouts for layers the data has,
    /// so neither error is expected.
    pub(crate) fn layer(&self) -> Result<TileLayer<'_>, Error> {
        self.data
            .layer(&self.name)?
            .ok_or_else(|| Error::native(format!("SourceLayer: the tile data has no layer {:?}", self.name)))
    }
}

/// What making a bucket reads apart from the layout and its dependencies.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BuildParams {
    /// Whether the tile has had no layout yet.
    pub(crate) first_load: bool,
    /// Whether the map shows collision boxes, which only the types that
    /// place read.
    pub(crate) show_collision_boxes: bool,
    /// The tile the bucket is of.
    pub(crate) canonical: CanonicalTileID,
}

/// What a tile's layout reads, and where it records the glyphs and images
/// its layers need.
pub(crate) struct LayoutParameters<'a> {
    /// The tile the layout lays out, and how.
    pub(crate) bucket_parameters: &'a BucketParameters,
    /// The style's font faces, and its fallback faces after them.
    pub(crate) font_faces: Option<Arc<mln_text::glyph::FontFaces>>,
    /// Whether the style has a `glyphs` URL.
    pub(crate) glyph_pbfs: bool,
    /// Where a layout records the glyphs it needs.
    pub(crate) glyph_dependencies: &'a mut GlyphDependencies,
    /// Where a layout records the images it needs.
    pub(crate) image_dependencies: &'a mut ImageDependencies,
    /// The images of the style.
    pub(crate) available_images: &'a VecSet<String>,
    /// Where `image` expressions record the images not in `available_images`.
    pub(crate) unavailable_images: &'a UnavailableImages,
    /// The tile's feature groups, shared by the layouts' filters.
    pub(crate) filter_groups: &'a mut FilterGroups,
}

/// A layer type's layout of a group, as its [`Layout::new`] made it.
pub(crate) struct NewLayout<L> {
    pub(crate) layout: L,
    /// Whether it asked for glyphs or images, which its bucket then waits for.
    pub(crate) has_dependencies: bool,
    /// The tile and how it is laid out.
    parameters: BucketParameters,
}

/// The layout `L` of `group` on `source_layer`, with what it asks for
/// recorded in `parameters`.
pub(crate) fn new_layout<S: StyleLayerType, L: Layout<S>>(
    group: &[Arc<EvaluatedLayer<S>>],
    parameters: &mut LayoutParameters<'_>,
    source_layer: &SourceLayer,
) -> Result<NewLayout<L>, Error> {
    debug_assert!(!group.is_empty());
    let bucket_parameters = parameters.bucket_parameters;
    let mut evaluation = PropertyEvaluationParameters::new(f32::from(bucket_parameters.tile_id.overscaled_z));
    evaluation.global_state = bucket_parameters.global_state.clone();
    let mut cx = LayoutCx {
        layers: group,
        tile: bucket_parameters.tile_id,
        mode: bucket_parameters.mode,
        parameters: evaluation,
        source_layer: source_layer.layer()?,
        filter_groups: parameters.filter_groups,
        glyphs: parameters.glyph_dependencies,
        images: parameters.image_dependencies,
        available_images: parameters.available_images,
        unavailable_images: parameters.unavailable_images,
    };
    let layout = L::new(&mut cx)?;
    Ok(NewLayout { layout, has_dependencies: L::needs_resources(group), parameters: bucket_parameters.clone() })
}

/// The bucket of `layout`, a layout of `group` on `source_layer`, whose
/// dependencies resolved to `resolved`, with its features added to
/// `built`'s feature index.
pub(crate) fn build_layout<S: StyleLayerType, L: Layout<S>>(
    layout: NewLayout<L>,
    group: &[Arc<EvaluatedLayer<S>>],
    source_layer: &SourceLayer,
    resolved: Option<&Resolved>,
    built: &mut Built,
    build: &BuildParams,
) -> Result<Option<L::Bucket>, Error> {
    let NewLayout { layout, parameters, .. } = layout;
    let mut cx = BuildCx {
        layers: group,
        tile: parameters.tile_id,
        mode: parameters.mode,
        first_load: build.first_load,
        source_layer: source_layer.layer()?,
        resolved,
        feature_index: &mut built.feature_index,
    };
    layout.build(&mut cx)
}

/// The indexes of the features of `cx`'s source layer that its leader's
/// filter selects, in feature order.
pub(crate) fn select<S: StyleLayerType>(cx: &mut LayoutCx<'_, S>) -> Result<Vec<usize>, Error> {
    let leader = &cx.layers[0].style;
    let layer = &cx.source_layer;
    let context = EvaluationContext::new(f32::from(cx.tile.overscaled_z))
        .with_canonical_tile_id(Some(&cx.tile.canonical))
        .with_global_state(cx.parameters.global_state.as_deref());
    let passes = |feature: &dyn GeometryTileFeature| {
        leader.filter.evaluate(&EvaluationContext { feature: Some(feature), ..context })
    };
    let selected = cx.filter_groups.select(&leader.source_layer, layer, &leader.filter, &context, passes);
    let mut features = Vec::new();
    for i in 0..layer.len() {
        match &selected {
            Some(selected) if !selected[i] => {}
            Some(_) => features.push(i),
            None => {
                if passes(&layer.feature(i)?) {
                    features.push(i);
                }
            }
        }
    }
    Ok(features)
}
