//! The WGSL of the hillshades' programs.

use mln_engine::draw::ground::wgsl::hillshade_shading;
use mln_gfx::shaders::source::WgslSource;

pub(crate) const HILLSHADE_PREPARE: WgslSource = WgslSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = drawable.matrix * vec4<f32>(f32(in.pos.x), f32(in.pos.y), 0.0, 1.0);

    let epsilon = vec2<f32>(1.0, 1.0) / tileProps.dimension;
    let scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    out.tex_coord = vec2<f32>(f32(in.texture_pos.x), f32(in.texture_pos.y)) / 8192.0 * scale + epsilon;
    return out;
}
",
    ),
    fragment: Some(
        r"
struct FragmentInput {
    @location(0) tex_coord: vec2<f32>,
};

// The height in meters of the DEM at `coord`, by the rule of
// `mln_engine::tile::dem`. The channels aren't rounded: the decode is linear in
// them, so where a linear sampler interpolates them, it interpolates the
// heights.
fn getElevation(coord: vec2<f32>) -> f32 {
    let rgb = textureSample(image, image_sampler, coord).rgb * 255.0;
    let unpack = tileProps.unpack;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * tileProps.unpack_scale;
}

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let epsilon = vec2<f32>(1.0, 1.0) / tileProps.dimension;
    let tileSize = tileProps.dimension.x - 2.0;

    let a = getElevation(in.tex_coord + vec2<f32>(-epsilon.x, -epsilon.y));
    let b = getElevation(in.tex_coord + vec2<f32>(0.0, -epsilon.y));
    let c = getElevation(in.tex_coord + vec2<f32>(epsilon.x, -epsilon.y));
    let d = getElevation(in.tex_coord + vec2<f32>(-epsilon.x, 0.0));
    let f = getElevation(in.tex_coord + vec2<f32>(epsilon.x, 0.0));
    let g = getElevation(in.tex_coord + vec2<f32>(-epsilon.x, epsilon.y));
    let h = getElevation(in.tex_coord + vec2<f32>(0.0, epsilon.y));
    let i = getElevation(in.tex_coord + vec2<f32>(epsilon.x, epsilon.y));

    var exaggerationFactor = 0.3;
    if (tileProps.zoom < 4.5) {
        exaggerationFactor = 0.35;
    }
    if (tileProps.zoom < 2.0) {
        exaggerationFactor = 0.4;
    }
    var exaggeration = 0.0;
    if (tileProps.zoom < 15.0) {
        exaggeration = (tileProps.zoom - 15.0) * exaggerationFactor;
    }

    let denom = pow(2.0, exaggeration + (28.2562 - tileProps.zoom));
    let deriv = vec2<f32>((c + f + f + i) - (a + d + d + g),
                          (g + h + h + i) - (a + b + b + c)) * tileSize / denom;

    let color = clamp(vec4<f32>(deriv.x / 8.0 + 0.5,
                                 deriv.y / 8.0 + 0.5,
                                 1.0,
                                 1.0),
                      vec4<f32>(0.0, 0.0, 0.0, 0.0),
                      vec4<f32>(1.0, 1.0, 1.0, 1.0));
    return color;
}
",
    ),
    prelude: None,
};

pub(crate) const HILLSHADE: WgslSource = WgslSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[globalIndex.value];
    let pos = vec2<f32>(f32(in.pos.x), f32(in.pos.y));
    out.position = projectTileWithPoles(pos, pos, projectionVector[globalIndex.value]);

    var tex = vec2<f32>(f32(in.texture_pos.x), f32(in.texture_pos.y)) / 8192.0;
    tex.y = 1.0 - tex.y;
    out.tex_coord = tex;
    return out;
}
",
    ),
    fragment: Some(concat!(
        hillshade_shading!(),
        r"
struct FragmentInput {
    @location(0) tex_coord: vec2<f32>,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let tileProps = tilePropsVector[globalIndex.value];
    let pixel = textureSample(image, image_sampler, in.tex_coord);
    // Without lights, the slopes as they were prepared, which the terrain's
    // surface shades.
    if (tileProps.num_lights == 0) {
        return pixel;
    }

    let latRange = tileProps.latrange;
    let latitude = (latRange.x - latRange.y) * in.tex_coord.y + latRange.y;
    let scaleFactor = cos(radians(latitude));

    let deriv = ((pixel.rg * 8.0) - vec2<f32>(4.0, 4.0)) / scaleFactor;
    return hillshade_color(deriv, tileProps);
}
"
    )),
    prelude: None,
};
