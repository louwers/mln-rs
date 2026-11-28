use super::with_context;
use crate::draw::indexes::DrawIndexes;
use crate::draw::range::DrawRange;
use crate::layer::RasterLayoutVertex;

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
            ranges: vec![DrawRange::indexes(0)],
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
