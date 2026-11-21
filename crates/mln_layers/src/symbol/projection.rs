// The label-plane matrices, and the CPU projection that lays line labels'
// glyphs along their lines as the camera sees them. Symbols are not
// instanced: each glyph has four dynamic vertices.

use std::f32::consts::PI;
use std::iter;

use geometry::Point;
use glam::{DMat4, DVec2, DVec3, DVec4, Vec2};
use mln_map_types::{ProjectedTilePoint, TileProjector, View};
use mln_tile_id::geometry_tile_data::GeometryCoordinate;
use mln_util::constants::ONE_EM;

use crate::symbol::bucket::{
    PlacedSymbol, PlacedSymbolPools, SymbolBucket, SymbolDynamicLayoutVertex, SymbolSizeBinder, ZoomEvaluatedSize,
};
use crate::symbol::instance::WritingModes;

/// The directions on screen that a pitched, unrotated label's east and south
/// axes take, as unit vectors, or zero where an axis is edge-on.
pub(crate) fn tile_skew_vectors(state: &View) -> (DVec2, DVec2) {
    let cos_roll = state.roll().cos();
    let sin_roll = state.roll().sin();
    let cos_pitch = state.pitch().cos();
    let cos_bearing = (-state.angle()).cos();
    let sin_bearing = (-state.angle()).sin();
    let unit = |v: DVec2| if v.length() < 1.0e-9 { DVec2::ZERO } else { v / v.length() };
    let vec_south = unit(DVec2::new(
        -cos_bearing * cos_pitch * sin_roll - sin_bearing * cos_roll,
        -sin_bearing * cos_pitch * sin_roll + cos_bearing * cos_roll,
    ));
    let vec_east = unit(DVec2::new(
        cos_bearing * cos_pitch * cos_roll - sin_bearing * sin_roll,
        sin_bearing * cos_pitch * cos_roll + cos_bearing * sin_roll,
    ));
    (vec_east, vec_south)
}

pub(crate) fn tile_skew_matrix(state: &View) -> DMat4 {
    let (vec_east, vec_south) = tile_skew_vectors(state);
    DMat4::from_cols(vec_east.extend(0.0).extend(0.0), vec_south.extend(0.0).extend(0.0), DVec4::Z, DVec4::W)
}

/// A matrix for converting to the label coordinate space, from tile units
/// when pitched with the map and from clip space otherwise; the shaders apply
/// the tile's projection themselves.
pub(crate) fn label_plane_matrix(
    pitch_with_map: bool,
    rotate_with_map: bool,
    state: &View,
    pixels_to_tile_units: f32,
) -> DMat4 {
    if pitch_with_map {
        let s = f64::from(1.0 / pixels_to_tile_units);
        let scale = DMat4::from_scale(DVec3::new(s, s, 1.0));
        if rotate_with_map {
            scale
        } else {
            let skew = tile_skew_matrix(state);
            scale * skew.try_inverse().unwrap_or(skew)
        }
    } else {
        let size = state.size();
        DMat4::from_scale(DVec3::new(f64::from(size.width) / 2.0, -(f64::from(size.height) / 2.0), 1.0))
            * DMat4::from_translation(DVec3::new(1.0, -1.0, 0.0))
    }
}

/// A matrix for converting from the label coordinate space back to tile
/// units when pitched with the map, and to GL coordinates otherwise.
pub(crate) fn gl_coord_matrix(
    pitch_with_map: bool,
    rotate_with_map: bool,
    state: &View,
    pixels_to_tile_units: f32,
) -> DMat4 {
    if pitch_with_map {
        let s = f64::from(pixels_to_tile_units);
        let scale = DMat4::from_scale(DVec3::new(s, s, 1.0));
        if rotate_with_map { scale } else { scale * tile_skew_matrix(state) }
    } else {
        let size = state.size();
        DMat4::from_scale(DVec3::new(1.0, -1.0, 1.0))
            * DMat4::from_translation(DVec3::new(-1.0, -1.0, 0.0))
            * DMat4::from_scale(DVec3::new(2.0 / f64::from(size.width), 2.0 / f64::from(size.height), 1.0))
    }
}

/// `symbol`'s `text-size` or `icon-size` at the zoom `size` was evaluated
/// for.
#[must_use]
pub(crate) fn evaluate_size_for_feature(size: &ZoomEvaluatedSize, symbol: &PlacedSymbol) -> f32 {
    if size.is_feature_constant {
        size.size
    } else if size.is_zoom_constant {
        symbol.lower_size
    } else {
        symbol.lower_size + size.size_t * (symbol.upper_size - symbol.lower_size)
    }
}

/// Appends the four vertices of a glyph at `anchor`, turned by `angle`.
pub(crate) fn add_dynamic_attributes(anchor: Vec2, angle: f32, vertices: &mut Vec<SymbolDynamicLayoutVertex>) {
    vertices.extend([SymbolBucket::dynamic_layout_attributes(anchor, angle); 4]);
}

/// Appends the vertices of `glyph_count` glyphs placed where nothing draws.
pub(crate) fn hide_glyphs(glyph_count: usize, vertices: &mut Vec<SymbolDynamicLayoutVertex>) {
    let hidden = SymbolBucket::dynamic_layout_attributes(Vec2::NEG_INFINITY, 0.0);
    vertices.resize(vertices.len() + 4 * glyph_count, hidden);
}

/// A point projected for label placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProjectedPoint {
    pub(crate) point: Vec2,
    /// The projection's w: positive in front of the camera.
    pub(crate) camera_distance: f32,
    /// Behind the globe's horizon; never on Mercator.
    pub(crate) occluded: bool,
}

impl From<ProjectedTilePoint> for ProjectedPoint {
    fn from(projected: ProjectedTilePoint) -> Self {
        ProjectedPoint {
            point: DVec2::new(projected.point.x, projected.point.y).as_vec2(),
            camera_distance: projected.signed_distance_from_camera as f32,
            occluded: projected.occluded,
        }
    }
}

/// Projects tile points to the label plane a symbol is laid out in, through
/// the tile's projection.
#[derive(Clone)]
pub(crate) struct LabelPlaneProjector<'a> {
    tile: TileProjector<'a>,
    pitch_with_map: bool,
    translation: Vec2,
    /// Tile units to the pitched label plane, and back.
    pitched: DMat4,
    pitched_inverse: DMat4,
    viewport: Vec2,
    /// The symbol's `symbol-height-offset`, in meters.
    elevation: f64,
}

impl<'a> LabelPlaneProjector<'a> {
    /// A projector of `tile`'s points, moved by `translation` in tile
    /// units, into the label plane.
    #[must_use]
    pub(crate) fn new(
        tile: &TileProjector<'a>,
        pitch_with_map: bool,
        rotate_with_map: bool,
        pixels_to_tile_units: f32,
        translation: Vec2,
    ) -> Self {
        let size = tile.view().size();
        let (pitched, pitched_inverse) = if pitch_with_map {
            let matrix = label_plane_matrix(true, rotate_with_map, tile.view(), pixels_to_tile_units);
            (matrix, matrix.try_inverse().unwrap_or(DMat4::ZERO))
        } else {
            (DMat4::IDENTITY, DMat4::IDENTITY)
        };
        LabelPlaneProjector {
            tile: *tile,
            pitch_with_map,
            translation,
            pitched,
            pitched_inverse,
            viewport: Vec2::new(size.width as f32, size.height as f32),
            elevation: 0.0,
        }
    }

    /// This projector for a symbol `height_offset` meters above the ground.
    /// The pitched label plane is the same at any height; the shaders raise
    /// it.
    #[must_use]
    pub(crate) fn raised(&self, height_offset: f32) -> Self {
        LabelPlaneProjector { elevation: f64::from(height_offset), ..self.clone() }
    }

    /// Tile units to the label plane; distance and occlusion come from the
    /// projection.
    #[must_use]
    pub(crate) fn project(&self, tile_point: Vec2) -> ProjectedPoint {
        let translated = tile_point + self.translation;
        if self.pitch_with_map {
            let pos = self.pitched * translated.as_dvec2().extend(0.0).extend(1.0);
            return ProjectedPoint {
                point: (pos.truncate().truncate() / pos.w).as_vec2(),
                camera_distance: pos.w as f32,
                occluded: false,
            };
        }
        let projected = self.tile.project(&tile_point_f64(translated), self.elevation);
        let clip = DVec2::new(projected.point.x, projected.point.y);
        ProjectedPoint {
            point: ((clip * DVec2::new(0.5, -0.5) + 0.5) * self.viewport.as_dvec2()).as_vec2(),
            ..projected.into()
        }
    }

    /// Label plane to clip space, for orientation checks.
    #[must_use]
    pub(crate) fn to_clip_space(&self, label_plane_point: Vec2) -> Vec2 {
        if self.pitch_with_map {
            let pos = self.pitched_inverse * label_plane_point.as_dvec2().extend(0.0).extend(1.0);
            let tile_point = (pos.truncate().truncate() / pos.w).as_vec2();
            return self.to_clip_space_from_tile(tile_point).point;
        }
        Vec2::new(1.0, -1.0) * (label_plane_point / self.viewport * 2.0 - 1.0)
    }

    /// Tile units straight to clip space.
    #[must_use]
    pub(crate) fn to_clip_space_from_tile(&self, tile_point: Vec2) -> ProjectedPoint {
        self.tile.project(&tile_point_f64(tile_point), self.elevation).into()
    }
}

/// How far along its line, from the anchor, a glyph is placed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TileDistance {
    /// Along the line in tile units, to the start of the glyph's segment: 0
    /// on the anchor's segment.
    pub(crate) prev_tile_distance: f32,
    /// Along the glyph's segment in the label plane, from its start.
    pub(crate) last_segment_viewport_distance: f32,
}

/// A glyph placed on its line in the label plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlacedGlyph {
    pub(crate) point: Vec2,
    /// The angle of the segment the glyph is on, turned half around for a
    /// glyph read against the line's direction.
    pub(crate) angle: f32,
    pub(crate) tile_distance: TileDistance,
}

/// A label along a line: a placed symbol's geometry, borrowed from its
/// buffer's pools.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LineLabel<'a> {
    /// In tile units, on the segment from `line[segment]`.
    anchor: Vec2,
    segment: usize,
    line: &'a [GeometryCoordinate],
    /// Along the line from the anchor to each vertex, in tile units; empty
    /// for icons.
    tile_distances: &'a [f32],
    /// Each glyph's offset along the line from the anchor, in units of a
    /// `ONE_EM` font.
    glyph_offsets: &'a [f32],
    /// `text-offset` or `icon-offset`, in units of a `ONE_EM` font.
    line_offset: Vec2,
    writing_modes: WritingModes,
}

impl<'a> LineLabel<'a> {
    #[must_use]
    pub(crate) fn new(symbol: &PlacedSymbol, pools: &'a PlacedSymbolPools) -> Self {
        LineLabel {
            anchor: symbol.anchor_point,
            segment: symbol.segment,
            line: pools.line(symbol),
            tile_distances: pools.tile_distances(symbol),
            glyph_offsets: pools.glyph_offsets(symbol),
            line_offset: Vec2::from(symbol.line_offset),
            writing_modes: symbol.writing_modes,
        }
    }
}

/// Why a label's glyphs weren't placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unplaced {
    /// The line ends before a glyph.
    NotEnoughRoom,
    /// The label reads upside down; flipped, it reads upright.
    NeedsFlipping,
    /// The line is steep enough on screen for the label's vertical glyphs
    /// instead.
    UseVertical,
}

/// Places line labels' glyphs in one label plane, keeping a buffer for the
/// projections of each label's line.
pub(crate) struct LinePlacer<'a> {
    label_plane: &'a LabelPlaneProjector<'a>,
    projections: Vec<Option<ProjectedPoint>>,
}

impl<'a> LinePlacer<'a> {
    #[must_use]
    pub(crate) fn new(label_plane: &'a LabelPlaneProjector<'a>) -> Self {
        LinePlacer { label_plane, projections: Vec::new() }
    }

    /// Starts placing `label`, its glyphs `font_size` pixels in the label
    /// plane.
    pub(crate) fn begin(&mut self, label: LineLabel<'a>, font_size: f32) -> LabelPlacer<'_, 'a> {
        self.projections.clear();
        self.projections.resize(label.line.len(), None);
        let font_scale = font_size / ONE_EM;
        LabelPlacer {
            label_plane: self.label_plane,
            projections: &mut self.projections,
            occluded: false,
            anchor: self.label_plane.project(label.anchor).point,
            font_scale,
            line_offset: label.line_offset * font_scale,
            label,
        }
    }
}

/// Places one label's glyphs, projecting each vertex of its line once
/// however many glyphs walk past it.
pub(crate) struct LabelPlacer<'s, 'a> {
    label_plane: &'a LabelPlaneProjector<'a>,
    projections: &'s mut [Option<ProjectedPoint>],
    /// Whether any vertex projected so far is behind the globe's horizon.
    occluded: bool,
    label: LineLabel<'a>,
    /// The label's anchor in the label plane.
    anchor: Vec2,
    font_scale: f32,
    /// The label's offset from its line, in the label plane.
    line_offset: Vec2,
}

impl LabelPlacer<'_, '_> {
    fn project_vertex(&mut self, index: usize) -> ProjectedPoint {
        let (label_plane, line, occluded) = (self.label_plane, self.label.line, &mut self.occluded);
        *self.projections[index].get_or_insert_with(|| {
            let projected = label_plane.project(tile_point(line[index]));
            *occluded |= projected.occluded;
            projected
        })
    }

    /// The glyph `glyph_offset` (in units of a `ONE_EM` font) along the
    /// line from the anchor, or `None` where the line ends first. A flipped
    /// label reads against the line's direction.
    pub(crate) fn glyph_at(&mut self, glyph_offset: f32, flip: bool) -> Option<PlacedGlyph> {
        let offset = self.font_scale * glyph_offset;
        let offset = if flip { offset - self.line_offset.x } else { offset + self.line_offset.x };
        let forward = (offset > 0.0) != flip;
        let angle_offset = if flip { PI } else { 0.0 } + if forward { 0.0 } else { PI };
        let distance = offset.abs();

        let segment = self.label.segment;
        let line_len = self.label.line.len();
        let first = if forward { segment + 1 } else { segment };
        let mut vertices = iter::successors(Some(first), |&i| if forward { Some(i + 1) } else { i.checked_sub(1) })
            .take_while(|&i| i < line_len);

        // The walk goes along the line in the label plane from `prev` to
        // `current`, until the segment between them reaches the glyph. A
        // vertex is `None` when it's the anchor.
        let (mut prev, mut current) = (self.anchor, self.anchor);
        let mut current_tile = self.label.anchor;
        let (mut prev_vertex, mut current_vertex) = (None, None);
        let mut distance_to_prev = 0.0;
        let mut segment_length = 0.0;
        while distance_to_prev + segment_length <= distance {
            let index = vertices.next()?;
            let prev_tile = current_tile;
            (prev, prev_vertex) = (current, current_vertex);
            (current_tile, current_vertex) = (tile_point(self.label.line[index]), Some(index));
            distance_to_prev += segment_length;

            let projected = self.project_vertex(index);
            current = if projected.camera_distance > 0.0 {
                projected.point
            } else {
                // Behind the camera, the vertex has no projection. A point
                // far enough along the segment for the glyph stands in.
                project_truncated_segment(prev_tile, current_tile, prev, distance - distance_to_prev + 1.0, |p| {
                    self.label_plane.project(p).point
                })
            };
            segment_length = prev.distance(current);
        }

        let along_segment = distance - distance_to_prev;
        let direction = current - prev;
        let side = if forward { self.line_offset.y } else { -self.line_offset.y };
        let point = prev + direction * (along_segment / segment_length) + direction.perp() * (side / segment_length);
        let prev_tile_distance =
            prev_vertex.map_or(0.0, |i| self.label.tile_distances.get(i).copied().unwrap_or_default());
        Some(PlacedGlyph {
            point,
            angle: angle_offset + direction.y.atan2(direction.x),
            tile_distance: TileDistance { prev_tile_distance, last_segment_viewport_distance: along_segment },
        })
    }

    /// The label's first and last glyphs, or `None` where either doesn't
    /// fit, or the line is behind the globe's horizon.
    pub(crate) fn first_and_last_glyph(&mut self, flip: bool) -> Option<(PlacedGlyph, PlacedGlyph)> {
        let offsets = self.label.glyph_offsets;
        let first = self.glyph_at(*offsets.first()?, flip)?;
        let last = self.glyph_at(*offsets.last()?, flip)?;
        (!self.occluded).then_some((first, last))
    }

    /// Appends the vertices of the label's glyphs, turned to follow the line
    /// where `rotate_to_line`. With `keep_upright`, an unflipped label that
    /// would read upside down, or that its vertical glyphs should replace, is
    /// left unplaced.
    fn place_glyphs(
        &mut self,
        flip: bool,
        keep_upright: bool,
        rotate_to_line: bool,
        vertices: &mut Vec<SymbolDynamicLayoutVertex>,
    ) -> Result<(), Unplaced> {
        let check_upright = keep_upright && !flip;
        let mut push = |glyph: PlacedGlyph| {
            add_dynamic_attributes(glyph.point, if rotate_to_line { glyph.angle } else { 0.0 }, vertices);
        };
        match self.label.glyph_offsets {
            // A label of glyphs that are all 0x0 has no vertices.
            [] => {}
            &[offset] => {
                if check_upright {
                    self.check_segment_upright()?;
                }
                let glyph = self.glyph_at(offset, flip).filter(|_| !self.occluded).ok_or(Unplaced::NotEnoughRoom)?;
                push(glyph);
            }
            [_, middle @ .., _] => {
                let (first, last) = self.first_and_last_glyph(flip).ok_or(Unplaced::NotEnoughRoom)?;
                if check_upright {
                    let label_plane = self.label_plane;
                    let (first_clip, last_clip) =
                        (label_plane.to_clip_space(first.point), label_plane.to_clip_space(last.point));
                    self.check_upright(first_clip, last_clip)?;
                }
                push(first);
                for &offset in middle {
                    // Between two glyphs that fit, every glyph fits; one that
                    // rounding puts off the line is hidden.
                    let glyph = self.glyph_at(offset, flip).unwrap_or(PlacedGlyph {
                        point: Vec2::NEG_INFINITY,
                        angle: 0.0,
                        tile_distance: TileDistance::default(),
                    });
                    push(glyph);
                }
                push(last);
            }
        }
        Ok(())
    }

    /// Checks a one-glyph label's orientation against its segment's
    /// direction on screen.
    fn check_segment_upright(&self) -> Result<(), Unplaced> {
        let label_plane = self.label_plane;
        let to_clip = |p: Vec2| label_plane.to_clip_space_from_tile(p).point;
        let anchor = to_clip(self.label.anchor);
        let Some(&end) = self.label.line.get(self.label.segment + 1) else {
            return Err(Unplaced::NotEnoughRoom);
        };
        let end_tile = tile_point(end);
        let end = label_plane.to_clip_space_from_tile(end_tile);
        // The anchor is in view, but the segment's end may be behind the
        // camera, where any point closer along the segment gives its
        // direction.
        let end = if end.camera_distance > 0.0 {
            end.point
        } else {
            project_truncated_segment(self.label.anchor, end_tile, anchor, 1.0, to_clip)
        };
        self.check_upright(anchor, end)
    }

    /// Checks the orientation of the label running from `first` to `last`
    /// in clip space. A label that has vertical glyphs too uses them where
    /// the line is steeper on screen than level. Only the horizontal version
    /// decides: the two project slightly differently, so deciding each for
    /// itself could show both or neither.
    fn check_upright(&self, first: Vec2, last: Vec2) -> Result<(), Unplaced> {
        let writing_modes = self.label.writing_modes;
        if writing_modes == WritingModes::BOTH {
            let aspect_ratio = self.label_plane.viewport.x / self.label_plane.viewport.y;
            let rise = (last.y - first.y).abs();
            let run = (last.x - first.x).abs() * aspect_ratio;
            if rise > run {
                return Err(Unplaced::UseVertical);
            }
        }
        let upside_down = if writing_modes == WritingModes::VERTICAL { first.y < last.y } else { first.x > last.x };
        if upside_down { Err(Unplaced::NeedsFlipping) } else { Ok(()) }
    }
}

/// The point `min_length` along the segment from `prev_tile` toward
/// `current_tile`, projected by `project`, when `current_tile` has no
/// projection: the direction comes from a point one tile unit back from
/// `prev_tile`, which projects as `prev`.
fn project_truncated_segment(
    prev_tile: Vec2,
    current_tile: Vec2,
    prev: Vec2,
    min_length: f32,
    project: impl Fn(Vec2) -> Vec2,
) -> Vec2 {
    let unit_back = project(prev_tile + (prev_tile - current_tile).normalize_or_zero());
    let unit_segment = prev - unit_back;
    prev + unit_segment * (min_length / unit_segment.length())
}

fn tile_point(p: GeometryCoordinate) -> Vec2 {
    Vec2::new(f32::from(p.x), f32::from(p.y))
}

fn tile_point_f64(p: Vec2) -> Point<f64> {
    Point::new(f64::from(p.x), f64::from(p.y))
}

/// How line labels align with the map.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LineLabelAlignment {
    pub(crate) pitch_with_map: bool,
    /// Whether glyphs turn to follow the line; otherwise
    /// (`text-rotation-alignment: viewport-glyph`) they stay level with the
    /// viewport.
    pub(crate) rotate_with_map: bool,
    pub(crate) keep_upright: bool,
}

/// Replaces `vertices` with those of each glyph of the line labels
/// `symbols`, placed along their lines for `tile_projector`'s view.
pub(crate) fn reproject_line_labels(
    vertices: &mut Vec<SymbolDynamicLayoutVertex>,
    symbols: &[PlacedSymbol],
    pools: &PlacedSymbolPools,
    tile_projector: &TileProjector<'_>,
    size_binder: &SymbolSizeBinder,
    translation: Vec2,
    alignment: LineLabelAlignment,
) {
    let view = tile_projector.view();
    let zoom = view.zoom() as f32;
    let sizes = size_binder.evaluate_for_zoom(zoom);
    let viewport = view.size();
    let camera_to_center = view.camera_to_center_distance() as f32;
    // Labels whose anchors are more than 256 pixels off screen don't show.
    let clip_extent = 256.0 / DVec2::new(f64::from(viewport.width), f64::from(viewport.height)) * 2.0 + 1.0;

    let pixels_to_tile_units = tile_projector.tile_id().pixels_to_tile_units(1.0, zoom);
    let label_plane = LabelPlaneProjector::new(
        tile_projector,
        alignment.pitch_with_map,
        alignment.rotate_with_map,
        pixels_to_tile_units,
        translation,
    );
    let mut placer = LinePlacer::new(&label_plane);

    vertices.clear();
    // A label's vertical glyphs come right after its horizontal ones, and
    // show only when those find the line steep.
    let mut show_vertical = false;
    for symbol in symbols {
        let glyph_count = symbol.glyph_offsets.len();
        let vertical_shows = std::mem::take(&mut show_vertical);
        if symbol.hidden || (symbol.writing_modes == WritingModes::VERTICAL && !vertical_shows) {
            hide_glyphs(glyph_count, vertices);
            continue;
        }

        let tile_anchor = tile_point_f64(symbol.anchor_point);
        let anchor = tile_projector.project(&tile_anchor, 0.0);
        if anchor.occluded || !DVec2::new(anchor.point.x, anchor.point.y).abs().cmple(clip_extent).all() {
            hide_glyphs(glyph_count, vertices);
            continue;
        }

        let perspective_ratio = 0.5 + 0.5 * (anchor.signed_distance_from_camera as f32 / camera_to_center);
        let font_size = evaluate_size_for_feature(&sizes, symbol);
        let font_size = if alignment.pitch_with_map {
            font_size * perspective_ratio * tile_projector.pitched_text_correction(&tile_anchor) as f32
        } else {
            font_size / perspective_ratio
        };

        let mut label = placer.begin(LineLabel::new(symbol, pools), font_size);
        let mut place = |flip| label.place_glyphs(flip, alignment.keep_upright, alignment.rotate_with_map, vertices);
        let placed = match place(false) {
            Err(Unplaced::NeedsFlipping) => place(true),
            placed => placed,
        };
        if let Err(unplaced) = placed {
            show_vertical = unplaced == Unplaced::UseVertical;
            hide_glyphs(glyph_count, vertices);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, TAU};

    use geometry::Point;
    use glam::Vec2;
    use mln_map_types::{ConstrainMode, TileProjector, ViewportMode};
    use mln_runtime::map::transform::Transform;
    use mln_tile_id::UnwrappedTileID;
    use mln_util::size::Size;

    use super::{LabelPlaneProjector, LineLabel, LinePlacer, PlacedGlyph, TileDistance, Unplaced, WritingModes};

    /// A 512 px square map at zoom 0, where tile 0/0/0 fills the viewport
    /// and a pixel is 16 tile units.
    fn transform() -> Transform {
        let mut transform = Transform::new(ConstrainMode::HeightOnly, ViewportMode::Default);
        transform.resize(Size::new(512, 512)).unwrap();
        transform
    }

    fn label<'a>(
        anchor: Vec2,
        line: &'a [Point<i16>],
        tile_distances: &'a [f32],
        glyph_offsets: &'a [f32],
    ) -> LineLabel<'a> {
        LineLabel {
            anchor,
            segment: 0,
            line,
            tile_distances,
            glyph_offsets,
            line_offset: Vec2::ZERO,
            writing_modes: WritingModes { horizontal: true, vertical: false },
        }
    }

    fn assert_glyph(glyph: Option<PlacedGlyph>, point: Vec2, angle: f32, tile_distance: TileDistance) {
        let glyph = glyph.expect("the glyph fits");
        assert!(glyph.point.abs_diff_eq(point, 1e-3), "{} != {point}", glyph.point);
        let turn = (glyph.angle - angle).rem_euclid(TAU);
        assert!(turn.min(TAU - turn) < 1e-5, "{} != {angle}", glyph.angle);
        assert!((glyph.tile_distance.prev_tile_distance - tile_distance.prev_tile_distance).abs() < 1e-3);
        assert!(
            (glyph.tile_distance.last_segment_viewport_distance - tile_distance.last_segment_viewport_distance).abs()
                < 1e-3
        );
    }

    #[test]
    fn glyphs_follow_the_line_around_corners_and_stop_at_its_ends() {
        let transform = transform();
        let tile = TileProjector::new(transform.view(), &UnwrappedTileID::new(0, 0, 0));
        let label_plane = LabelPlaneProjector::new(&tile, false, true, 16.0, Vec2::ZERO);
        let mut placer = LinePlacer::new(&label_plane);
        // East from (128, 256) px to (256, 256), then south to (256, 384).
        let line = [Point::new(2048, 4096), Point::new(4096, 4096), Point::new(4096, 6144)];
        let tile_distances = [1024.0, 1024.0, 3072.0];
        let mut label = placer.begin(label(Vec2::new(3072.0, 4096.0), &line, &tile_distances, &[]), 24.0);

        let on_anchor_segment = |last| TileDistance { prev_tile_distance: 0.0, last_segment_viewport_distance: last };
        assert_glyph(label.glyph_at(32.0, false), Vec2::new(224.0, 256.0), 0.0, on_anchor_segment(32.0));
        assert_glyph(label.glyph_at(-32.0, false), Vec2::new(160.0, 256.0), 0.0, on_anchor_segment(32.0));
        assert_glyph(
            label.glyph_at(96.0, false),
            Vec2::new(256.0, 288.0),
            FRAC_PI_2,
            TileDistance { prev_tile_distance: 1024.0, last_segment_viewport_distance: 32.0 },
        );
        assert_eq!(label.glyph_at(193.0, false), None);
        assert_eq!(label.glyph_at(-65.0, false), None);
    }

    #[test]
    fn an_upright_label_on_a_line_drawn_westward_flips() {
        let transform = transform();
        let tile = TileProjector::new(transform.view(), &UnwrappedTileID::new(0, 0, 0));
        let label_plane = LabelPlaneProjector::new(&tile, false, true, 16.0, Vec2::ZERO);
        let mut placer = LinePlacer::new(&label_plane);
        // West from (384, 256) px to (128, 256).
        let line = [Point::new(6144, 4096), Point::new(2048, 4096)];
        let mut label =
            placer.begin(label(Vec2::new(4096.0, 4096.0), &line, &[0.0, 2048.0], &[-10.0, 0.0, 10.0]), 24.0);

        let mut vertices = Vec::new();
        assert_eq!(label.place_glyphs(false, true, true, &mut vertices), Err(Unplaced::NeedsFlipping));
        assert_eq!(vertices.len(), 0);
        assert_eq!(label.place_glyphs(true, true, true, &mut vertices), Ok(()));
        let xs: Vec<f32> = vertices.iter().step_by(4).map(|vertex| vertex.a1[0]).collect();
        assert_eq!(vertices.len(), 12);
        for (x, expected) in xs.iter().zip([246.0, 256.0, 266.0]) {
            assert!((x - expected).abs() < 1e-3, "{xs:?}");
        }
    }
}
