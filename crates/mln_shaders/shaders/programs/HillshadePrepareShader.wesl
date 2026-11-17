// The hillshade layer's first pass: a DEM tile's slope, from its elevations
// with a Sobel kernel, into a texture's red and green.

struct HillshadePrepareDrawableUBO {
    matrix: mat4x4<f32>,
};

struct HillshadePrepareTilePropsUBO {
    // The weights of the encoded elevation's channels, in meters.
    unpack: vec4<f32>,
    // The DEM's size in pixels, with its one-pixel border.
    dimension: vec2<f32>,
    zoom: f32,
    maxzoom: f32,
};

// ID_HILLSHADE_PREPARE_DRAWABLE_UBO
@group(2) @binding(2) var<uniform> drawable: HillshadePrepareDrawableUBO;
// ID_HILLSHADE_PREPARE_TILE_PROPS_UBO
@group(2) @binding(5) var<uniform> tileProps: HillshadePrepareTilePropsUBO;

// ID_HILLSHADE_IMAGE_TEXTURE: the DEM.
@group(3) @binding(0) var image_texture: texture_2d<f32>;
@group(3) @binding(3) var image_sampler: sampler;

// The locations are the ID_HILLSHADE_*_VERTEX_ATTRIBUTE ids.
struct VertexInput {
    @location(0) position: vec2<i32>,
    // The texture position, scaled by the tile extent, 8192.
    @location(1) texture_pos: vec2<i32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pos: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = drawable.matrix * vec4<f32>(vec2<f32>(in.position), 0.0, 1.0);
    // Inside the DEM's border.
    let epsilon = 1.0 / tileProps.dimension;
    let scale = (tileProps.dimension.x - 2.0) / tileProps.dimension.x;
    out.pos = vec2<f32>(in.texture_pos) / 8192.0 * scale + epsilon;
    return out;
}

// The elevation at `coord`, in meters.
fn elevation(coord: vec2<f32>) -> f32 {
    var data = textureSample(image_texture, image_sampler, coord) * 255.0;
    data.a = -1.0;
    return dot(data, tileProps.unpack);
}

@fragment
fn fs_main(@location(0) pos: vec2<f32>) -> @location(0) vec4<f32> {
    @if(OVERDRAW_INSPECTOR) {
        return vec4<f32>(1.0);
    }
    @else {
        let epsilon = 1.0 / tileProps.dimension;
        let tile_size = tileProps.dimension.x - 2.0;

        // The pixels around this one, for the Sobel kernel:
        // a b c
        // d e f
        // g h i
        let a = elevation(pos + vec2<f32>(-epsilon.x, -epsilon.y));
        let b = elevation(pos + vec2<f32>(0.0, -epsilon.y));
        let c = elevation(pos + vec2<f32>(epsilon.x, -epsilon.y));
        let d = elevation(pos + vec2<f32>(-epsilon.x, 0.0));
        let f = elevation(pos + vec2<f32>(epsilon.x, 0.0));
        let g = elevation(pos + vec2<f32>(-epsilon.x, epsilon.y));
        let h = elevation(pos + vec2<f32>(0.0, epsilon.y));
        let i = elevation(pos + vec2<f32>(epsilon.x, epsilon.y));

        // The pixel-space derivative to world-space slope: tile_size / (8 *
        // meters per pixel), with meters per pixel 2^(28.2562 - zoom),
        // exaggerated at lower zooms.
        var exaggeration_factor = 0.3;
        if (tileProps.zoom < 4.5) {
            exaggeration_factor = 0.35;
        }
        if (tileProps.zoom < 2.0) {
            exaggeration_factor = 0.4;
        }
        var exaggeration = 0.0;
        if (tileProps.zoom < 15.0) {
            exaggeration = (tileProps.zoom - 15.0) * exaggeration_factor;
        }
        let deriv = vec2<f32>((c + f + f + i) - (a + d + d + g), (g + h + h + i) - (a + b + b + c)) * tile_size /
            pow(2.0, exaggeration + (28.2562 - tileProps.zoom));

        // The slope, at most 4, from 0 to 1.
        return clamp(vec4<f32>(deriv.x / 8.0 + 0.5, deriv.y / 8.0 + 0.5, 1.0, 1.0), vec4<f32>(0.0), vec4<f32>(1.0));
    }
}
