//! How a tile's data becomes its contents: a geometry tile lays it out, a
//! raster or raster-dem tile decodes its image. Each has at most one job in
//! flight, for one generation of its inputs; `TileMut` drives them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_storage::Data;
use mln_style::tileset::RasterEncoding;
use mln_tile::TileData;
use mln_vec_map::VecSet;

use crate::image_store::ImageWait;
use crate::layout::tile_layout::{Dependencies, LayoutParams};
use crate::text::glyph_store::GlyphWait;
use crate::tile::contents::Answer;
use crate::tile::job::{DataKey, JobId, JobResult, SourceStyle};
use crate::tile::tile::TileKind;
use crate::tile::tile_workers::Lane;

pub(crate) enum Pipeline {
    Layout(Box<Layout>),
    Decode(Decode),
}

pub(crate) struct Layout {
    /// A resume shares its first try's generation.
    pub(crate) generation: u64,
    /// The last load's data, kept for relayouts and lost lanes.
    pub(crate) data: Option<LayoutData>,
    pub(crate) style: Option<Arc<SourceStyle>>,
    /// `first_load` is set as each job starts.
    pub(crate) params: LayoutParams,
    pub(crate) work: LayoutWork,
}

#[derive(Clone)]
pub(crate) struct LayoutData {
    pub(crate) key: DataKey,
    pub(crate) data: Option<TileData>,
    /// The tileset has no tile here: one of another zoom level draws instead.
    pub(crate) missing: bool,
}

pub(crate) enum LayoutWork {
    /// Nothing to do for this generation.
    Current,
    /// This generation's first try is next, once there are data and style.
    Outdated,
    Running(Running),
    Waiting(Waiting),
    /// A resume waiting for a lane. A first try waits as `Outdated`.
    Ready(Resume),
}

pub(crate) struct InFlight {
    pub(crate) id: JobId,
    pub(crate) lane: Lane,
    pub(crate) superseded: Arc<AtomicBool>,
}

pub(crate) struct Running {
    pub(crate) job: InFlight,
    /// `Some` for a resume, whose `answer` resolves it.
    pub(crate) known: Option<Dependencies>,
    pub(crate) answer: Answer,
    /// The images that existed as the job started.
    pub(crate) images: Arc<VecSet<String>>,
}

#[derive(Default)]
pub(crate) struct DependencyWait {
    pub(crate) glyphs: GlyphWait,
    pub(crate) images: ImageWait,
    /// The stores' revisions at the last try: only new ones can change it.
    pub(crate) revisions: Option<(u64, u64)>,
}

/// A first try that answered `NeedsDeps`.
pub(crate) struct Waiting {
    pub(crate) lane: Lane,
    pub(crate) needs: Dependencies,
    pub(crate) wait: DependencyWait,
    pub(crate) images: Arc<VecSet<String>>,
}

pub(crate) struct Resume {
    /// The lane whose worker kept what the first try discovered; `None`
    /// once a worker lacked the data.
    pub(crate) lane: Option<Lane>,
    pub(crate) known: Dependencies,
    pub(crate) answer: Answer,
    pub(crate) images: Arc<VecSet<String>>,
}

pub(crate) struct Decode {
    pub(crate) generation: u64,
    /// The DEM's encoding; `None` for a raster image.
    pub(crate) dem: Option<RasterEncoding>,
    pub(crate) work: DecodeWork,
}

/// A decode holds an image only while it is queued or in flight.
pub(crate) enum DecodeWork {
    Current,
    Queued(Data),
    /// Stale unless its generation is current. `image` is the current
    /// generation's, kept for a lost lane or a stale reply.
    Running {
        job: InFlight,
        image: Option<Data>,
    },
}

/// Whether `result`, of a job that got the images `then`, branched on one
/// that came or went by `now`.
pub(crate) fn outdated_by(result: &JobResult, then: &VecSet<String>, now: &VecSet<String>) -> bool {
    let deps = match result {
        JobResult::Laid(output) => &output.deps,
        JobResult::NeedsDeps(needs) => needs,
        JobResult::NeedsData | JobResult::Raster(_) | JobResult::Dem(_) | JobResult::Failed(_) => return false,
    };
    deps.unavailable_images.iter().any(|id| now.contains(id))
        || deps.images.keys().any(|id| then.contains(id) && !now.contains(id))
}

impl Pipeline {
    pub(crate) fn new(kind: TileKind, generation: u64, params: LayoutParams) -> Self {
        let decode = |dem| Pipeline::Decode(Decode { generation, dem, work: DecodeWork::Current });
        match kind {
            TileKind::Geometry => Pipeline::Layout(Box::new(Layout {
                generation,
                data: None,
                style: None,
                params,
                work: LayoutWork::Outdated,
            })),
            TileKind::Raster => decode(None),
            TileKind::Dem(encoding) => decode(Some(encoding)),
        }
    }

    pub(crate) fn layout(&self) -> Option<&Layout> {
        match self {
            Pipeline::Layout(layout) => Some(layout),
            Pipeline::Decode(_) => None,
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        match self {
            Pipeline::Layout(layout) => layout.generation,
            Pipeline::Decode(decode) => decode.generation,
        }
    }

    /// Whether work is under way or to start. An outdated layout with its
    /// style is pending while it has data to lay out, and while it waits for
    /// data until the tile settles without any, so that a tile isn't complete
    /// before its load answers.
    pub(crate) fn is_pending(&self, settled: bool) -> bool {
        match self {
            Pipeline::Layout(layout) => match &layout.work {
                LayoutWork::Current => false,
                LayoutWork::Outdated => layout.style.is_some() && (layout.data.is_some() || !settled),
                LayoutWork::Running(_) | LayoutWork::Waiting(_) | LayoutWork::Ready(_) => true,
            },
            Pipeline::Decode(decode) => {
                matches!(decode.work, DecodeWork::Queued(_) | DecodeWork::Running { image: Some(_), .. })
            }
        }
    }

    /// Whether there is a job to submit, or dependencies to resolve.
    pub(crate) fn has_work(&self) -> bool {
        match self {
            Pipeline::Layout(layout) => match &layout.work {
                LayoutWork::Waiting(_) | LayoutWork::Ready(_) => true,
                LayoutWork::Outdated => layout.data.is_some() && layout.style.is_some(),
                LayoutWork::Current | LayoutWork::Running(_) => false,
            },
            Pipeline::Decode(decode) => matches!(decode.work, DecodeWork::Queued(_)),
        }
    }

    pub(crate) fn in_flight(&self) -> Option<&InFlight> {
        match self {
            Pipeline::Layout(layout) => match &layout.work {
                LayoutWork::Running(running) => Some(&running.job),
                LayoutWork::Current | LayoutWork::Outdated | LayoutWork::Waiting(_) | LayoutWork::Ready(_) => None,
            },
            Pipeline::Decode(decode) => match &decode.work {
                DecodeWork::Running { job, .. } => Some(job),
                DecodeWork::Current | DecodeWork::Queued(_) => None,
            },
        }
    }

    /// The lane the work holds a place on, or whose worker keeps its state.
    pub(crate) fn lane(&self) -> Option<Lane> {
        match self {
            Pipeline::Layout(layout) => match &layout.work {
                LayoutWork::Running(Running { job: InFlight { lane, .. }, .. })
                | LayoutWork::Waiting(Waiting { lane, .. })
                | LayoutWork::Ready(Resume { lane: Some(lane), .. }) => Some(*lane),
                LayoutWork::Current | LayoutWork::Outdated | LayoutWork::Ready(_) => None,
            },
            Pipeline::Decode(_) => self.in_flight().map(|job| job.lane),
        }
    }
}

impl Layout {
    /// Takes `generation`: a job in flight becomes stale, other work outdated.
    pub(crate) fn bump(&mut self, generation: u64) {
        self.generation = generation;
        match &self.work {
            LayoutWork::Running(running) => running.job.superseded.store(true, Ordering::Relaxed),
            _ => self.work = LayoutWork::Outdated,
        }
    }

    /// The dependencies the work names, apart from what the tile shows.
    pub(crate) fn dependencies(&self) -> Option<&Dependencies> {
        match &self.work {
            LayoutWork::Running(Running { known: Some(known), .. })
            | LayoutWork::Ready(Resume { known, .. })
            | LayoutWork::Waiting(Waiting { needs: known, .. }) => Some(known),
            LayoutWork::Running(_) | LayoutWork::Current | LayoutWork::Outdated => None,
        }
    }
}

impl Decode {
    /// Queues the image the decode in flight held, if any.
    pub(crate) fn requeue(&mut self) {
        if let DecodeWork::Running { image, .. } = &mut self.work {
            self.work = image.take().map_or(DecodeWork::Current, DecodeWork::Queued);
        }
    }
}
