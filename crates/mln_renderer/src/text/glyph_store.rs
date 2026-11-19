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
// A character that glyph PBFs can draw takes its glyph from the first
// source that has it, in the order `pbf_first` gives: the stack's glyph PBF,
// unless the style has no glyph URL; and local glyphs, from the style's
// fallback font faces that cover it and then the platform's
// `LocalGlyphRasterizer` (`LocalGlyphs`). Local glyphs come first for CJK
// when `LocalGlyphs::ideographs` is set, and otherwise draw what the PBF
// lacks. Either way the glyph keeps its PBF glyph ID, so the layout doesn't
// change.
//
// Font faces: the style's "font-faces" are fetched as font files
// (`Resource::font_face`), or read from the bytes a face holds (an app's
// fallback faces can), and shaped here, on the render thread, with the
// `FontFaceEngine` (font_face_engine.rs): mln_text's Shaper (HarfRust)
// natively, ready at once. Text that the device's fonts shape
// (`GlyphIDType::System`) is shaped the same way, a piece at a time: each
// piece with the font that `LocalGlyphs`' rasterizer gives for its first
// character. An engine that needs time to
// get ready (the browser build loads its shaper as a second wasm module) is
// asked to as soon as the style has font faces; a font file that arrives
// before it is ready waits, parsed in the update after the engine reports
// back.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use mln_storage::{FileSource, Request, Resource, Response};
use mln_text::glyph::{
    FontData, FontFaces, Glyph, GlyphDependencies, GlyphID, GlyphIDType, GlyphMap, GlyphRange, ShapeRequests,
    face_type, get_glyph_range,
};
use mln_text::glyph_cache::{GlyphCache, GlyphCacheRange};
use mln_text::glyph_pbf::parse_glyph_pbf;
use mln_text::local_glyph_rasterizer::LocalGlyphs;
use mln_text::tagged_string::font_stack_hash;
use mln_text::weight::REGULAR_WEIGHT;
use mln_text::{BiDiRun, ShapedText, visual_runs};
use mln_util::error::Error;
use mln_util::font_stack::FontStack;
use mln_util::hash::HashMap;
use mln_util::i18n;
use mln_util::logging::{Event, Log};
use mln_util::pool::TaskGroup;
use mln_util::tiny_sdf::transform_raster_to_sdf;
use mln_vec_map::{VecMap, VecSet};
use parking_lot::Mutex;

use crate::renderer_event::RendererEvent;
use crate::text::font_face_engine::{FontFaceEngine, FontFaceShaper};
use crate::tile::inbox::{InboxSender, TileEvent, Waker};

/// Each font-face text of each font stack, shaped by its face.
pub(crate) type ShapeResults = VecMap<FontStack, VecMap<GlyphIDType, BTreeMap<Vec<char>, Arc<ShapedText>>>>;

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
    /// The PBF glyph IDs that no local font has.
    no_local: BTreeSet<GlyphID>,
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

/// One of the device's fonts loaded for shaping, with the type of its glyphs.
type SystemShaper = (GlyphIDType, Rc<dyn FontFaceShaper>);

/// The loaded HarfBuzz shapers of each font stack, by font face type.
type Shapers = VecMap<FontStack, VecMap<GlyphIDType, Rc<dyn FontFaceShaper>>>;

/// What the font-face engine's `prepare` reported, and whether a font file
/// waits for it, so that the report asks for an update; shared with its
/// `ready` callback, which may run on another thread.
#[derive(Default)]
struct EngineReadiness {
    result: Option<Result<(), String>>,
    waiting: bool,
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
    pub(crate) shaping: ShapeResults,
    /// Whether a range they needed failed, so that a retry lays the tile out
    /// again.
    pub(crate) failed: bool,
}

pub(crate) struct GlyphStore {
    glyph_url: String,
    entries: HashMap<FontStack, Entry>,
    /// What the store reported since the last `take_events`: the ranges it
    /// requested, loaded or failed to load.
    events: Vec<RendererEvent>,
    local: LocalGlyphs,
    glyph_cache: Arc<GlyphCache>,
    /// Where glyph PBFs are parsed.
    pool: TaskGroup,
    /// Asks the renderer for an update, from any thread, for the engine's
    /// report.
    wake: Waker,
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
    shapers: Shapers,
    /// The device's fonts that shaped text, by `LocalFont::key`, with the
    /// type of their glyphs: `None` for one the engine can't read.
    system_shapers: HashMap<Arc<str>, Option<SystemShaper>>,
    /// Those shapers by the type of their glyphs.
    system_types: HashMap<GlyphIDType, Rc<dyn FontFaceShaper>>,
}

impl GlyphStore {
    /// A store that draws `local` glyphs, shares `glyph_cache`, parses glyph
    /// PBFs on `pool` and loads font faces with `engine`. When the engine
    /// reports back while font files wait for it, it calls `wake`, on the
    /// engine's thread, for the `update` that takes them in.
    pub(crate) fn new(
        local: LocalGlyphs,
        glyph_cache: Arc<GlyphCache>,
        pool: TaskGroup,
        engine: Arc<dyn FontFaceEngine>,
        wake: Waker,
    ) -> GlyphStore {
        GlyphStore {
            glyph_url: String::new(),
            entries: HashMap::default(),
            events: Vec::new(),
            local,
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
            shapers: VecMap::new(),
            system_shapers: HashMap::default(),
            system_types: HashMap::default(),
        }
    }

    pub(crate) fn set_url(&mut self, url: &str) {
        if self.glyph_url != url {
            url.clone_into(&mut self.glyph_url);
        }
    }

    /// What the store reported since the last call, oldest first.
    pub(crate) fn take_events(&mut self) -> Vec<RendererEvent> {
        std::mem::take(&mut self.events)
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
        let mut failed = false;
        loop {
            let mut to_request: Vec<(FontStack, GlyphRange)> = Vec::new();
            let mut waits = false;
            for (font_stack, glyph_ids) in &dependencies.glyphs {
                let mut ranges: BTreeSet<GlyphRange> = BTreeSet::new();
                for glyph_id in glyph_ids {
                    let needs = if glyph_id.type_ == GlyphIDType::FontPBF {
                        self.pbf_glyph(font_stack, *glyph_id, since, &mut failed)
                    } else {
                        Some(get_glyph_range(*glyph_id))
                    };
                    ranges.extend(needs);
                }
                let entry = self.entries.entry(font_stack.clone()).or_default();
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
            if to_request.is_empty() {
                if waits {
                    return None;
                }
                break;
            }

            // A font file that a face holds loads at once, and the glyphs
            // that waited for it look again.
            for (font_stack, range) in to_request {
                let Some(file_source) = file_source else {
                    waits = true;
                    continue;
                };
                self.request_range(&font_stack, range, file_source, inbox);
                let entry = self.entries.get(&font_stack);
                waits |=
                    entry.is_some_and(|entry| matches!(entry.ranges.get(&range), Some(RangeState::Loading { .. })));
            }
            if waits {
                return None;
            }
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

    /// Draws the PBF glyph `glyph_id` of `font_stack` from the first source
    /// that has it, PBF or local (`pbf_first`): `None` once it is drawn or
    /// known missing, else the range that the next source needs. A range
    /// that failed during the wait that began at failure `since` counts as
    /// lacking the glyph, and sets `failed`.
    fn pbf_glyph(
        &mut self,
        font_stack: &FontStack,
        glyph_id: GlyphID,
        since: u64,
        failed: &mut bool,
    ) -> Option<GlyphRange> {
        let pbf_first = self.pbf_first(glyph_id);
        for pbf in [pbf_first, !pbf_first] {
            let source = if pbf {
                self.pbf_source(font_stack, glyph_id, since)
            } else {
                self.local_source(font_stack, glyph_id, since)
            };
            match source {
                Source::Has => return None,
                Source::Lacks { failed: true } => *failed = true,
                Source::Lacks { failed: false } => {}
                Source::Needs(range) => return Some(range),
            }
        }
        None
    }

    /// Whether the PBF glyph `glyph_id` comes from glyph PBFs before local
    /// fonts.
    fn pbf_first(&self, glyph_id: GlyphID) -> bool {
        !(self.local.ideographs && glyph_id.char().is_some_and(i18n::allows_fixed_width_glyph_generation))
    }

    /// Whether `font_stack`'s glyph PBF has `glyph_id`.
    fn pbf_source(&self, font_stack: &FontStack, glyph_id: GlyphID, since: u64) -> Source {
        if self.glyph_url.is_empty() {
            return Source::Lacks { failed: false };
        }
        let range = get_glyph_range(glyph_id);
        match self.entries.get(font_stack).and_then(|entry| entry.ranges.get(&range)) {
            Some(RangeState::Loaded(Some(glyphs))) if glyphs.contains_key(&glyph_id) => Source::Has,
            Some(RangeState::Loaded(_)) => Source::Lacks { failed: false },
            Some(RangeState::Failed { at }) if *at > since => Source::Lacks { failed: true },
            _ => Source::Needs(range),
        }
    }

    /// Draws the PBF glyph `glyph_id` of `font_stack` from local fonts: the
    /// style's fallback font faces that cover it, in order, then the
    /// platform's rasterizer.
    fn local_source(&mut self, font_stack: &FontStack, glyph_id: GlyphID, since: u64) -> Source {
        if let Some(entry) = self.entries.get(font_stack) {
            if entry.glyphs.contains_key(&glyph_id) {
                return Source::Has;
            }
            if entry.no_local.contains(&glyph_id) {
                return Source::Lacks { failed: false };
            }
        }
        let code = u32::from(glyph_id.code);
        let faces: Vec<GlyphIDType> = (self.font_faces.iter().flat_map(|faces| faces.iter()))
            .filter(|face| {
                face.name.is_empty() && face.ranges.iter().any(|&(first, last)| (first..=last).contains(&code))
            })
            .map(|face| face.type_)
            .collect();
        let mut failed = false;
        let mut glyph = None;
        for type_ in faces {
            let range = get_glyph_range(GlyphID::with_type(0, type_));
            match self.entries.get(font_stack).and_then(|entry| entry.ranges.get(&range)) {
                Some(RangeState::Loaded(_)) => {
                    glyph = self.face_glyph(font_stack, type_, glyph_id);
                    if glyph.is_some() {
                        break;
                    }
                }
                Some(RangeState::Failed { at }) if *at > since => failed = true,
                _ => return Source::Needs(range),
            }
        }
        let glyph = glyph.or_else(|| {
            let rasterizer = self.local.rasterizer.as_ref()?;
            rasterizer.rasterize_glyph(font_stack, glyph_id).map(to_sdf)
        });
        let entry = self.entries.entry(font_stack.clone()).or_default();
        if let Some(glyph) = glyph {
            entry.glyphs.insert(glyph_id, Arc::new(Glyph { id: glyph_id, ..glyph }));
            Source::Has
        } else {
            entry.no_local.insert(glyph_id);
            Source::Lacks { failed }
        }
    }

    /// The glyph of the character that the PBF glyph `glyph_id` names, from
    /// the loaded font face of `type_`, unshaped.
    fn face_glyph(&self, font_stack: &FontStack, type_: GlyphIDType, glyph_id: GlyphID) -> Option<Glyph> {
        let shaper = self.shaper(font_stack, type_)?;
        let c = glyph_id.char()?;
        let mut shaped = ShapedText::default();
        shaper.shape(&[c], false, &mut shaped);
        let [id] = shaped.glyphs[..] else { return None };
        if id.code == 0 {
            return None;
        }
        let mut glyph = shaper.rasterize(id);
        glyph.bitmap = transform_raster_to_sdf(&glyph.bitmap, 8.0, 0.25);
        Some(glyph)
    }

    /// Shapes each font-face text with its font, and the device's fonts'
    /// text with theirs, and adds the glyphs it shaped to `glyphs`.
    fn shape(&mut self, requests: &ShapeRequests, glyphs: &mut GlyphMap) -> ShapeResults {
        let mut results = ShapeResults::new();
        for (font_stack, font_types) in requests {
            let font_stack_hash = font_stack_hash(font_stack);
            for (type_, strs) in font_types {
                let shaper = self.shaper(font_stack, *type_);
                for str_ in strs {
                    let mut shaped = ShapedText::default();
                    // Shapes each run in its own direction, in visual order.
                    for run in visual_runs(str_) {
                        if *type_ == GlyphIDType::System {
                            self.shape_with_system_fonts(font_stack, &run, &mut shaped);
                        } else if let Some(shaper) = &shaper {
                            shaper.shape(&run.text, run.rtl, &mut shaped);
                        }
                    }
                    for glyph_id in &shaped.glyphs {
                        let need_shape =
                            !glyphs.get(&font_stack_hash).is_some_and(|glyphs| glyphs.contains_key(glyph_id));
                        if need_shape {
                            let glyph = self.glyph(font_stack, *glyph_id);
                            let id = glyph.id;
                            glyphs.entry(font_stack_hash).or_default().entry(id).or_insert(drawn(id).then_some(glyph));
                        }
                    }

                    results
                        .entry(font_stack.clone())
                        .or_default()
                        .entry(*type_)
                        .or_default()
                        .insert(str_.clone(), Arc::new(shaped));
                }
            }
        }
        results
    }

    /// Shapes `run` with the device's fonts, a piece at a time: each piece
    /// the characters that one font draws, with the marks and joiners after
    /// them. A character that no font has is left out.
    fn shape_with_system_fonts(&mut self, font_stack: &FontStack, run: &BiDiRun, shaped: &mut ShapedText) {
        let mut pieces: Vec<(Rc<dyn FontFaceShaper>, Vec<char>)> = Vec::new();
        for &c in &run.text {
            if i18n::extends_previous(c)
                && let Some((_, piece)) = pieces.last_mut()
            {
                piece.push(c);
                continue;
            }
            let Some(shaper) = self.system_shaper(font_stack, c) else { continue };
            match pieces.last_mut() {
                Some((last, piece)) if Rc::ptr_eq(last, &shaper) => piece.push(c),
                _ => pieces.push((shaper, vec![c])),
            }
        }
        // A right-to-left run's later pieces lie to the left of its earlier
        // ones.
        if run.rtl {
            pieces.reverse();
        }
        for (shaper, piece) in pieces {
            shaper.shape(&piece, run.rtl, shaped);
        }
    }

    /// The shaper of the device's font that draws `c` for a label in
    /// `font_stack`, loaded the first time.
    fn system_shaper(&mut self, font_stack: &FontStack, c: char) -> Option<Rc<dyn FontFaceShaper>> {
        let font = self.local.rasterizer.as_ref()?.font(font_stack, c)?;
        if let Some(loaded) = self.system_shapers.get(&font.key) {
            return loaded.as_ref().map(|(_, shaper)| Rc::clone(shaper));
        }
        self.prepare_engine();
        let type_ = face_type(&font.key, "", &[]);
        let loaded = matches!(self.engine_readiness.lock().result, Some(Ok(())))
            .then(|| self.engine.load(type_, font.data, font.weight))
            .flatten()
            .map(|shaper| (type_, Rc::<dyn FontFaceShaper>::from(shaper)));
        if let Some((type_, shaper)) = &loaded {
            self.system_types.insert(*type_, Rc::clone(shaper));
        }
        self.system_shapers.insert(font.key, loaded.clone());
        loaded.map(|(_, shaper)| shaper)
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
        if let Some(data) = self.font_face_data(range.type_) {
            // A face that holds its font file needs no request.
            self.set_range(font_stack, range, RangeState::Loading { id, request: None });
            self.events.push(RendererEvent::GlyphsRequested { font_stack: font_stack.clone(), range });
            self.process_data(Ok(Some(GlyphData::Font(data))), font_stack, range, id);
            return;
        }

        let resource = if range.type_ == GlyphIDType::FontPBF {
            Resource::glyphs(&self.glyph_url, font_stack, (range.first, range.second))
        } else {
            let url = self.font_face_url(range.type_);
            if url.is_empty() {
                // The request of an empty URL fails, as the glyphs do.
                Log::error(Event::Style, "Try download a glyph doesn't in current faces");
            }
            Resource::font_face(&url)
        };

        self.events.push(RendererEvent::GlyphsRequested { font_stack: font_stack.clone(), range });

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

    /// Takes in the answer to request `id`, the glyphs of `range` of
    /// `font_stack` or the error that failed it.
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
                let interned: GlyphCacheRange =
                    glyphs.into_iter().map(|glyph| (glyph.id, self.glyph_cache.intern_glyph(glyph))).collect();
                range_glyphs = Some(self.glyph_cache.intern_range(interned));
            }
            Some(GlyphData::Font(data)) => {
                if self.defer_until_engine_ready(&data, font_stack, range, id) {
                    return;
                }
                if self.load_shaper(font_stack, range.type_, data) {
                    let id = GlyphID::with_type(0, range.type_);
                    let entry = self.entries.entry(font_stack.clone()).or_default();
                    entry.glyphs.insert(id, Arc::new(Glyph { id, ..Glyph::default() }));
                }
            }
            None => {}
        }

        self.revision += 1;
        if let Some(error) = error {
            self.failures += 1;
            self.set_range(font_stack, range, RangeState::Failed { at: self.failures });
            let range_name = if range.type_ == GlyphIDType::FontPBF {
                format!("{}-{}", range.first, range.second)
            } else {
                format!("{}(font file)", range.type_.id())
            };
            Log::error(
                Event::Style,
                format!(
                    "Failed to load glyph range {range_name} for font stack {}:( {error})",
                    mln_util::font_stack::font_stack_to_string(font_stack)
                ),
            );
            self.events.push(RendererEvent::GlyphsError {
                font_stack: font_stack.clone(),
                range,
                error: error.clone(),
            });
            self.events.push(RendererEvent::ResourceError(error));
        } else {
            self.set_range(font_stack, range, RangeState::Loaded(range_glyphs));
            self.events.push(RendererEvent::GlyphsLoaded { font_stack: font_stack.clone(), range });
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

    pub(crate) fn glyph(&mut self, font_stack: &FontStack, glyph_id: GlyphID) -> Arc<Glyph> {
        if let Some(glyph) = self.entries.get(font_stack).and_then(|entry| entry.find_glyph(glyph_id)) {
            return glyph;
        }

        if glyph_id.type_ != GlyphIDType::FontPBF
            && let Some(shaper) = self.shaper(font_stack, glyph_id.type_)
        {
            let mut glyph = shaper.rasterize(glyph_id);
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
        let (readiness, wake) = (Arc::clone(&self.engine_readiness), Arc::clone(&self.wake));
        self.engine.prepare(Box::new(move |result| {
            if let Err(error) = &result {
                Log::error(Event::Glyph, format!("font faces can't be loaded: {error}"));
            }
            let waiting = {
                let mut readiness = readiness.lock();
                readiness.result = Some(result);
                std::mem::take(&mut readiness.waiting)
            };
            if waiting {
                wake();
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
        readiness.waiting = true;
        true
    }

    /// The shaper of a style or fallback face of `font_stack`, or of one of
    /// the device's fonts, by the type of its glyphs.
    fn shaper(&self, font_stack: &FontStack, type_: GlyphIDType) -> Option<Rc<dyn FontFaceShaper>> {
        let face = self.shapers.get(font_stack).and_then(|shapers| shapers.get(&type_));
        face.or_else(|| self.system_types.get(&type_)).cloned()
    }

    /// Loads the font face `data` for `font_stack`; false, as for an invalid
    /// font, when the engine failed to get ready.
    fn load_shaper(&mut self, font_stack: &FontStack, type_: GlyphIDType, data: FontData) -> bool {
        if !matches!(self.engine_readiness.lock().result, Some(Ok(()))) {
            return false;
        }
        let Some(shaper) = self.engine.load(type_, data, REGULAR_WEIGHT as u16) else {
            return false;
        };
        self.shapers.entry(font_stack.clone()).or_default().insert(type_, Rc::from(shaper));
        true
    }

    fn font_face_url(&self, type_: GlyphIDType) -> String {
        self.font_faces
            .iter()
            .flat_map(|faces| faces.iter())
            .find(|face| face.type_ == type_)
            .map(|face| face.url.clone())
            .unwrap_or_default()
    }

    /// The font file that the face of `type_` holds, if it holds one.
    fn font_face_data(&self, type_: GlyphIDType) -> Option<FontData> {
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

/// A local glyph's coverage as a signed distance field.
fn to_sdf(mut glyph: Glyph) -> Glyph {
    let radius = if glyph.metrics.is_double_resolution { 16.0 } else { 8.0 };
    glyph.bitmap = transform_raster_to_sdf(&glyph.bitmap, radius, 0.25);
    glyph
}

/// What a source of a PBF glyph has of it.
enum Source {
    /// The glyph, now in the store.
    Has,
    /// Not the glyph: its range failed during the wait if `failed`.
    Lacks { failed: bool },
    /// The range it waits for.
    Needs(GlyphRange),
}
