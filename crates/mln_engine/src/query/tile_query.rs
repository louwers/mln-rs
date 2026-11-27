//! A rendered-feature query of a tile's features, as a layer's hit test
//! sees it ([`Layer::hit`](crate::layer::Layer::hit)).

use std::borrow::Cow;

use geometry::Point;
use glam::{DVec2, Vec2};
use mln_style::types::TranslateAnchorType;

use crate::camera::View;
use crate::geo::ScreenCoordinate;
use crate::query::screen::{ScreenPoint, ScreenTile};
use crate::tile::UnwrappedTileID;

/// A rendered-feature query of a tile's features, for a layer: the query in
/// the view's logical pixels, and the tile as the frame drew it there
/// ([`ScreenTile`]), through which the layer's hit test finds where its
/// features draw, in the same pixels. A layer that translates its features
/// tests them through [`translated`](Self::translated), whose projections
/// draw them translated.
#[derive(Clone, Copy)]
pub struct TileQuery<'a> {
    screen: &'a [ScreenCoordinate],
    tile: &'a ScreenTile<'a>,
    /// Where the query's lines of sight meet the ground the layer's
    /// features lie on, in the tile's units.
    ground: &'a [Vec2],
    /// The tile units the layer's draws translate its features by.
    translation: DVec2,
    /// The zoom the tile's data was laid out at.
    zoom: u8,
}

impl<'a> TileQuery<'a> {
    /// The query `screen` of `tile`, which meets the ground the layer lies
    /// on at `ground`, and whose data was laid out at `zoom`.
    pub(crate) fn new(screen: &'a [ScreenCoordinate], tile: &'a ScreenTile<'a>, ground: &'a [Vec2], zoom: u8) -> Self {
        TileQuery { screen, tile, ground, translation: DVec2::ZERO, zoom }
    }

    /// The query's geometry, in the view's logical pixels from its top left:
    /// one point, or a closed ring.
    #[must_use]
    pub fn geometry(&self) -> &'a [ScreenCoordinate] {
        self.screen
    }

    /// The query's geometry as [`Vec2`]s, in the view's logical pixels from
    /// its top left.
    #[must_use]
    pub fn pixels(&self) -> Vec<Vec2> {
        self.screen.iter().map(|p| Vec2::new(p.x as f32, p.y as f32)).collect()
    }

    /// The camera and projection the frame drew with.
    #[must_use]
    pub fn view(&self) -> &'a View {
        self.tile.view()
    }

    /// The tile, in the world copy it draws in.
    #[must_use]
    pub fn tile_id(&self) -> UnwrappedTileID {
        self.tile.tile_id()
    }

    /// The zoom the tile's data was laid out at, the zoom of a layer's
    /// layout properties in its buckets
    /// ([`LayoutCx::zoom`](crate::layer::LayoutCx::zoom)): the tile's own,
    /// overscaled past its source's maximum zoom.
    #[must_use]
    pub fn zoom(&self) -> u8 {
        self.zoom
    }

    /// The query of a layer whose draws translate its features by
    /// `translate` pixels anchored to `anchor`: its projections draw the
    /// features' points translated, and its points on the map move back by
    /// as much.
    #[must_use]
    pub fn translated(&self, translate: [f32; 2], anchor: TranslateAnchorType) -> TileQuery<'a> {
        TileQuery { translation: self.tile.translation(translate, anchor), ..*self }
    }

    /// Whether the layer's points stand on the ground the frame drew, over
    /// terrain ([`ground_height`](Self::ground_height)).
    #[must_use]
    pub fn stands(&self) -> bool {
        self.tile.stands()
    }

    /// The height, in meters, of the ground the layer stands on at `point`,
    /// in the tile's units ([`ScreenTile::ground_height`]).
    #[must_use]
    pub fn ground_height(&self, point: Point<f64>) -> f64 {
        self.tile.ground_height(point)
    }

    /// Where `point`, in the tile's units, `height` meters above sea level,
    /// draws, translated; `None` where the frame doesn't show it
    /// ([`ScreenTile::project`]).
    #[must_use]
    pub fn project(&self, point: Point<f64>, height: f64) -> Option<ScreenPoint> {
        self.tile.project(self.translate(point), height)
    }

    /// Where the ring `ring`, points in the tile's units `height` meters
    /// above sea level, shows, translated ([`ScreenTile::project_ring`]).
    #[must_use]
    pub fn project_ring(&self, ring: &[Point<f64>], height: f64) -> Vec<Vec2> {
        let ring: Vec<Point<f64>> = ring.iter().map(|&p| self.translate(p)).collect();
        self.tile.project_ring(&ring, height)
    }

    /// Whether the ground in front of what draws at `at` hides it
    /// ([`ScreenTile::behind_ground`]).
    #[must_use]
    pub fn behind_ground(&self, at: &ScreenPoint) -> bool {
        self.tile.behind_ground(at)
    }

    /// How many of the tile's units a logical pixel of something that lies
    /// on the map spans at `point` ([`ScreenTile::pixel`]).
    #[must_use]
    pub fn pixel(&self, point: Point<f64>) -> f64 {
        self.tile.pixel(point)
    }

    /// The query on the map, for features that lie on it: where its lines
    /// of sight meet the ground the layer's features lie on (over terrain,
    /// for a layer that drapes, the ground the frame drew; else sea level),
    /// in the tile's units, within the tile, where the frame draws them of
    /// this tile, and moved back by the translation. A point whose line
    /// meets no ground, or meets it outside the tile, meets the map nowhere.
    #[must_use]
    pub fn on_ground(&self) -> Cow<'a, [Vec2]> {
        let extent = mln_style::constants::EXTENT as f32;
        let inside = |p: &Vec2| (0.0..=extent).contains(&p.x) && (0.0..=extent).contains(&p.y);
        let within = if self.ground.iter().all(inside) {
            Cow::Borrowed(self.ground)
        } else if self.ground.len() < 3 {
            Cow::Owned(self.ground.iter().copied().filter(inside).collect())
        } else {
            Cow::Owned(clip_to_square(self.ground, extent))
        };
        if self.translation == DVec2::ZERO {
            return within;
        }
        let shift = self.translation.as_vec2();
        Cow::Owned(within.iter().map(|&p| p - shift).collect())
    }

    /// The query on the level `height` meters above sea level, for a mark
    /// that lies flat on the map there: where its lines of sight meet that
    /// level ([`ScreenTile::on_level`]), in the tile's units, moved back by
    /// the translation.
    #[must_use]
    pub fn on_level(&self, height: f64) -> Vec<Vec2> {
        let shift = self.translation.as_vec2();
        let mut level = self.tile.on_level(self.screen, height);
        for p in &mut level {
            *p -= shift;
        }
        level
    }

    /// `point`, translated.
    fn translate(&self, point: Point<f64>) -> Point<f64> {
        Point::new(point.x + self.translation.x, point.y + self.translation.y)
    }
}

/// The ring `ring` clipped to the square from 0 to `extent` on each axis.
fn clip_to_square(ring: &[Vec2], extent: f32) -> Vec<Vec2> {
    // Each edge of the square: whether a point is inside it, and where a
    // segment crosses it.
    let sides: [fn(Vec2, f32) -> f32; 4] = [|p, _| p.x, |p, extent| extent - p.x, |p, _| p.y, |p, extent| extent - p.y];
    let mut clipped = ring.to_vec();
    for distance in sides {
        let input = std::mem::take(&mut clipped);
        if input.is_empty() {
            break;
        }
        for (i, &a) in input.iter().enumerate() {
            let b = input[(i + 1) % input.len()];
            let (da, db) = (distance(a, extent), distance(b, extent));
            if da >= 0.0 {
                clipped.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                clipped.push(a + (b - a) * (da / (da - db)));
            }
        }
    }
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_clips_to_the_tile() {
        let ring = [Vec2::new(-10.0, 10.0), Vec2::new(10.0, 10.0), Vec2::new(10.0, 20.0), Vec2::new(-10.0, 20.0)];
        let clipped = clip_to_square(&ring, 100.0);
        assert_eq!(clipped, [Vec2::new(0.0, 10.0), Vec2::new(10.0, 10.0), Vec2::new(10.0, 20.0), Vec2::new(0.0, 20.0)]);
        let outside = [Vec2::new(-10.0, 10.0), Vec2::new(-5.0, 10.0), Vec2::new(-5.0, 20.0)];
        assert_eq!(clip_to_square(&outside, 100.0), []);
    }
}
