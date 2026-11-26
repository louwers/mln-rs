// The label-plane matrices, and the CPU projection that lays line labels'
// glyphs along their lines as the camera sees them. Symbols are not
// instanced: each glyph has four dynamic vertices.

use std::f32::consts::PI;
use std::iter;

use geometry::Point;
use glam::{DMat4, DVec2, DVec3, DVec4, Vec2};
use mln_map_types::{ProjectedTilePoint, TileProjector, View};
use mln_style::feature::GeometryCoordinate;
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style::types::SymbolHeightAnchorType;
use mln_tile_id::UnwrappedTileID;
use mln_util::constants::ONE_EM;

use crate::buckets::symbol_bucket::{
    PlacedSymbol, PlacedSymbolPools, SymbolBucket, SymbolDynamicLayoutVertex, SymbolSizeBinder, ZoomEvaluatedSize,
};
use crate::layout::symbol_instance::WritingMode;
use crate::terrain::{TerrainElevation, TileGround};

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
    if size.kind.is_feature_constant() {
        size.size
    } else if size.kind.is_zoom_constant() {
        symbol.lower_size
    } else {
        symbol.lower_size + size.size_t * (symbol.upper_size - symbol.lower_size)
    }
}

/// Appends the four vertices of a glyph at `anchor`, turned by `angle`,
/// raised to `height` where it is of a label along a line pitched with the
/// map.
pub(crate) fn add_dynamic_attributes(
    anchor: Vec2,
    angle: f32,
    height: f32,
    vertices: &mut Vec<SymbolDynamicLayoutVertex>,
) {
    vertices.extend([SymbolBucket::dynamic_layout_attributes(anchor, angle, height); 4]);
}

/// Appends the vertices of `glyph_count` glyphs placed where nothing draws.
pub(crate) fn hide_glyphs(glyph_count: usize, vertices: &mut Vec<SymbolDynamicLayoutVertex>) {
    let hidden = SymbolBucket::dynamic_layout_attributes(Vec2::NEG_INFINITY, 0.0, 0.0);
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

/// How high a tile's symbols stand: over terrain, on the ground under their
/// anchors, where `symbol-height-anchor` measures their
/// `symbol-height-offset` from the ground; else on sea level.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SymbolHeights<'a> {
    ground: Option<TileGround<'a>>,
}

impl<'a> SymbolHeights<'a> {
    /// The heights of the symbols of `layout` in `tile`, over `ground` where
    /// terrain draws. Symbols drawn over the view (`symbol-screen-space`)
    /// stand on nothing.
    pub(crate) fn new(
        ground: Option<&'a TerrainElevation>,
        tile: UnwrappedTileID,
        layout: &SymbolLayoutProperties::PossiblyEvaluated,
    ) -> Self {
        let from_ground = *layout.symbol_height_anchor.uniform() == SymbolHeightAnchorType::Ground
            && !*layout.symbol_screen_space.uniform();
        SymbolHeights { ground: ground.filter(|_| from_ground).map(|ground| TileGround::new(ground, tile)) }
    }

    /// Whether the symbols stand on the ground, rather than on sea level.
    #[must_use]
    pub(crate) fn on_ground(&self) -> bool {
        self.ground.is_some()
    }

    /// The height above sea level, in meters, of a symbol `height_offset`
    /// meters up from its anchor at `point`, in tile units, before
    /// `*-translate`: the translation moves a symbol on screen, not over the
    /// ground, as GL JS's shaders draw it.
    #[must_use]
    pub(crate) fn at(&self, point: Vec2, height_offset: f32) -> f32 {
        self.ground.map_or(0.0, |ground| ground.height(point)) + height_offset
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
    /// What the symbols stand on, and the symbol's `symbol-height-offset`,
    /// in meters, above it.
    heights: SymbolHeights<'a>,
    height_offset: f32,
}

impl<'a> LabelPlaneProjector<'a> {
    /// A projector of `tile`'s points, moved by `translation` in tile
    /// units, into the label plane, for symbols that stand at `heights`.
    #[must_use]
    pub(crate) fn new(
        tile: &TileProjector<'a>,
        heights: SymbolHeights<'a>,
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
            heights,
            height_offset: 0.0,
        }
    }

    /// This projector for a symbol `height_offset` meters up from its
    /// anchor. The pitched label plane is the same at any height; the
    /// shaders raise it.
    #[must_use]
    pub(crate) fn raised(&self, height_offset: f32) -> Self {
        LabelPlaneProjector { height_offset, ..self.clone() }
    }

    /// The height above sea level, in meters, that a symbol stands at over
    /// `tile_point`, before its translation, which moves it on screen only.
    #[must_use]
    pub(crate) fn elevation(&self, tile_point: Vec2) -> f32 {
        self.heights.at(tile_point, self.height_offset)
    }

    /// Tile units to the label plane, at the height of the ground under
    /// `tile_point`: a point of the geometry a label is laid out along.
    /// Distance and occlusion come from the projection.
    #[must_use]
    pub(crate) fn project(&self, tile_point: Vec2) -> ProjectedPoint {
        self.project_at(tile_point, self.elevation(tile_point))
    }

    /// Tile units to the label plane, at `elevation` meters above sea
    /// level: for a probe of a direction from an anchor, which the shaders
    /// take at the anchor's height.
    #[must_use]
    pub(crate) fn project_at(&self, tile_point: Vec2, elevation: f32) -> ProjectedPoint {
        let translated = tile_point + self.translation;
        if self.pitch_with_map {
            let pos = self.pitched * translated.as_dvec2().extend(0.0).extend(1.0);
            return ProjectedPoint {
                point: (pos.truncate().truncate() / pos.w).as_vec2(),
                camera_distance: pos.w as f32,
                occluded: false,
            };
        }
        let projected = self.tile.project(&tile_point_f64(translated), f64::from(elevation));
        let clip = DVec2::new(projected.point.x, projected.point.y);
        ProjectedPoint {
            point: ((clip * DVec2::new(0.5, -0.5) + 0.5) * self.viewport.as_dvec2()).as_vec2(),
            ..projected.into()
        }
    }

    /// Label plane to clip space, for a point of a label's path
    /// `elevation` meters above sea level, which a point in the plane
    /// pitched with the map needs and one in the viewport's has already.
    #[must_use]
    pub(crate) fn to_clip_space(&self, label_plane_point: Vec2, elevation: f32) -> Vec2 {
        if self.pitch_with_map {
            let pos = self.pitched_inverse * label_plane_point.as_dvec2().extend(0.0).extend(1.0);
            let tile_point = (pos.truncate().truncate() / pos.w).as_vec2();
            let projected: ProjectedPoint = self.tile.project(&tile_point_f64(tile_point), f64::from(elevation)).into();
            return projected.point;
        }
        Vec2::new(1.0, -1.0) * (label_plane_point / self.viewport * 2.0 - 1.0)
    }

    /// Tile units straight to clip space.
    #[must_use]
    pub(crate) fn to_clip_space_from_tile(&self, tile_point: Vec2) -> ProjectedPoint {
        self.tile.project(&tile_point_f64(tile_point), f64::from(self.elevation(tile_point))).into()
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
    /// The height above sea level, in meters, of the label's path at the
    /// glyph ([`LabelPlacer::line_height`]), which the shaders raise a glyph
    /// pitched with the map to.
    pub(crate) height: f32,
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
    writing_modes: WritingMode,
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
    /// The heights of the ground under each vertex of the line, as the walk
    /// reaches them; none where the labels don't stand on the ground.
    heights: Vec<Option<f32>>,
}

impl<'a> LinePlacer<'a> {
    #[must_use]
    pub(crate) fn new(label_plane: &'a LabelPlaneProjector<'a>) -> Self {
        LinePlacer { label_plane, projections: Vec::new(), heights: Vec::new() }
    }

    /// Starts placing `label`, its glyphs `font_size` pixels in the label
    /// plane.
    pub(crate) fn begin(&mut self, label: LineLabel<'a>, font_size: f32) -> LabelPlacer<'_, 'a> {
        self.projections.clear();
        self.projections.resize(label.line.len(), None);
        self.heights.clear();
        if self.label_plane.heights.on_ground() {
            self.heights.resize(label.line.len(), None);
        }
        let font_scale = font_size / ONE_EM;
        LabelPlacer {
            label_plane: self.label_plane,
            projections: &mut self.projections,
            heights: &mut self.heights,
            anchor_height: self.label_plane.elevation(label.anchor),
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
    /// The heights of the ground under the line's vertices, and its anchor.
    heights: &'s mut [Option<f32>],
    anchor_height: f32,
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
    /// The height above sea level of the label's path `tile_distance` tile
    /// units along its line from its anchor, back along the line where
    /// negative. The path runs through the anchor and the line's vertices,
    /// each at the height of the ground under it, straight between them,
    /// and keeps its last height past the line's ends. The glyphs are laid
    /// out along it ([`glyph_at`](Self::glyph_at)), and what follows it, as
    /// collision circles, follows the glyphs.
    pub(crate) fn line_height(&mut self, tile_distance: f32) -> f32 {
        if self.heights.is_empty() {
            // Off the ground, the path is level.
            return self.anchor_height;
        }
        let forward = tile_distance > 0.0;
        let distance = tile_distance.abs();
        let segment = self.label.segment;
        let line_len = self.label.line.len();
        let first = if forward { segment + 1 } else { segment };
        let vertices = iter::successors(Some(first), |&i| if forward { Some(i + 1) } else { i.checked_sub(1) })
            .take_while(|&i| i < line_len);
        let (mut prev_distance, mut prev_height) = (0.0, self.anchor_height);
        for index in vertices {
            let (label_plane, line) = (self.label_plane, self.label.line);
            let height = *self.heights[index].get_or_insert_with(|| label_plane.elevation(tile_point(line[index])));
            let vertex_distance = self.label.tile_distances.get(index).copied().unwrap_or_default();
            if vertex_distance >= distance {
                let span = vertex_distance - prev_distance;
                let t = if span > 0.0 { (distance - prev_distance) / span } else { 0.0 };
                return prev_height + (height - prev_height) * t;
            }
            (prev_distance, prev_height) = (vertex_distance, height);
        }
        prev_height
    }

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
        let t = along_segment / segment_length;
        let point = prev + direction * t + direction.perp() * (side / segment_length);
        let tile_distance = |vertex: Option<usize>| {
            vertex.map_or(0.0, |i| self.label.tile_distances.get(i).copied().unwrap_or_default())
        };
        let prev_tile_distance = tile_distance(prev_vertex);
        // Where the glyph is along the line, at the fraction of its segment
        // it is in the label plane, which the path's height follows.
        let along_line = prev_tile_distance + (tile_distance(current_vertex) - prev_tile_distance) * t;
        Some(PlacedGlyph {
            point,
            height: self.line_height(if forward { along_line } else { -along_line }),
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
            let angle = if rotate_to_line { glyph.angle } else { 0.0 };
            add_dynamic_attributes(glyph.point, angle, glyph.height, vertices);
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
                    let (first_clip, last_clip) = (
                        label_plane.to_clip_space(first.point, first.height),
                        label_plane.to_clip_space(last.point, last.height),
                    );
                    self.check_upright(first_clip, last_clip)?;
                }
                push(first);
                for &offset in middle {
                    // Between two glyphs that fit, every glyph fits; one that
                    // rounding puts off the line is hidden.
                    let glyph = self.glyph_at(offset, flip).unwrap_or(PlacedGlyph {
                        point: Vec2::NEG_INFINITY,
                        height: 0.0,
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
        if writing_modes == WritingMode::Both {
            let aspect_ratio = self.label_plane.viewport.x / self.label_plane.viewport.y;
            let rise = (last.y - first.y).abs();
            let run = (last.x - first.x).abs() * aspect_ratio;
            if rise > run {
                return Err(Unplaced::UseVertical);
            }
        }
        let upside_down = if writing_modes == WritingMode::Vertical { first.y < last.y } else { first.x > last.x };
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
/// `symbols`, placed along their lines, which stand at `heights`, for
/// `tile_projector`'s view.
#[expect(clippy::too_many_arguments, reason = "the labels, their buffer's pools, and how they project")]
pub(crate) fn reproject_line_labels(
    vertices: &mut Vec<SymbolDynamicLayoutVertex>,
    symbols: &[PlacedSymbol],
    pools: &PlacedSymbolPools,
    tile_projector: &TileProjector<'_>,
    heights: SymbolHeights<'_>,
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
        heights,
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
        if symbol.hidden || (symbol.writing_modes == WritingMode::Vertical && !vertical_shows) {
            hide_glyphs(glyph_count, vertices);
            continue;
        }

        let tile_anchor = tile_point_f64(symbol.anchor_point);
        let anchor = tile_projector.project(&tile_anchor, f64::from(heights.at(symbol.anchor_point, 0.0)));
        if anchor.occluded || !DVec2::new(anchor.point.x, anchor.point.y).abs().cmple(clip_extent).all() {
            hide_glyphs(glyph_count, vertices);
            continue;
        }

        // The shaders scale the glyphs by the anchor's perspective ratio, so
        // they are spaced along the line by it too.
        let camera_to_anchor = anchor.signed_distance_from_camera as f32;
        let distance_ratio = if alignment.pitch_with_map {
            camera_to_anchor / camera_to_center
        } else {
            camera_to_center / camera_to_anchor
        };
        let perspective_ratio = (0.5 + 0.5 * distance_ratio).clamp(0.0, 4.0);
        let font_size = evaluate_size_for_feature(&sizes, symbol) * perspective_ratio;
        let font_size = if alignment.pitch_with_map {
            font_size * tile_projector.pitched_text_correction(&tile_anchor) as f32
        } else {
            font_size
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
    use mln_map::transform::Transform;
    use mln_map_types::{ConstrainMode, TileProjector, TransformState, View, ViewportMode};
    use mln_style::tile_id::CanonicalTileID;
    use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
    use mln_util::geo::LatLng;
    use mln_util::size::Size;

    use super::{
        LabelPlaneProjector, LineLabel, LineLabelAlignment, LinePlacer, PlacedGlyph, PlacedSymbol, PlacedSymbolPools,
        SymbolHeights, SymbolSizeBinder, TileDistance, Unplaced, WritingMode, reproject_line_labels,
    };
    use crate::util::pool::PoolRange;

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
            writing_modes: WritingMode::Horizontal,
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
        let label_plane = LabelPlaneProjector::new(&tile, SymbolHeights::default(), false, true, 16.0, Vec2::ZERO);
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
        let label_plane = LabelPlaneProjector::new(&tile, SymbolHeights::default(), false, true, 16.0, Vec2::ZERO);
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

    /// Tile 12/2000/1500, in a 512 px square view of its middle at zoom
    /// 12.5, pitched 60° and turned 0.4 rad.
    fn pitched_tile() -> (OverscaledTileID, View) {
        let tile = OverscaledTileID::new(12, 0, CanonicalTileID::new(12, 2000, 1500));
        let mut state = TransformState::default();
        state.set_size(Size::new(512, 512));
        let n = std::f64::consts::PI * (1.0 - 2.0 * 1500.5 / 4096.0);
        let center = LatLng::new(n.sinh().atan().to_degrees(), 2000.5 / 4096.0 * 360.0 - 180.0).unwrap();
        state.set_lat_lng_zoom(&center, 12.5);
        state.set_pitch(60f64.to_radians());
        state.set_angle(0.4);
        (tile, state.view().clone())
    }

    /// The 24 px line label `glyph_offsets` along `line` from `anchor`, with
    /// its line's pools.
    fn line_label(anchor: Vec2, line: &[Point<i16>], glyph_offsets: &[f32]) -> (PlacedSymbol, PlacedSymbolPools) {
        let mut pools = PlacedSymbolPools::default();
        let vertices: Vec<Vec2> = line.iter().map(|&p| super::tile_point(p)).collect();
        let symbol = PlacedSymbol {
            anchor_point: anchor,
            segment: 0,
            lower_size: 24.0,
            upper_size: 24.0,
            line_offset: [0.0; 2],
            writing_modes: WritingMode::Horizontal,
            line: PoolRange::extend(&mut pools.lines, line.iter().copied()),
            tile_distances: PoolRange::extend(&mut pools.tile_distances, vertices.iter().map(|v| v.distance(anchor))),
            glyph_offsets: PoolRange::extend(&mut pools.glyph_offsets, glyph_offsets.iter().copied()),
            start_index: 0,
            angle: 0.0,
            height_offset: 0.0,
            hidden: false,
        };
        (symbol, pools)
    }

    /// A viewport-pitched label along a line away from the view's center is
    /// spaced by the size its glyphs draw at, which the shaders scale by
    /// 0.5 + 0.5 × the camera's distance to the center over its distance to
    /// the anchor.
    #[test]
    fn a_viewport_label_is_spaced_by_the_size_its_glyphs_draw_at() {
        let (tile, view) = pitched_tile();
        let projector = TileProjector::new(&view, &tile.to_unwrapped());
        // Below the view's center, nearer the camera.
        let anchor = Vec2::new(4100.0, 7200.0);
        let (symbol, pools) = line_label(anchor, &[Point::new(1000, 7200), Point::new(7000, 7200)], &[-40.0, 40.0]);
        let alignment = LineLabelAlignment { pitch_with_map: false, rotate_with_map: true, keep_upright: false };
        let size = SymbolSizeBinder::Constant { layout_size: 24.0, covering_ranges: None, expression: None };
        let mut vertices = Vec::new();
        reproject_line_labels(
            &mut vertices,
            std::slice::from_ref(&symbol),
            &pools,
            &projector,
            SymbolHeights::default(),
            &size,
            Vec2::ZERO,
            alignment,
        );
        let glyphs: Vec<Vec2> =
            vertices.iter().step_by(4).map(|vertex| Vec2::new(vertex.a1[0], vertex.a1[1])).collect();
        let [first, last] = glyphs[..] else { panic!("two glyphs: {glyphs:?}") };
        assert!(first.is_finite() && last.is_finite(), "both glyphs show: {glyphs:?}");
        let font_size = first.distance(last) / 80.0 * 24.0;

        let projected = projector.project(&Point::new(f64::from(anchor.x), f64::from(anchor.y)), 0.0);
        let camera_to_anchor = projected.signed_distance_from_camera / view.camera_to_center_distance();
        let drawn = 24.0 * (0.5 + 0.5 / camera_to_anchor) as f32;
        assert!((font_size - drawn).abs() < 1e-3 * drawn, "{font_size} px, drawn at {drawn} px");
        assert!(camera_to_anchor < 0.8, "{camera_to_anchor} of the camera's distance to the center");
    }
}
