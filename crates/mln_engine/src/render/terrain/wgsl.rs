//! The WGSL of the terrain's programs, and the snippets of the ground that
//! other programs include.

use mln_gfx::shaders::source::WgslSource;

/// What the terrain variant of a program that hides behind the ground in
/// front of it reads of the ground's depth, within `#ifdef TERRAIN` of its
/// vertex stage: whether that ground hides a point (`ground_visibility`).
#[doc(hidden)]
#[macro_export]
macro_rules! terrain_wgsl_ground_visibility {
    () => {
        r"
#ifdef TERRAIN
// GL JS's `depthOpacity`: 1 where a point `w` from the eye, at `ndc`, is in
// front of the ground drawn there, fading to 0 behind it over GL JS's ramp
// of its NDC depth, which `depth_factor` scales; 1 where no ground is drawn
// there or `ndc` is outside the view.
fn ground_depth_opacity(ndc: vec2<f32>, w: f32, depth_factor: f32) -> f32 {
    if (any(abs(ndc) > vec2<f32>(1.0))) {
        return 1.0;
    }
    let size = vec2<i32>(textureDimensions(ground_depth));
    // The texels' rows run from the top.
    let pos = (vec2<f32>(ndc.x, -ndc.y) * 0.5 + 0.5) * vec2<f32>(size);
    let texel = clamp(vec2<i32>(floor(pos)), vec2<i32>(0), size - 1);
    let bytes = vec4<u32>(round(textureLoad(ground_depth, texel, 0) * 255.0));
    let ground = bitcast<f32>(bytes.x | (bytes.y << 8u) | (bytes.z << 16u) | (bytes.w << 24u));
    if (!(ground > 0.0)) {
        return 1.0;
    }
    let d = depth_factor * (1.0 / w - 1.0 / ground) + 0.0001;
    return 1.0 - clamp(-500.0 * d, 0.0, 1.0);
}

// GL JS's `calculate_visibility` of the point at `clip`: 1 where the ground
// drawn in front doesn't hide it, 0 where it does, averaged with the point
// 0.01 NDC above it where it is partly hidden. The ramp's scale is GL JS's:
// `2n` of a near plane `n` a fiftieth of the view's height, in pixels, a
// `GlobalPaintParamsUBO`'s `world_size.y / pixel_ratio`.
fn ground_visibility(clip: vec4<f32>, view_height: f32) -> f32 {
    if (clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = clip.xy / clip.w;
    let depth_factor = view_height / 25.0;
    let visibility = ground_depth_opacity(ndc, clip.w, depth_factor);
    if (visibility > 0.95) {
        return 1.0;
    }
    return (visibility + ground_depth_opacity(ndc + vec2<f32>(0.0, 0.01), clip.w, depth_factor)) / 2.0;
}
#endif
"
    };
}
pub(crate) use crate::terrain_wgsl_ground_height as ground_height;

/// The height rule of `crate::tile::dem`, over the DEM tile a draw reads: its
/// RGBA8 texels behind a one-texel border, decoded by a `TerrainSurfaceUBO`.
#[doc(hidden)]
#[macro_export]
macro_rules! terrain_wgsl_ground_height {
    () => {
        r"
// The height in meters of the DEM's bordered texel at `texel`, clamped to
// the texture, decoded by `props`.
fn ground_texel_height(texel: vec2<i32>, props: TerrainSurfaceUBO) -> f32 {
    let last = vec2<i32>(textureDimensions(ground_dem)) - 1;
    let rgb = round(textureLoad(ground_dem, clamp(texel, vec2<i32>(0), last), 0).rgb * 255.0);
    let unpack = props.unpack;
    return (rgb.r * unpack.x + rgb.g * unpack.y + rgb.b * unpack.z + unpack.w) * props.unpack_scale;
}

// The height in meters, exaggerated, at `pos` in tile units of a tile that
// reads its DEM at `dem_coords`: the DEM uv's scale and offset, then the
// DEM's width in pixels, 0 for flat ground.
fn ground_height(pos: vec2<f32>, dem_coords: vec4<f32>, props: TerrainSurfaceUBO) -> f32 {
    let dim = dem_coords.w;
    if (dim == 0.0) {
        return 0.0;
    }
    let coord = (pos / 8192.0 * dem_coords.x + dem_coords.yz) * dim + 0.5;
    let first = floor(coord);
    let f = coord - first;
    let texel = vec2<i32>(first);
    let tl = ground_texel_height(texel, props);
    let tr = ground_texel_height(texel + vec2<i32>(1, 0), props);
    let bl = ground_texel_height(texel + vec2<i32>(0, 1), props);
    let br = ground_texel_height(texel + vec2<i32>(1, 1), props);
    let top = tl * (1.0 - f.x) + tr * f.x;
    let bottom = bl * (1.0 - f.x) + br * f.x;
    return (top * (1.0 - f.y) + bottom * f.y) * props.exaggeration;
}
"
    };
}

/// How a hillshade shades its slopes: its tile and evaluated blocks, and
/// `hillshade_color`, which shades a slope by the lights of `props`, the
/// `HillshadeEvaluatedPropsUBO` that the program binds. A macro, so that
/// `concat!` puts it in the fragment stages of the hillshade and of the
/// terrain's surface, which shades a drape of a hillshade's slopes.
#[doc(hidden)]
#[macro_export]
macro_rules! terrain_wgsl_hillshade_shading {
    () => {
        r"
const STANDARD: i32 = 0;
const COMBINED: i32 = 1;
const IGOR: i32 = 2;
const MULTIDIRECTIONAL: i32 = 3;
const BASIC: i32 = 4;

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

// The color of the slope `deriv`, shaded as `tileProps` says by the lights
// of `props`, the program's `HillshadeEvaluatedPropsUBO`.
fn hillshade_color(deriv: vec2<f32>, tileProps: HillshadeTilePropsUBO) -> vec4<f32> {
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
"
    };
}
pub(crate) use crate::terrain_wgsl_hillshade_shading as hillshade_shading;

/// The terrain's surface: a tile's grid, raised to the heights of the tile's
/// DEM with its skirts lowered and projected through the tile's entry of
/// `projectionVector`, in the colors of the tile's drape. A pole's vertices
/// stand at sea level. Where the tile's entry of `lightVector` has lights,
/// the drape holds a hillshade's slopes, premultiplied by its coverage,
/// which the surface shades with them. The fog of `terrain` covers it.
pub(crate) const TERRAIN: WgslSource = WgslSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // Where in the drape, its north-west corner at 0 and its rows from the
    // north, as they are drawn.
    @location(0) drape_pos: vec2<f32>,
    // How far along the view, in pixels: the clip w.
    @location(1) view_depth: f32,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    let drawable = drawableVector[globalIndex.value];
    let projection = projectionVector[globalIndex.value];
    let pos = vec2<f32>(in.pos.xy);
    let skirt = f32(in.pos.z);
    let height = surface_height(pos, drawable.dem_coords, terrain);
    var out: VertexOutput;
    out.position = projectTileFor3D(pos, height - skirt * terrain.skirt_length, projection);
    out.drape_pos = clamp(pos / 8192.0, vec2<f32>(0.0), vec2<f32>(1.0));
    out.view_depth = out.position.w;
    return out;
}
",
    ),
    fragment: Some(concat!(
        hillshade_shading!(),
        r"
struct FragmentInput {
    @location(0) drape_pos: vec2<f32>,
    @location(1) view_depth: f32,
};

@fragment
fn main(in: FragmentInput) -> @location(0) vec4<f32> {
    let color = textureSample(drape, drape_sampler, in.drape_pos);
    let light = lightVector[globalIndex.value];
    if (light.num_lights == 0 || color.a == 0.0) {
        return fogged(color, in.view_depth, terrain);
    }
    // The tile's latitudes run from its north edge, where the drape's rows
    // start.
    let latitude = mix(light.latrange.x, light.latrange.y, in.drape_pos.y);
    let deriv = ((color.rg / color.a) * 8.0 - vec2<f32>(4.0, 4.0)) / cos(radians(latitude));
    return fogged(hillshade_color(deriv, light) * color.a, in.view_depth, terrain);
}
"
    )),
    prelude: Some(TERRAIN_PRELUDE),
};

/// The ground's depth: the surface's grid, raised and projected as
/// `TERRAIN` does, writing each pixel's eye distance, an `f32`'s bits,
/// least significant byte first, as the texel's channels.
pub(crate) const TERRAIN_DEPTH: WgslSource = WgslSource {
    vertex: Some(
        r"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // The eye distance, which interpolates exactly across a triangle.
    @location(0) distance: f32,
};

@vertex
fn main(in: VertexInput) -> VertexOutput {
    let drawable = drawableVector[globalIndex.value];
    let projection = projectionVector[globalIndex.value];
    let pos = vec2<f32>(in.pos.xy);
    let skirt = f32(in.pos.z);
    let height = surface_height(pos, drawable.dem_coords, terrain);
    var out: VertexOutput;
    out.position = projectTileFor3D(pos, height - skirt * terrain.skirt_length, projection);
    out.distance = out.position.w;
    return out;
}
",
    ),
    fragment: Some(
        r"
@fragment
fn main(@location(0) distance: f32) -> @location(0) vec4<f32> {
    let bits = bitcast<u32>(distance);
    let bytes = vec4<u32>(bits, bits >> 8u, bits >> 16u, bits >> 24u) & vec4<u32>(255u);
    return vec4<f32>(bytes) / 255.0;
}
",
    ),
    prelude: Some(TERRAIN_PRELUDE),
};

/// The surface's and the probe's prelude: the terrain's uniform block, the
/// height rule, the height of the surface's vertices, and the fog.
pub const TERRAIN_PRELUDE: &str = concat!(
    ground_height!(),
    r"
// How the terrain's DEM decodes, its exaggeration and its skirts' length,
// then the fog over the surface.

// `color`, premultiplied, of the surface `view_depth` pixels along the
// view, covered by `fog`, as GL JS's terrain is: mixed in linear components
// toward the fog's color by the square of how far past the ground blend the
// fog's depth is. The fog's alpha weighs its color; the coverage stays.
fn fogged(color: vec4<f32>, view_depth: f32, fog: TerrainSurfaceUBO) -> vec4<f32> {
    let near = fog.depth_range.x;
    let far = fog.depth_range.y;
    let depth = min(far / (far - near) * (1.0 - near / view_depth), 1.0);
    if (fog.ground_blend >= 1.0 || depth <= fog.ground_blend || color.a == 0.0) {
        return color;
    }
    let horizon = smoothstep(0.0, 1.0, (depth - fog.horizon_blend) / max(1.0 - fog.horizon_blend, 1e-6));
    let tint = mix(fog.fog_color, fog.horizon_color, horizon);
    let along = (depth - fog.ground_blend) / (1.0 - fog.ground_blend);
    let strength = along * along;
    let gamma = vec3<f32>(2.2);
    let surface = pow(color.rgb / color.a, gamma);
    let mixed = surface * (1.0 - strength * tint.a) + tint.rgb * strength;
    return vec4<f32>(pow(mixed, 1.0 / gamma) * color.a, color.a);
}

// The height of the surface's vertex at `pos`, in tile units of a tile that
// reads its DEM at `dem_coords`: the ground's, or sea level at a pole.
fn surface_height(pos: vec2<f32>, dem_coords: vec4<f32>, props: TerrainSurfaceUBO) -> f32 {
    if (pos.y < GLOBE_POLE_NORTH_Y || pos.y > GLOBE_POLE_SOUTH_Y) {
        return 0.0;
    }
    return ground_height(pos, dem_coords, props);
}
"
);
