use std::sync::Arc;

use mln_render::layer::{BuildCx, EvaluatedLayer, FeaturePatterns, Layout, LayoutCx};
use mln_util::error::Error;

use super::FillExtrusion;
use super::bucket::{FillExtrusionBucket, WallQuads, tile_units_per_meter};
use super::tessellator::Tessellator;

/// The features a tile's extrusions draw, by index, each with its patterns,
/// and how far their corners round off, in meters.
pub struct FillExtrusionLayout {
    features: Vec<(usize, FeaturePatterns)>,
    corner_distance: f32,
}

impl Layout<FillExtrusion> for FillExtrusionLayout {
    type Bucket = FillExtrusionBucket;

    /// Whether a layer of the group has a pattern, whose images it waits
    /// for.
    fn needs_resources(group: &[Arc<EvaluatedLayer<FillExtrusion>>]) -> bool {
        group.iter().any(|layer| {
            let pattern = &layer.paint.fill_extrusion_pattern;
            !pattern.is_constant() || !pattern.faded().to.id().is_empty()
        })
    }

    fn new(cx: &mut LayoutCx<'_, FillExtrusion>) -> Result<Self, Error> {
        let layout = cx.layers()[0].style.layout.evaluate(cx.parameters());
        let corner_distance = *layout.fill_extrusion_rounded_corner_distance.uniform();
        let selected = cx.selected()?;
        let patterns = cx.need_patterns(|paint| &paint.fill_extrusion_pattern, &selected)?;
        Ok(FillExtrusionLayout { features: selected.into_iter().zip(patterns).collect(), corner_distance })
    }

    fn build(self, cx: &mut BuildCx<'_, FillExtrusion>) -> Result<Option<FillExtrusionBucket>, Error> {
        let tile = cx.tile();
        let granularity = cx.subdivision_granularity().fill.granularity_for_zoom_level(tile.z);
        let corner_distance = f64::from(self.corner_distance) * tile_units_per_meter(&tile);
        let wall_quads = cx.cpu_rasterizer().then(|| WallQuads { binders: cx.binders(), ..WallQuads::default() });
        let mut bucket = FillExtrusionBucket { binders: cx.binders(), wall_quads, ..FillExtrusionBucket::default() };
        let mut tessellator = Tessellator::default();
        for (i, patterns) in &self.features {
            let feature = cx.feature(*i)?;
            let geometry = feature.geometry()?;
            bucket.add(&geometry, &tile, granularity, corner_distance, &mut tessellator);
            let vertices = bucket.vertices.len();
            cx.populate_patterns(&mut bucket.binders, *i, &feature, vertices, patterns);
            if let Some(quads) = &mut bucket.wall_quads {
                cx.populate_patterns(&mut quads.binders, *i, &feature, 4 * vertices, patterns);
            }
            cx.index(*i, &geometry);
        }
        if bucket.triangles.is_empty() {
            return Ok(None);
        }
        if let Some(quads) = &mut bucket.wall_quads {
            *quads.vertices.get_mut() = FillExtrusionBucket::wall_quad_vertices(bucket.vertices.get());
        }
        Ok(Some(bucket))
    }
}
