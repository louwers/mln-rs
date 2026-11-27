// The store lives on the render thread, owned by the tile store. Tiles
// don't register with it: a tile waiting for glyphs asks it to `resolve`
// them again after it changes (`revision`). A tile laid out without a range
// that failed lays out again once a later request loads it: the store
// reports each such range once (`take_recovered`). Responses arrive in the
// tile store's inbox, from whatever thread answers, and the tile store hands
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
// fallback font faces that cover it, then the face of the device's font
// that `LocalGlyphs`' chooser chose for it, then `LocalGlyphs`' rasterizer
// (the browser's). Local glyphs come first for CJK when
// `LocalGlyphs::ideographs` is set, and otherwise draw what the PBF lacks.
// Either way the glyph keeps its PBF glyph ID, so the layout doesn't change.
// A glyph drawn because a source before it failed stands in until a failed
// range of its stack loads; the next resolve then draws it again.
//
// Font faces: the style's "font-faces" are fetched as font files
// (`Resource::font_face`), or read from the bytes a face holds (an app's
// fallback faces can), and shaped here, on the render thread, with the
// `FontFaceEngine` (font_face_engine.rs): `Shaper` (HarfRust)
// natively, ready at once. An engine that needs time to get ready (the
// browser build loads its shaper as a second wasm module) is asked to as
// soon as the style has font faces; a font file that arrives before it is
// ready waits, parsed in the update after the engine reports back.
//
// The device's fonts: text that they shape (`GlyphIDType::SYSTEM`) is
// shaped a piece at a time, each piece the clusters (`clusters`) that one
// face was chosen for, and a local glyph comes from the face chosen for its
// character. The chooser can block on the platform's font matcher and font
// files, so it runs on a serial lane of the pool, never here: a resolve that
// meets a cluster not chosen yet asks about it and waits, as for a glyph
// range, and the answer arrives in the inbox (`take_device_fonts`). A chosen
// face holds its font file, and loads, waits for the engine and shapes as
// the style's faces do.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use mln_style::collections::{VecMap, VecSet};
use mln_style::error::Error;
use mln_style::hash::HashMap;
use mln_style::i18n;
use parking_lot::Mutex;

use crate::exec::pool::TaskGroup;
use crate::log_target;
use crate::render::events::RendererEvent;
use crate::storage::{FileSource, Request, Resource, Response};
use crate::text::font_face_engine::{FontFaceEngine, FontFaceShaper};
use crate::text::font_stack::FontStack;
use crate::text::glyph::{
    FontData, FontFace, FontFaces, Glyph, GlyphDependencies, GlyphID, GlyphIDType, GlyphMap, GlyphRange, ShapeRequests,
    get_glyph_range,
};
use crate::text::glyph_cache::{GlyphCache, GlyphCacheRange};
use crate::text::glyph_pbf::parse_glyph_pbf;
use crate::text::local_fonts::LocalGlyphs;
use crate::text::tagged_string::font_stack_hash;
use crate::text::tiny_sdf::transform_raster_to_sdf;
use crate::text::weight::REGULAR_WEIGHT;
use crate::text::{BiDiRun, ShapedText, visual_runs};
use crate::tile_store::inbox::{InboxSender, TileEvent, Waker};

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

/// Glyph ranges, each with its font stack.
pub(crate) type GlyphRanges = VecSet<(FontStack, GlyphRange)>;

/// Where a glyph range is.
enum RangeState {
    /// Requested and not answered yet, by the store's `id`th request, which
    /// `request` holds open; `None` for a font file that waits for the
    /// engine. A response to another request is stale. `retry` if the
    /// range's last request failed.
    Loading {
        id: u64,
        #[expect(dead_code, reason = "dropping the request cancels it")]
        request: Option<Request>,
        retry: bool,
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
    /// Local glyphs and the glyphs of font files. PBF glyphs are in the
    /// ranges.
    glyphs: BTreeMap<GlyphID, Arc<Glyph>>,
    /// The PBF glyph IDs that no local font has, of the faces that loaded.
    no_local: BTreeSet<GlyphID>,
    /// The local glyphs in `glyphs` drawn because a source before theirs
    /// failed. They go once a failed range of the stack loads, so that the
    /// next resolve draws them from the first source again.
    stand_ins: BTreeSet<GlyphID>,
    /// The device fonts of the characters asked about.
    device: HashMap<Vec<char>, DeviceFont>,
}

/// Where the device font of a character of a font stack stands.
#[derive(Clone, Copy)]
enum DeviceFont {
    /// Asked of the chooser, not answered yet.
    Asked,
    /// The chosen face's type, or `None` where no device font draws it.
    Chosen(Option<GlyphIDType>),
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
    /// The ranges they lacked glyphs of because the ranges failed to load,
    /// so that the tile lays out again once one loads.
    pub(crate) failed: GlyphRanges,
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
    /// Where the chooser runs: one choice at a time, off the render thread.
    /// Never waited on, since `TaskGroup::wait` runs queued tasks on the
    /// waiting thread.
    choices: TaskGroup,
    /// The characters to ask the chooser about at the end of this resolve.
    asks: VecMap<FontStack, Vec<Vec<char>>>,
    /// The device faces chosen so far, found by type after the style's faces.
    device_faces: Vec<FontFace>,
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
    /// The ranges that loaded after failing, since the last
    /// `take_recovered`.
    recovered: GlyphRanges,

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
}

impl GlyphStore {
    /// A store that draws `local` glyphs, shares `glyph_cache`, parses glyph
    /// PBFs and chooses device fonts on `pool`, and loads font faces with
    /// `engine`. When the engine reports back while font files wait for it,
    /// it calls `wake`, on the engine's thread, for the `update` that takes
    /// them in.
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
            choices: TaskGroup::serial(pool.pool()),
            pool,
            asks: VecMap::new(),
            device_faces: Vec::new(),
            wake,
            failures: 0,
            requests: 0,
            revision: 0,
            recovered: GlyphRanges::default(),
            font_faces: None,
            engine,
            engine_prepared: false,
            engine_readiness: Arc::default(),
            deferred_responses: Vec::new(),
            shapers: VecMap::new(),
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

    /// The ranges that loaded since the last call after a request for them
    /// failed. A resolve that lacked one of them (`ResolvedGlyphs::failed`)
    /// would now have its glyphs.
    pub(crate) fn take_recovered(&mut self) -> GlyphRanges {
        std::mem::take(&mut self.recovered)
    }

    /// Takes in the answer to request `id`, for `font_stack`'s `range`,
    /// unless the range was dropped or requested again since.
    pub(crate) fn take_response(&mut self, font_stack: &FontStack, range: GlyphRange, id: u64, result: GlyphResult) {
        if self.is_loading(font_stack, range, id) {
            self.process_data(result, font_stack, range, id);
        }
    }

    /// Takes in the chooser's answer for clusters of `font_stack`. Each
    /// cluster still asked about takes its face, which the store keeps, and
    /// waiting resolves look again. An answer for an evicted stack, or for a
    /// cluster not asked about, is dropped.
    pub(crate) fn take_device_fonts(&mut self, font_stack: &FontStack, faces: Vec<(Vec<char>, Option<FontFace>)>) {
        let Some(entry) = self.entries.get_mut(font_stack) else { return };
        let mut changed = false;
        for (cluster, face) in faces {
            let Some(state @ DeviceFont::Asked) = entry.device.get_mut(&cluster) else { continue };
            *state = DeviceFont::Chosen(face.as_ref().map(|face| face.type_));
            changed = true;
            if let Some(face) = face
                && !self.device_faces.iter().any(|known| known.type_ == face.type_)
            {
                self.device_faces.push(face);
            }
        }
        if changed {
            self.revision += 1;
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
    /// text, once every device font they need is chosen and every range
    /// loaded or failed during `wait`. Until then `None`, having asked the
    /// chooser about the characters not chosen yet and requested the ranges
    /// not loaded from `file_source`, which answer to `inbox`. A range of a
    /// face that holds its font file loads at once, without a file source;
    /// without one, the others never load.
    pub(crate) fn resolve(
        &mut self,
        dependencies: &GlyphDependencies,
        wait: &mut GlyphWait,
        file_source: Option<&Arc<dyn FileSource>>,
        inbox: &InboxSender,
    ) -> Option<ResolvedGlyphs> {
        let resolved = self.resolve_inner(dependencies, wait, file_source, inbox);
        self.ask_device_fonts(inbox);
        resolved
    }

    /// `resolve`, before it asks about the characters this one queued.
    fn resolve_inner(
        &mut self,
        dependencies: &GlyphDependencies,
        wait: &mut GlyphWait,
        file_source: Option<&Arc<dyn FileSource>>,
        inbox: &InboxSender,
    ) -> Option<ResolvedGlyphs> {
        let since = *wait.since.get_or_insert(self.failures);
        let mut failed = GlyphRanges::default();
        loop {
            let mut waits = false;
            let mut needed: Vec<(&FontStack, GlyphRange)> = Vec::new();
            for (font_stack, glyph_ids) in &dependencies.glyphs {
                for glyph_id in glyph_ids {
                    let source = if glyph_id.type_ == GlyphIDType::FontPBF {
                        self.pbf_glyph(font_stack, *glyph_id, since)
                    } else {
                        Source::Needs(get_glyph_range(*glyph_id))
                    };
                    match source {
                        Source::Has { failed: ranges } | Source::Lacks { failed: ranges } => {
                            failed.extend(ranges.into_iter().map(|range| (font_stack.clone(), range)));
                        }
                        Source::Needs(range) => needed.push((font_stack, range)),
                        Source::Waits => waits = true,
                    }
                }
            }
            for (font_stack, types) in &dependencies.shapes {
                // The clusters that `shape_with_system_fonts` shapes.
                for run in types.get(&GlyphIDType::SYSTEM).into_iter().flatten().flat_map(|text| visual_runs(text)) {
                    for cluster in clusters(&run.text) {
                        match self.device_font(font_stack, cluster) {
                            DeviceFont::Asked => waits = true,
                            DeviceFont::Chosen(Some(type_)) => {
                                needed.push((font_stack, get_glyph_range(GlyphID::with_type(0, type_))));
                            }
                            DeviceFont::Chosen(None) => {}
                        }
                    }
                }
            }
            needed.sort_unstable();
            needed.dedup();

            let mut to_request: Vec<(FontStack, GlyphRange)> = Vec::new();
            for (font_stack, range) in needed {
                match self.entries.get(font_stack).and_then(|entry| entry.ranges.get(&range)) {
                    Some(RangeState::Loaded(_)) => {}
                    Some(RangeState::Failed { at }) if *at > since => {
                        failed.insert((font_stack.clone(), range));
                    }
                    Some(RangeState::Loading { .. }) => waits = true,
                    // Never requested, or failed before this wait began.
                    _ => to_request.push((font_stack.clone(), range)),
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
                if let Some(data) = self.face(range.type_).and_then(|face| face.data.clone()) {
                    self.load_held(&font_stack, range, data);
                } else if let Some(file_source) = file_source {
                    self.request_range(&font_stack, range, file_source, inbox);
                } else {
                    waits = true;
                    continue;
                }
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
    /// that has it, PBF or local (`pbf_first`): the first source's answer
    /// that isn't `Lacks`, else `Lacks` with the ranges of either that
    /// failed during the wait that began at failure `since`.
    fn pbf_glyph(&mut self, font_stack: &FontStack, glyph_id: GlyphID, since: u64) -> Source {
        let pbf_first = self.pbf_first(glyph_id);
        let mut failed = Vec::new();
        for pbf in [pbf_first, !pbf_first] {
            let source = if pbf {
                self.pbf_source(font_stack, glyph_id, since)
            } else {
                self.local_source(font_stack, glyph_id, since)
            };
            match source {
                Source::Lacks { failed: lacked } => failed.extend(lacked),
                Source::Has { failed: lacked } => {
                    failed.extend(lacked);
                    if !pbf && let Some(entry) = self.entries.get_mut(font_stack) {
                        if failed.is_empty() {
                            entry.stand_ins.remove(&glyph_id);
                        } else {
                            entry.stand_ins.insert(glyph_id);
                        }
                    }
                    return Source::Has { failed };
                }
                source => return source,
            }
        }
        Source::Lacks { failed }
    }

    /// Whether the PBF glyph `glyph_id` comes from glyph PBFs before local
    /// fonts.
    fn pbf_first(&self, glyph_id: GlyphID) -> bool {
        !(self.local.ideographs && glyph_id.char().is_some_and(i18n::allows_fixed_width_glyph_generation))
    }

    /// Whether `font_stack`'s glyph PBF has `glyph_id`.
    fn pbf_source(&self, font_stack: &FontStack, glyph_id: GlyphID, since: u64) -> Source {
        if self.glyph_url.is_empty() {
            return Source::Lacks { failed: Vec::new() };
        }
        let range = get_glyph_range(glyph_id);
        match self.entries.get(font_stack).and_then(|entry| entry.ranges.get(&range)) {
            Some(RangeState::Loaded(Some(glyphs))) if glyphs.contains_key(&glyph_id) => {
                Source::Has { failed: Vec::new() }
            }
            Some(RangeState::Loaded(_)) => Source::Lacks { failed: Vec::new() },
            Some(RangeState::Failed { at }) if *at > since => Source::Lacks { failed: vec![range] },
            _ => Source::Needs(range),
        }
    }

    /// Draws the PBF glyph `glyph_id` of `font_stack` from local fonts: the
    /// style's fallback font faces that cover it, in order, then the device
    /// font chosen for it, then the rasterizer.
    fn local_source(&mut self, font_stack: &FontStack, glyph_id: GlyphID, since: u64) -> Source {
        if let Some(entry) = self.entries.get(font_stack) {
            // A stand-in's sources are looked at again, so that a failed
            // face is requested again.
            if entry.glyphs.contains_key(&glyph_id) && !entry.stand_ins.contains(&glyph_id) {
                return Source::Has { failed: Vec::new() };
            }
            if entry.no_local.contains(&glyph_id) {
                return Source::Lacks { failed: Vec::new() };
            }
        }
        let code = u32::from(glyph_id.code);
        let faces: Vec<GlyphIDType> = (self.font_faces.iter().flat_map(|faces| faces.iter()))
            .filter(|face| {
                face.name.is_empty()
                    && face.type_ != GlyphIDType::SYSTEM
                    && face.ranges.iter().any(|&(first, last)| (first..=last).contains(&code))
            })
            .map(|face| face.type_)
            .collect();
        let mut failed = Vec::new();
        let mut glyph = None;
        for type_ in faces {
            match self.face_source(font_stack, type_, glyph_id, since, &mut failed) {
                Ok(None) => {}
                Ok(found) => {
                    glyph = found;
                    break;
                }
                Err(range) => return Source::Needs(range),
            }
        }
        if glyph.is_none()
            && let Some(c) = glyph_id.char()
        {
            match self.device_font(font_stack, &[c]) {
                DeviceFont::Asked => return Source::Waits,
                DeviceFont::Chosen(Some(type_)) => {
                    match self.face_source(font_stack, type_, glyph_id, since, &mut failed) {
                        Ok(found) => glyph = found,
                        Err(range) => return Source::Needs(range),
                    }
                }
                DeviceFont::Chosen(None) => {}
            }
        }
        if glyph.is_none() && self.entries.get(font_stack).is_some_and(|entry| entry.glyphs.contains_key(&glyph_id)) {
            // The stand-in the rasterizer drew.
            return Source::Has { failed };
        }
        let glyph = glyph.or_else(|| {
            let rasterizer = self.local.rasterizer.as_ref()?;
            rasterizer.rasterize_glyph(font_stack, glyph_id).map(to_sdf)
        });
        let entry = self.entries.entry(font_stack.clone()).or_default();
        if let Some(glyph) = glyph {
            entry.glyphs.insert(glyph_id, Arc::new(Glyph { id: glyph_id, ..glyph }));
            Source::Has { failed }
        } else {
            // A face that failed may have the glyph: a later wait requests
            // its file again.
            if failed.is_empty() {
                entry.no_local.insert(glyph_id);
            }
            Source::Lacks { failed }
        }
    }

    /// The PBF glyph `glyph_id` from `font_stack`'s font face of `type_`:
    /// `Ok(None)` when the loaded face lacks it, or when the face's file
    /// failed to load during the wait that began at failure `since`, which
    /// adds the face's range to `failed`; `Err` with the face's range while
    /// it isn't loaded.
    fn face_source(
        &self,
        font_stack: &FontStack,
        type_: GlyphIDType,
        glyph_id: GlyphID,
        since: u64,
        failed: &mut Vec<GlyphRange>,
    ) -> Result<Option<Glyph>, GlyphRange> {
        let range = get_glyph_range(GlyphID::with_type(0, type_));
        match self.entries.get(font_stack).and_then(|entry| entry.ranges.get(&range)) {
            Some(RangeState::Loaded(_)) => Ok(self.face_glyph(font_stack, type_, glyph_id)),
            Some(RangeState::Failed { at }) if *at > since => {
                failed.push(range);
                Ok(None)
            }
            _ => Err(range),
        }
    }

    /// The glyph that the loaded font face of `type_` maps the character of
    /// the PBF glyph `glyph_id` to, unshaped, as a PBF draws it.
    fn face_glyph(&self, font_stack: &FontStack, type_: GlyphIDType, glyph_id: GlyphID) -> Option<Glyph> {
        let shaper = self.shaper(font_stack, type_)?;
        let mut glyph = shaper.rasterize(shaper.glyph_of(glyph_id.char()?)?);
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
                        if *type_ == GlyphIDType::SYSTEM {
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
    /// the clusters that one face was chosen for. A cluster that no face
    /// draws, or whose face doesn't load, is left out.
    fn shape_with_system_fonts(&self, font_stack: &FontStack, run: &BiDiRun, shaped: &mut ShapedText) {
        let device = self.entries.get(font_stack).map(|entry| &entry.device);
        let mut pieces: Vec<(GlyphIDType, Vec<char>)> = Vec::new();
        for cluster in clusters(&run.text) {
            let Some(DeviceFont::Chosen(Some(type_))) = device.and_then(|device| device.get(cluster)).copied() else {
                continue;
            };
            match pieces.last_mut() {
                Some((last, piece)) if *last == type_ => piece.extend_from_slice(cluster),
                _ => pieces.push((type_, cluster.to_vec())),
            }
        }
        // A right-to-left run's later pieces lie to the left of its earlier
        // ones.
        if run.rtl {
            pieces.reverse();
        }
        for (type_, piece) in pieces {
            if let Some(shaper) = self.shaper(font_stack, type_) {
                shaper.shape(&piece, run.rtl, shaped);
            }
        }
    }

    /// `cluster`'s device font for `font_stack`: `Chosen(None)` without a
    /// chooser; `Asked` while it is being chosen, which asks the chooser
    /// about it at the end of the resolve the first time.
    fn device_font(&mut self, font_stack: &FontStack, cluster: &[char]) -> DeviceFont {
        if self.local.chooser.is_none() {
            return DeviceFont::Chosen(None);
        }
        let entry = self.entries.entry(font_stack.clone()).or_default();
        if let Some(&state) = entry.device.get(cluster) {
            return state;
        }
        entry.device.insert(cluster.to_vec(), DeviceFont::Asked);
        self.asks.get_or_insert_with(font_stack, Vec::new).push(cluster.to_vec());
        DeviceFont::Asked
    }

    /// Has the chooser choose the device fonts of the clusters asked about
    /// since the last call, one task per font stack on `choices`, each of
    /// which sends its answer to `inbox`.
    fn ask_device_fonts(&mut self, inbox: &InboxSender) {
        let Some(chooser) = &self.local.chooser else { return };
        for (font_stack, clusters) in std::mem::take(&mut self.asks) {
            let (chooser, inbox) = (Arc::clone(chooser), inbox.clone());
            self.choices.spawn(move || {
                let faces = clusters.into_iter().map(|cluster| {
                    let face = chooser.choose(&font_stack, &cluster);
                    (cluster, face)
                });
                let faces = faces.collect();
                inbox.send(TileEvent::DeviceFonts { font_stack, faces });
            });
        }
    }

    /// Loads `range` of `font_stack` from `data`, the font file its face
    /// holds, without a request.
    fn load_held(&mut self, font_stack: &FontStack, range: GlyphRange, data: FontData) {
        self.requests += 1;
        let id = self.requests;
        let retry = self.has_failed(font_stack, range);
        self.set_range(font_stack, range, RangeState::Loading { id, request: None, retry });
        self.events.push(RendererEvent::GlyphsRequested { font_stack: font_stack.clone(), range });
        self.process_data(Ok(Some(GlyphData::Font(data))), font_stack, range, id);
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
        let resource = if range.type_ == GlyphIDType::FontPBF {
            Resource::glyphs(&self.glyph_url, font_stack, (range.first, range.second))
        } else {
            let url = self.face(range.type_).map(|face| face.url.as_str()).unwrap_or_default();
            if url.is_empty() {
                // The request of an empty URL fails, as the glyphs do.
                log::error!(target: log_target::STYLE, "font face {} has no URL", range.type_.id());
            }
            Resource::font_face(url)
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
        let retry = self.has_failed(font_stack, range);
        file_source.load(&resource, responder);
        self.set_range(font_stack, range, RangeState::Loading { id, request: Some(req), retry });
    }

    /// Whether `font_stack`'s `range` failed to load, or is loading again
    /// after it did.
    fn has_failed(&self, font_stack: &FontStack, range: GlyphRange) -> bool {
        let state = self.entries.get(font_stack).and_then(|entry| entry.ranges.get(&range));
        matches!(state, Some(RangeState::Failed { .. } | RangeState::Loading { retry: true, .. }))
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
                format!("{} (font file)", range.type_.id())
            };
            log::error!(
                target: log_target::STYLE,
                "Failed to load glyph range {range_name} for font stack {}: {error}",
                crate::text::font_stack::font_stack_to_string(font_stack)
            );
            self.events.push(RendererEvent::GlyphsError {
                font_stack: font_stack.clone(),
                range,
                error: error.clone(),
            });
            self.events.push(RendererEvent::ResourceError(error));
        } else {
            if self.has_failed(font_stack, range) {
                self.recovered.insert((font_stack.clone(), range));
                if let Some(entry) = self.entries.get_mut(font_stack) {
                    for glyph_id in std::mem::take(&mut entry.stand_ins) {
                        entry.glyphs.remove(&glyph_id);
                    }
                }
            }
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
                log::error!(target: log_target::GLYPH, "font faces can't be loaded: {error}");
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

    /// The loaded shaper of `font_stack`'s face of `type_`.
    fn shaper(&self, font_stack: &FontStack, type_: GlyphIDType) -> Option<Rc<dyn FontFaceShaper>> {
        self.shapers.get(font_stack)?.get(&type_).cloned()
    }

    /// Loads the font face `data` for `font_stack`; false, as for an invalid
    /// font, when the engine failed to get ready.
    fn load_shaper(&mut self, font_stack: &FontStack, type_: GlyphIDType, data: FontData) -> bool {
        if !matches!(self.engine_readiness.lock().result, Some(Ok(()))) {
            return false;
        }
        let weight = self.face(type_).map_or(REGULAR_WEIGHT as u16, |face| face.weight);
        let Some(shaper) = self.engine.load(type_, data, weight) else {
            return false;
        };
        self.shapers.entry(font_stack.clone()).or_default().insert(type_, Rc::from(shaper));
        true
    }

    /// The style's or app's face of `type_`, else the device's.
    fn face(&self, type_: GlyphIDType) -> Option<&FontFace> {
        if type_ == GlyphIDType::FontPBF {
            return None;
        }
        let faces = self.font_faces.iter().flat_map(|faces| faces.iter());
        faces.chain(&self.device_faces).find(|face| face.type_ == type_)
    }
}

/// `text`'s clusters, which one font draws: each character with the marks
/// and joiners after it (`i18n::extends_previous`).
fn clusters(text: &[char]) -> impl Iterator<Item = &[char]> {
    text.chunk_by(|_, &next| i18n::extends_previous(next))
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
    /// The glyph, now in the store: `failed` holds the ranges of the
    /// sources before it that failed during the wait.
    Has { failed: Vec<GlyphRange> },
    /// Not the glyph: `failed` holds the ranges that would have had it
    /// and failed during the wait.
    Lacks { failed: Vec<GlyphRange> },
    /// The range it waits for.
    Needs(GlyphRange),
    /// Its device font is being chosen.
    Waits,
}
