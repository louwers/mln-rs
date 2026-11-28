// Ports the raster cases of test/gl/bucket.test.cpp.
use geometry::Point;
use mln_engine::layer::RasterLayoutVertex;
use mln_style::tile_id::CanonicalTileID;

use super::with_resources;
use crate::layers::quads::{StoredQuads, TileMask, mask_quads};
use crate::layers::segments::SegmentBase;

// gtest: Buckets.RasterBucket
#[test]
fn a_masks_quads_go_into_the_stores() {
    with_resources(|res| {
        let quads = StoredQuads::of_mask(res, &mask(&[(1, 0, 0), (1, 1, 1)])).expect("the mask's quads");
        assert_eq!(quads.vertices.count(), 8);

        let whole = mask(&[(0, 0, 0)]);
        assert!(StoredQuads::of_mask(res, &whole).is_none(), "the whole tile draws the tile's quad");
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

fn raster_vertex(p: (i16, i16), t: (u16, u16)) -> RasterLayoutVertex {
    RasterLayoutVertex::new(Point::new(p.0, p.1), Point::new(t.0, t.1))
}

// gtest: Buckets.RasterBucketMaskEmpty
#[test]
fn raster_bucket_mask_empty() {
    let (vertices, indices, segments) = mask_quads(&mask(&[]));
    assert_eq!(Vec::<RasterLayoutVertex>::new(), vertices);
    assert_eq!(Vec::<u16>::new(), indices);
    assert_eq!(Vec::<(usize, usize, usize, usize)>::new(), segment_tuples(&segments));
}

// gtest: Buckets.RasterBucketMaskNoChildren
#[test]
fn raster_bucket_mask_no_children() {
    // A mask of 0/0/0 doesn't produce buffers since we're instead using the
    // global shared buffers.
    let (vertices, indices, segments) = mask_quads(&mask(&[(0, 0, 0)]));
    assert_eq!(Vec::<RasterLayoutVertex>::new(), vertices);
    assert_eq!(Vec::<u16>::new(), indices);
    assert_eq!(Vec::<(usize, usize, usize, usize)>::new(), segment_tuples(&segments));
}

// gtest: Buckets.RasterBucketMaskTwoChildren
#[test]
fn raster_bucket_mask_two_children() {
    let (vertices, indices, segments) = mask_quads(&mask(&[(1, 0, 0), (1, 1, 1)]));
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
        vertices
    );

    assert_eq!(
        vec![
            // 1/0/1
            0u16, 1, 2, 1, 2, 3, //
            // 1/1/1
            4, 5, 6, 5, 6, 7,
        ],
        indices
    );

    assert_eq!(vec![(0, 0, 8, 12)], segment_tuples(&segments));
}

// gtest: Buckets.RasterBucketMaskComplex
#[test]
fn raster_bucket_mask_complex() {
    let (vertices, indices, segments) =
        mask_quads(&mask(&[(1, 0, 1), (1, 1, 0), (2, 2, 3), (2, 3, 2), (3, 6, 7), (3, 7, 6)]));
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
        vertices
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
        indices
    );

    assert_eq!(vec![(0, 0, 24, 36)], segment_tuples(&segments));
}
