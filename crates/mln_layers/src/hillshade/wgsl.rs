//! The programs of the `hillshade` layer type in WGSL, for `mln_webgpu`.

use mln_gfx::shaders::source::wgsl::Wgsl;

pub(super) const HILLSHADE_PREPARE_SHADER: Wgsl = Wgsl {
    vertex: r"
struct VertexInput {
    @location(0) position: vec2<i32>,
    @location(1) texcoord: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = drawable.matrix * vec4<f32>(f32(in.position.x), f32(in.position.y), 0.0, 1.0);

    let epsilon = vec2<f32>(1.0, 1.0) / tileProps.dimension;
    let scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    out.tex_coord = vec2<f32>(f32(in.texcoord.x), f32(in.texcoord.y)) / 8192.0 * scale + epsilon;
    return out;
}
",
    fragment: r"
struct FragmentInput {
    @location(0) tex_coord: vec2<f32>,
};

// The height in meters of the DEM at `coord`, by the rule of
// `mln_util::dem`. The channels aren't rounded: the decode is linear in
// them, so where a linear sampler interpolates them, it interpolates the
// heights.
fn getElevation(coord: vec2<f32>) -> f32 {
    let rgb = textureSample(image, image_sampler, coord).rgb * 255.0;
    let unpack = tileProps.unpack;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * tileProps.unpack_scale;
}

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

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
    prelude: "",
};

pub(super) const HILLSHADE_SHADER: Wgsl = Wgsl {
    vertex: r"
struct VertexInput {
    @location(0) position: vec2<i32>,
    @location(1) texcoord: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let drawable = drawableVector[drawIndex];
    let pos = vec2<f32>(f32(in.position.x), f32(in.position.y));
    out.position = projectTileWithPoles(pos, pos, projectionVector[drawIndex]);

    var tex = vec2<f32>(f32(in.texcoord.x), f32(in.texcoord.y)) / 8192.0;
    tex.y = 1.0 - tex.y;
    out.tex_coord = tex;
    return out;
}
",
    fragment: r"
const STANDARD: i32 = 0;
const COMBINED: i32 = 1;
const IGOR: i32 = 2;
const MULTIDIRECTIONAL: i32 = 3;
const BASIC: i32 = 4;

struct FragmentInput {
    @location(0) tex_coord: vec2<f32>,
};

fn get_aspect(deriv: vec2<f32>) -> f32 {
    let aspectDefault = 0.5 * PI * select(-1.0, 1.0, deriv.y > 0.0);
    return select(aspectDefault, atan2(deriv.y, -deriv.x), deriv.x != 0.0);
}

fn standard_hillshade(deriv: vec2<f32>, tileProps: HillshadeTilePropsUBO) -> vec4<f32> {
    let azimuth = props.azimuths.x + PI;
    let slope = atan(0.625 * length(deriv));
    let aspect = get_aspect(deriv);

    let intensity = tileProps.exaggeration;

    let base = 1.875 - intensity * 1.75;
    let maxValue = 0.5 * PI;
    let denom = pow(base, maxValue) - 1.0;
    let useNonLinear = abs(intensity - 0.5) > 1e-6;
    let scaledSlope = select(slope, ((pow(base, slope) - 1.0) / denom) * maxValue, useNonLinear);

    let accent = cos(scaledSlope);
    let accentColor = (1.0 - accent) * props.accent * clamp(intensity * 2.0, 0.0, 1.0);

    let shade = abs(glMod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);
    let shadeColor = mix(props.shadows[0], props.highlights[0], shade) * sin(scaledSlope) * clamp(intensity * 2.0, 0.0, 1.0);

    return accentColor * (1.0 - shadeColor.a) + shadeColor;
}

fn basic_hillshade(deriv: vec2<f32>, tileProps: HillshadeTilePropsUBO) -> vec4<f32> {
    let scaled_deriv = deriv * tileProps.exaggeration * 2.0;
    let azimuth = props.azimuths.x + PI;
    let cos_az = cos(azimuth);
    let sin_az = sin(azimuth);
    let cos_alt = cos(props.altitudes.x);
    let sin_alt = sin(props.altitudes.x);

    let cang = (sin_alt - (scaled_deriv.y * cos_az * cos_alt - scaled_deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(scaled_deriv, scaled_deriv));
    let shade = clamp(cang, 0.0, 1.0);

    return select(props.shadows[0] * (1.0 - 2.0 * shade),
                  props.highlights[0] * (2.0 * shade - 1.0),
                  shade > 0.5);
}

fn multidirectional_hillshade(deriv: vec2<f32>, tileProps: HillshadeTilePropsUBO) -> vec4<f32> {
    let scaled_deriv = deriv * tileProps.exaggeration * 2.0;
    var total_color = vec4<f32>(0.0, 0.0, 0.0, 0.0);

    let num_lights = min(tileProps.num_lights, 4);

    for (var i: i32 = 0; i < num_lights; i = i + 1) {
        let altitude = select(select(select(props.altitudes.x, props.altitudes.y, i == 1), props.altitudes.z, i == 2), props.altitudes.w, i == 3);
        let azimuth = select(select(select(props.azimuths.x, props.azimuths.y, i == 1), props.azimuths.z, i == 2), props.azimuths.w, i == 3);

        let cos_alt = cos(altitude);
        let sin_alt = sin(altitude);
        let cos_az = -cos(azimuth);
        let sin_az = -sin(azimuth);

        let cang = (sin_alt - (scaled_deriv.y * cos_az * cos_alt - scaled_deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(scaled_deriv, scaled_deriv));
        let shade = clamp(cang, 0.0, 1.0);

        if (shade > 0.5) {
            total_color = total_color + props.highlights[i] * (2.0 * shade - 1.0) / f32(num_lights);
        } else {
            total_color = total_color + props.shadows[i] * (1.0 - 2.0 * shade) / f32(num_lights);
        }
    }

    return total_color;
}

fn combined_hillshade(deriv: vec2<f32>, tileProps: HillshadeTilePropsUBO) -> vec4<f32> {
    let scaled_deriv = deriv * tileProps.exaggeration * 2.0;
    let azimuth = props.azimuths.x + PI;
    let cos_az = cos(azimuth);
    let sin_az = sin(azimuth);
    let cos_alt = cos(props.altitudes.x);
    let sin_alt = sin(props.altitudes.x);

    let cang = acos((sin_alt - (scaled_deriv.y * cos_az * cos_alt - scaled_deriv.x * sin_az * cos_alt)) / sqrt(1.0 + dot(scaled_deriv, scaled_deriv)));
    let clamped_cang = clamp(cang, 0.0, PI / 2.0);

    let shade = clamped_cang * atan(length(scaled_deriv)) * 4.0 / PI / PI;
    let highlight = (PI / 2.0 - clamped_cang) * atan(length(scaled_deriv)) * 4.0 / PI / PI;

    return props.shadows[0] * shade + props.highlights[0] * highlight;
}

fn igor_hillshade(deriv: vec2<f32>, tileProps: HillshadeTilePropsUBO) -> vec4<f32> {
    let scaled_deriv = deriv * tileProps.exaggeration * 2.0;
    let aspect = get_aspect(scaled_deriv);
    let azimuth = props.azimuths.x + PI;

    let slope_strength = atan(length(scaled_deriv)) * 2.0 / PI;
    let aspect_strength = 1.0 - abs(glMod((aspect + azimuth) / PI + 0.5, 2.0) - 1.0);

    let shadow_strength = slope_strength * aspect_strength;
    let highlight_strength = slope_strength * (1.0 - aspect_strength);

    return props.shadows[0] * shadow_strength + props.highlights[0] * highlight_strength;
}

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
#endif

    let tileProps = tilePropsVector[drawIndex];
    let pixel = textureSample(image, image_sampler, in.tex_coord);

    let latRange = tileProps.latrange;
    let latitude = (latRange.x - latRange.y) * in.tex_coord.y + latRange.y;
    let scaleFactor = cos(radians(latitude));

    let deriv = ((pixel.rg * 8.0) - vec2<f32>(4.0, 4.0)) / scaleFactor;

    if (tileProps.method == BASIC) {
        return basic_hillshade(deriv, tileProps);
    } else if (tileProps.method == COMBINED) {
        return combined_hillshade(deriv, tileProps);
    } else if (tileProps.method == IGOR) {
        return igor_hillshade(deriv, tileProps);
    } else if (tileProps.method == MULTIDIRECTIONAL) {
        return multidirectional_hillshade(deriv, tileProps);
    } else {
        return standard_hillshade(deriv, tileProps);
    }
}
",
    prelude: r"
fn radians(degrees: f32) -> f32 {
    return PI * degrees / 180.0;
}
",
};
