// Ports test/gl/resource_pool.test.cpp.
//
// The whole C++ file is `#if MLN_RENDER_BACKEND_OPENGL`: it tests
// gl::Context::getTexturePool(): allocation, reuse and release of GL
// textures, with context.empty().
// mln_gl, the port of the OpenGL backend, has no texture pool (each texture
// is a GL texture of its own), so this stays ignored; the
// backend-independent bucket tests of test/gl are in bucket_test.rs.

// gtest: ResourcePool.TexturePool
#[test]
#[ignore = "mln_gl has no texture pool"]
fn texture_pool() {}
