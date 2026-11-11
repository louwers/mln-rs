// test/gl/*.test.cpp: the gtests of parts of the C++ OpenGL backend that
// mln_gl, its port, doesn't have (all ignored, see each file).
// test/gl/bucket.test.cpp is ported in tests/bucket_test.rs.

mod context;
#[path = "enum.rs"]
mod enum_;
mod gl_functions;
mod object;
mod resource_pool;
