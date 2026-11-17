//! How a tile's data becomes its contents: a geometry tile lays its data out
//! with its source's style ([`Layout`]), and a raster or raster-dem tile
//! decodes its image ([`Decode`]). Each runs one job at a time, for one
//! generation of its inputs; a reply applies only if it names the current
//! generation.
//!
//! These are plain state machines: the store hands them the generations it
//! takes from its source's counter and the lanes its workers have room on,
//! and carries out the steps they return.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_storage::Data;
use mln_tile::TileData;
use mln_util::error::Error;
use mln_util::hash::HashSet;
use mln_util::logging::{Event, Log};
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::buckets::raster_bucket::RasterBucket;
use crate::image_store::ImageWait;
use crate::layout::tile_layout::{Dependencies, LayoutParams};
use crate::text::glyph_store::GlyphWait;
use crate::tile::contents::{Answer, Contents};
use crate::tile::job::{DataInput, DataKey, Job, JobId, JobResult, LayoutJob, LayoutOutput, Run, SourceStyle, TileKey};
use crate::tile::tile::{Shown, TileKind};
use crate::tile::tile_workers::Lane;

/// How a tile's data becomes its contents; the variant follows its source's
/// kind.
pub(crate) enum Pipeline {
    Layout(Box<Layout>),
    Decode(Decode),
}

/// A geometry tile's layout: its data, its style and parameters, and the
/// work of laying them out.
pub(crate) struct Layout {
    /// The generation of the inputs. A resume shares its first try's.
    generation: u64,
    /// The data of the last load, kept for relayouts and for a job whose
    /// lane is lost.
    data: Option<LayoutData>,
    style: Option<Arc<SourceStyle>>,
    /// The parameters jobs lay out with; `first_load` is set as each starts.
    params: LayoutParams,
    work: LayoutWork,
}

/// The data of one load, as a layout reads it.
pub(crate) struct LayoutData {
    pub(crate) key: DataKey,
    /// `None` for a tile without data (204, no tile in an archive).
    pub(crate) data: Option<TileData>,
    /// The tileset has no tile here, so a tile of another zoom level draws
    /// in its place.
    pub(crate) missing: bool,
}

/// Where a layout's work stands.
pub(crate) enum LayoutWork {
    /// Nothing to do for this generation: the tile shows its layout,
    /// failed, or waits for the data or style it lacks.
    Current,
    /// The generation has no job yet: its first try is next.
    Outdated,
    Running(Running),
    Waiting(Waiting),
    Ready(Resume),
}

/// A job in flight.
pub(crate) struct InFlight {
    pub(crate) id: JobId,
    pub(crate) lane: Lane,
    /// Set once the tile no longer wants the job's result.
    pub(crate) superseded: Arc<AtomicBool>,
}

/// A layout job in flight, of the current generation or (when stale) an
/// earlier one.
pub(crate) struct Running {
    pub(crate) job: InFlight,
    /// The job is a resume, whose `answer` resolves `known`.
    pub(crate) known: Option<Dependencies>,
    pub(crate) answer: Answer,
    /// The images that exist, as the job got them.
    pub(crate) images: Arc<VecSet<String>>,
}

/// A wait for the stores to resolve a job's dependencies.
#[derive(Default)]
pub(crate) struct DependencyWait {
    pub(crate) glyphs: GlyphWait,
    pub(crate) images: ImageWait,
    /// The stores' revisions at the last try, which only another revision
    /// can change the outcome of.
    pub(crate) revisions: Option<(u64, u64)>,
}

/// A first try that answered `NeedsDeps`: the tile waits for the stores.
pub(crate) struct Waiting {
    pub(crate) lane: Lane,
    pub(crate) needs: Dependencies,
    pub(crate) wait: DependencyWait,
    /// The images the job got, which its resume discovers with again.
    pub(crate) images: Arc<VecSet<String>>,
}

/// A resume, ready but waiting for a lane with room. A first try never
/// waits here: it waits as `Outdated`.
pub(crate) struct Resume {
    /// The lane whose worker keeps what the first try discovered; `None`
    /// once a worker lacked the data.
    pub(crate) lane: Option<Lane>,
    pub(crate) known: Dependencies,
    pub(crate) answer: Answer,
    /// The images the first try got, which the resume discovers with again.
    pub(crate) images: Arc<VecSet<String>>,
}

/// What the store does with a layout's reply.
pub(crate) enum LayoutStep {
    /// Show `output`, with `answer`: a first try's may hold more than it
    /// uses.
    Show {
        output: LayoutOutput,
        answer: Answer,
        first_try: bool,
    },
    /// Nothing: the layout waits for its dependencies.
    Wait,
    /// The worker lacked data `key`: the next job carries it.
    Resend(DataKey),
    /// The reply is stale and goes. `images_moved`: it branched on an image
    /// that came or went since, so the tile lays out again without the
    /// answer it shows.
    Outdated {
        images_moved: bool,
        result: JobResult,
    },
    Failed(Error),
}

/// A raster or raster-dem tile's decode of its image.
pub(crate) struct Decode {
    generation: u64,
    work: DecodeWork,
}

/// Where a decode stands. The tile holds an image only while it is queued
/// or decoding.
pub(crate) enum DecodeWork {
    /// The tile shows its last decode, or nothing.
    Current,
    /// This generation's image, waiting for a lane.
    Queued(Data),
    /// A decode in flight, stale unless its generation is current. `image`
    /// is the current generation's, kept for a lost lane or a stale reply;
    /// `None` once a load brought no image.
    Running { job: InFlight, image: Option<Data> },
}

/// What the store does with a decode's reply.
pub(crate) enum DecodeStep {
    /// The reply is stale and goes.
    Stale,
    Show(Contents),
    Failed(Error),
}

/// Whether `result`, of a job that got the images `then`, branched on one
/// that came or went since, as `now` has them: an image its discovery found
/// unavailable that is there now, or one it found available that is gone.
/// What else it names resolved, or resolves, as the stores have it.
fn outdated_by(result: &JobResult, then: &VecSet<String>, now: &VecSet<String>) -> bool {
    let deps = match result {
        JobResult::Laid(output) => &output.deps,
        JobResult::NeedsDeps(needs) => needs,
        JobResult::NeedsData | JobResult::Raster(_) | JobResult::Dem(_) | JobResult::Failed(_) => return false,
    };
    deps.unavailable_images.iter().any(|id| now.contains(id))
        || deps.images.keys().any(|id| then.contains(id) && !now.contains(id))
}

impl Pipeline {
    /// The pipeline of a new tile of `kind` at `generation`, which lays out
    /// with `params`.
    pub(crate) fn new(kind: TileKind, generation: u64, params: LayoutParams) -> Self {
        match kind {
            TileKind::Geometry => Pipeline::Layout(Box::new(Layout::new(generation, params))),
            TileKind::Raster | TileKind::Dem(_) => Pipeline::Decode(Decode { generation, work: DecodeWork::Current }),
        }
    }

    pub(crate) fn layout(&self) -> Option<&Layout> {
        match self {
            Pipeline::Layout(layout) => Some(layout),
            Pipeline::Decode(_) => None,
        }
    }

    pub(crate) fn layout_mut(&mut self) -> Option<&mut Layout> {
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

    /// Whether work is under way or to start; `settled` for a tile that has
    /// done what it can since it last started loading.
    pub(crate) fn is_pending(&self, settled: bool) -> bool {
        match self {
            Pipeline::Layout(layout) => layout.is_pending(settled),
            Pipeline::Decode(decode) => decode.is_pending(),
        }
    }

    /// Whether there is a job to submit, or dependencies to resolve.
    pub(crate) fn has_work(&self) -> bool {
        match self {
            Pipeline::Layout(layout) => layout.has_work(),
            Pipeline::Decode(decode) => matches!(decode.work, DecodeWork::Queued(_)),
        }
    }

    /// The job in flight.
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
            Pipeline::Layout(layout) => layout.lane(),
            Pipeline::Decode(_) => self.in_flight().map(|job| job.lane),
        }
    }

    /// The reply to the job in flight is stale: the work goes on with the
    /// current generation.
    pub(crate) fn stale_replied(&mut self) {
        match self {
            Pipeline::Layout(layout) => layout.stale_replied(),
            Pipeline::Decode(decode) => decode.stale_replied(),
        }
    }

    /// The lane the work holds is gone: its job never replies, and the work
    /// starts again at `generation`.
    pub(crate) fn lane_lost(&mut self, generation: u64) {
        match self {
            Pipeline::Layout(layout) => layout.lane_lost(generation),
            Pipeline::Decode(decode) => decode.lane_lost(generation),
        }
    }
}

impl Layout {
    fn new(generation: u64, params: LayoutParams) -> Self {
        Layout { generation, data: None, style: None, params, work: LayoutWork::Outdated }
    }

    pub(crate) fn data(&self) -> Option<&LayoutData> {
        self.data.as_ref()
    }

    pub(crate) fn params(&self) -> &LayoutParams {
        &self.params
    }

    #[cfg(test)]
    pub(crate) fn work(&self) -> &LayoutWork {
        &self.work
    }

    /// Takes generation `generation`: a job in flight becomes stale, and
    /// any other work outdated.
    pub(crate) fn bump(&mut self, generation: u64) {
        self.generation = generation;
        match &self.work {
            LayoutWork::Running(running) => running.job.superseded.store(true, Ordering::Relaxed),
            _ => self.work = LayoutWork::Outdated,
        }
    }

    /// Lays out `data` at `generation`.
    pub(crate) fn set_data(&mut self, data: LayoutData, generation: u64) {
        self.data = Some(data);
        self.bump(generation);
    }

    /// Lays out with `style`, for `pixel_ratio` and `granularity`, at
    /// `generation`.
    pub(crate) fn set_style(
        &mut self,
        style: Arc<SourceStyle>,
        pixel_ratio: f32,
        granularity: SubdivisionGranularitySetting,
        generation: u64,
    ) {
        self.style = Some(style);
        self.params.pixel_ratio = pixel_ratio;
        self.params.subdivision_granularity = granularity;
        self.bump(generation);
    }

    /// Lays out with collision boxes shown or not, at `generation`.
    pub(crate) fn set_show_collision_boxes(&mut self, show: bool, generation: u64) {
        self.params.show_collision_boxes = show;
        self.bump(generation);
    }

    /// Drops the data and style, and waits for new ones at `generation`.
    pub(crate) fn reset(&mut self, generation: u64) {
        self.data = None;
        self.style = None;
        self.bump(generation);
    }

    /// Whether work is under way or to start. A layout with its style counts
    /// as started from the tile's first load on, as C++'s does from
    /// `setLayers`, until the tile has `settled` without data.
    fn is_pending(&self, settled: bool) -> bool {
        match &self.work {
            LayoutWork::Current => false,
            LayoutWork::Outdated => self.style.is_some() && (self.data.is_some() || !settled),
            LayoutWork::Running(_) | LayoutWork::Waiting(_) | LayoutWork::Ready(_) => true,
        }
    }

    fn has_work(&self) -> bool {
        match &self.work {
            LayoutWork::Waiting(_) | LayoutWork::Ready(_) => true,
            LayoutWork::Outdated => self.data.is_some() && self.style.is_some(),
            LayoutWork::Current | LayoutWork::Running(_) => false,
        }
    }

    fn lane(&self) -> Option<Lane> {
        match &self.work {
            LayoutWork::Running(Running { job: InFlight { lane, .. }, .. })
            | LayoutWork::Waiting(Waiting { lane, .. })
            | LayoutWork::Ready(Resume { lane: Some(lane), .. }) => Some(*lane),
            LayoutWork::Current | LayoutWork::Outdated | LayoutWork::Ready(_) => None,
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

    /// Whether a change of the images `changed`, of which `available` exist
    /// now, touches the work: a resume's answer names one, or a job waiting
    /// for its images branched on one that came or went (an image the
    /// stores still have to resolve resolves as it is now). A first try's
    /// reply tells whether the change touched it.
    pub(crate) fn touches_images(&self, changed: &HashSet<String>, available: &VecSet<String>) -> bool {
        match &self.work {
            LayoutWork::Running(Running { known: Some(known), .. }) | LayoutWork::Ready(Resume { known, .. }) => {
                known.images.keys().any(|id| changed.contains(id))
            }
            LayoutWork::Waiting(Waiting { needs, .. }) => {
                needs.unavailable_images.iter().any(|id| changed.contains(id) && available.contains(id))
                    || needs.images.keys().any(|id| changed.contains(id) && !available.contains(id))
            }
            LayoutWork::Running(_) | LayoutWork::Current | LayoutWork::Outdated => false,
        }
    }

    /// Whether the job running or ready lacks glyphs of a range that failed
    /// to load.
    pub(crate) fn lacks_failed_glyphs(&self) -> bool {
        match &self.work {
            LayoutWork::Running(Running { answer, .. }) | LayoutWork::Ready(Resume { answer, .. }) => {
                answer.glyphs_failed
            }
            LayoutWork::Current | LayoutWork::Outdated | LayoutWork::Waiting(_) => false,
        }
    }

    /// Resolves the dependencies a waiting job needs through `resolve`,
    /// which answers once the stores have them all or know them missing:
    /// the resume is then ready.
    pub(crate) fn resolve(&mut self, resolve: impl FnOnce(&mut Waiting) -> Option<Resume>) {
        if let LayoutWork::Waiting(waiting) = &mut self.work
            && let Some(resume) = resolve(waiting)
        {
            self.work = LayoutWork::Ready(resume);
        }
    }

    /// Starts the next job of tile `tile`, which shows `shown`: a resume,
    /// or a first try with the images `images_now` and the answer `shown`
    /// has unless it is outdated. `place` picks the job's lane for its data
    /// key, and whether the job can name the data rather than carry it; its
    /// second argument asks for a lane that gets the data. Returns the lane
    /// and the job; `None`, with the work as it was, if there is none to
    /// start or no lane has room.
    pub(crate) fn next_job(
        &mut self,
        tile: TileKey,
        shown: Option<&Shown>,
        images_now: &Arc<VecSet<String>>,
        place: impl FnOnce(DataKey, bool) -> Option<(Lane, bool)>,
    ) -> Option<(Lane, Run)> {
        let (Some(data), Some(style)) = (&self.data, &self.style) else {
            return None;
        };
        if !matches!(self.work, LayoutWork::Outdated | LayoutWork::Ready(_)) {
            return None;
        }
        let (lane, cached) = place(data.key, data.data.is_none())?;
        let (known, answer, images) = if let LayoutWork::Ready(Resume { known, answer, images, .. }) =
            std::mem::replace(&mut self.work, LayoutWork::Outdated)
        {
            (Some(known), answer, images)
        } else {
            let answer = shown
                .filter(|shown| !shown.answer_outdated)
                .and_then(|shown| shown.contents.geometry())
                .map(|contents| contents.answer.clone())
                .unwrap_or_default();
            (None, answer, Arc::clone(images_now))
        };
        let input = if cached {
            DataInput::Cached(data.key)
        } else {
            DataInput::Full { key: data.key, data: data.data.clone() }
        };
        let job = LayoutJob {
            style: Arc::clone(style),
            available_images: Arc::clone(&images),
            data: input,
            params: LayoutParams { first_load: shown.is_none(), ..self.params },
            resolved: Arc::clone(&answer.resolved),
        };
        let id = JobId { tile, generation: self.generation };
        let superseded = Arc::new(AtomicBool::new(false));
        let job_in_flight = InFlight { id, lane, superseded: Arc::clone(&superseded) };
        self.work = LayoutWork::Running(Running { job: job_in_flight, known, answer, images });
        Some((lane, Run { id, job: Job::Layout(job), superseded }))
    }

    /// Takes the job in flight, whose reply arrived; the work is outdated
    /// until the reply applies.
    pub(crate) fn take_running(&mut self) -> Option<Running> {
        match std::mem::replace(&mut self.work, LayoutWork::Outdated) {
            LayoutWork::Running(running) => Some(running),
            work => {
                self.work = work;
                None
            }
        }
    }

    /// Applies `result`, the reply to `running`, while the images
    /// `available` exist.
    pub(crate) fn apply(&mut self, running: Running, result: JobResult, available: &Arc<VecSet<String>>) -> LayoutStep {
        let Running { job, known, answer, images } = running;
        let images_moved = !Arc::ptr_eq(&images, available) && outdated_by(&result, &images, available);
        if job.id.generation != self.generation || images_moved {
            self.work = LayoutWork::Outdated;
            return LayoutStep::Outdated { images_moved, result };
        }
        match result {
            JobResult::Laid(output) => {
                self.work = LayoutWork::Current;
                LayoutStep::Show { output, answer, first_try: known.is_none() }
            }
            JobResult::NeedsDeps(needs) if known.is_none() => {
                let wait = DependencyWait::default();
                self.work = LayoutWork::Waiting(Waiting { lane: job.lane, needs, wait, images });
                LayoutStep::Wait
            }
            JobResult::NeedsDeps(_) => {
                self.work = LayoutWork::Current;
                Log::error(
                    Event::General,
                    format!("Tile {} asked again for the dependencies it was given", job.id.tile.id),
                );
                LayoutStep::Failed(Error::native("a layout asked again for the dependencies it was given"))
            }
            JobResult::NeedsData => {
                // A resume keeps its answer and generation; a first try runs
                // again with the images as they are then.
                self.work = match known {
                    Some(known) => LayoutWork::Ready(Resume { lane: None, known, answer, images }),
                    None => LayoutWork::Outdated,
                };
                match &self.data {
                    Some(data) => LayoutStep::Resend(data.key),
                    None => LayoutStep::Wait,
                }
            }
            JobResult::Failed(error) => {
                self.work = LayoutWork::Current;
                LayoutStep::Failed(error)
            }
            JobResult::Raster(_) | JobResult::Dem(_) => {
                self.work = LayoutWork::Current;
                LayoutStep::Failed(Error::native("a geometry tile's layout answered as a decode"))
            }
        }
    }

    fn stale_replied(&mut self) {
        self.work = LayoutWork::Outdated;
    }

    fn lane_lost(&mut self, generation: u64) {
        self.bump(generation);
        self.work = LayoutWork::Outdated;
    }

    /// A first try of the current generation, as if in flight, whose result
    /// a test applies.
    #[cfg(test)]
    pub(crate) fn run_now(&mut self, tile: TileKey, images: Arc<VecSet<String>>) -> Running {
        let job =
            InFlight { id: JobId { tile, generation: self.generation }, lane: Lane(0), superseded: Arc::default() };
        Running { job, known: None, answer: Answer::default(), images }
    }
}

impl Decode {
    /// Decodes `image` at `generation`; with no image, decodes nothing. A
    /// decode in flight goes on, stale, until it replies.
    pub(crate) fn set_image(&mut self, image: Option<Data>, generation: u64) {
        self.generation = generation;
        self.work = match std::mem::replace(&mut self.work, DecodeWork::Current) {
            DecodeWork::Running { job, .. } => {
                job.superseded.store(true, Ordering::Relaxed);
                DecodeWork::Running { job, image }
            }
            DecodeWork::Current | DecodeWork::Queued(_) => image.map_or(DecodeWork::Current, DecodeWork::Queued),
        };
    }

    fn is_pending(&self) -> bool {
        matches!(self.work, DecodeWork::Queued(_) | DecodeWork::Running { image: Some(_), .. })
    }

    /// Starts the decode of the queued image, of a tile of `kind`, on the
    /// lane `lane` picks. Returns the lane and the job; `None`, with the
    /// work as it was, if nothing is queued or no lane has room.
    pub(crate) fn next_job(
        &mut self,
        tile: TileKey,
        kind: TileKind,
        lane: impl FnOnce() -> Option<Lane>,
    ) -> Option<(Lane, Run)> {
        let DecodeWork::Queued(image) = &self.work else {
            return None;
        };
        let lane = lane()?;
        let image = Arc::clone(image);
        let data = Arc::clone(&image);
        let job = match kind {
            TileKind::Dem(encoding) => Job::Dem { data, encoding },
            TileKind::Raster | TileKind::Geometry => Job::Raster { data },
        };
        let id = JobId { tile, generation: self.generation };
        let superseded = Arc::new(AtomicBool::new(false));
        let in_flight = InFlight { id, lane, superseded: Arc::clone(&superseded) };
        self.work = DecodeWork::Running { job: in_flight, image: Some(image) };
        Some((lane, Run { id, job, superseded }))
    }

    /// Applies `result`, the reply to job `id`. Any reply of the current
    /// generation lets go of the image.
    pub(crate) fn apply(&mut self, id: JobId, result: JobResult) -> DecodeStep {
        if id.generation != self.generation {
            self.stale_replied();
            return DecodeStep::Stale;
        }
        self.work = DecodeWork::Current;
        match result {
            JobResult::Raster(image) => DecodeStep::Show(Contents::Raster(Arc::new(RasterBucket::new(image)))),
            JobResult::Dem(dem) => DecodeStep::Show(Contents::Dem(Arc::new(HillshadeBucket::with_dem_data(dem)))),
            JobResult::Failed(error) => DecodeStep::Failed(error),
            JobResult::Laid(_) | JobResult::NeedsDeps(_) | JobResult::NeedsData => {
                DecodeStep::Failed(Error::native("a raster tile's decode answered as a layout"))
            }
        }
    }

    /// The image the decode in flight held, if any, queues for its own.
    fn requeue(&mut self) {
        if let DecodeWork::Running { image, .. } = &mut self.work {
            self.work = image.take().map_or(DecodeWork::Current, DecodeWork::Queued);
        }
    }

    fn stale_replied(&mut self) {
        self.requeue();
    }

    fn lane_lost(&mut self, generation: u64) {
        self.generation = generation;
        self.requeue();
    }
}
