// The layout keeps the tile's `SourceLayer` (shared tile data) and owned
// `SymbolFeature`s (see symbol_feature.rs), so it can wait for glyphs and
// images across worker messages.
//

use std::borrow::Cow;
use std::range::RangeInclusive;
use std::sync::Arc;

use geometry::Point;
use glam::Vec2;
use mln_style::constants::{EXTENT, TILE_SIZE_D};
use mln_style::error::Error;
use mln_style::expression::formatted::K_FORMATTED_SECTION_TEXT_COLOR;
use mln_style::expression::{EvaluationContext, GlobalStateMap, Value};
use mln_style::feature::{GeometryCoordinate, GeometryCoordinates, GeometryTileFeature};
use mln_style::geometry::FeatureType;
use mln_style::layers::Symbol;
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style::padding::Padding;
use mln_style::property::evaluated::Evaluated;
use mln_style::property::value::PropertyValue;
use mln_style::range::Range;
use mln_style::tile_id::CanonicalTileID;
use mln_style::types::{
    AlignmentType, AnchorOffsetPair, IconTextFitType, SymbolAnchorType, SymbolPlacementType, SymbolZOrderType,
    TextJustifyType, TextRotationAlignmentType, TextTransformType, TextVariableAnchorType, TextWritingModeType,
    VariableAnchorOffsetCollection,
};
use mln_style::{PropertyEvaluationParameters, StyleValue as _, i18n};
use mln_util::constants::ONE_EM;
use mln_vec_map::VecMap;

use crate::buckets::symbol_bucket::{
    Buffer, PaintProperties, PlacedSymbol, QuadCorner, SortKeyRange, SymbolBucket, SymbolBucketState,
    SymbolBucketStyle, SymbolLayoutFlags, SymbolSizeBinder, max_height_offset,
};
use crate::draw::mesh::Indexed;
use crate::geometry::anchor::Anchor;
use crate::image::image_store::ImageType;
use crate::layer::bucket_parameters::BucketParameters;
use crate::layer::layer_type::EvaluatedLayer;
use crate::layer::layout::{BuildParams, Layout, LayoutParameters, SourceLayer, layers_by_id};
use crate::layer::tile_layout::Resolved;
use crate::layout::clip_lines::clip_lines;
use crate::layout::merge_lines::merge_lines;
use crate::layout::symbol_feature::SymbolFeature;
use crate::layout::symbol_instance::{
    IconPart, Justification, LaidOutSymbol, ShapedTextOrientations, SymbolInstance, SymbolInstancePools, SymbolQuadSet,
    TextPart, TextRuns, WritingMode,
};
use crate::map::MapMode;
use crate::placement::collision_feature::{AlongLine, BoxLayout, CollisionBox, CollisionFeature};
use crate::placement::collision_index::OverlapMode;
use crate::query::feature_index::{FeatureIndexBuilder, IndexedSubfeature};
use crate::text::atlas::ImagePositions;
use crate::text::get_anchors::{LineLabel, get_anchors, get_center_anchor};
use crate::text::glyph::{GlyphID, GlyphIDType, GlyphPositions, Shaping};
use crate::text::glyph_store::ShapeResults;
use crate::text::quads::{SymbolQuad, SymbolQuads, glyph_quads, icon_quads};
use crate::text::shaping::{PositionedIcon, ShapingOptions, anchor_justification, shape_text};
use crate::text::tagged_string::{SectionIndex, SectionOptions};
use crate::text::{ShapedText, TaggedString, shape_arabic, starts_right_to_left};
use crate::util::pool::PoolRange;

/// Whether `text-field` is set: a constant that is not empty, or an
/// expression.
fn has_text_field(layout: &SymbolLayoutProperties::PossiblyEvaluated) -> bool {
    layout.text_field.constant().is_none_or(|t| !t.empty())
}
fn has_text_font(layout: &SymbolLayoutProperties::PossiblyEvaluated) -> bool {
    layout.text_font.constant().is_none_or(|t| !t.is_empty())
}
fn has_icon_image(layout: &SymbolLayoutProperties::PossiblyEvaluated) -> bool {
    layout.icon_image.constant().is_none_or(|t| !t.empty())
}

/// A run of code points that one font face draws.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GlyphTypeSpan {
    range: RangeInclusive<u32>,
    type_: GlyphIDType,
    /// Whether a fallback face draws it, only where PBFs can't.
    fallback: bool,
}

/// Which font draws each character of one font stack's text, as the style
/// spec resolves fonts. The faces that name one of the stack's fonts win, in
/// the stack's order; then glyph PBFs, if the style has them. The fallback
/// faces, which have no name, come before the PBFs only for the text they
/// can't shape (`i18n::char_in_supported_script`); in a style without PBFs,
/// they and then the device's fonts draw the rest, if the faces include
/// them ([`FontFace::system`](crate::text::glyph::FontFace::system)), except
/// the characters that line breaking and vertical writing place on their
/// own, such as spaces and CJK (`i18n::breaks_or_stands_upright`). Else the
/// rest takes its glyphs one at a time, from PBFs or local fonts
/// (`glyph_store.rs`). Marks and joiners stay with the font of the
/// character before them.
///
/// The spans are sorted and disjoint, so a lookup is a binary search.
#[derive(Debug, Default)]
struct GlyphTypeTable {
    spans: Vec<GlyphTypeSpan>,
    /// Whether the style has glyph PBFs.
    glyph_pbfs: bool,
    /// What draws the characters that no face does.
    rest: GlyphIDType,
}

impl GlyphTypeTable {
    fn new(stack: &[String], faces: &crate::text::glyph::FontFaces, glyph_pbfs: bool) -> Self {
        let named =
            stack.iter().flat_map(|name| faces.iter().filter(move |face| !name.is_empty() && face.name == *name));
        let fallbacks = faces.iter().filter(|face| face.name.is_empty());
        let candidates: Vec<&crate::text::glyph::FontFace> = named.chain(fallbacks).collect();
        // The ranges as far as a code point reaches.
        let ranges = || {
            candidates.iter().flat_map(|face| &face.ranges).filter_map(|&(first, last)| {
                let last = last.min(u32::from(char::MAX));
                (first <= last).then_some((first, last))
            })
        };
        // Which face draws a code point stays the same between consecutive
        // bounds.
        let mut bounds: Vec<u32> = ranges().flat_map(|(first, last)| [first, last + 1]).collect();
        bounds.sort_unstable();
        bounds.dedup();
        let mut spans: Vec<GlyphTypeSpan> = Vec::new();
        for pair in bounds.windows(2) {
            let (first, last) = (pair[0], pair[1] - 1);
            let Some(face) = candidates.iter().find(|face| face.ranges.iter().any(|r| first >= r.0 && first <= r.1))
            else {
                continue;
            };
            let fallback = face.name.is_empty();
            match spans.last_mut() {
                Some(span) if span.type_ == face.type_ && span.range.last + 1 == first => {
                    span.range = (span.range.start..=last).into();
                }
                _ => spans.push(GlyphTypeSpan { range: (first..=last).into(), type_: face.type_, fallback }),
            }
        }
        let system = faces.iter().any(|face| face.type_ == GlyphIDType::SYSTEM);
        let rest = if system && !glyph_pbfs { GlyphIDType::SYSTEM } else { GlyphIDType::FontPBF };
        GlyphTypeTable { spans, glyph_pbfs, rest }
    }

    /// The glyph type of `c`, after a character of type `previous`.
    fn get(&self, c: char, previous: GlyphIDType) -> GlyphIDType {
        if i18n::is_variation_selector(c) || (previous != GlyphIDType::FontPBF && i18n::extends_previous(c)) {
            return previous;
        }
        if self.spans.is_empty() {
            return self.rest_for(c);
        }
        let code_point = u32::from(c);
        let index = self.spans.partition_point(|span| span.range.last < code_point);
        match self.spans.get(index) {
            Some(span)
                if span.range.start <= code_point
                    && !(span.fallback && self.glyph_pbfs && i18n::char_in_supported_script(c)) =>
            {
                span.type_
            }
            _ => self.rest_for(c),
        }
    }

    /// The type of `c` where no face draws it.
    fn rest_for(&self, c: char) -> GlyphIDType {
        if self.rest == GlyphIDType::SYSTEM && i18n::breaks_or_stands_upright(c) {
            GlyphIDType::FontPBF
        } else {
            self.rest
        }
    }
}

/// The glyph type tables of the font stacks that one layout's text uses, a
/// few at most, so it searches them in order.
struct GlyphTypeTables {
    faces: Option<Arc<crate::text::glyph::FontFaces>>,
    glyph_pbfs: bool,
    tables: Vec<(Vec<String>, GlyphTypeTable)>,
}

impl GlyphTypeTables {
    fn new(faces: Option<Arc<crate::text::glyph::FontFaces>>, glyph_pbfs: bool) -> Self {
        GlyphTypeTables { faces: faces.filter(|faces| !faces.is_empty()), glyph_pbfs, tables: Vec::new() }
    }

    fn get(&mut self, stack: &[String]) -> &GlyphTypeTable {
        let Some(faces) = &self.faces else {
            static EMPTY: GlyphTypeTable =
                GlyphTypeTable { spans: Vec::new(), glyph_pbfs: true, rest: GlyphIDType::FontPBF };
            return &EMPTY;
        };
        let glyph_pbfs = self.glyph_pbfs;
        let index = self.tables.iter().position(|(known, _)| known.as_slice() == stack).unwrap_or_else(|| {
            self.tables.push((stack.to_vec(), GlyphTypeTable::new(stack, faces, glyph_pbfs)));
            self.tables.len() - 1
        });
        &self.tables[index].1
    }
}

#[must_use]
pub(crate) fn section_options_to_value(options: &SectionOptions) -> Value {
    let mut result = mln_style::expression::value::ObjectType::new();
    // TODO: Data driven properties that can be overridden on per section
    // basis. TextOpacity TextHaloColor TextHaloWidth TextHaloBlur
    if let Some(text_color) = options.text_color {
        result.insert(K_FORMATTED_SECTION_TEXT_COLOR.to_owned(), Value::Color(text_color));
    }
    Value::Object(result)
}

fn create_layout(
    unevaluated: &SymbolLayoutProperties::Unevaluated,
    zoom: f32,
    global_state: Option<Arc<GlobalStateMap>>,
) -> SymbolLayoutProperties::PossiblyEvaluated {
    let mut evaluation_parameters = PropertyEvaluationParameters::new(zoom);
    evaluation_parameters.global_state = global_state;
    let mut layout = unevaluated.evaluate(&evaluation_parameters);

    let placement = layout.symbol_placement.clone();
    if *layout.icon_rotation_alignment.uniform() == AlignmentType::Auto {
        layout.icon_rotation_alignment = Evaluated::Constant(if *placement.uniform() == SymbolPlacementType::Point {
            AlignmentType::Viewport
        } else {
            AlignmentType::Map
        });
    }

    if *layout.text_rotation_alignment.uniform() == TextRotationAlignmentType::Auto {
        layout.text_rotation_alignment = Evaluated::Constant(if *placement.uniform() == SymbolPlacementType::Point {
            TextRotationAlignmentType::Viewport
        } else {
            TextRotationAlignmentType::Map
        });
    }

    // If unspecified `*-pitch-alignment` inherits `*-rotation-alignment`, and
    // `viewport-glyph` pitches with the viewport.
    if *layout.text_pitch_alignment.uniform() == AlignmentType::Auto {
        layout.text_pitch_alignment =
            Evaluated::Constant(if *layout.text_rotation_alignment.uniform() == TextRotationAlignmentType::Map {
                AlignmentType::Map
            } else {
                AlignmentType::Viewport
            });
    }
    if *layout.icon_pitch_alignment.uniform() == AlignmentType::Auto {
        layout.icon_pitch_alignment = layout.icon_rotation_alignment.clone();
    }

    layout
}

// The radial offset is to the edge of the text box. In the horizontal
// direction, the edge of the text box is where glyphs start. But in the
// vertical direction, the glyphs appear to "start" at the baseline. We don't
// actually load baseline data, but we assume an offset of ONE_EM - 17, as
// the shaping does.
const BASELINE_OFFSET: f32 = 7.0;

fn evaluate_radial_offset(anchor: SymbolAnchorType, radial_offset: f32) -> [f32; 2] {
    let mut result = [0.0_f32, 0.0_f32];
    let radial_offset = mln_style::math::fmaxf(radial_offset, 0.0); // Ignore negative offset.
    // Solve for r where r^2 + r^2 = radial_offset^2.
    let hypotenuse = radial_offset / std::f32::consts::SQRT_2;

    match anchor {
        SymbolAnchorType::TopRight | SymbolAnchorType::TopLeft => result[1] = hypotenuse - BASELINE_OFFSET,
        SymbolAnchorType::BottomRight | SymbolAnchorType::BottomLeft => result[1] = -hypotenuse + BASELINE_OFFSET,
        SymbolAnchorType::Bottom => result[1] = -radial_offset + BASELINE_OFFSET,
        SymbolAnchorType::Top => result[1] = radial_offset - BASELINE_OFFSET,
        _ => {}
    }

    match anchor {
        SymbolAnchorType::TopRight | SymbolAnchorType::BottomRight => result[0] = -hypotenuse,
        SymbolAnchorType::TopLeft | SymbolAnchorType::BottomLeft => result[0] = hypotenuse,
        SymbolAnchorType::Left => result[0] = radial_offset,
        SymbolAnchorType::Right => result[0] = -radial_offset,
        _ => {}
    }

    result
}

/// The context to evaluate `feature`'s layout at `zoom` in.
fn feature_context<'a>(zoom: f32, feature: &'a SymbolFeature, canonical: &'a CanonicalTileID) -> EvaluationContext<'a> {
    EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(canonical))
}

/// What `prepare_symbols` made of a feature: its text shaped and its icon
/// placed in the image atlas.
struct ShapedFeature {
    text: ShapedTextOrientations,
    icon: Option<PositionedIcon>,
    text_offset: [f32; 2],
    /// `text-size` and `icon-size` a zoom level up, which size the symbols'
    /// collision boxes.
    text_size: f32,
    icon_size: f32,
}

/// What the symbols of one feature share: its shapes, and how its quads and
/// collision boxes are laid out.
struct FeatureSymbols<'a> {
    shaped: &'a ShapedFeature,
    /// The icon fitted to the vertical text.
    vertical_icon: Option<&'a PositionedIcon>,
    values: &'a SymbolLayoutProperties::Values,
    icons: &'a ImagePositions,
    text_placement: SymbolPlacementType,
    has_icon_text_fit: bool,
    allow_vertical_placement: bool,
    overscaling: f32,
    text_box_scale: f32,
    text_padding: f32,
    icon_box_scale: f32,
    icon_padding: Padding,
    variable_anchors: Option<VariableAnchorOffsetCollection>,
    height_offset: f32,
    indexed_feature: IndexedSubfeature,
    key: &'a str,
    feature_index: usize,
    data_feature_index: usize,
}

impl FeatureSymbols<'_> {
    /// The quads of the feature's symbols along `line`.
    fn quads(&self, line: GeometryCoordinates) -> Arc<SymbolQuadSet> {
        let text = &self.shaped.text;
        let glyphs = |shaping: &Shaping| {
            glyph_quads(
                shaping,
                self.shaped.text_offset,
                self.values,
                self.text_placement,
                self.icons,
                self.allow_vertical_placement,
            )
        };
        let icon = |icon: &PositionedIcon| icon_quads(icon, self.values.icon_rotate, self.has_icon_text_fit);
        let mut horizontal: [SymbolQuads; 3] = Default::default();
        if text.single_line {
            if let Some(shaping) = text.any_horizontal() {
                horizontal[Justification::Right as usize] = glyphs(shaping);
            }
        } else {
            for justification in Justification::ALL {
                let shaping = text.get(justification);
                if !shaping.is_empty() {
                    horizontal[justification as usize] = glyphs(shaping);
                }
            }
        }
        Arc::new(SymbolQuadSet {
            line,
            single_line: text.single_line,
            horizontal,
            vertical: if text.vertical.is_empty() { SymbolQuads::new() } else { glyphs(&text.vertical) },
            icon: self.shaped.icon.as_ref().map(icon),
            vertical_icon: self.shaped.icon.as_ref().and(self.vertical_icon).map(icon),
        })
    }

    /// The feature's symbol at `anchor`, whose collision boxes and key go to
    /// `pools`.
    fn symbol(&self, anchor: Anchor, quads: Arc<SymbolQuadSet>, pools: &mut SymbolInstancePools) -> LaidOutSymbol {
        // Vertical text turns its boxes a quarter.
        const VERTICAL_ROTATION: f32 = 90.0;
        let line = &quads.line;
        let along_line = (self.text_placement != SymbolPlacementType::Point)
            .then_some(AlongLine { line, overscaling: self.overscaling });
        let text_feature = |shaping: &Shaping, rotate: f32, boxes: &mut Vec<CollisionBox>| {
            let layout = BoxLayout { scale: self.text_box_scale, padding: Padding::uniform(self.text_padding), rotate };
            CollisionFeature::new_text(&anchor, shaping, along_line, &layout, &self.indexed_feature, boxes)
        };
        let icon_feature = |icon: &PositionedIcon, rotate: f32, boxes: &mut Vec<CollisionBox>| {
            let layout = BoxLayout { scale: self.icon_box_scale, padding: self.icon_padding, rotate };
            CollisionFeature::new_icon(&anchor, Some(icon), &layout, &self.indexed_feature, boxes)
        };
        let text = &self.shaped.text;
        let vertical = self.allow_vertical_placement && !text.vertical.is_empty();
        let text_part = quads.has_text().then(|| TextPart {
            runs: TextRuns::default(),
            collision: text_feature(text.any(), self.values.text_rotate, &mut pools.collision_boxes),
            vertical_collision: vertical.then(|| {
                text_feature(&text.vertical, self.values.text_rotate + VERTICAL_ROTATION, &mut pools.collision_boxes)
            }),
            box_scale: self.text_box_scale,
            variable_anchors: self.variable_anchors.clone(),
        });
        let icon_part = self.shaped.icon.as_ref().map(|icon| IconPart {
            run: 0,
            vertical_run: None,
            collision: icon_feature(icon, self.values.icon_rotate, &mut pools.collision_boxes),
            vertical_collision: self.vertical_icon.filter(|_| vertical).map(|icon| {
                icon_feature(icon, self.values.icon_rotate + VERTICAL_ROTATION, &mut pools.collision_boxes)
            }),
            sdf: icon.image.sdf,
        });
        LaidOutSymbol {
            symbol: SymbolInstance {
                anchor,
                key: PoolRange::extend(&mut pools.keys, self.key.bytes()),
                data_feature_index: self.data_feature_index,
                height_offset: self.height_offset,
                cross_tile_id: None,
                text: text_part,
                icon: icon_part,
            },
            quads,
            feature: self.feature_index,
            text_offset: self.shaped.text_offset,
            icon_offset: self.values.icon_offset,
        }
    }
}

pub(crate) struct SymbolLayout {
    pub(crate) layer_paint_properties: VecMap<String, Arc<EvaluatedLayer<Symbol>>>,
    /// Shared by every symbol's `IndexedSubfeature`, which placement clones
    /// for each symbol it places.
    pub(crate) bucket_leader_id: Arc<str>,
    symbols: Vec<LaidOutSymbol>,
    symbol_instance_pools: SymbolInstancePools,
    pub(crate) sort_key_ranges: Vec<SortKeyRange>,

    compare_text: VecMap<String, Vec<Anchor>>,

    // Stores the layer so that we can hold on to GeometryTileFeature
    // instances in SymbolFeature, which may reference data from this object.
    source_layer: SourceLayer,
    /// `source_layer`'s name, shared like `bucket_leader_id`.
    source_layer_name: Arc<str>,
    overscaling: f32,
    zoom: f32,
    canonical_id: CanonicalTileID,
    mode: MapMode,
    pixel_ratio: f32,
    tile_pixel_ratio: f32,

    icons_need_linear: bool,
    sort_features_by_y: bool,
    sort_features_by_key: bool,
    allow_vertical_placement: bool,
    icons_in_text: bool,
    placement_modes: Vec<TextWritingModeType>,

    text_size: PropertyValue<f32>,
    icon_size: PropertyValue<f32>,
    text_radial_offset: PropertyValue<f32>,
    text_variable_anchor_offset: PropertyValue<VariableAnchorOffsetCollection>,

    layout: Arc<SymbolLayoutProperties::PossiblyEvaluated>,

    features: Vec<SymbolFeature>,

    /// Whether a feature has text that font files shape, which
    /// `finalize_symbols` takes the shaping of.
    has_font_text: bool,
}

impl SymbolLayout {
    pub(crate) fn new(
        parameters: &BucketParameters,
        layers: &[Arc<EvaluatedLayer<Symbol>>],
        source_layer: SourceLayer,
        layout_parameters: &mut LayoutParameters<'_>,
    ) -> Result<Self, Error> {
        fn capture<T>(property_value: &mut PropertyValue<T>, global_state: Option<&Arc<GlobalStateMap>>) {
            if let Some(expression) = property_value.as_expression_mut() {
                expression.capture_global_state(global_state.cloned());
            }
        }

        let leader = &*layers[0].style;
        let overscaling = parameters.tile_id.overscale_factor() as f32;
        let zoom = f32::from(parameters.tile_id.overscaled_z);
        let tile_size = (TILE_SIZE_D * f64::from(overscaling)) as u32;
        let tile_pixel_ratio = EXTENT as f32 / tile_size as f32;
        let layout = Arc::new(create_layout(&leader.layout, zoom, parameters.global_state.clone()));

        let mut this = SymbolLayout {
            layer_paint_properties: VecMap::new(),
            bucket_leader_id: Arc::from(leader.id.as_str()),
            symbols: Vec::new(),
            symbol_instance_pools: SymbolInstancePools::default(),
            sort_key_ranges: Vec::new(),
            compare_text: VecMap::new(),
            source_layer_name: Arc::from(source_layer.name.as_str()),
            source_layer,
            overscaling,
            zoom,
            canonical_id: parameters.tile_id.canonical,
            mode: parameters.mode,
            pixel_ratio: parameters.pixel_ratio,
            tile_pixel_ratio,
            icons_need_linear: false,
            sort_features_by_y: false,
            sort_features_by_key: false,
            allow_vertical_placement: false,
            icons_in_text: false,
            placement_modes: Vec::new(),
            text_size: leader.layout.text_size.clone(),
            icon_size: leader.layout.icon_size.clone(),
            text_radial_offset: leader.layout.text_radial_offset.clone(),
            text_variable_anchor_offset: leader.layout.text_variable_anchor_offset.clone(),
            layout,
            features: Vec::new(),
            has_font_text: false,
        };

        // These property values are retained unevaluated; capture the
        // global state so that later evaluations can resolve "global-state"
        // expressions.
        capture(&mut this.text_size, parameters.global_state.as_ref());
        capture(&mut this.icon_size, parameters.global_state.as_ref());
        capture(&mut this.text_radial_offset, parameters.global_state.as_ref());
        capture(&mut this.text_variable_anchor_offset, parameters.global_state.as_ref());

        let layout = Arc::clone(&this.layout);
        let has_text = has_text_field(&layout) && has_text_font(&layout);
        let has_icon = has_icon_image(&layout);

        if !has_text && !has_icon {
            return Ok(this);
        }

        let has_symbol_sort_key = !leader.layout.symbol_sort_key.is_undefined();
        let symbol_z_order = &layout.symbol_z_order;
        this.sort_features_by_key = *symbol_z_order.uniform() != SymbolZOrderType::ViewportY && has_symbol_sort_key;
        let z_order_by_viewport_y = *symbol_z_order.uniform() == SymbolZOrderType::ViewportY
            || (*symbol_z_order.uniform() == SymbolZOrderType::Auto && !this.sort_features_by_key);
        this.sort_features_by_y = z_order_by_viewport_y
            && (OverlapMode::text(&layout) != OverlapMode::Never
                || OverlapMode::icon(&layout) != OverlapMode::Never
                || *layout.text_ignore_placement.uniform()
                || *layout.icon_ignore_placement.uniform());
        let sort_features = this.sort_features_by_key && !layout.symbol_sort_key.is_constant();
        if *layout.symbol_placement.uniform() == SymbolPlacementType::Point {
            let modes = layout.text_writing_mode.uniform().clone();
            // Remove duplicates and preserve order.
            let mut seen: Vec<TextWritingModeType> = Vec::new();
            let mut placement_modes = Vec::new();
            for placement_mode in modes {
                this.allow_vertical_placement =
                    this.allow_vertical_placement || placement_mode == TextWritingModeType::Vertical;
                if !seen.contains(&placement_mode) {
                    seen.push(placement_mode);
                    placement_modes.push(placement_mode);
                }
            }
            this.placement_modes = placement_modes;
        }

        this.layer_paint_properties = layers_by_id(layers);

        // Determine glyph dependencies
        let source_layer = this.source_layer.clone();
        let geometry_layer = source_layer.layer()?;
        let feature_count = geometry_layer.len();
        let canonical_id = this.canonical_id;
        let context = EvaluationContext::new(this.zoom)
            .with_canonical_tile_id(Some(&parameters.tile_id.canonical))
            .with_global_state(parameters.global_state.as_deref())
            .with_font_faces(layout_parameters.font_faces.as_deref());
        let passes = |feature: &dyn GeometryTileFeature| {
            leader.filter.evaluate(&EvaluationContext { feature: Some(feature), ..context })
        };
        let selected = layout_parameters.filter_groups.select(
            &source_layer.name,
            &geometry_layer,
            &leader.filter,
            &context,
            passes,
        );
        let mut glyph_type_tables =
            GlyphTypeTables::new(layout_parameters.font_faces.clone(), layout_parameters.glyph_pbfs);
        for i in 0..feature_count {
            if selected.as_ref().is_some_and(|selected| !selected[i]) {
                continue;
            }
            let feature = geometry_layer.feature(i)?;
            if selected.is_none() && !passes(&feature) {
                continue;
            }

            let mut ft = SymbolFeature::new(&feature, feature.geometry()?);

            ft.index = i;

            if has_text {
                let formatted = layout.text_field.evaluate(
                    &EvaluationContext::with_zoom_feature(zoom, Some(&ft))
                        .with_available_images(Some(layout_parameters.available_images))
                        .with_unavailable_images(Some(layout_parameters.unavailable_images))
                        .with_canonical_tile_id(Some(&canonical_id))
                        .with_font_faces(layout_parameters.font_faces.as_deref()),
                );
                let text_transform = layout.text_transform.evaluate(
                    &EvaluationContext::with_zoom_feature(zoom, Some(&ft)).with_canonical_tile_id(Some(&canonical_id)),
                );
                let base_font_stack: Vec<String> = layout.text_font.evaluate(
                    &EvaluationContext::with_zoom_feature(zoom, Some(&ft)).with_canonical_tile_id(Some(&canonical_id)),
                );

                let mut formatted_text = TaggedString::default();
                let mut section_table: VecMap<usize, usize> = VecMap::new();
                // Vertical layout reads the runs of a label that stands
                // upright in places, such as digits among CJK, a character
                // at a time: where the layer can lay text out vertically,
                // the device's fonts shape only such a label's complex
                // scripts.
                let may_be_vertical = this.allow_vertical_placement
                    || (*layout.text_rotation_alignment.uniform() != TextRotationAlignmentType::Viewport
                        && *layout.symbol_placement.uniform() != SymbolPlacementType::Point);
                let upright = may_be_vertical
                    && formatted
                        .sections
                        .iter()
                        .any(|section| section.text.chars().any(i18n::has_upright_vertical_orientation));

                for (section_index, section) in formatted.sections.iter().enumerate() {
                    if let Some(image) = &section.image {
                        // An empty image name means no image, so it is not an image
                        // dependency.
                        if !image.empty() {
                            layout_parameters
                                .image_dependencies
                                .entry(image.id().to_owned())
                                .or_insert(ImageType::Icon);
                        }
                        formatted_text.add_image_section(image.id(), section.vertical_align.unwrap_or_default());
                        continue;
                    }
                    let mut u8string = section.text.clone();
                    if text_transform == TextTransformType::Uppercase {
                        u8string = u8string.to_uppercase();
                    } else if text_transform == TextTransformType::Lowercase {
                        u8string = u8string.to_lowercase();
                    }

                    // Only the sections that draw from glyph PBFs take their
                    // Arabic presentation forms. A font file needs the
                    // letters themselves, to apply its own joining rules.
                    let chars: Vec<char> = u8string.chars().collect();
                    let section_scale = section.font_scale.unwrap_or(1.0);
                    let section_font_stack = section.font_stack.clone().unwrap_or_else(|| base_font_stack.clone());
                    let glyph_types = glyph_type_tables.get(&section_font_stack);
                    let type_of = |c: char, previous: GlyphIDType| match glyph_types.get(c, previous) {
                        GlyphIDType::SYSTEM if upright && i18n::char_in_supported_script(c) => GlyphIDType::FontPBF,
                        type_ => type_,
                    };

                    let mut sub_string: Vec<char> = Vec::new();
                    // An empty string's first character is the terminating NUL.
                    let mut sub_string_type = type_of(chars.first().copied().unwrap_or('\0'), GlyphIDType::FontPBF);

                    let mut add_sub_string =
                        |sub_string: &mut Vec<char>,
                         sub_string_type: GlyphIDType,
                         key_section: bool,
                         formatted_text: &mut TaggedString| {
                            if sub_string.is_empty() {
                                return;
                            }
                            if sub_string_type == GlyphIDType::FontPBF
                                && let Cow::Owned(shaped) = shape_arabic(sub_string)
                            {
                                *sub_string = shaped;
                            }
                            let mut options = SectionOptions::new(
                                section_scale as f32,
                                section_font_stack.clone(),
                                sub_string_type,
                                section.text_color,
                            );
                            options.key_section = key_section;
                            options.vertical_align = section.vertical_align.unwrap_or_default();
                            formatted_text.add_text_section(sub_string, options);
                            section_table.insert(formatted_text.sections().len() - 1, section_index);
                            if sub_string_type != GlyphIDType::FontPBF {
                                layout_parameters
                                    .glyph_dependencies
                                    .shapes
                                    .entry(section_font_stack.clone())
                                    .or_default()
                                    .entry(sub_string_type)
                                    .or_default()
                                    .insert(sub_string.clone());
                            }
                        };

                    for ch in chars {
                        let ch_type = type_of(ch, sub_string_type);
                        if ch_type != sub_string_type {
                            add_sub_string(&mut sub_string, sub_string_type, false, &mut formatted_text);
                            sub_string.clear();
                            sub_string_type = ch_type;
                        }

                        sub_string.push(ch);
                    }

                    add_sub_string(&mut sub_string, sub_string_type, true, &mut formatted_text);
                }

                let can_verticalize_text = *layout.text_rotation_alignment.uniform()
                    != TextRotationAlignmentType::Viewport
                    && *layout.symbol_placement.uniform() != SymbolPlacementType::Point
                    && formatted_text.allows_vertical_writing_mode();
                let allows_vertical = formatted_text.allows_vertical_writing_mode();

                // Loop through all characters of this text and collect
                // unique codepoints.
                for j in 0..formatted_text.len() {
                    let section_index = usize::from(formatted_text.section_index(j));
                    let section = &formatted_text.sections()[section_index];
                    if section.image_id.is_some() {
                        continue;
                    }
                    let section_font_stack =
                        section_table.get(&section_index).and_then(|&i| formatted.sections[i].font_stack.clone());
                    let font_stack = section_font_stack.unwrap_or_else(|| base_font_stack.clone());
                    let dependencies = layout_parameters.glyph_dependencies.glyphs.entry(font_stack).or_default();
                    if section.type_ == GlyphIDType::FontPBF {
                        // Glyph PBFs have no glyphs beyond the BMP.
                        let c = formatted_text.text()[j];
                        dependencies.extend(GlyphID::from_char(c));
                        if can_verticalize_text || (this.allow_vertical_placement && allows_vertical) {
                            dependencies.extend(i18n::verticalize_punctuation(c).and_then(GlyphID::from_char));
                        }
                    } else {
                        // A face's glyph 0 stands for its font file; the
                        // device's fonts have none to load.
                        if section.type_ != GlyphIDType::SYSTEM {
                            dependencies.insert(GlyphID::with_type(0, section.type_));
                        }
                        this.has_font_text = true;
                    }
                }
                ft.formatted_text = Some(formatted_text);
            }

            if has_icon {
                let icon = layout.icon_image.evaluate(
                    &EvaluationContext::with_zoom_feature(zoom, Some(&ft))
                        .with_available_images(Some(layout_parameters.available_images))
                        .with_unavailable_images(Some(layout_parameters.unavailable_images))
                        .with_canonical_tile_id(Some(&canonical_id)),
                );
                // An empty icon name means no icon, so it is not an image
                // dependency.
                if !icon.empty() {
                    layout_parameters.image_dependencies.entry(icon.id().to_owned()).or_insert(ImageType::Icon);
                    ft.icon = Some(icon);
                }
            }

            if ft.formatted_text.is_some() || ft.icon.is_some() {
                if this.sort_features_by_key {
                    ft.sort_key = layout.symbol_sort_key.evaluate(
                        &EvaluationContext::with_zoom_feature(zoom, Some(&ft))
                            .with_canonical_tile_id(Some(&canonical_id)),
                    );
                }
                if sort_features {
                    let lower_bound = this.features.partition_point(|f| f.sort_key < ft.sort_key);
                    this.features.insert(lower_bound, ft);
                } else {
                    this.features.push(ft);
                }
            }
        }
        drop(geometry_layer);

        if this.sort_features_by_key && !sort_features {
            // Inserting at the lower bound puts each feature before the
            // earlier ones with an equal key: keep that order without a sort.
            this.features.reverse();
        }

        if *layout.symbol_placement.uniform() == SymbolPlacementType::Line {
            merge_lines(&mut this.features);
        }

        Ok(this)
    }

    #[must_use]
    pub(crate) fn evaluate_variable_offset(
        anchor: SymbolAnchorType,
        offset: Option<[f32; 2]>,
        radial_offset: f32,
    ) -> [f32; 2] {
        let Some(mut offset) = offset else {
            return evaluate_radial_offset(anchor, radial_offset);
        };
        let mut result = [0.0_f32, 0.0_f32];
        offset[0] = offset[0].abs();
        offset[1] = offset[1].abs();

        match anchor {
            SymbolAnchorType::TopRight | SymbolAnchorType::TopLeft | SymbolAnchorType::Top => {
                result[1] = offset[1] - BASELINE_OFFSET;
            }
            SymbolAnchorType::BottomRight | SymbolAnchorType::BottomLeft | SymbolAnchorType::Bottom => {
                result[1] = -offset[1] + BASELINE_OFFSET;
            }
            SymbolAnchorType::Center | SymbolAnchorType::Left | SymbolAnchorType::Right => {}
        }

        match anchor {
            SymbolAnchorType::TopRight | SymbolAnchorType::BottomRight | SymbolAnchorType::Right => {
                result[0] = -offset[0];
            }
            SymbolAnchorType::TopLeft | SymbolAnchorType::BottomLeft | SymbolAnchorType::Left => {
                result[0] = offset[0];
            }
            SymbolAnchorType::Center | SymbolAnchorType::Top | SymbolAnchorType::Bottom => {}
        }

        result
    }

    /// The feature's variable anchors and offsets, from either
    /// `text-variable-anchor` or `text-variable-anchor-offset`, with the
    /// offsets converted from ems to pixels.
    fn text_variable_anchor_offset(&self, feature: &SymbolFeature) -> Option<VariableAnchorOffsetCollection> {
        let zoom = self.zoom;
        let canonical = &self.canonical_id;
        // If style specifies text-variable-anchor-offset, just return it
        if !self.text_variable_anchor_offset.is_undefined() {
            let variable_anchor_offset =
                self.layout.text_variable_anchor_offset.evaluate(&feature_context(zoom, feature, canonical));
            if variable_anchor_offset.is_empty() {
                return None;
            }
            let mut anchor_offsets = Vec::with_capacity(variable_anchor_offset.len());
            // Convert offsets from EM to PX, and apply baseline shift
            for anchor_offset in &variable_anchor_offset {
                let mut variable_text_offset = [anchor_offset.offset[0] * ONE_EM, anchor_offset.offset[1] * ONE_EM];
                match anchor_offset.anchor_type {
                    SymbolAnchorType::TopRight | SymbolAnchorType::TopLeft | SymbolAnchorType::Top => {
                        variable_text_offset[1] -= BASELINE_OFFSET;
                    }
                    SymbolAnchorType::BottomRight | SymbolAnchorType::BottomLeft | SymbolAnchorType::Bottom => {
                        variable_text_offset[1] += BASELINE_OFFSET;
                    }
                    SymbolAnchorType::Center | SymbolAnchorType::Left | SymbolAnchorType::Right => {}
                }
                anchor_offsets
                    .push(AnchorOffsetPair { anchor_type: anchor_offset.anchor_type, offset: variable_text_offset });
            }
            return Some(VariableAnchorOffsetCollection::new(anchor_offsets));
        }

        let variable_text_anchor: Vec<TextVariableAnchorType> =
            self.layout.text_variable_anchor.evaluate(&feature_context(zoom, feature, canonical));
        if variable_text_anchor.is_empty() {
            return None;
        }
        let (variable_text_offset, radial_offset) = if self.text_radial_offset.is_undefined() {
            let text_offset = self.layout.text_offset.evaluate(&feature_context(zoom, feature, canonical));
            (Some([text_offset[0] * ONE_EM, text_offset[1] * ONE_EM]), 0.0)
        } else {
            let radial_offset = self.layout.text_radial_offset.evaluate(&feature_context(zoom, feature, canonical));
            (None, radial_offset * ONE_EM)
        };

        let anchor_offset_pairs = variable_text_anchor
            .into_iter()
            .map(|anchor| AnchorOffsetPair {
                anchor_type: anchor,
                offset: Self::evaluate_variable_offset(anchor, variable_text_offset, radial_offset),
            })
            .collect();
        Some(VariableAnchorOffsetCollection::new(anchor_offset_pairs))
    }

    /// Adds the symbols of `feature`, the layout's feature `feature_index`,
    /// at each of its anchors.
    fn add_feature(
        &mut self,
        feature_index: usize,
        feature: &SymbolFeature,
        shaped: ShapedFeature,
        icons: &ImagePositions,
    ) {
        const GLYPH_SIZE: f32 = 24.0;
        let layout = Arc::clone(&self.layout);
        let context = feature_context(self.zoom, feature, &self.canonical_id);
        let values = layout.evaluate_all(&context);

        // A text size from a high zoom level, the same for every zoom, so
        // that labels keep their anchors as the map zooms.
        let text_max_size = layout.text_size.evaluate(&feature_context(18.0, feature, &self.canonical_id));
        let text_max_box_scale = self.tile_pixel_ratio * text_max_size / GLYPH_SIZE;
        let symbol_spacing = self.tile_pixel_ratio * *layout.symbol_spacing.uniform();
        let text_max_angle = layout.text_max_angle.uniform().to_radians();
        let placement = *layout.symbol_placement.uniform();
        let text_placement = if values.text_rotation_alignment == TextRotationAlignmentType::Viewport {
            SymbolPlacementType::Point
        } else {
            placement
        };

        let mut shaped = shaped;
        let has_icon_text_fit = values.icon_text_fit != IconTextFitType::None;
        let font_scale = shaped.text_size / GLYPH_SIZE;
        let mut vertical_icon = None;
        if let Some(icon) = shaped.icon.as_mut()
            && has_icon_text_fit
        {
            let fit = |icon: &mut PositionedIcon, text: &Shaping| {
                icon.fit_to_text(
                    text,
                    values.icon_text_fit,
                    values.icon_text_fit_padding,
                    values.icon_offset,
                    font_scale,
                );
            };
            if self.allow_vertical_placement && !shaped.text.vertical.is_empty() {
                let mut vertical = icon.clone();
                fit(&mut vertical, &shaped.text.vertical);
                vertical_icon = Some(vertical);
            }
            if let Some(text) = shaped.text.any_horizontal() {
                fit(icon, text);
            }
        }

        let key: String =
            feature.formatted_text.as_ref().map_or_else(String::new, |text| text.logical_text().iter().collect());
        let symbols = FeatureSymbols {
            shaped: &shaped,
            vertical_icon: vertical_icon.as_ref(),
            values: &values,
            icons,
            text_placement,
            has_icon_text_fit,
            allow_vertical_placement: self.allow_vertical_placement,
            overscaling: self.overscaling,
            text_box_scale: self.tile_pixel_ratio * font_scale,
            text_padding: values.text_padding * self.tile_pixel_ratio,
            icon_box_scale: self.tile_pixel_ratio * shaped.icon_size,
            icon_padding: values.icon_padding * self.tile_pixel_ratio,
            variable_anchors: self.text_variable_anchor_offset(feature),
            // The spec raises only point-placed symbols.
            height_offset: if placement == SymbolPlacementType::Point { values.symbol_height_offset } else { 0.0 },
            indexed_feature: IndexedSubfeature {
                index: feature.index,
                source_layer_name: Arc::clone(&self.source_layer_name),
                bucket_leader_id: Arc::clone(&self.bucket_leader_id),
                sort_index: self.symbols.len(),
                bucket_instance_id: 0,
                collision_group_id: 0,
                overlap_mode: OverlapMode::Never,
            },
            key: &key,
            feature_index,
            data_feature_index: feature.index,
        };

        let text_shaping =
            if shaped.text.vertical.is_empty() { shaped.text.any_horizontal() } else { Some(&shaped.text.vertical) };
        let line_label = LineLabel {
            text_width: text_shaping.map_or(0.0, |text| text.bounds.width()),
            icon_width: shaped.icon.as_ref().map_or(0.0, |icon| icon.bounds.width()),
            glyph_size: GLYPH_SIZE,
            box_scale: text_max_box_scale,
        };

        match placement {
            SymbolPlacementType::Line => {
                for line in clip_lines(&feature.geometry, 0, 0, EXTENT as i16, EXTENT as i16).iter() {
                    let anchors = get_anchors(line, &line_label, symbol_spacing, text_max_angle, self.overscaling);
                    let quads = symbols.quads(line.clone());
                    for anchor in anchors {
                        let too_close = feature.formatted_text.is_some()
                            && self.anchor_is_too_close(&key, symbol_spacing / 2.0, &anchor);
                        if !too_close {
                            self.add_symbol_instance(&symbols, feature, anchor, &quads);
                        }
                    }
                }
            }
            SymbolPlacementType::LineCenter => {
                // Each line takes a label at its center, unclipped; a line of
                // one point takes none.
                for line in feature.geometry.iter().filter(|line| line.len() > 1) {
                    let anchor = get_center_anchor(line, &line_label, text_max_angle);
                    if let Some(anchor) = anchor {
                        self.add_symbol_instance(&symbols, feature, anchor, &symbols.quads(line.clone()));
                    }
                }
            }
            SymbolPlacementType::Point => match feature.get_type() {
                FeatureType::Polygon => {
                    for polygon in mln_style::feature::classify_rings(&feature.geometry) {
                        let rings: polylabel::Polygon<f64> = polygon
                            .iter()
                            .map(|ring| ring.iter().map(|p| Point::new(f64::from(p.x), f64::from(p.y))).collect())
                            .collect();
                        // To a pixel, in tile units.
                        let Ok(poi) = polylabel::polylabel(&rings, f64::from(EXTENT) / TILE_SIZE_D) else {
                            continue;
                        };
                        let anchor = Anchor::new(poi.x as f32, poi.y as f32, 0.0, None);
                        self.add_symbol_instance(&symbols, feature, anchor, &symbols.quads(polygon[0].clone()));
                    }
                }
                FeatureType::LineString => {
                    for line in feature.geometry.iter() {
                        if let Some(start) = line.first() {
                            let anchor = Anchor::new(f32::from(start.x), f32::from(start.y), 0.0, None);
                            self.add_symbol_instance(&symbols, feature, anchor, &symbols.quads(line.clone()));
                        }
                    }
                }
                FeatureType::Point => {
                    for &point in feature.geometry.iter().flat_map(|points| points.iter()) {
                        let anchor = Anchor::new(f32::from(point.x), f32::from(point.y), 0.0, None);
                        self.add_symbol_instance(&symbols, feature, anchor, &symbols.quads(vec![point]));
                    }
                }
                FeatureType::Unknown => {}
            },
        }
    }

    /// Adds `symbols`' symbol at `anchor`. Outside tile mode, only a symbol
    /// anchored inside the tile is added: its neighbors add the others. Tile
    /// mode adds them all, so that the tile draws the symbols that reach
    /// into it and collides with its neighbors' symbols.
    fn add_symbol_instance(
        &mut self,
        symbols: &FeatureSymbols<'_>,
        feature: &SymbolFeature,
        anchor: Anchor,
        quads: &Arc<SymbolQuadSet>,
    ) {
        let tile = 0.0..EXTENT as f32;
        if self.mode != MapMode::Tile && !(tile.contains(&anchor.point.x) && tile.contains(&anchor.point.y)) {
            return;
        }
        self.symbols.push(symbols.symbol(anchor, Arc::clone(quads), &mut self.symbol_instance_pools));
        if self.sort_features_by_key {
            let len = self.symbols.len();
            match self.sort_key_ranges.last_mut() {
                Some(last) if last.sort_key == feature.sort_key => last.end = len,
                _ => self.sort_key_ranges.push(SortKeyRange { sort_key: feature.sort_key, start: len - 1, end: len }),
            }
        }
    }

    /// Whether a label of `text` was added within `repeat_distance` of
    /// `anchor`; if not, `anchor` counts from now on.
    fn anchor_is_too_close(&mut self, text: &str, repeat_distance: f32, anchor: &Anchor) -> bool {
        let anchors = self.compare_text.get_or_insert_with(text, Vec::new);
        if anchors.iter().any(|other| anchor.point.distance(other.point) < repeat_distance) {
            return true;
        }
        anchors.push(*anchor);
        false
    }

    /// The distance along `line` from `anchor` to each of its vertices, on
    /// either side of the anchor's segment; zeros for an anchor without one.
    pub(crate) fn tile_distances(line: &[GeometryCoordinate], anchor: &Anchor) -> Vec<f32> {
        let mut distances = vec![0.0; line.len()];
        let Some(segment) = anchor.segment.filter(|&segment| segment < line.len()) else {
            return distances;
        };
        let point = |i: usize| Vec2::new(f32::from(line[i].x), f32::from(line[i].y));
        let mut walk = |vertices: &mut dyn Iterator<Item = usize>| {
            let (mut sum, mut previous) = (0.0, anchor.point);
            for i in vertices {
                sum += previous.distance(point(i));
                distances[i] = sum;
                previous = point(i);
            }
        };
        walk(&mut (segment + 1..line.len()));
        walk(&mut (0..=segment).rev());
        distances
    }
}

/// Adds laid-out symbols' quads to a bucket's buffers, as runs.
struct BucketWriter<'a> {
    state: &'a mut SymbolBucketState,
    paint_properties: &'a mut VecMap<String, PaintProperties>,
    text_size: &'a SymbolSizeBinder,
    icon_size: &'a SymbolSizeBinder,
    /// The text's paint varies by section, so binders take each section's.
    section_overrides: bool,
    allow_vertical_placement: bool,
    canonical: &'a CanonicalTileID,
}

/// What the quads of a run share.
#[derive(Clone, Copy)]
struct RunInfo {
    anchor: Anchor,
    height_offset: f32,
    size: Range<f32>,
    offset: [f32; 2],
    writing_modes: WritingMode,
    angle: f32,
    line: PoolRange,
    tile_distances: PoolRange,
    sort_key: f32,
}

impl BucketWriter<'_> {
    /// Adds `laid_out`'s runs, and returns its symbol with them set.
    fn add(&mut self, laid_out: LaidOutSymbol, feature: &SymbolFeature) -> SymbolInstance {
        let LaidOutSymbol { mut symbol, quads, text_offset, icon_offset, .. } = laid_out;
        let (anchor, height_offset) = (symbol.anchor, symbol.height_offset);
        let allow_vertical_placement = self.allow_vertical_placement;
        let run = |size, offset, writing_modes, line, tile_distances| RunInfo {
            anchor,
            height_offset,
            size,
            offset,
            writing_modes,
            angle: if allow_vertical_placement && writing_modes == WritingMode::Vertical {
                std::f32::consts::FRAC_PI_2
            } else {
                0.0
            },
            line,
            tile_distances,
            sort_key: feature.sort_key,
        };

        // Icons go first, so that a text's runs follow its icon's.
        if let Some(icon) = &mut symbol.icon {
            let size = self.icon_size.vertex_size_data(feature);
            let buffer = if icon.sdf { &mut self.state.sdf_icon } else { &mut self.state.icon };
            let mut add = |run_quads: &[SymbolQuad], writing_modes| {
                let line = PoolRange::extend(&mut buffer.pools.lines, quads.line.iter().copied());
                let info = run(size, icon_offset, writing_modes, line, PoolRange::default());
                add_run(buffer, &info, run_quads, |_, _| {})
            };
            icon.run = add(quads.icon.as_deref().unwrap_or_default(), WritingMode::None);
            icon.vertical_run = quads.vertical_icon.as_deref().map(|run_quads| add(run_quads, WritingMode::Vertical));
            let length = buffer.attribute_data.len();
            for binders in self.paint_properties.values_mut() {
                binders.icon_binders.populate_vertex_vectors(
                    feature,
                    length,
                    symbol.data_feature_index,
                    &ImagePositions::default(),
                    None,
                    self.canonical,
                    &Value::Null,
                );
            }
        }

        if let Some(text) = &mut symbol.text
            && let Some(formatted_text) = &feature.formatted_text
        {
            let size = self.text_size.vertex_size_data(feature);
            let BucketWriter { state, paint_properties, canonical, section_overrides, .. } = self;
            let buffer = &mut state.text;
            // The text's runs share its line and the distances along it.
            let distances = SymbolLayout::tile_distances(&quads.line, &anchor);
            let tile_distances = PoolRange::extend(&mut buffer.pools.tile_distances, distances);
            let line = PoolRange::extend(&mut buffer.pools.lines, quads.line.iter().copied());

            let mut section = None;
            let mut populate_section = |section: usize, length: usize| {
                let section = section_options_to_value(&formatted_text.sections()[section]);
                for binders in paint_properties.values_mut() {
                    binders.text_binders.populate_vertex_vectors(
                        feature,
                        length,
                        feature.index,
                        &ImagePositions::default(),
                        None,
                        canonical,
                        &section,
                    );
                }
            };
            let mut add = |run_quads: &[SymbolQuad], writing_modes| {
                let info = run(size, text_offset, writing_modes, line, tile_distances);
                add_run(buffer, &info, run_quads, |quad, length| {
                    if *section_overrides {
                        if let Some(previous) = section
                            && previous != quad.section_index
                        {
                            populate_section(previous, length);
                        }
                        section = Some(quad.section_index);
                    }
                })
            };
            let writing_modes = quads.text_writing_modes();
            if quads.single_line {
                text.runs.horizontal = [Some(add(&quads.horizontal[Justification::Right as usize], writing_modes)); 3];
            } else {
                for justification in Justification::ALL {
                    let run_quads = &quads.horizontal[justification as usize];
                    if !run_quads.is_empty() {
                        text.runs.horizontal[justification as usize] = Some(add(run_quads, writing_modes));
                    }
                }
            }
            if !quads.vertical.is_empty() {
                text.runs.vertical = Some(add(&quads.vertical, WritingMode::Vertical));
            }
            let length = buffer.attribute_data.len();
            populate_section(section.unwrap_or(0), length);
        }
        symbol
    }
}

/// Adds `quads` to `buffer` as one run, and returns the run's index.
/// `before_quad` takes each quad, and the buffer's vertex count before its
/// vertices.
fn add_run(
    buffer: &mut Buffer,
    run: &RunInfo,
    quads: &[SymbolQuad],
    mut before_quad: impl FnMut(&SymbolQuad, usize),
) -> u32 {
    let mut glyph_offsets = PoolRange::at_end(&buffer.pools.glyph_offsets);
    let mut start_index = None;
    for quad in quads {
        before_quad(quad, buffer.attribute_data.len());
        let index = add_quad(buffer, run, quad);
        start_index.get_or_insert(index);
        glyph_offsets.push(&mut buffer.pools.glyph_offsets, quad.glyph_offset.x);
    }
    buffer.placed_symbols.push(PlacedSymbol {
        anchor_point: run.anchor.point,
        segment: run.anchor.segment.unwrap_or(0),
        lower_size: run.size.min,
        upper_size: run.size.max,
        line_offset: run.offset,
        writing_modes: run.writing_modes,
        line: run.line,
        tile_distances: run.tile_distances,
        glyph_offsets,
        start_index: start_index.unwrap_or(0),
        angle: run.angle,
        height_offset: run.height_offset,
        hidden: false,
    });
    (buffer.placed_symbols.len() - 1) as u32
}

/// Adds the vertices and triangles of `quad`, and returns the index of its
/// first vertex in its segment.
fn add_quad(buffer: &mut Buffer, run: &RunInfo, quad: &SymbolQuad) -> usize {
    let tex = quad.tex;
    let (tl, br) = (quad.pixel_offset_tl, quad.pixel_offset_br);
    let corner = |offset, tex, pixel_offset| QuadCorner { offset, tex, pixel_offset };
    let corners = [
        corner(quad.tl, [tex.x, tex.y], tl),
        corner(quad.tr, [tex.x + tex.w, tex.y], Vec2::new(br.x, tl.y)),
        corner(quad.bl, [tex.x, tex.y + tex.h], Vec2::new(tl.x, br.y)),
        corner(quad.br, [tex.x + tex.w, tex.y + tex.h], br),
    ];
    let anchor = run.anchor.point;
    let first = buffer.attribute_data.len();
    buffer
        .attribute_data
        .get_mut()
        .extend(corners.map(|corner| SymbolBucket::layout_vertex(anchor, quad, corner, run.size, run.height_offset)));
    // Placement writes these before the quad draws; until then they agree
    // in count with the layout vertices.
    buffer.dynamic_attribute_data.extend([SymbolBucket::dynamic_layout_attributes(anchor, 0.0, 0.0); 4]);
    buffer.opacity_attribute_data.extend([SymbolBucket::opacity_attributes(true, 1.0); 4]);
    let mut triangles = Indexed { indexes: &mut buffer.triangles, segments: &mut buffer.segments };
    triangles.add_keyed(buffer.attribute_data.get_mut(), first, run.sort_key, [[0, 1, 2], [1, 2, 3]])
}

impl Layout<Symbol> for SymbolLayout {
    fn has_dependencies(&self) -> bool {
        !self.features.is_empty()
    }

    /// With `resolved`, applies the shaping of the text that font faces
    /// draw and prepares the symbols, and has no bucket without symbols.
    fn create_bucket(
        &mut self,
        resolved: Option<&Resolved>,
        _feature_index: &mut FeatureIndexBuilder,
        build: &BuildParams,
    ) -> Result<Option<SymbolBucket>, Error> {
        if let Some(resolved) = resolved {
            if self.has_font_text {
                self.finalize_symbols(&resolved.shaping);
            }
            self.prepare_symbols(&resolved.glyphs, &resolved.icons);
            if self.symbols.is_empty() {
                return Ok(None);
            }
        }
        Ok(Some(self.build_bucket(build.first_load, &build.canonical)))
    }

    fn layers(&self) -> &VecMap<String, Arc<EvaluatedLayer<Symbol>>> {
        &self.layer_paint_properties
    }
}

impl SymbolLayout {
    /// Applies the shaping of the text that font files draw,
    /// before `prepare_symbols`.
    fn finalize_symbols(&mut self, results: &ShapeResults) {
        let unshaped = Arc::new(ShapedText::default());
        for feature in &mut self.features {
            if feature.geometry.is_empty() {
                continue;
            }

            let Some(formatted_text) = &feature.formatted_text else {
                continue;
            };
            if !formatted_text.has_font_sections() || formatted_text.is_shaped() {
                continue;
            }

            let mut shaped_string = TaggedString::default();

            let sections = formatted_text.sections();
            let style_text = formatted_text.styled_text();

            let mut sub_string: Vec<char> = Vec::new();
            let mut section_index = style_text.styles.first().copied().unwrap_or(0);

            let apply_sub_string =
                |sub_string: &[char], section_index: SectionIndex, shaped_string: &mut TaggedString| {
                    if sub_string.is_empty() {
                        return;
                    }
                    let section = &sections[section_index as usize];
                    if let Some(image_id) = &section.image_id {
                        shaped_string.add_image_section(image_id, section.vertical_align);
                    } else if section.type_ == GlyphIDType::FontPBF {
                        shaped_string.add_text_section(sub_string, section.clone());
                    } else {
                        let shaped = results
                            .get(&section.font_stack)
                            .and_then(|types| types.get(&section.type_))
                            .and_then(|strings| strings.get(sub_string))
                            .unwrap_or(&unshaped);

                        shaped_string.add_shaped_section(shaped, starts_right_to_left(sub_string), section.clone());
                    }
                };

            for (ch, sec) in style_text.text.iter().zip(style_text.styles.iter()) {
                if section_index != *sec {
                    apply_sub_string(&sub_string, section_index, &mut shaped_string);

                    sub_string.clear();
                    section_index = *sec;
                }

                sub_string.push(*ch);
            }

            apply_sub_string(&sub_string, section_index, &mut shaped_string);

            shaped_string.set_unshaped_text(style_text.text.clone());
            feature.formatted_text = Some(shaped_string);
        }

        self.has_font_text = false;
    }

    /// Prepares the symbols once their glyphs and images are available.
    #[expect(
        clippy::unnecessary_unwrap,
        reason = "the text is borrowed where it is used, as `feature` is also passed on in between"
    )]
    fn prepare_symbols(&mut self, glyphs: &GlyphPositions, icons: &ImagePositions) {
        let layout = Arc::clone(&self.layout);
        let zoom = self.zoom;
        let canonical = self.canonical_id;
        let is_point_placement = *layout.symbol_placement.uniform() == SymbolPlacementType::Point;
        let text_along_line =
            *layout.text_rotation_alignment.uniform() != TextRotationAlignmentType::Viewport && !is_point_placement;

        let mut features = std::mem::take(&mut self.features);
        for (feature_index, feature) in features.iter_mut().enumerate() {
            if feature.geometry.is_empty() {
                continue;
            }

            let mut shaped_text_orientations = ShapedTextOrientations::default();
            let mut shaped_icon: Option<PositionedIcon> = None;
            let mut text_offset = [0.0_f32, 0.0_f32];
            let layout_text_size = layout.text_size.evaluate(&feature_context(zoom + 1.0, feature, &canonical));
            let layout_text_size_at_bucket_zoom_level =
                layout.text_size.evaluate(&feature_context(zoom, feature, &canonical));
            let layout_icon_size = layout.icon_size.evaluate(&feature_context(zoom + 1.0, feature, &canonical));

            // if feature has text, shape the text
            if feature.formatted_text.is_some() && layout_text_size > 0.0 {
                let line_height = *layout.text_line_height.uniform() * ONE_EM;
                let text = feature.formatted_text.as_ref().expect("text");
                let spacing = if i18n::allows_letter_spacing(text.logical_text()) {
                    layout.text_letter_spacing.evaluate(&feature_context(zoom, feature, &canonical)) * ONE_EM
                } else {
                    0.0
                };
                let max_width = if is_point_placement {
                    layout.text_max_width.evaluate(&feature_context(zoom, feature, &canonical)) * ONE_EM
                } else {
                    0.0
                };

                let variable_anchor_offsets = self.text_variable_anchor_offset(feature);
                let text_anchor = layout.text_anchor.evaluate(&feature_context(zoom, feature, &canonical));
                if variable_anchor_offsets.as_ref().is_none_or(VariableAnchorOffsetCollection::is_empty) {
                    // Layers with variable anchors use the
                    // `text-radial-offset` property and the [x, y] offset
                    // vector is calculated at placement time instead of
                    // layout time
                    let radial_offset = layout.text_radial_offset.evaluate(&feature_context(zoom, feature, &canonical));
                    if radial_offset > 0.0 {
                        // The style spec says don't use `text-offset` and
                        // `text-radial-offset` together but doesn't actually
                        // specify what happens if you use both. We go with
                        // the radial offset.
                        text_offset = evaluate_radial_offset(text_anchor, radial_offset * ONE_EM);
                    } else {
                        let offset = layout.text_offset.evaluate(&feature_context(zoom, feature, &canonical));
                        text_offset = [offset[0] * ONE_EM, offset[1] * ONE_EM];
                    }
                }
                let mut text_justify = if text_along_line {
                    TextJustifyType::Center
                } else {
                    layout.text_justify.evaluate(&feature_context(zoom, feature, &canonical))
                };

                let allow_vertical_placement = self.allow_vertical_placement;
                let apply_shaping = |formatted_text: &TaggedString,
                                     writing_mode: TextWritingModeType,
                                     anchor: SymbolAnchorType,
                                     justify: TextJustifyType,
                                     offset: &[f32; 2]|
                 -> Shaping {
                    let options = ShapingOptions {
                        max_width,
                        line_height,
                        anchor,
                        justify,
                        spacing,
                        offset: *offset,
                        writing_mode,
                        allow_vertical_placement,
                        text_size: layout_text_size,
                        text_size_at_bucket_zoom: layout_text_size_at_bucket_zoom_level,
                    };
                    shape_text(formatted_text, &options, glyphs, icons)
                };

                let formatted_text = feature.formatted_text.as_mut().expect("text");

                // If this layer uses text-variable-anchor, generate shapings
                // for all justification possibilities.
                if !text_along_line
                    && let Some(variable_anchor_offsets) = variable_anchor_offsets.as_ref().filter(|v| !v.is_empty())
                {
                    let mut justifications: Vec<TextJustifyType> = Vec::new();
                    if text_justify == TextJustifyType::Auto {
                        for anchor_offset in variable_anchor_offsets {
                            justifications.push(anchor_justification(anchor_offset.anchor_type));
                        }
                    } else {
                        justifications.push(text_justify);
                    }
                    for justification in justifications.into_iter().filter_map(Justification::of) {
                        if !shaped_text_orientations.get(justification).is_empty() {
                            continue;
                        }
                        // If using text-variable-anchor for the layer, we use
                        // a center anchor for all shapings and apply the
                        // offsets for the anchor in the placement step.
                        let shaping = apply_shaping(
                            formatted_text,
                            TextWritingModeType::Horizontal,
                            SymbolAnchorType::Center,
                            justification.into(),
                            &text_offset,
                        );
                        if !shaping.is_empty() {
                            let single = shaping.lines.len() == 1;
                            *shaped_text_orientations.get_mut(justification) = shaping;
                            if single {
                                shaped_text_orientations.single_line = true;
                                break;
                            }
                        }
                    }

                    // Vertical point label shaping where the layer allows
                    // vertical placement.
                    if allow_vertical_placement && formatted_text.allows_vertical_writing_mode() {
                        formatted_text.verticalize_punctuation();
                        // Vertical POI label placement is meant to be used
                        // for scripts that support vertical writing mode,
                        // thus, default style::TextJustifyType::Left
                        // justification is used. If Latin scripts would need
                        // to be supported, this should take into account
                        // other justifications.
                        shaped_text_orientations.vertical = apply_shaping(
                            formatted_text,
                            TextWritingModeType::Vertical,
                            text_anchor,
                            TextJustifyType::Left,
                            &text_offset,
                        );
                    }
                } else {
                    if text_justify == TextJustifyType::Auto {
                        text_justify = anchor_justification(text_anchor);
                    }

                    // Horizontal point or line label.
                    let shaping = apply_shaping(
                        formatted_text,
                        TextWritingModeType::Horizontal,
                        text_anchor,
                        text_justify,
                        &text_offset,
                    );
                    if !shaping.is_empty() {
                        *shaped_text_orientations.get_mut(Justification::Right) = shaping;
                    }

                    // Vertical point label shaping where the layer allows
                    // vertical placement.
                    if allow_vertical_placement && formatted_text.allows_vertical_writing_mode() {
                        formatted_text.verticalize_punctuation();
                        shaped_text_orientations.vertical = apply_shaping(
                            formatted_text,
                            TextWritingModeType::Vertical,
                            text_anchor,
                            TextJustifyType::Left,
                            &text_offset,
                        );
                    }

                    // Verticalized line label.
                    if text_along_line && formatted_text.allows_vertical_writing_mode() {
                        formatted_text.verticalize_punctuation();
                        shaped_text_orientations.vertical = apply_shaping(
                            formatted_text,
                            TextWritingModeType::Vertical,
                            text_anchor,
                            text_justify,
                            &text_offset,
                        );
                    }
                }
            }

            // if feature has icon, get sprite atlas position
            if let Some(icon) = &feature.icon
                && let Some(image) = icons.get(icon.id())
            {
                shaped_icon = Some(PositionedIcon::new(
                    image,
                    layout.icon_offset.evaluate(&feature_context(zoom, feature, &canonical)),
                    layout.icon_anchor.evaluate(&feature_context(zoom, feature, &canonical)),
                ));
                if image.pixel_ratio != self.pixel_ratio || layout.icon_rotate.constant_or(&1.0) != 0.0 {
                    self.icons_need_linear = true;
                }
            }

            // Add the feature if it has either shaped text or an icon.
            let text = shaped_text_orientations.any_horizontal();
            self.icons_in_text |= text.is_some_and(|text| text.icons_in_text);
            if text.is_some() || shaped_icon.is_some() {
                let shaped = ShapedFeature {
                    text: shaped_text_orientations,
                    icon: shaped_icon,
                    text_offset,
                    text_size: layout_text_size,
                    icon_size: layout_icon_size,
                };
                self.add_feature(feature_index, feature, shaped, icons);
            }

            feature.geometry.clear();
        }
        self.features = features;

        self.compare_text.clear();
    }

    fn build_bucket(&mut self, first_load: bool, canonical: &CanonicalTileID) -> SymbolBucket {
        let style = SymbolBucketStyle {
            layout: Arc::clone(&self.layout),
            paint_properties: &self.layer_paint_properties,
            text_size: &self.text_size,
            icon_size: &self.icon_size,
            zoom: self.zoom,
            tile_pixel_ratio: self.tile_pixel_ratio,
            leader_id: Arc::clone(&self.bucket_leader_id),
        };
        let flags = SymbolLayoutFlags {
            icons_need_linear: self.icons_need_linear,
            sort_features_by_y: self.sort_features_by_y,
            icons_in_text: self.icons_in_text,
            allow_vertical_placement: self.allow_vertical_placement,
            placement_modes: std::mem::take(&mut self.placement_modes),
        };
        let pools = std::mem::take(&mut self.symbol_instance_pools);
        let mut bucket = SymbolBucket::new(style, flags, Vec::new(), pools, std::mem::take(&mut self.sort_key_ranges));
        let mut state = std::mem::take(&mut bucket.state);
        let mut paint_properties = std::mem::take(&mut bucket.paint_properties);
        let mut writer = BucketWriter {
            state: &mut state,
            paint_properties: &mut paint_properties,
            text_size: &bucket.text_size_binder,
            icon_size: &bucket.icon_size_binder,
            section_overrides: bucket.has_format_section_overrides(),
            allow_vertical_placement: self.allow_vertical_placement,
            canonical,
        };
        let symbols: Vec<SymbolInstance> = std::mem::take(&mut self.symbols)
            .into_iter()
            .map(|laid_out| {
                let feature = &self.features[laid_out.feature];
                writer.add(laid_out, feature)
            })
            .collect();
        bucket.max_height_offset = max_height_offset(&symbols);
        state.symbol_instances = symbols;
        state.just_reloaded = !first_load;
        bucket.state = state;
        bucket.paint_properties = paint_properties;
        bucket
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use mln_style::i18n::char_in_supported_script;

    use super::{BASELINE_OFFSET, GlyphTypeTable, SymbolAnchorType, SymbolLayout, evaluate_radial_offset};
    use crate::text::glyph::{FontFace, FontFaces, GlyphIDType};

    #[test]
    fn variable_offset_distinguishes_cartesian_and_radial_offsets() {
        for anchor in [
            SymbolAnchorType::Center,
            SymbolAnchorType::Top,
            SymbolAnchorType::Bottom,
            SymbolAnchorType::Left,
            SymbolAnchorType::Right,
            SymbolAnchorType::TopLeft,
            SymbolAnchorType::TopRight,
            SymbolAnchorType::BottomLeft,
            SymbolAnchorType::BottomRight,
        ] {
            for radial_offset in [-12.0, 0.0, 12.0] {
                assert_eq!(
                    SymbolLayout::evaluate_variable_offset(anchor, None, radial_offset),
                    evaluate_radial_offset(anchor, radial_offset),
                );
            }
        }
        assert_eq!(
            SymbolLayout::evaluate_variable_offset(SymbolAnchorType::TopRight, Some([-12.0, -24.0]), 0.0),
            [-12.0, 24.0 - BASELINE_OFFSET],
        );
        assert_eq!(
            SymbolLayout::evaluate_variable_offset(SymbolAnchorType::TopLeft, Some([12.0, f32::MAX]), 0.0),
            [12.0, f32::MAX],
        );
    }

    /// The faces' rule, read off them for each character.
    fn scan(c: char, stack: &[String], faces: &FontFaces, glyph_pbfs: bool, previous: GlyphIDType) -> GlyphIDType {
        if mln_style::i18n::is_variation_selector(c)
            || (previous != GlyphIDType::FontPBF && mln_style::i18n::extends_previous(c))
        {
            return previous;
        }
        let covers = |face: &FontFace| face.ranges.iter().any(|&(first, last)| (first..=last).contains(&u32::from(c)));
        let named =
            stack.iter().flat_map(|name| faces.iter().filter(move |face| !name.is_empty() && face.name == *name));
        // PBFs draw the text they can, before a fallback face.
        let fallbacks =
            faces.iter().filter(|face| face.name.is_empty() && !(glyph_pbfs && char_in_supported_script(c)));
        let system = !glyph_pbfs && faces.iter().any(|face| face.type_ == GlyphIDType::SYSTEM);
        let rest = if system && !mln_style::i18n::breaks_or_stands_upright(c) {
            GlyphIDType::SYSTEM
        } else {
            GlyphIDType::FontPBF
        };
        named.chain(fallbacks).find(|face| covers(face)).map_or(rest, |face| face.type_)
    }

    fn face(name: &str, type_: u16, ranges: &[(u32, u32)]) -> FontFace {
        let mut face = FontFace::new(name, "font.ttf", ranges.to_vec());
        face.type_ = GlyphIDType::face(NonZeroU16::new(type_).unwrap());
        face
    }

    #[test]
    fn glyph_type_table_matches_the_faces_for_every_character() {
        let mut faces: FontFaces = vec![
            face("", 1, &[(0x3000, 0x30FF), (0x4E00, 0x9FFF)]),
            face("Noto Sans", 2, &[(0x0600, 0x06FF), (0x4E00, 0x4FFF)]),
            face("Open Sans", 3, &[(0x0000, 0x007F), (0x0500, 0x0650)]),
            face("Noto Sans", 4, &[(0x0700, 0x07FF), (0xFE00, 0xFE00)]),
            face("", 5, &[(0x0640, 0x0800), (0xF000, 0x1_0FFFF)]),
            face("", 6, &[(0x0900, 0x0800), (0x1_0000, 0x1_0FFF)]),
            face("", 1, &[(0x3100, 0x31FF)]),
            face("Noto Sans", 8, &[(0x1_F600, 0x1_F64F), (0x2_0000, 0x2_A6DF)]),
            face("Open Sans", 9, &[(0x1_F000, 0xFFFF_FFFF)]),
        ];
        let stacks: [&[&str]; 5] =
            [&[], &["Open Sans"], &["Noto Sans", "Open Sans"], &["Open Sans", "Noto Sans"], &["Missing", ""]];
        let check = |faces: &FontFaces| {
            for (stack, glyph_pbfs) in stacks.iter().flat_map(|stack| [(stack, true), (stack, false)]) {
                let stack: Vec<String> = stack.iter().map(|name| (*name).to_owned()).collect();
                let table = GlyphTypeTable::new(&stack, faces, glyph_pbfs);
                // The BMP, and around the bounds beyond it.
                let beyond = [0x1_0000, 0x1_0FFF, 0x1_F000, 0x1_F600, 0x1_F64F, 0x2_0000, 0x2_A6DF, 0x10_FFFF]
                    .into_iter()
                    .flat_map(|bound: u32| bound.saturating_sub(1)..=bound + 1);
                for c in (0..=0xFFFF).chain(beyond).filter_map(char::from_u32) {
                    for previous in [GlyphIDType::FontPBF, GlyphIDType::face(NonZeroU16::new(7).unwrap())] {
                        let expected = scan(c, &stack, faces, glyph_pbfs, previous);
                        assert_eq!(table.get(c, previous), expected, "{c:?} in {stack:?}, PBFs {glyph_pbfs}");
                    }
                }
            }
        };
        check(&faces);
        faces.push(FontFace::system(vec![(0x0900, 0x097F), (0x1780, 0x17FF)]));
        check(&faces);
    }

    #[test]
    fn fallback_faces_draw_only_what_pbfs_cannot() {
        let faces: FontFaces = vec![face("", 1, &[(0x0000, 0xFFFF)])];
        let table = GlyphTypeTable::new(&["Open Sans".to_owned()], &faces, true);
        let devanagari = GlyphIDType::face(NonZeroU16::new(1).unwrap());
        assert_eq!(table.get('a', GlyphIDType::FontPBF), GlyphIDType::FontPBF);
        assert_eq!(table.get('\u{4E2D}', GlyphIDType::FontPBF), GlyphIDType::FontPBF);
        assert_eq!(table.get('\u{0915}', GlyphIDType::FontPBF), devanagari);
    }

    #[test]
    fn the_devices_fonts_draw_what_pbfs_cannot_or_all_without_pbfs() {
        let faces: FontFaces = vec![FontFace::system(vec![(0x0900, 0x097F)])];
        let stack = ["Open Sans".to_owned()];
        let with_pbfs = GlyphTypeTable::new(&stack, &faces, true);
        assert_eq!(with_pbfs.get('a', GlyphIDType::FontPBF), GlyphIDType::FontPBF);
        assert_eq!(with_pbfs.get('\u{0915}', GlyphIDType::FontPBF), GlyphIDType::SYSTEM);
        // A script the device has no font for stays with the PBFs.
        assert_eq!(with_pbfs.get('\u{1780}', GlyphIDType::FontPBF), GlyphIDType::FontPBF);
        let without_pbfs = GlyphTypeTable::new(&stack, &faces, false);
        for c in ['a', '\u{0915}', '\u{1780}', '\u{0627}'] {
            assert_eq!(without_pbfs.get(c, GlyphIDType::FontPBF), GlyphIDType::SYSTEM, "{c:?}");
        }
        // Line breaking and vertical writing place these on their own.
        for c in [' ', '\n', '-', '\u{4E2D}', '\u{AC00}'] {
            assert_eq!(without_pbfs.get(c, GlyphIDType::SYSTEM), GlyphIDType::FontPBF, "{c:?}");
        }
        // A joiner stays with the shaped text before it.
        assert_eq!(with_pbfs.get('\u{200D}', GlyphIDType::SYSTEM), GlyphIDType::SYSTEM);
        assert_eq!(with_pbfs.get('\u{200D}', GlyphIDType::FontPBF), GlyphIDType::FontPBF);
    }

    #[test]
    fn variation_selectors_stay_with_their_base() {
        let faces: FontFaces = vec![face("Noto Sans", 2, &[(0x4E00, 0x9FFF)])];
        let table = GlyphTypeTable::new(&["Noto Sans".to_owned()], &faces, true);
        let han = GlyphIDType::face(NonZeroU16::new(2).unwrap());
        for selector in ['\u{FE00}', '\u{FE0F}', '\u{E0100}'] {
            assert_eq!(table.get(selector, han), han, "{selector:?}");
        }
    }

    #[test]
    fn glyph_type_table_is_empty_without_a_face_for_the_stack() {
        let faces: FontFaces = vec![face("Noto Sans", 2, &[(0x0600, 0x06FF)])];
        let table = GlyphTypeTable::new(&["Open Sans".to_owned()], &faces, true);
        assert_eq!(table.spans, Vec::new());
        assert_eq!(table.get('\u{0620}', GlyphIDType::FontPBF), GlyphIDType::FontPBF);
    }
}
