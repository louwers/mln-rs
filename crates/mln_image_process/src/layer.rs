use mln_rs::layer::{
    Buffer, Color, Draw, Draws, IndexBuffer, Layer, Mesh, MeshDesc, NewCx, NoTiles, PrepareCx, SamplerState, Size,
    Target, Texture, TextureChannelDataType, TextureFilterType, TextureWrapType, UpdateCx,
};

use crate::ImageProcess;
use crate::keys::{Operations, SourceTarget};
use crate::plan::plan;
use crate::program::{PROCESS, Process};

/// How a pass samples its textures: linearly, what lies past their edges
/// as what is at them.
const LINEAR: SamplerState = SamplerState {
    filter: TextureFilterType::Linear,
    wrap_u: TextureWrapType::Clamp,
    wrap_v: TextureWrapType::Clamp,
    max_anisotropy: 1,
    mipmapped: false,
};

/// An `org.maplibre.image-process` layer: a quad over what it draws into,
/// the scratch targets of its passes but the last, and the passes it draws.
pub struct ImageProcessLayer {
    corners: Buffer<[i16; 2]>,
    quad: IndexBuffer,
    scratch: Vec<Target>,
    /// The passes the frame draws.
    passes: Vec<Pass>,
}

/// A pass the layer draws: its mesh, with the textures it samples, and its
/// block.
struct Pass {
    textures: [Texture; 2],
    mesh: Mesh,
    block: Process,
}

impl Layer for ImageProcessLayer {
    type Style = ImageProcess;
    type Input = NoTiles;
    type Tile = ();

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        ImageProcessLayer {
            corners: cx.res.buffer(&[[0, 0], [1, 0], [0, 1], [1, 1]]),
            quad: cx.res.index_buffer(&[0, 1, 2, 1, 2, 3]),
            scratch: Vec::new(),
            passes: Vec::new(),
        }
    }

    fn update(&mut self, cx: &mut UpdateCx<'_, Self>) {
        let drawn = self.update_passes(cx).unwrap_or(0);
        self.passes.truncate(drawn);
        self.scratch.truncate(drawn.saturating_sub(1));
    }

    fn prepare(&self, _cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let Some((last, passes)) = self.passes.split_last() else { return };
        for (pass, target) in passes.iter().zip(&self.scratch) {
            out.target(target).block(&pass.block).push(&pass.mesh, Draw::DEFAULT, ());
        }
        out.main().block(&last.block).push(&last.mesh, Draw::DEFAULT, ());
    }
}

impl ImageProcessLayer {
    /// Brings the frame's passes up to date, and returns how many it draws:
    /// `None` where the layer reads a target the style doesn't declare, or
    /// has no source target.
    fn update_passes(&mut self, cx: &mut UpdateCx<'_, Self>) -> Option<usize> {
        let layer = cx.layer;
        let style = &layer.style;
        let SourceTarget(source) = style.extra::<SourceTarget>()?;
        let operations = style.extra::<Operations>().map_or(&[][..], |operations| &operations.0[..]);
        let frame = cx.frame.target_size;
        let ratio = cx.frame.pixel_ratio;
        let logical = Size::new(
            (frame.width as f32 / ratio).ceil().max(1.0) as u32,
            (frame.height as f32 / ratio).ceil().max(1.0) as u32,
        );
        let passes = plan(operations, cx.frame.camera.zoom() as f32, frame, logical);
        let mut input = cx.target(source)?;
        for (index, pass) in passes.iter().enumerate() {
            let other = match &pass.other {
                Some(other) => cx.target(other)?,
                None => input.clone(),
            };
            let next = (index + 1 < passes.len()).then(|| self.scratch(cx, index, pass.size));
            self.pass(cx, index, [input, other], &pass.block);
            let Some(next) = next else { break };
            input = next;
        }
        Some(passes.len())
    }

    /// The texture of the scratch target of the pass at `index`, `size`
    /// texels.
    fn scratch(&mut self, cx: &mut UpdateCx<'_, Self>, index: usize, size: Size) -> Texture {
        if index == self.scratch.len() {
            let clear = Color::new(0.0, 0.0, 0.0, 0.0);
            self.scratch.push(cx.res.target(size, TextureChannelDataType::HalfFloat, clear));
        }
        let target = &self.scratch[index];
        cx.res.resize(target, size);
        target.texture().clone()
    }

    /// Makes the pass at `index` sample `textures` with `block`, with a mesh
    /// made again only for other textures.
    fn pass(&mut self, cx: &mut UpdateCx<'_, Self>, index: usize, textures: [Texture; 2], block: &Process) {
        if let Some(pass) = self.passes.get_mut(index).filter(|pass| pass.textures == textures) {
            pass.block = *block;
            return;
        }
        let mesh = {
            let mut desc = MeshDesc::new(&PROCESS, &self.quad);
            desc.vertices(&self.corners).texture(&textures[0], LINEAR).texture(&textures[1], LINEAR);
            cx.res.mesh(&desc)
        };
        let pass = Pass { textures, mesh, block: *block };
        if index == self.passes.len() {
            self.passes.push(pass);
        } else {
            self.passes[index] = pass;
        }
    }
}
