// Ports test/gl/gl_functions.test.cpp.
//
// The whole C++ file is `#if MLN_RENDER_BACKEND_OPENGL`: it tests that every
// OpenGL ES 2.0/3.0 function pointer in platform/gl_functions is loaded.
// mln_gl, the port of the OpenGL backend, calls OpenGL through the glow
// crate, which loads the functions, so this stays ignored; the
// backend-independent bucket tests of test/gl are in bucket_test.rs.

// gtest: GLFunctions.OpenGLES
#[test]
#[ignore = "mln_gl calls OpenGL through glow, which loads the functions"]
fn open_gles() {}
