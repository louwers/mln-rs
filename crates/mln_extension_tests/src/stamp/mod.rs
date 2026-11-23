//! The `stamp` source type, whose tiles each hold a [`Stamp`] that its
//! origin makes from the tile's id, and the `stamp-fill` and `stamp-mark`
//! layer types, which view a stamp source's tiles: one fills each tile with
//! its stamp's fill, the other draws a square of its stamp's mark in the
//! middle of the tile.
//!
//! A stamp source takes two members in style JSON: `version`, a number
//! that every stamp carries, so that a new version gives each tile a new
//! stamp, and `hold`, which, while true, leaves new tiles without a stamp,
//! as tiles whose values take long to make are.

mod program;

use std::convert::Infallible;
use std::sync::Arc;

use mln_rs::layer::{
    Draw, Draws, FrameTile, Layer, LayerTypeInfo, Mesh, MeshDesc, NewCx, NoProperties, PrepareCx, StyleLayerType,
    TileCx, TileTransform, UpdateCx,
};
use mln_rs::mln_render::{RegisterError, RegistryBuilder};
use mln_rs::source::conversion::Error;
use mln_rs::source::{
    CanonicalTileID, Convertible, Cover, DataOrigin, Functions, LoadContext, OriginUpdate, OverscaledTileID, Range,
    Source, SourceData, SourceKind, SourceType, SourceTypeInfo, StyleSourceType, Supply, ValueKind, ValueTiles,
};

use self::program::{RECT, RectDraw};

/// The kind of a stamp source's tiles, which the stamp layer types view.
const STAMPS: SourceKind = SourceKind::Value(ValueKind::of::<Stamp>());

/// What a stamp source's tile holds: its colors, made from its id and the
/// source's version.
#[derive(Debug)]
pub struct Stamp {
    pub tile: CanonicalTileID,
    pub version: u32,
    /// What `stamp-fill` fills the tile with, premultiplied.
    pub fill: [f32; 4],
    /// What `stamp-mark` draws in the middle of the tile, premultiplied.
    pub mark: [f32; 4],
}

impl Stamp {
    /// The colors in their order: each tile takes one by where it is among
    /// its neighbours, and the next by its version.
    const COLORS: [[f32; 4]; 4] =
        [[1.0, 0.0, 0.0, 1.0], [0.0, 0.5, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], [1.0, 1.0, 0.0, 1.0]];

    fn of(tile: CanonicalTileID, version: u32) -> Stamp {
        let at = (tile.x % 2 + 2 * (tile.y % 2) + version) as usize;
        Stamp { tile, version, fill: Stamp::COLORS[at % 4], mark: Stamp::COLORS[(at + 2) % 4] }
    }
}

/// The `stamp` source type.
pub struct StampSource;

/// A stamp source's members.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StampOptions {
    pub version: u32,
    pub hold: bool,
}

impl StyleSourceType for StampSource {
    const INFO: SourceTypeInfo = SourceTypeInfo { name: "stamp", layer_kind: STAMPS, from_json: true };

    type Options = StampOptions;
    type Description = ();

    fn from_json(value: &Convertible<'_>, _functions: &Arc<Functions>, error: &mut Error) -> Option<SourceData<Self>> {
        let version = match value.object_member("version") {
            Some(version) => version.to_number().filter(|version| *version >= 0.0)? as u32,
            None => 0,
        };
        let hold = match value.object_member("hold") {
            Some(hold) => hold.to_bool()?,
            None => false,
        };
        error.message.clear();
        Some(SourceData::new(StampOptions { version, hold }))
    }
}

impl SourceType for StampSource {
    type Data = ValueTiles<Stamp>;

    fn supply(_source: &Source) -> Supply<ValueTiles<Stamp>> {
        Supply::tiles(StampOrigin::default())
    }
}

/// Where a stamp source's tiles get their stamps: from the origin itself,
/// as each tile starts, unless the source holds them.
#[derive(Default)]
struct StampOrigin {
    /// The source's members as of its last version.
    options: StampOptions,
}

impl StampOrigin {
    fn deliver(&self, cx: &mut LoadContext<'_, '_, Self>) {
        cx.deliver(Some(Arc::new(Stamp::of(cx.id().canonical, self.options.version))));
    }
}

impl DataOrigin for StampOrigin {
    type Data = ValueTiles<Stamp>;
    type Tile = ();
    type Reply = Infallible;

    /// A new version gives every tile a new stamp, and the source letting
    /// go of its tiles gives those that wait theirs; the cached tiles go.
    fn update(&mut self, update: &mut OriginUpdate<'_, Self>) -> Option<Cover> {
        let options = SourceData::<StampSource>::recover(update.source())?.options.clone();
        let new_version = options.version != self.options.version;
        if options != self.options {
            self.options = options;
            if !self.options.hold {
                update.update_tiles(
                    |_| true,
                    |(), cx| {
                        if new_version || !cx.is_renderable() {
                            self.deliver(cx);
                        }
                    },
                );
            }
            if new_version {
                update.clear_cache();
            }
        }
        Some(Cover {
            tile_size: 32,
            zoom_range: Range::new(0, 22),
            bounds: None,
            round_zoom: false,
            overscale: false,
            prefetch: false,
            synchronous: false,
        })
    }

    /// A tile in a new world copy takes the stamp of the same tile in
    /// another; any other gets its own, unless the source holds it.
    fn start(&mut self, cx: &mut LoadContext<'_, '_, Self>, copy: Option<&()>) {
        if copy.is_some() && cx.take_copy() {
            return;
        }
        if !self.options.hold {
            self.deliver(cx);
        }
    }

    fn receive(&mut self, (): &mut (), reply: Infallible, _cx: &mut LoadContext<'_, '_, Self>) {
        match reply {}
    }
}

/// A stamp a layer viewed in a tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub tile: OverscaledTileID,
    /// Where the stamp is in memory, which tells one value from another.
    pub stamp: usize,
    pub version: u32,
}

impl Seen {
    fn of(tile: FrameTile, stamp: &Stamp) -> Seen {
        Seen { tile: tile.id(), stamp: std::ptr::from_ref(stamp) as usize, version: stamp.version }
    }
}

/// What a stamp layer saw: the stamps it made its tiles' values of, and
/// those of the tiles its last frame drew.
#[derive(Default)]
struct Sight {
    made: Vec<Seen>,
    drawn: Vec<Seen>,
}

/// The style half of a layer type that views stamps, whose layers draw a
/// rectangle of each tile in a color of its stamp: `stamp-fill` or
/// `stamp-mark`.
pub trait StampView: StyleLayerType {
    /// The rectangle, in tile units, and its color.
    fn draw(stamp: &Stamp) -> RectDraw;
}

/// The `stamp-fill` layer type's style half: each tile filled with its
/// stamp's fill.
pub struct StampFill;

impl StyleLayerType for StampFill {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "stamp-fill", source: STAMPS, from_json: true };
    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();
}

impl StampView for StampFill {
    fn draw(stamp: &Stamp) -> RectDraw {
        RectDraw { rect: [0.0, 0.0, 8192.0, 8192.0], color: stamp.fill }
    }
}

/// The `stamp-mark` layer type's style half: a square of each tile's
/// stamp's mark, half the tile wide, in its middle.
pub struct StampMark;

impl StyleLayerType for StampMark {
    const INFO: LayerTypeInfo = LayerTypeInfo { name: "stamp-mark", source: STAMPS, from_json: true };
    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();
}

impl StampView for StampMark {
    fn draw(stamp: &Stamp) -> RectDraw {
        RectDraw { rect: [2048.0, 2048.0, 6144.0, 6144.0], color: stamp.mark }
    }
}

/// A layer of the stamp layer type `V`, whose value of a tile is its draw.
pub struct StampLayer<V> {
    quad: Mesh<RectDraw>,
    sight: Sight,
    view: std::marker::PhantomData<fn() -> V>,
}

impl<V> StampLayer<V> {
    /// The stamps the layer made its tiles' values of, oldest first.
    #[must_use]
    pub fn made(&self) -> Vec<Seen> {
        self.sight.made.clone()
    }

    /// The stamps of the tiles its last frame drew.
    #[must_use]
    pub fn drawn(&self) -> Vec<Seen> {
        self.sight.drawn.clone()
    }
}

impl<V: StampView> Layer for StampLayer<V> {
    type Style = V;
    type Input = ValueTiles<Stamp>;
    type Tile = RectDraw;

    fn new(cx: &mut NewCx<'_, Self>) -> Self {
        let corners = cx.res.buffer(&[[0i16, 0], [1, 0], [1, 1], [0, 1]]);
        let indexes = cx.res.index_buffer(&[0, 1, 2, 0, 2, 3]);
        let mut quad = MeshDesc::new(&RECT, &indexes);
        quad.vertices(&corners);
        StampLayer { quad: cx.res.mesh(&quad), sight: Sight::default(), view: std::marker::PhantomData }
    }

    fn update(&mut self, cx: &mut UpdateCx<'_, Self>) {
        let drawn = cx.tiles.iter().filter_map(|(tile, stamp, _)| Some(Seen::of(tile, stamp?)));
        self.sight.drawn = drawn.collect();
    }

    fn tile(&mut self, _cx: &mut TileCx<'_, Self>, tile: FrameTile, stamp: &Stamp) -> Option<RectDraw> {
        self.sight.made.push(Seen::of(tile, stamp));
        Some(V::draw(stamp))
    }

    /// Each tile's draw clips to its tile, where a parent lies under the
    /// children that draw.
    fn prepare(&self, cx: &PrepareCx<'_, Self>, out: &mut Draws) {
        let main = out.main();
        for (tile, _, draw) in cx.tiles() {
            let at = tile.project(cx.frame, &TileTransform::default());
            main.push_at(&self.quad, &at, Draw { clip: true, ..Draw::DEFAULT }, *draw);
        }
    }
}

/// The `stamp-fill` layer type's render half.
pub type StampFillLayer = StampLayer<StampFill>;

/// The `stamp-mark` layer type's render half.
pub type StampMarkLayer = StampLayer<StampMark>;

/// Adds the `stamp` source type and the `stamp-fill` and `stamp-mark`
/// layer types to `builder`.
pub fn register(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_source_type::<StampSource>()?;
    builder.add_layer_type::<StampFillLayer>()?;
    builder.add_layer_type::<StampMarkLayer>()
}
