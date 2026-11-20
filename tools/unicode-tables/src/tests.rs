//! Fails when `mln_text`'s tables differ from what the tool writes from the
//! UCD files in `ucd/`, or when those aren't of unicode-bidi's Unicode version.

use std::path::Path;

use super::tables;

#[test]
#[cfg_attr(
    any(target_os = "android", target_env = "ohos"),
    ignore = "reads crates/mln_text, which the device's copy of this package lacks"
)]
fn the_tables_are_up_to_date() {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let tables = tables(&Path::new(&manifest_dir).join("../.."))
        .unwrap_or_else(|error| panic!("{error}: run `mise run generate:unicode-tables`"));
    for (path, table) in tables {
        let checked_in = std::fs::read_to_string(&path).unwrap();
        assert!(checked_in == table, "{} is out of date: run `mise run generate:unicode-tables`", path.display());
    }
}
