use std::fmt;

use glam::DVec2;
use mln_style::geo::LatLng;

/// The ground of a terrain as a frame drew it, which the map's camera
/// stands on. A renderer makes it and hands it to the map; it doesn't change
/// once made, so the map reads it on its own thread.
pub trait Elevation: fmt::Debug + Send + Sync {
    /// The height of the ground at `location`, in meters with the terrain's
    /// exaggeration, as the frame drew it: by the height rule, from the DEM
    /// that the tile drawn there read, its own or, while that loads, an
    /// ancestor's; 0 where the tile draws flat, as before any of them has
    /// loaded. `None` where no tile was drawn.
    fn at(&self, location: LatLng) -> Option<f64>;

    /// The lowest height, in meters with the exaggeration, of the DEM that
    /// the tile drawn at `location` read; `None` where no tile was drawn.
    fn min_at(&self, location: LatLng) -> Option<f64>;

    /// The height of the surface the frame drew at `world`, a Web Mercator
    /// point at zoom 0 (from 0 to 1 across, y south, x in the world copy the
    /// view shows it in), in meters with the exaggeration: as
    /// [`at`](Self::at), but only of the tiles drawn in `world`'s copy.
    /// `None` where no tile was drawn there.
    fn surface_at(&self, world: DVec2) -> Option<f64>;

    /// The lowest and highest ground of the tiles the frame drew, in meters
    /// with the exaggeration, as their DEM data says, 0 for a tile drawn
    /// flat: it holds every height [`surface_at`](Self::surface_at) gives
    /// but along a tile's edge, which reads its neighbors' data within half
    /// a pixel of the DEM. `None` where no tile was drawn.
    fn range(&self) -> Option<(f64, f64)>;
}
