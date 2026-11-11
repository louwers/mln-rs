// Ports test/gl/enum.test.cpp.
//
// The whole C++ file is `#if MLN_RENDER_BACKEND_OPENGL`: it tests the
// gl::Enum<T> conversions between gfx types and GLenum values.
// mln_gl, the port of the OpenGL backend, converts gfx types to GLenum values
// only (its enums.rs), so these stay ignored; the backend-independent bucket
// tests of test/gl are in bucket_test.rs.

// gtest: GL.DrawModeType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn draw_mode_type() {}

// gtest: GL.ColorBlendEquationType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn color_blend_equation_type() {}

// gtest: GL.ColorBlendFactorType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn color_blend_factor_type() {}

// gtest: GL.DepthFunctionType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn depth_function_type() {}

// gtest: GL.DepthMaskType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn depth_mask_type() {}

// gtest: GL.StencilFunctionType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn stencil_function_type() {}

// gtest: GL.StencilOpType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn stencil_op_type() {}

// gtest: GL.CullFaceSideType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn cull_face_side_type() {}

// gtest: GL.CullFaceWindingType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn cull_face_winding_type() {}

// gtest: GL.BufferUsageType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn buffer_usage_type() {}

// gtest: GL.TexturePixelType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn texture_pixel_type() {}

// gtest: GL.TextureChannelDataType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn texture_channel_data_type() {}

// gtest: GL.RenderbufferPixelType
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn renderbuffer_pixel_type() {}

// gtest: GL.sizedFor
#[test]
#[ignore = "mln_gl has only the conversions to GLenum, not back (gl::Enum<T>::from)"]
fn sized_for() {}
