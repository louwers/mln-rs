use geometry::Point;
use mln_gfx::shaders::segment::SegmentBase;
use mln_tile_id::CanonicalTileID;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use super::with_context;
use crate::gfx::indexes::DrawIndexes;
use crate::tile::raster::{RasterContents, RasterLayoutVertex};
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
        assert_eq!(context.vertices(&quads.vertices).expect("stored").count(), 8);
        assert!(geometry.vertices().is_empty(), "the upload moves the vertices");
        assert_eq!(quads.vertices.count(), 8);

        bucket.set_mask(mask(&[(1, 0, 0)]));
        bucket.upload(context);
        let quads = bucket.geometry().quads().expect("the new mask's quads");
        assert_eq!(context.vertices(&quads.vertices).expect("stored").count(), 4);

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

fn raster_vertex(p: (i16, i16), t: (u16, u16)) -> RasterLayoutVertex {
    RasterLayoutVertex::new(Point::new(p.0, p.1), Point::new(t.0, t.1))
}

// gtest: Buckets.RasterBucketMaskEmpty
#[test]
fn raster_bucket_mask_empty() {
    let mut bucket = RasterContents::with_shared_image(None);
    bucket.set_mask(mask(&[]));
    let geometry = bucket.geometry();
    assert_eq!(Vec::<RasterLayoutVertex>::new(), geometry.vertices());
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
    assert_eq!(Vec::<RasterLayoutVertex>::new(), geometry.vertices());
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
        let vertices = context.add_vertices(vec![[1i16, 2], [3, 4]]);
        assert!(context.vertices(&vertices).is_some_and(|stored| stored.needs_upload() && stored.bytes().len() == 8));

        context.upload_vertices(gpu, &vertices).expect("uploads").expect("a buffer");
        assert!(context.vertices(&vertices).is_some_and(|stored| !stored.needs_upload() && stored.bytes().is_empty()));
        assert_eq!(vertices.count(), 2);
    });
}

// Dynamic vertices' updates fill the storage of the bytes they were made of,
// which their uploads keep.
#[test]
fn dynamic_vertices_keep_their_storage_across_uploads() {
    with_context(|gpu, context| {
        let typed = context.add_dynamic_vertices(&[[1i16, 2], [3, 4]]);
        let storage = context.vertices(typed.shared()).map(|stored| stored.bytes().as_ptr());
        for x in 5..8 {
            context.upload_vertices(gpu, typed.shared()).expect("uploads").expect("a buffer");
            assert!(context.update_vertices(&typed, &[[x, x], [x, x]]));
            assert_eq!(context.vertices(typed.shared()).map(|stored| stored.bytes().as_ptr()), storage, "{x}");
        }
    });
}

// The holder of typed vertices replaces them with as many vertices of the
// type, which the next upload writes over their buffer, counted as a buffer
// update, and which the draws that bind them bind again.
#[test]
fn typed_vertices_take_updates_of_as_many_vertices() {
    with_context(|gpu, context| {
        let typed = context.add_dynamic_vertices(&[[1i16, 2], [3, 4]]);
        let vertices = typed.shared().clone();
        let buffer = context.upload_vertices(gpu, &vertices).expect("uploads").expect("a buffer");
        assert!(context.vertices(&vertices).is_some_and(|stored| !stored.needs_upload()));

        assert!(!context.update_vertices(&typed, &[[5, 6]]), "another count");
        assert!(context.update_vertices(&typed, &[[5, 6], [7, 8]]));
        assert!(context.vertices(&vertices).is_some_and(|stored| stored.bytes()
            == bytemuck::cast_slice::<i16, u8>(&[5, 6, 7, 8])
            && stored.needs_upload()));
        let before = context.rendering_stats().clone();
        let rewritten = context.upload_vertices(gpu, &vertices).expect("uploads").expect("a buffer");
        assert_eq!(buffer, rewritten);
        let after = context.rendering_stats().clone();
        assert_eq!(after.vertex_update_bytes, before.vertex_update_bytes + 8);
        assert_eq!(after.buffer_updates, before.buffer_updates + 1);
        assert_eq!(after.num_vertex_buffers, before.num_vertex_buffers);
        assert!(context.vertices(&vertices).is_some_and(|stored| !stored.needs_upload()));

        // An upload without an update writes nothing.
        context.upload_vertices(gpu, &vertices).expect("uploads");
        assert_eq!(context.rendering_stats().buffer_updates, after.buffer_updates);
        assert_eq!(vertices.count(), 2);
    });
}

// A draw that binds vertices writes their update in its next upload, however
// soon after its last upload the update comes.
#[test]
fn a_draw_writes_an_update_of_the_vertices_it_binds() {
    use mln_gfx::shaders::shader_defines::ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE;
    use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};

    use crate::draw_objects::{DrawGeometry, DrawObjects};
    use crate::gfx::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};

    with_context(|gpu, context| {
        let variant = mln_gfx::projection_variant::ProjectionVariant::Mercator;
        let shader = context.shader_key(&mln_gfx::shaders::programs::CLIPPING_MASK, Uniforms::default(), variant);
        let vertex = |x: i16| [x, x];
        let typed = context.add_dynamic_vertices(&[vertex(0), vertex(1), vertex(2)]);
        let stride = size_of::<[i16; 2]>() as u32;
        let mut attrs = VertexAttributeArray::new();
        attrs.set_shared(
            ID_CLIPPING_MASK_POS_VERTEX_ATTRIBUTE,
            SharedAttribute::new(typed.shared().clone(), AttributeDataType::Short2, 0, stride),
        );
        let geometry = DrawGeometry {
            vertices: attrs,
            vertex_count: 3,
            mode: DrawMode::Triangles(),
            indexes: DrawIndexes::new(vec![0, 1, 2]),
            segments: vec![SegmentBase::new(0, 0)],
            instances: None,
        };
        let mut objects = DrawObjects::new(
            shader,
            ColorMode::alpha_blended(),
            CullFaceMode::disabled(),
            geometry,
            Default::default(),
        );
        objects.upload(gpu, context);
        let uploaded = context.rendering_stats().buffer_updates;

        for x in 3..6 {
            assert!(context.update_vertices(&typed, &[vertex(x), vertex(x), vertex(x)]));
            objects.upload(gpu, context);
            assert!(context.vertices(typed.shared()).is_some_and(|stored| !stored.needs_upload()), "{x}");
            assert_eq!(context.rendering_stats().buffer_updates, uploaded + x as usize - 2, "{x}");
        }
        // Without an update, an upload writes nothing.
        objects.upload(gpu, context);
        assert_eq!(context.rendering_stats().buffer_updates, uploaded + 3);
    });
}

// A draw binds the buffer of the vertices it reads as a storage buffer as a
// buffer of its own, which its uploads write the vertices' updates to.
#[test]
fn a_draw_binds_the_vertices_it_reads_as_a_storage_buffer() {
    use mln_gfx::shaders::shader_defines::ID_FILL_EXTRUSION_WALL_VERTICES as ID;
    use mln_gfx::{ColorMode, CullFaceMode, DrawMode};

    use crate::draw_objects::{DrawGeometry, DrawObjects};
    use crate::gfx::vertex_attribute::{Uniforms, VertexAttributeArray};

    with_context(|gpu, context| {
        let variant = mln_gfx::projection_variant::ProjectionVariant::Mercator;
        // The draw binds what it's given to read, whatever its program: the
        // engine's clipping mask stands in for the pulled fill extrusions.
        let shader = context.shader_key(&mln_gfx::shaders::programs::CLIPPING_MASK, Uniforms::default(), variant);
        let typed = context.add_dynamic_vertices(&[[0u32; 2], [1; 2]]);
        let geometry = DrawGeometry {
            vertices: VertexAttributeArray::new(),
            vertex_count: 4,
            mode: DrawMode::Triangles(),
            indexes: DrawIndexes::new(vec![0, 2, 1, 1, 2, 3]),
            segments: vec![SegmentBase::new(0, 0)],
            instances: None,
        };
        let mut objects = DrawObjects::new(
            shader,
            ColorMode::alpha_blended(),
            CullFaceMode::back_ccw(),
            geometry,
            Default::default(),
        );
        objects.read_as_storage(ID, Some(typed.shared().clone()));
        assert!(objects.storage_buffer(ID).is_none(), "bound before the upload");

        objects.upload(gpu, context);
        let buffer = context.upload_vertices(gpu, typed.shared()).expect("uploads");
        assert!(buffer.is_some() && objects.storage_buffer(ID) == buffer);
        let uploaded = context.rendering_stats().buffer_updates;

        // An update goes to the same buffer.
        assert!(context.update_vertices(&typed, &[[2; 2], [3; 2]]));
        objects.upload(gpu, context);
        assert_eq!(context.rendering_stats().buffer_updates, uploaded + 1);
        assert_eq!(objects.storage_buffer(ID), buffer);

        objects.read_as_storage(ID, None);
        objects.upload(gpu, context);
        assert!(objects.storage_buffer(ID).is_none());
    });
}

// What draws share stays in the context's stores while a handle to it lasts,
// and goes once the last one drops and the context destroys what the
// renderer released.
#[test]
fn store_entries_last_as_long_as_their_handles() {
    with_context(|gpu, context| {
        let vertices = context.add_vertices(vec![[1i16, 2]]);
        let held = vertices.clone();
        let texture = context.add_texture();
        let draw_texture = texture.clone();
        drop(vertices);
        drop(texture);
        context.destroy_released(gpu);
        assert!(context.vertices(&held).is_some());
        assert!(context.texture(&draw_texture).is_some());

        let key = held.key();
        drop(held);
        drop(draw_texture);
        context.destroy_released(gpu);
        assert!(!context.has_vertices(key));
    });
}
