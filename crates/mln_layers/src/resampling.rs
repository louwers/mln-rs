//! How a layer's `resampling` filters the image it draws.

use mln_gfx::types::TextureFilterType;
use mln_style_types::ResamplingType;

/// The filter of `resampling`.
pub(crate) fn filter(resampling: ResamplingType) -> TextureFilterType {
    match resampling {
        ResamplingType::Linear => TextureFilterType::Linear,
        ResamplingType::Nearest => TextureFilterType::Nearest,
    }
}
