// Ports include/mln/shaders/shader_source.hpp (generated in C++)
//
// The per-backend `ShaderSource<BuiltIn, Backend::Type>` specialisations live
// with their backend (mln_webgpu::shaders).

/// This enum is used with the `ShaderSource` template to select source code for
/// the desired program and graphics back-end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltIn {
    None,
    Prelude,
    ClippingMaskProgram,
    BackgroundShader,
    BackgroundPatternShader,
    SkyShader,
    AtmosphereShader,
    CircleShader,
    CollisionBoxShader,
    CollisionCircleShader,
    DebugShader,
    FillShader,
    FillOutlineShader,
    FillPatternShader,
    FillOutlinePatternShader,
    FillOutlineTriangulatedShader,
    FillExtrusionShader,
    FillExtrusionInstancedShader,
    FillExtrusionPatternShader,
    FillExtrusionPatternInstancedShader,
    HeatmapShader,
    HeatmapTextureShader,
    HillshadePrepareShader,
    ColorReliefShader,
    HillshadeShader,
    LineShader,
    LineGradientShader,
    LinePatternShader,
    LineSDFShader,
    LineGradientSDFShader,
    RasterShader,
    SymbolIconShader,
    SymbolSDFShader,
    SymbolTextAndIconShader,
    WideVectorShader,
    GlobeDepthShader,
}
