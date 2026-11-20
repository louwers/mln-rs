use std::sync::Arc;

use mln_style::layer_type::LayerStyle;

use super::registry;
use crate::custom::{Custom, CustomLayerHost};

struct Host;

impl CustomLayerHost for Host {}

/// A renderer that is leaked with the style data of the style's layers,
/// because its graphics objects are gone, must not keep the host, whose user
/// data the C API releases once the host goes.
#[test]
fn the_style_data_of_a_removed_layer_doesnt_keep_its_host() {
    let host: Arc<dyn CustomLayerHost> = Arc::new(Host);
    let layer = Custom::layer(registry().style(), "custom", Arc::clone(&host));
    let style = LayerStyle::<Custom>::recover(layer.style()).unwrap();
    assert!(style.extra.is(&host));
    assert!(style.extra.get().is_some());
    drop(layer);
    assert!(style.extra.get().is_some(), "the caller still holds the host");
    drop(host);
    assert!(style.extra.get().is_none());
}

/// A copy of a custom layer keeps the host too.
#[test]
fn a_copy_of_a_layer_keeps_its_host() {
    let host: Arc<dyn CustomLayerHost> = Arc::new(Host);
    let layer = Custom::layer(registry().style(), "custom", Arc::clone(&host));
    let copy = layer.clone_ref("copy");
    let style = LayerStyle::<Custom>::recover(copy.style()).unwrap();
    drop((layer, host));
    assert!(style.extra.get().is_some(), "the copy holds the host");
    drop(copy);
    assert!(style.extra.get().is_none());
}
