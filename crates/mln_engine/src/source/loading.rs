//! What a source type's loading half returns: how a source's description
//! loads, what it reports about itself, and what an offline download fetches
//! for it.

use std::sync::Arc;

use mln_style::error::Error;
use mln_style::sources::GeoJSONOptions;
use mln_style::tileset::{Tileset, VectorEncoding};

/// Where a job hands its result: called at most once, from any thread,
/// while the job runs or later. Dropping it uncalled drops the result.
pub type Respond<R> = Box<dyn FnOnce(R) + Send>;

/// How a source's description loads, as its type says from its data.
pub enum Load<D> {
    /// Nothing loads: the source is loaded with the description it has.
    Done,
    /// The description, known without a fetch, such as a tileset that the
    /// style lists.
    Ready(D),
    /// The description that [`SourceType::describe`](super::SourceType::describe)
    /// makes from the source's options alone, with no data, such as GeoJSON
    /// that the style lists.
    Describe,
    /// The resource that [`SourceType::describe`](super::SourceType::describe)
    /// or, for an image, [`SourceType::describe_image`](super::SourceType::describe_image)
    /// turns into the description.
    Fetch(Fetch),
}

/// A resource that a source loads its description from, by its URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// A document that describes the source, such as TileJSON or GeoJSON.
    Source(String),
    /// An image, which the map's loader decodes.
    Image(String),
}

/// What describing a source found: its description, `None` for data that
/// describes nothing, or why the source failed to load.
pub type Described<D> = Result<Option<D>, Error>;

/// What a source reports about itself, whichever its type. A member that
/// the type doesn't have is `None`.
#[derive(Clone, Debug, Default)]
pub struct SourceSummary {
    /// The URL that the source loads its description from.
    pub url: Option<String>,
    /// The size of the source's tiles, in pixels.
    pub tile_size: Option<u16>,
    /// The tileset that the style lists, for a tileset source without a URL.
    pub inline_tileset: Option<Tileset>,
    /// How a vector source's tiles are encoded: as its loaded tileset says,
    /// else as the style does.
    pub vector_encoding: Option<VectorEncoding>,
    /// A GeoJSON source's options.
    pub geojson: Option<Arc<GeoJSONOptions>>,
}

/// What an offline download of a source fetches.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum OfflineResources {
    /// Nothing: the host gives the source's data.
    #[default]
    None,
    /// The tiles of a tileset that's known without a fetch, of tiles
    /// `tile_size` pixels wide, which cover a view at the zoom level
    /// nearest its own if `round_zoom` (see [`Cover::round_zoom`](super::Cover::round_zoom)).
    Tiles { tileset: Tileset, tile_size: u16, round_zoom: bool },
    /// The TileJSON at `url`, then the tiles, as [`OfflineResources::Tiles`]'s,
    /// of the tileset that
    /// [`SourceType::offline_tileset`](super::SourceType::offline_tileset)
    /// makes of it.
    TileJson { url: String, tile_size: u16, round_zoom: bool },
    /// The source's data at `url`: GeoJSON.
    Source { url: String },
    /// The source's image at `url`, which is nothing to fetch if empty.
    Image { url: String },
}
