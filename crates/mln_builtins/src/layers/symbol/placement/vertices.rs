//! A bucket's vertices after a placement: each run's opacity, and where its
//! quads draw, for labels that follow lines, take variable anchors or turn
//! upright.

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use geometry::Point;
use glam::Vec2;
use mln_engine::camera::{TileProjector, View};
use mln_engine::draw::ground::TerrainElevation;
use mln_engine::placement::Overlap;
use mln_engine::text::constants::ONE_EM;
use mln_style::types::{AlignmentType, IconTextFitType, TextRotationAlignmentType, TextWritingModeType};

use super::collision::{icon_overlap, text_overlap};
use super::context::translation;
use super::{BucketPlacementData, JointOpacityState, Symbols, VariableOffset};
use crate::layers::symbol::bucket::{Buffer, SymbolBucket, SymbolBucketState};
use crate::layers::symbol::instance::{Justification, SymbolInstance, TextPart};
use crate::layers::symbol::projection::{
    LabelPlaneProjector, LineLabelAlignment, SymbolHeights, add_dynamic_attributes, evaluate_size_for_feature,
    hide_glyphs, reproject_line_labels, tile_skew_vectors,
};

/// Appends `n` copies of `value` to `v`.
fn extend<T: Copy>(v: &mut Vec<T>, n: usize, value: T) {
    v.extend(std::iter::repeat_n(value, n));
}

/// What the current placement shows of the symbols, on the ground its
/// symbols stand on, where terrain draws.
pub(super) struct Shown<'a> {
    pub(super) symbols: &'a Symbols,
    pub(super) ground: Option<&'a TerrainElevation>,
}

impl Shown<'_> {
    /// The orientation the symbol `id` shows in, where its bucket places
    /// either.
    fn orientation_of(&self, allow_vertical: bool, id: Option<NonZeroU32>) -> Option<TextWritingModeType> {
        if !allow_vertical {
            return None;
        }
        self.symbols.placed_orientations.get(&id?).copied()
    }

    /// Whether the text run `run` of `text`, `vertical` or not, is the one
    /// its label at a variable anchor shows: of the orientation it was
    /// placed in, where its bucket places either (`allow_vertical`), and of
    /// the justification its anchor takes.
    fn shows_run(
        &self,
        allow_vertical: bool,
        text: &TextPart,
        id: Option<NonZeroU32>,
        run: usize,
        orientation: TextWritingModeType,
    ) -> bool {
        let shown = self.orientation_of(allow_vertical, id);
        if allow_vertical && shown != Some(orientation) {
            return false;
        }
        let Some(offset) = id.and_then(|id| self.symbols.variable_offsets.get(&id)) else {
            return false;
        };
        orientation == TextWritingModeType::Vertical
            || text.runs.get(Justification::of_anchor(offset.anchor)).is_none_or(|justified| justified == run)
    }

    /// Sets the opacities of the bucket's symbols from the placement, and
    /// their heights where they stand on its ground, and returns whether
    /// sorting the symbols for the map's bearing changed the order of the
    /// bucket's triangles. A symbol a bucket before it in the layer has, by
    /// `seen_cross_tile_ids`, is a duplicate, and hides.
    pub(crate) fn update_bucket_opacities(
        &self,
        bucket: &mut SymbolBucket,
        view: &View,
        data: &BucketPlacementData,
        seen_cross_tile_ids: &mut BTreeSet<NonZeroU32>,
    ) -> bool {
        let layout = &*bucket.layout;
        let text_always = text_overlap(layout) == Overlap::Always;
        let icon_always = icon_overlap(layout) == Overlap::Always;
        let has_text = bucket.has_text_data();
        let has_icons = bucket.has_icon_data() || bucket.has_sdf_icon_data();

        // A symbol that always overlaps shows before placement reaches it,
        // unless it waits for a part that may not overlap, so it doesn't
        // flicker as its tile loads.
        let shows_early = *layout.symbol_screen_space.uniform() || bucket.state.just_reloaded;
        let default_opacity = JointOpacityState::new(
            shows_early && text_always && (icon_always || !has_icons || *layout.icon_optional.uniform()),
            shows_early && icon_always && (text_always || !has_text || *layout.text_optional.uniform()),
            true,
        );
        let duplicate_opacity = JointOpacityState::new(false, false, true);

        let state = &mut bucket.state;
        for buffer in [&mut state.text, &mut state.icon, &mut state.sdf_icon] {
            buffer.opacity_attribute_data.clear();
            buffer.elevation_attribute_data.clear();
        }
        let heights = SymbolHeights::new(self.ground, data.tile.id, layout);

        let symbols = std::mem::take(&mut state.symbol_instances);
        for symbol in &symbols {
            let id = symbol.cross_tile_id;
            let opacity = match id {
                Some(id) if seen_cross_tile_ids.insert(id) => {
                    self.symbols.opacities.get(&id).copied().unwrap_or(default_opacity)
                }
                _ => duplicate_opacity,
            };
            let hidden = opacity.is_hidden();
            if let Some(text) = &symbol.text {
                let vertex = SymbolBucket::opacity_attributes(opacity.text.placed, opacity.text.opacity);
                for (run, _) in text.runs.all() {
                    set_run_opacity(&mut state.text, run, hidden, vertex, heights);
                }
            }
            if let Some(icon) = &symbol.icon {
                let vertex = SymbolBucket::opacity_attributes(opacity.icon.placed, opacity.icon.opacity);
                let buffer = if icon.sdf { &mut state.sdf_icon } else { &mut state.icon };
                for (run, _) in icon.runs() {
                    set_run_opacity(buffer, run, hidden, vertex, heights);
                }
            }
        }
        state.symbol_instances = symbols;
        bucket.sort_features(view.angle() as f32)
    }

    /// Lays out where the bucket's labels draw, for those that follow their
    /// lines, take variable anchors or turn upright, and returns whether it
    /// did. With a layer ID, lays out that layer's own positions, which it
    /// needs only at another translation than the leader's.
    pub(crate) fn update_bucket_positions(
        &self,
        bucket: &mut SymbolBucket,
        view: &View,
        data: &BucketPlacementData,
        layer_id: Option<&str>,
    ) -> bool {
        let layout = &*bucket.layout;
        let tile = &data.tile;
        let along_line = *layout.symbol_placement.uniform() != mln_style::types::SymbolPlacementType::Point;
        let has_text = bucket.has_text_data();
        let has_variable_anchors = bucket.has_variable_text_anchors() && has_text;
        let screen_space = *layout.symbol_screen_space.uniform();
        let text_translation = translation(&tile.id, &data.text_translate, view, screen_space);
        let icon_translation = translation(&tile.id, &data.icon_translate, view, screen_space);
        let state = &mut bucket.state;
        match layer_id {
            None => {
                state.dynamic_text_translation = text_translation;
                state.dynamic_icon_translation = icon_translation;
            }
            Some(layer_id) => {
                let moved = text_translation != state.dynamic_text_translation
                    || icon_translation != state.dynamic_icon_translation;
                if (!along_line && !has_variable_anchors)
                    || (!state.has_layer_dynamic_attribute_data(layer_id) && !moved)
                {
                    return false;
                }
            }
        }

        let tile_projector = TileProjector::with_data(view, &tile.id, tile.projection);
        let heights = SymbolHeights::new(self.ground, tile.id, layout);
        if along_line {
            let mut laid_out = false;
            if *layout.icon_rotation_alignment.uniform() == AlignmentType::Map {
                let alignment = LineLabelAlignment {
                    pitch_with_map: *layout.icon_pitch_alignment.uniform() == AlignmentType::Map,
                    rotate_with_map: true,
                    keep_upright: *layout.icon_keep_upright.uniform(),
                };
                for buffer in [&mut bucket.state.sdf_icon, &mut bucket.state.icon] {
                    if buffer.segments.is_empty() {
                        continue;
                    }
                    let (positions, runs, pools) = buffer.positions_and_symbols(layer_id);
                    reproject_line_labels(
                        positions,
                        runs,
                        pools,
                        &tile_projector,
                        heights,
                        &bucket.icon_size_binder,
                        icon_translation,
                        alignment,
                    );
                    laid_out = true;
                }
            }
            let rotation_alignment = *layout.text_rotation_alignment.uniform();
            if has_text && rotation_alignment != TextRotationAlignmentType::Viewport {
                let (positions, runs, pools) = bucket.state.text.positions_and_symbols(layer_id);
                reproject_line_labels(
                    positions,
                    runs,
                    pools,
                    &tile_projector,
                    heights,
                    &bucket.text_size_binder,
                    text_translation,
                    LineLabelAlignment {
                        pitch_with_map: *layout.text_pitch_alignment.uniform() == AlignmentType::Map,
                        rotate_with_map: rotation_alignment == TextRotationAlignmentType::Map,
                        keep_upright: *layout.text_keep_upright.uniform(),
                    },
                );
                laid_out = true;
            }
            laid_out
        } else if has_variable_anchors {
            self.lay_out_variable_anchors(bucket, view, data, layer_id, &tile_projector, text_translation);
            true
        } else if bucket.allow_vertical_placement && has_text {
            self.lay_out_orientations(bucket, layer_id);
            true
        } else {
            false
        }
    }

    /// Moves each label to the variable anchor it took, and an icon fitted
    /// to it with it.
    fn lay_out_variable_anchors(
        &self,
        bucket: &mut SymbolBucket,
        view: &View,
        data: &BucketPlacementData,
        layer_id: Option<&str>,
        tile_projector: &TileProjector<'_>,
        translation: Vec2,
    ) {
        let layout = &*bucket.layout;
        let tile = &data.tile;
        let zoom = view.zoom() as f32;
        let text_size = bucket.text_size_binder.evaluate_for_zoom(zoom);
        let tile_scale = 2f64.powf(view.zoom() - f64::from(tile.overscaled_id.overscaled_z)) as f32;
        let rotate_with_map = *layout.text_rotation_alignment.uniform() == TextRotationAlignmentType::Map;
        let pitch_with_map = *layout.text_pitch_alignment.uniform() == AlignmentType::Map;
        let fit_icons = *layout.icon_text_fit.uniform() != IconTextFitType::None
            && (bucket.has_icon_data() || bucket.has_sdf_icon_data());
        // The shader leaves the translation to us for variable anchors, so
        // the shifted anchor carries it.
        let heights = SymbolHeights::new(self.ground, tile.id, layout);
        let label_plane = LabelPlaneProjector::new(
            tile_projector,
            heights,
            pitch_with_map,
            rotate_with_map,
            tile.id.pixels_to_tile_units(1.0, zoom),
            translation,
        );
        // The shift follows the axes the collision box was projected along,
        // so the label lands in its own box.
        let (east, south) = if pitch_with_map && !rotate_with_map {
            let (east, south) = tile_skew_vectors(view);
            (east.as_vec2(), south.as_vec2())
        } else {
            (Vec2::X, Vec2::Y)
        };
        let shifted_anchor = |offset: &VariableOffset, run: &crate::layers::symbol::bucket::PlacedSymbol| {
            let label_plane = label_plane.raised(run.height_offset);
            let tile_anchor = run.anchor_point;
            let projected = if pitch_with_map {
                label_plane.to_clip_space_from_tile(tile_anchor)
            } else {
                label_plane.project(tile_anchor)
            };
            let projected_anchor = projected.point;
            let perspective_ratio = 0.5 + 0.5 * (view.camera_to_center_distance() as f32 / projected.camera_distance);
            let mut size = evaluate_size_for_feature(&text_size, run) * perspective_ratio / ONE_EM;
            if pitch_with_map {
                // From pixels to tile units.
                size *= bucket.tile_pixel_ratio / tile_scale;
            }
            let shift = offset.render_shift(size);
            if pitch_with_map {
                // Pitched text shifts in tile units before it projects.
                let correction = tile_projector.pitched_text_correction(&Point::new(
                    f64::from(tile_anchor.x + translation.x),
                    f64::from(tile_anchor.y + translation.y),
                )) as f32;
                label_plane.project(tile_anchor + (east * shift.x + south * shift.y) * correction).point
            } else if rotate_with_map {
                // Viewport labels stay east-west aligned, so the shift
                // follows the projected east, at the anchor's height, as
                // the shaders and the collision box turn it.
                let east = label_plane.project_at(tile_anchor + Vec2::X, label_plane.elevation(tile_anchor)).point;
                let to_east = east - projected_anchor;
                let angle = (to_east.y / to_east.x).atan() + if to_east.x < 0.0 { std::f32::consts::PI } else { 0.0 };
                projected_anchor + Vec2::from_angle(angle).rotate(shift)
            } else {
                projected_anchor + shift
            }
        };

        let allow_vertical = bucket.allow_vertical_placement;
        let SymbolBucketState {
            symbol_instances: symbols,
            text: text_buffer,
            icon,
            sdf_icon,
            has_variable_placement,
            ..
        } = &mut bucket.state;
        // Where each symbol's icon, and the icon fitted to its vertical
        // text, follow its text.
        let mut icon_anchors: Vec<[Option<Vec2>; 2]> = vec![[None; 2]; if fit_icons { symbols.len() } else { 0 }];
        let mut placed_any = false;
        let (positions, runs, _) = text_buffer.positions_and_symbols(layer_id);
        positions.clear();
        for (index, symbol) in symbols.iter().enumerate() {
            let Some(text) = &symbol.text else {
                continue;
            };
            let id = symbol.cross_tile_id;
            let offset = id.and_then(|id| self.symbols.variable_offsets.get(&id));
            for (run_index, orientation) in text.runs.all() {
                let run = &runs[run_index];
                let glyphs = run.glyph_offsets.len();
                let Some(offset) =
                    offset.filter(|_| !run.hidden && self.shows_run(allow_vertical, text, id, run_index, orientation))
                else {
                    hide_glyphs(glyphs, positions);
                    continue;
                };
                placed_any = true;
                let anchor = shifted_anchor(offset, run);
                if let Some(icon_anchor) = icon_anchors.get_mut(index) {
                    icon_anchor[orientation as usize].get_or_insert(anchor);
                }
                for _ in 0..glyphs {
                    add_dynamic_attributes(anchor, run.angle, 0.0, positions);
                }
            }
        }
        if layer_id.is_none() {
            *has_variable_placement = placed_any;
        }
        if !(fit_icons && placed_any) {
            return;
        }
        for (buffer, sdf) in [(icon, false), (sdf_icon, true)] {
            let (positions, runs, _) = buffer.positions_and_symbols(layer_id);
            positions.clear();
            for (index, symbol) in symbols.iter().enumerate() {
                let Some(icon) = symbol.icon.as_ref().filter(|icon| icon.sdf == sdf) else {
                    continue;
                };
                for (run_index, orientation) in icon.runs() {
                    let run = &runs[run_index];
                    match icon_anchors[index][orientation as usize].filter(|_| !run.hidden) {
                        Some(anchor) => {
                            for _ in 0..run.glyph_offsets.len() {
                                add_dynamic_attributes(anchor, run.angle, 0.0, positions);
                            }
                        }
                        None => hide_glyphs(run.glyph_offsets.len(), positions),
                    }
                }
            }
        }
    }

    /// Shows each label in the orientation it was placed in, and an icon
    /// fitted to it with it.
    fn lay_out_orientations(&self, bucket: &mut SymbolBucket, layer_id: Option<&str>) {
        let fit_icons = *bucket.layout.icon_text_fit.uniform() != IconTextFitType::None;
        let allow_vertical = bucket.allow_vertical_placement;
        let SymbolBucketState { symbol_instances: symbols, text, icon, sdf_icon, .. } = &mut bucket.state;
        let shown = |symbol: &SymbolInstance| self.orientation_of(allow_vertical, symbol.cross_tile_id);
        let write =
            |buffer: &mut Buffer,
             runs: &mut dyn Iterator<Item = (Option<TextWritingModeType>, usize, TextWritingModeType)>| {
                let (positions, placed, _) = buffer.positions_and_symbols(layer_id);
                positions.clear();
                for (shown, run_index, orientation) in runs {
                    let run = &placed[run_index];
                    if run.hidden || shown != Some(orientation) {
                        hide_glyphs(run.glyph_offsets.len(), positions);
                    } else {
                        for _ in 0..run.glyph_offsets.len() {
                            add_dynamic_attributes(run.anchor_point, run.angle, 0.0, positions);
                        }
                    }
                }
            };
        write(
            text,
            &mut symbols.iter().flat_map(|symbol| {
                let shown = shown(symbol);
                symbol
                    .text
                    .iter()
                    .flat_map(move |text| text.runs.all().map(move |(run, orientation)| (shown, run, orientation)))
            }),
        );
        // An icon fitted to its text turns with it.
        if fit_icons {
            for (buffer, sdf) in [(icon, false), (sdf_icon, true)] {
                write(
                    buffer,
                    &mut symbols.iter().flat_map(|symbol| {
                        let shown = shown(symbol);
                        symbol
                            .icon
                            .iter()
                            .filter(move |icon| icon.sdf == sdf)
                            .flat_map(move |icon| icon.runs().map(move |(run, orientation)| (shown, run, orientation)))
                    }),
                );
            }
        }
    }
}

/// Sets run `run` of `buffer` hidden or not, and its vertices' opacity to
/// `vertex` and, where its symbols stand on the ground, their height above
/// sea level to that of its anchor standing at `heights`. The runs come in
/// the order of their vertices, which these follow.
fn set_run_opacity(
    buffer: &mut Buffer,
    run: usize,
    hidden: bool,
    vertex: crate::layers::symbol::bucket::SymbolOpacityVertex,
    heights: SymbolHeights<'_>,
) {
    let run = &mut buffer.placed_symbols[run];
    run.hidden = hidden;
    let vertices = 4 * run.glyph_offsets.len();
    extend(&mut buffer.opacity_attribute_data, vertices, vertex);
    if heights.on_ground() {
        extend(&mut buffer.elevation_attribute_data, vertices, heights.at(run.anchor_point, run.height_offset));
    }
}
