//! The map's style loader: it fetches a style document, the sprites the
//! document names and the descriptions of the style's sources, and parses
//! the sprites and the descriptions that need it, and decodes image sources'
//! images, on the thread pool.
//!
//! Requests and jobs answer into one inbox of [`Reply`]s, from whichever
//! thread sends the response or finishes the job. The map drains it in
//! `process_events` and hands each reply to [`StyleLoader::on_reply`], which
//! returns what the map does as [`LoadEvent`]s. Each reply carries the number
//! of its load or request, so one that arrives after a later load or request
//! replaced its own is dropped.
//!
//! The loader watches each version of the style ([`StyleLoader::watch_sources`])
//! and loads the description of each source whose instance or
//! [`load`](mln_style::source::SourceData::load) it hasn't seen, as the
//! source's type says. What a load finds goes back into the style as an
//! [`Edit::SetSourceDescription`].

use std::sync::Arc;

use mln_style::error::Error;
use mln_style::hash::HashMap;
use mln_style::source::{DescribeImageJob, Described, ErasedDescription, Fetch, Load, Respond};
use mln_style::sprite::Sprite;
use mln_style::{Edit, SourceInstance, Style, StyleLoadError, StyleRegistry, image};
use mln_util::event_queue::{EventQueue, Waker};
use mln_util::pool::{Pool, TaskGroup};

use crate::image::decoder;
use crate::image::sprite::load_sprite;
use crate::log_target;
use crate::storage::{Data, FileSource, FileSourceExt, Request, Resource, ResourceError, Response};
use crate::text::glyph::{FontFaces, style_font_faces};

/// What a request or a job of the loader answers.
pub(crate) enum Reply {
    /// A response to the style document's request.
    Style { load: u64, response: Response },
    /// A response to a sprite's index or image request.
    Sprite { id: String, load: u64, part: SpritePart, response: Response },
    /// A sprite parsed, or why it didn't.
    SpriteParsed { id: String, load: u64, result: Result<Vec<Arc<image::Impl>>, Error> },
    /// A response to a source's description request, the `fetch`th
    /// request of the loader.
    Source { load: SourceLoadId, fetch: u64, response: Response },
    /// What a source's fetched description data describes, for the
    /// `describing`th response to the load.
    SourceDescribed { load: SourceLoadId, describing: u64, result: Described<ErasedDescription> },
}

/// A load of a source's description: the source's id and instance, and the
/// [`load`](mln_style::source::SourceData::load) of its data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceLoadId {
    pub(crate) id: Arc<str>,
    pub(crate) instance: SourceInstance,
    pub(crate) load: u64,
}

/// Which of a sprite's two resources a response is for.
#[derive(Clone, Copy)]
pub(crate) enum SpritePart {
    Json,
    Image,
}

/// What the map does about a load, in the order the loader returns them.
// A style is past the lint's threshold only where pointers are 64 bits.
#[cfg_attr(
    target_pointer_width = "64",
    expect(clippy::large_enum_variant, reason = "the events are handled as they are returned, not stored")
)]
pub(crate) enum LoadEvent {
    /// A load began.
    Loading,
    /// A style document parsed, which replaces the style, with the faces
    /// the map draws its text with.
    Parsed(Style, Option<Arc<FontFaces>>),
    /// The style and its sprite requests are in place.
    Loaded,
    /// The style document failed to load or parse.
    Failed(StyleLoadError),
    /// A resource failed to load, which fails a pending still image.
    ResourceError(Error),
    SpriteRequested(Option<Sprite>),
    /// A sprite's images, for the style.
    SpriteImages(Option<Sprite>, Vec<Arc<image::Impl>>),
    SpriteFailed(Option<Sprite>, Error),
    /// What a load of a source's description found, for the style as an
    /// [`Edit::SetSourceDescription`]. An `Err` is already the loader's last
    /// error.
    SourceDescribed {
        load: SourceLoadId,
        result: Described<ErasedDescription>,
    },
}

/// Where the load of a style source's description stands.
struct SourceLoad {
    load: SourceLoadId,
    /// Held to keep the request running, as a file source that revalidates
    /// answers again; dropping it cancels it.
    request: Option<Request>,
    /// The number of the request, which its responses carry.
    fetch: u64,
    /// Counts the responses whose data went to be described, so that a
    /// description of older data doesn't replace a newer one's.
    describing: u64,
    /// Whether the description loaded, or the source has nothing to load.
    loaded: bool,
    /// The last response was an error (`error_streak`).
    failing: bool,
    /// Whether the fetch is an image, which the loader decodes before the
    /// source's type describes it.
    image: bool,
}

/// A sprite of the parsed style, and where its load stands.
struct SpriteLoad {
    /// `None` for a style without a sprite.
    sprite: Option<Sprite>,
    /// Which load this is, so a reply to an earlier load doesn't answer it.
    load: u64,
    json: Option<Data>,
    image: Option<Data>,
    /// Held to keep the requests running; dropping them cancels them.
    requests: Vec<Request>,
    /// Whether the sprite loaded or failed for good. A style without a
    /// sprite that has no file source never loads.
    loaded: bool,
    /// The last response to the JSON request, and to the image request, was
    /// an error (`error_streak`).
    failing: [bool; 2],
}

/// Loads a map's style document, its sprites and its sources' descriptions
/// (see the module docs).
///
/// "Loaded" has two meanings here. The document is loaded from the time a
/// document parses until the next URL load begins: the map refuses edits of
/// the loaded document meanwhile, since the parse that ends the load
/// replaces the whole style. The sprites are loaded once each sprite of the
/// parsed document loaded or failed for good, and a source once its
/// description loaded or it has none to load; together they make the style
/// loaded for a still image. A source whose load failed isn't loaded.
pub(crate) struct StyleLoader {
    file_source: Option<Arc<dyn FileSource>>,
    jobs: TaskGroup,
    replies: EventQueue<Reply>,
    /// The URL of the style document, or `None` for a style loaded from
    /// JSON.
    url: Option<String>,
    request: Option<Request>,
    /// Counts the loads, style documents and sprites alike, and the source
    /// description requests.
    loads: u64,
    /// The load that the document request answers.
    style_load: u64,
    /// Whether a response to the document request brought a style, which a
    /// later response without content or not modified confirms.
    delivered: bool,
    /// The document request's last response was an error (`error_streak`).
    failing: bool,
    /// The parsed document's sprites.
    sprite_list: Vec<Sprite>,
    sprites: Vec<SpriteLoad>,
    pixel_ratio: f32,
    fallback_font_faces: Option<Arc<FontFaces>>,
    /// The last error of a sprite or a source, which fails still images.
    last_error: Option<Error>,
    /// A style document parsed, and no URL load began since.
    document_loaded: bool,
    /// The loads of the style's sources' descriptions, by source id.
    sources: HashMap<Arc<str>, SourceLoad>,
    /// An edit changed the style since its document parsed, so a later
    /// answer to the document's request doesn't replace it.
    edited: bool,
}

impl StyleLoader {
    /// A loader that requests from `file_source` and parses sprites and
    /// describes sources in a group of its own on `pool`, for
    /// `pixel_ratio`.
    pub(crate) fn new(file_source: Option<Arc<dyn FileSource>>, pool: &Pool, pixel_ratio: f32) -> Self {
        StyleLoader {
            file_source,
            jobs: TaskGroup::new(pool),
            replies: EventQueue::new(),
            url: None,
            request: None,
            loads: 0,
            style_load: 0,
            delivered: false,
            failing: false,
            sprite_list: Vec::new(),
            sprites: Vec::new(),
            pixel_ratio,
            fallback_font_faces: None,
            last_error: None,
            document_loaded: false,
            sources: HashMap::default(),
            edited: false,
        }
    }

    fn next_load(&mut self) -> u64 {
        self.loads += 1;
        self.loads
    }

    /// Takes the replies that arrived since the last call, oldest first.
    pub(crate) fn take_replies(&self) -> Vec<Reply> {
        self.replies.take()
    }

    pub(crate) fn has_replies(&self) -> bool {
        !self.replies.is_empty()
    }

    /// Sets the waker that runs, on any thread, when a reply arrives.
    pub(crate) fn set_waker(&self, waker: Option<Waker>) {
        self.replies.set_waker(waker);
    }

    /// Loads the style document `json`, parsed against `registry`, and
    /// cancels a URL load.
    pub(crate) fn load_json(&mut self, registry: &Arc<StyleRegistry>, json: &str) -> Vec<LoadEvent> {
        self.last_error = None;
        let mut events = vec![LoadEvent::Loading];
        self.request = None;
        self.style_load = self.next_load();
        self.url = None;
        self.parse(registry, json, &mut events);
        events
    }

    /// Fetches the style document at `url`, which replaces the style when
    /// it arrives. The style stays meanwhile, but its document isn't loaded.
    pub(crate) fn load_url(&mut self, url: &str) -> Vec<LoadEvent> {
        let Some(file_source) = self.file_source.clone() else {
            return vec![LoadEvent::Failed(StyleLoadError::Load(
                "Unable to find resource provider for style url.".to_owned(),
            ))];
        };

        self.last_error = None;
        self.document_loaded = false;
        self.url = Some(url.to_owned());

        self.delivered = false;
        self.failing = false;
        let load = self.next_load();
        self.style_load = load;
        let replies = self.replies.clone();
        self.request = Some(
            file_source.request(&Resource::style(url), move |response| replies.push(Reply::Style { load, response })),
        );
        vec![LoadEvent::Loading]
    }

    /// Handles a reply from the inbox for the map whose style is `style`,
    /// parsing a style document against its registry.
    pub(crate) fn on_reply(&mut self, style: &Style, reply: Reply) -> Vec<LoadEvent> {
        let registry = style.registry();
        let mut events = Vec::new();
        match reply {
            Reply::Style { load, response } => self.on_document_response(registry, load, &response, &mut events),
            Reply::Sprite { id, load, part, response } => {
                self.on_sprite_response(&id, load, part, response, &mut events);
            }
            Reply::SpriteParsed { id, load, result } => {
                let Some(entry) = self.sprite_load(&id, load) else { return events };
                let sprite = entry.sprite.clone();
                match result {
                    Ok(images) => {
                        entry.loaded = true;
                        events.push(LoadEvent::SpriteImages(sprite, images));
                    }
                    Err(error) => self.sprite_failed(sprite, error, &mut events),
                }
            }
            Reply::Source { load, fetch, response } => {
                self.on_source_response(style, load, fetch, response, &mut events);
            }
            Reply::SourceDescribed { load, describing, result } => {
                let Some(entry) = self.sources.get_mut(&load.id).filter(|entry| entry.load == load) else {
                    return events;
                };
                if entry.describing != describing {
                    return events;
                }
                match &result {
                    Ok(_) => entry.loaded = true,
                    Err(error) => self.last_error = Some(error.clone()),
                }
                events.push(LoadEvent::SourceDescribed { load, result });
            }
        }
        events
    }

    /// Loads the description of each source of `style` whose load the
    /// loader hasn't seen, and forgets the loads of the sources `style`
    /// lacks. Returns the edits that give sources the descriptions their
    /// types know without a fetch, for the map to make before it publishes
    /// `style`.
    pub(crate) fn watch_sources(&mut self, style: &Style) -> Vec<Edit> {
        let sources = style.get_sources();
        self.sources.retain(|id, entry| sources.get(id).is_some_and(|source| source.instance() == entry.load.instance));
        let mut edits = Vec::new();
        for source in sources.iter() {
            let load =
                SourceLoadId { id: Arc::from(source.id()), instance: source.instance(), load: source.data().load() };
            if self.sources.get(&load.id).is_some_and(|entry| entry.load == load) {
                continue;
            }
            let mut entry = SourceLoad {
                load: load.clone(),
                request: None,
                fetch: 0,
                describing: 0,
                loaded: true,
                failing: false,
                image: false,
            };
            match source.data().plan_load() {
                Load::Done => {}
                Load::Ready(description) => edits.push(Edit::SetSourceDescription {
                    id: source.id().to_owned(),
                    instance: load.instance,
                    load: load.load,
                    result: Ok(Some(description)),
                }),
                Load::Fetch(target) => {
                    entry.loaded = false;
                    entry.image = matches!(target, Fetch::Image(_));
                    let resource = match &target {
                        Fetch::Source(url) => Resource::source(url),
                        Fetch::Image(url) => Resource::image(url),
                    };
                    if let Some(file_source) = self.file_source.clone() {
                        let replies = self.replies.clone();
                        let reply_load = load.clone();
                        let fetch = self.next_load();
                        entry.fetch = fetch;
                        entry.request = Some(file_source.request(&resource, move |response| {
                            replies.push(Reply::Source { load: reply_load.clone(), fetch, response });
                        }));
                    }
                }
            }
            self.sources.insert(Arc::clone(&load.id), entry);
        }
        edits
    }

    /// Describes the data of a response to the description request of
    /// `style`'s source, on the thread pool if the source's type asks for
    /// it. A failed response fails the load at once.
    fn on_source_response(
        &mut self,
        style: &Style,
        load: SourceLoadId,
        fetch: u64,
        res: Response,
        events: &mut Vec<LoadEvent>,
    ) {
        let Some(entry) = self.sources.get_mut(&load.id).filter(|entry| entry.load == load && entry.fetch == fetch)
        else {
            return;
        };
        let Some(source) = style.get_source(&load.id).filter(|source| source.instance() == load.instance) else {
            return;
        };
        if let Err(error) = error_streak(&mut entry.failing, &res) {
            if let Some(error) = error {
                let error = Error::native(error.message.clone());
                self.last_error = Some(error.clone());
                events.push(LoadEvent::SourceDescribed { load, result: Err(error) });
            }
            return;
        }
        if res.not_modified {
            return;
        }
        let data = if res.no_content { None } else { Some(res.data.unwrap_or_default()) };
        entry.describing += 1;
        let describing = entry.describing;
        let image = entry.image;
        let replies = self.replies.clone();
        let respond: Respond<_> =
            Box::new(move |result| replies.push(Reply::SourceDescribed { load, describing, result }));
        if image {
            let describe = source.data().describe_image_job();
            self.jobs.spawn(move || describe_image(data, describe, respond));
            return;
        }
        let job = source.data().describe_job(data);
        if source.data().describes_in_background() {
            self.jobs.spawn(move || job(respond));
        } else {
            job(respond);
        }
    }

    fn on_document_response(
        &mut self,
        registry: &Arc<StyleRegistry>,
        load: u64,
        res: &Response,
        events: &mut Vec<LoadEvent>,
    ) {
        // A later answer doesn't replace a loaded style that an edit changed.
        if load != self.style_load || (self.edited && self.document_loaded) {
            return;
        }
        // An error fails the load only if no earlier response brought a
        // style.
        if let Err(error) = error_streak(&mut self.failing, res) {
            if let Some(error) = error {
                let message = format!("loading style failed: {}", error.message);
                log::error!(target: log_target::SETUP, "{message}");
                if !self.delivered {
                    events.push(LoadEvent::Failed(StyleLoadError::Load(message)));
                }
                events.push(LoadEvent::ResourceError(Error::native(error.message.clone())));
            }
            return;
        }
        // A response without content or not modified confirms the style an
        // earlier response brought. Without one, the load has no style, so
        // it fails.
        let empty = res.no_content || res.not_modified;
        if empty && !self.delivered {
            let error = if res.no_content {
                "the style request returned no content"
            } else {
                "the style request returned not modified, but no earlier response delivered the style"
            };
            let message = format!("loading style failed: {error}");
            log::error!(target: log_target::SETUP, "{message}");
            events.push(LoadEvent::Failed(StyleLoadError::Load(message)));
            events.push(LoadEvent::ResourceError(Error::native(error)));
        }
        if !empty && let Some(data) = &res.data {
            self.delivered = true;
            let json = String::from_utf8_lossy(data);
            self.parse(registry, &json, events);
        }
    }

    /// Parses `json` against `registry` and requests its sprites.
    fn parse(&mut self, registry: &Arc<StyleRegistry>, json: &str, events: &mut Vec<LoadEvent>) {
        let parsed = match Style::from_json(registry, json) {
            Ok(parsed) => parsed,
            Err(error) => {
                let message = format!("Failed to parse style: {error}");
                log::error!(target: log_target::PARSE_STYLE, "{message}");
                events.push(LoadEvent::Failed(StyleLoadError::Parse(message)));
                events.push(LoadEvent::ResourceError(error));
                return;
            }
        };
        self.document_loaded = true;
        self.edited = false;
        self.sprites.clear();
        self.sprite_list = parsed.get_sprites().to_vec();
        let mut sprite_events = Vec::new();
        if self.file_source.is_none() {
            self.sprites.push(SpriteLoad::new(None, 0));
            self.sprite_failed(
                None,
                Error::native("Unable to find resource provider for sprite url."),
                &mut sprite_events,
            );
        } else if self.sprite_list.is_empty() {
            // A style without a sprite loads as one with an empty sprite.
            let mut entry = SpriteLoad::new(None, 0);
            entry.loaded = true;
            self.sprites.push(entry);
            sprite_events.push(LoadEvent::SpriteImages(None, Vec::new()));
        } else {
            for sprite in self.sprite_list.clone() {
                self.request_sprite(sprite, &mut sprite_events);
            }
        }
        // The map publishes the parsed style once it knows where its sprites
        // stand.
        let font_faces =
            style_font_faces(parsed.get_font_faces().map(|faces| &**faces), self.fallback_font_faces.as_deref());
        events.push(LoadEvent::Parsed(parsed, font_faces));
        events.extend(sprite_events);
        events.push(LoadEvent::Loaded);
    }

    /// Requests `sprite`'s index and image, replacing an earlier load of it.
    fn request_sprite(&mut self, sprite: Sprite, events: &mut Vec<LoadEvent>) {
        let Some(file_source) = self.file_source.clone() else { return };
        events.push(LoadEvent::SpriteRequested(Some(sprite.clone())));

        let load = self.next_load();
        let request = |part: SpritePart, resource: Resource| {
            let replies = self.replies.clone();
            let id = sprite.id.clone();
            file_source.request(&resource, move |response| {
                replies.push(Reply::Sprite { id: id.clone(), load, part, response });
            })
        };
        let requests = vec![
            request(SpritePart::Json, Resource::sprite_json(&sprite.sprite_url, self.pixel_ratio)),
            request(SpritePart::Image, Resource::sprite_image(&sprite.sprite_url, self.pixel_ratio)),
        ];
        let mut entry = SpriteLoad::new(Some(sprite), load);
        entry.requests = requests;
        match self.sprites.iter_mut().find(|other| other.id() == entry.id()) {
            // The earlier load's requests are cancelled as they drop.
            Some(other) => *other = entry,
            None => self.sprites.push(entry),
        }
    }

    fn sprite_load(&mut self, id: &str, load: u64) -> Option<&mut SpriteLoad> {
        self.sprites.iter_mut().find(|entry| entry.id() == id && entry.load == load)
    }

    fn on_sprite_response(
        &mut self,
        id: &str,
        load: u64,
        part: SpritePart,
        res: Response,
        events: &mut Vec<LoadEvent>,
    ) {
        let Some(entry) = self.sprite_load(id, load) else { return };
        if let Err(error) = error_streak(&mut entry.failing[part as usize], &res) {
            if let Some(error) = error {
                let sprite = entry.sprite.clone();
                self.sprite_failed(sprite, Error::native(error.message.clone()), events);
            }
            return;
        }
        if res.not_modified {
            return;
        }
        // Only parse again for new data.
        let value = if res.no_content { Some(Arc::new(Vec::new())) } else { res.data };
        match part {
            SpritePart::Json => entry.json = value,
            SpritePart::Image => entry.image = value,
        }
        let (Some(image), Some(json)) = (entry.image.clone(), entry.json.clone()) else { return };

        let id = id.to_owned();
        let replies = self.replies.clone();
        // The image decodes through the `decoder`, which may answer after
        // the job returns (the browser's).
        let job_id = id.clone();
        self.jobs.spawn(move || {
            load_sprite(
                job_id,
                image,
                json,
                Box::new(move |result| replies.push(Reply::SpriteParsed { id, load, result })),
            );
        });
    }

    /// Records that `sprite` failed with `error`, which counts as loaded
    /// for a sprite the document names.
    fn sprite_failed(&mut self, sprite: Option<Sprite>, error: Error, events: &mut Vec<LoadEvent>) {
        self.last_error = Some(error.clone());
        log::error!(target: log_target::STYLE, "Failed to load sprite: {error}");
        let id = sprite.as_ref().map_or(DEFAULT_SPRITE, |sprite| sprite.id.as_str());
        if let Some(entry) = self.sprites.iter_mut().find(|entry| entry.id() == id) {
            entry.loaded = sprite.is_some();
        }
        events.push(LoadEvent::ResourceError(error.clone()));
        events.push(LoadEvent::SpriteFailed(sprite, error));
    }

    /// Sets the pixel ratio that sprites load for. Where that picks the
    /// sprites' other resolution, the loader loads them again; the style
    /// keeps its images until the new ones arrive.
    pub(crate) fn set_pixel_ratio(&mut self, pixel_ratio: f32) -> Vec<LoadEvent> {
        let before = Resource::sprite_json("", self.pixel_ratio).url;
        self.pixel_ratio = pixel_ratio;
        let mut events = Vec::new();
        if Resource::sprite_json("", pixel_ratio).url == before {
            return events;
        }
        for sprite in self.sprite_list.clone() {
            self.request_sprite(sprite, &mut events);
        }
        events
    }

    /// Sets the font faces that every font stack falls back to after the
    /// style's own, for the documents that parse from now on.
    pub(crate) fn set_fallback_font_faces(&mut self, faces: Option<Arc<FontFaces>>) {
        self.fallback_font_faces = faces;
    }

    /// Whether every sprite of the parsed document loaded or failed for
    /// good. False before a document parsed.
    pub(crate) fn sprites_loaded(&self) -> bool {
        !self.sprites.is_empty() && self.sprites.iter().all(|entry| entry.loaded)
    }

    /// Whether each source of `style` loaded its description, or has none to
    /// load.
    pub(crate) fn sources_loaded(&self, style: &Style) -> bool {
        style.get_sources().iter().all(|source| self.source_loaded(source.id()))
    }

    /// Whether the source `id` loaded its description, or has none to load.
    pub(crate) fn source_loaded(&self, id: &str) -> bool {
        self.sources.get(id).is_some_and(|entry| entry.loaded)
    }

    /// Whether a style document parsed, and no URL load began since.
    pub(crate) fn document_loaded(&self) -> bool {
        self.document_loaded
    }

    /// Records that an edit changed the style since its document parsed.
    pub(crate) fn edited(&mut self) {
        self.edited = true;
    }

    /// The last error of a sprite or a source since the last load began.
    pub(crate) fn last_error(&self) -> Option<&Error> {
        self.last_error.as_ref()
    }

    /// The URL of the style document, or `None` for one loaded from JSON.
    pub(crate) fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// Queues `response` as a later answer to the style document's request,
    /// as a file source that revalidates the style answers again.
    #[cfg(test)]
    pub(crate) fn push_style_response(&self, response: Response) {
        self.replies.push(Reply::Style { load: self.style_load, response });
    }
}

/// Follows a request's streak of errors in `failing`, so that each streak
/// reports once: returns `res`'s error as `Err`, which is `Some` only for
/// the first of a streak.
fn error_streak<'r>(failing: &mut bool, res: &'r Response) -> Result<(), Option<&'r ResourceError>> {
    let Some(error) = res.error.as_deref() else {
        *failing = false;
        return Ok(());
    };
    Err((!std::mem::replace(failing, true)).then_some(error))
}

/// Decodes `data`, an image source's fetched image, and describes the
/// image with `describe`, answering through `respond`: at once or, where the
/// platform decodes images asynchronously (the browser), later.
fn describe_image(data: Option<Data>, describe: DescribeImageJob, respond: Respond<Described<ErasedDescription>>) {
    let Some(data) = data else {
        respond(Err(Error::native("unexpectedly empty image url")));
        return;
    };
    decoder::decode(&data, move |result| {
        respond(result.map_err(|error| Error::native(error.message)).and_then(describe));
    });
}

/// The id a style without a sprite loads under.
const DEFAULT_SPRITE: &str = "default";

impl SpriteLoad {
    fn new(sprite: Option<Sprite>, load: u64) -> Self {
        SpriteLoad { sprite, load, json: None, image: None, requests: Vec::new(), loaded: false, failing: [false; 2] }
    }

    fn id(&self) -> &str {
        self.sprite.as_ref().map_or(DEFAULT_SPRITE, |sprite| sprite.id.as_str())
    }
}
