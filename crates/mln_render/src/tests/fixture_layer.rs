//! `dot`, the tests' layer type, which can't reach the built-in layer types
//! outside this crate. It is a layer type of a geometry source. Its layout
//! makes a bucket of each feature's points that its filter selects, with
//! each point's `dot-color`, which can read feature state, and adds the
//! features to the tile's feature index. A layer with a `dot-pattern` asks
//! for the pattern's image, and one with a `dot-icon` or a `dot-label` for
//! the icon of each feature and the glyphs of its label in the font stack
//! `dot-font`. `dot-size` changes nothing but the layout. A `dot` layer
//! draws nothing.
//!
//! `tint` is a layer type of a raster source, and `shade` one of a
//! raster-dem source, both drawing nothing.

use std::sync::Arc;

use mln_style::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property_table::{PropertyInfo, PropertyTable, PropertyVisitor};
use mln_style::property_value::StyleValue as _;
use mln_style_types::image::ImageType;
use mln_text::glyph::GlyphID;
use mln_util::error::Error;

use crate::draw::Draws;
use crate::layer::{
    Bucket, BuildCx, DemTiles, EvaluatedLayer, Features, GpuVec, Layer, LayerBinders, Layout, LayoutCx, NewCx,
    PrepareCx, RasterTiles, Upload,
};

mln_style::properties!(
    /// The layout properties of a `dot` layer.
    #[expect(non_snake_case, reason = "named as the layer types' property sets are")]
    pub(crate) mod DotLayoutProperties: layout {
        dot_font: data_driven Vec<String> = Vec::new(),
        dot_icon: data_driven Image = Image::default(),
        dot_label: data_driven Formatted = Formatted::default(),
        dot_size: data_driven f32 = 1.0_f32,
    }
    use mln_style::expression::Formatted, mln_style::expression::Image;
);

mln_style::properties!(
    /// The paint properties of a `dot` layer.
    #[expect(non_snake_case, reason = "named as the layer types' property sets are")]
    pub(crate) mod DotPaintProperties: paint {
        dot_color: data_driven Color = Color::black(),
        dot_pattern: cross_faded Image = Image::default(),
    }
    use mln_style::expression::Image, mln_util::Color;
);

static PROPERTIES: PropertyTable = PropertyTable::new(
    &[
        PropertyInfo::paint("dot-color").data_driven(),
        PropertyInfo::paint("dot-pattern"),
        PropertyInfo::layout("dot-font").data_driven(),
        PropertyInfo::layout("dot-icon").data_driven(),
        PropertyInfo::layout("dot-label").data_driven(),
        PropertyInfo::layout("dot-size").data_driven(),
    ],
    &[0, 2, 3, 4, 1, 5],
);

/// The image of `layer`'s `dot-pattern`, if it has one.
fn pattern(layer: &EvaluatedLayer<Dot>) -> Option<&str> {
    Some(layer.paint.dot_pattern.faded().to.id()).filter(|id| !id.is_empty())
}

/// Whether `layer` has an icon or a label, whose layout needs their images
/// and glyphs.
fn draws_icons_or_labels(layer: &EvaluatedLayer<Dot>) -> bool {
    let layout = &layer.style.layout;
    !layout.dot_icon.is_undefined() || !layout.dot_label.is_undefined()
}

/// The `dot` layer type, both its halves.
pub(crate) struct Dot;

impl StyleLayerType for Dot {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "dot", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = DotLayoutProperties::Unevaluated;
    type PaintProperties = DotPaintProperties::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        Some(match slot {
            0 => visitor.paint(|s| &s.paint.dot_color, |s| &mut s.paint.dot_color),
            1 => visitor.paint(|s| &s.paint.dot_pattern, |s| &mut s.paint.dot_pattern),
            2 => visitor.layout(|s| &s.layout.dot_font, |s| &mut s.layout.dot_font),
            3 => visitor.layout(|s| &s.layout.dot_icon, |s| &mut s.layout.dot_icon),
            4 => visitor.layout(|s| &s.layout.dot_label, |s| &mut s.layout.dot_label),
            5 => visitor.layout(|s| &s.layout.dot_size, |s| &mut s.layout.dot_size),
            _ => return None,
        })
    }
}

mod binders {
    use mln_util::Color;

    use super::DotPaintProperties;
    use crate::layer::PaintPropertyBinder;

    crate::paint_property_binders!(
        pub(crate) struct DotBinders for DotPaintProperties::PossiblyEvaluated {
            dot_color: PaintPropertyBinder<Color> => ["color"],
        }
    );
}

use binders::DotBinders;

/// A `dot` layer's bucket: the points of its features, with their colors.
#[derive(Clone, mln_wire::Wire)]
pub(crate) struct DotBucket {
    pub(crate) points: GpuVec<[i16; 2]>,
    binders: LayerBinders<DotBinders>,
}

impl Bucket for DotBucket {
    fn upload(&mut self, up: &mut Upload<'_>) {
        up.vec(&mut self.points);
        up.binders(&mut self.binders);
    }
}

/// A `dot` group's layout: the features its leader's filter selects, by
/// index in the source layer.
pub(crate) struct DotLayout(Vec<usize>);

impl Layout<Dot> for DotLayout {
    type Bucket = DotBucket;

    fn needs_resources(group: &[Arc<EvaluatedLayer<Dot>>]) -> bool {
        group.iter().any(|layer| pattern(layer).is_some() || draws_icons_or_labels(layer))
    }

    fn new(cx: &mut LayoutCx<'_, Dot>) -> Result<Self, Error> {
        let patterns: Vec<String> = cx.layers().iter().filter_map(|layer| pattern(layer).map(str::to_owned)).collect();
        for id in patterns {
            cx.need_image(&id, ImageType::Pattern);
        }
        let leader = Arc::clone(&cx.layers()[0]);
        let features = cx.selected()?;
        if !draws_icons_or_labels(&leader) {
            return Ok(DotLayout(features));
        }
        let layout = leader.style.layout.evaluate(cx.parameters());
        for &i in &features {
            let feature = cx.feature(i)?;
            let context = cx.context(&feature);
            let icon = layout.dot_icon.evaluate(&context);
            let label = (!leader.style.layout.dot_label.is_undefined())
                .then(|| (layout.dot_font.evaluate(&context), layout.dot_label.evaluate(&context)));
            if !icon.empty() {
                cx.need_image(icon.id(), ImageType::Icon);
            }
            if let Some((font_stack, label)) = label {
                let glyphs = label.sections.iter().flat_map(|section| section.text.chars());
                cx.need_glyphs(font_stack, glyphs.map(|c| GlyphID::from(c as u16)).collect::<Vec<_>>());
            }
        }
        Ok(DotLayout(features))
    }

    fn build(self, cx: &mut BuildCx<'_, Dot>) -> Result<Option<DotBucket>, Error> {
        let mut bucket = DotBucket { points: GpuVec::default(), binders: cx.binders() };
        for i in self.0 {
            let feature = cx.feature(i)?;
            let geometry = feature.geometry()?;
            bucket.points.get_mut().extend(geometry.iter().flatten().map(|p| [p.x, p.y]));
            cx.populate(&mut bucket.binders, i, &feature, bucket.points.len(), None, None);
            cx.index(i, &geometry);
        }
        Ok((!bucket.points.is_empty()).then_some(bucket))
    }
}

impl Layer for Dot {
    type Style = Dot;
    type Input = Features<DotLayout>;
    type Tile = ();

    fn new(_cx: &mut NewCx<'_, Self>) -> Self {
        Dot
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, _out: &mut Draws) {}
}

/// The `tint` layer type, both its halves.
pub(crate) struct Tint;

impl StyleLayerType for Tint {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "tint", source: SourceKind::Raster, from_json: true };

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();
}

impl Layer for Tint {
    type Style = Tint;
    type Input = RasterTiles;
    type Tile = ();

    fn new(_cx: &mut NewCx<'_, Self>) -> Self {
        Tint
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, _out: &mut Draws) {}
}

/// The `shade` layer type, both its halves.
pub(crate) struct Shade;

impl StyleLayerType for Shade {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "shade", source: SourceKind::RasterDem, from_json: true };

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();
}

impl Layer for Shade {
    type Style = Shade;
    type Input = DemTiles;
    type Tile = ();

    fn new(_cx: &mut NewCx<'_, Self>) -> Self {
        Shade
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, _out: &mut Draws) {}
}
