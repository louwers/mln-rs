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
    FillOutlineInstancedShader,
    FillOutlinePatternInstancedShader,
    /// Port-only: the outlines of `FillOutlineInstancedShader` as quads
    /// without vertex attributes. A vertex reads its segment's ends and
    /// paint values from storage buffers, by its index.
    FillOutlinePulledShader,
    FillOutlineTriangulatedShader,
    FillExtrusionShader,
    FillExtrusionInstancedShader,
    /// Port-only: the walls of `FillExtrusionInstancedShader` with every
    /// attribute per vertex, for walls whose quads are in the vertex buffer.
    FillExtrusionWallShader,
    /// Port-only: the walls of `FillExtrusionInstancedShader` as quads
    /// without vertex attributes. A vertex reads its wall's outline vertices
    /// and paint values from storage buffers, by its index.
    FillExtrusionPulledShader,
    FillExtrusionPatternShader,
    FillExtrusionPatternInstancedShader,
    /// Port-only: `FillExtrusionWallShader` of the pattern walls.
    FillExtrusionPatternWallShader,
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
    /// Port-only: a layer drawn into a texture, composited onto the frame.
    LayerCompositeShader,
    /// Port-only, for the backend conformance cases: quads whose vertex
    /// stage reads their color from a texture, by texel. The renderer
    /// doesn't draw with it.
    VertexTexelShader,
    /// The terrain's surface: a grid over a tile, raised by its DEM.
    TerrainShader,
    /// Port-only, for the backend conformance cases: probes whose vertex
    /// stage reads a height by the terrain's height rule, and whose fragment
    /// stage writes its bits. The renderer doesn't draw with it.
    TerrainProbeShader,
}
