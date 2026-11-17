// In mln_renderer rather than mln_text: it requests through mln_storage
// (mln_text stays free of it).
//
// The store lives on the render thread, owned by the tile store. Tiles
// don't register with it: a tile waiting for glyphs asks it to `resolve`
// them again after it changes (`revision`). Responses arrive in the tile
// store's inbox, from whatever thread answers, and the tile store hands
// them over (`take_response`).
//
// A glyph PBF is parsed on the renderer's thread pool as soon as it arrives,
// so that its bytes don't wait for the next update and its parsing stays off
// the render thread. Its glyphs then go through the `GlyphCache` the store is
// given (thread-safe, so renderers on other threads can share it), and each
// range keeps its glyphs in one shared map; `Entry::glyphs` holds only the
// local and HarfBuzz glyphs.
//
// Font faces (MLN_TEXT_SHAPING_HARFBUZZ): the style's "font-faces" are
// fetched as font files (`Resource::fontFace`), or read from the bytes a
// face holds (an app's fallback faces can), and shaped here, on the render
// thread, with the `FontFaceEngine` (font_face_engine.rs): mln_text's
// HBShaper (HarfRust) natively, ready at once. An engine that needs time to
// get ready (the browser build loads its shaper as a second wasm module) is
// asked to as soon as the style has font faces; a font file that arrives
// before it is ready waits, parsed in the update after the engine reports
// back.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::{AsyncTask, AsyncTaskSender, RunLoop};
use mln_storage::{FileSource, Request, Resource, Response};
use mln_text::glyph::{
    FontData, FontFaces, Glyph, GlyphDependencies, GlyphID, GlyphIDType, GlyphMap, GlyphRange, HBShapeRequests,
    get_glyph_range,
};
use mln_text::glyph_cache::{GlyphCache, GlyphCacheRange};
use mln_text::glyph_pbf::parse_glyph_pbf;
use mln_text::local_glyph_rasterizer::{DefaultLocalGlyphRasterizer, LocalGlyphRasterizer};
use mln_text::tagged_string::font_stack_hash;
use mln_text::{BiDi, HBShapeAdjust};
use mln_util::error::Error;
use mln_util::font_stack::FontStack;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};
use mln_util::pool::TaskGroup;
use mln_util::tiny_sdf::transform_raster_to_sdf;
use mln_vec_map::{VecMap, VecSet};
use parking_lot::Mutex;

use crate::text::font_face_engine::{FontFaceEngine, FontFaceShaper};
use crate::tile::inbox::{InboxSender, TileEvent};

#[derive(Clone, Debug, Default, mln_wire::Wire)]
pub(crate) struct HBShapeResult {
    pub(crate) str_: Vec<u16>,
    pub(crate) adjusts: Arc<Vec<HBShapeAdjust>>,
}

pub(crate) type HBShapeResults = VecMap<FontStack, VecMap<GlyphIDType, BTreeMap<Vec<u16>, HBShapeResult>>>;

/// What a glyph range's response brought.
pub(crate) enum GlyphData {
    /// The glyphs of a glyph PBF.
    Glyphs(Vec<Glyph>),
    /// A font file, which the font-face engine loads on the render thread.
    Font(FontData),
}

/// The answer to a glyph range's request: `None` for a response without
/// content.
pub(crate) type GlyphResult = Result<Option<GlyphData>, Error>;

pub(crate) trait GlyphStoreObserver {
    fn on_glyphs_loaded(&self, _font_stack: &FontStack, _range: GlyphRange) {}
    fn on_glyphs_error(&self, _font_stack: &FontStack, _range: GlyphRange, _error: Error) {}
    fn on_glyphs_requested(&self, _font_stack: &FontStack, _range: GlyphRange) {}
}

struct NullObserver;
impl GlyphStoreObserver for NullObserver {}

/// Where a glyph range is.
enum RangeState {
    /// Requested and not answered yet, by the store's `id`th request, which
    /// `request` holds open; `None` for a font file that waits for the
    /// engine. A response to another request is stale.
    Loading {
        id: u64,
        #[expect(dead_code, reason = "dropping the request cancels it")]
        request: Option<Request>,
    },
    /// Parsed: the glyphs of a glyph PBF, or `None` for a font file or a
    /// response without content.
    Loaded(Option<Arc<GlyphCacheRange>>),
    /// The last request failed, as the store's `at`th failure.
    Failed { at: u64 },
}

#[derive(Default)]
struct Entry {
    ranges: VecMap<GlyphRange, RangeState>,
    /// Local and HarfBuzz glyphs. PBF glyphs are in the ranges.
    glyphs: BTreeMap<GlyphID, Arc<Glyph>>,
}

impl Entry {
    fn find_glyph(&self, glyph_id: GlyphID) -> Option<Arc<Glyph>> {
        if let Some(glyph) = self.glyphs.get(&glyph_id) {
            return Some(Arc::clone(glyph));
        }
        if glyph_id.type_ != GlyphIDType::FontPBF {
            return None;
        }
        match self.ranges.get(&get_glyph_range(glyph_id))? {
            RangeState::Loaded(Some(glyphs)) => glyphs.get(&glyph_id).cloned(),
            _ => None,
        }
    }
}

/// The loaded HarfBuzz shapers of each font stack, by font face type.
type HBShapers = VecMap<FontStack, VecMap<GlyphIDType, Rc<dyn FontFaceShaper>>>;

/// What the font-face engine's `prepare` reported, and how to wake the
/// renderer when it does if a font file waits for it; shared with its
/// `ready` callback, which may run on another thread.
#[derive(Default)]
struct EngineReadiness {
    result: Option<Result<(), String>>,
    waker: Option<AsyncTaskSender>,
}

/// One wait for glyphs, across the resolves it takes. A range that failed
/// before the wait began is requested again; one that fails during it
/// resolves as missing.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GlyphWait {
    /// The store's failure count when the wait began.
    since: Option<u64>,
}

/// What a tile's glyph dependencies resolved to.
pub(crate) struct ResolvedGlyphs {
    /// The glyphs, the shaped glyphs of font-face text included: `None` for
    /// one that no font has or whose range failed.
    pub(crate) glyphs: GlyphMap,
    /// The shaping of the font-face text.
    pub(crate) shaping: HBShapeResults,
    /// Whether a range they needed failed, so that a retry lays the tile out
    /// again.
    pub(crate) failed: bool,
}

pub(crate) struct GlyphStore {
    /// The renderer's loop, where the engine's report arrives.
    run_loop: RunLoop,
    glyph_url: String,
    entries: HashMap<FontStack, Entry>,
    observer: Rc<dyn GlyphStoreObserver>,
    local_glyph_rasterizer: Box<dyn LocalGlyphRasterizer>,
    glyph_cache: Arc<GlyphCache>,
    /// Where glyph PBFs are parsed.
    pool: TaskGroup,
    /// Asks the renderer for an update, for the engine's report.
    wake: Rc<dyn Fn()>,
    /// How many requests failed.
    failures: u64,
    /// How many ranges were requested.
    requests: u64,
    /// Changes whenever a range is answered or dropped, so waiting tiles
    /// resolve again.
    revision: u64,

    // Shaping objects
    font_faces: Option<Arc<FontFaces>>,
    engine: Arc<dyn FontFaceEngine>,
    /// Whether `engine.prepare` was called.
    engine_prepared: bool,
    engine_readiness: Arc<Mutex<EngineReadiness>>,
    /// Font files that arrived before the engine was ready, with the
    /// request each answers.
    deferred_responses: Vec<(FontData, FontStack, GlyphRange, u64)>,
    /// Wakes the renderer to parse `deferred_responses` once the engine
    /// reports back.
    deferred_task: Option<AsyncTask>,
    hb_shapers: HBShapers,
}

impl GlyphStore {
    /// A store that shares `glyph_cache`, parses glyph PBFs on `pool`, loads
    /// font faces with `engine`, hears from the engine on `run_loop`, the
    /// renderer's, and calls `wake` there when that report is in for
    /// `update`.
    pub(crate) fn new(
        local_glyph_rasterizer: Option<Box<dyn LocalGlyphRasterizer>>,
        glyph_cache: Arc<GlyphCache>,
        pool: TaskGroup,
        engine: Arc<dyn FontFaceEngine>,
        run_loop: RunLoop,
        wake: Rc<dyn Fn()>,
    ) -> GlyphStore {
        let local_glyph_rasterizer =
            local_glyph_rasterizer.unwrap_or_else(|| Box::new(DefaultLocalGlyphRasterizer::new(None)));
        GlyphStore {
            run_loop,
            glyph_url: String::new(),
            entries: HashMap::default(),
            observer: Rc::new(NullObserver),
            local_glyph_rasterizer,
            glyph_cache,
            pool,
            wake,
            failures: 0,
            requests: 0,
            revision: 0,
            font_faces: None,
            engine,
            engine_prepared: false,
            engine_readiness: Arc::default(),
            deferred_responses: Vec::new(),
            deferred_task: None,
            hb_shapers: VecMap::new(),
        }
    }

    pub(crate) fn set_url(&mut self, url: &str) {
        if self.glyph_url != url {
            url.clone_into(&mut self.glyph_url);
        }
    }

    pub(crate) fn set_observer(&mut self, observer: Option<Rc<dyn GlyphStoreObserver>>) {
        self.observer = observer.unwrap_or_else(|| Rc::new(NullObserver));
    }

    /// Changes whenever a resolve that waited could now give another answer.
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// Takes in the answer to request `id`, for `font_stack`'s `range`,
    /// unless the range was dropped or requested again since.
    pub(crate) fn take_response(&mut self, font_stack: &FontStack, range: GlyphRange, id: u64, result: GlyphResult) {
        if self.is_loading(font_stack, range, id) {
            self.process_data(result, font_stack, range, id);
        }
    }

    /// Takes in the font files that waited for the engine once it reported
    /// back.
    pub(crate) fn update(&mut self) {
        if !self.deferred_responses.is_empty() && self.engine_readiness.lock().result.is_some() {
            for (data, font_stack, range, id) in std::mem::take(&mut self.deferred_responses) {
                if self.is_loading(&font_stack, range, id) {
                    self.process_data(Ok(Some(GlyphData::Font(data))), &font_stack, range, id);
                }
            }
        }
    }

    /// Whether `font_stack`'s `range` waits for request `id`.
    fn is_loading(&self, font_stack: &FontStack, range: GlyphRange, id: u64) -> bool {
        let state = self.entries.get(font_stack).and_then(|entry| entry.ranges.get(&range));
        matches!(state, Some(RangeState::Loading { id: loading, .. }) if *loading == id)
    }

    /// The glyphs of `dependencies`, and the shaping of their font-face
    /// text, once every range they need is loaded or failed during `wait`.
    /// Until then `None`, having requested the ranges that need it from
    /// `file_source`, whose responses go to `inbox`; without one, those
    /// ranges never load.
    pub(crate) fn resolve(
        &mut self,
        dependencies: &GlyphDependencies,
        wait: &mut GlyphWait,
        file_source: Option<&Arc<dyn FileSource>>,
        inbox: &InboxSender,
    ) -> Option<ResolvedGlyphs> {
        let since = *wait.since.get_or_insert(self.failures);
        let mut to_request: Vec<(FontStack, GlyphRange)> = Vec::new();
        let mut waits = false;
        let mut failed = false;
        for (font_stack, glyph_ids) in &dependencies.glyphs {
            let entry = self.entries.entry(font_stack.clone()).or_default();
            let mut ranges: BTreeSet<GlyphRange> = BTreeSet::new();
            for glyph_id in glyph_ids {
                if self.local_glyph_rasterizer.can_rasterize_glyph(font_stack, *glyph_id) {
                    if !entry.glyphs.contains_key(glyph_id) {
                        let glyph = generate_local_sdf(&*self.local_glyph_rasterizer, font_stack, *glyph_id);
                        entry.glyphs.insert(*glyph_id, Arc::new(glyph));
                    }
                } else {
                    ranges.insert(get_glyph_range(*glyph_id));
                }
            }
            for range in ranges {
                match entry.ranges.get(&range) {
                    Some(RangeState::Loaded(_)) => {}
                    Some(RangeState::Failed { at }) if *at > since => failed = true,
                    Some(RangeState::Loading { .. }) => waits = true,
                    // Never requested, or failed before this wait began.
                    _ => to_request.push((font_stack.clone(), range)),
                }
            }
        }

        // A font file that a face holds loads at once.
        for (font_stack, range) in to_request {
            let Some(file_source) = file_source else {
                waits = true;
                continue;
            };
            self.request_range(&font_stack, range, file_source, inbox);
            let entry = self.entries.get(&font_stack);
            waits |= entry.is_some_and(|entry| matches!(entry.ranges.get(&range), Some(RangeState::Loading { .. })));
        }
        if waits {
            return None;
        }

        let mut glyphs = GlyphMap::new();
        for (font_stack, glyph_ids) in &dependencies.glyphs {
            let entry = self.entries.entry(font_stack.clone()).or_default();
            let font_glyphs = glyphs.entry(font_stack_hash(font_stack)).or_default();
            for glyph_id in glyph_ids {
                // A font face's glyph 0 only marks its file as a dependency.
                let glyph = drawn(*glyph_id).then(|| entry.find_glyph(*glyph_id)).flatten();
                font_glyphs.entry(*glyph_id).or_insert(glyph);
            }
        }
        let shaping = self.shape(&dependencies.shapes, &mut glyphs);
        Some(ResolvedGlyphs { glyphs, shaping, failed })
    }

    /// Shapes each font-face text with its font, and adds the glyphs it
    /// shaped to `glyphs`.
    fn shape(&mut self, requests: &HBShapeRequests, glyphs: &mut GlyphMap) -> HBShapeResults {
        let mut results = HBShapeResults::new();
        let mut bidi = BiDi::new();
        for (font_stack, font_types) in requests {
            let font_stack_hash = font_stack_hash(font_stack);
            for (type_, strs) in font_types {
                let shaper = self.get_hb_shaper(font_stack, *type_);
                for str_ in strs {
                    let mut shaped_glyph_ids = Vec::new();
                    let mut shaped_adjusts = Vec::new();
                    // Shapes each run in its own direction, in visual order.
                    if let Some(shaper) = &shaper {
                        for run in bidi.visual_runs(str_) {
                            shaper.shape(&run.text, run.rtl, &mut shaped_glyph_ids, &mut shaped_adjusts);
                        }
                    }
                    let mut shaped_str: Vec<u16> = Vec::with_capacity(shaped_glyph_ids.len());
                    for glyph_id in &shaped_glyph_ids {
                        shaped_str.push(glyph_id.code);
                        let need_shape =
                            !glyphs.get(&font_stack_hash).is_some_and(|glyphs| glyphs.contains_key(glyph_id));
                        if need_shape {
                            let glyph = self.get_glyph(font_stack, *glyph_id);
                            let id = glyph.id;
                            glyphs.entry(font_stack_hash).or_default().entry(id).or_insert(drawn(id).then_some(glyph));
                        }
                    }

                    results
                        .entry(font_stack.clone())
                        .or_default()
                        .entry(*type_)
                        .or_default()
                        .insert(str_.clone(), HBShapeResult { str_: shaped_str, adjusts: Arc::new(shaped_adjusts) });
                }
            }
        }
        results
    }

    fn request_range(
        &mut self,
        font_stack: &FontStack,
        range: GlyphRange,
        file_source: &Arc<dyn FileSource>,
        inbox: &InboxSender,
    ) {
        self.requests += 1;
        let id = self.requests;
        if let Some(data) = self.get_font_face_data(range.type_) {
            // A face that holds its font file needs no request.
            self.set_range(font_stack, range, RangeState::Loading { id, request: None });
            self.observer.on_glyphs_requested(font_stack, range);
            self.process_data(Ok(Some(GlyphData::Font(data))), font_stack, range, id);
            return;
        }

        let resource = if range.type_ == GlyphIDType::FontPBF {
            Resource::glyphs(&self.glyph_url, font_stack, (range.first, range.second))
        } else {
            let url = self.get_font_face_url(range.type_);
            if url.is_empty() {
                // C++ logs and requests an empty Resource.
                Log::error(Event::Style, "Try download a glyph doesn't in current faces");
            }
            Resource::font_face(&url)
        };

        self.observer.on_glyphs_requested(font_stack, range);

        let inbox = inbox.clone();
        let callback_font_stack = font_stack.clone();
        let pool = (range.type_ == GlyphIDType::FontPBF).then(|| self.pool.clone());
        // Only the first answer applies: the range is no longer loading once
        // it has one. A revalidation's not-modified isn't an answer.
        let mut answered = false;
        let (req, responder) = Request::new(move |response: Response| {
            if response.not_modified || std::mem::replace(&mut answered, true) {
                return;
            }
            let font_stack = callback_font_stack.clone();
            let inbox = inbox.clone();
            match (font_data(response), &pool) {
                (Ok(Some(data)), Some(pool)) => {
                    pool.spawn(move || {
                        let result = parse_glyphs(range, &data);
                        inbox.send(TileEvent::Glyphs { font_stack, range, id, result });
                    });
                }
                (result, _) => {
                    let result = result.map(|data| data.map(GlyphData::Font));
                    inbox.send(TileEvent::Glyphs { font_stack, range, id, result });
                }
            }
        });
        file_source.load(&resource, responder);
        self.set_range(font_stack, range, RangeState::Loading { id, request: Some(req) });
    }

    fn set_range(&mut self, font_stack: &FontStack, range: GlyphRange, state: RangeState) {
        self.entries.entry(font_stack.clone()).or_default().ranges.insert(range, state);
    }

    /// The rest of `processResponse`, for the answer to request `id`.
    fn process_data(&mut self, result: GlyphResult, font_stack: &FontStack, range: GlyphRange, id: u64) {
        let (data, error) = match result {
            Ok(data) => (data, None),
            Err(error) => (None, Some(error)),
        };
        let mut range_glyphs = None;
        match data {
            Some(GlyphData::Glyphs(glyphs)) => {
                // Interned here rather than on the pool, where a renderer's
                // ranges would contend for the cache's lock.
                let interned: GlyphCacheRange = glyphs
                    .into_iter()
                    .filter(|glyph| !self.local_glyph_rasterizer.can_rasterize_glyph(font_stack, glyph.id))
                    .map(|glyph| (glyph.id, self.glyph_cache.intern_glyph(glyph)))
                    .collect();
                range_glyphs = Some(self.glyph_cache.intern_range(interned));
            }
            Some(GlyphData::Font(data)) => {
                if self.defer_until_engine_ready(&data, font_stack, range, id) {
                    return;
                }
                if self.load_hb_shaper(font_stack, range.type_, data) {
                    let id = GlyphID::with_type(0, range.type_);
                    if !self.local_glyph_rasterizer.can_rasterize_glyph(font_stack, id) {
                        let entry = self.entries.entry(font_stack.clone()).or_default();
                        entry.glyphs.insert(id, Arc::new(Glyph { id, ..Glyph::default() }));
                    }
                }
            }
            None => {}
        }

        self.revision += 1;
        if let Some(error) = error {
            self.failures += 1;
            self.set_range(font_stack, range, RangeState::Failed { at: self.failures });
            self.observer.on_glyphs_error(font_stack, range, error);
        } else {
            self.set_range(font_stack, range, RangeState::Loaded(range_glyphs));
            self.observer.on_glyphs_loaded(font_stack, range);
        }
    }

    /// Drops the glyphs and pending requests of the font stacks not in
    /// `keep`. A tile still waiting for them requests them again.
    pub(crate) fn evict(&mut self, keep: &VecSet<FontStack>) {
        let before = self.entries.len();
        self.entries.retain(|font_stack, _| keep.contains(font_stack));
        if self.entries.len() != before {
            self.revision += 1;
        }
    }

    pub(crate) fn get_glyph(&mut self, font_stack: &FontStack, glyph_id: GlyphID) -> Arc<Glyph> {
        if let Some(glyph) = self.entries.get(font_stack).and_then(|entry| entry.find_glyph(glyph_id)) {
            return glyph;
        }

        if glyph_id.type_ != GlyphIDType::FontPBF
            && let Some(shaper) = self.get_hb_shaper(font_stack, glyph_id.type_)
        {
            let mut glyph = shaper.rasterize(&glyph_id);
            glyph.bitmap = transform_raster_to_sdf(&glyph.bitmap, 8.0, 0.25);
            let glyph = Arc::new(glyph);
            self.entries.entry(font_stack.clone()).or_default().glyphs.insert(glyph_id, Arc::clone(&glyph));
            return glyph;
        }

        Arc::new(Glyph::default())
    }

    /// Sets the style's font faces. Once a style has font faces, the engine
    /// gets ready to load them while their files download.
    pub(crate) fn set_font_faces(&mut self, faces: Option<Arc<FontFaces>>) {
        let has_faces = faces.as_ref().is_some_and(|faces| !faces.is_empty());
        self.font_faces = faces;
        if has_faces {
            self.prepare_engine();
        }
    }

    /// Asks the engine to get ready, once.
    fn prepare_engine(&mut self) {
        if std::mem::replace(&mut self.engine_prepared, true) {
            return;
        }
        let readiness = Arc::clone(&self.engine_readiness);
        self.engine.prepare(Box::new(move |result| {
            if let Err(error) = &result {
                Log::error(Event::Glyph, format!("font faces can't be loaded: {error}"));
            }
            let waker = {
                let mut readiness = readiness.lock();
                readiness.result = Some(result);
                readiness.waker.take()
            };
            if let Some(waker) = waker {
                waker.send();
            }
        }));
    }

    /// Whether `data`, a font file, has to wait for the engine: if so, keeps
    /// it for `update`, which parses it once the engine reports back.
    fn defer_until_engine_ready(
        &mut self,
        data: &FontData,
        font_stack: &FontStack,
        range: GlyphRange,
        id: u64,
    ) -> bool {
        self.prepare_engine();
        let mut readiness = self.engine_readiness.lock();
        if readiness.result.is_some() {
            return false;
        }
        self.deferred_responses.push((data.clone(), font_stack.clone(), range, id));
        let wake = Rc::clone(&self.wake);
        let task = self.deferred_task.get_or_insert_with(|| AsyncTask::new(&self.run_loop, move || wake()));
        readiness.waker = Some(task.sender());
        true
    }

    fn get_hb_shaper(&self, font_stack: &FontStack, type_: GlyphIDType) -> Option<Rc<dyn FontFaceShaper>> {
        self.hb_shapers.get(font_stack).and_then(|shapers| shapers.get(&type_)).cloned()
    }

    /// Loads the font face `data` for `font_stack`; false, as for an invalid
    /// font, when the engine failed to get ready.
    fn load_hb_shaper(&mut self, font_stack: &FontStack, type_: GlyphIDType, data: FontData) -> bool {
        if !matches!(self.engine_readiness.lock().result, Some(Ok(()))) {
            return false;
        }
        let Some(shaper) = self.engine.load(type_, data) else {
            return false;
        };
        self.hb_shapers.entry(font_stack.clone()).or_default().insert(type_, Rc::from(shaper));
        true
    }

    fn get_font_face_url(&self, type_: GlyphIDType) -> String {
        self.font_faces
            .iter()
            .flat_map(|faces| faces.iter())
            .find(|face| face.type_ == type_)
            .map(|face| face.url.clone())
            .unwrap_or_default()
    }

    /// The font file that the face of `type_` holds, if it holds one.
    fn get_font_face_data(&self, type_: GlyphIDType) -> Option<FontData> {
        if type_ == GlyphIDType::FontPBF {
            return None;
        }
        self.font_faces.as_ref()?.iter().find(|face| face.type_ == type_)?.data.clone()
    }
}

/// What `response`, which isn't a not-modified one, brought: `None` when it
/// had no content.
fn font_data(response: Response) -> Result<Option<FontData>, Error> {
    match response.error {
        Some(error) => Err(Error::native(error.message)),
        None => Ok((!response.no_content).then(|| FontData::from(response.data.unwrap_or_default()))),
    }
}

/// The glyphs of `range`'s glyph PBF `data`.
fn parse_glyphs(range: GlyphRange, data: &FontData) -> GlyphResult {
    let glyphs = parse_glyph_pbf(&range, data.as_bytes()).map_err(|error| Error::native(error.to_string()))?;
    Ok(Some(GlyphData::Glyphs(glyphs)))
}

/// Whether `glyph_id` is a glyph a layout draws: a font face's glyph 0 only
/// marks its font file as a dependency.
fn drawn(glyph_id: GlyphID) -> bool {
    glyph_id.code != 0 || glyph_id.type_ == GlyphIDType::FontPBF
}

fn generate_local_sdf(rasterizer: &dyn LocalGlyphRasterizer, font_stack: &FontStack, glyph_id: GlyphID) -> Glyph {
    let mut local = rasterizer.rasterize_glyph(font_stack, glyph_id);
    let radius = if local.metrics.is_double_resolution { 16.0 } else { 8.0 };
    local.bitmap = transform_raster_to_sdf(&local.bitmap, radius, 0.25);
    local
}
