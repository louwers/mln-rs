use std::fmt;

use glam::DVec2;
use mln_util::geo::LatLng;

/// The ground of a terrain as a frame drew it, which the map's camera
/// stands on. A renderer makes it and hands it to the map; it doesn't change
/// once made, so the map reads it on its own thread.
pub trait Elevation: fmt::Debug + Send + Sync {
    /// The height of the ground at `location`, in meters with the terrain's
    /// exaggeration, as the frame drew it: by the height rule, from the DEM
    /// that the tile drawn there read, its own or, while that loads, an
    /// ancestor's. `Some(0.0)` where no DEM data will come; `None` where no
    /// tile was drawn, or the tile has no DEM data yet but will.
    fn at(&self, location: LatLng) -> Option<f64>;

    /// The lowest height, in meters with the exaggeration, of the DEM that
    /// the tile drawn at `location` read; `None` where no tile was drawn.
    fn min_at(&self, location: LatLng) -> Option<f64>;

    /// The height of the surface the frame drew at `world`, a Web Mercator
    /// point at zoom 0 (from 0 to 1 across, y south, x in any world copy), in
    /// meters with the exaggeration: as [`at`](Self::at), but `Some(0.0)`
    /// where a drawn tile has no DEM data yet, which draws flat meanwhile.
    /// `None` where no tile was drawn.
    fn surface_at(&self, world: DVec2) -> Option<f64>;
}
