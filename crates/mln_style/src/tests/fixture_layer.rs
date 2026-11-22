//! Layer types for the tests of this crate, which can't reach the built-in
//! layer types outside it: `plain`, a layer type without a source, whose
//! paint properties are a constant color and opacity and a cross-faded
//! pattern, `dot`, a layer type of a geometry source with data-driven paint
//! and layout properties and a constant layout property, and `shade`, a
//! layer type of a raster-dem source.

use crate::layer_type::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use crate::property_table::{PropertyInfo, PropertyTable, PropertyVisitor};

crate::properties!(
    /// The paint properties of a `plain` layer.
    #[expect(non_snake_case, reason = "named as the layer types' property sets are")]
    #[expect(unreachable_pub, dead_code, reason = "a property set has the API of the layer types' sets")]
    pub(crate) mod PlainPaintProperties: paint {
        plain_color: constant Color = Color::black(),
        plain_opacity: constant f32 = 1.0_f32,
        plain_pattern: cross_faded Image = Image::default(),
    }
    use crate::expression::Image, mln_util::Color;
);

static PROPERTIES: PropertyTable = PropertyTable::new(
    &[PropertyInfo::paint("plain-color"), PropertyInfo::paint("plain-opacity"), PropertyInfo::paint("plain-pattern")],
    &[0, 1, 2],
);

/// The `plain` layer type.
pub(crate) struct Plain;

impl StyleLayerType for Plain {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "plain", source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = PlainPaintProperties::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        Some(match slot {
            0 => visitor.paint(|s| &s.paint.plain_color, |s| &mut s.paint.plain_color),
            1 => visitor.paint(|s| &s.paint.plain_opacity, |s| &mut s.paint.plain_opacity),
            2 => visitor.paint(|s| &s.paint.plain_pattern, |s| &mut s.paint.plain_pattern),
            _ => return None,
        })
    }
}

crate::properties!(
    /// The layout properties of a `dot` layer.
    #[expect(non_snake_case, reason = "named as the layer types' property sets are")]
    #[expect(unreachable_pub, dead_code, reason = "a property set has the API of the layer types' sets")]
    pub(crate) mod DotLayoutProperties: layout {
        dot_alignment: constant AlignmentType = AlignmentType::Auto,
        dot_justify: data_driven TextJustifyType = TextJustifyType::Center,
        dot_size: data_driven f32 = 1.0_f32,
        dot_sort_key: data_driven f32 = 0.0_f32,
    }
    use mln_style_types::AlignmentType, mln_style_types::TextJustifyType;
);

crate::properties!(
    /// The paint properties of a `dot` layer.
    #[expect(non_snake_case, reason = "named as the layer types' property sets are")]
    #[expect(unreachable_pub, dead_code, reason = "a property set has the API of the layer types' sets")]
    pub(crate) mod DotPaintProperties: paint {
        dot_color: data_driven Color = Color::black(),
        dot_opacity: data_driven f32 = 1.0_f32,
        dot_width: data_driven f32 = 0.0_f32,
    }
    use mln_util::Color;
);

static DOT_PROPERTIES: PropertyTable = PropertyTable::new(
    &[
        PropertyInfo::paint("dot-color").data_driven(),
        PropertyInfo::paint("dot-opacity").data_driven(),
        PropertyInfo::paint("dot-width").data_driven(),
        PropertyInfo::layout("dot-alignment"),
        PropertyInfo::layout("dot-justify").data_driven(),
        PropertyInfo::layout("dot-size").data_driven(),
        PropertyInfo::layout("dot-sort-key").data_driven(),
    ],
    &[3, 0, 4, 1, 5, 6, 2],
);

/// The `dot` layer type.
pub(crate) struct Dot;

impl StyleLayerType for Dot {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "dot", source: SourceKind::Geometry, from_json: true };
    const PROPERTIES: &'static PropertyTable = &DOT_PROPERTIES;

    type LayoutProperties = DotLayoutProperties::Unevaluated;
    type PaintProperties = DotPaintProperties::Transitionable;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
        Some(match slot {
            0 => visitor.paint(|s| &s.paint.dot_color, |s| &mut s.paint.dot_color),
            1 => visitor.paint(|s| &s.paint.dot_opacity, |s| &mut s.paint.dot_opacity),
            2 => visitor.paint(|s| &s.paint.dot_width, |s| &mut s.paint.dot_width),
            3 => visitor.layout(|s| &s.layout.dot_alignment, |s| &mut s.layout.dot_alignment),
            4 => visitor.layout(|s| &s.layout.dot_justify, |s| &mut s.layout.dot_justify),
            5 => visitor.layout(|s| &s.layout.dot_size, |s| &mut s.layout.dot_size),
            6 => visitor.layout(|s| &s.layout.dot_sort_key, |s| &mut s.layout.dot_sort_key),
            _ => return None,
        })
    }
}

/// `shade`, a layer type of a raster-dem source without properties.
pub(crate) struct Shade;

static NO_PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

impl StyleLayerType for Shade {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "shade", source: SourceKind::RasterDem, from_json: true };
    const PROPERTIES: &'static PropertyTable = &NO_PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
}
