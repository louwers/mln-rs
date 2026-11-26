//! The textures of the DEM tiles the terrain's tiles read: each DEM's
//! bordered RGBA8 image, which the vertex stage reads texel by texel.

use std::collections::hash_map::Entry;
use std::rc::Rc;

use mln_style::bitmap::PremultipliedImage;
use mln_style::hash::{HashMap, HashSet};
use mln_style::size::Size;
use mln_util::identity::SimpleIdentity;

use crate::gfx::context::Context;
use crate::gfx::store::SharedTexture;
use crate::tile::dem::DemContents;

/// The DEM textures of a frame's terrain tiles, kept while a tile reads
/// them.
#[derive(Default)]
pub(crate) struct DemTextures {
    /// The texture of each DEM a tile reads, by its contents' id, with the
    /// generation of the DEM's border it holds.
    textures: HashMap<SimpleIdentity, (SharedTexture, u64)>,
    /// The DEMs the frame's tiles read so far.
    read: HashSet<SimpleIdentity>,
    /// A transparent 1×1 texture, for the tiles that read no DEM, and for
    /// the drape of a tile that the first stack draws nothing in.
    flat: Option<SharedTexture>,
}

impl DemTextures {
    /// The texture of `dem`'s DEM, which a tile of the frame reads: the one
    /// made before, or a new one if there was none or the DEM's border
    /// changed since (its `generation`).
    pub(crate) fn texture(&mut self, context: &mut Context, dem: &DemContents) -> SharedTexture {
        let id = dem.id();
        let generation = dem.generation();
        self.read.insert(id);
        let (texture, _) = match self.textures.entry(id) {
            Entry::Occupied(entry) if entry.get().1 == generation => entry.into_mut(),
            entry => {
                let image = dem.dem().image().clone();
                let texture = context.add_texture_with(|texture| texture.set_image(Some(Rc::new(image))));
                entry.insert_entry((texture, generation)).into_mut()
            }
        };
        texture.clone()
    }

    /// A transparent 1×1 texture: that of the tiles that read no DEM, whose
    /// shader reads no texel of it, and of the drape of a tile that the
    /// first stack draws nothing in.
    pub(crate) fn flat(&mut self, context: &mut Context) -> SharedTexture {
        let flat = self.flat.get_or_insert_with(|| {
            let image = PremultipliedImage::new(Size::new(1, 1));
            context.add_texture_with(|texture| texture.set_image(Some(Rc::new(image))))
        });
        flat.clone()
    }

    /// Drops the textures of the DEMs no tile of the frame read, once
    /// the frame's tiles took theirs.
    pub(crate) fn finish_frame(&mut self) {
        let read = &mut self.read;
        self.textures.retain(|id, _| read.contains(id));
        read.clear();
    }
}

#[cfg(test)]
mod tests {
    use mln_gfx::gpu::{Caps, ClipDepthRange};
    use mln_style::tileset::RasterEncoding;

    use super::*;
    use crate::geometry::dem_data::DEMData;
    use crate::tile::dem::Offset;

    fn context() -> Context {
        Context::new(
            Caps {
                buffer_offset_alignment: 256,
                max_texture_size: 4096,
                half_float_render_targets: false,
                clip_depth: ClipDepthRange::ZeroToOne,
                render_target_depth: None,
                clear_stencil_with_pass: false,
                cpu_rasterizer: false,
                max_line_width: 1.0,
                binds_vertex_buffers: false,
            },
            1.0,
        )
    }

    fn dem() -> DemContents {
        let image = PremultipliedImage::new(Size::new(4, 4));
        DemContents::new(DEMData::new(&image, RasterEncoding::Terrarium).unwrap())
    }

    #[test]
    fn a_dem_texture_uploads_again_once_its_border_changes() {
        let mut context = context();
        let mut textures = DemTextures::default();
        let mut dem = dem();
        let first = textures.texture(&mut context, &dem);
        assert_eq!(context.texture(&first).unwrap().size(), mln_gfx::Size::new(6, 6), "with its border");
        assert_eq!(textures.texture(&mut context, &dem), first);

        dem.backfill(&self::dem(), Offset { dx: 1, dy: 0 });
        let second = textures.texture(&mut context, &dem);
        assert_ne!(second, first);
        assert_eq!(textures.texture(&mut context, &dem), second);
    }

    #[test]
    fn a_dem_texture_goes_once_no_tile_reads_it() {
        let mut context = context();
        let mut textures = DemTextures::default();
        let (kept, dropped) = (dem(), dem());
        let kept_texture = textures.texture(&mut context, &kept);
        let dropped_texture = textures.texture(&mut context, &dropped);
        textures.finish_frame();

        assert_eq!(textures.texture(&mut context, &kept), kept_texture);
        textures.finish_frame();
        assert_eq!(textures.textures.len(), 1);
        assert_ne!(textures.texture(&mut context, &dropped), dropped_texture);
    }
}
