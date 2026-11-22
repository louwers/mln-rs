//! What placing one bucket's symbols reads of the bucket, its tile and its
//! layer.

use glam::Vec2;
use mln_map_types::{ProjectionData, TileProjector, View};
use mln_render::layer::TileTransform;
use mln_render::placement::Overlap;
use mln_style_types::{
    AlignmentType, IconTextFitType, SymbolPlacementType, SymbolZOrderType, TextRotationAlignmentType,
};
use mln_tile_id::UnwrappedTileID;
use mln_util::constants::{EXTENT, TILE_SIZE_D};

use super::{BucketPlacementData, PlacedTile, SymbolTranslate};
use crate::symbol::SymbolLayoutProperties;
use crate::symbol::bucket::{SymbolBucket, ZoomEvaluatedSize, symbols_by_viewport_y};
use crate::symbol::placement::collision::{CollisionBoundaries, PartCollision, icon_overlap, text_overlap};
use crate::symbol::projection::LabelPlaneProjector;

/// A symbol layer's `*-translate` in the tile's units.
pub(crate) fn translation(
    tile_id: &UnwrappedTileID,
    translate: &SymbolTranslate,
    view: &View,
    screen_space: bool,
) -> Vec2 {
    // A screen-space symbol takes its translate in the screen matrix.
    if screen_space {
        return Vec2::ZERO;
    }
    let how = TileTransform { translate: translate.offset, anchor: translate.anchor, ..TileTransform::default() };
    Vec2::from(how.tile_units(tile_id, view))
}

/// The projection without the horizon plane, so nothing on it counts as
/// hidden behind the planet.
fn without_horizon(mut data: ProjectionData) -> ProjectionData {
    data.clipping_plane = glam::DVec4::ZERO;
    data
}

/// What a placement decides of a bucket before it places its symbols.
#[derive(Clone, Copy)]
pub(crate) struct BucketOptions {
    /// The bucket's symbols skip their fade in.
    pub(crate) just_reloaded: bool,
    /// The tile's borders, in tile mode.
    pub(crate) tile_borders: Option<CollisionBoundaries>,
    pub(crate) avoid_edges: Option<CollisionBoundaries>,
    /// Debug drawing shows the collision boxes.
    pub(crate) debug: bool,
}

/// What placing the symbols of one bucket reads.
pub(crate) struct BucketContext<'a> {
    pub(crate) bucket: &'a SymbolBucket,
    pub(crate) data: &'a BucketPlacementData,
    pub(crate) view: &'a View,
    /// The bucket's symbols skip their fade in: placement hasn't placed it
    /// since its tile showed it.
    pub(crate) just_reloaded: bool,
    pub(crate) bearing: f32,
    /// Tile units to pixels at the tile's own zoom.
    pub(crate) pixel_ratio: f32,
    /// The scale of the placement's zoom over the tile's.
    scale: f32,
    pub(crate) rotate_text_with_map: bool,
    pub(crate) pitch_text_with_map: bool,
    /// The tile's projection; screen-space symbols are drawn over the view,
    /// so the planet doesn't hide them.
    pub(crate) tile_projector: TileProjector<'a>,
    text_plane: LabelPlaneProjector<'a>,
    icon_plane: LabelPlaneProjector<'a>,
    // Labels collide where `*-translate` puts them, line labels' circles
    // included.
    text_translation: Vec2,
    icon_translation: Vec2,
    /// The cosine of the view's pitch times its camera's distance from the
    /// center.
    pitch_factor: f32,
    pub(crate) text_size: ZoomEvaluatedSize,
    pub(crate) icon_size: ZoomEvaluatedSize,
    pub(crate) text_overlap: Overlap,
    pub(crate) icon_overlap: Overlap,
    /// A symbol whose parts always overlap shows even where placement left
    /// it out of the grid, as outside it. A part that always overlaps beside
    /// one that doesn't waits for placement, as it may hide with it.
    pub(crate) always_show_text: bool,
    pub(crate) always_show_icon: bool,
    pub(crate) has_icon_text_fit: bool,
    /// The tile's borders, in tile mode.
    pub(crate) tile_borders: Option<CollisionBoundaries>,
    /// The tile borders the symbols stay inside, in tile mode.
    pub(crate) avoid_edges: Option<CollisionBoundaries>,
    debug: bool,
}

impl<'a> BucketContext<'a> {
    pub(crate) fn new(
        data: &'a BucketPlacementData,
        bucket: &'a SymbolBucket,
        view: &'a View,
        placement_zoom: f32,
        options: BucketOptions,
    ) -> Self {
        let BucketOptions { just_reloaded, tile_borders, avoid_edges, debug } = options;
        let tile = &data.tile;
        let overscaled_id = tile.overscaled_id;
        let pixels_to_tile_units = tile.id.pixels_to_tile_units(1.0, placement_zoom);
        // In doubles, where the tile's overscaled zoom sits far below.
        let scale = 2f64.powf(f64::from(placement_zoom - f32::from(overscaled_id.overscaled_z))) as f32;
        let pixel_ratio = (TILE_SIZE_D * f64::from(overscaled_id.overscale_factor()) / f64::from(EXTENT)) as f32;

        let layout = &*bucket.layout;
        let rotate_text_with_map = *layout.text_rotation_alignment.uniform() == TextRotationAlignmentType::Map;
        let pitch_text_with_map = *layout.text_pitch_alignment.uniform() == AlignmentType::Map;
        let rotate_icon_with_map = *layout.icon_rotation_alignment.uniform() == AlignmentType::Map;
        let pitch_icon_with_map = *layout.icon_pitch_alignment.uniform() == AlignmentType::Map;
        let screen_space = *layout.symbol_screen_space.uniform();
        let text_translation = translation(&tile.id, &data.text_translate, view, screen_space);
        let icon_translation = translation(&tile.id, &data.icon_translate, view, screen_space);
        let projection = if screen_space { without_horizon(tile.projection) } else { tile.projection };
        let tile_projector = TileProjector::with_data(view, &tile.id, projection);
        let plane = |pitch, rotate, translation| {
            LabelPlaneProjector::new(&tile_projector, pitch, rotate, pixels_to_tile_units, translation)
        };

        let text_overlap = text_overlap(layout);
        let icon_overlap = icon_overlap(layout);
        let text_always = text_overlap == Overlap::Always;
        let icon_always = icon_overlap == Overlap::Always;
        let has_icons = bucket.has_icon_data() || bucket.has_sdf_icon_data();
        BucketContext {
            bucket,
            data,
            view,
            just_reloaded,
            bearing: view.angle() as f32,
            pixel_ratio,
            scale,
            rotate_text_with_map,
            pitch_text_with_map,
            text_plane: plane(pitch_text_with_map, rotate_text_with_map, text_translation),
            icon_plane: plane(pitch_icon_with_map, rotate_icon_with_map, icon_translation),
            tile_projector,
            text_translation,
            icon_translation,
            pitch_factor: (view.pitch().cos() * view.camera_to_center_distance()) as f32,
            text_size: bucket.text_size_binder.evaluate_for_zoom(placement_zoom),
            icon_size: bucket.icon_size_binder.evaluate_for_zoom(placement_zoom),
            text_overlap,
            icon_overlap,
            always_show_text: text_always && (icon_always || !has_icons || *layout.icon_optional.uniform()),
            always_show_icon: icon_always
                && (text_always || !bucket.has_text_data() || *layout.text_optional.uniform()),
            has_icon_text_fit: *layout.icon_text_fit.uniform() != IconTextFitType::None,
            tile_borders,
            avoid_edges,
            debug,
        }
    }

    pub(crate) fn layout(&self) -> &'a SymbolLayoutProperties::PossiblyEvaluated {
        &self.bucket.layout
    }

    pub(crate) fn tile(&self) -> &'a PlacedTile {
        &self.data.tile
    }

    /// How the bucket's text collides.
    pub(crate) fn text_collision(&self) -> PartCollision<'_> {
        self.collision(&self.text_plane, self.text_translation, self.text_overlap)
    }

    /// How the bucket's icons collide. Their boxes turn and pitch with the
    /// text's alignment.
    pub(crate) fn icon_collision(&self) -> PartCollision<'_> {
        self.collision(&self.icon_plane, self.icon_translation, self.icon_overlap)
    }

    fn collision<'s>(
        &'s self,
        label_plane: &'s LabelPlaneProjector<'a>,
        translation: Vec2,
        overlap: Overlap,
    ) -> PartCollision<'s> {
        PartCollision {
            view: self.view,
            pitch_factor: self.pitch_factor,
            tile_projector: &self.tile_projector,
            label_plane,
            translation,
            pixel_ratio: self.pixel_ratio,
            scale: self.scale,
            overlap,
            pitch_with_map: self.pitch_text_with_map,
            rotate_with_map: self.rotate_text_with_map,
            avoid_edges: self.avoid_edges.as_ref(),
            debug: self.debug,
        }
    }

    /// The indices of the bucket's symbols in its sort key range, in the
    /// order to place them: foreground first where they sort by viewport y.
    pub(crate) fn bucket_symbols(&self) -> Vec<usize> {
        let symbols = &self.bucket.state.symbol_instances;
        if *self.layout().symbol_z_order.uniform() == SymbolZOrderType::ViewportY {
            let mut order = symbols_by_viewport_y(symbols, self.bearing);
            order.reverse();
            return order;
        }
        self.bucket.symbols_in(self.data.sort_key_range.as_ref(), symbols.len()).collect()
    }

    /// Whether the bucket places its text along lines.
    pub(crate) fn along_line(&self) -> bool {
        *self.layout().symbol_placement.uniform() != SymbolPlacementType::Point
    }
}
