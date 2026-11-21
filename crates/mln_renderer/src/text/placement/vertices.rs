//! A bucket's vertices after a placement: each run's opacity, and where its
//! quads draw, for labels that follow lines, take variable anchors or turn
//! upright.

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use geometry::Point;
use glam::Vec2;
use mln_map_types::{TileProjector, View};
use mln_style_types::{AlignmentType, IconTextFitType, TextRotationAlignmentType, TextWritingModeType};
use mln_util::constants::ONE_EM;

use super::context::translation;
use super::{JointOpacityState, Placement, VariableOffset};
use crate::buckets::symbol_bucket::{Buffer, CollisionBuffer, SymbolBucket, SymbolBucketState};
use crate::layout::symbol_instance::{Justification, SymbolInstance, TextPart};
use crate::layout::symbol_projection::{
    LabelHeight, LabelPlaneProjector, LineLabelAlignment, SymbolElevation, add_dynamic_attributes,
    evaluate_size_for_feature, hide_glyphs, reproject_line_labels, symbol_ground,
    tile_skew_vectors,
};
use crate::render_layer::BucketPlacementData;
use crate::terrain::TerrainElevation;
use crate::text::collision_index::OverlapMode;
use crate::text::placement::CircleKey;

/// Appends `n` copies of `value` to `v`.
fn extend<T: Copy>(v: &mut Vec<T>, n: usize, value: T) {
    v.extend(std::iter::repeat_n(value, n));
}

/// The collision debug buffer a feature of `symbol` draws in.
fn collision_buffer(state: &mut SymbolBucketState, along_line: bool, text: bool) -> Option<&mut CollisionBuffer> {
    match (along_line, text) {
        (true, true) => state.text_collision_circle.as_mut(),
        (true, false) => state.icon_collision_circle.as_mut(),
        (false, true) => state.text_collision_box.as_mut(),
        (false, false) => state.icon_collision_box.as_mut(),
    }
    .filter(|buffer| !buffer.segments.is_empty())
}

/// The orientation a run of `vertical` text or its icon shows in.
fn orientation(vertical: bool) -> TextWritingModeType {
    if vertical { TextWritingModeType::Vertical } else { TextWritingModeType::Horizontal }
}

impl Placement {
    /// The orientation the symbol `id` shows in, where its bucket places
    /// either.
    fn orientation_of(&self, allow_vertical: bool, id: Option<NonZeroU32>) -> Option<TextWritingModeType> {
        if !allow_vertical {
            return None;
        }
        self.placed_orientations.get(&id?).copied()
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
        vertical: bool,
    ) -> bool {
        let shown = self.orientation_of(allow_vertical, id);
        if allow_vertical && shown != Some(orientation(vertical)) {
            return false;
        }
        let Some(offset) = id.and_then(|id| self.variable_offsets.get(&id)) else {
            return false;
        };
        vertical || text.runs.get(Justification::of_anchor(offset.anchor)).is_none_or(|justified| justified == run)
    }

    /// Sets the opacities of the bucket's symbols from the placement, and
    /// returns whether sorting the symbols for the map's bearing changed the
    /// order of the bucket's triangles. A symbol a bucket before it in the
    /// layer has, by `seen_cross_tile_ids`, is a duplicate, and hides.
    pub(crate) fn update_bucket_opacities(
        &self,
        bucket: &mut SymbolBucket,
        view: &View,
        seen_cross_tile_ids: &mut BTreeSet<NonZeroU32>,
    ) -> bool {
        let layout = &*bucket.layout;
        let variable_placement = bucket.has_variable_text_anchors();
        let has_icon_text_fit = *layout.icon_text_fit.uniform() != IconTextFitType::None;
        let rotate_with_map = *layout.text_rotation_alignment.uniform() == TextRotationAlignmentType::Map;
        let pitch_with_map = *layout.text_pitch_alignment.uniform() == AlignmentType::Map;
        let text_always = OverlapMode::text(layout) == OverlapMode::Always;
        let icon_always = OverlapMode::icon(layout) == OverlapMode::Always;
        let has_text = bucket.has_text_data();
        let has_icons = bucket.has_icon_data() || bucket.has_sdf_icon_data();
        let bucket_instance_id = bucket.bucket_instance_id;

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
        }
        for buffer in [
            &mut state.text_collision_box,
            &mut state.icon_collision_box,
            &mut state.text_collision_circle,
            &mut state.icon_collision_circle,
        ]
        .into_iter()
        .flatten()
        {
            buffer.dynamic_vertices.clear();
        }

        let symbols = std::mem::take(&mut state.symbol_instances);
        for (index, symbol) in symbols.iter().enumerate() {
            let id = symbol.cross_tile_id;
            let opacity = match id {
                Some(id) if seen_cross_tile_ids.insert(id) => {
                    self.opacities.get(&id).copied().unwrap_or(default_opacity)
                }
                _ => duplicate_opacity,
            };
            let hidden = opacity.is_hidden();
            if let Some(text) = &symbol.text {
                let vertex = SymbolBucket::opacity_attributes(opacity.text.placed, opacity.text.opacity);
                for (run, _) in text.runs.all() {
                    set_run_opacity(&mut state.text, run, hidden, vertex);
                }
            }
            if let Some(icon) = &symbol.icon {
                let vertex = SymbolBucket::opacity_attributes(opacity.icon.placed, opacity.icon.opacity);
                let buffer = if icon.sdf { &mut state.sdf_icon } else { &mut state.icon };
                for (run, _) in icon.runs() {
                    set_run_opacity(buffer, run, hidden, vertex);
                }
            }

            // Debug boxes show where a variable label last went, so a label
            // that just disappeared shows what it collided with; one never
            // placed shows its center box, unused.
            let offset = id.filter(|_| variable_placement).and_then(|id| self.variable_offsets.get(&id));
            let text_shift = offset
                .map_or(Vec2::ZERO, |offset| offset.screen_shift(rotate_with_map, pitch_with_map, view.angle() as f32));
            for (feature, is_text) in symbol.collision_features() {
                let Some(buffer) = collision_buffer(state, feature.along_line, is_text) else {
                    continue;
                };
                let placed = if is_text { opacity.text.placed } else { opacity.icon.placed };
                let box_count = feature.boxes.len();
                if feature.along_line {
                    let key = CircleKey { bucket_instance_id, symbol: index, text: is_text };
                    if let Some(circles) = self.collision_circles.get(&key) {
                        for circle in circles {
                            let vertex = SymbolBucket::collision_dynamic_vertex(placed, circle.is_none(), Vec2::ZERO);
                            extend(&mut buffer.dynamic_vertices, 4, vertex);
                        }
                    } else {
                        // Not placed: not loaded yet, or from a fading tile.
                        let vertex = SymbolBucket::collision_dynamic_vertex(placed, false, Vec2::ZERO);
                        extend(&mut buffer.dynamic_vertices, 4 * box_count, vertex);
                    }
                } else {
                    let used = !is_text || !variable_placement || offset.is_some();
                    let shift = if is_text || has_icon_text_fit { text_shift } else { Vec2::ZERO };
                    let vertex = SymbolBucket::collision_dynamic_vertex(placed, !used, shift);
                    extend(&mut buffer.dynamic_vertices, 4 * box_count, vertex);
                }
            }
        }
        state.symbol_instances = symbols;
        bucket.sort_features(view.angle() as f32)
    }

    /// Lays out where the bucket's labels draw, for those that follow their
    /// lines, take variable anchors or turn upright, standing on `ground`
    /// where terrain draws, and returns whether it did. With a layer ID, lays
    /// out that layer's own positions, which it needs only at another
    /// translation than the leader's.
    pub(crate) fn update_bucket_positions(
        &self,
        bucket: &mut SymbolBucket,
        view: &View,
        ground: Option<&TerrainElevation>,
        data: &BucketPlacementData,
        layer_id: Option<&str>,
    ) -> bool {
        let layout = &*bucket.layout;
        let tile = &data.tile;
        let along_line = *layout.symbol_placement.uniform() != mln_style_types::SymbolPlacementType::Point;
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
        // Labels along lines and at variable anchors stand on the ground.
        let ground = ground.filter(|_| along_line || has_variable_anchors);
        let ground = symbol_ground(ground, &tile.overscaled_id, &bucket.layout);
        let elevation = SymbolElevation::of(ground.as_ref(), &bucket.layout);
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
                        (&tile_projector, icon_translation, elevation),
                        &bucket.icon_size_binder,
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
                    (&tile_projector, text_translation, elevation),
                    &bucket.text_size_binder,
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
            let placed = (&tile_projector, text_translation, elevation);
            self.lay_out_variable_anchors(bucket, view, data, layer_id, placed);
            true
        } else if bucket.allow_vertical_placement && has_text {
            self.lay_out_orientations(bucket, layer_id);
            true
        } else {
            false
        }
    }

    /// Moves each label to the variable anchor it took, and an icon fitted
    /// to it with it, projected by the tile's projector, moved by the
    /// translation and standing on the elevation of `placed`.
    fn lay_out_variable_anchors(
        &self,
        bucket: &mut SymbolBucket,
        view: &View,
        data: &BucketPlacementData,
        layer_id: Option<&str>,
        placed: (&TileProjector<'_>, Vec2, SymbolElevation<'_>),
    ) {
        let (tile_projector, translation, elevation) = placed;
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
        let label_plane = LabelPlaneProjector::new(
            tile_projector,
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
        let shifted_anchor = |offset: &VariableOffset, run: &crate::buckets::symbol_bucket::PlacedSymbol| {
            let tile_anchor = run.anchor_point;
            // A label whose tile draws it nowhere shows nowhere either.
            let raised = elevation.at(tile_anchor, tile_anchor + translation, run.height_offset);
            let height = LabelHeight::Fixed(f64::from(raised.unwrap_or(run.height_offset)));
            let projected = if pitch_with_map {
                label_plane.to_clip_space_from_tile(tile_anchor, height)
            } else {
                label_plane.project(tile_anchor, height)
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
                label_plane.project(tile_anchor + (east * shift.x + south * shift.y) * correction, height).point
            } else if rotate_with_map {
                // Viewport labels stay east-west aligned, so the shift
                // follows the projected east.
                let east = label_plane.project(tile_anchor + Vec2::X, height).point;
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
            let offset = id.and_then(|id| self.variable_offsets.get(&id));
            for (run_index, vertical) in text.runs.all() {
                let run = &runs[run_index];
                let glyphs = run.glyph_offsets.len();
                let Some(offset) =
                    offset.filter(|_| !run.hidden && self.shows_run(allow_vertical, text, id, run_index, vertical))
                else {
                    hide_glyphs(glyphs, positions);
                    continue;
                };
                placed_any = true;
                let anchor = shifted_anchor(offset, run);
                if let Some(icon_anchor) = icon_anchors.get_mut(index) {
                    icon_anchor[usize::from(vertical)].get_or_insert(anchor);
                }
                for _ in 0..glyphs {
                    add_dynamic_attributes(anchor, run.angle, positions);
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
                for (run_index, vertical) in icon.runs() {
                    let run = &runs[run_index];
                    match icon_anchors[index][usize::from(vertical)].filter(|_| !run.hidden) {
                        Some(anchor) => {
                            for _ in 0..run.glyph_offsets.len() {
                                add_dynamic_attributes(anchor, run.angle, positions);
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
        let write = |buffer: &mut Buffer,
                     runs: &mut dyn Iterator<Item = (Option<TextWritingModeType>, usize, bool)>| {
            let (positions, placed, _) = buffer.positions_and_symbols(layer_id);
            positions.clear();
            for (shown, run_index, vertical) in runs {
                let run = &placed[run_index];
                if run.hidden || shown != Some(orientation(vertical)) {
                    hide_glyphs(run.glyph_offsets.len(), positions);
                } else {
                    for _ in 0..run.glyph_offsets.len() {
                        add_dynamic_attributes(run.anchor_point, run.angle, positions);
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
                    .flat_map(move |text| text.runs.all().map(move |(run, vertical)| (shown, run, vertical)))
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
                            .flat_map(move |icon| icon.runs().map(move |(run, vertical)| (shown, run, vertical)))
                    }),
                );
            }
        }
    }
}

/// Sets run `run` of `buffer` hidden or not, and its vertices' opacity to
/// `vertex`.
fn set_run_opacity(
    buffer: &mut Buffer,
    run: usize,
    hidden: bool,
    vertex: crate::buckets::symbol_bucket::SymbolOpacityVertex,
) {
    let run = &mut buffer.placed_symbols[run];
    run.hidden = hidden;
    extend(&mut buffer.opacity_attribute_data, 4 * run.glyph_offsets.len(), vertex);
}
