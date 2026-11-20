//! The textures of the DEM tiles the terrain's tiles read: each DEM's
//! bordered RGBA8 image, which the vertex stage reads texel by texel.

use std::collections::hash_map::Entry;
use std::rc::Rc;

use mln_util::hash::{HashMap, HashSet};
use mln_util::identity::SimpleIdentity;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::gfx::context::Context;
use crate::gfx::store::SharedTexture;
use crate::gfx::texture2d::Texture2D;

/// The DEM textures of a frame's terrain tiles, kept while a tile reads
/// them.
#[derive(Default)]
pub(crate) struct DemTextures {
    /// The texture of each DEM bucket a tile reads, with the generation of
    /// the bucket's border it holds.
    textures: HashMap<SimpleIdentity, (SharedTexture, u64)>,
    /// The buckets the frame's tiles read so far.
    read: HashSet<SimpleIdentity>,
    /// A transparent 1×1 texture, for the tiles that read no DEM, and for
    /// the drape of a tile that the first stack draws nothing in.
    flat: Option<SharedTexture>,
}

impl DemTextures {
    /// The texture of `bucket`'s DEM, which a tile of the frame reads: the
    /// one made before, or a new one if there was none or the bucket's
    /// border changed since (its `render_target_generation`).
    pub(crate) fn texture(&mut self, context: &mut Context, bucket: &HillshadeBucket) -> SharedTexture {
        let id = bucket.base.bucket_id;
        let generation = bucket.render_target_generation();
        self.read.insert(id);
        let (texture, _) = match self.textures.entry(id) {
            Entry::Occupied(entry) if entry.get().1 == generation => entry.into_mut(),
            entry => {
                let image = bucket.dem_data().image().clone();
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

    /// The bytes of the textures it holds that are made, in `context`.
    pub(crate) fn bytes(&self, context: &Context) -> usize {
        let made = |texture: &SharedTexture| {
            context.texture(texture).filter(|texture| texture.gpu_texture().is_some()).map_or(0, Texture2D::bytes)
        };
        self.textures.values().map(|(texture, _)| made(texture)).sum::<usize>() + self.flat.as_ref().map_or(0, made)
    }

    /// Whether it holds no texture.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.textures.is_empty() && self.flat.is_none()
    }

    /// Drops the textures of the buckets no tile of the frame read, once
    /// the frame's tiles took theirs.
    pub(crate) fn finish_frame(&mut self) {
        let read = &mut self.read;
        self.textures.retain(|id, _| read.contains(id));
        read.clear();
    }
}

#[cfg(test)]
mod tests {
    use mln_gfx::gpu::Caps;
    use mln_util::clip_depth::ClipDepthRange;
    use mln_util::tileset::RasterEncoding;

    use super::*;
    use crate::geometry::dem_data::DEMData;

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

    fn bucket() -> HillshadeBucket {
        let image = PremultipliedImage::new(Size::new(4, 4));
        HillshadeBucket::with_dem_data(DEMData::new(&image, RasterEncoding::Terrarium).unwrap())
    }

    #[test]
    fn a_dem_texture_uploads_again_once_its_border_changes() {
        let mut context = context();
        let mut textures = DemTextures::default();
        let mut bucket = bucket();
        let first = textures.texture(&mut context, &bucket);
        assert_eq!(context.texture(&first).unwrap().size(), Size::new(6, 6), "with its border");
        assert_eq!(textures.texture(&mut context, &bucket), first);

        bucket.reset_render_target_prepared();
        let second = textures.texture(&mut context, &bucket);
        assert_ne!(second, first);
        assert_eq!(textures.texture(&mut context, &bucket), second);
    }

    #[test]
    fn a_dem_texture_goes_once_no_tile_reads_it() {
        let mut context = context();
        let mut textures = DemTextures::default();
        let (kept, dropped) = (bucket(), bucket());
        let kept_texture = textures.texture(&mut context, &kept);
        let dropped_texture = textures.texture(&mut context, &dropped);
        textures.finish_frame();

        assert_eq!(textures.texture(&mut context, &kept), kept_texture);
        textures.finish_frame();
        assert_eq!(textures.textures.len(), 1);
        assert_ne!(textures.texture(&mut context, &dropped), dropped_texture);
    }
}
