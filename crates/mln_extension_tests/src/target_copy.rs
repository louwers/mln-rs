//! The `target-copy` layer type: a layer without a source that reads a
//! named target of the style, the one its key `test:source-target` names,
//! and draws it over the map at its place among the layers, or into its own
//! destination. It copies the target into a target of its own first, so
//! that what it draws before the frame's passes reads the named target too.

use mln_rs::layer::AttributeDataType::Short2;
use mln_rs::layer::source::{glsl, glsl_es, msl, wgsl};
use mln_rs::layer::{
    Attribute, Buffer, Convertible, Depth, Draw, Draws, Functions, IndexBuffer, Layer, LayerTypeInfo, Mesh, MeshDesc,
    NewCx, NoProperties, NoTiles, PrepareCx, Program, Projection, Resources, SamplerState, Size, SourceKind, Sources,
    StyleKey, StyleLayerType, Target, TextureChannelDataType, UpdateCx, Value, conversion, program,
};
use mln_rs::mln_util::Color;

/// The `target-copy` layer type's style half.
pub struct TargetCopy;

impl StyleLayerType for TargetCopy {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "target-copy", source: SourceKind::None, from_json: true };
    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();
}

/// The name of the named target that a `target-copy` layer reads.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceTarget(pub String);

/// A `target-copy` layer's `test:source-target`.
pub static SOURCE_TARGET: StyleKey<SourceTarget> =
    StyleKey::layer_of::<TargetCopy>("test:source-target", parse_source, |source| Value::String(source.0.clone()));

fn parse_source(value: &Convertible<'_>, _: &std::sync::Arc<Functions>) -> Result<SourceTarget, conversion::Error> {
    value.to_string().map(SourceTarget).ok_or_else(|| conversion::Error::new("expected a target's name"))
}

/// Draws `image`, a texture of the frame's size, texel for texel onto what
/// it draws into, as premultiplied colors.
pub static COPY: Program = Program {
    name: "target-copy",
    buffers: &[],
    attributes: &[Attribute::new("pos", Short2)],
    instance_attributes: &[],
    textures: &[program::Texture::unfilterable("image")],
    properties: &[],
    projection: Projection::None,
    sources: Sources::NONE
        .wgsl(&wgsl::Wgsl {
            prelude: "",
            vertex: "
struct VertexInput {
    @location(0) pos: vec2<i32>,
};
@vertex fn main(in: VertexInput) -> @builtin(position) vec4<f32> {
    return vec4<f32>(vec2<f32>(in.pos) * 2.0 - 1.0, 0.0, 1.0);
}",
            fragment: "
@fragment fn main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
#ifdef OVERDRAW_INSPECTOR
    return vec4<f32>(1.0);
#else
    return textureLoad(image, vec2<i32>(position.xy), 0);
#endif
}",
        })
        .msl(&msl::Msl {
            vertex_main_function: "vertexMain",
            fragment_main_function: "fragmentMain",
            prelude: "",
            source: "
struct VertexStage {
    short2 pos [[attribute(0)]];
};
struct FragmentStage {
    float4 position [[position, invariant]];
};
FragmentStage vertex vertexMain(thread const VertexStage vertex_in [[stage_in]]) {
    return { .position = float4(float2(vertex_in.pos) * 2.0 - 1.0, 0.0, 1.0) };
}
half4 fragment fragmentMain(FragmentStage in [[stage_in]]) {
#if defined(OVERDRAW_INSPECTOR)
    return half4(1.0);
#else
    return half4(image.read(uint2(in.position.xy)));
#endif
}",
        })
        .glsl_es(&glsl_es::Glsl {
            vertex: "layout (location = 0) in vec2 a_pos;
void main() {
    gl_Position = vec4(a_pos * 2.0 - 1.0, 0.0, 1.0);
}",
            fragment: "
void main() {
#ifdef OVERDRAW_INSPECTOR
    fragColor = vec4(1.0);
#else
    fragColor = texelFetch(u_image, ivec2(gl_FragCoord.xy), 0);
#endif
}",
        })
        .glsl(&glsl::Glsl {
            prelude: "",
            vertex: "
layout(location = 0) in ivec2 in_pos;
void main() {
    gl_Position = vec4(vec2(in_pos) * 2.0 - 1.0, 0.0, 1.0);
}",
            fragment: "
layout(location = 0) out vec4 out_color;
void main() {
#if defined(OVERDRAW_INSPECTOR)
    out_color = vec4(1.0);
#else
    out_color = texelFetch(sampler2D(image, image_sampler), ivec2(gl_FragCoord.xy), 0);
#endif
}",
        }),
};

/// A `target-copy` layer: the quad over the frame, its own target, and the
/// meshes that draw the named target into it and it onto the frame.
pub struct TargetCopyLayer {
    quad: Buffer<[i16; 2]>,
    indexes: IndexBuffer,
    own: Target,
    from_own: Mesh,
    /// The mesh that draws the named target the layer read this frame into
    /// its own, with the texture it samples; `None` where it read none.
    into_own: Option<(mln_rs::layer::Texture, Mesh)>,
    read: bool,
}

/// A mesh of the quad that draws `texture` with [`COPY`].
fn copy(res: &mut Resources, quad: &Buffer<[i16; 2]>, indexes: &IndexBuffer, texture: &mln_rs::layer::Texture) -> Mesh {
    let mut mesh = MeshDesc::new(&COPY, indexes);
    mesh.vertices(quad).texture(texture, SamplerState::default());
    res.mesh(&mesh)
}

impl Layer for TargetCopyLayer {
    type Style = TargetCopy;
    type Input = NoTiles;
    type Tile = ();

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        let quad = cx.res.buffer(&[[0, 0], [1, 0], [0, 1], [1, 1]]);
        let indexes = cx.res.index_buffer(&[0, 1, 2, 1, 2, 3]);
        let own = cx.res.target(Size::new(1, 1), TextureChannelDataType::UnsignedByte, Color::new(0.0, 0.0, 0.0, 0.0));
        let from_own = copy(cx.res, &quad, &indexes, own.texture());
        TargetCopyLayer { quad, indexes, own, from_own, into_own: None, read: false }
    }

    fn update(&mut self, cx: &mut UpdateCx<'_, Self>) {
        let source = cx.layer.style.extra::<SourceTarget>().map(|source| source.0.clone());
        let Some(texture) = source.and_then(|source| cx.target(&source)) else {
            self.read = false;
            return;
        };
        cx.res.resize(&self.own, cx.frame.target_size);
        if self.into_own.as_ref().is_none_or(|(read, _)| *read != texture) {
            let mesh = copy(cx.res, &self.quad, &self.indexes, &texture);
            self.into_own = Some((texture, mesh));
        }
        self.read = true;
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let Some((_, into_own)) = self.into_own.as_ref().filter(|_| self.read) else { return };
        let draw = Draw { depth: Depth::Off, ..Draw::DEFAULT };
        out.target(&self.own).push(into_own, draw, ());
        out.main().push(&self.from_own, draw, ());
    }
}
