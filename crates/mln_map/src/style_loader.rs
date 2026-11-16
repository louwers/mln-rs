//! The map's style loader: it fetches a style document and the sprites the
//! document names, and parses the sprites on the thread pool.
//!
//! Requests and jobs answer into one inbox of [`Reply`]s, on the map's loop.
//! The map drains it in `process_events` and hands each reply to
//! [`StyleLoader::on_reply`], which returns what the map does as
//! [`LoadEvent`]s. A reply to a load that a later one replaced carries the
//! older load's number and is dropped.

use std::sync::Arc;

use mln_actor::{RunLoop, TaggedScheduler};
use mln_sprite::load_sprite;
use mln_storage::{Data, FileSource, FileSourceExt, Request, Resource, Response};
use mln_style::sprite::Sprite;
use mln_style::{Parsed, Style, StyleLoadError, StyleRegistry, image};
use mln_text::glyph::FontFaces;
use mln_util::error::Error;
use mln_util::event_queue::{EventQueue, Waker};
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};

/// What a request or a job of the loader answers.
pub(crate) enum Reply {
    /// A response to the style document's request.
    Style { load: u64, response: Response },
    /// A response to a sprite's index or image request.
    Sprite { id: String, load: u64, part: SpritePart, response: Response },
    /// A sprite parsed, or why it didn't.
    SpriteParsed { id: String, load: u64, result: Result<Vec<Arc<image::Impl>>, Error> },
}

/// Which of a sprite's two resources a response is for.
#[derive(Clone, Copy)]
pub(crate) enum SpritePart {
    Json,
    Image,
}

/// What the map does about a load, in the order the loader returns them.
#[expect(clippy::large_enum_variant, reason = "the events are handled as they are returned, not stored")]
pub(crate) enum LoadEvent {
    /// A load began.
    Loading,
    /// A style document parsed, which replaces the style and its sources.
    Parsed(Parsed),
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
}

/// Loads a map's style document and its sprites (see the module docs).
///
/// "Loaded" has two meanings here. The document is loaded from the time a
/// document parses until the next URL load begins: the map refuses edits of
/// the loaded document meanwhile, since the parse that ends the load
/// replaces the whole style. The sprites are loaded once each sprite of the
/// parsed document loaded or failed for good; with the sources, they make
/// the style loaded for a still image.
pub(crate) struct StyleLoader {
    file_source: Option<Arc<dyn FileSource>>,
    run_loop: RunLoop,
    thread_pool: TaggedScheduler,
    replies: EventQueue<Reply>,
    /// The URL of the style document, or `None` for a style loaded from
    /// JSON.
    url: Option<String>,
    request: Option<Request>,
    /// Counts the loads, style documents and sprites alike.
    loads: u64,
    /// The load that the document request answers.
    style_load: u64,
    /// Whether a response to the document request brought a style, which a
    /// later response without content or not modified confirms.
    delivered: bool,
    /// The parsed document's sprites.
    sprite_list: Vec<Sprite>,
    sprites: Vec<SpriteLoad>,
    pixel_ratio: f32,
    fallback_font_faces: Option<Arc<FontFaces>>,
    /// The last error of a sprite or a source, which fails still images.
    last_error: Option<Error>,
    /// A style document parsed, and no URL load began since.
    document_loaded: bool,
    /// An edit changed the style since its document parsed, so a later
    /// answer to the document's request doesn't replace it.
    edited: bool,
}

impl StyleLoader {
    /// A loader that requests from `file_source`, answers on `run_loop` and
    /// parses sprites on its pool, for `pixel_ratio`.
    pub(crate) fn new(file_source: Option<Arc<dyn FileSource>>, run_loop: &RunLoop, pixel_ratio: f32) -> Self {
        StyleLoader {
            file_source,
            run_loop: run_loop.clone(),
            thread_pool: TaggedScheduler::new(run_loop.pool(), SimpleIdentity::new()),
            replies: EventQueue::new(),
            url: None,
            request: None,
            loads: 0,
            style_load: 0,
            delivered: false,
            sprite_list: Vec::new(),
            sprites: Vec::new(),
            pixel_ratio,
            fallback_font_faces: None,
            last_error: None,
            document_loaded: false,
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

    /// Sets the waker that runs when a reply arrives.
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
        let load = self.next_load();
        self.style_load = load;
        let replies = self.replies.clone();
        self.request = Some(file_source.request(
            &self.run_loop.scheduler(),
            &Resource::style(url),
            Box::new(move |response: Response| replies.push(Reply::Style { load, response })),
        ));
        vec![LoadEvent::Loading]
    }

    /// Handles a reply from the inbox, parsing a style document against
    /// `registry`.
    pub(crate) fn on_reply(&mut self, registry: &Arc<StyleRegistry>, reply: Reply) -> Vec<LoadEvent> {
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
        }
        events
    }

    fn on_document_response(
        &mut self,
        registry: &Arc<StyleRegistry>,
        load: u64,
        res: &Response,
        events: &mut Vec<LoadEvent>,
    ) {
        if load != self.style_load {
            return;
        }
        // Don't allow a loaded, edited style to be overwritten with a new version.
        if self.edited && self.document_loaded {
            return;
        }

        // A response without content or not modified confirms the style an
        // earlier response brought. Without one, the load has no style, so
        // it fails.
        let empty = res.no_content || res.not_modified;
        let error = match &res.error {
            Some(error) => Some(error.message.clone()),
            None if empty && !self.delivered => Some(
                if res.no_content {
                    "the style request returned no content"
                } else {
                    "the style request returned not modified, but no earlier response delivered the style"
                }
                .to_owned(),
            ),
            None => None,
        };
        if let Some(error) = error {
            let message = format!("loading style failed: {error}");
            Log::error(Event::Setup, &message);
            events.push(LoadEvent::Failed(StyleLoadError::Load(message)));
            events.push(LoadEvent::ResourceError(Error::native(error)));
        } else if !empty && let Some(data) = &res.data {
            self.delivered = true;
            let json = String::from_utf8_lossy(data);
            self.parse(registry, &json, events);
        }
    }

    /// Parses `json` against `registry` and requests its sprites.
    fn parse(&mut self, registry: &Arc<StyleRegistry>, json: &str, events: &mut Vec<LoadEvent>) {
        let parsed = match Style::from_json(registry, json, self.fallback_font_faces.as_ref()) {
            Ok(parsed) => parsed,
            Err(error) => {
                let message = format!("Failed to parse style: {error}");
                Log::error(Event::ParseStyle, &message);
                events.push(LoadEvent::Failed(StyleLoadError::Parse(message)));
                events.push(LoadEvent::ResourceError(error));
                return;
            }
        };
        self.document_loaded = true;
        self.edited = false;
        self.sprites.clear();
        self.sprite_list = parsed.style.get_sprites().to_vec();
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
        events.push(LoadEvent::Parsed(parsed));
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
            file_source.request(
                &self.run_loop.scheduler(),
                &resource,
                Box::new(move |response: Response| {
                    replies.push(Reply::Sprite { id: id.clone(), load, part, response });
                }),
            )
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
        if let Some(error) = &res.error {
            let sprite = entry.sprite.clone();
            let error = Error::native(error.message.clone());
            self.sprite_failed(sprite, error, events);
            return;
        } else if res.not_modified {
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
        self.thread_pool.schedule_and_reply_later(
            &self.run_loop.scheduler(),
            move |respond| load_sprite(job_id, image, json, respond),
            move |result| replies.push(Reply::SpriteParsed { id, load, result }),
        );
    }

    /// Records that `sprite` failed with `error`, which counts as loaded
    /// for a sprite the document names.
    fn sprite_failed(&mut self, sprite: Option<Sprite>, error: Error, events: &mut Vec<LoadEvent>) {
        self.last_error = Some(error.clone());
        Log::error(Event::Style, format!("Failed to load sprite: {error}"));
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

    /// Whether a style document parsed, and no URL load began since.
    pub(crate) fn document_loaded(&self) -> bool {
        self.document_loaded
    }

    /// Records that an edit changed the style since its document parsed.
    pub(crate) fn edited(&mut self) {
        self.edited = true;
    }

    /// Records a source's load error, which fails still images until the
    /// next load.
    pub(crate) fn set_error(&mut self, error: Error) {
        self.last_error = Some(error);
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

/// The id a style without a sprite loads under.
const DEFAULT_SPRITE: &str = "default";

impl SpriteLoad {
    fn new(sprite: Option<Sprite>, load: u64) -> Self {
        SpriteLoad { sprite, load, json: None, image: None, requests: Vec::new(), loaded: false }
    }

    fn id(&self) -> &str {
        self.sprite.as_ref().map_or(DEFAULT_SPRITE, |sprite| sprite.id.as_str())
    }
}
