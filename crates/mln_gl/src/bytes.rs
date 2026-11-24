// The calls that hand GL bytes of the process's memory, or have it fill
// them.
//
// In the browser, WebGL takes the bytes as a typed array, a view of the wasm
// instance's memory, which the memory's growth detaches. glow makes the view
// in one call into JavaScript and passes it to WebGL in another, and taking
// the view as a JS value in between can grow the memory: wasm-bindgen keeps
// the free slots of its table of JS values in a vector, which grows with the
// table. WebGL then gets an array of no bytes. The calls here go through
// web-sys's instead, which take a slice: their bindings make the view inside
// the call into JavaScript that hands it to WebGL.

#[cfg(not(target_family = "wasm"))]
use glow::HasContext;

#[cfg(not(target_family = "wasm"))]
use crate::types::Gl;

/// What hands a context bytes: its GL functions, or in the browser its
/// `WebGL2RenderingContext`.
#[derive(Clone, Copy)]
pub(crate) struct Bytes<'a> {
    #[cfg(not(target_family = "wasm"))]
    gl: &'a Gl,
    #[cfg(target_family = "wasm")]
    webgl2: &'a web_sys::WebGl2RenderingContext,
}

#[cfg(not(target_family = "wasm"))]
impl<'a> Bytes<'a> {
    pub(crate) fn new(gl: &'a Gl) -> Bytes<'a> {
        Bytes { gl }
    }

    /// `glBufferData` of `bytes`.
    ///
    /// # Safety
    ///
    /// The context is current, with a buffer bound to `target`.
    pub(crate) unsafe fn buffer_data(self, target: u32, bytes: &[u8], usage: u32) {
        // SAFETY: the caller's.
        unsafe { self.gl.buffer_data_u8_slice(target, bytes, usage) };
    }

    /// `glBufferSubData` of `bytes`, from `offset`.
    ///
    /// # Safety
    ///
    /// The context is current, with a buffer bound to `target` that holds
    /// the bytes written.
    pub(crate) unsafe fn buffer_sub_data(self, target: u32, offset: i32, bytes: &[u8]) {
        // SAFETY: the caller's.
        unsafe { self.gl.buffer_sub_data_u8_slice(target, offset, bytes) };
    }

    /// `glTexSubImage2D` of level 0 of the 2D texture bound to the active
    /// unit: `bytes` are the texels of `format` and `type_` of the `width` x
    /// `height` region at `x`, `y`.
    ///
    /// # Safety
    ///
    /// The context is current, and `bytes` holds the region's texels, which
    /// are inside the texture.
    pub(crate) unsafe fn tex_sub_image_2d(
        self,
        [x, y, width, height]: [i32; 4],
        format: u32,
        type_: u32,
        bytes: &[u8],
    ) {
        let texels = glow::PixelUnpackData::Slice(Some(bytes));
        // SAFETY: the caller's.
        unsafe { self.gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, x, y, width, height, format, type_, texels) };
    }

    /// `glReadPixels` of the `width` x `height` region at `x`, `y` of the
    /// bound framebuffer, as RGBA bytes into `pixels`.
    ///
    /// # Safety
    ///
    /// The context is current, and `pixels` holds the region's texels as
    /// the pack alignment lays them out.
    pub(crate) unsafe fn read_pixels(self, [x, y, width, height]: [i32; 4], pixels: &mut [u8]) {
        let texels = glow::PixelPackData::Slice(Some(pixels));
        // SAFETY: the caller's.
        unsafe { self.gl.read_pixels(x, y, width, height, glow::RGBA, glow::UNSIGNED_BYTE, texels) };
    }
}

#[cfg(target_family = "wasm")]
impl<'a> Bytes<'a> {
    pub(crate) fn new(webgl2: &'a web_sys::WebGl2RenderingContext) -> Bytes<'a> {
        Bytes { webgl2 }
    }

    /// `bufferData` of `bytes`.
    ///
    /// # Safety
    ///
    /// None in the browser: the signature is the native one.
    pub(crate) unsafe fn buffer_data(self, target: u32, bytes: &[u8], usage: u32) {
        self.webgl2.buffer_data_with_u8_array(target, bytes, usage);
    }

    /// `bufferSubData` of `bytes`, from `offset`.
    ///
    /// # Safety
    ///
    /// None in the browser: the signature is the native one.
    pub(crate) unsafe fn buffer_sub_data(self, target: u32, offset: i32, bytes: &[u8]) {
        self.webgl2.buffer_sub_data_with_i32_and_u8_array(target, offset, bytes);
    }

    /// `texSubImage2D` of level 0 of the 2D texture bound to the active
    /// unit: `bytes` are the texels of `format` and `type_` of the `width` x
    /// `height` region at `x`, `y`. WebGL reads texels of a type wider than
    /// a byte from a typed array of that type, which is made over a copy of
    /// the bytes: one over the instance's memory would need them aligned.
    ///
    /// # Safety
    ///
    /// None in the browser: the signature is the native one.
    pub(crate) unsafe fn tex_sub_image_2d(
        self,
        [x, y, width, height]: [i32; 4],
        format: u32,
        type_: u32,
        bytes: &[u8],
    ) {
        use web_sys::WebGl2RenderingContext as Gl;
        let copy = || {
            let copy = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
            copy.copy_from(bytes);
            copy.buffer()
        };
        let texels: Option<js_sys::Object> = match type_ {
            Gl::UNSIGNED_SHORT
            | Gl::UNSIGNED_SHORT_5_6_5
            | Gl::UNSIGNED_SHORT_5_5_5_1
            | Gl::UNSIGNED_SHORT_4_4_4_4
            | Gl::HALF_FLOAT => Some(js_sys::Uint16Array::new(&copy()).into()),
            Gl::UNSIGNED_INT
            | Gl::UNSIGNED_INT_5_9_9_9_REV
            | Gl::UNSIGNED_INT_2_10_10_10_REV
            | Gl::UNSIGNED_INT_10F_11F_11F_REV
            | Gl::UNSIGNED_INT_24_8 => Some(js_sys::Uint32Array::new(&copy()).into()),
            Gl::FLOAT => Some(js_sys::Float32Array::new(&copy()).into()),
            _ => None,
        };
        let (target, gl) = (Gl::TEXTURE_2D, self.webgl2);
        let written = match &texels {
            Some(texels) => gl.tex_sub_image_2d_with_i32_and_i32_and_u32_and_type_and_opt_array_buffer_view(
                target,
                0,
                x,
                y,
                width,
                height,
                format,
                type_,
                Some(texels),
            ),
            None => gl.tex_sub_image_2d_with_i32_and_i32_and_u32_and_type_and_opt_u8_array(
                target,
                0,
                x,
                y,
                width,
                height,
                format,
                type_,
                Some(bytes),
            ),
        };
        // What WebGL throws, for arguments it doesn't take.
        written.unwrap_or_else(|error| wasm_bindgen::throw_val(error));
    }

    /// `readPixels` of the `width` x `height` region at `x`, `y` of the
    /// bound framebuffer, as RGBA bytes into `pixels`.
    ///
    /// # Safety
    ///
    /// None in the browser: the signature is the native one.
    pub(crate) unsafe fn read_pixels(self, [x, y, width, height]: [i32; 4], pixels: &mut [u8]) {
        use web_sys::WebGl2RenderingContext as Gl;
        self.webgl2
            .read_pixels_with_opt_u8_array(x, y, width, height, Gl::RGBA, Gl::UNSIGNED_BYTE, Some(pixels))
            .unwrap_or_else(|error| wasm_bindgen::throw_val(error));
    }
}
