// Ports src/mln/gl/state.hpp and src/mln/gl/value.hpp/.cpp
//
// C++ wraps each piece of GL state in a `State<value::T>` that remembers the
// value it last set, and the context has one member per piece. The port
// keeps them in one struct: a `None` is C++'s dirty state, whose next set
// always reaches GL. Only the states the drawable path sets are here.

use std::cell::Cell;

use glow::HasContext;
use mln_gfx::color_mode::{ColorMode, Mask};
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::depth_mode::DepthMode;
use mln_gfx::shaders::shader_defines::MAX_UBO_COUNT_PER_SHADER;
use mln_gfx::stencil_mode::{StencilMode, Test};
use mln_gfx::texture2d::MAX_ACTIVE_TEXTURE_UNITS;
use mln_gfx::types::{
    ColorBlendEquationType, ColorBlendFactorType, CullFaceSideType, CullFaceWindingType, DepthFunctionType,
    DepthMaskType, StencilFunctionType, StencilOpType,
};
use mln_util::Color;

use crate::enums;
use crate::types::{BufferID, FramebufferID, Gl, ProgramID, TextureID, VertexArrayID};

/// A scissor box or a viewport: x, y, width, height, from the bottom left.
pub type Rect = (i32, i32, i32, i32);

/// Sets `cell` to `value`; true if GL has to be told.
fn changed<T: Copy + PartialEq>(cell: &Cell<Option<T>>, value: T) -> bool {
    if cell.get() == Some(value) {
        return false;
    }
    cell.set(Some(value));
    true
}

#[derive(Default)]
pub struct State {
    program: Cell<Option<Option<ProgramID>>>,
    vertex_array: Cell<Option<Option<VertexArrayID>>>,
    framebuffer: Cell<Option<Option<FramebufferID>>>,
    active_texture_unit: Cell<Option<u32>>,
    textures: [Cell<Option<Option<TextureID>>>; MAX_ACTIVE_TEXTURE_UNITS as usize],
    /// The buffer range bound to each uniform buffer binding point.
    uniform_buffers: [Cell<Option<(BufferID, i32, i32)>>; MAX_UBO_COUNT_PER_SHADER],

    depth_test: Cell<Option<bool>>,
    depth_func: Cell<Option<DepthFunctionType>>,
    depth_mask: Cell<Option<bool>>,
    depth_range: Cell<Option<(f32, f32)>>,

    stencil_test: Cell<Option<bool>>,
    stencil_func: Cell<Option<(StencilFunctionType, i32, u32)>>,
    stencil_mask: Cell<Option<u32>>,
    stencil_op: Cell<Option<(StencilOpType, StencilOpType, StencilOpType)>>,

    blend: Cell<Option<bool>>,
    blend_equation: Cell<Option<ColorBlendEquationType>>,
    blend_func: Cell<Option<(ColorBlendFactorType, ColorBlendFactorType)>>,
    blend_color: Cell<Option<Color>>,
    color_mask: Cell<Option<Mask>>,

    cull_face: Cell<Option<bool>>,
    cull_face_side: Cell<Option<CullFaceSideType>>,
    cull_face_winding: Cell<Option<CullFaceWindingType>>,

    /// `Some(None)`: the scissor test is off.
    scissor: Cell<Option<Option<Rect>>>,
    viewport: Cell<Option<Rect>>,

    clear_color: Cell<Option<Color>>,
    clear_depth: Cell<Option<f32>>,
    clear_stencil: Cell<Option<i32>>,

    line_width: Cell<Option<f32>>,
    pixel_store_pack_alignment: Cell<Option<i32>>,
    pixel_store_unpack_alignment: Cell<Option<i32>>,
}

impl State {
    /// `Context::setDirtyState`: forgets what GL was last told, after code
    /// outside the backend has used the context.
    pub fn set_dirty(&self) {
        self.program.set(None);
        self.vertex_array.set(None);
        self.framebuffer.set(None);
        self.active_texture_unit.set(None);
        for texture in &self.textures {
            texture.set(None);
        }
        for buffer in &self.uniform_buffers {
            buffer.set(None);
        }
        self.depth_test.set(None);
        self.depth_func.set(None);
        self.depth_mask.set(None);
        self.depth_range.set(None);
        self.stencil_test.set(None);
        self.stencil_func.set(None);
        self.stencil_mask.set(None);
        self.stencil_op.set(None);
        self.blend.set(None);
        self.blend_equation.set(None);
        self.blend_func.set(None);
        self.blend_color.set(None);
        self.color_mask.set(None);
        self.cull_face.set(None);
        self.cull_face_side.set(None);
        self.cull_face_winding.set(None);
        self.scissor.set(None);
        self.viewport.set(None);
        self.clear_color.set(None);
        self.clear_depth.set(None);
        self.clear_stencil.set(None);
        self.line_width.set(None);
        self.pixel_store_pack_alignment.set(None);
        self.pixel_store_unpack_alignment.set(None);
    }

    fn capability(gl: &Gl, cell: &Cell<Option<bool>>, capability: u32, enabled: bool) {
        if changed(cell, enabled) {
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

    /// `value::Program`
    pub fn use_program(&self, gl: &Gl, program: Option<ProgramID>) {
        if changed(&self.program, program) {
            // SAFETY: the program is alive, or none.
            unsafe { gl.use_program(program) };
        }
    }

    pub fn current_program(&self) -> Option<ProgramID> {
        self.program.get().flatten()
    }

    /// `value::BindVertexArray`
    pub fn bind_vertex_array(&self, gl: &Gl, vertex_array: Option<VertexArrayID>) {
        if changed(&self.vertex_array, vertex_array) {
            // SAFETY: the vertex array is alive, or none.
            unsafe { gl.bind_vertex_array(vertex_array) };
        }
    }

    /// `value::BindFramebuffer`
    pub fn bind_framebuffer(&self, gl: &Gl, framebuffer: Option<FramebufferID>) {
        if changed(&self.framebuffer, framebuffer) {
            // SAFETY: the framebuffer is alive, or the default one.
            unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, framebuffer) };
        }
    }

    /// `value::ActiveTextureUnit` and `value::BindTexture`: binds `texture`
    /// to `unit`, which becomes the active one if GL has to be told.
    pub fn bind_texture(&self, gl: &Gl, unit: u32, texture: Option<TextureID>) {
        let Some(cell) = self.textures.get(unit as usize) else {
            debug_assert!(false, "texture unit out of range");
            return;
        };
        if cell.get() == Some(texture) {
            return;
        }
        self.active_texture(gl, unit);
        cell.set(Some(texture));
        // SAFETY: the texture is alive, or none.
        unsafe { gl.bind_texture(glow::TEXTURE_2D, texture) };
    }

    /// Makes `unit` the active texture unit, with `texture` bound to it, for
    /// calls that change the texture.
    pub fn activate_texture(&self, gl: &Gl, unit: u32, texture: Option<TextureID>) {
        self.active_texture(gl, unit);
        if let Some(cell) = self.textures.get(unit as usize)
            && changed(cell, texture)
        {
            // SAFETY: the texture is alive, or none.
            unsafe { gl.bind_texture(glow::TEXTURE_2D, texture) };
        }
    }

    fn active_texture(&self, gl: &Gl, unit: u32) {
        if changed(&self.active_texture_unit, unit) {
            // SAFETY: `unit` is below MAX_ACTIVE_TEXTURE_UNITS, which GL ES 3.0 has.
            unsafe { gl.active_texture(glow::TEXTURE0 + unit) };
        }
    }

    /// Forgets `texture`, which is being deleted or rendered into: unbinds it
    /// from the units it is bound to.
    pub fn unbind_texture(&self, gl: &Gl, texture: TextureID) {
        for unit in 0..self.textures.len() {
            if self.textures[unit].get() == Some(Some(texture)) {
                self.bind_texture(gl, unit as u32, None);
            }
        }
    }

    /// Binds `size` bytes at `offset` of `buffer` to the uniform buffer
    /// binding point `index`.
    pub fn bind_uniform_buffer_range(&self, gl: &Gl, index: usize, buffer: BufferID, offset: i32, size: i32) {
        let Some(cell) = self.uniform_buffers.get(index) else {
            debug_assert!(false, "uniform buffer binding out of range");
            return;
        };
        if changed(cell, (buffer, offset, size)) {
            // SAFETY: the range is inside the buffer, which is alive.
            unsafe { gl.bind_buffer_range(glow::UNIFORM_BUFFER, index as u32, Some(buffer), offset, size) };
        }
    }

    /// Forgets `buffer`, which is being deleted or has new storage.
    pub fn forget_uniform_buffer(&self, buffer: BufferID) {
        for cell in &self.uniform_buffers {
            if cell.get().is_some_and(|(bound, _, _)| bound == buffer) {
                cell.set(None);
            }
        }
    }

    /// `Context::setDepthMode`
    pub fn set_depth_mode(&self, gl: &Gl, depth: &DepthMode) {
        if depth.func == DepthFunctionType::Always && depth.mask != DepthMaskType::ReadWrite {
            Self::capability(gl, &self.depth_test, glow::DEPTH_TEST, false);
            // Depth-related state should not matter when the depth test is
            // disabled, but on Adreno 2xx GPUs it apparently does
            // (mapbox-gl-native#9164), so it is set all the same.
        } else {
            Self::capability(gl, &self.depth_test, glow::DEPTH_TEST, true);
        }
        if changed(&self.depth_func, depth.func) {
            // SAFETY: a depth function of GL ES 3.0.
            unsafe { gl.depth_func(enums::depth_function(depth.func)) };
        }
        self.set_depth_mask(gl, depth.mask == DepthMaskType::ReadWrite);
        if changed(&self.depth_range, (depth.range.min, depth.range.max)) {
            // SAFETY: GL clamps the range.
            unsafe { gl.depth_range_f32(depth.range.min, depth.range.max) };
        }
    }

    fn set_depth_mask(&self, gl: &Gl, write: bool) {
        if changed(&self.depth_mask, write) {
            // SAFETY: no preconditions.
            unsafe { gl.depth_mask(write) };
        }
    }

    /// `Context::setStencilMode`
    pub fn set_stencil_mode(&self, gl: &Gl, stencil: &StencilMode) {
        if stencil.test == Test::Always && stencil.mask == 0 {
            Self::capability(gl, &self.stencil_test, glow::STENCIL_TEST, false);
            return;
        }
        Self::capability(gl, &self.stencil_test, glow::STENCIL_TEST, true);
        self.set_stencil_mask(gl, stencil.mask);
        if changed(&self.stencil_op, (stencil.fail, stencil.depth_fail, stencil.pass)) {
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
        if changed(&self.stencil_func, (func, stencil.ref_, mask)) {
            // SAFETY: a stencil function of GL ES 3.0.
            unsafe { gl.stencil_func(enums::stencil_function(func), stencil.ref_, mask) };
        }
    }

    fn set_stencil_mask(&self, gl: &Gl, mask: u32) {
        if changed(&self.stencil_mask, mask) {
            // SAFETY: no preconditions.
            unsafe { gl.stencil_mask(mask) };
        }
    }

    /// `Context::setColorMode`
    pub fn set_color_mode(&self, gl: &Gl, color: &ColorMode) {
        match color.blend_function {
            mln_gfx::color_mode::BlendFunction::Replace => Self::capability(gl, &self.blend, glow::BLEND, false),
            function => {
                Self::capability(gl, &self.blend, glow::BLEND, true);
                if changed(&self.blend_color, color.blend_color) {
                    let c = color.blend_color;
                    // SAFETY: no preconditions.
                    unsafe { gl.blend_color(c.r, c.g, c.b, c.a) };
                }
                if changed(&self.blend_equation, function.equation()) {
                    // SAFETY: a blend equation of GL ES 3.0.
                    unsafe { gl.blend_equation(enums::color_blend_equation(function.equation())) };
                }
                if changed(&self.blend_func, (function.src_factor(), function.dst_factor())) {
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

    fn set_color_mask(&self, gl: &Gl, mask: Mask) {
        if changed(&self.color_mask, mask) {
            // SAFETY: no preconditions.
            unsafe { gl.color_mask(mask.r, mask.g, mask.b, mask.a) };
        }
    }

    /// `Context::setCullFaceMode`
    pub fn set_cull_face_mode(&self, gl: &Gl, mode: &CullFaceMode) {
        Self::capability(gl, &self.cull_face, glow::CULL_FACE, mode.enabled);
        // These shouldn't need to be updated when face culling is disabled,
        // but we might end up having the same issues with Adreno 2xx GPUs as
        // noted in `set_depth_mode`.
        if changed(&self.cull_face_side, mode.side) {
            // SAFETY: a face of GL ES 3.0.
            unsafe { gl.cull_face(enums::cull_face_side(mode.side)) };
        }
        if changed(&self.cull_face_winding, mode.winding) {
            // SAFETY: a winding of GL ES 3.0.
            unsafe { gl.front_face(enums::cull_face_winding(mode.winding)) };
        }
    }

    /// `value::ScissorTest`: restricts drawing to `rect`, or to nothing less
    /// than the framebuffer without one.
    pub fn set_scissor(&self, gl: &Gl, rect: Option<Rect>) {
        if !changed(&self.scissor, rect) {
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

    /// `value::Viewport`
    pub fn set_viewport(&self, gl: &Gl, rect: Rect) {
        if changed(&self.viewport, rect) {
            // SAFETY: the sizes aren't negative.
            unsafe { gl.viewport(rect.0, rect.1, rect.2, rect.3) };
        }
    }

    /// `value::LineWidth`
    pub fn set_line_width(&self, gl: &Gl, width: f32) {
        if changed(&self.line_width, width) {
            // SAFETY: the width is positive (`DrawMode`).
            unsafe { gl.line_width(width) };
        }
    }

    /// `value::PixelStorePack`
    pub fn set_pixel_store_pack_alignment(&self, gl: &Gl, alignment: i32) {
        if changed(&self.pixel_store_pack_alignment, alignment) {
            // SAFETY: an alignment GL defines.
            unsafe { gl.pixel_store_i32(glow::PACK_ALIGNMENT, alignment) };
        }
    }

    /// `value::PixelStoreUnpack`
    pub fn set_pixel_store_unpack_alignment(&self, gl: &Gl, alignment: i32) {
        if changed(&self.pixel_store_unpack_alignment, alignment) {
            // SAFETY: an alignment GL defines.
            unsafe { gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, alignment) };
        }
    }

    /// `Context::clear`: clears the buffers given a value, all of each: the
    /// scissor test and the buffers' write masks would restrict it.
    pub fn clear(&self, gl: &Gl, color: Option<Color>, depth: Option<f32>, stencil: Option<i32>) {
        let mut mask = 0;
        if let Some(color) = color {
            mask |= glow::COLOR_BUFFER_BIT;
            if changed(&self.clear_color, color) {
                // SAFETY: no preconditions.
                unsafe { gl.clear_color(color.r, color.g, color.b, color.a) };
            }
            self.set_color_mask(gl, Mask { r: true, g: true, b: true, a: true });
        }
        if let Some(depth) = depth {
            mask |= glow::DEPTH_BUFFER_BIT;
            if changed(&self.clear_depth, depth) {
                // SAFETY: no preconditions.
                unsafe { gl.clear_depth_f32(depth) };
            }
            self.set_depth_mask(gl, true);
        }
        if let Some(stencil) = stencil {
            mask |= glow::STENCIL_BUFFER_BIT;
            if changed(&self.clear_stencil, stencil) {
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
    pub fn forget_program(&self, program: ProgramID) {
        if self.program.get() == Some(Some(program)) {
            self.program.set(None);
        }
    }

    pub fn forget_vertex_array(&self, vertex_array: VertexArrayID) {
        if self.vertex_array.get() == Some(Some(vertex_array)) {
            self.vertex_array.set(None);
        }
    }

    pub fn forget_framebuffer(&self, framebuffer: FramebufferID) {
        if self.framebuffer.get() == Some(Some(framebuffer)) {
            self.framebuffer.set(None);
        }
    }

    pub fn forget_texture(&self, texture: TextureID) {
        for cell in &self.textures {
            if cell.get() == Some(Some(texture)) {
                cell.set(None);
            }
        }
    }
}
