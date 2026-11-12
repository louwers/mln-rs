// Ports vendor/maplibre-native-base/deps/shelf-pack-cpp/include/mapbox/shelf-pack.hpp
// (mapbox::Bin, mapbox::Shelf, mapbox::ShelfPack, version 2.1.1).
//
// The C++ hands out `Bin*` into the shelves' `std::deque<Bin>` (stable
// addresses: bins are only appended) and keeps them in `usedbins_` and
// `freebins_`. Here a bin is addressed by a [`BinRef`] (shelf index, bin
// index), which compares like the pointer; `ShelfPack::bin` reads it.
// `clear()` drops the shelves and so invalidates every `BinRef`, as it
// leaves every `Bin*` dangling in C++. `int32_t` arithmetic wraps (UB on
// overflow in C++).

use std::collections::{BTreeMap, VecDeque};

pub const SHELF_PACK_VERSION: &str = "2.1.1";

/// `Bin`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bin {
    pub id: i32,
    pub w: i32,
    pub h: i32,
    pub maxw: i32,
    pub maxh: i32,
    pub x: i32,
    pub y: i32,
    refcount_: i32,
}

impl Default for Bin {
    fn default() -> Self {
        Bin::with_all(-1, -1, -1, -1, -1, -1, -1)
    }
}

impl Bin {
    /// `Bin(id, w, h)` (the other arguments default to -1).
    #[must_use]
    pub fn new(id: i32, w: i32, h: i32) -> Self {
        Bin::with_all(id, w, h, -1, -1, -1, -1)
    }

    /// `Bin(id, w, h, maxw, maxh, x, y)`; `maxw`/`maxh` of -1 mean `w`/`h`.
    #[must_use]
    pub fn with_all(id: i32, w: i32, h: i32, maxw: i32, maxh: i32, x: i32, y: i32) -> Self {
        let mut bin = Bin { id, w, h, maxw, maxh, x, y, refcount_: 0 };
        if bin.maxw == -1 {
            bin.maxw = bin.w;
        }
        if bin.maxh == -1 {
            bin.maxh = bin.h;
        }
        bin
    }

    #[must_use]
    pub fn refcount(&self) -> i32 {
        self.refcount_
    }
}

/// A `Bin*` into a `ShelfPack`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BinRef {
    shelf: usize,
    bin: usize,
}

/// `Shelf`
#[derive(Clone, Debug)]
pub struct Shelf {
    x_: i32,
    y_: i32,
    w_: i32,
    h_: i32,
    wfree_: i32,
    bins_: VecDeque<Bin>,
}

impl Shelf {
    #[must_use]
    pub fn new(y1: i32, w1: i32, h1: i32) -> Self {
        Shelf { x_: 0, y_: y1, w_: w1, h_: h1, wfree_: w1, bins_: VecDeque::new() }
    }

    /// `alloc(id, w, h)`: the index of the new bin in this shelf.
    fn alloc(&mut self, id: i32, w1: i32, h1: i32) -> Option<usize> {
        if w1 > self.wfree_ || h1 > self.h_ {
            return None;
        }
        let x1 = self.x_;
        self.x_ = self.x_.wrapping_add(w1);
        self.wfree_ = self.wfree_.wrapping_sub(w1);
        self.bins_.push_back(Bin::with_all(id, w1, h1, w1, self.h_, x1, self.y_));
        Some(self.bins_.len() - 1)
    }

    pub fn resize(&mut self, w1: i32) -> bool {
        self.wfree_ = self.wfree_.wrapping_add(w1.wrapping_sub(self.w_));
        self.w_ = w1;
        true
    }

    #[must_use]
    pub fn x(&self) -> i32 {
        self.x_
    }
    #[must_use]
    pub fn y(&self) -> i32 {
        self.y_
    }
    #[must_use]
    pub fn w(&self) -> i32 {
        self.w_
    }
    #[must_use]
    pub fn h(&self) -> i32 {
        self.h_
    }
    #[must_use]
    pub fn wfree(&self) -> i32 {
        self.wfree_
    }
}

/// `ShelfPack::ShelfPackOptions`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShelfPackOptions {
    pub auto_resize: bool,
}

/// `ShelfPack::PackOptions`
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PackOptions {
    pub in_place: bool,
}

/// `ShelfPack`
#[derive(Clone, Debug)]
pub struct ShelfPack {
    width_: i32,
    height_: i32,
    max_id_: i32,
    auto_resize_: bool,

    shelves_: VecDeque<Shelf>,
    usedbins_: BTreeMap<i32, BinRef>,
    freebins_: Vec<BinRef>,
    stats_: BTreeMap<i32, i32>,
}

impl Default for ShelfPack {
    fn default() -> Self {
        Self::new(0, 0, ShelfPackOptions::default())
    }
}

impl ShelfPack {
    /// `ShelfPack(w, h, options)`: non-positive sizes become 64.
    #[must_use]
    pub fn new(w: i32, h: i32, options: ShelfPackOptions) -> Self {
        ShelfPack {
            width_: if w > 0 { w } else { 64 },
            height_: if h > 0 { h } else { 64 },
            auto_resize_: options.auto_resize,
            max_id_: 0,
            shelves_: VecDeque::new(),
            usedbins_: BTreeMap::new(),
            freebins_: Vec::new(),
            stats_: BTreeMap::new(),
        }
    }

    /// The bin a `BinRef` points to (`*bin`).
    #[must_use]
    pub fn bin(&self, r: BinRef) -> &Bin {
        &self.shelves_[r.shelf].bins_[r.bin]
    }

    fn bin_mut(&mut self, r: BinRef) -> &mut Bin {
        &mut self.shelves_[r.shelf].bins_[r.bin]
    }

    /// `pack(bins, options)`
    pub fn pack(&mut self, bins: &mut [Bin], options: PackOptions) -> Vec<BinRef> {
        let mut results = Vec::new();

        for bin in bins.iter_mut() {
            if bin.w > 0 && bin.h > 0 {
                let Some(allocation) = self.pack_one(bin.id, bin.w, bin.h) else {
                    continue;
                };
                if options.in_place {
                    let a = *self.bin(allocation);
                    bin.id = a.id;
                    bin.x = a.x;
                    bin.y = a.y;
                }
                results.push(allocation);
            }
        }

        self.shrink();

        results
    }

    /// `packOne(id, w, h)`
    pub fn pack_one(&mut self, id: i32, w: i32, h: i32) -> Option<BinRef> {
        let mut id = id;
        let mut y: i32 = 0;
        let mut best_shelf: Option<usize> = None;
        let mut best_freebin: Option<BinRef> = None;
        let mut best_waste = i32::MAX;

        // if id was supplied, attempt a lookup..
        if id == -1 {
            self.max_id_ = self.max_id_.wrapping_add(1);
            id = self.max_id_;
        } else {
            if let Some(pbin) = self.get_bin(id) {
                // we packed this bin already
                self.ref_(pbin);
                return Some(pbin);
            }
            self.max_id_ = if self.max_id_ < id { id } else { self.max_id_ };
        }

        // First try to reuse a free bin..
        for i in 0..self.freebins_.len() {
            let freebin = self.freebins_[i];
            let fb = *self.bin(freebin);
            // exactly the right height and width, use it..
            if h == fb.maxh && w == fb.maxw {
                return Some(self.alloc_freebin(freebin, id, w, h));
            }
            // not enough height or width, skip it..
            if h > fb.maxh || w > fb.maxw {
                continue;
            }
            // extra height or width, minimize wasted area..
            if h <= fb.maxh && w <= fb.maxw {
                let waste = fb.maxw.wrapping_mul(fb.maxh).wrapping_sub(w.wrapping_mul(h));
                if waste < best_waste {
                    best_waste = waste;
                    best_freebin = Some(freebin);
                }
            }
        }

        // Next find the best shelf
        for i in 0..self.shelves_.len() {
            let shelf = &self.shelves_[i];
            y = y.wrapping_add(shelf.h());

            // not enough width on this shelf, skip it..
            if w > shelf.wfree() {
                continue;
            }
            // exactly the right height, pack it..
            if h == shelf.h() {
                return self.alloc_shelf(i, id, w, h);
            }
            // not enough height, skip it..
            if h > shelf.h() {
                continue;
            }
            // extra height, minimize wasted area..
            if h < shelf.h() {
                let waste = shelf.h().wrapping_sub(h).wrapping_mul(w);
                if waste < best_waste {
                    best_waste = waste;
                    best_shelf = Some(i);
                }
            }
        }

        if let Some(freebin) = best_freebin {
            return Some(self.alloc_freebin(freebin, id, w, h));
        }

        if let Some(shelf) = best_shelf {
            return self.alloc_shelf(shelf, id, w, h);
        }

        // No free bins or shelves.. add shelf..
        if h <= self.height_.wrapping_sub(y) && w <= self.width_ {
            self.shelves_.push_back(Shelf::new(y, self.width_, h));
            return self.alloc_shelf(self.shelves_.len() - 1, id, w, h);
        }

        // No room for more shelves..
        // If `autoResize` option is set, grow the sprite as follows:
        //  * double whichever sprite dimension is smaller (`w1` or `h1`)
        //  * if sprite dimensions are equal, grow width before height
        //  * accomodate very large bin requests (big `w` or `h`)
        if self.auto_resize_ {
            let (h1, mut h2) = (self.height_, self.height_);
            let (w1, mut w2) = (self.width_, self.width_);

            if w1 <= h1 || w > w1 {
                // grow width..
                w2 = (if w > w1 { w } else { w1 }).wrapping_mul(2);
            }
            if h1 < w1 || h > h1 {
                // grow height..
                h2 = (if h > h1 { h } else { h1 }).wrapping_mul(2);
            }

            self.resize(w2, h2);
            return self.pack_one(id, w, h); // retry
        }

        None
    }

    /// `shrink()`
    pub fn shrink(&mut self) {
        if !self.shelves_.is_empty() {
            let mut w2: i32 = 0;
            let mut h2: i32 = 0;

            for shelf in &self.shelves_ {
                h2 = h2.wrapping_add(shelf.h());
                let used = shelf.w().wrapping_sub(shelf.wfree());
                w2 = if w2 < used { used } else { w2 };
            }

            self.resize(w2, h2);
        }
    }

    /// `getBin(id)`
    #[must_use]
    pub fn get_bin(&self, id: i32) -> Option<BinRef> {
        self.usedbins_.get(&id).copied()
    }

    /// `ref(bin)`
    pub fn ref_(&mut self, r: BinRef) -> i32 {
        let bin = self.bin_mut(r);
        bin.refcount_ = bin.refcount_.wrapping_add(1);
        let (refcount, h) = (bin.refcount_, bin.h);
        if refcount == 1 {
            // a new Bin.. record height in stats historgram..
            let s = self.stats_.entry(h).or_insert(0);
            *s = s.wrapping_add(1);
        }

        refcount
    }

    /// `unref(bin)`
    pub fn unref(&mut self, r: BinRef) -> i32 {
        let bin = self.bin_mut(r);
        if bin.refcount_ == 0 {
            return 0;
        }

        bin.refcount_ -= 1;
        let (refcount, h, id) = (bin.refcount_, bin.h, bin.id);
        if refcount == 0 {
            let s = self.stats_.entry(h).or_insert(0);
            *s = s.wrapping_sub(1);
            self.usedbins_.remove(&id);
            self.freebins_.push(r);
        }

        refcount
    }

    /// `clear()`
    pub fn clear(&mut self) {
        self.shelves_.clear();
        self.freebins_.clear();
        self.usedbins_.clear();
        self.stats_.clear();
        self.max_id_ = 0;
    }

    /// `resize(w, h)`
    pub fn resize(&mut self, w: i32, h: i32) -> bool {
        self.width_ = w;
        self.height_ = h;
        for shelf in &mut self.shelves_ {
            shelf.resize(w);
        }
        true
    }

    #[must_use]
    pub fn width(&self) -> i32 {
        self.width_
    }

    #[must_use]
    pub fn height(&self) -> i32 {
        self.height_
    }

    /// The bin-height histogram (`stats_`, private in C++).
    #[must_use]
    pub fn stats(&self) -> &BTreeMap<i32, i32> {
        &self.stats_
    }

    /// The shelves (`shelves_`, private in C++).
    #[must_use]
    pub fn shelves(&self) -> &VecDeque<Shelf> {
        &self.shelves_
    }

    fn alloc_freebin(&mut self, r: BinRef, id: i32, w: i32, h: i32) -> BinRef {
        self.freebins_.retain(|b| *b != r);
        let bin = self.bin_mut(r);
        bin.id = id;
        bin.w = w;
        bin.h = h;
        bin.refcount_ = 0;
        self.usedbins_.insert(id, r);
        self.ref_(r);
        r
    }

    fn alloc_shelf(&mut self, shelf: usize, id: i32, w: i32, h: i32) -> Option<BinRef> {
        let bin = self.shelves_[shelf].alloc(id, w, h)?;
        let r = BinRef { shelf, bin };
        self.usedbins_.insert(id, r);
        self.ref_(r);
        Some(r)
    }
}
