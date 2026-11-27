//! What a raster tile holds on the render thread: its decoded image, and
//! the parts of its extent it draws, which its source masks. The layers that
//! draw it make their textures and quads of them.

use std::sync::Arc;

use mln_style::bitmap::PremultipliedImage;
use mln_style::tile_id::CanonicalTileID;

use crate::draw::identity::SimpleIdentity;
use crate::layer::layer_drawing::MadeFrom;
use crate::tile_store::tile_mask::{TileMask, is_whole};

/// The mask that keeps a tile's whole extent.
fn whole() -> TileMask {
    let mut mask = TileMask::new();
    mask.insert(CanonicalTileID::new(0, 0, 0));
    mask
}

/// The parts of a raster or raster-dem tile's extent that it draws: the
/// whole tile, unless better tiles cover some of it.
pub struct Masking {
    mask: TileMask,
    /// Counts the changes of the mask, after which the layers build their
    /// draws of the tile again.
    generation: u64,
}

impl Default for Masking {
    /// The whole tile.
    fn default() -> Self {
        Masking { mask: whole(), generation: 0 }
    }
}

impl Masking {
    /// The tiles of the tile's extent that it draws: the whole tile's,
    /// `(0, 0, 0)`, unless better tiles cover some of it.
    #[must_use]
    pub fn mask(&self) -> &TileMask {
        &self.mask
    }

    /// The number of times the mask changed.
    #[must_use]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether the mask keeps less than the whole tile.
    #[must_use]
    pub(crate) fn is_masked(&self) -> bool {
        !is_whole(&self.mask)
    }

    /// Makes `mask` the tile's. Returns whether it changed.
    pub(crate) fn set(&mut self, mask: TileMask) -> bool {
        if self.mask == mask {
            return false;
        }
        self.mask = mask;
        self.generation += 1;
        true
    }

    /// Makes the whole tile the mask, unless it is. Returns whether it
    /// changed.
    pub(crate) fn unmask(&mut self) -> bool {
        self.is_masked() && self.set(whole())
    }
}

/// A raster tile's image, and the parts of the tile it draws.
pub struct RasterContents {
    id: SimpleIdentity,
    pub(crate) image: Arc<PremultipliedImage>,
    pub masking: Masking,
}

impl RasterContents {
    #[must_use]
    pub(crate) fn new(image: PremultipliedImage) -> Self {
        Self::with_shared_image(Arc::new(image))
    }

    /// Contents of `image`, which they share, drawn whole.
    #[must_use]
    pub(crate) fn with_shared_image(image: Arc<PremultipliedImage>) -> Self {
        RasterContents { id: SimpleIdentity::new(), image, masking: Masking::default() }
    }

    /// The contents' identity, by which the layers keep what they make of
    /// them.
    #[must_use]
    pub(crate) fn id(&self) -> SimpleIdentity {
        self.id
    }

    /// What a layer's value of a tile of these contents is made from: their
    /// identity, with the generation of their mask.
    #[must_use]
    pub(crate) fn stamp(&self) -> MadeFrom {
        (self.id, [self.masking.generation(), 0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(ids: &[(u8, u32, u32)]) -> TileMask {
        ids.iter().map(|&(z, x, y)| CanonicalTileID::new(z, x, y)).collect()
    }

    #[test]
    fn a_mask_counts_its_changes() {
        let mut masking = Masking::default();
        assert!(!masking.is_masked());
        assert!(!masking.set(whole()), "the whole tile, as before");
        assert!(!masking.unmask());
        assert_eq!(masking.generation(), 0);

        assert!(masking.set(mask(&[(1, 0, 0)])));
        assert!(masking.is_masked());
        assert!(!masking.set(mask(&[(1, 0, 0)])));
        assert_eq!(masking.generation(), 1);

        assert!(masking.unmask());
        assert_eq!(masking.mask(), &whole());
        assert_eq!(masking.generation(), 2);

        assert!(masking.set(mask(&[])));
        assert!(masking.is_masked(), "an empty mask keeps less than the whole tile");
    }
}
