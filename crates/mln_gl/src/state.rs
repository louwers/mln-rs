// The GL state that the backend sets, each piece with the value it last set,
// so that a call that sets it again reaches GL only when the value changes.

use glow::HasContext;
use mln_gfx::color_mode::{ColorMode, Mask};
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::{DepthState, StencilState, StencilTest};
use mln_gfx::shaders::shader_defines::MAX_UBO_COUNT_PER_SHADER;
use mln_gfx::texture2d::MAX_ACTIVE_TEXTURE_UNITS;
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    StencilFunctionType, StencilOpType,
};
use mln_util::Color;

use crate::enums;
use crate::types::{BufferID, FramebufferID, Gl, ProgramID, SamplerID, TextureID, VertexArrayID};

/// A scissor box or a viewport: x, y, width, height, from the bottom left.
pub(crate) type Rect = (i32, i32, i32, i32);

/// What GL was last told about one piece of state.
#[derive(Clone, Copy, Default, PartialEq)]
enum Cached<T> {
    /// Not known, so the next set always reaches GL.
    #[default]
    Dirty,
    /// GL holds this value.
    Known(T),
}

/// Sets `cached` to `value`; true if GL has to be told.
fn changed<T: Copy + PartialEq>(cached: &mut Cached<T>, value: T) -> bool {
    if *cached == Cached::Known(value) {
        return false;
    }
    *cached = Cached::Known(value);
    true
}

#[derive(Default)]
pub(crate) struct State {
    program: Cached<Option<ProgramID>>,
    vertex_array: Cached<Option<VertexArrayID>>,
    framebuffer: Cached<Option<FramebufferID>>,
    active_texture_unit: Cached<u32>,
    textures: [Cached<Option<TextureID>>; MAX_ACTIVE_TEXTURE_UNITS as usize],
    samplers: [Cached<Option<SamplerID>>; MAX_ACTIVE_TEXTURE_UNITS as usize],
    /// The buffer range bound to each uniform buffer binding point.
    uniform_buffers: [Cached<(BufferID, i32, i32)>; MAX_UBO_COUNT_PER_SHADER],

    depth_test: Cached<bool>,
    depth_func: Cached<DepthFunctionType>,
    depth_mask: Cached<bool>,
    depth_range: Cached<(f32, f32)>,

    stencil_test: Cached<bool>,
    stencil_func: Cached<(StencilFunctionType, i32, u32)>,
    stencil_mask: Cached<u32>,
    stencil_op: Cached<(StencilOpType, StencilOpType, StencilOpType)>,

    blend: Cached<bool>,
    blend_equation: Cached<ColorBlendEquationType>,
    blend_func: Cached<(ColorBlendFactorType, ColorBlendFactorType)>,
    blend_color: Cached<Color>,
    color_mask: Cached<Mask>,

    cull_face: Cached<bool>,
    cull_face_side: Cached<CullFaceSideType>,
    cull_face_winding: Cached<CullFaceWindingType>,

    /// `Known(None)`: the scissor test is off.
    scissor: Cached<Option<Rect>>,
    viewport: Cached<Rect>,

    clear_color: Cached<Color>,
    clear_depth: Cached<f32>,
    clear_stencil: Cached<i32>,

    line_width: Cached<f32>,
    pixel_store_pack_alignment: Cached<i32>,
    pixel_store_unpack_alignment: Cached<i32>,
}

impl State {
    /// Forgets what GL was last told, after code outside the backend has used
    /// the context.
    pub(crate) fn set_dirty(&mut self) {
        *self = State::default();
    }

    fn capability(gl: &Gl, cached: &mut Cached<bool>, capability: u32, enabled: bool) {
        if changed(cached, enabled) {
            // SAFETY: `capability` is one GL ES 3.0 defines.
            unsafe {
                if enabled {
                    gl.enable(capability);
                } else {
                    gl.disable(capability);
                }
            }
        }
    }

    pub(crate) fn use_program(&mut self, gl: &Gl, program: Option<ProgramID>) {
        if changed(&mut self.program, program) {
            // SAFETY: the program is alive, or none.
            unsafe { gl.use_program(program) };
        }
    }

    pub(crate) fn bind_vertex_array(&mut self, gl: &Gl, vertex_array: Option<VertexArrayID>) {
        if changed(&mut self.vertex_array, vertex_array) {
            // SAFETY: the vertex array is alive, or none.
            unsafe { gl.bind_vertex_array(vertex_array) };
        }
    }

    pub(crate) fn bind_framebuffer(&mut self, gl: &Gl, framebuffer: Option<FramebufferID>) {
        if changed(&mut self.framebuffer, framebuffer) {
            // SAFETY: the framebuffer is alive, or the default one.
            unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, framebuffer) };
        }
    }

    /// Binds `texture` to `unit`, which becomes the active one if GL has to
    /// be told.
    pub(crate) fn bind_texture(&mut self, gl: &Gl, unit: u32, texture: Option<TextureID>) {
        let Some(&cached) = self.textures.get(unit as usize) else {
            debug_assert!(false, "texture unit out of range");
            return;
        };
        if cached == Cached::Known(texture) {
            return;
        }
        self.active_texture(gl, unit);
        self.textures[unit as usize] = Cached::Known(texture);
        // SAFETY: the texture is alive, or none.
        unsafe { gl.bind_texture(glow::TEXTURE_2D, texture) };
    }

    /// Makes `unit` the active texture unit, with `texture` bound to it, for
    /// calls that change the texture.
    pub(crate) fn activate_texture(&mut self, gl: &Gl, unit: u32, texture: Option<TextureID>) {
        self.active_texture(gl, unit);
        if let Some(cached) = self.textures.get_mut(unit as usize)
            && changed(cached, texture)
        {
            // SAFETY: the texture is alive, or none.
            unsafe { gl.bind_texture(glow::TEXTURE_2D, texture) };
        }
    }

    fn active_texture(&mut self, gl: &Gl, unit: u32) {
        if changed(&mut self.active_texture_unit, unit) {
            // SAFETY: `unit` is below MAX_ACTIVE_TEXTURE_UNITS, which GL ES 3.0 has.
            unsafe { gl.active_texture(glow::TEXTURE0 + unit) };
        }
    }

    /// Binds `sampler` to `unit`, where it overrides the sampling parameters
    /// of the texture bound there.
    pub(crate) fn bind_sampler(&mut self, gl: &Gl, unit: u32, sampler: Option<SamplerID>) {
        let Some(cached) = self.samplers.get_mut(unit as usize) else {
            debug_assert!(false, "texture unit out of range");
            return;
        };
        if changed(cached, sampler) {
            // SAFETY: the sampler is alive, or none, and the unit is below
            // MAX_ACTIVE_TEXTURE_UNITS.
            unsafe { gl.bind_sampler(unit, sampler) };
        }
    }

    /// Unbinds the samplers from every texture unit, for code outside the
    /// backend that samples with its textures' own parameters.
    pub(crate) fn unbind_samplers(&mut self, gl: &Gl) {
        for unit in 0..self.samplers.len() {
            self.bind_sampler(gl, unit as u32, None);
        }
    }

    /// Forgets `sampler`, which is being deleted.
    pub(crate) fn forget_sampler(&mut self, sampler: SamplerID) {
        for cached in &mut self.samplers {
            if *cached == Cached::Known(Some(sampler)) {
                *cached = Cached::Dirty;
            }
        }
    }

    /// Binds `size` bytes at `offset` of `buffer` to the uniform buffer
    /// binding point `index`.
    pub(crate) fn bind_uniform_buffer_range(
        &mut self,
        gl: &Gl,
        index: usize,
        buffer: BufferID,
        offset: i32,
        size: i32,
    ) {
        let Some(cached) = self.uniform_buffers.get_mut(index) else {
            debug_assert!(false, "uniform buffer binding out of range");
            return;
        };
        if changed(cached, (buffer, offset, size)) {
            // SAFETY: the range is inside the buffer, which is alive.
            unsafe { gl.bind_buffer_range(glow::UNIFORM_BUFFER, index as u32, Some(buffer), offset, size) };
        }
    }

    /// Forgets `buffer`, which is being deleted or has new storage.
    pub(crate) fn forget_uniform_buffer(&mut self, buffer: BufferID) {
        for cached in &mut self.uniform_buffers {
            if matches!(*cached, Cached::Known((bound, _, _)) if bound == buffer) {
                *cached = Cached::Dirty;
            }
        }
    }

    /// Sets the depth test and write of `depth`, and the depths `range`
    /// that clip z maps to.
    pub(crate) fn set_depth(&mut self, gl: &Gl, depth: &DepthState, range: (f32, f32)) {
        if depth.compare == DepthFunctionType::Always && !depth.write {
            Self::capability(gl, &mut self.depth_test, glow::DEPTH_TEST, false);
            // Depth-related state should not matter when the depth test is
            // disabled, but on Adreno 2xx GPUs it apparently does
            // (mapbox-gl-native#9164), so it is set all the same.
        } else {
            Self::capability(gl, &mut self.depth_test, glow::DEPTH_TEST, true);
        }
        if changed(&mut self.depth_func, depth.compare) {
            // SAFETY: a depth function of GL ES 3.0.
            unsafe { gl.depth_func(enums::depth_function(depth.compare)) };
        }
        self.set_depth_mask(gl, depth.write);
        if changed(&mut self.depth_range, range) {
            // `glDepthRange` on desktop OpenGL, which has `glDepthRangef`
            // only from 4.1, and `glDepthRangef` on OpenGL ES.
            // SAFETY: GL clamps the range.
            unsafe { gl.depth_range(f64::from(range.0), f64::from(range.1)) };
        }
    }

    fn set_depth_mask(&mut self, gl: &Gl, write: bool) {
        if changed(&mut self.depth_mask, write) {
            // SAFETY: no preconditions.
            unsafe { gl.depth_mask(write) };
        }
    }

    /// Sets the stencil test and operations of `stencil`, which test and
    /// write `reference`.
    pub(crate) fn set_stencil(&mut self, gl: &Gl, stencil: &StencilState, reference: i32) {
        if stencil.test == StencilTest::Always && stencil.write_mask == 0 {
            Self::capability(gl, &mut self.stencil_test, glow::STENCIL_TEST, false);
            return;
        }
        Self::capability(gl, &mut self.stencil_test, glow::STENCIL_TEST, true);
        self.set_stencil_mask(gl, stencil.write_mask);
        if changed(&mut self.stencil_op, (stencil.fail, stencil.depth_fail, stencil.pass)) {
            // SAFETY: stencil operations of GL ES 3.0.
            unsafe {
                gl.stencil_op(
                    enums::stencil_op(stencil.fail),
                    enums::stencil_op(stencil.depth_fail),
                    enums::stencil_op(stencil.pass),
                );
            }
        }
        let func = stencil.test.func();
        let mask = stencil.test.masked().unwrap_or(0);
        if changed(&mut self.stencil_func, (func, reference, mask)) {
            // SAFETY: a stencil function of GL ES 3.0.
            unsafe { gl.stencil_func(enums::stencil_function(func), reference, mask) };
        }
    }

    fn set_stencil_mask(&mut self, gl: &Gl, mask: u32) {
        if changed(&mut self.stencil_mask, mask) {
            // SAFETY: no preconditions.
            unsafe { gl.stencil_mask(mask) };
        }
    }

    pub(crate) fn set_color_mode(&mut self, gl: &Gl, color: &ColorMode) {
        match color.blend_function {
            mln_gfx::color_mode::BlendFunction::Replace => Self::capability(gl, &mut self.blend, glow::BLEND, false),
            function => {
                Self::capability(gl, &mut self.blend, glow::BLEND, true);
                if changed(&mut self.blend_color, color.blend_color) {
                    let c = color.blend_color;
                    // SAFETY: no preconditions.
                    unsafe { gl.blend_color(c.r, c.g, c.b, c.a) };
                }
                if changed(&mut self.blend_equation, function.equation()) {
                    // SAFETY: a blend equation of GL ES 3.0.
                    unsafe { gl.blend_equation(enums::color_blend_equation(function.equation())) };
                }
                if changed(&mut self.blend_func, (function.src_factor(), function.dst_factor())) {
                    // SAFETY: blend factors of GL ES 3.0.
                    unsafe {
                        gl.blend_func(
                            enums::color_blend_factor(function.src_factor()),
                            enums::color_blend_factor(function.dst_factor()),
                        );
                    }
                }
            }
        }
        self.set_color_mask(gl, color.mask);
    }

    fn set_color_mask(&mut self, gl: &Gl, mask: Mask) {
        if changed(&mut self.color_mask, mask) {
            // SAFETY: no preconditions.
            unsafe { gl.color_mask(mask.r, mask.g, mask.b, mask.a) };
        }
    }

    pub(crate) fn set_cull_face_mode(&mut self, gl: &Gl, mode: CullFaceMode) {
        Self::capability(gl, &mut self.cull_face, glow::CULL_FACE, mode.enabled);
        // These shouldn't need to be updated when face culling is disabled,
        // but we might end up having the same issues with Adreno 2xx GPUs as
        // noted in `set_depth`.
        if changed(&mut self.cull_face_side, mode.side) {
            // SAFETY: a face of GL ES 3.0.
            unsafe { gl.cull_face(enums::cull_face_side(mode.side)) };
        }
        if changed(&mut self.cull_face_winding, mode.winding) {
            // SAFETY: a winding of GL ES 3.0.
            unsafe { gl.front_face(enums::cull_face_winding(mode.winding)) };
        }
    }

    /// Restricts drawing to `rect`, or to nothing less than the framebuffer
    /// without one.
    pub(crate) fn set_scissor(&mut self, gl: &Gl, rect: Option<Rect>) {
        if !changed(&mut self.scissor, rect) {
            return;
        }
        // SAFETY: GL rejects a negative size, which a `u32` can't give.
        unsafe {
            match rect {
                Some((x, y, width, height)) => {
                    gl.enable(glow::SCISSOR_TEST);
                    gl.scissor(x, y, width, height);
                }
                None => gl.disable(glow::SCISSOR_TEST),
            }
        }
    }

    pub(crate) fn set_viewport(&mut self, gl: &Gl, rect: Rect) {
        if changed(&mut self.viewport, rect) {
            // SAFETY: the sizes aren't negative.
            unsafe { gl.viewport(rect.0, rect.1, rect.2, rect.3) };
        }
    }

    pub(crate) fn set_line_width(&mut self, gl: &Gl, width: f32) {
        if changed(&mut self.line_width, width) {
            // SAFETY: the width is positive (`DrawMode`).
            unsafe { gl.line_width(width) };
        }
    }

    pub(crate) fn set_pixel_store_pack_alignment(&mut self, gl: &Gl, alignment: i32) {
        if changed(&mut self.pixel_store_pack_alignment, alignment) {
            // SAFETY: an alignment GL defines.
            unsafe { gl.pixel_store_i32(glow::PACK_ALIGNMENT, alignment) };
        }
    }

    pub(crate) fn set_pixel_store_unpack_alignment(&mut self, gl: &Gl, alignment: i32) {
        if changed(&mut self.pixel_store_unpack_alignment, alignment) {
            // SAFETY: an alignment GL defines.
            unsafe { gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, alignment) };
        }
    }

    /// Clears the buffers given a value, all of each: the scissor test and
    /// the buffers' write masks would restrict it.
    pub(crate) fn clear(&mut self, gl: &Gl, color: Option<Color>, depth: Option<f32>, stencil: Option<i32>) {
        let mut mask = 0;
        if let Some(color) = color {
            mask |= glow::COLOR_BUFFER_BIT;
            if changed(&mut self.clear_color, color) {
                // SAFETY: no preconditions.
                unsafe { gl.clear_color(color.r, color.g, color.b, color.a) };
            }
            self.set_color_mask(gl, Mask { r: true, g: true, b: true, a: true });
        }
        if let Some(depth) = depth {
            mask |= glow::DEPTH_BUFFER_BIT;
            if changed(&mut self.clear_depth, depth) {
                // `glClearDepth` on desktop OpenGL, which has `glClearDepthf`
                // only from 4.1, and `glClearDepthf` on OpenGL ES.
                // SAFETY: no preconditions.
                unsafe { gl.clear_depth(f64::from(depth)) };
            }
            self.set_depth_mask(gl, true);
        }
        if let Some(stencil) = stencil {
            mask |= glow::STENCIL_BUFFER_BIT;
            if changed(&mut self.clear_stencil, stencil) {
                // SAFETY: no preconditions.
                unsafe { gl.clear_stencil(stencil) };
            }
            self.set_stencil_mask(gl, 0xFF);
        }
        if mask != 0 {
            self.set_scissor(gl, None);
            // SAFETY: `mask` names buffers only.
            unsafe { gl.clear(mask) };
        }
    }

    /// Forgets the objects in `f`'s lists, which are being deleted.
    pub(crate) fn forget_program(&mut self, program: ProgramID) {
        if self.program == Cached::Known(Some(program)) {
            self.program = Cached::Dirty;
        }
    }

    pub(crate) fn forget_vertex_array(&mut self, vertex_array: VertexArrayID) {
        if self.vertex_array == Cached::Known(Some(vertex_array)) {
            self.vertex_array = Cached::Dirty;
        }
    }

    pub(crate) fn forget_framebuffer(&mut self, framebuffer: FramebufferID) {
        if self.framebuffer == Cached::Known(Some(framebuffer)) {
            self.framebuffer = Cached::Dirty;
        }
    }

    pub(crate) fn forget_texture(&mut self, texture: TextureID) {
        for cached in &mut self.textures {
            if *cached == Cached::Known(Some(texture)) {
                *cached = Cached::Dirty;
            }
        }
    }
}
