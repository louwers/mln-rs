use std::sync::Arc;

use geometry::Point;
use mln_style::collections::VecMap;
use mln_style::layers::symbol_layer_properties::SymbolLayoutProperties;
use mln_style::property::value::PropertyValue;
use mln_style::tile_id::CanonicalTileID;

use super::with_context;
use crate::draw::context::Context;
use crate::draw::indexes::DrawIndexes;
use crate::draw::quads::{RasterLayoutVertex, StoredQuads, mask_quads};
use crate::draw::segment::SegmentBase;
use crate::layer::bucket::{Bucket, Upload};
use crate::layer::bucket_geometry::BucketGeometry;
use crate::symbol::bucket::{SymbolBucket, SymbolBucketStyle, SymbolLayoutFlags};
use crate::symbol::instance::SymbolInstancePools;
use crate::tile_store::raster::Masking;
use crate::tile_store::tile_mask::TileMask;

/// Uploads `bucket`, keeping its geometry in `geometry`, through `context`.
fn upload(bucket: &mut impl Bucket, geometry: &mut BucketGeometry, context: &mut Context) {
    bucket.upload(&mut Upload::geometry(geometry, &[], context));
}

// gtest: Buckets.SymbolBucket
#[test]
fn symbol_bucket() {
    let layout = Arc::new(SymbolLayoutProperties::PossiblyEvaluated::default());
    let (instances, pools) = (Vec::new(), SymbolInstancePools::default());
    let mut bucket = SymbolBucket::new(
        SymbolBucketStyle {
            layout,
            paint_properties: &VecMap::new(),
            text_size: &PropertyValue::Constant(16.0),
            icon_size: &PropertyValue::Constant(1.0),
            zoom: 0.0,
            tile_pixel_ratio: 1.0,
            leader_id: Arc::from("test"),
        },
        SymbolLayoutFlags::default(),
        instances,
        pools,
        Vec::new(),
    );
    assert!(!bucket.has_icon_data());
    assert!(!bucket.has_sdf_icon_data());
    assert!(!bucket.has_text_data());
    assert!(!bucket.has_data());

    bucket.state.text.segments.push(SegmentBase::new(0, 0));
    assert!(bucket.has_text_data());
    assert!(bucket.has_data());

    with_context(|_, context| {
        upload(&mut bucket, &mut BucketGeometry::default(), context);
    });
}

// gtest: Buckets.RasterBucket
#[test]
fn a_masks_quads_go_into_the_stores() {
    let mut masking = Masking::default();
    with_context(|_, context| {
        assert!(StoredQuads::of_mask(context, &masking).is_none(), "an unmasked tile draws the tile's quad");

        masking.set(mask(&[(1, 0, 0), (1, 1, 1)]));
        let quads = StoredQuads::of_mask(context, &masking).expect("the mask's quads");
        assert_eq!(context.vertices(&quads.vertices).expect("stored").count(), 8);

        masking.set(mask(&[(0, 0, 0)]));
        assert!(StoredQuads::of_mask(context, &masking).is_none(), "the whole tile draws the tile's quad");
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
    use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DrawMode};

    use crate::draw::objects::{DrawGeometry, Mesh};
    use crate::draw::vertex_attribute::{SharedAttribute, Uniforms, VertexAttributeArray};

    with_context(|gpu, context| {
        let variant = mln_gfx::projection_variant::ProjectionVariant::Mercator;
        let shader = context.shader_key(&crate::shaders::programs::CLIPPING_MASK, Uniforms::default(), variant);
        let vertex = |x: i16| RasterLayoutVertex { a1: [x, x], a2: [0, 0] };
        let typed = context.add_dynamic_vertices(&[vertex(0), vertex(1), vertex(2)]);
        let stride = size_of::<RasterLayoutVertex>() as u32;
        let mut attrs = VertexAttributeArray::new();
        attrs.set_shared("pos", SharedAttribute::new(typed.shared().clone(), AttributeDataType::Short2, 0, stride));
        let geometry = DrawGeometry {
            vertices: attrs,
            vertex_count: 3,
            mode: DrawMode::Triangles(),
            indexes: DrawIndexes::new(vec![0, 1, 2]),
            segments: vec![SegmentBase::new(0, 0)],
            instances: None,
        };
        let mesh = Mesh::new(
            context,
            shader,
            ColorMode::alpha_blended(),
            CullFaceMode::disabled(),
            geometry,
            Default::default(),
        );
        mesh.upload(gpu, context);
        let uploaded = context.rendering_stats().buffer_updates;

        for x in 3..6 {
            assert!(context.update_vertices(&typed, &[vertex(x), vertex(x), vertex(x)]));
            mesh.upload(gpu, context);
            assert!(context.vertices(typed.shared()).is_some_and(|stored| !stored.needs_upload()), "{x}");
            assert_eq!(context.rendering_stats().buffer_updates, uploaded + x as usize - 2, "{x}");
        }
        // Without an update, an upload writes nothing.
        mesh.upload(gpu, context);
        assert_eq!(context.rendering_stats().buffer_updates, uploaded + 3);
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
