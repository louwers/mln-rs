// C++ passes four callbacks that all touch the caller's tile map and hands
// `Tile*`s between them. Here the caller implements `RenderablesContext`,
// and tiles are named by their key in the caller's tile storage (the
// `OverscaledTileID` the tile was stored under).

use std::collections::HashSet;

use mln_tile_id::{OverscaledTileID, UnwrappedTileID};
use mln_util::range::Range;

use crate::tile::tile::TileNecessity;

/// The tile storage and the callbacks `updateRenderables` works with.
pub trait RenderablesContext {
    /// Returns the key of the existing tile with this id, if there is one.
    fn get_tile(&mut self, id: &OverscaledTileID) -> Option<OverscaledTileID>;
    /// Creates and stores the tile with this id and returns its key; `None`
    /// if no tile is created (e.g. outside the TileJSON bounds).
    fn create_tile(&mut self, id: &OverscaledTileID) -> Option<OverscaledTileID>;
    fn retain_tile(&mut self, tile: &OverscaledTileID, necessity: TileNecessity);
    fn render_tile(&mut self, id: &UnwrappedTileID, tile: &OverscaledTileID);

    /// Returns the prefetched tiles, as (tile id, storage key) pairs.
    fn prefetched_tiles(&self) -> Vec<PrefetchedTile>;

    fn is_renderable(&self, tile: &OverscaledTileID) -> bool;
    fn has_tried_cache(&self, tile: &OverscaledTileID) -> bool;
    fn is_loaded(&self, tile: &OverscaledTileID) -> bool;
}

/// A `PrefetchedTileMap` entry: a tile's id, and its key in the storage.
pub type PrefetchedTile = (OverscaledTileID, OverscaledTileID);

pub fn update_renderables<C: RenderablesContext>(
    ctx: &mut C,
    ideal_tile_ids: &[OverscaledTileID],
    zoom_range: &Range<u8>,
    max_parent_overscale_factor: Option<u8>,
) {
    let mut checked: HashSet<OverscaledTileID> = HashSet::new();
    let mut covered;
    let mut parent_or_child_tile_found;
    let mut overscaled_z: i32;

    // for (all in the set of ideal tiles of the source) {
    for ideal_data_tile_id in ideal_tile_ids {
        debug_assert!(ideal_data_tile_id.canonical.z >= zoom_range.min);
        debug_assert!(ideal_data_tile_id.canonical.z <= zoom_range.max);
        debug_assert!(ideal_data_tile_id.overscaled_z >= ideal_data_tile_id.canonical.z);

        let ideal_render_tile_id = ideal_data_tile_id.to_unwrapped();
        let mut tile = ctx.get_tile(ideal_data_tile_id);
        if tile.is_none() {
            tile = ctx.create_tile(ideal_data_tile_id);
            // For source types where TileJSON.bounds is set, tiles outside
            // the bounds are not created
            if tile.is_none() {
                continue;
            }
        }
        let ideal_tile = tile.expect("tile");

        // if (source has the tile and bucket is loaded) {
        if ctx.is_renderable(&ideal_tile) {
            ctx.retain_tile(&ideal_tile, TileNecessity::Required);
            ctx.render_tile(&ideal_render_tile_id, &ideal_tile);
        } else {
            // We are now attempting to load child and parent tiles.
            let mut parent_has_tried_optional = ctx.has_tried_cache(&ideal_tile);
            let mut parent_is_loaded = ctx.is_loaded(&ideal_tile);

            // The tile isn't loaded yet, but retain it anyway because it's an
            // ideal tile.
            ctx.retain_tile(&ideal_tile, TileNecessity::Required);
            covered = true;
            parent_or_child_tile_found = false;
            overscaled_z = i32::from(ideal_data_tile_id.overscaled_z) + 1;
            if overscaled_z > i32::from(zoom_range.max) {
                // We're looking for an overzoomed child tile.
                let child_data_tile_id = ideal_data_tile_id.scaled_to(overscaled_z as u8);
                let tile = ctx.get_tile(&child_data_tile_id);
                match tile {
                    Some(tile) if ctx.is_renderable(&tile) => {
                        ctx.retain_tile(&tile, TileNecessity::Optional);
                        ctx.render_tile(&ideal_render_tile_id, &tile);
                        parent_or_child_tile_found = true;
                    }
                    _ => covered = false,
                }
            } else {
                // Check all four actual child tiles.
                for child_tile_id in ideal_data_tile_id.canonical.children() {
                    let child_data_tile_id =
                        OverscaledTileID::new(overscaled_z as u8, ideal_render_tile_id.wrap, child_tile_id);
                    let tile = ctx.get_tile(&child_data_tile_id);
                    match tile {
                        Some(tile) if ctx.is_renderable(&tile) => {
                            ctx.retain_tile(&tile, TileNecessity::Optional);
                            ctx.render_tile(&child_data_tile_id.to_unwrapped(), &tile);
                            parent_or_child_tile_found = true;
                        }
                        _ => {
                            // At least one child tile doesn't exist, so we
                            // are going to look for parents as well.
                            covered = false;
                        }
                    }
                }
            }

            if !covered {
                // We couldn't find child tiles that entirely cover the ideal
                // tile.
                overscaled_z = i32::from(ideal_data_tile_id.overscaled_z) - 1;
                while overscaled_z >= i32::from(zoom_range.min) {
                    let parent_data_tile_id = ideal_data_tile_id.scaled_to(overscaled_z as u8);

                    // Request / render parent tile only if it's overscale
                    // factor is less than defined maximum.
                    if let Some(max_parent_overscale_factor) = max_parent_overscale_factor
                        && i32::from(ideal_data_tile_id.overscaled_z) - overscaled_z
                            > i32::from(max_parent_overscale_factor)
                    {
                        break;
                    }

                    if checked.contains(&parent_data_tile_id) {
                        // Break parent tile ascent, this route has been
                        // checked by another child tile before.
                        break;
                    }
                    checked.insert(parent_data_tile_id);

                    let mut tile = ctx.get_tile(&parent_data_tile_id);
                    if tile.is_none() && (parent_has_tried_optional || parent_is_loaded) {
                        tile = ctx.create_tile(&parent_data_tile_id);
                    }

                    if let Some(tile) = tile {
                        if parent_is_loaded {
                            // Now that we've checked the child and know for
                            // sure that we can't load it, we attempt to load
                            // the parent from the network.
                            ctx.retain_tile(&tile, TileNecessity::Required);
                        } else {
                            // We haven't completed loading the child, so we
                            // only do an optional (cache) request in an
                            // attempt to quickly load data that we can show.
                            ctx.retain_tile(&tile, TileNecessity::Optional);
                        }

                        // Save the current values, since they're the parent
                        // of the next iteration of the parent tile ascent
                        // loop.
                        parent_has_tried_optional = ctx.has_tried_cache(&tile);
                        parent_is_loaded = ctx.is_loaded(&tile);

                        if ctx.is_renderable(&tile) {
                            ctx.render_tile(&parent_data_tile_id.to_unwrapped(), &tile);
                            parent_or_child_tile_found = true;
                            // Break parent tile ascent, since we found one.
                            break;
                        }
                    }
                    overscaled_z -= 1;
                }

                if !parent_or_child_tile_found {
                    // Reuse prefetched tiles in order to avoid empty screen
                    for (prefetched_data_tile_id, prefetched_tile) in ctx.prefetched_tiles() {
                        let prefetched_render_tile_id = prefetched_data_tile_id.to_unwrapped();
                        if ctx.is_renderable(&prefetched_tile)
                            && prefetched_data_tile_id.canonical.z <= zoom_range.max
                            && prefetched_data_tile_id.is_child_of(ideal_data_tile_id)
                        {
                            ctx.retain_tile(&prefetched_tile, TileNecessity::Optional);
                            ctx.render_tile(&prefetched_render_tile_id, &prefetched_tile);
                        }
                    }
                }
            }
        }
    }
}
