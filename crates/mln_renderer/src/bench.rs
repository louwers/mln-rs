//! Entry points to the renderer's internal stages, as functions over data,
//! for the benchmarks (`tools/bench`) to measure. Built only with the
//! `bench` feature, and hidden from the documentation: they aren't API.
//!
//! Each entry point takes and returns public types only, so that the
//! stages' internals can change without changing what the benchmarks call.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

use mln_map_types::MapMode;
use mln_storage::Resource;
use mln_style::Style;
use mln_style::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use mln_style_types::VisibilityType;
use mln_style_types::image::{ImageMap, ImageType};
use mln_text::glyph::{GlyphMap, get_glyph_range};
use mln_text::glyph_pbf::parse_glyph_pbf;
use mln_text::tagged_string::font_stack_hash;
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_vec_map::VecSet;

use crate::Registry;
use crate::gfx::dynamic_texture_atlas::DynamicTextureAtlas;
use crate::layer_type::ErasedEvaluatedLayer;
use crate::layout::tile_layout::{Dependencies, LayoutParams, Resolved};
use crate::text::glyph_store::HBShapeResults;
use crate::tile::contents::LayoutResult;
use crate::tile::job::{DataInput, DataKey, Job, JobId, JobResult, LayoutJob, Run, SourceKey, SourceStyle, TileKey};
use crate::tile::tile_workers::InlineWorkers;

/// A vector tile's layout job, as a tile worker runs it once the tile has
/// every glyph and image it needs: the selection of each layer's features,
/// and the buckets and feature index built from them.
pub struct TileLayout {
    id: OverscaledTileID,
    style: Arc<SourceStyle>,
    available_images: Arc<VecSet<String>>,
    params: LayoutParams,
    resolved: Arc<Resolved>,
}

impl TileLayout {
    /// The layout of tile `z`/`x`/`y` of `source` in `style`, by those of
    /// the style's visible layers on `source` whose type `include` accepts,
    /// evaluated at the tile's zoom as the renderer evaluates them, for a
    /// continuous map at pixel ratio 1.
    ///
    /// It lays `data` out once to find what the layout needs, and resolves
    /// that as the renderer would: glyph ranges from the style's glyph URL,
    /// whose bytes `load` gives, and images from the style's images, packed
    /// into an atlas. Fails for a style without the source, a layer the
    /// renderer has no type for, or a layout that fails.
    pub fn new(
        style: &Style,
        source: &str,
        include: impl Fn(&str) -> bool,
        (z, x, y): (u8, u32, u32),
        data: &TileData,
        load: impl Fn(&str) -> Option<Vec<u8>>,
    ) -> Result<TileLayout, Error> {
        if style.get_source(source).is_none() {
            return Err(Error::invalid_argument(format!("the style has no source {source}")));
        }
        let registry = Registry::with_builtins();
        let parameters = PropertyEvaluationParameters::new(f32::from(z));
        let mut layers: Vec<Arc<dyn ErasedEvaluatedLayer>> = Vec::new();
        for layer in style.get_layers() {
            let base = layer.style().base();
            if base.source != source || base.visibility == VisibilityType::None || !include(layer.info().name) {
                continue;
            }
            let mut render_layer = registry
                .layer_type(layer.info().name)
                .and_then(|layer_type| layer_type.create_render_layer(layer.style()))
                .ok_or_else(|| Error::invalid_argument(format!("no render layer for {}", layer.get_id())))?;
            let render_layer =
                Rc::get_mut(&mut render_layer).ok_or_else(|| Error::native("a new render layer is shared"))?.get_mut();
            render_layer.evaluate(&parameters);
            layers.push(Arc::clone(&render_layer.base().evaluated_properties));
        }

        let mut layout = TileLayout {
            id: OverscaledTileID::from_zxy(z, x, y),
            style: Arc::new(SourceStyle {
                version: 0,
                layers,
                global_state: Some(Arc::clone(style.get_global_state())),
                font_faces: style.get_font_faces().cloned(),
            }),
            available_images: Arc::new(style.get_images().iter().map(|image| image.id.clone()).collect()),
            params: LayoutParams {
                mode: MapMode::Continuous,
                pixel_ratio: 1.0,
                show_collision_boxes: false,
                subdivision_granularity: style.get_projection().get_subdivision_granularity(),
                first_load: true,
                capture_rendered_features: false,
            },
            resolved: Arc::default(),
        };
        match layout.job(data.clone()) {
            JobResult::NeedsDeps(dependencies) => layout.resolved = Arc::new(resolve(&dependencies, style, &load)?),
            JobResult::Laid(_) => {}
            JobResult::Failed(error) => return Err(error),
            JobResult::NeedsData | JobResult::Raster(_) | JobResult::Dem(_) => {
                return Err(Error::native("a layout job answered as no layout does"));
            }
        }
        Ok(layout)
    }

    /// Lays `data` out, as the job that a tile worker runs once its glyphs
    /// and images are resolved: the result holds the tile's buckets.
    pub fn run(&self, data: TileData) -> Result<LaidOut, Error> {
        match self.job(data) {
            JobResult::Laid(output) => Ok(LaidOut(output.result)),
            JobResult::Failed(error) => Err(error),
            JobResult::NeedsDeps(_) | JobResult::NeedsData | JobResult::Raster(_) | JobResult::Dem(_) => {
                Err(Error::native("the layout needs what it didn't need before"))
            }
        }
    }

    fn job(&self, data: TileData) -> JobResult {
        let tile = TileKey { source: SourceKey(1), id: self.id };
        let key = DataKey { source: SourceKey(1), tile: self.id.canonical, load: 1 };
        let job = LayoutJob {
            style: Arc::clone(&self.style),
            available_images: Arc::clone(&self.available_images),
            data: DataInput::Full { key, data: Some(data) },
            params: self.params,
            resolved: Arc::clone(&self.resolved),
        };
        let run = Run { id: JobId { tile, generation: 1 }, job: Job::Layout(job), superseded: Arc::default() };
        InlineWorkers::run(run).result
    }
}

/// A tile's buckets and feature index, as its layout made them.
pub struct LaidOut(LayoutResult);

impl LaidOut {
    /// The number of layers that got a bucket.
    #[must_use]
    pub fn buckets(&self) -> usize {
        self.0.layer_render_data.len()
    }
}

/// What `dependencies` resolve to: the glyphs of the ranges that `load`
/// gives for the style's glyph URL, and the style's images, packed into an
/// atlas as the renderer packs them.
fn resolve(
    dependencies: &Dependencies,
    style: &Style,
    load: &impl Fn(&str) -> Option<Vec<u8>>,
) -> Result<Resolved, Error> {
    let mut glyphs = GlyphMap::new();
    for (font_stack, ids) in &dependencies.glyphs.glyphs {
        let font_glyphs = glyphs.entry(font_stack_hash(font_stack)).or_default();
        let ranges: BTreeSet<_> = ids.iter().map(|id| get_glyph_range(*id)).collect();
        for range in ranges {
            let url = Resource::glyphs(style.get_glyph_url(), font_stack, (range.first, range.second)).url;
            let Some(bytes) = load(&url) else { continue };
            let parsed = parse_glyph_pbf(&range, &bytes).map_err(|error| Error::native(format!("{url}: {error}")))?;
            for glyph in parsed {
                font_glyphs.insert(glyph.id, Some(Arc::new(glyph)));
            }
        }
        for id in ids {
            font_glyphs.entry(*id).or_insert(None);
        }
    }

    let (mut icons, mut patterns) = (ImageMap::new(), ImageMap::new());
    for (id, type_) in &dependencies.images {
        if let Some(image) = style.get_image(id) {
            let map = match type_ {
                ImageType::Icon => &mut icons,
                ImageType::Pattern => &mut patterns,
            };
            map.insert(id.clone(), Arc::clone(image));
        }
    }
    let missing_images = dependencies
        .images
        .keys()
        .filter(|id| !icons.contains_key(*id) && !patterns.contains_key(*id))
        .cloned()
        .collect();

    let mut atlas = DynamicTextureAtlas::new();
    let glyph_page = atlas.pack_glyphs(&glyphs);
    let image_page = atlas.pack_images(&icons, &patterns);
    Ok(Resolved {
        glyphs: glyph_page.positions,
        shaping: HBShapeResults::new(),
        icons: image_page.icons,
        patterns: image_page.patterns,
        missing_images,
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use feature::{Feature, Identifier, PropertyMap, Value};
    use geo_types::{Geometry, LineString, Point};

    use super::*;

    fn feature(geometry: Geometry<i16>) -> Feature<i16, Arc<PropertyMap>> {
        let mut properties = PropertyMap::new();
        properties.insert("name".to_owned(), Value::String("ab".to_owned()));
        Feature { geometry: Some(geometry), properties: Arc::new(properties), id: Identifier::Null }
    }

    #[test]
    fn lays_out_with_the_glyphs_it_loads() {
        let style = Style::from_json(
            Registry::with_builtins().style(),
            r#"{
                "version": 8,
                "glyphs": "local://glyphs/{fontstack}/{range}.pbf",
                "sources": {"s": {"type": "vector", "tiles": ["local://tiles/{z}-{x}-{y}.mvt"]}},
                "layers": [
                    {"id": "roads", "type": "line", "source": "s", "source-layer": "l"},
                    {"id": "names", "type": "symbol", "source": "s", "source-layer": "l",
                     "layout": {"text-field": ["get", "name"], "text-font": ["Test"]}}
                ]
            }"#,
            None,
        )
        .unwrap();
        let data = TileData::Features(Arc::new(vec![
            feature(Geometry::LineString(LineString::from(vec![(100, 100), (4000, 4000)]))),
            feature(Geometry::Point(Point::new(2000, 1000))),
        ]));
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/test/fixtures/resources/glyphs.pbf");
        let glyphs = std::fs::read(path).unwrap();
        let requested = RefCell::new(Vec::new());
        let load = |url: &str| {
            requested.borrow_mut().push(url.to_owned());
            Some(glyphs.clone())
        };

        let layout = TileLayout::new(&style, "s", |_| true, (0, 0, 0), &data, load).unwrap();
        assert_eq!(*requested.borrow(), ["local://glyphs/Test/0-255.pbf"]);
        assert_eq!(layout.run(data.clone()).unwrap().buckets(), 2);

        let lines = TileLayout::new(&style, "s", |type_| type_ == "line", (0, 0, 0), &data, |_| None).unwrap();
        assert_eq!(lines.run(data).unwrap().buckets(), 1);
    }
}
