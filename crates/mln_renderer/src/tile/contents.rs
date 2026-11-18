//! What a tile draws and answers queries with: its contents, built from one
//! load of its data. A tile keeps showing its contents while newer data or
//! work is pending, and the contents keep what they were built from (the
//! tile data, the resolved glyphs and images, and the leases on their atlas
//! bins), so queries and feature state read what is drawn.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use mln_style::expression::GlobalStateMap;
use mln_style::layer_type::ErasedLayerStyle;
use mln_style_types::image::{ImagePositions, ImageType};
use mln_text::glyph::{GlyphID, GlyphPositions};
use mln_text::tagged_string::font_stack_hash;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::feature::Feature;
use mln_util::hash::HashMap;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};
use mln_util::mat4::Mat4;
use mln_util::pool::TaskGroup;
use mln_vec_map::{VecMap, VecSet};

use crate::bucket::{Bucket, LayerFeatureStates, UploadNeeds};
use crate::bucket_geometry::{BucketGeometry, GeometryUpload};
use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::buckets::raster_bucket::RasterBucket;
use crate::geometry::feature_index::{FeatureIndex, QueryLayers, QueryResults};
use crate::gfx::context::Context;
use crate::gfx::dynamic_texture_atlas::{AtlasLease, BinUse, DynamicTextureAtlas};
use crate::layer_drawing::{BucketChanges, ChangedBuckets};
use crate::layer_type::ErasedEvaluatedLayer;
use crate::layout::tile_layout::{Dependencies, Resolved};
use crate::query::{RenderedQueryOptions, SourceQueryOptions};
use crate::render_layer::ErasedLayerBucket;
use crate::render_orchestrator::drop_on_workers;
use crate::source_state::SourceFeatureState;
use crate::text::glyph_store::HBShapeResults;
use crate::tile_mask::TileMask;
use crate::tile_render_data::TileAtlasTextures;

/// A layout's buckets, each once, and the bucket of each layer it laid out,
/// with the values it laid the layer out with. The layers of a layout group
/// share their bucket.
#[derive(Default)]
pub(crate) struct LayoutBuckets {
    buckets: Vec<Box<dyn Bucket>>,
    layers: HashMap<String, LaidLayer>,
}

/// A layer a layout laid out: the index of its bucket, and its values.
#[derive(Clone)]
struct LaidLayer {
    bucket: usize,
    layer: Arc<dyn ErasedEvaluatedLayer>,
}

impl LayoutBuckets {
    /// Adds `bucket`, the bucket of `layers`, for those of them that have
    /// none yet. Drops it if they all have one.
    pub(crate) fn add(
        &mut self,
        bucket: Box<dyn Bucket>,
        layers: impl IntoIterator<Item = (String, Arc<dyn ErasedEvaluatedLayer>)>,
    ) {
        let index = self.buckets.len();
        let mut used = false;
        for (id, layer) in layers {
            self.layers.entry(id).or_insert_with(|| {
                used = true;
                LaidLayer { bucket: index, layer }
            });
        }
        if used {
            self.buckets.push(bucket);
        }
    }

    /// The bucket of the layer `layer_id`.
    #[inline]
    pub(crate) fn get(&self, layer_id: &str) -> Option<ErasedLayerBucket<'_>> {
        Some(self.layer_bucket(self.layers.get(layer_id)?))
    }

    fn layer_bucket<'a>(&'a self, laid: &'a LaidLayer) -> ErasedLayerBucket<'a> {
        ErasedLayerBucket { bucket: &*self.buckets[laid.bucket], bucket_index: laid.bucket, layer: &*laid.layer }
    }

    /// The number of layers laid out.
    #[cfg(any(test, feature = "bench"))]
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.layers.len()
    }

    /// The bucket of the layer `layer_id`, out of the layout.
    #[cfg(test)]
    pub(crate) fn into_bucket(mut self, layer_id: &str) -> Option<Box<dyn Bucket>> {
        let index = self.layers.get(layer_id)?.bucket;
        Some(self.buckets.swap_remove(index))
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn contains_key(&self, layer_id: &str) -> bool {
        self.layers.contains_key(layer_id)
    }

    /// The layers laid out, with their buckets, in no order.
    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, ErasedLayerBucket<'_>)> + '_ {
        self.layers.iter().map(|(id, laid)| (id, self.layer_bucket(laid)))
    }

    /// The layers laid out, each with the index of its bucket and the values
    /// it was laid out with, in no order.
    pub(crate) fn laid_layers(&self) -> impl Iterator<Item = (&String, usize, &Arc<dyn ErasedEvaluatedLayer>)> + '_ {
        self.layers.iter().map(|(id, laid)| (id, laid.bucket, &laid.layer))
    }

    /// The number of buckets, each of which some layer renders.
    pub(crate) fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    /// The bucket at `index`, as [`ErasedLayerBucket::bucket_index`] gives it.
    pub(crate) fn bucket_mut(&mut self, index: usize) -> Option<&mut dyn Bucket> {
        Some(&mut **self.buckets.get_mut(index)?)
    }

    /// The bucket at `index`, as [`ErasedLayerBucket::bucket_index`] gives it.
    pub(crate) fn bucket(&self, index: usize) -> Option<&dyn Bucket> {
        Some(&**self.buckets.get(index)?)
    }

    /// The buckets, each once.
    pub(crate) fn buckets_mut(&mut self) -> impl Iterator<Item = &mut dyn Bucket> + '_ {
        self.buckets.iter_mut().map(|bucket| &mut **bucket)
    }

    /// The values the layout laid the layer `layer_id` out with.
    pub(crate) fn layer_mut(&mut self, layer_id: &str) -> Option<&mut Arc<dyn ErasedEvaluatedLayer>> {
        Some(&mut self.layers.get_mut(layer_id)?.layer)
    }

    /// A copy of the layers that `keep` keeps, and of their buckets.
    pub(crate) fn copy_where(&self, mut keep: impl FnMut(&Arc<dyn ErasedEvaluatedLayer>) -> bool) -> Self {
        let mut copy = LayoutBuckets::default();
        let mut copied: Vec<Option<usize>> = vec![None; self.buckets.len()];
        for (id, laid) in &self.layers {
            if !keep(&laid.layer) {
                continue;
            }
            let bucket = *copied[laid.bucket].get_or_insert_with(|| {
                copy.buckets.push(self.buckets[laid.bucket].clone_bucket());
                copy.buckets.len() - 1
            });
            copy.layers.insert(id.clone(), LaidLayer { bucket, layer: Arc::clone(&laid.layer) });
        }
        copy
    }
}

/// A layout: each layer's bucket, and the feature index.
pub(crate) struct LayoutResult {
    pub(crate) layer_render_data: LayoutBuckets,
    pub(crate) feature_index: Arc<FeatureIndex>,
}

impl LayoutResult {
    #[must_use]
    pub(crate) fn new(layer_render_data: LayoutBuckets, feature_index: Arc<FeatureIndex>) -> Self {
        LayoutResult { layer_render_data, feature_index }
    }

    /// The bucket of `layer_impl`'s layer, if it was laid out as a layer
    /// of the same type. A tile shows its contents until a relayout for a
    /// changed layer arrives, so a layer that changed type meanwhile finds
    /// nothing.
    #[inline]
    pub(crate) fn layer_bucket(&self, layer_impl: &dyn ErasedLayerStyle) -> Option<ErasedLayerBucket<'_>> {
        let result = self.layer_render_data.get(&layer_impl.base().id)?;
        if result.layer.style().info().name != layer_impl.info().name {
            return None;
        }
        Some(result)
    }

    /// Brings the buckets' feature state to `states`, for the features of
    /// the data they were built from; `pattern_positions` are where the
    /// layout's patterns are in its atlas. Returns the ids of the buckets
    /// whose vertex data changed.
    pub(crate) fn set_feature_state(
        &mut self,
        states: &LayerFeatureStates,
        pattern_positions: &ImagePositions,
    ) -> Vec<SimpleIdentity> {
        let mut changed = Vec::new();
        let Some(layers) = self.feature_index.get_data() else {
            return changed;
        };
        if states.is_empty() {
            return changed;
        }
        let LayoutBuckets { buckets, layers: laid } = &mut self.layer_render_data;
        for (layer_id, laid) in laid.iter() {
            let source_layer_id = &laid.layer.style().base().source_layer;
            // The worker got the layer from the same data.
            if let Ok(Some(source_layer)) = layers.layer(source_layer_id) {
                let Some(feature_states) = states.get(source_layer_id) else {
                    continue;
                };
                if feature_states.is_empty() {
                    continue;
                }
                let bucket = &mut buckets[laid.bucket];
                if bucket.has_data() && bucket.update(feature_states, &source_layer, layer_id, pattern_positions) {
                    changed.push(bucket.get_id());
                }
            }
        }
        changed
    }
}

/// The atlas pages a layout draws its glyphs and images from. Dropping it
/// gives their bins back.
pub(crate) struct TileAtlases {
    pub(crate) glyphs: AtlasLease,
    pub(crate) images: AtlasLease,
    /// Where the layout's patterns are, which drawing them and feature state
    /// read.
    pub(crate) pattern_positions: ImagePositions,
}

/// What a tile's dependencies resolved to, with the leases on the atlas
/// bins their positions name. `atlases` is `None` for a layout that waited
/// for nothing.
#[derive(Clone, Default)]
pub(crate) struct Answer {
    pub(crate) resolved: Arc<Resolved>,
    pub(crate) atlases: Option<Rc<TileAtlases>>,
    /// Whether it lacked glyphs of a range that failed to load.
    pub(crate) glyphs_failed: bool,
}

impl Answer {
    /// This answer cut down to what `deps` use: the positions and missing
    /// images they name, with leases on only those bins of the same pages.
    /// `None` if it holds nothing else.
    pub(crate) fn trimmed(&self, deps: &Dependencies, atlas: &mut DynamicTextureAtlas) -> Option<Answer> {
        let resolved = &*self.resolved;
        let atlases = self.atlases.as_ref()?;

        let mut shaping = HBShapeResults::new();
        for (font_stack, types) in &deps.glyphs.shapes {
            let Some(shaped) = resolved.shaping.get(font_stack) else {
                continue;
            };
            for (type_, texts) in types {
                let Some(shaped) = shaped.get(type_) else {
                    continue;
                };
                for text in texts {
                    if let Some(result) = shaped.get(text) {
                        let kept = shaping.entry(font_stack.clone()).or_default().entry(*type_).or_default();
                        kept.insert(text.clone(), result.clone());
                    }
                }
            }
        }
        // The glyphs the layout names, and those its shaped text came out as.
        let mut wanted: VecMap<u64, BTreeSet<GlyphID>> = VecMap::new();
        for (font_stack, ids) in &deps.glyphs.glyphs {
            wanted.entry(font_stack_hash(font_stack)).or_default().extend(ids.iter().copied());
        }
        for (font_stack, types) in &shaping {
            let ids = wanted.entry(font_stack_hash(font_stack)).or_default();
            for (type_, texts) in types {
                ids.extend(
                    texts
                        .values()
                        .flat_map(|result| result.str_.iter().map(|code| GlyphID { code: *code, type_: *type_ })),
                );
            }
        }
        let mut glyphs = GlyphPositions::new();
        for (font_stack, ids) in &wanted {
            let Some(positions) = resolved.glyphs.get(font_stack) else {
                continue;
            };
            let kept = glyphs.entry(*font_stack).or_default();
            kept.extend(ids.iter().filter_map(|id| Some((*id, *positions.get(id)?))));
        }
        let images_of = |positions: &ImagePositions, type_: ImageType| -> ImagePositions {
            positions
                .iter()
                .filter(|(id, _)| deps.images.get(*id) == Some(&type_))
                .map(|(id, position)| (id.clone(), position.clone()))
                .collect()
        };
        let icons = images_of(&resolved.icons, ImageType::Icon);
        let patterns = images_of(&resolved.patterns, ImageType::Pattern);
        let missing_images: VecSet<_> =
            resolved.missing_images.iter().filter(|id| deps.images.contains_key(*id)).cloned().collect();

        let glyph_count = |positions: &GlyphPositions| positions.values().map(BTreeMap::len).sum::<usize>();
        let shaping_count = |shaping: &HBShapeResults| {
            shaping.values().flat_map(|types| types.values()).map(BTreeMap::len).sum::<usize>()
        };
        if glyph_count(&glyphs) == glyph_count(&resolved.glyphs)
            && shaping_count(&shaping) == shaping_count(&resolved.shaping)
            && icons.len() == resolved.icons.len()
            && patterns.len() == resolved.patterns.len()
            && missing_images.len() == resolved.missing_images.len()
        {
            return None;
        }

        let glyph_lease = atlas.retain(&atlases.glyphs, |bin| match bin {
            BinUse::Glyph(font_stack, id) => glyphs.get(font_stack).is_some_and(|positions| positions.contains_key(id)),
            BinUse::Image(..) => false,
            BinUse::Dummy => true,
        });
        let image_lease = atlas.retain(&atlases.images, |bin| match bin {
            BinUse::Image(id, ImageType::Icon) => icons.contains_key(id),
            BinUse::Image(id, ImageType::Pattern) => patterns.contains_key(id),
            BinUse::Glyph(..) => false,
            BinUse::Dummy => true,
        });
        let resolved = Resolved { glyphs, shaping, icons, patterns: patterns.clone(), missing_images };
        let atlases = TileAtlases { glyphs: glyph_lease, images: image_lease, pattern_positions: patterns };
        Some(Answer {
            resolved: Arc::new(resolved),
            atlases: Some(Rc::new(atlases)),
            glyphs_failed: self.glyphs_failed,
        })
    }
}

/// What a geometry tile draws: a layout of one load of its data.
pub(crate) struct GeometryContents {
    /// The buckets, and the feature index, which holds the data the layout
    /// was built from.
    pub(crate) result: LayoutResult,
    /// The tileset has no tile here: the tile lays out without data but
    /// isn't renderable, so a tile of another zoom level draws in its place.
    pub(crate) missing: bool,
    /// What its glyphs and images resolved to, with the leases on their bins.
    pub(crate) answer: Answer,
    /// What the layout used and asked for.
    pub(crate) deps: Rc<Dependencies>,
    pub(crate) textures: TileAtlasTextures,
    /// The geometry the buckets' uploads moved into the context's stores.
    pub(crate) bucket_geometry: BucketGeometry,
}

impl GeometryContents {
    pub(crate) fn new(result: LayoutResult, missing: bool, answer: Answer, deps: Dependencies) -> Self {
        let textures = answer.atlases.as_ref().map_or_else(TileAtlasTextures::default, |atlases| {
            TileAtlasTextures::new(atlases.glyphs.page(), atlases.images.page())
        });
        GeometryContents {
            result,
            missing,
            answer,
            deps: Rc::new(deps),
            textures,
            bucket_geometry: BucketGeometry::default(),
        }
    }

    pub(crate) fn pattern_positions(&self) -> Option<&ImagePositions> {
        self.answer.atlases.as_ref().map(|atlases| &atlases.pattern_positions)
    }

    /// Readies the buckets that are new or changed since their last upload
    /// for the tile's draws ([`Bucket::upload`]), moving their geometry into
    /// `context`'s stores.
    pub(crate) fn upload(&mut self, context: &mut Context, needs: UploadNeeds) {
        let mut upload = GeometryUpload::new(&mut self.bucket_geometry, context);
        for bucket in self.result.layer_render_data.buckets_mut() {
            if bucket.needs_upload() {
                bucket.upload(&mut upload, needs);
            }
        }
    }

    /// Brings the buckets' feature state to `states`, for the features of
    /// the data they were built from, and adds the buckets whose vertex data
    /// that changed to `changed`.
    pub(crate) fn set_feature_state(&mut self, states: &LayerFeatureStates, changed: &mut ChangedBuckets) {
        let no_patterns = ImagePositions::default();
        let pattern_positions = self.answer.atlases.as_ref().map_or(&no_patterns, |atlases| &atlases.pattern_positions);
        for id in self.result.set_feature_state(states, pattern_positions) {
            changed.mark(id, BucketChanges { paint: true, ..BucketChanges::default() });
        }
    }

    /// The largest query radius of `layers`' buckets.
    pub(crate) fn query_padding(&self, layers: &QueryLayers<'_>) -> f32 {
        let mut query_padding: f32 = 0.0;
        for layer in layers.values() {
            let Some(data) = self.result.layer_bucket(&*layer.base().style) else {
                continue;
            };
            if data.bucket.has_data() {
                let evaluated = &layer.base().evaluated_properties;
                query_padding = query_padding.max(evaluated.entry().query_radius(data.bucket, evaluated));
            }
        }
        query_padding
    }

    /// Adds the rendered features of tile `id` in `query_geometry` to
    /// `result`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the results, tile, query geometry, camera, layers, options, global and feature state and projection of a feature query"
    )]
    pub(crate) fn query_rendered_features(
        &self,
        id: &OverscaledTileID,
        result: &mut QueryResults,
        query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        transform_state: &mln_map_types::TransformState,
        layers: &QueryLayers<'_>,
        options: &RenderedQueryOptions,
        global_state: Option<&GlobalStateMap>,
        proj_matrix: &Mat4,
        feature_state: &SourceFeatureState,
    ) -> Result<(), Error> {
        if self.result.feature_index.get_data().is_none() {
            return Ok(());
        }
        let query_padding = self.query_padding(layers);
        let projector = mln_map_types::TileProjector::with_data(
            transform_state,
            &id.to_unwrapped(),
            transform_state.get_projection_data_with(&id.to_unwrapped(), proj_matrix),
        );
        self.result.feature_index.query(
            result,
            query_geometry,
            transform_state,
            &projector,
            mln_util::constants::TILE_SIZE_D * f64::from(id.overscale_factor()),
            2f64.powf(transform_state.get_zoom() - f64::from(id.overscaled_z)),
            options,
            global_state,
            id,
            layers,
            query_padding * transform_state.max_pitch_scale_factor(),
            feature_state,
        )
    }

    /// Adds the features of tile `id` that `options` select to `result`:
    /// those of the source layers it names, or with `single_layer`, those
    /// of the data's one layer whatever it names.
    pub(crate) fn query_source_features(
        &self,
        id: &OverscaledTileID,
        single_layer: bool,
        result: &mut Vec<Feature>,
        options: &SourceQueryOptions,
        global_state: Option<&GlobalStateMap>,
    ) -> Result<(), Error> {
        let Some(data) = self.result.feature_index.get_data() else {
            return Ok(());
        };
        if single_layer {
            // Ignore the sourceLayer, there is only one
            if let Some(layer) = data.layer("")? {
                query_layer_source_features(&layer, result, options, global_state, id)?;
            }
            return Ok(());
        }
        // No source layers, specified, nothing to do
        let Some(source_layers) = &options.source_layers else {
            Log::warning(Event::General, "At least one sourceLayer required");
            return Ok(());
        };
        for source_layer in source_layers {
            // Go through all sourceLayers, if any
            // to gather all the features
            if let Some(layer) = data.layer(source_layer)? {
                let start = result.len();
                query_layer_source_features(&layer, result, options, global_state, id)?;
                for feature in &mut result[start..] {
                    feature.source_layer.clone_from(source_layer);
                }
            }
        }
        Ok(())
    }
}

/// What a tile draws.
pub(crate) enum Contents {
    Geometry(GeometryContents),
    /// A raster tile's image.
    Raster(RasterBucket),
    /// A raster-dem tile's DEM.
    Dem(HillshadeBucket),
    /// A raster tile without data: it draws nothing in its place.
    Empty,
}

impl Contents {
    /// Whether the tile draws these contents, rather than a tile of another
    /// zoom level in their place.
    pub(crate) fn is_renderable(&self) -> bool {
        match self {
            Contents::Geometry(contents) => !contents.missing,
            Contents::Raster(_) | Contents::Dem(_) | Contents::Empty => true,
        }
    }

    pub(crate) fn geometry(&self) -> Option<&GeometryContents> {
        match self {
            Contents::Geometry(contents) => Some(contents),
            Contents::Raster(_) | Contents::Dem(_) | Contents::Empty => None,
        }
    }

    pub(crate) fn geometry_mut(&mut self) -> Option<&mut GeometryContents> {
        match self {
            Contents::Geometry(contents) => Some(contents),
            Contents::Raster(_) | Contents::Dem(_) | Contents::Empty => None,
        }
    }

    /// The bucket of a raster or raster-dem tile, which all its layers draw.
    pub(crate) fn shared_bucket(&self) -> Option<&dyn Bucket> {
        match self {
            Contents::Raster(bucket) => Some(bucket),
            Contents::Dem(bucket) => Some(bucket),
            Contents::Geometry(_) | Contents::Empty => None,
        }
    }

    pub(crate) fn set_mask(&mut self, mask: TileMask) {
        match self {
            Contents::Raster(bucket) => bucket.set_mask(mask),
            Contents::Dem(bucket) => bucket.set_mask(mask),
            Contents::Geometry(_) | Contents::Empty => {}
        }
    }

    /// The bucket of `layer_impl`'s layer: a raster or raster-dem tile's
    /// for any layer.
    pub(crate) fn bucket(&self, layer_impl: &dyn ErasedLayerStyle) -> Option<&dyn Bucket> {
        match self {
            Contents::Geometry(contents) => contents.result.layer_bucket(layer_impl).map(|data| data.bucket),
            Contents::Raster(_) | Contents::Dem(_) => self.shared_bucket(),
            Contents::Empty => None,
        }
    }

    /// Readies the buckets that are new or changed since their last upload
    /// for the frame's draws ([`Bucket::upload`]), moving their geometry
    /// into `context`'s stores.
    pub(crate) fn upload(&mut self, context: &mut Context, needs: UploadNeeds) {
        match self {
            Contents::Geometry(contents) => contents.upload(context, needs),
            Contents::Raster(bucket) => bucket.upload_image(),
            Contents::Dem(bucket) => bucket.upload_dem(),
            Contents::Empty => {}
        }
    }

    /// Whether a layer with `layer_properties` draws from these contents:
    /// for a geometry tile, whether its layout has the layer, whose render
    /// data then takes the new properties if only their values changed.
    pub(crate) fn layer_properties_updated(&mut self, layer_properties: &Arc<dyn ErasedEvaluatedLayer>) -> bool {
        let Contents::Geometry(contents) = self else {
            return !matches!(self, Contents::Empty);
        };
        let layer_impl = layer_properties.style();
        let Some(layer) = contents.result.layer_render_data.layer_mut(&layer_impl.base().id) else {
            return false;
        };
        // A layer that changed type meanwhile isn't laid out (see
        // `layer_bucket`).
        if layer.style().info().name != layer_impl.info().name {
            return false;
        }
        if !Arc::ptr_eq(layer, layer_properties) && layer.constants_mask() == layer_properties.constants_mask() {
            *layer = Arc::clone(layer_properties);
        }
        true
    }
}

/// Lets go of a tile's contents. A layout's buckets and feature index are
/// freed on `pool`, the renderer's workers, since a view that drops many
/// tiles would otherwise spend frames freeing them; the atlas leases drop
/// here, on the render thread.
pub(crate) fn release(contents: Contents, pool: &TaskGroup) {
    if let Contents::Geometry(GeometryContents {
        result: LayoutResult { layer_render_data, feature_index },
        answer,
        ..
    }) = contents
    {
        drop_on_workers(pool, (layer_render_data, feature_index));
        drop(answer);
    }
}

/// The feature loop of a tile's source query.
pub(crate) fn query_layer_source_features(
    layer: &mln_tile::TileLayer<'_>,
    result: &mut Vec<Feature>,
    options: &SourceQueryOptions,
    global_state: Option<&GlobalStateMap>,
    id: &OverscaledTileID,
) -> Result<(), Error> {
    for i in 0..layer.len() {
        let feature = layer.feature(i)?;

        // Apply filter, if any
        if let Some(filter) = &options.filter {
            let context =
                mln_style::expression::EvaluationContext::with_zoom_feature(f32::from(id.overscaled_z), Some(&feature))
                    .with_global_state(global_state);
            if !filter.evaluate(&context) {
                continue;
            }
        }

        let converted = mln_tile::convert_feature(&feature, &feature.geometry()?, &id.canonical)?;
        result.push(Feature::from(converted));
    }
    Ok(())
}
