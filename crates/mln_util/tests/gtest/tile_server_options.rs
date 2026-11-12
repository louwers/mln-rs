// Ports test/util/tile_server_options.test.cpp.

use mln_util::tile_server_options::TileServerOptions;

// gtest: TileServerOptions.CopyAssignment
#[test]
fn copy_assignment() {
    let options = TileServerOptions::mapbox_configuration();
    #[allow(clippy::redundant_clone)]
    let options_copy = options.clone();
    assert!(!std::ptr::eq(&raw const options_copy, &raw const options));
    assert_eq!(options_copy.base_url(), options.base_url());
}

// gtest: TileServerOptions.CopyConstructor
#[test]
fn copy_constructor() {
    let options = TileServerOptions::mapbox_configuration();
    let options_copy = TileServerOptions::clone(&options);
    assert!(!std::ptr::eq(&raw const options_copy, &raw const options));
    assert_eq!(options_copy.base_url(), options.base_url());
}
