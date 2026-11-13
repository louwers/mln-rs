// The observer methods take `&self` (C++: non-const virtuals): the style
// holds an `Rc<dyn Observer>`, and the map's implementation uses interior
// mutability. `LayerObserver` / `LightObserver` are implemented by
// `Style::Impl` itself in C++; here they are the `on_layer_changed` /
// `on_light_changed` methods the `LayerRefMut` / `LightRefMut` guards call
// (see layer.rs).

use mln_util::error::Error;

use crate::source::Source;
use crate::sprite::Sprite;
use crate::style::StyleLoadError;

pub trait SourceObserver {
    fn on_source_loaded(&self, _source: &dyn Source) {}
    fn on_source_changed(&self, _source: &dyn Source) {}
    fn on_source_error(&self, _source: &dyn Source, _error: Error) {}
    fn on_source_description_changed(&self, _source: &dyn Source) {}
}

pub trait Observer: SourceObserver {
    fn on_style_loading(&self) {}
    fn on_style_loaded(&self) {}
    fn on_update(&self) {}
    fn on_style_error(&self, _error: StyleLoadError) {}
    fn on_resource_error(&self, _error: Error) {}

    fn on_sprite_loaded(&self, _sprite: Option<&Sprite>) {}
    fn on_sprite_error(&self, _sprite: Option<&Sprite>, _error: Error) {}
    fn on_sprite_requested(&self, _sprite: Option<&Sprite>) {}

    /// Port-only (glfw): a `loadURL` response arrived. C++'s response
    /// lambda captures the `Style::Impl` and parses at once; in Rust the
    /// owner of the style must call `Impl::process_style_responses` (the
    /// Map does it when this fires).
    fn on_style_response(&self) {}
}

/// The default `Observer nullObserver`.
#[derive(Default)]
pub(crate) struct NullObserver;

impl SourceObserver for NullObserver {}
impl Observer for NullObserver {}
