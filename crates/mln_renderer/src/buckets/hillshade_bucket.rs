// A raster-dem tile's bucket lives on the render thread, in its tile's
// contents. Its layers each keep the render target they prepare its DEM
// into, per bucket id, and the bucket counts the changes that call for
// preparing it again (`render_target_generation`).

use crate::bucket::BucketBase;
use crate::buckets::raster_bucket::MaskedGeometry;
use crate::geometry::dem_data::DEMData;
use crate::gfx::context::Context;
use crate::tile_mask::TileMask;

/// A raster-dem tile's DEM, which the tile backfills from its neighbours,
/// and the geometry that draws it, which the tile masks (`set_mask`).
#[derive(Clone)]
pub(crate) struct HillshadeBucket {
    pub(crate) base: BucketBase,
    geometry: MaskedGeometry,
    demdata: DEMData,
    /// Incremented whenever the render target has to be prepared again.
    render_target_generation: u64,
}

impl HillshadeBucket {
    #[must_use]
    pub(crate) fn with_dem_data(demdata: DEMData) -> Self {
        HillshadeBucket {
            base: BucketBase::default(),
            geometry: MaskedGeometry::default(),
            demdata,
            render_target_generation: 0,
        }
    }

    /// The masked geometry.
    pub(crate) fn geometry(&self) -> &MaskedGeometry {
        &self.geometry
    }

    pub(crate) fn dem_data(&self) -> &DEMData {
        &self.demdata
    }

    pub(crate) fn dem_data_mut(&mut self) -> &mut DEMData {
        &mut self.demdata
    }

    /// Calls for the layers to prepare the DEM's render target again.
    pub(crate) fn reset_render_target_prepared(&mut self) {
        self.render_target_generation += 1;
    }

    /// The number of times the render target was called to be prepared
    /// again.
    pub(crate) fn render_target_generation(&self) -> u64 {
        self.render_target_generation
    }

    pub(crate) fn set_mask(&mut self, mask: TileMask) {
        if self.geometry.set_mask(mask) {
            self.base.uploaded = false;
        }
    }

    /// Whether the bucket has a DEM to draw.
    #[must_use]
    pub(crate) fn has_data(&self) -> bool {
        self.dem_data().get_image().valid()
    }

    /// Readies a bucket with a DEM for its draws, once and again after its
    /// geometry changes: moves its geometry into `context`'s stores. The
    /// layers make the DEM's textures.
    pub(crate) fn upload(&mut self, context: &mut Context) {
        if !self.base.uploaded && self.has_data() {
            self.geometry.upload(context);
            self.base.uploaded = true;
        }
    }
}
