//! A tile as a frame drew it on the screen, which a rendered-feature query
//! tests a layer's features through: where a point of the tile draws, in
//! logical pixels, whether the frame shows it there, and how large a pixel
//! of what lies on the map is there.

use geometry::Point;
use glam::{DVec2, Vec2};
use mln_style::constants::EXTENT;
use mln_style::geo::WrapMode;
use mln_style::types::TranslateAnchorType;

use crate::camera::{TileCoordinate, TileProjector, VerticalPerspectiveProjection, View};
use crate::geo::ScreenCoordinate;
use crate::render::terrain::{TerrainElevation, TileGround};
use crate::render::tile::RenderTile;
use crate::tile::UnwrappedTileID;

/// A point of a feature where a frame draws it: in the view's logical
/// pixels from its top left, and how far it is from the camera, as the
/// vertex shaders' clip-space `w`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenPoint {
    pub point: Vec2,
    pub distance: f64,
}

/// A tile as a frame drew it on the screen, for a layer: projects the
/// tile's points as the layer's vertex shaders do, with the projection as
/// blended between the globe and Mercator, and decides where the frame shows
/// them.
#[derive(Clone, Copy)]
pub struct ScreenTile<'a> {
    view: &'a View,
    projector: TileProjector<'a>,
    /// The ground the layer's points stand on, for a layer that stands on
    /// terrain.
    ground: Option<TileGround<'a>>,
    /// The ground the frame drew, for a layer that meets terrain.
    elevation: Option<&'a TerrainElevation>,
}

impl<'a> ScreenTile<'a> {
    /// The tile that `projector` projects in `view`, standing on `ground`
    /// and hidden by `elevation`, the frame's ground.
    pub(crate) fn new(
        view: &'a View,
        projector: &TileProjector<'a>,
        ground: Option<TileGround<'a>>,
        elevation: Option<&'a TerrainElevation>,
    ) -> Self {
        ScreenTile { view, projector: *projector, ground, elevation }
    }

    /// The tile `tile` of `view`, for a layer that stands on `elevation`,
    /// the frame's ground, where it has some.
    pub(crate) fn standing(view: &'a View, tile: UnwrappedTileID, elevation: Option<&'a TerrainElevation>) -> Self {
        let ground = elevation.map(|elevation| TileGround::new(elevation, tile));
        ScreenTile { view, projector: TileProjector::new(view, &tile), ground, elevation }
    }

    /// The camera and projection the frame drew with.
    #[must_use]
    pub fn view(&self) -> &'a View {
        self.view
    }

    /// The tile, in the world copy it draws in.
    #[must_use]
    pub fn tile_id(&self) -> UnwrappedTileID {
        *self.projector.tile_id()
    }

    /// Whether the layer's points stand on the ground the frame drew: over
    /// terrain, for a layer that stands on it.
    #[must_use]
    pub fn stands(&self) -> bool {
        self.ground.is_some()
    }

    /// The height, in meters, of the ground that the layer stands on at
    /// `point`, in the tile's units: the ground the frame drew, for a layer
    /// that stands on terrain; else 0, sea level.
    #[must_use]
    pub fn ground_height(&self, point: Point<f64>) -> f64 {
        self.ground.map_or(0.0, |ground| f64::from(ground.height(Vec2::new(point.x as f32, point.y as f32))))
    }

    /// Where `point`, in the tile's units, `height` meters above sea level,
    /// draws; `None` where the frame doesn't show it: behind the camera or
    /// behind the globe's horizon.
    #[must_use]
    pub fn project(&self, point: Point<f64>, height: f64) -> Option<ScreenPoint> {
        let drawn = self.projector.project_as_drawn(&point, height);
        if drawn.occluded || drawn.signed_distance_from_camera <= 0.0 {
            return None;
        }
        let size = self.view.size();
        // Clip space's y goes up.
        let point = Vec2::new(
            ((drawn.point.x + 1.0) * f64::from(size.width) * 0.5) as f32,
            ((1.0 - drawn.point.y) * f64::from(size.height) * 0.5) as f32,
        );
        Some(ScreenPoint { point, distance: drawn.signed_distance_from_camera })
    }

    /// Where the ring `ring`, points in the tile's units `height` meters
    /// above sea level, shows: its points where they draw
    /// ([`project`](Self::project)), and where an edge passes out of sight,
    /// the last point of it that shows, which bisection finds.
    #[must_use]
    pub fn project_ring(&self, ring: &[Point<f64>], height: f64) -> Vec<Vec2> {
        let lerp = |a: Point<f64>, b: Point<f64>, t: f64| Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
        let screen = |p: Point<f64>| self.project(p, height).map(|p| p.point);
        let mut outline = Vec::with_capacity(ring.len());
        for (&a, &b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
            let (shown_a, shown_b) = (screen(a), screen(b));
            outline.extend(shown_a);
            let ((Some(last), None) | (None, Some(last))) = (shown_a, shown_b) else {
                continue;
            };
            // From the end that shows toward the one that doesn't.
            let (from, to) = if shown_a.is_some() { (a, b) } else { (b, a) };
            let (mut last, mut shown, mut hidden) = (last, 0.0, 1.0);
            for _ in 0..16 {
                let mid = f64::midpoint(shown, hidden);
                match screen(lerp(from, to, mid)) {
                    Some(point) => (last, shown) = (point, mid),
                    None => hidden = mid,
                }
            }
            outline.push(last);
        }
        outline
    }

    /// Whether the ground in front of what draws at `at` hides it, as the
    /// draws of a layer that hides behind the ground fade it: by GL JS's
    /// ramp of depth, past halfway, against the nearest ground the frame
    /// drew at `at`'s pixel. False without terrain, for a layer that draws
    /// flat, and where `at` is outside the view.
    #[must_use]
    pub fn behind_ground(&self, at: &ScreenPoint) -> bool {
        let Some(elevation) = self.elevation else {
            return false;
        };
        let opacity = |point: Vec2| self.ground_opacity(elevation, point, at.distance);
        // The shaders' `calculate_visibility`: a point partly hidden takes
        // the mean with the point 0.01 NDC above it.
        let visibility = opacity(at.point);
        let visibility = if visibility > 0.95 {
            1.0
        } else {
            let up = 0.005 * self.view.size().height as f32;
            f64::midpoint(visibility, opacity(at.point - Vec2::new(0.0, up)))
        };
        visibility < 0.5
    }

    /// GL JS's `depthOpacity` of a point `distance` from the camera at the
    /// screen point `point`: 1 where it is in front of the nearest ground
    /// drawn there, fading to 0 behind it; 1 where no ground is drawn there
    /// or `point` is outside the view.
    fn ground_opacity(&self, elevation: &TerrainElevation, point: Vec2, distance: f64) -> f64 {
        let size = self.view.size();
        let (width, height) = (f64::from(size.width), f64::from(size.height));
        let (x, y) = (f64::from(point.x), f64::from(point.y));
        if !(0.0..=width).contains(&x) || !(0.0..=height).contains(&y) {
            return 1.0;
        }
        let Some(hit) = self.view.ground_under(&ScreenCoordinate::new(x, height - y), elevation) else {
            return 1.0;
        };
        let Some(ground) = self.project(self.world_point(hit.world), hit.height) else {
            return 1.0;
        };
        // The ramp's scale: `2n` of a near plane `n` a fiftieth of the
        // view's height.
        let depth_factor = height / 25.0;
        let d = depth_factor * (1.0 / distance - 1.0 / ground.distance) + 0.0001;
        1.0 - (-500.0 * d).clamp(0.0, 1.0)
    }

    /// `world`, a Web Mercator point at zoom 0, in the tile's units.
    fn world_point(&self, world: DVec2) -> Point<f64> {
        let tile = self.tile_id();
        let tiles = f64::from(1u32 << tile.canonical.z);
        let extent = f64::from(EXTENT);
        Point::new(
            ((world.x - f64::from(tile.wrap)) * tiles - f64::from(tile.canonical.x)) * extent,
            (world.y * tiles - f64::from(tile.canonical.y)) * extent,
        )
    }

    /// How many of the tile's units a logical pixel of something that lies
    /// on the map spans at `point`, a point of the tile, as the shaders
    /// extrude such a size: at the view's zoom and, on the globe, the same
    /// length on the ground everywhere, the one a pixel spans at the
    /// center.
    #[must_use]
    pub fn pixel(&self, point: Point<f64>) -> f64 {
        let view = self.view;
        let units = f64::from(self.tile_id().pixels_to_tile_units(1.0, view.zoom() as f32));
        if !view.is_globe() {
            return units;
        }
        // The shaders' `projectLineThickness`: the inverse of the ratio of
        // the parallel's circumference to the equator's.
        let tile = self.tile_id();
        let tiles = f64::from(1u32 << tile.canonical.z);
        let mercator_y = (f64::from(tile.canonical.y) + point.y / f64::from(EXTENT)) / tiles;
        let t = (std::f64::consts::PI - mercator_y * std::f64::consts::TAU).exp();
        let thickness = (t * t + 1.0) / (2.0 * t);
        let transition = view.projection_transition();
        let thickness = if transition < 0.999 { 1.0 + (thickness - 1.0) * transition } else { thickness };
        units / view.projection().pixel_scale(view) * thickness
    }

    /// The translation, in the tile's units, that the draws of a layer
    /// translated by `translate` pixels anchored to `anchor` move its
    /// features by.
    #[must_use]
    pub fn translation(&self, translate: [f32; 2], anchor: TranslateAnchorType) -> DVec2 {
        let [x, y] = RenderTile::tile_unit_translation(self.projector.tile_id(), translate, anchor, self.view);
        DVec2::new(f64::from(x), f64::from(y))
    }

    /// Where the lines of sight through `screen`, points in the view's
    /// logical pixels from its top left, meet the level `height` meters
    /// above sea level, the plane or on the globe the sphere there, between
    /// the near and far planes, in the tile's units; a point whose line
    /// misses it meets it nowhere. On the globe, a ring's points whose lines
    /// miss the sphere stand for the nearest points of its horizon, so that
    /// a ring around the whole planet holds what it shows.
    #[must_use]
    pub fn on_level(&self, screen: &[ScreenCoordinate], height: f64) -> Vec<Vec2> {
        let view = self.view;
        let size_height = f64::from(view.size().height);
        let ring = screen.len() > 1;
        screen
            .iter()
            .filter_map(|p| {
                let up = ScreenCoordinate::new(p.x, size_height - p.y);
                let world = if view.is_globe() {
                    if !ring && !VerticalPerspectiveProjection::screen_coordinate_hits_globe(view, &up, height) {
                        return None;
                    }
                    let location = view.screen_coordinate_to_lat_lng_at(&up, height, WrapMode::Unwrapped).ok()?;
                    let world = TileCoordinate::from_lat_lng(0.0, &location).p;
                    DVec2::new(world.x, world.y)
                } else {
                    let (near, far) = view.line_of_sight(&up)?;
                    let t = (height - near.z) / (far.z - near.z);
                    if !(0.0..=1.0).contains(&t) {
                        return None;
                    }
                    near.lerp(far, t).truncate()
                };
                Some(self.nearest_copy(self.world_point(world)))
            })
            .collect()
    }

    /// `point`, in the tile's units, moved by whole worlds to the copy
    /// nearest the tile: on the globe, which has one copy of each place,
    /// the one the tile's features are in.
    fn nearest_copy(&self, point: Point<f64>) -> Vec2 {
        let world = f64::from(EXTENT) * f64::from(1u32 << self.tile_id().canonical.z);
        let center = f64::from(EXTENT) / 2.0;
        let x = if self.view.is_globe() { point.x - ((point.x - center) / world).round() * world } else { point.x };
        Vec2::new(x as f32, point.y as f32)
    }
}
