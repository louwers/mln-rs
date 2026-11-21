//! Fails when the checkout's counts differ from `baseline.toml`, as
//! `mise run ratchet` does.

use std::path::Path;

use super::{read_baseline, scan};

#[test]
#[cfg_attr(
    any(target_os = "android", target_env = "ohos"),
    ignore = "scans the checkout, which the device's copy of this package lacks"
)]
fn the_baseline_is_up_to_date() {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let manifest_dir = Path::new(&manifest_dir);
    let baseline = read_baseline(&manifest_dir.join("baseline.toml")).unwrap();
    let current = scan(&manifest_dir.join("../..")).unwrap();
    assert!(current == baseline, "the counts differ from tools/ratchet/baseline.toml: run `mise run ratchet`");
}
