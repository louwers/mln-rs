// Ports test/gl/context.test.cpp.
//
// The whole C++ file is `#if MLN_RENDER_BACKEND_OPENGL`: it tests
// gl::Context with ContextMode::Shared: custom GL drawing outside the render
// loop, then test::checkImage("test/fixtures/shared_context").
// mln_gl, the port of the OpenGL backend, draws nothing outside the render
// loop (it has no custom layers), so this stays ignored; the
// backend-independent bucket tests of test/gl are in bucket_test.rs.

// gtest: GLContextMode.Shared
#[test]
#[ignore = "mln_gl draws nothing outside the render loop, in a shared GL context or otherwise"]
fn shared() {}
