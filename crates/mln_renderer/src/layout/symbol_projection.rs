// Ports src/mln/layout/symbol_projection.hpp and
// src/mln/layout/symbol_projection.cpp: the label-plane matrices and the
// CPU line-label projection (see the C++ file's overview of coordinate
// spaces). MLN_USE_SYMBOL_INSTANCING is off: four dynamic vertices per
// glyph.

use geometry::Point;
use mln_map_types::TransformState;
use mln_text::glyph::WritingModeType;
use mln_tile_id::geometry_tile_data::GeometryCoordinates;
use mln_util::constants::ONE_EM;
use mln_util::mat4::{Mat4, matrix};
use mln_util::vectors::Vec4;

use crate::buckets::symbol_bucket::{
    PlacedSymbol, SymbolBucket, SymbolDynamicLayoutVertex, SymbolSizeBinder, ZoomEvaluatedSize,
};
use crate::render_tile::RenderTile;

/// `getTileSkewVectors(state, vecEast, vecSouth)`.
pub fn get_tile_skew_vectors(state: &TransformState) -> ([f64; 2], [f64; 2]) {
    let cos_roll = state.get_roll().cos();
    let sin_roll = state.get_roll().sin();
    let cos_pitch = state.get_pitch().cos();
    let cos_bearing = (-state.get_bearing()).cos();
    let sin_bearing = (-state.get_bearing()).sin();
    let mut vec_south = [
        -cos_bearing * cos_pitch * sin_roll - sin_bearing * cos_roll,
        -sin_bearing * cos_pitch * sin_roll + cos_bearing * cos_roll,
    ];
    let vec_south_len = vec_south[0].hypot(vec_south[1]);
    if vec_south_len < 1.0e-9 {
        vec_south = [0.0, 0.0];
    } else {
        vec_south[0] /= vec_south_len;
        vec_south[1] /= vec_south_len;
    }
    let mut vec_east = [
        cos_bearing * cos_pitch * cos_roll - sin_bearing * sin_roll,
        sin_bearing * cos_pitch * cos_roll + cos_bearing * sin_roll,
    ];
    let vec_east_len = vec_east[0].hypot(vec_east[1]);
    if vec_east_len < 1.0e-9 {
        vec_east = [0.0, 0.0];
    } else {
        vec_east[0] /= vec_east_len;
        vec_east[1] /= vec_east_len;
    }
    (vec_east, vec_south)
}

/// `getTileSkewMatrix(state)`.
pub fn get_tile_skew_matrix(state: &TransformState) -> Mat4 {
    let (vec_east, vec_south) = get_tile_skew_vectors(state);
    let mut m: Mat4 = [0.0; 16];
    matrix::identity(&mut m);
    m[0] = vec_east[0];
    m[1] = vec_east[1];
    m[4] = vec_south[0];
    m[5] = vec_south[1];
    m
}

/// `getLabelPlaneMatrix(posMatrix, pitchWithMap, rotateWithMap, state,
/// pixelsToTileUnits)`: a matrix for converting from tile units to the
/// correct label coordinate space.
pub fn get_label_plane_matrix(
    pos_matrix: &Mat4,
    pitch_with_map: bool,
    rotate_with_map: bool,
    state: &TransformState,
    pixels_to_tile_units: f32,
) -> Mat4 {
    let mut m: Mat4 = [0.0; 16];
    matrix::identity(&mut m);
    if pitch_with_map {
        let s = f64::from(1.0 / pixels_to_tile_units);
        let a = m;
        matrix::scale(&mut m, &a, s, s, 1.0);
        if !rotate_with_map {
            let skew = get_tile_skew_matrix(state);
            let mut inverted = skew;
            matrix::invert(&mut inverted, &skew);
            let a = m;
            matrix::multiply(&mut m, &a, &inverted);
        }
    } else {
        let size = state.get_size();
        let a = m;
        matrix::scale(&mut m, &a, f64::from(size.width) / 2.0, -(f64::from(size.height) / 2.0), 1.0);
        let a = m;
        matrix::translate(&mut m, &a, 1.0, -1.0, 0.0);
        let a = m;
        matrix::multiply(&mut m, &a, pos_matrix);
    }
    m
}

/// `getGlCoordMatrix(posMatrix, pitchWithMap, rotateWithMap, state,
/// pixelsToTileUnits)`: a matrix for converting from the correct label
/// coordinate space to gl coords.
pub fn get_gl_coord_matrix(
    pos_matrix: &Mat4,
    pitch_with_map: bool,
    rotate_with_map: bool,
    state: &TransformState,
    pixels_to_tile_units: f32,
) -> Mat4 {
    let mut m: Mat4 = [0.0; 16];
    matrix::identity(&mut m);
    if pitch_with_map {
        let a = m;
        matrix::multiply(&mut m, &a, pos_matrix);
        let s = f64::from(pixels_to_tile_units);
        let a = m;
        matrix::scale(&mut m, &a, s, s, 1.0);
        if !rotate_with_map {
            let a = m;
            matrix::multiply(&mut m, &a, &get_tile_skew_matrix(state));
        }
    } else {
        let size = state.get_size();
        let a = m;
        matrix::scale(&mut m, &a, 1.0, -1.0, 1.0);
        let a = m;
        matrix::translate(&mut m, &a, -1.0, -1.0, 0.0);
        let a = m;
        matrix::scale(&mut m, &a, 2.0 / f64::from(size.width), 2.0 / f64::from(size.height), 1.0);
    }
    m
}

/// `TileDistance`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileDistance {
    pub prev_tile_distance: f32,
    pub last_segment_viewport_distance: f32,
}

impl TileDistance {
    pub fn new(prev_tile_distance: f32, last_segment_viewport_distance: f32) -> Self {
        TileDistance { prev_tile_distance, last_segment_viewport_distance }
    }
}

/// `PlacedGlyph`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedGlyph {
    pub point: Point<f32>,
    pub angle: f32,
    pub tile_distance: Option<TileDistance>,
}

impl PlacedGlyph {
    pub fn new(point: Point<f32>, angle: f32, tile_distance: Option<TileDistance>) -> Self {
        PlacedGlyph { point, angle, tile_distance }
    }
}

/// `PointAndCameraDistance = std::pair<Point<float>, float>`.
pub type PointAndCameraDistance = (Point<f32>, f32);

/// `Point<float> + Point<float>`.
fn add(a: Point<f32>, b: Point<f32>) -> Point<f32> {
    Point::new(a.x + b.x, a.y + b.y)
}

/// `Point<float> - Point<float>`.
fn sub(a: Point<f32>, b: Point<f32>) -> Point<f32> {
    Point::new(a.x - b.x, a.y - b.y)
}

/// `Point<float> * float`.
fn mul(a: Point<f32>, b: f32) -> Point<f32> {
    Point::new(a.x * b, a.y * b)
}

/// `util::mag<float>(Point<float>)` (util/math.hpp): a float sqrt.
fn mag_f32(a: Point<f32>) -> f32 {
    (a.x * a.x + a.y * a.y).sqrt()
}

/// `util::unit<float>(Point<float>)`.
fn unit_f32(a: Point<f32>) -> Point<f32> {
    let magnitude = mag_f32(a);
    if magnitude == 0.0 {
        return a;
    }
    mul(a, 1.0 / magnitude)
}

/// `util::perp(Point<float>)`.
fn perp(a: Point<f32>) -> Point<f32> {
    Point::new(-a.y, a.x)
}

/// `util::dist<float>(Point<float>, Point<float>)`.
fn dist_f32(a: Point<f32>, b: Point<f32>) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

/// `convertPoint<float>(Point<int16_t>)`.
fn convert_point(p: &Point<i16>) -> Point<f32> {
    Point::new(p.x as f32, p.y as f32)
}

/// `project(point, matrix)`.
pub fn project(point: Point<f32>, matrix: &Mat4) -> PointAndCameraDistance {
    let mut pos: Vec4 = [point.x as f64, point.y as f64, 0.0, 1.0];
    let a = pos;
    matrix::transform_mat4(&mut pos, &a, matrix);
    (Point::new((pos[0] / pos[3]) as f32, (pos[1] / pos[3]) as f32), pos[3] as f32)
}

/// `evaluateSizeForFeature(zoomEvaluatedSize, placedSymbol)`.
pub fn evaluate_size_for_feature(zoom_evaluated_size: &ZoomEvaluatedSize, placed_symbol: &PlacedSymbol) -> f32 {
    if zoom_evaluated_size.is_feature_constant {
        zoom_evaluated_size.size
    } else if zoom_evaluated_size.is_zoom_constant {
        placed_symbol.lower_size
    } else {
        placed_symbol.lower_size + zoom_evaluated_size.size_t * (placed_symbol.upper_size - placed_symbol.lower_size)
    }
}

/// `isVisible(anchorPos, clippingBuffer)`.
fn is_visible(anchor_pos: &Vec4, clipping_buffer: &[f64; 2]) -> bool {
    let x = anchor_pos[0] / anchor_pos[3];
    let y = anchor_pos[1] / anchor_pos[3];
    x >= -clipping_buffer[0] && x <= clipping_buffer[0] && y >= -clipping_buffer[1] && y <= clipping_buffer[1]
}

/// `addDynamicAttributes(anchorPoint, angle, dynamicAttributeData)`.
pub fn add_dynamic_attributes(
    anchor_point: Point<f32>,
    angle: f32,
    dynamic_attribute_data: &mut Vec<SymbolDynamicLayoutVertex>,
) {
    let dynamic_attributes = SymbolBucket::dynamic_layout_attributes(anchor_point, angle);
    dynamic_attribute_data.push(dynamic_attributes);
    dynamic_attribute_data.push(dynamic_attributes);
    dynamic_attribute_data.push(dynamic_attributes);
    dynamic_attribute_data.push(dynamic_attributes);
}

/// `hideGlyphs(numGlyphs, dynamicVertexArray)`.
pub fn hide_glyphs(num_glyphs: usize, dynamic_vertex_array: &mut Vec<SymbolDynamicLayoutVertex>) {
    let count = 4;
    let offscreen_point = Point::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    if dynamic_vertex_array.is_empty() {
        dynamic_vertex_array.reserve(count * num_glyphs);
    }
    for _ in 0..num_glyphs {
        add_dynamic_attributes(offscreen_point, 0.0, dynamic_vertex_array);
    }
}

/// `PlacementResult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlacementResult {
    Ok,
    NotEnoughRoom,
    NeedsFlipping,
    UseVertical,
}

/// `projectTruncatedLineSegment(...)`.
fn project_truncated_line_segment(
    previous_tile_point: Point<f32>,
    current_tile_point: Point<f32>,
    previous_projected_point: Point<f32>,
    minimum_length: f32,
    projection_matrix: &Mat4,
) -> Point<f32> {
    // We are assuming "previousTilePoint" won't project to a point within one
    // unit of the camera plane If it did, that would mean our label extended
    // all the way out from within the viewport to a (very distant) point near
    // the plane of the camera. We wouldn't be able to render the label anyway
    // once it crossed the plane of the camera.
    let projected_unit_vertex =
        project(add(previous_tile_point, unit_f32(sub(previous_tile_point, current_tile_point))), projection_matrix).0;
    let projected_unit_segment = sub(previous_projected_point, projected_unit_vertex);

    add(previous_projected_point, mul(projected_unit_segment, minimum_length / mag_f32(projected_unit_segment)))
}

/// `placeGlyphAlongLine(...)`.
#[allow(clippy::too_many_arguments)]
fn place_glyph_along_line(
    offset_x: f32,
    line_offset_x: f32,
    line_offset_y: f32,
    flip: bool,
    projected_anchor_point: Point<f32>,
    tile_anchor_point: Point<f32>,
    anchor_segment: u16,
    line: &GeometryCoordinates,
    tile_distances: &[f32],
    label_plane_matrix: &Mat4,
    return_tile_distance: bool,
) -> Option<PlacedGlyph> {
    let combined_offset_x = if flip { offset_x - line_offset_x } else { offset_x + line_offset_x };

    let mut dir: i16 = if combined_offset_x > 0.0 { 1 } else { -1 };

    let mut angle: f32 = 0.0;
    if flip {
        // The label needs to be flipped to keep text upright.
        // Iterate in the reverse direction.
        dir *= -1;
        angle = std::f32::consts::PI;
    }

    if dir < 0 {
        angle += std::f32::consts::PI;
    }

    let mut current_index: i32 = if dir > 0 { anchor_segment as i32 } else { anchor_segment as i32 + 1 };

    let initial_index = current_index;
    let mut current = projected_anchor_point;
    let mut prev = projected_anchor_point;
    let mut distance_to_prev: f32 = 0.0;
    let mut current_segment_distance: f32 = 0.0;
    let abs_offset_x = combined_offset_x.abs();

    while distance_to_prev + current_segment_distance <= abs_offset_x {
        current_index += dir as i32;

        // offset does not fit on the projected line
        if current_index < 0 || current_index >= line.len() as i32 {
            return None;
        }

        prev = current;
        let projection = project(convert_point(&line[current_index as usize]), label_plane_matrix);
        if projection.1 > 0.0 {
            current = projection.0;
        } else {
            // The vertex is behind the plane of the camera, so we can't project it
            // Instead, we'll create a vertex along the line that's far enough to include the glyph
            let previous_tile_point = if distance_to_prev == 0.0 {
                tile_anchor_point
            } else {
                convert_point(&line[(current_index - dir as i32) as usize])
            };
            let current_tile_point = convert_point(&line[current_index as usize]);
            current = project_truncated_line_segment(
                previous_tile_point,
                current_tile_point,
                prev,
                abs_offset_x - distance_to_prev + 1.0,
                label_plane_matrix,
            );
        }

        distance_to_prev += current_segment_distance;
        current_segment_distance = dist_f32(prev, current);
    }

    // The point is on the current segment. Interpolate to find it.
    let segment_interpolation_t = (abs_offset_x - distance_to_prev) / current_segment_distance;
    let prev_to_current = sub(current, prev);
    let mut p = add(mul(prev_to_current, segment_interpolation_t), prev);

    // offset the point from the line to text-offset and icon-offset
    // (`util::mag` defaults to a double result: the float sqrt promoted.)
    let mag = mag_f32(prev_to_current) as f64;
    p = add(p, mul(perp(prev_to_current), ((line_offset_y * dir as f32) as f64 / mag) as f32));

    let segment_angle = angle + (current.y - prev.y).atan2(current.x - prev.x);

    Some(PlacedGlyph::new(
        p,
        segment_angle,
        if return_tile_distance {
            Some(TileDistance::new(
                if (current_index - dir as i32) == initial_index {
                    0.0
                } else {
                    tile_distances[(current_index - dir as i32) as usize]
                },
                abs_offset_x - distance_to_prev,
            ))
        } else {
            None
        },
    ))
}

/// `placeFirstAndLastGlyph(...)`.
#[allow(clippy::too_many_arguments)]
pub fn place_first_and_last_glyph(
    font_scale: f32,
    line_offset_x: f32,
    line_offset_y: f32,
    flip: bool,
    anchor_point: Point<f32>,
    tile_anchor_point: Point<f32>,
    symbol: &PlacedSymbol,
    label_plane_matrix: &Mat4,
    return_tile_distance: bool,
) -> Option<(PlacedGlyph, PlacedGlyph)> {
    let (Some(&first_glyph_offset), Some(&last_glyph_offset)) =
        (symbol.glyph_offsets.first(), symbol.glyph_offsets.last())
    else {
        debug_assert!(false);
        return None;
    };

    let first_placed_glyph = place_glyph_along_line(
        font_scale * first_glyph_offset,
        line_offset_x,
        line_offset_y,
        flip,
        anchor_point,
        tile_anchor_point,
        symbol.segment as u16,
        &symbol.line,
        &symbol.tile_distances,
        label_plane_matrix,
        return_tile_distance,
    )?;

    let last_placed_glyph = place_glyph_along_line(
        font_scale * last_glyph_offset,
        line_offset_x,
        line_offset_y,
        flip,
        anchor_point,
        tile_anchor_point,
        symbol.segment as u16,
        &symbol.line,
        &symbol.tile_distances,
        label_plane_matrix,
        return_tile_distance,
    )?;

    Some((first_placed_glyph, last_placed_glyph))
}

/// Whether a horizontal line label kept upright has to be drawn flipped, given
/// where its first and last glyph project (gl coordinates) when it is drawn
/// unflipped. Without a keep-upright margin (`keep_upright_margin` 0, the
/// default), or when it is drawn for the first time (`was_flipped` unset), it
/// is flipped when it runs right to left. With a margin, in degrees (see
/// `MapOptions::with_keep_upright_margin`), it keeps the direction it was
/// drawn with until that direction is more than the margin past the vertical:
/// without it a label on a line close to the vertical of a rotating map turns
/// over and back at every small rotation.
pub fn line_label_needs_flip(
    first_point: Point<f32>,
    last_point: Point<f32>,
    aspect_ratio: f32,
    was_flipped: Option<bool>,
    keep_upright_margin: f32,
) -> bool {
    let Some(was_flipped) = was_flipped.filter(|_| keep_upright_margin > 0.0) else {
        // No margin, or drawn for the first time: flipped when it runs right to left.
        return first_point.x > last_point.x;
    };
    // Gl coordinates: y points up; x scaled by the aspect ratio so that angles are the screen's.
    let dx = (last_point.x - first_point.x) * aspect_ratio;
    let dy = last_point.y - first_point.y;
    let margin = (dx * dx + dy * dy).sqrt() * mln_math::deg2radf(keep_upright_margin).sin();
    if was_flipped { dx <= margin } else { dx < -margin }
}

/// `requiresOrientationChange(writingModes, firstPoint, lastPoint, aspectRatio,
/// wasFlipped, keepUprightMargin)`.
fn requires_orientation_change(
    writing_modes: WritingModeType,
    first_point: Point<f32>,
    last_point: Point<f32>,
    aspect_ratio: f32,
    was_flipped: Option<bool>,
    keep_upright_margin: f32,
) -> Option<PlacementResult> {
    if writing_modes == (WritingModeType::Horizontal | WritingModeType::Vertical) {
        // On top of choosing whether to flip, choose whether to render this
        // version of the glyphs or the alternate vertical glyphs. We can't just
        // filter out vertical glyphs in the horizontal range because the
        // horizontal and vertical versions can have slightly different
        // projections which could lead to angles where both or neither showed.
        let rise = (last_point.y - first_point.y).abs();
        let run = (last_point.x - first_point.x).abs() * aspect_ratio;
        if rise > run {
            return Some(PlacementResult::UseVertical);
        }
    }

    if if writing_modes == WritingModeType::Vertical {
        first_point.y < last_point.y
    } else {
        line_label_needs_flip(first_point, last_point, aspect_ratio, was_flipped, keep_upright_margin)
    } {
        // Includes "horizontalOnly" case for labels without vertical glyphs
        return Some(PlacementResult::NeedsFlipping);
    }
    None
}

/// `placeGlyphsAlongLine(...)`.
#[allow(clippy::too_many_arguments)]
fn place_glyphs_along_line(
    symbol: &PlacedSymbol,
    font_size: f32,
    flip: bool,
    keep_upright: bool,
    keep_upright_margin: f32,
    pos_matrix: &Mat4,
    label_plane_matrix: &Mat4,
    gl_coord_matrix: &Mat4,
    dynamic_vertex_array: &mut Vec<SymbolDynamicLayoutVertex>,
    projected_anchor_point: Point<f32>,
    aspect_ratio: f32,
) -> PlacementResult {
    let font_scale = font_size / ONE_EM;
    let line_offset_x = symbol.line_offset[0] * font_scale;
    let line_offset_y = symbol.line_offset[1] * font_scale;

    let mut placed_glyphs: Vec<PlacedGlyph> = Vec::new();
    if symbol.glyph_offsets.len() > 1 {
        let Some(first_and_last_glyph) = place_first_and_last_glyph(
            font_scale,
            line_offset_x,
            line_offset_y,
            flip,
            projected_anchor_point,
            symbol.anchor_point,
            symbol,
            label_plane_matrix,
            false,
        ) else {
            return PlacementResult::NotEnoughRoom;
        };

        let first_point = project(first_and_last_glyph.0.point, gl_coord_matrix).0;
        let last_point = project(first_and_last_glyph.1.point, gl_coord_matrix).0;

        if keep_upright && !flip {
            if let Some(orientation_change) = requires_orientation_change(
                symbol.writing_modes,
                first_point,
                last_point,
                aspect_ratio,
                symbol.keep_upright_flipped,
                keep_upright_margin,
            ) {
                return orientation_change;
            }
        }

        placed_glyphs.reserve(symbol.glyph_offsets.len());
        placed_glyphs.push(first_and_last_glyph.0);
        for glyph_index in 1..symbol.glyph_offsets.len() - 1 {
            let glyph_offset_x = symbol.glyph_offsets[glyph_index];
            // Since first and last glyph fit on the line, we're sure that the
            // rest of the glyphs can be placed
            let placed_glyph = place_glyph_along_line(
                glyph_offset_x * font_scale,
                line_offset_x,
                line_offset_y,
                flip,
                projected_anchor_point,
                symbol.anchor_point,
                symbol.segment as u16,
                &symbol.line,
                &symbol.tile_distances,
                label_plane_matrix,
                false,
            );
            match placed_glyph {
                Some(placed_glyph) => placed_glyphs.push(placed_glyph),
                None => {
                    placed_glyphs.push(PlacedGlyph::new(Point::new(f32::NEG_INFINITY, f32::NEG_INFINITY), 0.0, None))
                }
            }
        }
        placed_glyphs.push(first_and_last_glyph.1);
    } else if symbol.glyph_offsets.len() == 1 {
        // Only a single glyph to place
        // So, determine whether to flip based on projected angle of the line segment it's on
        if keep_upright && !flip {
            let a = project(symbol.anchor_point, pos_matrix).0;
            let tile_segment_end = convert_point(&symbol.line[symbol.segment + 1]);
            let projected_vertex = project(tile_segment_end, pos_matrix);
            // We know the anchor will be in the viewport, but the end of the
            // line segment may be behind the plane of the camera, in which case
            // we can use a point at any arbitrary (closer) point on the
            // segment.
            let b = if projected_vertex.1 > 0.0 {
                projected_vertex.0
            } else {
                project_truncated_line_segment(symbol.anchor_point, tile_segment_end, a, 1.0, pos_matrix)
            };

            if let Some(orientation_change) = requires_orientation_change(
                symbol.writing_modes,
                a,
                b,
                aspect_ratio,
                symbol.keep_upright_flipped,
                keep_upright_margin,
            ) {
                return orientation_change;
            }
        }
        let glyph_offset_x = symbol.glyph_offsets[0];
        let Some(single_glyph) = place_glyph_along_line(
            font_scale * glyph_offset_x,
            line_offset_x,
            line_offset_y,
            flip,
            projected_anchor_point,
            symbol.anchor_point,
            symbol.segment as u16,
            &symbol.line,
            &symbol.tile_distances,
            label_plane_matrix,
            false,
        ) else {
            return PlacementResult::NotEnoughRoom;
        };

        placed_glyphs.push(single_glyph);
    }

    // The number of placedGlyphs must equal the number of glyphOffsets, which
    // must correspond to the number of glyph vertices There may be 0 glyphs
    // here, if a label consists entirely of glyphs that have 0x0 dimensions
    if dynamic_vertex_array.is_empty() {
        dynamic_vertex_array.reserve(4 * placed_glyphs.len());
    }
    for placed_glyph in &placed_glyphs {
        add_dynamic_attributes(placed_glyph.point, placed_glyph.angle, dynamic_vertex_array);
    }

    PlacementResult::Ok
}

/// `reprojectLineLabels(dynamicVertexArray, placedSymbols, posMatrix,
/// pitchWithMap, rotateWithMap, keepUpright, keepUprightMargin, tile,
/// sizeBinder, state)`.
///
/// With a `keep_upright_margin` above 0 (degrees, text only), each drawn
/// symbol remembers the direction it was drawn with in
/// `PlacedSymbol::keep_upright_flipped` and keeps it, see
/// [`line_label_needs_flip`]. With 0, nothing is remembered or read.
#[allow(clippy::too_many_arguments)]
pub fn reproject_line_labels(
    dynamic_vertex_array: &mut Vec<SymbolDynamicLayoutVertex>,
    placed_symbols: &mut [PlacedSymbol],
    pos_matrix: &Mat4,
    pitch_with_map: bool,
    rotate_with_map: bool,
    keep_upright: bool,
    keep_upright_margin: f32,
    tile: &RenderTile,
    size_binder: &SymbolSizeBinder,
    state: &TransformState,
) {
    let partially_evaluated_size = size_binder.evaluate_for_zoom(state.get_zoom() as f32);

    let size = state.get_size();
    let clipping_buffer: [f64; 2] = [256.0 / size.width as f64 * 2.0 + 1.0, 256.0 / size.height as f64 * 2.0 + 1.0];

    let pixels_to_tile_units = tile.id.pixels_to_tile_units(1.0, state.get_zoom() as f32);

    let label_plane_matrix =
        get_label_plane_matrix(pos_matrix, pitch_with_map, rotate_with_map, state, pixels_to_tile_units);

    let gl_coord_matrix = get_gl_coord_matrix(pos_matrix, pitch_with_map, rotate_with_map, state, pixels_to_tile_units);

    dynamic_vertex_array.clear();

    let mut use_vertical = false;

    for placed_symbol in placed_symbols.iter_mut() {
        // Don't do calculations for vertical glyphs unless the previous symbol
        // was horizontal and we determined that vertical glyphs were necessary.
        // Also don't do calculations for symbols that are collided and fully
        // faded out
        if placed_symbol.hidden || (placed_symbol.writing_modes == WritingModeType::Vertical && !use_vertical) {
            hide_glyphs(placed_symbol.glyph_offsets.len(), dynamic_vertex_array);
            continue;
        }
        // Awkward... but we're counting on the paired "vertical" symbol coming
        // immediately after its horizontal counterpart
        use_vertical = false;

        let mut anchor_pos: Vec4 = [placed_symbol.anchor_point.x as f64, placed_symbol.anchor_point.y as f64, 0.0, 1.0];
        let a = anchor_pos;
        matrix::transform_mat4(&mut anchor_pos, &a, pos_matrix);

        // Don't bother calculating the correct point for invisible labels.
        if !is_visible(&anchor_pos, &clipping_buffer) {
            hide_glyphs(placed_symbol.glyph_offsets.len(), dynamic_vertex_array);
            continue;
        }

        let camera_to_anchor_distance = anchor_pos[3] as f32;
        let perspective_ratio = 0.5 + 0.5 * (camera_to_anchor_distance / state.get_camera_to_center_distance());

        let font_size = evaluate_size_for_feature(&partially_evaluated_size, placed_symbol);
        let pitch_scaled_font_size =
            if pitch_with_map { font_size * perspective_ratio } else { font_size / perspective_ratio };

        let anchor_point = project(placed_symbol.anchor_point, &label_plane_matrix).0;

        let place_unflipped = place_glyphs_along_line(
            placed_symbol,
            pitch_scaled_font_size,
            false, /*unflipped*/
            keep_upright,
            keep_upright_margin,
            pos_matrix,
            &label_plane_matrix,
            &gl_coord_matrix,
            dynamic_vertex_array,
            anchor_point,
            size.aspect_ratio(),
        );

        use_vertical = place_unflipped == PlacementResult::UseVertical;

        if place_unflipped == PlacementResult::NotEnoughRoom
            || use_vertical
            || (place_unflipped == PlacementResult::NeedsFlipping
                && place_glyphs_along_line(
                    placed_symbol,
                    pitch_scaled_font_size,
                    true, /*flipped*/
                    keep_upright,
                    keep_upright_margin,
                    pos_matrix,
                    &label_plane_matrix,
                    &gl_coord_matrix,
                    dynamic_vertex_array,
                    anchor_point,
                    size.aspect_ratio(),
                ) == PlacementResult::NotEnoughRoom)
        {
            hide_glyphs(placed_symbol.glyph_offsets.len(), dynamic_vertex_array);
        } else if keep_upright && keep_upright_margin > 0.0 {
            // Remember the direction it was drawn with (see line_label_needs_flip).
            placed_symbol.keep_upright_flipped = Some(place_unflipped == PlacementResult::NeedsFlipping);
        }
    }
}
