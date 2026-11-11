// Ports test/gl/object.test.cpp.
//
// The whole C++ file is `#if MLN_RENDER_BACKEND_OPENGL`: it tests
// gl::State<T> (value caching, dirty tracking) and
// gl::UniqueTexture/UniqueBuffer lifetimes on a gl::Context.
// mln_gl, the port of the OpenGL backend, keeps the GL state in one struct
// (its state.rs) and its objects without the unique wrappers, so these stay
// ignored; the backend-independent bucket tests of test/gl are in
// bucket_test.rs.

// gtest: GLObject.Value
#[test]
#[ignore = "mln_gl has no gl::State<T> or gl::Unique* objects (its state.rs and context.rs)"]
fn value() {}

// gtest: GLObject.Store
#[test]
#[ignore = "mln_gl has no gl::State<T> or gl::Unique* objects (its state.rs and context.rs)"]
fn store() {}
