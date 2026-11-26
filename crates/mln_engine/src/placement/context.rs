//! What placing one bucket's symbols reads of the bucket, its tile and its
//! layer.

use glam::Vec2;
use mln_style::constants::{EXTENT, TILE_SIZE_D};
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style::types::{
    AlignmentType, IconTextFitType, SymbolPlacementType, SymbolZOrderType, TextRotationAlignmentType,
};
use mln_vec_map::VecMap;

use crate::camera::{ProjectionData, TileProjector, View};
use crate::layer::render_layer::{BucketPlacementData, PlacedTile, SymbolTranslate};
use crate::placement::collision_index::{CollisionBoundaries, OverlapMode, PartCollision};
use crate::render::terrain::TerrainElevation;
use crate::render::tile::RenderTile;
use crate::symbol::bucket::{SymbolBucket, ZoomEvaluatedSize, symbols_by_viewport_y};
use crate::symbol::projection::{LabelPlaneProjector, SymbolHeights};
use crate::tile::UnwrappedTileID;

/// The collision groups of the sources: with cross-source collisions, every
/// symbol collides with every other; without, only with its own source's.
pub(crate) struct CollisionGroups {
    ids: VecMap<String, u16>,
    cross_source_collisions: bool,
}

impl CollisionGroups {
    pub(crate) fn new(cross_source_collisions: bool) -> Self {
        CollisionGroups { ids: VecMap::new(), cross_source_collisions }
    }

    /// The group of `source_id`'s symbols, which they collide with alone;
    /// `None` where they collide with all, as group 0.
    pub(crate) fn get(&mut self, source_id: &str) -> Option<u16> {
        if self.cross_source_collisions {
            return None;
        }
        let next = self.ids.len() as u16 + 1;
        Some(*self.ids.get_or_insert_with(source_id, || next))
    }
}

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
    Vec2::from(RenderTile::tile_unit_translation(tile_id, translate.offset, translate.anchor, view))
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
    pub(crate) collision_group: Option<u16>,
    /// The tile's borders, in tile mode.
    pub(crate) tile_borders: Option<CollisionBoundaries>,
    pub(crate) avoid_edges: Option<CollisionBoundaries>,
}

/// What placing the symbols of one bucket reads.
pub(crate) struct PlacementContext<'a> {
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
    /// What the bucket's symbols stand on.
    heights: SymbolHeights<'a>,
    // Labels collide where `*-translate` puts them, line labels' circles
    // included.
    text_translation: Vec2,
    icon_translation: Vec2,
    /// The group the bucket's symbols collide with alone, and their id in
    /// the grid; `None` for every group, as group 0.
    pub(crate) collision_group: Option<u16>,
    pub(crate) text_size: ZoomEvaluatedSize,
    pub(crate) icon_size: ZoomEvaluatedSize,
    pub(crate) text_overlap_mode: OverlapMode,
    pub(crate) icon_overlap_mode: OverlapMode,
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
}

impl<'a> PlacementContext<'a> {
    /// The context for placing `bucket`, `data`'s bucket, seen from `view`,
    /// its symbols standing on `ground` where terrain draws.
    pub(crate) fn new(
        data: &'a BucketPlacementData,
        bucket: &'a SymbolBucket,
        view: &'a View,
        ground: Option<&'a TerrainElevation>,
        placement_zoom: f32,
        options: BucketOptions,
    ) -> Self {
        let BucketOptions { just_reloaded, collision_group, tile_borders, avoid_edges } = options;
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
        let heights = SymbolHeights::new(ground, tile.id, layout);
        let plane = |pitch, rotate, translation| {
            LabelPlaneProjector::new(&tile_projector, heights, pitch, rotate, pixels_to_tile_units, translation)
        };

        let text_overlap_mode = OverlapMode::text(layout);
        let icon_overlap_mode = OverlapMode::icon(layout);
        let text_always = text_overlap_mode == OverlapMode::Always;
        let icon_always = icon_overlap_mode == OverlapMode::Always;
        let has_icons = bucket.has_icon_data() || bucket.has_sdf_icon_data();
        PlacementContext {
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
            heights,
            tile_projector,
            text_translation,
            icon_translation,
            collision_group,
            text_size: bucket.text_size_binder.evaluate_for_zoom(placement_zoom),
            icon_size: bucket.icon_size_binder.evaluate_for_zoom(placement_zoom),
            text_overlap_mode,
            icon_overlap_mode,
            always_show_text: text_always && (icon_always || !has_icons || *layout.icon_optional.uniform()),
            always_show_icon: icon_always
                && (text_always || !bucket.has_text_data() || *layout.text_optional.uniform()),
            has_icon_text_fit: *layout.icon_text_fit.uniform() != IconTextFitType::None,
            tile_borders,
            avoid_edges,
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
        self.collision(&self.text_plane, self.text_translation, self.text_overlap_mode)
    }

    /// How the bucket's icons collide. Their boxes turn and pitch with the
    /// text's alignment.
    pub(crate) fn icon_collision(&self) -> PartCollision<'_> {
        self.collision(&self.icon_plane, self.icon_translation, self.icon_overlap_mode)
    }

    fn collision<'s>(
        &'s self,
        label_plane: &'s LabelPlaneProjector<'a>,
        translation: Vec2,
        overlap_mode: OverlapMode,
    ) -> PartCollision<'s> {
        PartCollision {
            tile_projector: &self.tile_projector,
            label_plane,
            heights: self.heights,
            translation,
            pixel_ratio: self.pixel_ratio,
            scale: self.scale,
            overlap_mode,
            pitch_with_map: self.pitch_text_with_map,
            rotate_with_map: self.rotate_text_with_map,
            avoid_edges: self.avoid_edges.as_ref(),
            collision_group: self.collision_group,
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
