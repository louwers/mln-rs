// Ports vendor/maplibre-native-base/deps/shelf-pack-cpp/test/test.cpp, test
// by test (same names, same assertions). `Bin*` is `Option<BinRef>`;
// `ptr->field` reads `sprite.bin(ptr)`.

#![expect(
    unused_mut,
    unused_assignments,
    reason = "the tests keep the C++ tests' statements, including bindings they never change and assignments they never read"
)]

use crate::{Bin, BinRef, PackOptions, SHELF_PACK_VERSION, ShelfPack, ShelfPackOptions};

fn b(sprite: &ShelfPack, r: Option<BinRef>) -> Bin {
    *sprite.bin(r.expect("non-null Bin*"))
}

#[test]
fn test_version() {
    let version = SHELF_PACK_VERSION.to_owned();
    assert_ne!(version, "");
}

#[test]
fn test_pack1() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let mut bins: Vec<Bin> = Vec::new();
    let mut results: Vec<BinRef> = Vec::new();
    bins.push(Bin::new(-1, 10, 10));
    bins.push(Bin::new(-1, 10, 10));
    bins.push(Bin::new(-1, 10, 10));
    results = sprite.pack(&mut bins, PackOptions::default());
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(sprite.bin(results[0]).id, 1);
    assert_eq!(sprite.bin(results[0]).x, 0);
    assert_eq!(sprite.bin(results[0]).y, 0);
    assert_eq!(sprite.bin(results[0]).w, 10);
    assert_eq!(sprite.bin(results[0]).h, 10);
    assert_eq!(sprite.bin(results[0]).maxw, 10);
    assert_eq!(sprite.bin(results[0]).maxh, 10);
    //  x: 10, y: 0, w: 10, h: 10
    assert_eq!(sprite.bin(results[1]).id, 2);
    assert_eq!(sprite.bin(results[1]).x, 10);
    assert_eq!(sprite.bin(results[1]).y, 0);
    assert_eq!(sprite.bin(results[1]).w, 10);
    assert_eq!(sprite.bin(results[1]).h, 10);
    assert_eq!(sprite.bin(results[1]).maxw, 10);
    assert_eq!(sprite.bin(results[1]).maxh, 10);
    //  x: 20, y: 0, w: 10, h: 10
    assert_eq!(sprite.bin(results[2]).id, 3);
    assert_eq!(sprite.bin(results[2]).x, 20);
    assert_eq!(sprite.bin(results[2]).y, 0);
    assert_eq!(sprite.bin(results[2]).w, 10);
    assert_eq!(sprite.bin(results[2]).h, 10);
    assert_eq!(sprite.bin(results[2]).maxw, 10);
    assert_eq!(sprite.bin(results[2]).maxh, 10);
}

#[test]
fn test_pack2() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let mut bins: Vec<Bin> = Vec::new();
    let mut results: Vec<BinRef> = Vec::new();
    bins.push(Bin::new(-1, 10, 10));
    bins.push(Bin::new(-1, 10, 15));
    bins.push(Bin::new(-1, 10, 20));
    results = sprite.pack(&mut bins, PackOptions::default());
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(sprite.bin(results[0]).id, 1);
    assert_eq!(sprite.bin(results[0]).x, 0);
    assert_eq!(sprite.bin(results[0]).y, 0);
    assert_eq!(sprite.bin(results[0]).w, 10);
    assert_eq!(sprite.bin(results[0]).h, 10);
    assert_eq!(sprite.bin(results[0]).maxw, 10);
    assert_eq!(sprite.bin(results[0]).maxh, 10);
    //  x: 0, y: 10, w: 10, h: 15
    assert_eq!(sprite.bin(results[1]).id, 2);
    assert_eq!(sprite.bin(results[1]).x, 0);
    assert_eq!(sprite.bin(results[1]).y, 10);
    assert_eq!(sprite.bin(results[1]).w, 10);
    assert_eq!(sprite.bin(results[1]).h, 15);
    assert_eq!(sprite.bin(results[1]).maxw, 10);
    assert_eq!(sprite.bin(results[1]).maxh, 15);
    //  x: 0, y: 25, w: 10, h: 20
    assert_eq!(sprite.bin(results[2]).id, 3);
    assert_eq!(sprite.bin(results[2]).x, 0);
    assert_eq!(sprite.bin(results[2]).y, 25);
    assert_eq!(sprite.bin(results[2]).w, 10);
    assert_eq!(sprite.bin(results[2]).h, 20);
    assert_eq!(sprite.bin(results[2]).maxw, 10);
    assert_eq!(sprite.bin(results[2]).maxh, 20);
}

#[test]
fn test_pack3() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let mut bins: Vec<Bin> = Vec::new();
    let mut results: Vec<BinRef> = Vec::new();
    bins.push(Bin::new(-1, 10, 10));
    bins.push(Bin::new(-1, 10, 15));
    bins.push(Bin::new(-1, 10, 20));
    bins.push(Bin::new(-1, 10, 9));
    results = sprite.pack(&mut bins, PackOptions::default());
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(sprite.bin(results[0]).id, 1);
    assert_eq!(sprite.bin(results[0]).x, 0);
    assert_eq!(sprite.bin(results[0]).y, 0);
    assert_eq!(sprite.bin(results[0]).w, 10);
    assert_eq!(sprite.bin(results[0]).h, 10);
    assert_eq!(sprite.bin(results[0]).maxw, 10);
    assert_eq!(sprite.bin(results[0]).maxh, 10);
    //  x: 0, y: 10, w: 10, h: 15
    assert_eq!(sprite.bin(results[1]).id, 2);
    assert_eq!(sprite.bin(results[1]).x, 0);
    assert_eq!(sprite.bin(results[1]).y, 10);
    assert_eq!(sprite.bin(results[1]).w, 10);
    assert_eq!(sprite.bin(results[1]).h, 15);
    assert_eq!(sprite.bin(results[1]).maxw, 10);
    assert_eq!(sprite.bin(results[1]).maxh, 15);
    //  x: 0, y: 25, w: 10, h: 20
    assert_eq!(sprite.bin(results[2]).id, 3);
    assert_eq!(sprite.bin(results[2]).x, 0);
    assert_eq!(sprite.bin(results[2]).y, 25);
    assert_eq!(sprite.bin(results[2]).w, 10);
    assert_eq!(sprite.bin(results[2]).h, 20);
    assert_eq!(sprite.bin(results[2]).maxw, 10);
    assert_eq!(sprite.bin(results[2]).maxh, 20);
    //  x: 10, y: 0, w: 10, h: 9
    assert_eq!(sprite.bin(results[3]).id, 4);
    assert_eq!(sprite.bin(results[3]).x, 10);
    assert_eq!(sprite.bin(results[3]).y, 0);
    assert_eq!(sprite.bin(results[3]).w, 10);
    assert_eq!(sprite.bin(results[3]).h, 9);
    assert_eq!(sprite.bin(results[3]).maxw, 10);
    assert_eq!(sprite.bin(results[3]).maxh, 10);
}

#[test]
fn test_pack4() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let mut bins: Vec<Bin> = Vec::new();
    let options = PackOptions { in_place: true };
    bins.push(Bin::new(-1, 10, 10));
    bins.push(Bin::new(-1, 10, 10));
    bins.push(Bin::new(-1, 10, 10));
    sprite.pack(&mut bins, options);
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(bins[0].id, 1);
    assert_eq!(bins[0].x, 0);
    assert_eq!(bins[0].y, 0);
    assert_eq!(bins[0].w, 10);
    assert_eq!(bins[0].h, 10);
    //  x: 10, y: 0, w: 10, h: 10
    assert_eq!(bins[1].id, 2);
    assert_eq!(bins[1].x, 10);
    assert_eq!(bins[1].y, 0);
    assert_eq!(bins[1].w, 10);
    assert_eq!(bins[1].h, 10);
    //  x: 20, y: 0, w: 10, h: 10
    assert_eq!(bins[2].id, 3);
    assert_eq!(bins[2].x, 20);
    assert_eq!(bins[2].y, 0);
    assert_eq!(bins[2].w, 10);
    assert_eq!(bins[2].h, 10);
}

#[test]
fn test_pack5() {
    let mut sprite = ShelfPack::new(20, 20, ShelfPackOptions::default());
    let mut bins: Vec<Bin> = Vec::new();
    let mut results: Vec<BinRef> = Vec::new();
    let options = PackOptions { in_place: true };
    bins.push(Bin::new(-1, 10, 10));
    bins.push(Bin::new(-1, 10, 10));
    bins.push(Bin::new(-1, 10, 30)); // should skip
    bins.push(Bin::new(-1, 10, 10));
    results = sprite.pack(&mut bins, options);
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(sprite.bin(results[0]).id, 1);
    assert_eq!(sprite.bin(results[0]).x, 0);
    assert_eq!(sprite.bin(results[0]).y, 0);
    assert_eq!(sprite.bin(results[0]).w, 10);
    assert_eq!(sprite.bin(results[0]).h, 10);
    assert_eq!(sprite.bin(results[0]).maxw, 10);
    assert_eq!(sprite.bin(results[0]).maxh, 10);
    //  x: 10, y: 0, w: 10, h: 10
    assert_eq!(sprite.bin(results[1]).id, 2);
    assert_eq!(sprite.bin(results[1]).x, 10);
    assert_eq!(sprite.bin(results[1]).y, 0);
    assert_eq!(sprite.bin(results[1]).w, 10);
    assert_eq!(sprite.bin(results[1]).h, 10);
    assert_eq!(sprite.bin(results[1]).maxw, 10);
    assert_eq!(sprite.bin(results[1]).maxh, 10);
    //  x: 0, y: 10, w: 10, h: 10
    assert_eq!(sprite.bin(results[2]).id, 4);
    assert_eq!(sprite.bin(results[2]).x, 0);
    assert_eq!(sprite.bin(results[2]).y, 10);
    assert_eq!(sprite.bin(results[2]).w, 10);
    assert_eq!(sprite.bin(results[2]).h, 10);
    assert_eq!(sprite.bin(results[2]).maxw, 10);
    assert_eq!(sprite.bin(results[2]).maxh, 10);
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(bins[0].id, 1);
    assert_eq!(bins[0].x, 0);
    assert_eq!(bins[0].y, 0);
    assert_eq!(bins[0].w, 10);
    assert_eq!(bins[0].h, 10);
    //  x: 10, y: 0, w: 10, h: 10
    assert_eq!(bins[1].id, 2);
    assert_eq!(bins[1].x, 10);
    assert_eq!(bins[1].y, 0);
    assert_eq!(bins[1].w, 10);
    assert_eq!(bins[1].h, 10);
    //  x: -1, y: -1, w: 10, h: 30
    assert_eq!(bins[2].id, -1);
    assert_eq!(bins[2].x, -1);
    assert_eq!(bins[2].y, -1);
    assert_eq!(bins[2].w, 10);
    assert_eq!(bins[2].h, 30);
    //  x: 0, y: 10, w: 10, h: 10
    assert_eq!(bins[3].id, 4);
    assert_eq!(bins[3].x, 0);
    assert_eq!(bins[3].y, 10);
    assert_eq!(bins[3].w, 10);
    assert_eq!(bins[3].h, 10);
}

#[test]
fn test_pack6() {
    let mut bins: Vec<Bin> = Vec::new();
    let mut results: Vec<BinRef> = Vec::new();
    bins.push(Bin::new(-1, 10, 10));
    bins.push(Bin::new(-1, 5, 15));
    bins.push(Bin::new(-1, 25, 15));
    bins.push(Bin::new(-1, 10, 20));
    let options = ShelfPackOptions { auto_resize: true };
    let mut sprite = ShelfPack::new(10, 10, options);
    results = sprite.pack(&mut bins, PackOptions::default());
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(sprite.bin(results[0]).id, 1);
    assert_eq!(sprite.bin(results[0]).x, 0);
    assert_eq!(sprite.bin(results[0]).y, 0);
    assert_eq!(sprite.bin(results[0]).w, 10);
    assert_eq!(sprite.bin(results[0]).h, 10);
    assert_eq!(sprite.bin(results[0]).maxw, 10);
    assert_eq!(sprite.bin(results[0]).maxh, 10);
    //  x: 0, y: 10, w: 5, h: 15
    assert_eq!(sprite.bin(results[1]).id, 2);
    assert_eq!(sprite.bin(results[1]).x, 0);
    assert_eq!(sprite.bin(results[1]).y, 10);
    assert_eq!(sprite.bin(results[1]).w, 5);
    assert_eq!(sprite.bin(results[1]).h, 15);
    assert_eq!(sprite.bin(results[1]).maxw, 5);
    assert_eq!(sprite.bin(results[1]).maxh, 15);
    //  x: 5, y: 10, w: 25, h: 15
    assert_eq!(sprite.bin(results[2]).id, 3);
    assert_eq!(sprite.bin(results[2]).x, 5);
    assert_eq!(sprite.bin(results[2]).y, 10);
    assert_eq!(sprite.bin(results[2]).w, 25);
    assert_eq!(sprite.bin(results[2]).h, 15);
    assert_eq!(sprite.bin(results[2]).maxw, 25);
    assert_eq!(sprite.bin(results[2]).maxh, 15);
    //  x: 0, y: 25, w: 10, h: 20
    assert_eq!(sprite.bin(results[3]).id, 4);
    assert_eq!(sprite.bin(results[3]).x, 0);
    assert_eq!(sprite.bin(results[3]).y, 25);
    assert_eq!(sprite.bin(results[3]).w, 10);
    assert_eq!(sprite.bin(results[3]).h, 20);
    assert_eq!(sprite.bin(results[3]).maxw, 10);
    assert_eq!(sprite.bin(results[3]).maxh, 20);
    // Since shelf-pack doubles width/height when packing bins one by one
    // (first width, then height) this would result in a 50x60 sprite here.
    // But this can be shrunk to a 30x45 sprite.
    assert_eq!(sprite.width(), 30);
    assert_eq!(sprite.height(), 45);
}

#[test]
fn test_pack_one1() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let bin = sprite.pack_one(1000, 10, 10);
    assert_eq!(b(&sprite, bin).id, 1000);
    assert_eq!(b(&sprite, bin).x, 0);
    assert_eq!(b(&sprite, bin).y, 0);
    assert_eq!(b(&sprite, bin).w, 10);
    assert_eq!(b(&sprite, bin).h, 10);
    assert_eq!(b(&sprite, bin).maxw, 10);
    assert_eq!(b(&sprite, bin).maxh, 10);
    assert_eq!(sprite.get_bin(1000), bin);
}

#[test]
fn test_pack_one2() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let bin1 = sprite.pack_one(-1, 10, 10);
    let bin2 = sprite.pack_one(-1, 10, 10);
    assert_eq!(b(&sprite, bin1).id, 1);
    assert_eq!(b(&sprite, bin2).id, 2);
}

#[test]
fn test_pack_one3() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let bin1 = sprite.pack_one(1, 10, 10);
    let bin2 = sprite.pack_one(-1, 10, 10);
    assert_eq!(b(&sprite, bin1).id, 1);
    assert_eq!(b(&sprite, bin2).id, 2);
}

#[test]
fn test_pack_one4() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let bin1 = sprite.pack_one(1000, 10, 10);
    assert_eq!(b(&sprite, bin1).id, 1000);
    assert_eq!(b(&sprite, bin1).x, 0);
    assert_eq!(b(&sprite, bin1).y, 0);
    assert_eq!(b(&sprite, bin1).w, 10);
    assert_eq!(b(&sprite, bin1).h, 10);
    assert_eq!(b(&sprite, bin1).maxw, 10);
    assert_eq!(b(&sprite, bin1).maxh, 10);
    let bin2 = sprite.pack_one(1000, 10, 10);
    assert_eq!(b(&sprite, bin2).id, 1000);
    assert_eq!(b(&sprite, bin2).x, 0);
    assert_eq!(b(&sprite, bin2).y, 0);
    assert_eq!(b(&sprite, bin2).w, 10);
    assert_eq!(b(&sprite, bin2).h, 10);
    assert_eq!(b(&sprite, bin2).maxw, 10);
    assert_eq!(b(&sprite, bin2).maxh, 10);
    assert_eq!(bin1, bin2); // same bin
}

#[test]
fn test_pack_one5() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let bin1 = sprite.pack_one(-1, 10, 10);
    let bin2 = sprite.pack_one(-1, 10, 10);
    let bin3 = sprite.pack_one(-1, 10, 10);
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin1).id, 1);
    assert_eq!(b(&sprite, bin1).x, 0);
    assert_eq!(b(&sprite, bin1).y, 0);
    assert_eq!(b(&sprite, bin1).w, 10);
    assert_eq!(b(&sprite, bin1).h, 10);
    assert_eq!(b(&sprite, bin1).maxw, 10);
    assert_eq!(b(&sprite, bin1).maxh, 10);
    //  x: 10, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin2).id, 2);
    assert_eq!(b(&sprite, bin2).x, 10);
    assert_eq!(b(&sprite, bin2).y, 0);
    assert_eq!(b(&sprite, bin2).w, 10);
    assert_eq!(b(&sprite, bin2).h, 10);
    assert_eq!(b(&sprite, bin2).maxw, 10);
    assert_eq!(b(&sprite, bin2).maxh, 10);
    //  x: 20, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin3).id, 3);
    assert_eq!(b(&sprite, bin3).x, 20);
    assert_eq!(b(&sprite, bin3).y, 0);
    assert_eq!(b(&sprite, bin3).w, 10);
    assert_eq!(b(&sprite, bin3).h, 10);
    assert_eq!(b(&sprite, bin3).maxw, 10);
    assert_eq!(b(&sprite, bin3).maxh, 10);
}

#[test]
fn test_pack_one6() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let bin1 = sprite.pack_one(-1, 10, 10);
    let bin2 = sprite.pack_one(-1, 10, 15);
    let bin3 = sprite.pack_one(-1, 10, 20);
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin1).id, 1);
    assert_eq!(b(&sprite, bin1).x, 0);
    assert_eq!(b(&sprite, bin1).y, 0);
    assert_eq!(b(&sprite, bin1).w, 10);
    assert_eq!(b(&sprite, bin1).h, 10);
    assert_eq!(b(&sprite, bin1).maxw, 10);
    assert_eq!(b(&sprite, bin1).maxh, 10);
    //  x: 0, y: 10, w: 10, h: 15
    assert_eq!(b(&sprite, bin2).id, 2);
    assert_eq!(b(&sprite, bin2).x, 0);
    assert_eq!(b(&sprite, bin2).y, 10);
    assert_eq!(b(&sprite, bin2).w, 10);
    assert_eq!(b(&sprite, bin2).h, 15);
    assert_eq!(b(&sprite, bin2).maxw, 10);
    assert_eq!(b(&sprite, bin2).maxh, 15);
    //  x: 0, y: 25, w: 10, h: 20
    assert_eq!(b(&sprite, bin3).id, 3);
    assert_eq!(b(&sprite, bin3).x, 0);
    assert_eq!(b(&sprite, bin3).y, 25);
    assert_eq!(b(&sprite, bin3).w, 10);
    assert_eq!(b(&sprite, bin3).h, 20);
    assert_eq!(b(&sprite, bin3).maxw, 10);
    assert_eq!(b(&sprite, bin3).maxh, 20);
}

#[test]
fn test_pack_one7() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let bin1 = sprite.pack_one(-1, 10, 10);
    let bin2 = sprite.pack_one(-1, 10, 15);
    let bin3 = sprite.pack_one(-1, 10, 20);
    let bin4 = sprite.pack_one(-1, 10, 9);
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin1).id, 1);
    assert_eq!(b(&sprite, bin1).x, 0);
    assert_eq!(b(&sprite, bin1).y, 0);
    assert_eq!(b(&sprite, bin1).w, 10);
    assert_eq!(b(&sprite, bin1).h, 10);
    assert_eq!(b(&sprite, bin1).maxw, 10);
    assert_eq!(b(&sprite, bin1).maxh, 10);
    //  x: 0, y: 10, w: 10, h: 15
    assert_eq!(b(&sprite, bin2).id, 2);
    assert_eq!(b(&sprite, bin2).x, 0);
    assert_eq!(b(&sprite, bin2).y, 10);
    assert_eq!(b(&sprite, bin2).w, 10);
    assert_eq!(b(&sprite, bin2).h, 15);
    assert_eq!(b(&sprite, bin2).maxw, 10);
    assert_eq!(b(&sprite, bin2).maxh, 15);
    //  x: 0, y: 25, w: 10, h: 20
    assert_eq!(b(&sprite, bin3).id, 3);
    assert_eq!(b(&sprite, bin3).x, 0);
    assert_eq!(b(&sprite, bin3).y, 25);
    assert_eq!(b(&sprite, bin3).w, 10);
    assert_eq!(b(&sprite, bin3).h, 20);
    assert_eq!(b(&sprite, bin3).maxw, 10);
    assert_eq!(b(&sprite, bin3).maxh, 20);
    //  x: 10, y: 0, w: 10, h: 9
    assert_eq!(b(&sprite, bin4).id, 4);
    assert_eq!(b(&sprite, bin4).x, 10);
    assert_eq!(b(&sprite, bin4).y, 0);
    assert_eq!(b(&sprite, bin4).w, 10);
    assert_eq!(b(&sprite, bin4).h, 9);
    assert_eq!(b(&sprite, bin4).maxw, 10);
    assert_eq!(b(&sprite, bin4).maxh, 10);
}

#[test]
fn test_pack_one8() {
    let mut sprite = ShelfPack::new(10, 10, ShelfPackOptions::default());
    let bin1 = sprite.pack_one(-1, 10, 10);
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin1).id, 1);
    assert_eq!(b(&sprite, bin1).x, 0);
    assert_eq!(b(&sprite, bin1).y, 0);
    assert_eq!(b(&sprite, bin1).w, 10);
    assert_eq!(b(&sprite, bin1).h, 10);
    assert_eq!(b(&sprite, bin1).maxw, 10);
    assert_eq!(b(&sprite, bin1).maxh, 10);
    let bin2 = sprite.pack_one(-1, 10, 10);
    assert_eq!(bin2, None);
}

#[test]
fn test_pack_one9() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    sprite.pack_one(1, 10, 10);
    sprite.pack_one(2, 10, 10);
    sprite.pack_one(3, 10, 10);
    let bin2 = sprite.get_bin(2);
    sprite.unref(bin2.unwrap());
    let bin4 = sprite.pack_one(4, 10, 10);
    //  x: 10, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin4).id, 4);
    assert_eq!(b(&sprite, bin4).x, 10);
    assert_eq!(b(&sprite, bin4).y, 0);
    assert_eq!(b(&sprite, bin4).w, 10);
    assert_eq!(b(&sprite, bin4).h, 10);
    assert_eq!(b(&sprite, bin4).maxw, 10);
    assert_eq!(b(&sprite, bin4).maxh, 10);
    assert_eq!(bin4, bin2); // reused bin2
}

#[test]
fn test_pack_one10() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    sprite.pack_one(1, 10, 10);
    sprite.pack_one(2, 10, 15);
    sprite.pack_one(3, 10, 20);
    let bin2 = sprite.get_bin(2);
    sprite.unref(sprite.get_bin(3).unwrap());
    sprite.unref(sprite.get_bin(2).unwrap());
    sprite.unref(sprite.get_bin(1).unwrap());
    let bin4 = sprite.pack_one(4, 10, 13);
    //  x: 0, y: 10, w: 10, h: 13
    assert_eq!(b(&sprite, bin4).id, 4);
    assert_eq!(b(&sprite, bin4).x, 0);
    assert_eq!(b(&sprite, bin4).y, 10);
    assert_eq!(b(&sprite, bin4).w, 10);
    assert_eq!(b(&sprite, bin4).h, 13);
    assert_eq!(b(&sprite, bin4).maxw, 10);
    assert_eq!(b(&sprite, bin4).maxh, 15);
    assert_eq!(bin4, bin2); // reused bin2
}

#[test]
fn test_pack_one11() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    sprite.pack_one(1, 10, 10);
    sprite.pack_one(2, 10, 15);
    let bin2 = sprite.get_bin(2);
    sprite.unref(bin2.unwrap());
    let bin3 = sprite.pack_one(3, 10, 10);
    //  x: 10, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin3).id, 3);
    assert_eq!(b(&sprite, bin3).x, 10);
    assert_eq!(b(&sprite, bin3).y, 0);
    assert_eq!(b(&sprite, bin3).w, 10);
    assert_eq!(b(&sprite, bin3).h, 10);
    assert_eq!(b(&sprite, bin3).maxw, 10);
    assert_eq!(b(&sprite, bin3).maxh, 10);
    assert_ne!(bin3, bin2); // did not reuse bin2
}

#[test]
fn test_pack_one12() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    sprite.pack_one(1, 10, 10);
    let bin2 = sprite.pack_one(2, 10, 15);
    sprite.unref(bin2.unwrap());
    let bin3 = sprite.pack_one(3, 10, 13);
    //  x: 0, y: 10, w: 10, h: 13
    assert_eq!(b(&sprite, bin3).id, 3);
    assert_eq!(b(&sprite, bin3).x, 0);
    assert_eq!(b(&sprite, bin3).y, 10);
    assert_eq!(b(&sprite, bin3).w, 10);
    assert_eq!(b(&sprite, bin3).h, 13);
    assert_eq!(b(&sprite, bin3).maxw, 10);
    assert_eq!(b(&sprite, bin3).maxh, 15);
    assert_eq!(bin3, bin2); // reused bin2
    sprite.unref(bin3.unwrap());
    let bin4 = sprite.pack_one(4, 10, 14);
    //  x: 0, y: 10, w: 10, h: 14
    assert_eq!(b(&sprite, bin4).id, 4);
    assert_eq!(b(&sprite, bin4).x, 0);
    assert_eq!(b(&sprite, bin4).y, 10);
    assert_eq!(b(&sprite, bin4).w, 10);
    assert_eq!(b(&sprite, bin4).h, 14);
    assert_eq!(b(&sprite, bin4).maxw, 10);
    assert_eq!(b(&sprite, bin4).maxh, 15);
    assert_eq!(bin4, bin2); // reused bin2
    assert_eq!(bin4, bin3); // reused bin3
}

#[test]
fn test_get_bin1() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let bin = sprite.get_bin(1);
    assert_eq!(bin, None);
}

#[test]
fn test_get_bin2() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let bin = sprite.pack_one(1, 10, 10);
    let bin1 = sprite.get_bin(1);
    assert_eq!(bin, bin1);
}

#[test]
fn test_ref() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let bin1 = sprite.pack_one(1, 10, 10);
    assert_eq!(b(&sprite, bin1).refcount(), 1);
    assert_eq!(sprite.ref_(bin1.unwrap()), 2);
    assert_eq!(b(&sprite, bin1).refcount(), 2);
    let bin2 = sprite.pack_one(2, 10, 10);
    assert_eq!(b(&sprite, bin2).refcount(), 1);
    assert_eq!(sprite.ref_(bin2.unwrap()), 2);
    assert_eq!(b(&sprite, bin2).refcount(), 2);
    let bin3 = sprite.pack_one(3, 10, 15);
    assert_eq!(b(&sprite, bin3).refcount(), 1);
    assert_eq!(sprite.ref_(bin3.unwrap()), 2);
    assert_eq!(b(&sprite, bin3).refcount(), 2);
}

#[test]
fn test_unref1() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    // setup
    let bin1 = sprite.pack_one(1, 10, 10);
    sprite.ref_(bin1.unwrap());
    let bin2 = sprite.pack_one(2, 10, 10);
    sprite.ref_(bin2.unwrap());
    let bin3 = sprite.pack_one(3, 10, 15);
    sprite.ref_(bin3.unwrap());
    assert_eq!(b(&sprite, bin1).refcount(), 2);
    assert_eq!(b(&sprite, bin2).refcount(), 2);
    assert_eq!(b(&sprite, bin3).refcount(), 2);
    assert_eq!(sprite.unref(bin3.unwrap()), 1);
    assert_eq!(b(&sprite, bin3).refcount(), 1);
    assert_eq!(sprite.unref(bin3.unwrap()), 0);
    assert_eq!(b(&sprite, bin3).refcount(), 0);
    assert_eq!(sprite.get_bin(3), None);
    assert_eq!(sprite.unref(bin2.unwrap()), 1);
    assert_eq!(b(&sprite, bin2).refcount(), 1);
    assert_eq!(sprite.unref(bin2.unwrap()), 0);
    assert_eq!(b(&sprite, bin2).refcount(), 0);
    assert_eq!(sprite.get_bin(2), None);
}

#[test]
fn test_unref2() {
    let mut sprite = ShelfPack::new(64, 64, ShelfPackOptions::default());
    let bin = sprite.pack_one(1, 10, 10);
    assert_eq!(sprite.unref(bin.unwrap()), 0);
    assert_eq!(b(&sprite, bin).refcount(), 0);
    assert_eq!(sprite.unref(bin.unwrap()), 0); // still 0
    assert_eq!(b(&sprite, bin).refcount(), 0); // still 0
}

#[test]
fn test_clear() {
    let mut sprite = ShelfPack::new(10, 10, ShelfPackOptions::default());
    let bin1 = sprite.pack_one(-1, 10, 10);
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin1).id, 1);
    assert_eq!(b(&sprite, bin1).x, 0);
    assert_eq!(b(&sprite, bin1).y, 0);
    assert_eq!(b(&sprite, bin1).w, 10);
    assert_eq!(b(&sprite, bin1).h, 10);
    assert_eq!(b(&sprite, bin1).maxw, 10);
    assert_eq!(b(&sprite, bin1).maxh, 10);
    let bin2 = sprite.pack_one(-1, 10, 10);
    assert_eq!(bin2, None);
    sprite.clear();
    let bin3 = sprite.pack_one(-1, 10, 10);
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin3).id, 1); // clear resets max id
    assert_eq!(b(&sprite, bin3).x, 0);
    assert_eq!(b(&sprite, bin3).y, 0);
    assert_eq!(b(&sprite, bin3).w, 10);
    assert_eq!(b(&sprite, bin3).h, 10);
    assert_eq!(b(&sprite, bin3).maxw, 10);
    assert_eq!(b(&sprite, bin3).maxh, 10);
}

#[test]
fn test_shrink() {
    let mut sprite = ShelfPack::new(20, 20, ShelfPackOptions::default());
    sprite.pack_one(-1, 10, 5);
    assert_eq!(sprite.width(), 20);
    assert_eq!(sprite.height(), 20);
    sprite.shrink();
    assert_eq!(sprite.width(), 10);
    assert_eq!(sprite.height(), 5);
}

#[test]
fn test_resize1() {
    let mut sprite = ShelfPack::new(10, 10, ShelfPackOptions::default());
    let bin1 = sprite.pack_one(1, 10, 10);
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin1).id, 1);
    assert_eq!(b(&sprite, bin1).x, 0);
    assert_eq!(b(&sprite, bin1).y, 0);
    assert_eq!(b(&sprite, bin1).w, 10);
    assert_eq!(b(&sprite, bin1).h, 10);
    assert_eq!(b(&sprite, bin1).maxw, 10);
    assert_eq!(b(&sprite, bin1).maxh, 10);
    assert!(sprite.resize(20, 10)); // grow width
    assert_eq!(sprite.width(), 20);
    assert_eq!(sprite.height(), 10);
    let bin2 = sprite.pack_one(2, 10, 10);
    //  x: 10, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin2).id, 2);
    assert_eq!(b(&sprite, bin2).x, 10);
    assert_eq!(b(&sprite, bin2).y, 0);
    assert_eq!(b(&sprite, bin2).w, 10);
    assert_eq!(b(&sprite, bin2).h, 10);
    assert_eq!(b(&sprite, bin2).maxw, 10);
    assert_eq!(b(&sprite, bin2).maxh, 10);
    assert!(sprite.resize(20, 20)); // grow height
    assert_eq!(sprite.width(), 20);
    assert_eq!(sprite.height(), 20);
    let bin3 = sprite.pack_one(3, 10, 10);
    //  x: 0, y: 10, w: 10, h: 10
    assert_eq!(b(&sprite, bin3).id, 3);
    assert_eq!(b(&sprite, bin3).x, 0);
    assert_eq!(b(&sprite, bin3).y, 10);
    assert_eq!(b(&sprite, bin3).w, 10);
    assert_eq!(b(&sprite, bin3).h, 10);
    assert_eq!(b(&sprite, bin3).maxw, 10);
    assert_eq!(b(&sprite, bin3).maxh, 10);
}

#[test]
fn test_resize2() {
    let options = ShelfPackOptions { auto_resize: true };
    let mut sprite = ShelfPack::new(10, 10, options);
    let bin1 = sprite.pack_one(1, 10, 10);
    //  x: 0, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin1).id, 1);
    assert_eq!(b(&sprite, bin1).x, 0);
    assert_eq!(b(&sprite, bin1).y, 0);
    assert_eq!(b(&sprite, bin1).w, 10);
    assert_eq!(b(&sprite, bin1).h, 10);
    assert_eq!(b(&sprite, bin1).maxw, 10);
    assert_eq!(b(&sprite, bin1).maxh, 10);
    assert_eq!(sprite.width(), 10);
    assert_eq!(sprite.height(), 10);
    let bin2 = sprite.pack_one(2, 10, 10);
    //  x: 10, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin2).id, 2);
    assert_eq!(b(&sprite, bin2).x, 10);
    assert_eq!(b(&sprite, bin2).y, 0);
    assert_eq!(b(&sprite, bin2).w, 10);
    assert_eq!(b(&sprite, bin2).h, 10);
    assert_eq!(b(&sprite, bin2).maxw, 10);
    assert_eq!(b(&sprite, bin2).maxh, 10);
    assert_eq!(sprite.width(), 20);
    assert_eq!(sprite.height(), 10);
    let bin3 = sprite.pack_one(3, 10, 10);
    //  x: 0, y: 10, w: 10, h: 10
    assert_eq!(b(&sprite, bin3).id, 3);
    assert_eq!(b(&sprite, bin3).x, 0);
    assert_eq!(b(&sprite, bin3).y, 10);
    assert_eq!(b(&sprite, bin3).w, 10);
    assert_eq!(b(&sprite, bin3).h, 10);
    assert_eq!(b(&sprite, bin3).maxw, 10);
    assert_eq!(b(&sprite, bin3).maxh, 10);
    assert_eq!(sprite.width(), 20);
    assert_eq!(sprite.height(), 20);
    let bin4 = sprite.pack_one(4, 10, 10);
    //  x: 10, y: 10, w: 10, h: 10
    assert_eq!(b(&sprite, bin4).id, 4);
    assert_eq!(b(&sprite, bin4).x, 10);
    assert_eq!(b(&sprite, bin4).y, 10);
    assert_eq!(b(&sprite, bin4).w, 10);
    assert_eq!(b(&sprite, bin4).h, 10);
    assert_eq!(b(&sprite, bin4).maxw, 10);
    assert_eq!(b(&sprite, bin4).maxh, 10);
    assert_eq!(sprite.width(), 20);
    assert_eq!(sprite.height(), 20);
    let bin5 = sprite.pack_one(5, 10, 10);
    //  x: 20, y: 0, w: 10, h: 10
    assert_eq!(b(&sprite, bin5).id, 5);
    assert_eq!(b(&sprite, bin5).x, 20);
    assert_eq!(b(&sprite, bin5).y, 0);
    assert_eq!(b(&sprite, bin5).w, 10);
    assert_eq!(b(&sprite, bin5).h, 10);
    assert_eq!(b(&sprite, bin5).maxw, 10);
    assert_eq!(b(&sprite, bin5).maxh, 10);
    assert_eq!(sprite.width(), 40);
    assert_eq!(sprite.height(), 20);
}

#[test]
fn test_resize3() {
    let options = ShelfPackOptions { auto_resize: true };
    let mut sprite = ShelfPack::new(10, 10, options);
    let bin1 = sprite.pack_one(1, 20, 10);
    //  x: 0, y: 0, w: 20, h: 10
    assert_eq!(b(&sprite, bin1).id, 1);
    assert_eq!(b(&sprite, bin1).x, 0);
    assert_eq!(b(&sprite, bin1).y, 0);
    assert_eq!(b(&sprite, bin1).w, 20);
    assert_eq!(b(&sprite, bin1).h, 10);
    assert_eq!(b(&sprite, bin1).maxw, 20);
    assert_eq!(b(&sprite, bin1).maxh, 10);
    assert_eq!(sprite.width(), 40);
    assert_eq!(sprite.height(), 10);
    let bin2 = sprite.pack_one(2, 10, 40);
    //  x: 0, y: 10, w: 10, h: 40
    assert_eq!(b(&sprite, bin2).id, 2);
    assert_eq!(b(&sprite, bin2).x, 0);
    assert_eq!(b(&sprite, bin2).y, 10);
    assert_eq!(b(&sprite, bin2).w, 10);
    assert_eq!(b(&sprite, bin2).h, 40);
    assert_eq!(b(&sprite, bin2).maxw, 10);
    assert_eq!(b(&sprite, bin2).maxh, 40);
    assert_eq!(sprite.width(), 40);
    assert_eq!(sprite.height(), 80);
}
