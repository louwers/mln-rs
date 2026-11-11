#![cfg(target_vendor = "apple")]

// Ports the `MLN_RENDER_BACKEND_METAL` part of test/util/hash.test.cpp; the
// rest is in crates/mln_gfx/tests/gtest_hash.rs.
//
// The port has no Metal source for the instanced fill extrusion and location
// indicator shaders (`shader_source` returns `None`), so the test checks the
// listed shaders that have one.

use std::collections::BTreeSet;

use mln_gfx::shaders::shader_source::BuiltIn;
use mln_gfx::util_hash::order_independent_hash;
use mln_mtl::shaders::shader_source::shader_source;

/// `std::next_permutation`: rearranges into the next lexicographically
/// greater permutation; false (and sorted ascending) after the last one.
fn next_permutation(values: &mut [u64]) -> bool {
    if values.len() < 2 {
        return false;
    }
    let mut i = values.len() - 1;
    while i > 0 && values[i - 1] >= values[i] {
        i -= 1;
    }
    if i == 0 {
        values.reverse();
        return false;
    }
    let mut j = values.len() - 1;
    while values[j] <= values[i - 1] {
        j -= 1;
    }
    values.swap(i - 1, j);
    values[i..].reverse();
    true
}

/// Ensure that no combination of attributes used by a shader definition produce a hash conflict
fn check_shader_hashes(shader_id: BuiltIn) {
    let Some(source) = shader_source(shader_id) else {
        return;
    };
    let attributes: Vec<u64> =
        source.attributes.iter().map(|attrib| attrib.id as u64).collect::<BTreeSet<_>>().into_iter().collect();

    let mut subset_hashes = BTreeSet::new();
    // Every non-empty subset, one bit of `mask` per attribute.
    for mask in 1u32..1 << attributes.len() {
        let mut subset: Vec<u64> =
            attributes.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, &id)| id).collect();
        let mut first_hash = None;
        loop {
            let hash = order_independent_hash(subset.iter().copied());
            match first_hash {
                // All permutations of the subset must yield the same hash key as the first
                Some(first) => assert_eq!(first, hash, "{}: {subset:?}", source.name),
                None => {
                    first_hash = Some(hash);
                    // All subset hashes must be unique
                    assert!(subset_hashes.insert(hash), "{}: {subset:?}", source.name);
                }
            }
            if !next_permutation(&mut subset) {
                break;
            }
        }
    }
    assert_eq!(1 << attributes.len(), 1 + subset_hashes.len(), "{}", source.name);
}

// gtest: OrderIndependentHash.Shaders
#[test]
fn shaders() {
    for shader_id in [
        BuiltIn::BackgroundShader,
        BuiltIn::BackgroundPatternShader,
        BuiltIn::CircleShader,
        BuiltIn::CollisionBoxShader,
        BuiltIn::CollisionCircleShader,
        BuiltIn::DebugShader,
        BuiltIn::FillShader,
        BuiltIn::FillOutlineShader,
        BuiltIn::FillPatternShader,
        BuiltIn::FillOutlinePatternShader,
        BuiltIn::FillOutlineTriangulatedShader,
        BuiltIn::FillExtrusionShader,
        BuiltIn::FillExtrusionInstancedShader,
        BuiltIn::FillExtrusionPatternShader,
        BuiltIn::FillExtrusionPatternInstancedShader,
        BuiltIn::HeatmapShader,
        BuiltIn::HeatmapTextureShader,
        BuiltIn::HillshadePrepareShader,
        BuiltIn::HillshadeShader,
        BuiltIn::LineShader,
        BuiltIn::LineGradientShader,
        BuiltIn::LinePatternShader,
        BuiltIn::LineSDFShader,
        BuiltIn::LocationIndicatorShader,
        BuiltIn::LocationIndicatorTexturedShader,
        BuiltIn::RasterShader,
        BuiltIn::SymbolIconShader,
        BuiltIn::SymbolSDFShader,
        BuiltIn::SymbolTextAndIconShader,
    ] {
        check_shader_hashes(shader_id);
    }
}
