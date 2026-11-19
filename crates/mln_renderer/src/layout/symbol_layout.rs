// The layout keeps the tile's `SourceLayer` (shared tile data) and owned
// `SymbolFeature`s (see symbol_feature.rs), so it can wait for glyphs and
// images across worker messages.
//

use std::borrow::Cow;
use std::cmp::Ordering;
use std::range::RangeInclusive;
use std::sync::Arc;

use geometry::Point;
use glam::Vec2;
use mln_expression::Value;
use mln_expression::formatted::K_FORMATTED_SECTION_TEXT_COLOR;
use mln_map_types::MapMode;
use mln_style::evaluated::Evaluated;
use mln_style::expression::{EvaluationContext, GlobalStateMap};
use mln_style::layers::Symbol;
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style::property_value::PropertyValue;
use mln_style::{PropertyEvaluationParameters, StyleValue as _};
use mln_style_types::image::{ImagePositions, ImageType};
use mln_style_types::{
    AlignmentType, AnchorOffsetPair, IconTextFitType, SymbolAnchorType, SymbolPlacementType, SymbolZOrderType,
    TextJustifyType, TextRotationAlignmentType, TextTransformType, TextVariableAnchorType, TextWritingModeType,
    VariableAnchorOffsetCollection,
};
use mln_text::glyph::{GlyphID, GlyphIDType, GlyphPositions, Shaping, WritingModeType};
use mln_text::shaping::{PositionedIcon, get_anchor_justification, get_shaping};
use mln_text::tagged_string::SectionOptions;
use mln_text::{ShapedText, TaggedString, shape_arabic, starts_right_to_left};
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::{GeometryCoordinate, GeometryCoordinates, GeometryTileFeature};
use mln_util::constants::{EXTENT, ONE_EM, TILE_SIZE_D};
use mln_util::error::Error;
use mln_util::geometry::FeatureType;
use mln_util::i18n;
use mln_util::range::Range;
use mln_vec_map::VecMap;

use crate::bucket_parameters::BucketParameters;
use crate::buckets::symbol_bucket::{
    Buffer, CollisionBuffer, PaintProperties, PlacedSymbol, SortKeyRange, SymbolBucket, SymbolBucketState,
};
use crate::geometry::anchor::{Anchor, Anchors};
use crate::geometry::feature_index::{FeatureIndex, IndexedSubfeature};
use crate::layer_type::EvaluatedLayer;
use crate::layout::clip_lines::clip_lines;
use crate::layout::layout::{BuildParams, Layout, LayoutParameters, SourceLayer, layers_by_id};
use crate::layout::merge_lines::merge_lines;
use crate::layout::symbol_feature::SymbolFeature;
use crate::layout::symbol_instance::{
    ShapedTextOrientations, SymbolContent, SymbolInstance, SymbolInstancePools, SymbolInstanceSharedData,
};
use crate::layout::tile_layout::Resolved;
use crate::text::collision_index::OverlapMode;
use crate::text::get_anchors::{get_anchors, get_center_anchor};
use crate::text::glyph_store::ShapeResults;
use crate::text::quads::{SymbolQuad, SymbolQuads};
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
/// them ([`FontFace::system`](mln_text::glyph::FontFace::system)), except
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
    fn new(stack: &[String], faces: &mln_text::glyph::FontFaces, glyph_pbfs: bool) -> Self {
        let named =
            stack.iter().flat_map(|name| faces.iter().filter(move |face| !name.is_empty() && face.name == *name));
        let fallbacks = faces.iter().filter(|face| face.name.is_empty());
        let candidates: Vec<&mln_text::glyph::FontFace> = named.chain(fallbacks).collect();
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
    faces: Option<Arc<mln_text::glyph::FontFaces>>,
    glyph_pbfs: bool,
    tables: Vec<(Vec<String>, GlyphTypeTable)>,
}

impl GlyphTypeTables {
    fn new(faces: Option<Arc<mln_text::glyph::FontFaces>>, glyph_pbfs: bool) -> Self {
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
    let mut result = mln_expression::value::ObjectType::new();
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

fn dist(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let dx = bx - ax;
    let dy = by - ay;
    (dx * dx + dy * dy).sqrt()
}

// The radial offset is to the edge of the text box. In the horizontal
// direction, the edge of the text box is where glyphs start. But in the
// vertical direction, the glyphs appear to "start" at the baseline. We don't
// actually load baseline data, but we assume an offset of ONE_EM - 17, as
// the shaping does.
const BASELINE_OFFSET: f32 = 7.0;

/// Any horizontal shaping: this is used for collision purposes, and all the
/// justifications have the same collision box.
fn default_horizontal_shaping(shaped_text_orientations: &ShapedTextOrientations) -> &Shaping {
    if shaped_text_orientations.right().is_some() {
        return shaped_text_orientations.right();
    }
    if shaped_text_orientations.center.is_some() {
        return &shaped_text_orientations.center;
    }
    if shaped_text_orientations.left.is_some() {
        return &shaped_text_orientations.left;
    }
    &shaped_text_orientations.horizontal
}

fn shaping_for_text_justify_type(
    shaped_text_orientations: &mut ShapedTextOrientations,
    type_: TextJustifyType,
) -> &mut Shaping {
    match type_ {
        TextJustifyType::Right => shaped_text_orientations.right_mut(),
        TextJustifyType::Left => &mut shaped_text_orientations.left,
        TextJustifyType::Center => &mut shaped_text_orientations.center,
        TextJustifyType::Auto => {
            debug_assert!(false);
            &mut shaped_text_orientations.horizontal
        }
    }
}

fn evaluate_radial_offset(anchor: SymbolAnchorType, radial_offset: f32) -> [f32; 2] {
    let mut result = [0.0_f32, 0.0_f32];
    let radial_offset = mln_math::fmaxf(radial_offset, 0.0); // Ignore negative offset.
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

pub(crate) struct SymbolLayout {
    pub(crate) layer_paint_properties: VecMap<String, Arc<EvaluatedLayer<Symbol>>>,
    /// Shared by every symbol's `IndexedSubfeature`, which placement clones
    /// for each symbol it places.
    pub(crate) bucket_leader_id: Arc<str>,
    pub(crate) symbol_instances: Vec<SymbolInstance>,
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
            symbol_instances: Vec::new(),
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
                    // The text ends at its first NUL, as in C++.
                    let chars: Vec<char> = u8string.chars().take_while(|&c| c != '\0').collect();
                    let section_scale = section.font_scale.unwrap_or(1.0);
                    let section_font_stack = section.font_stack.clone().unwrap_or_else(|| base_font_stack.clone());
                    let glyph_types = glyph_type_tables.get(&section_font_stack);

                    let mut sub_string: Vec<char> = Vec::new();
                    // An empty string's first character is the terminating NUL.
                    let mut sub_string_type =
                        glyph_types.get(chars.first().copied().unwrap_or('\0'), GlyphIDType::FontPBF);

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
                            formatted_text.add_text_section(
                                sub_string,
                                section_scale,
                                &section_font_stack,
                                sub_string_type,
                                key_section,
                                section.text_color,
                                section.vertical_align.unwrap_or_default(),
                            );
                            section_table.insert(formatted_text.get_sections().len() - 1, section_index);
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
                        let ch_type = glyph_types.get(ch, sub_string_type);
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
                for j in 0..formatted_text.length() {
                    let section_index = formatted_text.get_section_index(j) as usize;
                    let section = &formatted_text.get_sections()[section_index];
                    if section.image_id.is_some() {
                        continue;
                    }
                    let section_font_stack =
                        section_table.get(&section_index).and_then(|&i| formatted.sections[i].font_stack.clone());
                    let font_stack = section_font_stack.unwrap_or_else(|| base_font_stack.clone());
                    let dependencies = layout_parameters.glyph_dependencies.glyphs.entry(font_stack).or_default();
                    if section.type_ == GlyphIDType::FontPBF {
                        // Glyph PBFs have no glyphs beyond the BMP.
                        let c = formatted_text.char_at(j);
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
            let variable_anchor_offset = self.layout.text_variable_anchor_offset.evaluate(
                &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(canonical)),
            );
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

        let variable_text_anchor: Vec<TextVariableAnchorType> = self.layout.text_variable_anchor.evaluate(
            &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(canonical)),
        );
        if variable_text_anchor.is_empty() {
            return None;
        }
        let (variable_text_offset, radial_offset) = if self.text_radial_offset.is_undefined() {
            let text_offset = self.layout.text_offset.evaluate(
                &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(canonical)),
            );
            (Some([text_offset[0] * ONE_EM, text_offset[1] * ONE_EM]), 0.0)
        } else {
            let radial_offset = self.layout.text_radial_offset.evaluate(
                &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(canonical)),
            );
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

    #[expect(
        clippy::too_many_arguments,
        reason = "one feature's shaped text and icon, its images and its evaluated text and icon layout"
    )]
    fn add_feature(
        &mut self,
        layout_feature_index: usize,
        feature: &SymbolFeature,
        shaped_text_orientations: &ShapedTextOrientations,
        mut shaped_icon: Option<PositionedIcon>,
        icons: &ImagePositions,
        text_offset: [f32; 2],
        layout_text_size: f32,
        layout_icon_size: f32,
        icon_type: SymbolContent,
    ) {
        let min_scale = 0.5_f32;
        let glyph_size = 24.0_f32;
        let zoom = self.zoom;
        let canonical = self.canonical_id;
        let layout = Arc::clone(&self.layout);

        let icon_offset = layout.icon_offset.evaluate(
            &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(&canonical)),
        );

        // To reduce the number of labels that jump around when zooming we
        // need to use a text-size value that is the same for all zoom
        // levels. This calculates text-size at a high zoom level so that all
        // tiles can use the same value when calculating anchor positions.
        let text_max_size = layout.text_size.evaluate(
            &EvaluationContext::with_zoom_feature(18.0, Some(feature)).with_canonical_tile_id(Some(&canonical)),
        );

        let font_scale = layout_text_size / glyph_size;
        let text_box_scale = self.tile_pixel_ratio * font_scale;
        let text_max_box_scale = self.tile_pixel_ratio * text_max_size / glyph_size;
        let icon_box_scale = self.tile_pixel_ratio * layout_icon_size;
        let symbol_spacing = self.tile_pixel_ratio * *layout.symbol_spacing.uniform();
        let text_padding = *layout.text_padding.uniform() * self.tile_pixel_ratio;
        let icon_padding = layout.icon_padding.evaluate(
            &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(&canonical)),
        ) * self.tile_pixel_ratio;
        let text_max_angle = (layout.text_max_angle).uniform().to_radians_stepwise();
        let icon_rotation = layout.icon_rotate.evaluate(
            &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(&canonical)),
        );
        let text_rotation = layout.text_rotate.evaluate(
            &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(&canonical)),
        );
        let variable_anchor_offsets = self.text_variable_anchor_offset(feature);
        // The spec raises only point-placed symbols.
        let height_offset = if *layout.symbol_placement.uniform() == SymbolPlacementType::Point {
            layout.symbol_height_offset.evaluate(
                &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(&canonical)),
            )
        } else {
            0.0
        };

        let text_placement = if *layout.text_rotation_alignment.uniform() == TextRotationAlignmentType::Viewport {
            SymbolPlacementType::Point
        } else {
            *layout.symbol_placement.uniform()
        };

        let text_repeat_distance = symbol_spacing / 2.0;
        let evaluated_layout_properties = layout.evaluate_all(
            &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(&canonical)),
        );
        let indexed_feature = IndexedSubfeature {
            index: feature.index,
            source_layer_name: Arc::clone(&self.source_layer_name),
            bucket_leader_id: Arc::clone(&self.bucket_leader_id),
            sort_index: self.symbol_instances.len(),
            bucket_instance_id: 0,
            collision_group_id: 0,
            overlap_mode: OverlapMode::Never,
        };

        let icon_text_fit = evaluated_layout_properties.icon_text_fit;
        let has_icon_text_fit = icon_text_fit != IconTextFitType::None;
        // Adjust shaped icon size when icon-text-fit is used.
        let mut vertically_shaped_icon: Option<PositionedIcon> = None;
        if let Some(icon) = shaped_icon.as_mut()
            && has_icon_text_fit
        {
            // Create vertically shaped icon for vertical writing mode if needed.
            if self.allow_vertical_placement && shaped_text_orientations.vertical.is_some() {
                let mut vertical = icon.clone();
                vertical.fit_icon_to_text(
                    &shaped_text_orientations.vertical,
                    icon_text_fit,
                    layout.icon_text_fit_padding.uniform(),
                    &icon_offset,
                    font_scale,
                );
                vertically_shaped_icon = Some(vertical);
            }
            let shaped_text = default_horizontal_shaping(shaped_text_orientations);
            if shaped_text.is_some() {
                icon.fit_icon_to_text(
                    shaped_text,
                    icon_text_fit,
                    layout.icon_text_fit_padding.uniform(),
                    &icon_offset,
                    font_scale,
                );
            }
        }

        let key: String =
            feature.formatted_text.as_ref().map_or_else(String::new, |text| text.logical_text().iter().collect());

        let allow_vertical_placement = self.allow_vertical_placement;
        let create_symbol_instance_shared_data = |line: GeometryCoordinates| {
            Arc::new(SymbolInstanceSharedData::new(
                line,
                shaped_text_orientations,
                shaped_icon.as_ref(),
                vertically_shaped_icon.as_ref(),
                &evaluated_layout_properties,
                text_placement,
                text_offset,
                icons,
                icon_rotation,
                icon_type,
                has_icon_text_fit,
                allow_vertical_placement,
            ))
        };

        let mode = self.mode;
        let overscaling = self.overscaling;
        let sort_features_by_key = self.sort_features_by_key;
        let add_symbol_instance =
            |this: &mut SymbolLayout, anchor: Anchor, shared_data: Arc<SymbolInstanceSharedData>| {
                let anchor_inside_tile = anchor.point.x >= 0.0
                    && anchor.point.x < EXTENT as f32
                    && anchor.point.y >= 0.0
                    && anchor.point.y < EXTENT as f32;

                if mode == MapMode::Tile || anchor_inside_tile {
                    // For static/continuous rendering, only add symbols anchored
                    // within this tile: neighboring symbols will be added as part
                    // of the neighboring tiles. In tiled rendering mode, add all
                    // symbols in the buffers so that we can: (1) render symbols
                    // that overlap into this tile (2) approximate collision
                    // detection effects from neighboring symbols
                    this.symbol_instances.push(SymbolInstance::new(
                        anchor,
                        shared_data,
                        shaped_text_orientations,
                        shaped_icon.as_ref(),
                        vertically_shaped_icon.as_ref(),
                        text_box_scale,
                        text_padding,
                        text_placement,
                        text_offset,
                        icon_box_scale,
                        icon_padding,
                        icon_offset,
                        &indexed_feature,
                        layout_feature_index,
                        feature.index,
                        key.as_bytes(),
                        overscaling,
                        icon_rotation,
                        text_rotation,
                        variable_anchor_offsets.as_ref(),
                        allow_vertical_placement,
                        icon_type,
                        height_offset,
                        &mut this.symbol_instance_pools,
                    ));

                    if sort_features_by_key {
                        let len = this.symbol_instances.len();
                        match this.sort_key_ranges.last_mut() {
                            Some(last) if last.sort_key == feature.sort_key => last.end = len,
                            _ => this.sort_key_ranges.push(SortKeyRange {
                                sort_key: feature.sort_key,
                                start: len - 1,
                                end: len,
                            }),
                        }
                    }
                }
            };

        let feature_type = feature.get_type();
        let text_shaping = if shaped_text_orientations.vertical.is_some() {
            &shaped_text_orientations.vertical
        } else {
            default_horizontal_shaping(shaped_text_orientations)
        };
        let (text_left, text_right) = (text_shaping.left, text_shaping.right);
        let (icon_left, icon_right) = shaped_icon.as_ref().map_or((0.0, 0.0), |icon| (icon.left(), icon.right()));

        let placement = &layout.symbol_placement;
        if *placement.uniform() == SymbolPlacementType::Line {
            let clipped_lines = clip_lines(&feature.geometry, 0, 0, EXTENT as i16, EXTENT as i16);
            for line in clipped_lines.iter() {
                let anchors: Anchors = get_anchors(
                    line,
                    symbol_spacing,
                    text_max_angle,
                    text_left,
                    text_right,
                    icon_left,
                    icon_right,
                    glyph_size,
                    text_max_box_scale,
                    self.overscaling,
                );
                let shared_data = create_symbol_instance_shared_data(line.clone());
                for anchor in anchors {
                    let too_close = feature.formatted_text.is_some()
                        && self.anchor_is_too_close(&key, text_repeat_distance, &anchor);
                    if !too_close {
                        add_symbol_instance(self, anchor, Arc::clone(&shared_data));
                    }
                }
            }
        } else if *placement.uniform() == SymbolPlacementType::LineCenter {
            // No clipping, multiple lines per feature are allowed
            // "lines" with only one point are ignored, as clipping ignores
            // them
            for line in feature.geometry.iter() {
                if line.len() > 1 {
                    let anchor = get_center_anchor(
                        line,
                        text_max_angle,
                        text_left,
                        text_right,
                        icon_left,
                        icon_right,
                        glyph_size,
                        text_max_box_scale,
                    );
                    if let Some(anchor) = anchor {
                        add_symbol_instance(self, anchor, create_symbol_instance_shared_data(line.clone()));
                    }
                }
            }
        } else if feature_type == FeatureType::Polygon {
            for polygon in mln_tile::geometry_tile_data::classify_rings(&feature.geometry) {
                let mut poly: polylabel::Polygon<f64> = Vec::new();
                for ring in polygon.iter() {
                    let r: polylabel::LinearRing<f64> =
                        ring.iter().map(|p| Point::new(f64::from(p.x), f64::from(p.y))).collect();
                    poly.push(r);
                }

                // 1 pixel worth of precision, in tile coordinates
                let Ok(poi) = polylabel::polylabel(&poly, f64::from(EXTENT) / TILE_SIZE_D) else {
                    continue;
                };
                let anchor = Anchor::new(poi.x as f32, poi.y as f32, 0.0, Some(min_scale as usize));
                add_symbol_instance(self, anchor, create_symbol_instance_shared_data(polygon[0].clone()));
            }
        } else if feature_type == FeatureType::LineString {
            for line in feature.geometry.iter() {
                // Skip invalid LineStrings.
                if line.is_empty() {
                    continue;
                }

                let anchor = Anchor::new(f32::from(line[0].x), f32::from(line[0].y), 0.0, Some(min_scale as usize));
                add_symbol_instance(self, anchor, create_symbol_instance_shared_data(line.clone()));
            }
        } else if feature_type == FeatureType::Point {
            for points in feature.geometry.iter() {
                for point in points {
                    let anchor = Anchor::new(f32::from(point.x), f32::from(point.y), 0.0, Some(min_scale as usize));
                    add_symbol_instance(self, anchor, create_symbol_instance_shared_data(vec![*point]));
                }
            }
        }
    }

    fn anchor_is_too_close(&mut self, text: &str, repeat_distance: f32, anchor: &Anchor) -> bool {
        if let Some(other_anchors) = self.compare_text.get(text) {
            for other_anchor in other_anchors {
                if dist(anchor.point.x, anchor.point.y, other_anchor.point.x, other_anchor.point.y) < repeat_distance {
                    return true;
                }
            }
        }
        self.compare_text.entry(text.to_owned()).or_default().push(*anchor);
        false
    }

    /// Writes the tile distances of `line`'s vertices from `anchor` to
    /// `tile_distances`, one for each vertex, along the line on either side
    /// of the anchor. A line without an anchor segment leaves them as they
    /// are.
    pub(crate) fn calculate_tile_distances(line: &[GeometryCoordinate], anchor: &Anchor, tile_distances: &mut [f32]) {
        debug_assert_eq!(line.len(), tile_distances.len());
        if let Some(segment) = anchor.segment {
            debug_assert!(segment < line.len());
            let p = |i: usize| (f32::from(line[i].x), f32::from(line[i].y));
            let mut sum_forward_length = if segment + 1 < line.len() {
                let (x, y) = p(segment + 1);
                dist(anchor.point.x, anchor.point.y, x, y)
            } else {
                0.0
            };
            let (sx, sy) = p(segment);
            let mut sum_backward_length = dist(anchor.point.x, anchor.point.y, sx, sy);
            for (i, tile_distance) in tile_distances.iter_mut().enumerate().skip(segment + 1) {
                *tile_distance = sum_forward_length;
                if i < line.len() - 1 {
                    let (ax, ay) = p(i + 1);
                    let (bx, by) = p(i);
                    sum_forward_length += dist(ax, ay, bx, by);
                }
            }
            let mut i = segment;
            loop {
                tile_distances[i] = sum_backward_length;
                if i != 0 {
                    let (ax, ay) = p(i - 1);
                    let (bx, by) = p(i);
                    sum_backward_length += dist(ax, ay, bx, by);
                    i -= 1;
                } else {
                    break; // Add break to avoid unsigned integer overflow when i==0
                }
            }
        }
    }

    fn update_paint_properties_for_section(
        paint_properties: &mut VecMap<String, PaintProperties>,
        state: &SymbolBucketState,
        feature: &SymbolFeature,
        section_index: usize,
        canonical: &CanonicalTileID,
    ) {
        let formatted_section =
            section_options_to_value(feature.formatted_text.as_ref().expect("text").section_at(section_index));
        let length = state.text.attribute_data.len();
        for binders in paint_properties.values_mut() {
            binders.text_binders.populate_vertex_vectors(
                feature,
                length,
                feature.index,
                &ImagePositions::default(),
                None,
                canonical,
                &formatted_section,
            );
        }
    }

    /// Adds symbol quads to the bucket and returns the formatted section index
    /// of the last added quad.
    #[expect(
        clippy::too_many_arguments,
        reason = "the bucket, its state and binders, the symbol and feature, and the quads of one writing mode"
    )]
    fn add_symbol_glyph_quads(
        &self,
        bucket: &SymbolBucket,
        state: &mut SymbolBucketState,
        paint_properties: &mut VecMap<String, PaintProperties>,
        symbol_instance: &SymbolInstance,
        feature: &SymbolFeature,
        writing_mode: WritingModeType,
        glyph_quads: &SymbolQuads,
        canonical: &CanonicalTileID,
        mut last_added_section: Option<usize>,
        line: &mut Option<(PoolRange, PoolRange)>,
    ) -> (usize, usize) {
        let size_data = bucket.text_size_binder.vertex_size_data(feature);
        let has_format_section_overrides = bucket.has_format_section_overrides();
        let placed_icon_index = if writing_mode == WritingModeType::Vertical {
            symbol_instance.placed_vertical_icon_index
        } else {
            symbol_instance.placed_icon_index
        };

        // The symbol's text variants share its line and its distances.
        let pools = &mut state.text.pools;
        let (line, tile_distances) = *line.get_or_insert_with(|| {
            let line = symbol_instance.line();
            let tile_distances = PoolRange::extend(&mut pools.tile_distances, std::iter::repeat_n(0.0, line.len()));
            Self::calculate_tile_distances(
                line,
                symbol_instance.anchor(),
                tile_distances.of_mut(&mut pools.tile_distances),
            );
            (PoolRange::extend(&mut pools.lines, line.iter().copied()), tile_distances)
        });
        let glyph_offsets = PoolRange::at_end(&pools.glyph_offsets);
        state.text.placed_symbols.push(PlacedSymbol::new(
            symbol_instance.anchor().point,
            symbol_instance.anchor().segment.unwrap_or(0),
            size_data.min,
            size_data.max,
            symbol_instance.text_offset(),
            writing_mode,
            line,
            tile_distances,
            glyph_offsets,
            placed_icon_index,
        ));
        let placed_index = state.text.placed_symbols.len() - 1;
        state.text.placed_symbols[placed_index].height_offset = symbol_instance.height_offset();
        state.text.placed_symbols[placed_index].angle =
            if self.allow_vertical_placement && writing_mode == WritingModeType::Vertical {
                std::f32::consts::FRAC_PI_2
            } else {
                0.0
            };

        let mut first_symbol = true;
        for symbol_quad in glyph_quads {
            if has_format_section_overrides {
                if let Some(last) = last_added_section
                    && last != symbol_quad.section_index
                {
                    Self::update_paint_properties_for_section(paint_properties, state, feature, last, canonical);
                }
                last_added_section = Some(symbol_quad.section_index);
            }
            let index = Self::add_symbol(
                &mut state.text,
                size_data,
                symbol_quad,
                symbol_instance.anchor(),
                placed_index,
                feature.sort_key,
            );
            if first_symbol {
                state.text.placed_symbols[placed_index].start_index = index;
                first_symbol = false;
            }
        }

        (last_added_section.unwrap_or(0), placed_index)
    }

    /// Adds a placed symbol quad to `buffer`.
    fn add_symbol(
        buffer: &mut Buffer,
        size_data: Range<f32>,
        symbol: &SymbolQuad,
        label_anchor: &Anchor,
        placed_symbol: usize,
        sort_key: f32,
    ) -> usize {
        const VERTEX_LENGTH: usize = 4;
        let height_offset = buffer.placed_symbols[placed_symbol].height_offset;

        let tl = symbol.tl;
        let tr = symbol.tr;
        let bl = symbol.bl;
        let br = symbol.br;
        let tex = symbol.tex;
        let pixel_offset_tl = symbol.pixel_offset_tl;
        let pixel_offset_br = symbol.pixel_offset_br;
        let min_font_scale = symbol.min_font_scale;

        let (vertex_offset, index_offset) = (buffer.attribute_data.len(), buffer.triangles.len());
        let segment = match buffer.segments.last_mut() {
            Some(last)
                if u16::try_from(last.vertex_length + VERTEX_LENGTH).is_ok()
                    && (last.sort_key - sort_key).abs().partial_cmp(&f32::EPSILON) != Some(Ordering::Greater) =>
            {
                last
            }
            _ => {
                // A segment's length counts indices (three per triangle), not
                // triangles.
                let mut segment =
                    mln_gfx::shaders::segment::SegmentBase::with_lengths(vertex_offset, index_offset, 0, 0);
                segment.sort_key = sort_key;
                buffer.segments.push_mut(segment)
            }
        };

        // We're generating triangle fans, so we always start with the first
        // coordinate in this polygon.
        debug_assert!(u16::try_from(segment.vertex_length).is_ok());
        let index = segment.vertex_length as u16;

        let anchor = label_anchor.point;
        let glyph_offset_y = symbol.glyph_offset.y;
        // coordinates (2 triangles)
        buffer.attribute_data.get_mut().push(SymbolBucket::layout_attributes(
            anchor,
            tl,
            glyph_offset_y,
            tex.x,
            tex.y,
            [tex.x, tex.y],
            size_data,
            symbol.is_sdf,
            pixel_offset_tl,
            min_font_scale,
            height_offset,
        ));
        buffer.attribute_data.get_mut().push(SymbolBucket::layout_attributes(
            anchor,
            tr,
            glyph_offset_y,
            tex.x + tex.w,
            tex.y,
            [tex.x, tex.y],
            size_data,
            symbol.is_sdf,
            Vec2::new(pixel_offset_br.x, pixel_offset_tl.y),
            min_font_scale,
            height_offset,
        ));
        buffer.attribute_data.get_mut().push(SymbolBucket::layout_attributes(
            anchor,
            bl,
            glyph_offset_y,
            tex.x,
            tex.y + tex.h,
            [tex.x, tex.y],
            size_data,
            symbol.is_sdf,
            Vec2::new(pixel_offset_tl.x, pixel_offset_br.y),
            min_font_scale,
            height_offset,
        ));
        buffer.attribute_data.get_mut().push(SymbolBucket::layout_attributes(
            anchor,
            br,
            glyph_offset_y,
            tex.x + tex.w,
            tex.y + tex.h,
            [tex.x, tex.y],
            size_data,
            symbol.is_sdf,
            pixel_offset_br,
            min_font_scale,
            height_offset,
        ));

        // Dynamic/Opacity vertices are initialized so that the vertex count
        // always agrees with the layout vertex buffer, but they will always
        // be updated before rendering happens
        let dynamic_attributes = SymbolBucket::dynamic_layout_attributes(anchor, 0.0);
        buffer.dynamic_attribute_data.extend_from_slice(&[dynamic_attributes; 4]);

        let opacity_attributes = SymbolBucket::opacity_attributes(true, 1.0);
        buffer.opacity_attribute_data.extend_from_slice(&[opacity_attributes; 4]);

        // add the two triangles, referencing the four coordinates we just
        // inserted.
        buffer.triangles.extend_from_slice(&[index, index + 1, index + 2]);
        buffer.triangles.extend_from_slice(&[index + 1, index + 2, index + 3]);

        segment.vertex_length += VERTEX_LENGTH;
        segment.index_length += 6;

        buffer.placed_symbols[placed_symbol].glyph_offsets.push(&mut buffer.pools.glyph_offsets, symbol.glyph_offset.x);

        index as usize
    }

    fn add_symbols(
        buffer: &mut Buffer,
        size_data: Range<f32>,
        symbols: &SymbolQuads,
        label_anchor: &Anchor,
        placed_symbol: usize,
        sort_key: f32,
    ) -> usize {
        let mut first_symbol = true;
        let mut first_index = 0;
        for symbol in symbols {
            let index = Self::add_symbol(buffer, size_data, symbol, label_anchor, placed_symbol, sort_key);
            if first_symbol {
                first_index = index;
                first_symbol = false;
            }
        }
        first_index
    }

    fn add_to_debug_buffers(state: &mut SymbolBucketState) {
        if !Self::has_symbol_instances_in(state) {
            return;
        }

        let instances = std::mem::take(&mut state.symbol_instances);
        let pools = std::mem::take(&mut state.symbol_instance_pools);
        for symbol_instance in &instances {
            let mut populate_collision_box = |feature: &crate::text::collision_feature::CollisionFeature,
                                              is_text: bool| {
                let collision_buffer: &mut CollisionBuffer = if feature.along_line {
                    if is_text {
                        state.text_collision_circle.get_or_insert_with(Default::default)
                    } else {
                        state.icon_collision_circle.get_or_insert_with(Default::default)
                    }
                } else if is_text {
                    state.text_collision_box.get_or_insert_with(Default::default)
                } else {
                    state.icon_collision_box.get_or_insert_with(Default::default)
                };

                for b in pools.boxes(feature) {
                    const VERTEX_LENGTH: usize = 4;
                    let anchor = b.anchor;

                    let tl = Vec2::new(b.x1, b.y1);
                    let tr = Vec2::new(b.x2, b.y1);
                    let bl = Vec2::new(b.x1, b.y2);
                    let br = Vec2::new(b.x2, b.y2);

                    let index_length = if feature.along_line { 6 } else { 8 };

                    // A segment's length counts indices, not primitives.
                    let (vertex_offset, index_offset) =
                        (collision_buffer.vertices.len(), collision_buffer.indices.len());
                    let segment = match collision_buffer.segments.last_mut() {
                        Some(last) if u16::try_from(last.vertex_length + VERTEX_LENGTH).is_ok() => last,
                        _ => collision_buffer
                            .segments
                            .push_mut(mln_gfx::shaders::segment::SegmentBase::new(vertex_offset, index_offset)),
                    };
                    let index = segment.vertex_length as u16;

                    let symbol_anchor = symbol_instance.anchor().point;
                    let height = symbol_instance.height_offset();
                    for corner in [tl, tr, br, bl] {
                        collision_buffer.vertices.push(SymbolBucket::collision_layout_vertex(
                            anchor,
                            symbol_anchor,
                            corner,
                            height,
                        ));
                    }

                    // Dynamic vertices are initialized so that the vertex
                    // count always agrees with the layout vertex buffer, but
                    // they will always be updated before rendering happens
                    let dynamic_vertex = SymbolBucket::collision_dynamic_vertex(false, false, Vec2::ZERO);
                    collision_buffer.dynamic_vertices.extend_from_slice(&[dynamic_vertex; 4]);

                    if feature.along_line {
                        collision_buffer.indices.extend_from_slice(&[index, index + 1, index + 2]);
                        collision_buffer.indices.extend_from_slice(&[index, index + 2, index + 3]);
                    } else {
                        collision_buffer.indices.extend_from_slice(&[index, index + 1]);
                        collision_buffer.indices.extend_from_slice(&[index + 1, index + 2]);
                        collision_buffer.indices.extend_from_slice(&[index + 2, index + 3]);
                        collision_buffer.indices.extend_from_slice(&[index + 3, index]);
                    }

                    segment.vertex_length += VERTEX_LENGTH;
                    segment.index_length += index_length;
                }
            };
            populate_collision_box(symbol_instance.text_collision_feature(), true);
            if let Some(feature) = symbol_instance.vertical_text_collision_feature() {
                populate_collision_box(feature, true);
            }
            if let Some(feature) = symbol_instance.vertical_icon_collision_feature() {
                populate_collision_box(feature, false);
            }
            populate_collision_box(symbol_instance.icon_collision_feature(), false);
        }
        state.symbol_instances = instances;
        state.symbol_instance_pools = pools;
    }

    fn has_symbol_instances_in(state: &SymbolBucketState) -> bool {
        !state.symbol_instances.is_empty()
    }
}

/// Degrees in radians as `deg * π / 180`, rounding after each step, which
/// `f32::to_radians`, rounding `π / 180` first, doesn't match in the last
/// bit.
trait ToRadiansStepwise {
    fn to_radians_stepwise(self) -> f32;
}

impl ToRadiansStepwise for f32 {
    fn to_radians_stepwise(self) -> f32 {
        self * std::f32::consts::PI / 180.0
    }
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
        _feature_index: &mut FeatureIndex,
        build: &BuildParams,
    ) -> Result<Option<SymbolBucket>, Error> {
        if let Some(resolved) = resolved {
            if self.has_font_text {
                self.finalize_symbols(&resolved.shaping);
            }
            self.prepare_symbols(&resolved.glyphs, &resolved.icons);
            if self.symbol_instances.is_empty() {
                return Ok(None);
            }
        }
        Ok(Some(self.build_bucket(build.first_load, build.show_collision_boxes, &build.canonical)))
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

            let sections = formatted_text.get_sections();
            let style_text = formatted_text.get_styled_text();

            let mut sub_string: Vec<char> = Vec::new();
            let mut section_index = style_text.styles.first().copied().unwrap_or(0);

            let apply_sub_string = |sub_string: &[char], section_index: u8, shaped_string: &mut TaggedString| {
                if sub_string.is_empty() {
                    return;
                }
                let section = &sections[section_index as usize];
                if let Some(image_id) = &section.image_id {
                    shaped_string.add_image_section(image_id, section.vertical_align);
                } else if section.type_ == GlyphIDType::FontPBF {
                    shaped_string.add_text_section(
                        sub_string,
                        section.scale,
                        &section.font_stack,
                        section.type_,
                        section.key_section,
                        section.text_color,
                        section.vertical_align,
                    );
                } else {
                    let shaped = results
                        .get(&section.font_stack)
                        .and_then(|types| types.get(&section.type_))
                        .and_then(|strings| strings.get(sub_string))
                        .unwrap_or(&unshaped);

                    shaped_string.add_shaped_section(
                        shaped,
                        section.scale,
                        &section.font_stack,
                        section.type_,
                        starts_right_to_left(sub_string),
                        section.key_section,
                        section.text_color,
                        section.vertical_align,
                    );
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
            let layout_text_size = layout.text_size.evaluate(
                &EvaluationContext::with_zoom_feature(zoom + 1.0, Some(feature))
                    .with_canonical_tile_id(Some(&canonical)),
            );
            let layout_text_size_at_bucket_zoom_level = layout.text_size.evaluate(
                &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(&canonical)),
            );
            let layout_icon_size = layout.icon_size.evaluate(
                &EvaluationContext::with_zoom_feature(zoom + 1.0, Some(feature))
                    .with_canonical_tile_id(Some(&canonical)),
            );

            // if feature has text, shape the text
            if feature.formatted_text.is_some() && layout_text_size > 0.0 {
                let line_height = *layout.text_line_height.uniform() * ONE_EM;
                let text = feature.formatted_text.as_ref().expect("text");
                let spacing = if i18n::allows_letter_spacing(text.logical_text()) {
                    layout.text_letter_spacing.evaluate(
                        &EvaluationContext::with_zoom_feature(zoom, Some(feature))
                            .with_canonical_tile_id(Some(&canonical)),
                    ) * ONE_EM
                } else {
                    0.0
                };
                let max_width = if is_point_placement {
                    layout.text_max_width.evaluate(
                        &EvaluationContext::with_zoom_feature(zoom, Some(feature))
                            .with_canonical_tile_id(Some(&canonical)),
                    ) * ONE_EM
                } else {
                    0.0
                };

                let variable_anchor_offsets = self.text_variable_anchor_offset(feature);
                let text_anchor = layout.text_anchor.evaluate(
                    &EvaluationContext::with_zoom_feature(zoom, Some(feature)).with_canonical_tile_id(Some(&canonical)),
                );
                if variable_anchor_offsets.as_ref().is_none_or(VariableAnchorOffsetCollection::is_empty) {
                    // Layers with variable anchors use the
                    // `text-radial-offset` property and the [x, y] offset
                    // vector is calculated at placement time instead of
                    // layout time
                    let radial_offset = layout.text_radial_offset.evaluate(
                        &EvaluationContext::with_zoom_feature(zoom, Some(feature))
                            .with_canonical_tile_id(Some(&canonical)),
                    );
                    if radial_offset > 0.0 {
                        // The style spec says don't use `text-offset` and
                        // `text-radial-offset` together but doesn't actually
                        // specify what happens if you use both. We go with
                        // the radial offset.
                        text_offset = evaluate_radial_offset(text_anchor, radial_offset * ONE_EM);
                    } else {
                        let offset = layout.text_offset.evaluate(
                            &EvaluationContext::with_zoom_feature(zoom, Some(feature))
                                .with_canonical_tile_id(Some(&canonical)),
                        );
                        text_offset = [offset[0] * ONE_EM, offset[1] * ONE_EM];
                    }
                }
                let mut text_justify = if text_along_line {
                    TextJustifyType::Center
                } else {
                    layout.text_justify.evaluate(
                        &EvaluationContext::with_zoom_feature(zoom, Some(feature))
                            .with_canonical_tile_id(Some(&canonical)),
                    )
                };

                let allow_vertical_placement = self.allow_vertical_placement;
                let apply_shaping = |formatted_text: &TaggedString,
                                     writing_mode: WritingModeType,
                                     text_anchor: SymbolAnchorType,
                                     text_justify: TextJustifyType,
                                     text_offset: &[f32; 2]|
                 -> Shaping {
                    get_shaping(
                        formatted_text,
                        max_width,
                        line_height,
                        text_anchor,
                        text_justify,
                        spacing,
                        text_offset,
                        writing_mode,
                        glyphs,
                        icons,
                        layout_text_size,
                        layout_text_size_at_bucket_zoom_level,
                        allow_vertical_placement,
                    )
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
                            justifications.push(get_anchor_justification(anchor_offset.anchor_type));
                        }
                    } else {
                        justifications.push(text_justify);
                    }
                    for justification in justifications {
                        if shaping_for_text_justify_type(&mut shaped_text_orientations, justification).is_some() {
                            continue;
                        }
                        // If using text-variable-anchor for the layer, we use
                        // a center anchor for all shapings and apply the
                        // offsets for the anchor in the placement step.
                        let shaping = apply_shaping(
                            formatted_text,
                            WritingModeType::Horizontal,
                            SymbolAnchorType::Center,
                            justification,
                            &text_offset,
                        );
                        if shaping.is_some() {
                            let single = shaping.positioned_lines.len() == 1;
                            *shaping_for_text_justify_type(&mut shaped_text_orientations, justification) = shaping;
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
                            WritingModeType::Vertical,
                            text_anchor,
                            TextJustifyType::Left,
                            &text_offset,
                        );
                    }
                } else {
                    if text_justify == TextJustifyType::Auto {
                        text_justify = get_anchor_justification(text_anchor);
                    }

                    // Horizontal point or line label.
                    let shaping = apply_shaping(
                        formatted_text,
                        WritingModeType::Horizontal,
                        text_anchor,
                        text_justify,
                        &text_offset,
                    );
                    if shaping.is_some() {
                        shaped_text_orientations.horizontal = shaping;
                    }

                    // Vertical point label shaping where the layer allows
                    // vertical placement.
                    if allow_vertical_placement && formatted_text.allows_vertical_writing_mode() {
                        formatted_text.verticalize_punctuation();
                        shaped_text_orientations.vertical = apply_shaping(
                            formatted_text,
                            WritingModeType::Vertical,
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
                            WritingModeType::Vertical,
                            text_anchor,
                            text_justify,
                            &text_offset,
                        );
                    }
                }
            }

            // if feature has icon, get sprite atlas position
            let mut icon_type = SymbolContent::None;
            if let Some(icon) = &feature.icon
                && let Some(image) = icons.get(icon.id())
            {
                icon_type = SymbolContent::IconRGBA;
                shaped_icon = Some(PositionedIcon::shape_icon(
                    image,
                    &layout.icon_offset.evaluate(
                        &EvaluationContext::with_zoom_feature(zoom, Some(feature))
                            .with_canonical_tile_id(Some(&canonical)),
                    ),
                    layout.icon_anchor.evaluate(
                        &EvaluationContext::with_zoom_feature(zoom, Some(feature))
                            .with_canonical_tile_id(Some(&canonical)),
                    ),
                ));
                if image.sdf {
                    icon_type = SymbolContent::IconSDF;
                }
                if image.pixel_ratio != self.pixel_ratio || layout.icon_rotate.constant_or(&1.0) != 0.0 {
                    self.icons_need_linear = true;
                }
            }

            // Add the feature if it has either shaped text or an icon.
            let default_shaping = default_horizontal_shaping(&shaped_text_orientations);
            self.icons_in_text |= default_shaping.is_some() && default_shaping.icons_in_text;
            if default_shaping.is_some() || shaped_icon.is_some() {
                self.add_feature(
                    feature_index,
                    feature,
                    &shaped_text_orientations,
                    shaped_icon,
                    icons,
                    text_offset,
                    layout_text_size,
                    layout_icon_size,
                    icon_type,
                );
            }

            feature.geometry.clear();
        }
        self.features = features;

        self.compare_text.clear();
    }

    fn build_bucket(
        &mut self,
        first_load: bool,
        show_collision_boxes: bool,
        canonical: &CanonicalTileID,
    ) -> SymbolBucket {
        let mut bucket = SymbolBucket::new(
            Arc::clone(&self.layout),
            &self.layer_paint_properties,
            &self.text_size,
            &self.icon_size,
            self.zoom,
            self.icons_need_linear,
            self.sort_features_by_y,
            self.bucket_leader_id.to_string(),
            std::mem::take(&mut self.symbol_instances),
            std::mem::take(&mut self.symbol_instance_pools),
            std::mem::take(&mut self.sort_key_ranges),
            self.tile_pixel_ratio,
            self.allow_vertical_placement,
            std::mem::take(&mut self.placement_modes),
            self.icons_in_text,
        );

        // Out of the bucket while the symbols fill them.
        let mut state = std::mem::take(&mut bucket.state);
        let mut paint_properties = std::mem::take(&mut bucket.paint_properties);
        let mut instances = std::mem::take(&mut state.symbol_instances);
        {
            let bucket_ref = &bucket;
            let state = &mut state;
            for symbol_instance in &mut instances {
                let has_text = symbol_instance.has_text();
                let has_icon = symbol_instance.has_icon();
                let single_line = symbol_instance.single_line();

                let feature = &self.features[symbol_instance.layout_feature_index()];

                // Insert final placement into collision tree and add
                // glyphs/icons to buffers

                // Process icon first, so that text symbols would have
                // the index of the icon, which updates the dynamic vertices of
                // an `icon-text-fit` image.
                if has_icon {
                    let size_data = bucket_ref.icon_size_binder.vertex_size_data(feature);
                    let sdf = symbol_instance.has_sdf_icon();
                    let mut place_icon = |icon_quads: &SymbolQuads, writing_mode: WritingModeType| -> usize {
                        let icon_buffer = if sdf { &mut state.sdf_icon } else { &mut state.icon };
                        let pools = &mut icon_buffer.pools;
                        let line = PoolRange::extend(&mut pools.lines, symbol_instance.line().iter().copied());
                        let glyph_offsets = PoolRange::at_end(&pools.glyph_offsets);
                        icon_buffer.placed_symbols.push(PlacedSymbol::new(
                            symbol_instance.anchor().point,
                            symbol_instance.anchor().segment.unwrap_or(0),
                            size_data.min,
                            size_data.max,
                            symbol_instance.icon_offset(),
                            writing_mode,
                            line,
                            PoolRange::default(),
                            glyph_offsets,
                            None,
                        ));
                        let index = icon_buffer.placed_symbols.len() - 1;
                        icon_buffer.placed_symbols[index].height_offset = symbol_instance.height_offset();
                        icon_buffer.placed_symbols[index].angle =
                            if self.allow_vertical_placement && writing_mode == WritingModeType::Vertical {
                                std::f32::consts::FRAC_PI_2
                            } else {
                                0.0
                            };
                        let start_index = Self::add_symbols(
                            icon_buffer,
                            size_data,
                            icon_quads,
                            symbol_instance.anchor(),
                            index,
                            feature.sort_key,
                        );
                        icon_buffer.placed_symbols[index].start_index = start_index;
                        index
                    };

                    let icon_quads = symbol_instance.icon_quads().cloned().unwrap_or_default();
                    let index = place_icon(&icon_quads, WritingModeType::None);
                    let vertical_index = symbol_instance
                        .vertical_icon_quads()
                        .cloned()
                        .map(|vertical_icon_quads| place_icon(&vertical_icon_quads, WritingModeType::Vertical));
                    symbol_instance.placed_icon_index = Some(index);
                    if vertical_index.is_some() {
                        symbol_instance.placed_vertical_icon_index = vertical_index;
                    }

                    let length =
                        if sdf { state.sdf_icon.attribute_data.len() } else { state.icon.attribute_data.len() };
                    for binders in paint_properties.values_mut() {
                        binders.icon_binders.populate_vertex_vectors(
                            feature,
                            length,
                            symbol_instance.data_feature_index(),
                            &ImagePositions::default(),
                            None,
                            canonical,
                            &Value::Null,
                        );
                    }
                }

                if has_text && feature.formatted_text.is_some() {
                    let mut last_added_section: Option<usize> = None;
                    let mut text_line = None;
                    let writing_modes = symbol_instance.writing_modes();
                    if single_line {
                        let quads = symbol_instance.right_justified_glyph_quads().clone();
                        let (section, placed) = self.add_symbol_glyph_quads(
                            bucket_ref,
                            state,
                            &mut paint_properties,
                            symbol_instance,
                            feature,
                            writing_modes,
                            &quads,
                            canonical,
                            last_added_section,
                            &mut text_line,
                        );
                        last_added_section = Some(section);
                        symbol_instance.placed_right_text_index = Some(placed);
                        symbol_instance.placed_center_text_index = Some(placed);
                        symbol_instance.placed_left_text_index = Some(placed);
                    } else {
                        if symbol_instance.right_justified_glyph_quads_size() != 0 {
                            let quads = symbol_instance.right_justified_glyph_quads().clone();
                            let (section, placed) = self.add_symbol_glyph_quads(
                                bucket_ref,
                                state,
                                &mut paint_properties,
                                symbol_instance,
                                feature,
                                writing_modes,
                                &quads,
                                canonical,
                                last_added_section,
                                &mut text_line,
                            );
                            last_added_section = Some(section);
                            symbol_instance.placed_right_text_index = Some(placed);
                        }
                        if symbol_instance.center_justified_glyph_quads_size() != 0 {
                            let quads = symbol_instance.center_justified_glyph_quads().clone();
                            let (section, placed) = self.add_symbol_glyph_quads(
                                bucket_ref,
                                state,
                                &mut paint_properties,
                                symbol_instance,
                                feature,
                                writing_modes,
                                &quads,
                                canonical,
                                last_added_section,
                                &mut text_line,
                            );
                            last_added_section = Some(section);
                            symbol_instance.placed_center_text_index = Some(placed);
                        }
                        if symbol_instance.left_justified_glyph_quads_size() != 0 {
                            let quads = symbol_instance.left_justified_glyph_quads().clone();
                            let (section, placed) = self.add_symbol_glyph_quads(
                                bucket_ref,
                                state,
                                &mut paint_properties,
                                symbol_instance,
                                feature,
                                writing_modes,
                                &quads,
                                canonical,
                                last_added_section,
                                &mut text_line,
                            );
                            last_added_section = Some(section);
                            symbol_instance.placed_left_text_index = Some(placed);
                        }
                    }
                    if (writing_modes & WritingModeType::Vertical) != WritingModeType::None
                        && symbol_instance.vertical_glyph_quads_size() != 0
                    {
                        let quads = symbol_instance.vertical_glyph_quads().clone();
                        let (section, placed) = self.add_symbol_glyph_quads(
                            bucket_ref,
                            state,
                            &mut paint_properties,
                            symbol_instance,
                            feature,
                            WritingModeType::Vertical,
                            &quads,
                            canonical,
                            last_added_section,
                            &mut text_line,
                        );
                        last_added_section = Some(section);
                        symbol_instance.placed_vertical_text_index = Some(placed);
                    }
                    // The instance has text, so a section was added.
                    debug_assert!(last_added_section.is_some());
                    Self::update_paint_properties_for_section(
                        &mut paint_properties,
                        state,
                        feature,
                        last_added_section.unwrap_or(0),
                        canonical,
                    );
                }

                symbol_instance.release_shared_data();
            }
            state.symbol_instances = instances;

            if show_collision_boxes {
                Self::add_to_debug_buffers(state);
            }
            if !first_load {
                state.just_reloaded = true;
            }
        }
        bucket.state = state;
        bucket.paint_properties = paint_properties;

        bucket
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use mln_text::glyph::{FontFace, FontFaces, GlyphIDType};
    use mln_util::i18n::char_in_supported_script;

    use super::{BASELINE_OFFSET, GlyphTypeTable, SymbolAnchorType, SymbolLayout, evaluate_radial_offset};

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
        if mln_util::i18n::is_variation_selector(c)
            || (previous != GlyphIDType::FontPBF && mln_util::i18n::extends_previous(c))
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
        let rest = if system && !mln_util::i18n::breaks_or_stands_upright(c) {
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
