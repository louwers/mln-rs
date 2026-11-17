// What the dashed line programs share: their uniform blocks, where a vertex
// samples its dash patterns, and how a fragment's coverage of a dash fades.

struct LineSDFDrawableUBO {
    matrix: mat4x4<f32>,
    // The scale and row of the dash patterns at the zoom levels the dashes
    // cross-fade between.
    patternscale_a: vec2<f32>,
    patternscale_b: vec2<f32>,
    tex_y_a: f32,
    tex_y_b: f32,
    ratio: f32,
    // Each data-driven property's interpolation between its zoom stops.
    color_t: f32,
    blur_t: f32,
    opacity_t: f32,
    gapwidth_t: f32,
    offset_t: f32,
    width_t: f32,
    floorwidth_t: f32,
    // The height of the dash atlas, where the dash array is data-driven.
    atlas_height: f32,
    pad2: f32,
};

struct LineSDFTilePropsUBO {
    sdfgamma: f32,
    // How far the cross-fade from the first pattern to the second is.
    mix: f32,
    pad1: f32,
    pad2: f32,
    // To the size of any line program's element (see `line`).
    @if(!gl) pad3: vec4<f32>,
    @if(!gl) pad4: vec4<f32>,
    @if(!gl) pad5: vec4<f32>,
};

// ID_LINE_DRAWABLE_UBO: one element for each drawable of the layer.
@group(1) @binding(2) var<storage, read> drawableVector: array<LineSDFDrawableUBO>;
// ID_LINE_TILE_PROPS_UBO: one element for each drawable of the layer.
@group(1) @binding(3) var<storage, read> tilePropsVector: array<LineSDFTilePropsUBO>;

// The drawable's dash patterns scaled by a data-driven dash array's own:
// `dasharray` is the pattern's middle row, row span and length in the atlas.
fn with_dasharray(drawable: LineSDFDrawableUBO, dasharray: vec4<f32>) -> LineSDFDrawableUBO {
    var out = drawable;
    out.patternscale_a = vec2<f32>(drawable.patternscale_a.x / dasharray.z, -dasharray.y / 2.0 / drawable.atlas_height);
    out.patternscale_b = vec2<f32>(drawable.patternscale_b.x / dasharray.z, out.patternscale_a.y);
    out.tex_y_a = (dasharray.x + 0.5) / drawable.atlas_height;
    out.tex_y_b = out.tex_y_a;
    return out;
}

// Where a vertex samples the dash patterns: the two zoom levels' positions in
// the dash atlas.
struct DashPositions {
    tex_a: vec2<f32>,
    tex_b: vec2<f32>,
};

// The positions of a vertex `linesofar` along the line, on the side of
// `normal_y`, of a line `floorwidth` wide at the integer zoom.
fn dash_positions(linesofar: f32, normal_y: f32, floorwidth: f32, drawable: LineSDFDrawableUBO) -> DashPositions {
    var out: DashPositions;
    out.tex_a = vec2<f32>(linesofar * drawable.patternscale_a.x / floorwidth,
                          normal_y * drawable.patternscale_a.y + drawable.tex_y_a);
    out.tex_b = vec2<f32>(linesofar * drawable.patternscale_b.x / floorwidth,
                          normal_y * drawable.patternscale_b.y + drawable.tex_y_b);
    return out;
}

// The coverage of a dash at the distance field values `dist_a` and `dist_b`
// sampled from the zoom levels' patterns, for a line `floorwidth` wide at the
// integer zoom, with a data-driven dash array's length `dash_width` (1 where
// it is uniform).
fn dash_alpha(dist_a: f32, dist_b: f32, floorwidth: f32, dash_width: f32, tile: LineSDFTilePropsUBO) -> f32 {
    let sdfdist = mix(dist_a, dist_b, tile.mix);
    let sdfgamma = tile.sdfgamma / dash_width;
    return smoothstep(0.5 - sdfgamma / floorwidth, 0.5 + sdfgamma / floorwidth, sdfdist);
}
