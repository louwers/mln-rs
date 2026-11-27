//! Entry points to the built-in layer types' stages, as functions over
//! data, for the benchmarks (`tools/bench`) to measure, beside the engine's
//! (`mln_engine::bench`). Built only with the `bench` feature, and hidden
//! from the documentation: they aren't API.

use std::sync::Arc;

use mln_engine::UpdateParameters;
use mln_engine::bench::{HeldBuckets, LaidOut, place_alone, placing_layers, render_tiles};
use mln_engine::placement::{LaidBuckets, PlacingLayer};
use mln_style::error::Error;
use mln_style::layer::LayerStyle;
use mln_style::layers::Symbol;
use mln_style::property::evaluation::PropertyEvaluationParameters;
use mln_style::types::VisibilityType;

use crate::layers::symbol::bucket::SymbolBucket;
use crate::layers::symbol::layer::RenderSymbolLayer;
use crate::layers::symbol::placement::cross_tile_index::CrossTileSymbolIndex;
use crate::layers::symbol::placement::{PlacedTile, SymbolLayer, SymbolPlacement, symbol_translates};

/// The symbols of laid-out tiles in a view, as a renderer finds which to
/// show: the buckets its symbol layers place and draw, each tile's matrices
/// for the view, and the view's update.
pub struct SymbolView {
    update: Arc<UpdateParameters>,
    /// The style's symbol layers that lead or draw a bucket of the tiles,
    /// in the style's order.
    layers: Vec<SymbolLayer>,
    /// Those that lead a bucket, as placement gets them.
    placing: Vec<PlacingLayer>,
    /// The participant that places them.
    placement: SymbolPlacement,
    /// The keys of the buckets `HeldBuckets` holds for the view.
    held: Vec<mln_engine::placement::BucketKey>,
}

impl Drop for SymbolView {
    fn drop(&mut self) {
        for key in &self.held {
            HeldBuckets::release(key);
        }
    }
}

impl SymbolView {
    /// The view that `update` shows of `tiles`, laid out by the visible
    /// symbol layers on `source` in `update`'s style. Fails for a layer the
    /// renderer has no type for.
    ///
    /// The harness makes the update, since its fields are what a renderer
    /// takes from a map: a comparison builds this module against libraries
    /// whose updates have other fields.
    pub fn new(source: &str, tiles: &[&LaidOut], update: UpdateParameters) -> Result<SymbolView, Error> {
        let render_tiles = render_tiles(tiles.iter().map(|laid_out| laid_out.id()), &update.view);

        let parameters = PropertyEvaluationParameters::new(update.view.zoom() as f32);
        let mut layers = Vec::new();
        let mut held = Vec::new();
        for layer in update.style.get_layers() {
            let base = layer.style().base();
            if base.source != source || base.visibility == VisibilityType::None || layer.info().name != "symbol" {
                continue;
            }
            let style = LayerStyle::<Symbol>::recover(layer.style())
                .ok_or_else(|| Error::invalid_argument(format!("no render layer for {}", layer.get_id())))?;
            // As the participant collects the layer's tiles' buckets.
            let values = mln_engine::bench::evaluate::<RenderSymbolLayer>(style, &parameters);
            let translates = symbol_translates(&values);
            let mut symbol_layer = SymbolLayer::new(layers.len(), Arc::clone(&values.style));
            let mut buckets = HeldBuckets::take();
            for (laid_out, tile) in tiles.iter().zip(&render_tiles) {
                let Some(key) = buckets.hold::<SymbolBucket>(laid_out, layer.get_id(), tile) else { continue };
                held.push(key);
                let laid: &dyn LaidBuckets = &buckets;
                let Some(bucket) = laid.get::<SymbolBucket>(&key) else { continue };
                let feature_index = || Some(Arc::clone(laid_out.feature_index()));
                symbol_layer.add_bucket(&values, &PlacedTile::new(tile), key, bucket, feature_index, translates);
            }
            buckets.restore();
            if !symbol_layer.is_empty() {
                layers.push(symbol_layer);
            }
        }

        let placing: Vec<_> = layers.iter().filter(|layer| layer.places()).map(SymbolLayer::placing).collect();
        let placing = placing_layers(&placing, update.cross_source_collisions);
        let placement = SymbolPlacement::with_layers(layers.clone());
        Ok(SymbolView { update: Arc::new(update), layers, placing, placement, held })
    }

    /// Adds the view's symbol buckets to `index`, as a renderer does before
    /// it places them: each symbol takes the id of the same symbol in a
    /// tile of another zoom that `index` holds, or a new one, and `index`
    /// drops the buckets of each layer that the view no longer has. Returns
    /// how many layers added a bucket.
    pub fn index(&self, index: &mut SymbolIndex) -> Result<usize, Error> {
        let longitude = self.update.view.center(mln_style::geo::WrapMode::Unwrapped).longitude() as f32;
        let mut added = 0;
        let mut buckets = HeldBuckets::take();
        for layer in self.layers.iter().rev() {
            let result = layer.index(&mut index.0, longitude, &self.update.view, &mut buckets.placement());
            if result.buckets_added {
                added += 1;
            }
        }
        buckets.restore();
        Ok(added)
    }

    /// Places the view's symbols, topmost layer first, each where it
    /// collides with none placed before it, after a placement that showed
    /// none: what a renderer does when the view changes. Returns how many
    /// symbols it shows. Their ids come from [`SymbolView::index`]: a symbol
    /// without one shows nowhere.
    #[must_use]
    pub fn place(&mut self) -> usize {
        let mut buckets = HeldBuckets::take();
        self.placement.prepare_alone(&self.placing, &buckets);
        place_alone(&mut self.placement, &self.placing, &self.update, &mut buckets);
        buckets.restore();
        self.placement.shown()
    }
}

/// The renderer's cross-tile symbol index: the symbols of the tiles that
/// it shows, by layer, with the ids that keep a symbol the same across
/// tiles of different zooms.
#[derive(Default)]
pub struct SymbolIndex(CrossTileSymbolIndex);

#[cfg(test)]
mod tests {
    use geo_types::{Geometry, LineString, Point};
    use mln_engine::bench::{Codec, TileLayout};
    use mln_engine::camera::TransformState;
    use mln_engine::map::{MapMode, TerrainOptions};
    use mln_engine::tile::TileData;
    use mln_engine::tile_store::TileLodMode;
    use mln_style::Style;
    use mln_style::bitmap::PremultipliedImage;
    use mln_style::feature::{Feature, Identifier, PropertyMap, Value};
    use mln_style::geo::LatLng;
    use mln_style::size::Size;
    use mln_style::time::Instant;

    use super::*;

    fn feature(id: u64, geometry: Geometry<i16>) -> Feature<i16, Arc<PropertyMap>> {
        let mut properties = PropertyMap::new();
        properties.insert("name".to_owned(), Value::String("ab".to_owned()));
        Feature { geometry: Some(geometry), properties: Arc::new(properties), id: Identifier::Uint(id) }
    }

    /// A style of a symbol layer of text and icons over source `s`, with
    /// the icon's image.
    fn style() -> Style {
        let mut style = Style::from_json(
            crate::registry().style(),
            r#"{
                "version": 8,
                "glyphs": "local://glyphs/{fontstack}/{range}.pbf",
                "sources": {"s": {"type": "vector", "tiles": ["local://tiles/{z}-{x}-{y}.mvt"]}},
                "layers": [
                    {"id": "names", "type": "symbol", "source": "s", "source-layer": "l",
                     "layout": {"text-field": ["get", "name"], "text-font": ["Test"], "icon-image": "marker",
                         "icon-allow-overlap": true, "text-allow-overlap": true}}
                ]
            }"#,
        )
        .unwrap();
        let marker = PremultipliedImage::new(Size::new(16, 16));
        let marker = mln_style::image::Impl::new(
            "marker".to_owned(),
            marker,
            1.0,
            false,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
        );
        style.apply(mln_style::Edit::AddImage(Arc::new(marker.unwrap()))).unwrap();
        style
    }

    /// The update of a continuous map of `style` at pixel ratio 1, of
    /// `width` × `height` pixels centered on 0, 0 at `zoom`.
    fn update(style: &Style, zoom: f64, (width, height): (u32, u32)) -> UpdateParameters {
        let mut state = TransformState::default();
        state.set_size(Size::new(width, height));
        state.set_lat_lng_zoom(&LatLng::new(0.0, 0.0).unwrap(), zoom);
        UpdateParameters {
            style_loaded: true,
            mode: MapMode::Continuous,
            pixel_ratio: 1.0,
            time_point: Some(Instant::now()),
            view: state.view().clone(),
            min_zoom: state.min_zoom(),
            max_zoom: state.limits().max_zoom,
            camera_moving: false,
            style: Arc::new(style.clone()),
            font_faces: None,
            sprite_loaded: true,
            feature_states: Arc::default(),
            file_source: None,
            prefetch_zoom_delta: 0,
            still_image_request: false,
            still_image_request_id: 0,
            update_id: 0,
            cross_source_collisions: true,
            fast_pfor_enabled: false,
            terrain: TerrainOptions::default(),
            elevation: None,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: mln_style::math::deg2rad(60.0),
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: style.get_projection().get_subdivision_granularity(),
        }
    }

    /// A line and a point, both named.
    fn data() -> TileData {
        TileData::Features(Arc::new(vec![
            feature(1, Geometry::LineString(LineString::from(vec![(100, 100), (4000, 4000)]))),
            feature(2, Geometry::Point(Point::new(2000, 1000))),
        ]))
    }

    fn glyphs() -> Vec<u8> {
        let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("testdata/test/fixtures/resources/glyphs.pbf");
        std::fs::read(path).unwrap()
    }

    #[test]
    fn places_the_symbols_it_indexes() {
        let (style, data, glyphs) = (style(), data(), glyphs());
        let symbols = |zoom| {
            let layout = TileLayout::overscaled(
                &crate::registry(),
                &style,
                "s",
                |type_| type_ == "symbol",
                (0, 0, 0),
                zoom,
                &data,
                |_| Some(glyphs.clone()),
            )
            .unwrap();
            layout.run(data.clone()).unwrap()
        };
        let (before, after) = (symbols(0), symbols(1));
        let zoomed_out = SymbolView::new("s", &[&before], update(&style, 0.0, (512, 512))).unwrap();
        let mut zoomed_in = SymbolView::new("s", &[&after], update(&style, 1.0, (1024, 1024))).unwrap();

        // Unindexed symbols show nowhere.
        assert_eq!(zoomed_in.place(), 0);
        let mut index = SymbolIndex::default();
        assert_eq!(zoomed_out.index(&mut index).unwrap(), 1);
        assert_eq!(zoomed_in.index(&mut index).unwrap(), 1);
        // Indexing a view again adds nothing.
        assert_eq!(zoomed_in.index(&mut index).unwrap(), 0);
        assert_eq!(zoomed_in.place(), 2);
    }

    /// A symbol layout crosses the worker codec with its text and its icons.
    #[test]
    fn symbol_buckets_cross_the_codec() {
        let (style, data, glyphs) = (style(), data(), glyphs());
        let registry = crate::registry();
        let layout = TileLayout::new(&registry, &style, "s", |_| true, (0, 0, 0), &data, |_| Some(glyphs.clone()));
        let layout = layout.unwrap();
        let mut codec = Codec::new(&registry);
        let reply = codec.send(&layout, data.clone()).unwrap().run();
        let crossed = codec.reply(&reply).unwrap();
        let symbols = crossed.bucket::<SymbolBucket>("names").unwrap();
        assert!(symbols.has_text_data() && symbols.has_icon_data());
        assert!(!symbols.state.symbol_instances.is_empty());
    }
}
