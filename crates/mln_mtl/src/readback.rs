// Port-only: offscreen and surface targets read their image back through a
// blit into shared memory (C++ reads the texture with `getBytes`, which on
// macOS needs a synchronized managed texture).

use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLOrigin, MTLSize, MTLTexture,
};

/// Encodes a copy of all of `texture` into `buffer`, rows `bytes_per_row`
/// apart. False if the command buffer has no blit encoder to give.
pub(crate) fn encode_texture_copy(
    command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
    texture: &ProtocolObject<dyn MTLTexture>,
    bytes_per_row: usize,
    buffer: &ProtocolObject<dyn MTLBuffer>,
) -> bool {
    let (width, height) = (texture.width(), texture.height());
    debug_assert!(buffer.length() >= bytes_per_row * height);
    let Some(blit) = command_buffer.blitCommandEncoder() else {
        return false;
    };
    // SAFETY: the region is the whole texture and the buffer holds it.
    unsafe {
        blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
            texture,
            0,
            0,
            MTLOrigin { x: 0, y: 0, z: 0 },
            MTLSize { width, height, depth: 1 },
            buffer,
            0,
            bytes_per_row,
            bytes_per_row * height,
        );
    }
    blit.endEncoding();
    true
}

/// The first `length` bytes of a shared buffer that no GPU work writes.
pub(crate) fn buffer_bytes(buffer: &ProtocolObject<dyn MTLBuffer>, length: usize) -> &[u8] {
    debug_assert!(buffer.length() >= length);
    // SAFETY: the buffer is shared memory of at least `length` bytes, and no
    // GPU work writes it while the slice lives.
    unsafe { std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), length) }
}
