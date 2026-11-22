use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_tile_id::CanonicalTileID;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use super::with_context;
use crate::bucket_geometry::{BucketGeometry, GpuVec};
use crate::gfx::buffer_store::StoredBuffer;
use crate::gfx::context::Resources;
use crate::handles::ImageVertex;
use crate::tile::raster::RasterContents;
use crate::tile_mask::TileMask;

// gtest: Buckets.RasterBucket
#[test]
fn a_raster_buckets_upload_moves_its_quads_into_the_stores_once_per_mask() {
    let rgba = PremultipliedImage::new(Size::new(1, 1));
    let mut bucket = RasterContents::new(rgba);
    assert!(bucket.has_data());
    bucket.set_mask(mask(&[(1, 0, 0), (1, 1, 1)]));

    with_context(|_, context| {
        bucket.upload(context);
        let geometry = bucket.geometry();
        let quads = geometry.quads().expect("the mask's quads");
        assert_eq!(context.stored_buffer(&quads.vertices).expect("stored").count(), 8);
        assert!(geometry.vertices().is_empty(), "the upload moves the vertices");
        assert_eq!(quads.vertices.count(), 8);

        bucket.set_mask(mask(&[(1, 0, 0)]));
        bucket.upload(context);
        let quads = bucket.geometry().quads().expect("the new mask's quads");
        assert_eq!(context.stored_buffer(&quads.vertices).expect("stored").count(), 4);

        bucket.set_mask(mask(&[(0, 0, 0)]));
        bucket.upload(context);
        assert!(bucket.geometry().quads().is_none(), "the whole tile draws the tile's quad");
    });
}

/// A segment's offsets and lengths, which the tests compare.
fn segment_tuples(segments: &[SegmentBase]) -> Vec<(usize, usize, usize, usize)> {
    segments.iter().map(|s| (s.vertex_offset, s.index_offset, s.vertex_length, s.index_length)).collect()
}

fn mask(ids: &[(u8, u32, u32)]) -> TileMask {
    let mut mask = TileMask::new();
    for &(z, x, y) in ids {
        mask.insert(CanonicalTileID::new(z, x, y));
    }
    mask
}

fn raster_vertex(p: (i16, i16), t: (u16, u16)) -> ImageVertex {
    ImageVertex::new(Point::new(p.0, p.1), Point::new(t.0, t.1))
}

// gtest: Buckets.RasterBucketMaskEmpty
#[test]
fn raster_bucket_mask_empty() {
    let mut bucket = RasterContents::with_shared_image(None);
    bucket.set_mask(mask(&[]));
    let geometry = bucket.geometry();
    assert_eq!(Vec::<ImageVertex>::new(), geometry.vertices());
    assert_eq!(Vec::<u16>::new(), geometry.indices());
    assert_eq!(Vec::<(usize, usize, usize, usize)>::new(), segment_tuples(geometry.segments()));
}

// gtest: Buckets.RasterBucketMaskNoChildren
#[test]
fn raster_bucket_mask_no_children() {
    let mut bucket = RasterContents::with_shared_image(None);
    bucket.set_mask(mask(&[(0, 0, 0)]));

    // A mask of 0/0/0 doesn't produce buffers since we're instead using the
    // global shared buffers.
    let geometry = bucket.geometry();
    assert_eq!(Vec::<ImageVertex>::new(), geometry.vertices());
    assert_eq!(Vec::<u16>::new(), geometry.indices());
    assert_eq!(Vec::<(usize, usize, usize, usize)>::new(), segment_tuples(geometry.segments()));
}

// gtest: Buckets.RasterBucketMaskTwoChildren
#[test]
fn raster_bucket_mask_two_children() {
    let mut bucket = RasterContents::with_shared_image(None);
    bucket.set_mask(mask(&[(1, 0, 0), (1, 1, 1)]));

    let geometry = bucket.geometry();
    assert_eq!(
        vec![
            // 1/0/1
            raster_vertex((0, 0), (0, 0)),
            raster_vertex((4096, 0), (4096, 0)),
            raster_vertex((0, 4096), (0, 4096)),
            raster_vertex((4096, 4096), (4096, 4096)),
            // 1/1/1
            raster_vertex((4096, 4096), (4096, 4096)),
            raster_vertex((8192, 4096), (8192, 4096)),
            raster_vertex((4096, 8192), (4096, 8192)),
            raster_vertex((8192, 8192), (8192, 8192)),
        ],
        geometry.vertices()
    );

    assert_eq!(
        vec![
            // 1/0/1
            0u16, 1, 2, 1, 2, 3, //
            // 1/1/1
            4, 5, 6, 5, 6, 7,
        ],
        geometry.indices()
    );

    assert_eq!(vec![(0, 0, 8, 12)], segment_tuples(geometry.segments()));
}

// gtest: Buckets.RasterBucketMaskComplex
#[test]
fn raster_bucket_mask_complex() {
    let mut bucket = RasterContents::with_shared_image(None);
    bucket.set_mask(mask(&[(1, 0, 1), (1, 1, 0), (2, 2, 3), (2, 3, 2), (3, 6, 7), (3, 7, 6)]));

    let geometry = bucket.geometry();
    assert_eq!(
        vec![
            // 1/0/1
            raster_vertex((0, 4096), (0, 4096)),
            raster_vertex((4096, 4096), (4096, 4096)),
            raster_vertex((0, 8192), (0, 8192)),
            raster_vertex((4096, 8192), (4096, 8192)),
            // 1/1/0
            raster_vertex((4096, 0), (4096, 0)),
            raster_vertex((8192, 0), (8192, 0)),
            raster_vertex((4096, 4096), (4096, 4096)),
            raster_vertex((8192, 4096), (8192, 4096)),
            // 2/2/3
            raster_vertex((4096, 6144), (4096, 6144)),
            raster_vertex((6144, 6144), (6144, 6144)),
            raster_vertex((4096, 8192), (4096, 8192)),
            raster_vertex((6144, 8192), (6144, 8192)),
            // 2/3/2
            raster_vertex((6144, 4096), (6144, 4096)),
            raster_vertex((8192, 4096), (8192, 4096)),
            raster_vertex((6144, 6144), (6144, 6144)),
            raster_vertex((8192, 6144), (8192, 6144)),
            // 3/6/7
            raster_vertex((6144, 7168), (6144, 7168)),
            raster_vertex((7168, 7168), (7168, 7168)),
            raster_vertex((6144, 8192), (6144, 8192)),
            raster_vertex((7168, 8192), (7168, 8192)),
            // 3/7/6
            raster_vertex((7168, 6144), (7168, 6144)),
            raster_vertex((8192, 6144), (8192, 6144)),
            raster_vertex((7168, 7168), (7168, 7168)),
            raster_vertex((8192, 7168), (8192, 7168)),
        ],
        geometry.vertices()
    );

    assert_eq!(
        vec![
            // 1/0/1
            0u16, 1, 2, 1, 2, 3, //
            // 1/1/0
            4, 5, 6, 5, 6, 7, //
            // 2/2/3
            8, 9, 10, 9, 10, 11, //
            // 2/3/2
            12, 13, 14, 13, 14, 15, //
            // 3/6/7
            16, 17, 18, 17, 18, 19, //
            // 3/7/6
            20, 21, 22, 21, 22, 23,
        ],
        geometry.indices()
    );

    assert_eq!(vec![(0, 0, 24, 36)], segment_tuples(geometry.segments()));
}

// Stored vertices free their CPU copy once they are in a buffer.
#[test]
fn stored_vertices_free_their_data_once_they_have_a_buffer() {
    with_context(|gpu, context| {
        let vertices = context.add_buffer(vec![[1i16, 2], [3, 4]]);
        assert!(
            context.stored_buffer(&vertices).is_some_and(|stored| stored.needs_upload() && stored.bytes().len() == 8)
        );

        context.upload_buffer(gpu, &vertices).expect("uploads").expect("a buffer");
        assert!(
            context.stored_buffer(&vertices).is_some_and(|stored| !stored.needs_upload() && stored.bytes().is_empty())
        );
        assert_eq!(vertices.count(), 2);
    });
}

// A kept vector writes its buffer again only once it changed, as placement
// changes it through `get_mut`.
#[test]
fn a_kept_vector_uploads_again_only_once_it_changed() {
    with_context(|gpu, context| {
        let mut vec = GpuVec::new(vec![1.0f32, 2.0]);
        let mut geometry = BucketGeometry::default();
        geometry.upload(&mut vec, true, &[], context);
        let entry = geometry.buffer(&vec).expect("a buffer").entry.clone();
        context.upload_buffer(gpu, &entry).expect("uploads").expect("a buffer");
        let needs_upload = |context: &Resources| context.stored_buffer(&entry).is_some_and(StoredBuffer::needs_upload);

        geometry.upload(&mut vec, true, &[], context);
        assert!(!needs_upload(context), "unchanged");

        vec.get_mut()[0] = 3.0;
        geometry.upload(&mut vec, true, &[], context);
        assert!(
            context.stored_buffer(&entry).is_some_and(|s| s.bytes() == bytemuck::cast_slice::<f32, u8>(&[3.0, 2.0]))
        );
        context.upload_buffer(gpu, &entry).expect("uploads").expect("a buffer");
        assert!(!needs_upload(context), "written");
        assert_eq!(vec.get(), [3.0, 2.0], "the bucket keeps its data");
    });
}

// What draws share stays in the context's stores while a handle to it lasts,
// and goes once the last one drops and the context destroys what the
// renderer released.
#[test]
fn store_entries_last_as_long_as_their_handles() {
    with_context(|gpu, context| {
        let vertices = context.add_buffer(vec![[1i16, 2]]);
        let held = vertices.clone();
        let texture = context.add_texture();
        let draw_texture = texture.clone();
        drop(vertices);
        drop(texture);
        context.destroy_released(gpu);
        assert!(context.stored_buffer(&held).is_some());
        assert!(context.stored_texture(&draw_texture).is_some());

        let key = held.key();
        drop(held);
        drop(draw_texture);
        context.destroy_released(gpu);
        assert!(!context.has_buffer(key));
    });
}
