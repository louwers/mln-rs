//! A handle to a map that lives on its runtime's thread.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use mln_engine::text::font_face_engine::FontFaceEngine;
use mln_engine::{Custom, CustomLayerHost, Registry, UpdateReader};
use mln_map::{
    BoundOptions, Camera, CameraCommandId, CameraDelta, CameraOptions, CameraUpdate, Elevation, FrameClock,
    FreeCameraOptions, LogicalExtent, MapEvent, MapMode, MapProjection, MapSnapshot, MapView, ProjectionMode,
    StyleLoadError, TransitionEnd, TransitionOutcome,
};
use mln_style::Edit;
use mln_style::error::{Error, ErrorKind};
use mln_style::geo::LatLngBounds;
use mln_style::hash::HashMap;
use mln_style::size::Size;
use mln_style::time::Instant;
use mln_text::GlyphCache;
use mln_text::local_fonts::LocalGlyphs;
use mln_util::event_queue::Waker;
use mln_util::pool::Pool;
use mln_util::queue::TimedTask;
use parking_lot::Mutex;

use crate::camera::{self, CameraFitArea, CameraFitOptions, CameraFitUpdate, TileOptions, ViewportOptions};
use crate::completion::{Completion, Resolver};
use crate::event::{EventMask, EventSource, RuntimeEvent, RuntimeEventKind};
use crate::feature_state::{FeatureState, FeatureStateSelector};
use crate::render::{
    Abandoner, AttachOptions, RenderSession, RenderTarget, SessionId, SessionQueue, SessionShared, Worker, worker_post,
};
use crate::runtime::{CoreQueue, Outbox, Runtime, RuntimeCore, RuntimeShared, post, post_at};
use crate::style::{CallbackSources, GeoJsonUpdates};

/// The ID of a map, unique within its runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MapId(pub u64);

/// How to create a map.
#[derive(Clone, Debug)]
pub struct MapOptions {
    pub map: mln_map::MapOptions,
    /// The events that the map reports.
    pub event_mask: EventMask,
    /// JSON text of the font faces that the map falls back to for the text
    /// that the style's fonts don't draw, as `Map::set_fallback_font_faces`
    /// takes it: the text that glyph PBFs can't shape, all the text of a
    /// style without them, and the glyphs that come from local fonts, before
    /// the system's. With it, the map shapes no text with the system's fonts.
    pub fallback_font_faces: Option<String>,
    /// Whether the map draws from the system's fonts what the style's fonts
    /// and `fallback_font_faces` don't. It shapes with them the scripts that
    /// glyph PBFs can't shape, and all the text of a style without PBFs
    /// (`mln_system_fonts::font_face`), from the fonts the labels'
    /// `text-font` names first; and draws from them the glyphs a style's
    /// PBFs lack, and with `local_ideographs`, CJK
    /// (`mln_system_fonts::SystemFonts`). On by default. The first map that
    /// uses them waits for the platform's font matcher to load. In the
    /// browser, which has no font files to read, the browser draws each
    /// glyph of that text in the font stack's families, then its sans-serif
    /// font, as MapLibre GL JS does, unshaped.
    pub system_fonts: bool,
    /// Whether CJK ideographs, kana and hangul come from local fonts (the
    /// fallback font faces, then the system's) before glyph PBFs, as
    /// MapLibre GL JS's `localIdeographFontFamily` draws them, rather than
    /// downloading a glyph PBF for every few characters. On by default.
    pub local_ideographs: bool,
    /// Whether the map's render sessions keep tiles in memory after they
    /// leave the view, so they show again without loading. On by default.
    pub tile_cache_enabled: bool,
}

impl Default for MapOptions {
    fn default() -> Self {
        MapOptions {
            map: mln_map::MapOptions::default(),
            event_mask: EventMask::default(),
            fallback_font_faces: None,
            system_fonts: true,
            local_ideographs: true,
            tile_cache_enabled: true,
        }
    }
}

/// What a map command left behind once it ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Applied {
    /// The command's place among the map's commands: the map's creation is
    /// 1, and each command takes the next number when it is sent.
    pub sequence: u64,
    /// The map's generation after the command: a snapshot with this
    /// generation or a later one includes the command.
    pub generation: u64,
    /// The map's update generation after the command: a frame that renders
    /// this update or a later one shows the command.
    pub render_update_generation: u64,
}

/// Moves `map`'s camera to the one that fits `area`.
fn fit(map: &mut mln_map::Map, area: &CameraFitArea, update: CameraFitUpdate) -> Result<CameraCommandId, Error> {
    let CameraFitUpdate { mode, fit, animation } = update;
    let camera = camera::camera_for(map, area, &fit)?;
    Ok(map.update_camera(&CameraUpdate { mode, camera, animation, ..CameraUpdate::default() }))
}

/// The camera, and the generation of the map that it belongs to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraQuery {
    pub generation: u64,
    pub camera: Camera,
}

/// What the runtime's thread publishes for a map's handles.
pub(crate) struct Published {
    /// The map's snapshot and its ground as of its last flush, `None` until
    /// the map is built: one lock, so that a projection's view and ground
    /// come from the same flush.
    state: Mutex<Option<PublishedState>>,
    /// The map's updates for its renderer, once the map is built.
    updates: OnceLock<UpdateReader>,
    /// The sequence number of the last command that ran and whose changes a
    /// frame can show.
    applied: AtomicU64,
    /// The map's frame clock, once the map is built: each frame demand sets
    /// it.
    frame_clock: OnceLock<FrameClock>,
    /// The frame samples that demands asked for, numbered from 1, and the
    /// last that the map took and a frame can show.
    samples_requested: AtomicU64,
    sampled: AtomicU64,
    /// Whether a sample is on its way to the runtime's thread, which takes
    /// the demands that ask meanwhile.
    sample_queued: AtomicBool,
    event_mask: AtomicU64,
    /// The still image that waits for a frame, as the
    /// `still_image_request_id` of the updates that ask for it, or 0.
    still_image: AtomicU64,
}

/// The map's state as of a flush.
struct PublishedState {
    snapshot: MapSnapshot,
    /// The ground the map draws (`mln_map::Map::ground`).
    ground: Option<Arc<dyn Elevation>>,
}

impl Published {
    /// The sequence number of the last command that ran and whose changes a
    /// frame can show.
    pub(crate) fn applied(&self) -> u64 {
        self.applied.load(Ordering::SeqCst)
    }

    /// The last frame sample that the map took and a frame can show.
    pub(crate) fn sampled(&self) -> u64 {
        self.sampled.load(Ordering::SeqCst)
    }

    /// Whether a camera command was still animating as of the map's last
    /// published state.
    pub(crate) fn animating(&self) -> bool {
        self.state.lock().as_ref().is_some_and(|state| state.snapshot.transition_in_progress)
    }

    /// The still image that waits for a frame, as the
    /// `still_image_request_id` of the updates that ask for it, or 0 when
    /// none waits.
    pub(crate) fn pending_still_image(&self) -> u64 {
        self.still_image.load(Ordering::SeqCst)
    }

    pub(crate) fn new(event_mask: EventMask) -> Self {
        Published {
            state: Mutex::new(None),
            updates: OnceLock::new(),
            applied: AtomicU64::new(0),
            frame_clock: OnceLock::new(),
            samples_requested: AtomicU64::new(0),
            sampled: AtomicU64::new(0),
            sample_queued: AtomicBool::new(false),
            event_mask: AtomicU64::new(event_mask.bits()),
            still_image: AtomicU64::new(0),
        }
    }
}

pub(crate) struct MapShared {
    id: MapId,
    runtime: Arc<RuntimeShared>,
    mode: MapMode,
    /// The map's scale factor, an `f64`'s bits: the options' pixel ratio,
    /// until a session's resize sends another.
    scale_factor: AtomicU64,
    tile_cache_enabled: bool,
    system_fonts: bool,
    local_ideographs: bool,
    /// The render session attached to the map, from its attach until its
    /// detach completes.
    session: Mutex<Option<SessionClaim>>,
    /// The sequence number of the last command sent. It stays locked while
    /// a command is numbered and posted, so the runtime's thread runs the
    /// map's commands in the order of their numbers.
    sent: Mutex<u64>,
    /// Whether the handles refuse commands.
    released: AtomicBool,
    /// Whether the map's removal was posted to the runtime's thread.
    removed: AtomicBool,
    published: Arc<Published>,
}

impl MapShared {
    pub(crate) fn new(id: MapId, runtime: Arc<RuntimeShared>, options: &MapOptions, published: Arc<Published>) -> Self {
        MapShared {
            id,
            runtime,
            mode: options.map.map_mode,
            scale_factor: AtomicU64::new(f64::from(options.map.pixel_ratio).to_bits()),
            tile_cache_enabled: options.tile_cache_enabled,
            system_fonts: options.system_fonts,
            local_ideographs: options.local_ideographs,
            session: Mutex::new(None),
            sent: Mutex::new(1),
            released: AtomicBool::new(false),
            removed: AtomicBool::new(false),
            published,
        }
    }

    /// A completion that the runtime's thread resolves, or with
    /// `discard`, one whose result is discarded, as
    /// [`Completion::discarded`] makes it.
    fn completion<R: Send + 'static>(&self, discard: Option<fn(Error)>) -> (Completion<R>, Resolver<R>) {
        match discard {
            Some(on_failure) => Completion::discarded(on_failure),
            None => self.runtime.completion(),
        }
    }

    /// Numbers a command and posts it to the runtime's thread.
    pub(crate) fn send<R: Send + 'static>(
        &self,
        discard: Option<fn(Error)>,
        command: impl FnOnce(&mut RuntimeCore, &mut Outbox, MapId, u64, Resolver<R>) + Send + 'static,
    ) -> Result<Completion<R>, Error> {
        let mut sent = self.sent.lock();
        if self.released.load(Ordering::SeqCst) {
            return Err(released());
        }
        self.runtime.check_running()?;
        let (completion, resolver) = self.completion(discard);
        let id = self.id;
        *sent += 1;
        let sequence = *sent;
        self.runtime.post(move |core, outbox| command(core, outbox, id, sequence, resolver));
        Ok(completion)
    }

    /// Releases the map: no command is sent after its removal. Fails when
    /// the map was released already.
    fn release(&self, done: Option<Resolver<()>>) -> Result<(), Error> {
        self.close()?;
        self.remove(done);
        Ok(())
    }

    /// Refuses the commands sent from now on. Fails when the map was
    /// released already.
    fn close(&self) -> Result<(), Error> {
        let _sent = self.sent.lock();
        if self.released.swap(true, Ordering::SeqCst) {
            return Err(released());
        }
        Ok(())
    }

    /// Removes the closed map from its runtime's thread, after the commands
    /// sent before, once.
    fn remove(&self, done: Option<Resolver<()>>) {
        if self.removed.swap(true, Ordering::SeqCst) {
            return;
        }
        let id = self.id;
        // A runtime that a panic stopped let go of the map already.
        self.runtime.post_or_stopped(
            done,
            move |core, outbox, done| remove(core, id, done, outbox),
            |done| {
                if let Some(done) = done {
                    done.resolve(Ok(()));
                }
            },
        );
        self.runtime.map_released();
    }
}

impl Drop for MapShared {
    fn drop(&mut self) {
        let _ = self.close();
        let deferred = self.session.get_mut().take().and_then(|claim| claim.removal);
        self.remove(deferred);
    }
}

/// A handle to a map, which lives on its runtime's thread.
///
/// Each command takes the next sequence number when it is sent and runs on
/// the runtime's thread after every command sent before it, to this map,
/// another map or the runtime. It returns a [`Completion`] of its result, or
/// an error when it can't be sent, and then never completes. A command sent
/// before the map is built waits for it. `Clone` is another handle to the
/// same map, and any thread may use one.
///
/// The map lives until [`release`](Self::release) or until its last handle
/// drops. A command on a released map fails with
/// [`ErrorKind::InvalidState`](mln_style::error::ErrorKind).
#[derive(Clone)]
pub struct Map {
    pub(crate) shared: Arc<MapShared>,
    /// Where the failures of the handle's commands go when it discards
    /// their results.
    discard: Option<fn(Error)>,
}

impl std::fmt::Debug for Map {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Map").field("id", &self.shared.id).field("discards_results", &self.discard.is_some()).finish()
    }
}

impl PartialEq for Map {
    fn eq(&self, other: &Map) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl Eq for Map {}

fn released() -> Error {
    Error::invalid_state("the map was released")
}

pub(crate) fn style_not_loaded() -> Error {
    Error::invalid_state("style is not loaded")
}

/// The error of a command whose map failed to be built, or was released
/// while the command was on its way.
pub(crate) fn missing_map() -> Error {
    Error::invalid_state("the map failed to be built or was released")
}

/// What a render session runs with the outcome of a command it waits for.
type Reply<T> = Box<dyn FnOnce(&mut Worker, Result<T, Error>, &mut Outbox) + Send>;

/// What a render session waits for from a command on the runtime's thread:
/// `reply` runs on the session's thread with the command's outcome, or with
/// an error if the command drops without one, as when a panic stopped the
/// runtime.
struct SessionReply<T: Send + 'static> {
    queue: SessionQueue,
    reply: Option<Reply<T>>,
}

impl<T: Send + 'static> SessionReply<T> {
    fn new(
        queue: SessionQueue,
        reply: impl FnOnce(&mut Worker, Result<T, Error>, &mut Outbox) + Send + 'static,
    ) -> Self {
        SessionReply { queue, reply: Some(Box::new(reply)) }
    }

    fn send(mut self, result: Result<T, Error>) {
        if let Some(reply) = self.reply.take() {
            worker_post(&self.queue, move |worker, outbox| reply(worker, result, outbox));
        }
    }
}

impl<T: Send + 'static> Drop for SessionReply<T> {
    fn drop(&mut self) {
        if let Some(reply) = self.reply.take() {
            let error = Error::cancelled("the runtime stopped before the command ran");
            worker_post(&self.queue, move |worker, outbox| reply(worker, Err(error), outbox));
        }
    }
}

impl Map {
    pub(crate) fn new(shared: MapShared) -> Self {
        Map { shared: Arc::new(shared), discard: None }
    }

    /// A handle to the same map whose commands discard their results, for a
    /// caller that doesn't need them: each command's completion acts as one
    /// whose result was taken, and costs no allocation, and a failure,
    /// cancellations included, goes to `on_failure` on the runtime's thread.
    #[must_use]
    pub fn discarding_results(&self, on_failure: fn(Error)) -> Map {
        Map { shared: Arc::clone(&self.shared), discard: Some(on_failure) }
    }

    /// Numbers a command and posts it to the runtime's thread, with a
    /// completion as the handle makes them.
    pub(crate) fn post<R: Send + 'static>(
        &self,
        command: impl FnOnce(&mut RuntimeCore, &mut Outbox, MapId, u64, Resolver<R>) + Send + 'static,
    ) -> Result<Completion<R>, Error> {
        self.shared.send(self.discard, command)
    }

    #[must_use]
    pub fn id(&self) -> MapId {
        self.shared.id
    }

    /// When the map renders, as its options set it.
    #[must_use]
    pub fn mode(&self) -> MapMode {
        self.shared.mode
    }

    /// The runtime that the map lives on.
    #[must_use]
    pub fn runtime(&self) -> Runtime {
        Runtime { shared: Arc::clone(&self.shared.runtime) }
    }

    /// The sequence number of the last command sent to the map.
    #[must_use]
    pub fn sent_sequence(&self) -> u64 {
        *self.shared.sent.lock()
    }

    /// The sequence number of the last command that ran. While the
    /// runtime's thread runs several queued commands as one burst, they
    /// count once the frame that shows them all can.
    #[must_use]
    pub fn applied_sequence(&self) -> u64 {
        self.shared.published.applied.load(Ordering::SeqCst)
    }

    /// The map's state after the last command that ran, without waiting for
    /// the commands still on their way. To read the state after them, send a
    /// command and read the snapshot once it completes, or use a query such
    /// as [`camera`](Self::camera). Its `update_generation` counts the
    /// renderer's updates as they are when it is read, which can be later
    /// than the rest of the state.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) until the
    /// map is built, and after it is released.
    pub fn snapshot(&self) -> Result<MapSnapshot, Error> {
        let mut snapshot = self.published(|state| state.snapshot)?;
        // The map publishes an update, which the renderer may draw at once,
        // before it copies the snapshot: count the updates as they are now, so
        // a frame never shows a later update than the snapshot.
        if let Some(updates) = self.shared.published.updates.get() {
            snapshot.update_generation = updates.generation();
        }
        Ok(snapshot)
    }

    /// A projection of `view`, or of the map's view as of
    /// [`snapshot`](Self::snapshot) for `None`, on the ground the map drew
    /// then (`mln_map::Map::ground`): it converts as the map does, over
    /// terrain on the ground the renderer drew, else on the flat map. It is
    /// the caller's, and doesn't follow the map.
    ///
    /// Fails as [`snapshot`](Self::snapshot) does, and for `view` as
    /// [`MapView::to_state`](mln_map::MapView::to_state).
    pub fn projection(&self, view: Option<&MapView>) -> Result<MapProjection, Error> {
        let (snapshot_view, ground) = self.published(|state| (state.snapshot.view, state.ground.clone()))?;
        Ok(MapProjection::from_view(view.unwrap_or(&snapshot_view))?.with_ground(ground))
    }

    /// What `read` takes from the map's last flush. Fails with
    /// [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) until the map is
    /// built, and after it is released.
    fn published<T>(&self, read: impl FnOnce(&PublishedState) -> T) -> Result<T, Error> {
        if self.shared.released.load(Ordering::SeqCst) {
            return Err(released());
        }
        self.shared.published.state.lock().as_ref().map(read).ok_or_else(|| Error::invalid_state("the map isn't built"))
    }

    /// The events that the map reports, as of the last command that ran.
    #[must_use]
    pub fn event_mask(&self) -> EventMask {
        EventMask::from_bits(self.shared.published.event_mask.load(Ordering::SeqCst)).unwrap_or_default()
    }

    /// Sends a command that runs `apply` on the map's entry, and completes
    /// with what `finish` makes of its result and the map after it.
    pub(crate) fn send<T: 'static, R: Send + 'static>(
        &self,
        apply: impl FnOnce(&mut MapEntry, &mut Outbox) -> Result<T, Error> + Send + 'static,
        finish: impl FnOnce(T, &MapEntry) -> R + Send + 'static,
    ) -> Result<Completion<R>, Error> {
        self.post(move |core, outbox, id, sequence, resolver| {
            run_command(core, id, sequence, apply, finish, resolver, outbox);
        })
    }

    /// A command that changes the map.
    pub(crate) fn command(
        &self,
        apply: impl FnOnce(&mut mln_map::Map) -> Result<(), Error> + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        self.send(move |entry, _| apply(&mut entry.core), |(), entry| entry.applied())
    }

    /// A command that reads the map.
    pub(crate) fn query<T: Send + 'static>(
        &self,
        read: impl FnOnce(&mut mln_map::Map) -> Result<T, Error> + Send + 'static,
    ) -> Result<Completion<T>, Error> {
        self.send(move |entry, _| read(&mut entry.core), |value, _| value)
    }

    /// Runs `f` on the map itself, on the runtime's thread, in its place
    /// among the map's commands. It sees the effects of the commands sent
    /// before it. The map handles what `f` changed, and reports its events,
    /// before the completion.
    pub fn with_core<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut mln_map::Map) -> R + Send + 'static,
    ) -> Result<Completion<R>, Error> {
        self.query(move |map| Ok(f(map)))
    }

    /// Releases the map. The completion reports when the runtime has let go
    /// of it. A render session that was released, but hasn't let go of the
    /// map yet, is abandoned, as
    /// [`RenderSession::abandon`](crate::RenderSession::abandon) does: a
    /// caller-driven one that the host stopped servicing would otherwise
    /// hold the map for good.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) when the
    /// map was released already, or while a render session that wasn't
    /// released is attached to it.
    pub fn release(&self) -> Result<Completion<()>, Error> {
        self.release_with(false)
    }

    /// Releases the map whatever its state, as dropping its last handle
    /// does, and lets the runtime finish in the background: the completion
    /// reports when the runtime has let go of it, and may be dropped. Every
    /// command fails with [`ErrorKind::InvalidState`](mln_style::error::ErrorKind)
    /// from now on. A render session that is attached and wasn't released
    /// keeps drawing the map's last update, and the runtime lets go of the
    /// map once the session detaches or is abandoned. A released one is
    /// abandoned, as [`release`](Self::release) does.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) when the
    /// map was released already.
    pub fn dispose(&self) -> Result<Completion<()>, Error> {
        self.release_with(true)
    }

    fn release_with(&self, in_any_state: bool) -> Result<Completion<()>, Error> {
        let (completion, resolver) = self.shared.completion(self.discard);
        let claimed = {
            let mut claim = self.shared.session.lock();
            match claim.as_mut() {
                None => None,
                Some(held) => match held.session.upgrade() {
                    // Abandoned now, or ended already and about to let go.
                    Some(session) if session.is_released() => Some((held.id, Some(session))),
                    // The session lets go of the map when it ends, and the
                    // map is removed then.
                    Some(_) if in_any_state => {
                        self.shared.close()?;
                        held.removal = Some(resolver);
                        return Ok(completion);
                    }
                    Some(_) => return Err(Error::invalid_state("map still has an attached render session")),
                    // The session is gone without letting go of the map.
                    None => Some((held.id, None)),
                },
            }
        };
        if let Some((id, session)) = claimed {
            if let Some(session) = session {
                let _ = session.abandon(Abandoner::Library);
            }
            self.release_session_claim(id);
        }
        self.shared.release(Some(resolver))?;
        Ok(completion)
    }

    // MARK: - Map

    /// Sets the map's size and scale factor. The map keeps its tiles, and a
    /// resize to the extent the map has changes nothing, so it renders
    /// nothing. Fails with [`ErrorKind::InvalidState`](mln_style::error::ErrorKind)
    /// while a render session is attached, since the session sizes the map
    /// to its target: resize the session instead. Fails with
    /// [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for an empty size,
    /// or a scale factor that isn't finite and positive.
    pub fn resize(&self, extent: LogicalExtent) -> Result<Completion<Applied>, Error> {
        let LogicalExtent { size, scale_factor } = extent;
        if self.shared.session.lock().is_some() {
            return Err(Error::invalid_state("the map has a render session: resize the session instead"));
        }
        if size.is_empty() {
            return Err(Error::invalid_argument("logical size dimensions must be positive"));
        }
        if !scale_factor.is_finite() || scale_factor <= 0.0 {
            return Err(Error::invalid_argument(format!("scale factor {scale_factor} isn't finite and positive")));
        }
        // The scale factor that a session attached later renders at.
        self.shared.scale_factor.store(scale_factor.to_bits(), Ordering::SeqCst);
        self.command(move |map| {
            if map.snapshot().view.extent.size != size {
                map.set_size(size)?;
            }
            map.set_pixel_ratio(scale_factor as f32)
        })
    }

    /// Renders a new frame of a continuous map, even when nothing changed.
    /// Fails with [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) for a
    /// static or tile map, which renders only still images.
    pub fn request_repaint(&self) -> Result<Completion<Applied>, Error> {
        if self.shared.mode != MapMode::Continuous {
            return Err(Error::invalid_state("map is not in continuous mode"));
        }
        self.command(|map| {
            map.trigger_repaint();
            Ok(())
        })
    }

    /// Holds the map's changes back from rendering until the matching
    /// [`end_command_group`](Self::end_command_group), so that they show in
    /// the same frame. Groups nest, and the changes show when the outermost
    /// group ends.
    pub fn begin_command_group(&self) -> Result<Completion<Applied>, Error> {
        self.command(|map| {
            map.begin_command_group();
            Ok(())
        })
    }

    /// Ends the group that [`begin_command_group`](Self::begin_command_group)
    /// began. Completes with [`ErrorKind::InvalidState`](mln_style::error::ErrorKind)
    /// when no group is open.
    pub fn end_command_group(&self) -> Result<Completion<Applied>, Error> {
        self.command(mln_map::Map::end_command_group)
    }

    /// Begins a command group that ends when the guard drops, or with
    /// [`CommandGroup::end`].
    pub fn command_group(&self) -> Result<CommandGroup, Error> {
        drop(self.begin_command_group()?);
        Ok(CommandGroup { map: Some(self.clone()) })
    }

    /// Selects the events that the map reports.
    pub fn set_event_mask(&self, mask: EventMask) -> Result<Completion<Applied>, Error> {
        self.send(
            move |entry, _| {
                if entry.mask != mask {
                    entry.mask = mask;
                    entry.published.event_mask.store(mask.bits(), Ordering::SeqCst);
                    entry.core.record_change();
                }
                Ok(())
            },
            |(), entry| entry.applied(),
        )
    }

    // MARK: - Rendering

    /// Attaches a render session, which renders the map into `target`, and
    /// sizes the map to the target. A map has one session at a time. The
    /// session takes frame demands and other work at once, and runs them
    /// once the completion reports it ready.
    ///
    /// The session renders on a thread of its own, or, with
    /// [`Driver::CallerThread`](crate::Driver::CallerThread), on the thread
    /// that takes its [`DriverWork`](crate::DriverWork): the attach completes
    /// in that thread's first service.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) when the
    /// map already has a session or was released, or a window is minimized
    /// or has no client area, and with
    /// [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for an empty
    /// target, a [`Surface::wgpu`](crate::Surface::wgpu) without the
    /// host's device, or a Win32 window with a pixel format of its own that
    /// OpenGL would present to. The completion fails as
    /// [`Device::new`](crate::Device::new) does when the
    /// session has to create its device and can't, with
    /// [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) when the window or
    /// surface had no area to present to by then, with
    /// [`ErrorKind::Unsupported`](mln_style::error::ErrorKind) when no GPU can present
    /// to a window, and with [`ErrorKind::Native`](mln_style::error::ErrorKind) when
    /// the window has no surface to give.
    pub fn attach(
        &self,
        target: RenderTarget,
        options: AttachOptions,
    ) -> Result<(RenderSession, Completion<()>), Error> {
        crate::render::attach(self, target, options, |session, attached| (session.clone(), attached))
    }

    /// Attaches a render session as [`attach`](Self::attach) does, and calls
    /// `then` with it and the attach's completion before the session's
    /// thread may deliver the attach's result. A callback that `then`
    /// attaches with [`Completion::on_complete`] runs where the session
    /// delivers it: on the session's thread, inside its driver call, even
    /// when the session attaches before `then` runs. One attached after
    /// `attach` returned would run at once on this thread instead.
    ///
    /// `then` keeps the session with a clone of it: the session is released
    /// once its last handle drops. Fails as `attach` does, before `then`
    /// runs.
    pub fn attach_then<R>(
        &self,
        target: RenderTarget,
        options: AttachOptions,
        then: impl FnOnce(&RenderSession, Completion<()>) -> R,
    ) -> Result<R, Error> {
        crate::render::attach(self, target, options, then)
    }

    /// Renders a still image of a static or tile map, once every resource
    /// that it needs has loaded. The map's render session draws it, and
    /// [`RenderSession::read_texture`] reads its pixels once the completion
    /// reports success.
    ///
    /// While the image waits, the session has a demand of its own for it,
    /// whose [`FrameResult`](crate::FrameResult) has no token. The demand
    /// waits for the map to load what the image needs, and parks while a
    /// borrowed ring has no texture to render into. It finishes with the
    /// image's frame, which completes the image once its result is queued:
    /// a frame of the host's demands completes none. The host's demands
    /// neither replace it nor are replaced by it. When the session ends its
    /// demands, by a barrier, a target's release or a detach, the demand
    /// ends too, and the image waits for the next one: once the barrier ran,
    /// the target came back, or the next session attached.
    ///
    /// Fails with [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) for a
    /// continuous map. Completes with
    /// [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) while another image
    /// is on its way or a command group is open,
    /// [`ErrorKind::Native`](mln_style::error::ErrorKind) when the
    /// style or a resource that the image needs fails to load, and
    /// [`ErrorKind::Cancelled`](mln_style::error::ErrorKind) when the map is released
    /// first.
    pub fn request_still_image(&self) -> Result<Completion<()>, Error> {
        if self.shared.mode == MapMode::Continuous {
            return Err(Error::invalid_state("the map is not in static or tile mode"));
        }
        self.post(move |core, outbox, id, sequence, resolver| {
            if !core.maps.contains_key(&id) {
                return outbox.resolve(resolver, Err(missing_map()));
            }
            apply_command(
                core,
                id,
                sequence,
                outbox,
                move |entry, outbox| {
                    if entry.pending_still.is_some() {
                        let error = Error::invalid_state("the map is rendering a still image already");
                        return outbox.resolve(resolver, Err(error));
                    }
                    // The group holds back the update that the image needs.
                    if entry.core.command_group_depth() > 0 {
                        let error = Error::invalid_state("a still image can't render while a command group is open");
                        return outbox.resolve(resolver, Err(error));
                    }
                    let refused = entry.core.render_still().err().map(|error| Err(still_image_error(error)));
                    entry.pending_still = Some(PendingStill { resolver, refused });
                },
                |_, (), _, _| {},
            );
        })
    }

    /// Whether the map was released or disposed.
    pub(crate) fn is_released(&self) -> bool {
        self.shared.released.load(Ordering::SeqCst)
    }

    pub(crate) fn runtime_thread(&self) -> std::thread::ThreadId {
        self.shared.runtime.thread()
    }

    /// The map's scale factor: device pixels per logical pixel. It is the
    /// pixel ratio of the map's options until a render session's
    /// [`resize`](crate::RenderSession::resize) sets another, and the map's
    /// next session renders at it.
    #[must_use]
    pub fn scale_factor(&self) -> f64 {
        f64::from_bits(self.shared.scale_factor.load(Ordering::SeqCst))
    }

    pub(crate) fn tile_cache_enabled(&self) -> bool {
        self.shared.tile_cache_enabled
    }

    /// How the map's renderers draw glyphs from local fonts: with system
    /// fonts, from the fonts the system chooses, or in the browser, which
    /// has no font files, from the browser's canvas, falling back to its
    /// sans-serif font as MapLibre GL JS's `localIdeographFontFamily`
    /// defaults to.
    pub(crate) fn local_glyphs(&self) -> LocalGlyphs {
        let mut local = LocalGlyphs { ideographs: self.shared.local_ideographs, ..LocalGlyphs::default() };
        if self.shared.system_fonts {
            #[cfg(not(target_family = "wasm"))]
            {
                local.chooser = Some(Arc::new(mln_system_fonts::SystemFonts));
            }
            #[cfg(target_family = "wasm")]
            {
                let font_family = "sans-serif".to_owned();
                local.rasterizer =
                    Some(Arc::new(mln_text::browser_glyph_rasterizer::BrowserGlyphRasterizer::new(font_family)));
            }
        }
        local
    }

    pub(crate) fn glyph_cache(&self) -> Arc<GlyphCache> {
        Arc::clone(&self.shared.runtime.glyph_cache)
    }

    /// Where the runtime's background work runs.
    pub(crate) fn pool(&self) -> Pool {
        self.shared.runtime.pool.clone()
    }

    /// What the renderers of the runtime's maps load font faces with.
    pub(crate) fn font_face_engine(&self) -> Arc<dyn FontFaceEngine> {
        Arc::clone(&self.shared.runtime.font_face_engine)
    }

    /// What the map parses its style against, its renderer draws and its
    /// tile workers lay out: its runtime's
    /// [`registry`](crate::Runtime::registry). A filter for a query parses
    /// against its functions.
    #[must_use]
    pub fn registry(&self) -> &Arc<Registry> {
        &self.shared.runtime.registry
    }

    /// Reserves the map for the session `id`.
    pub(crate) fn claim_session(&self, id: SessionId, session: Weak<SessionShared>) -> Result<(), Error> {
        let mut claim = self.shared.session.lock();
        if self.shared.released.load(Ordering::SeqCst) {
            return Err(released());
        }
        if claim.is_some() {
            return Err(Error::invalid_state("the map already has an attached render session"));
        }
        *claim = Some(SessionClaim { id, session, removal: None });
        Ok(())
    }

    /// Lets another session attach once the session `id` is gone, and
    /// removes the map when a dispose waited for the session.
    pub(crate) fn release_session_claim(&self, id: SessionId) {
        let claim = {
            let mut session = self.shared.session.lock();
            if session.as_ref().is_some_and(|claim| claim.id == id) { session.take() } else { None }
        };
        if let Some(removal) = claim.and_then(|claim| claim.removal) {
            self.shared.remove(Some(removal));
        }
    }

    /// Sends the command that binds the session `id`, whose thread runs
    /// `queue`, to the map: it sizes the map to `size`, and hands the
    /// session the map's updates. The map tells the session of the commands
    /// it runs from `awaited` on.
    pub(crate) fn bind_session(
        &self,
        id: SessionId,
        queue: SessionQueue,
        awaited: Arc<AtomicU64>,
        size: Size,
    ) -> Result<(), Error> {
        let reply = SessionReply::new(queue.clone(), |worker, bound, outbox| match bound {
            Ok((sequence, reader, events, published)) => worker.bound(sequence, reader, events, published, outbox),
            Err(error) => worker.bind_failed(error, outbox),
        });
        let sent = self.shared.send(Some(drop), move |core, outbox, map_id, sequence, resolver: Resolver<()>| {
            drop(resolver);
            if !core.maps.contains_key(&map_id) {
                return reply.send(Err(missing_map()));
            }
            let link = SessionLink { id, queue, awaited };
            apply_command(
                core,
                map_id,
                sequence,
                outbox,
                move |entry, _| {
                    entry.session = Some(link);
                    let resized = entry.core.set_size(size);
                    (resized, entry.core.update_reader(), entry.core.renderer_events(), Arc::clone(&entry.published))
                },
                move |_, (resized, reader, events, published), _, _| {
                    reply.send(resized.map(|()| (sequence, reader, events, published)));
                },
            );
        });
        sent.map(drop)
    }

    /// Unbinds the session `id` from the map on the runtime's thread, after
    /// the commands sent before.
    pub(crate) fn unbind_session(&self, id: SessionId) {
        let map_id = self.shared.id;
        // A runtime that shut down has no map to unbind from.
        self.shared.runtime.post_or((), move |core, _, ()| unbind(core, map_id, id), |()| {});
    }

    /// Unbinds the session `id` as [`unbind_session`](Self::unbind_session)
    /// does, then runs `then` on the session's thread.
    pub(crate) fn unbind_session_then(
        &self,
        id: SessionId,
        queue: SessionQueue,
        then: impl FnOnce(&mut Worker, &mut Outbox) + Send + 'static,
    ) {
        let map_id = self.shared.id;
        let reply = SessionReply::new(queue, move |worker, _: Result<(), Error>, outbox| then(worker, outbox));
        // A runtime that shut down or stopped has no map to unbind from, and
        // the reply's drop runs `then`.
        self.shared.runtime.post_or(
            reply,
            move |core, _, reply| {
                unbind(core, map_id, id);
                reply.send(Ok(()));
            },
            drop,
        );
    }

    /// For a frame demanded now: sets the map's frame clock, which its
    /// animations sample at and its camera commands start from, and while
    /// an animation runs, has the map sample it at this time. Returns the
    /// sample that the frame waits for, or 0 for none. The demands that ask
    /// before the runtime's thread takes a sample share it.
    pub(crate) fn frame_demanded(&self) -> u64 {
        let published = &self.shared.published;
        if let Some(clock) = published.frame_clock.get() {
            clock.set(Some(Instant::now()));
        }
        // A still map has nothing to sample, and its frames don't wait for
        // the runtime's thread.
        if self.shared.mode != MapMode::Continuous || !published.animating() {
            return 0;
        }
        let sample = published.samples_requested.fetch_add(1, Ordering::SeqCst) + 1;
        if !published.sample_queued.swap(true, Ordering::SeqCst) {
            let id = self.shared.id;
            self.shared.runtime.post_apart(move |core, outbox| sample_frame(core, id, outbox));
        }
        sample
    }

    /// Sends a command that resizes the map to `size` at `scale_factor`,
    /// then runs `resize` with its outcome on the thread of the session that
    /// `queue` runs, before the session learns of any later command.
    pub(crate) fn resize_for_session(
        &self,
        size: Size,
        scale_factor: f64,
        queue: SessionQueue,
        resize: impl FnOnce(&mut Worker, Result<Applied, Error>, &mut Outbox) + Send + 'static,
    ) -> Result<(), Error> {
        // The scale factor that a session attached later renders at.
        self.shared.scale_factor.store(scale_factor.to_bits(), Ordering::SeqCst);
        let reply = SessionReply::new(queue, resize);
        let sent = self.shared.send(Some(drop), move |core, outbox, id, sequence, resolver: Resolver<()>| {
            drop(resolver);
            if !core.maps.contains_key(&id) {
                return reply.send(Err(missing_map()));
            }
            apply_command(
                core,
                id,
                sequence,
                outbox,
                move |entry, _| {
                    // A host may hand the session the target it has, such as
                    // on every frame. That changes nothing, so it publishes
                    // no update, which a demand sent before would draw.
                    let pixel_ratio = scale_factor as f32;
                    let extent = entry.core.snapshot().view.extent;
                    if extent.size == size && extent.scale_factor == f64::from(pixel_ratio) {
                        return Ok(());
                    }
                    entry.core.set_size(size)?;
                    entry.core.set_pixel_ratio(pixel_ratio)
                },
                move |_, resized, applied, _| reply.send(resized.map(|()| applied)),
            );
        });
        sent.map(drop)
    }

    // MARK: - Feature state

    /// Merges `state` into the state of the feature that `selector` names:
    /// its keys replace the same keys of the feature's state, and the other
    /// keys stay.
    ///
    /// The map keeps the state until it is removed, its source leaves the
    /// style, or a style loads: [`remove_style_source`](Self::remove_style_source)
    /// removes the state of the source's features, and a style that loads
    /// removes the state of every source. To set state for a new style's
    /// sources, wait until [`set_style_url`](Self::set_style_url) or
    /// [`set_style_json`](Self::set_style_json) completes.
    ///
    /// The map's next update for its render session carries the state, so a
    /// frame shows it, and a rendered query reads it, once the frame renders
    /// that update. A command that changes no state publishes no update: an
    /// empty `state` merges nothing.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for a
    /// selector without a feature.
    pub fn set_feature_state(
        &self,
        selector: &FeatureStateSelector,
        state: FeatureState,
    ) -> Result<Completion<Applied>, Error> {
        let key = selector.key(true)?;
        self.command(move |map| {
            let feature_id = key.feature_id.as_deref().unwrap_or_default();
            map.set_feature_state(&key.source_id, Some(&key.source_layer_id), feature_id, state);
            Ok(())
        })
    }

    /// The state of the feature that `selector` names, empty when it has
    /// none. Fails with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind)
    /// for a selector without a feature.
    pub fn feature_state(&self, selector: &FeatureStateSelector) -> Result<Completion<FeatureState>, Error> {
        let key = selector.key(true)?;
        self.query(move |map| {
            let feature_id = key.feature_id.as_deref().unwrap_or_default();
            Ok(map.feature_state(&key.source_id, Some(&key.source_layer_id), feature_id))
        })
    }

    /// Removes the state that `selector` selects: one key of a feature, all
    /// of a feature's state, or that of every feature in a source layer.
    /// The removal reaches the render session as
    /// [`set_feature_state`](Self::set_feature_state) describes, and a
    /// removal of no state publishes no update. Fails with
    /// [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for a state key
    /// without a feature.
    pub fn remove_feature_state(&self, selector: &FeatureStateSelector) -> Result<Completion<Applied>, Error> {
        let key = selector.key(false)?;
        self.command(move |map| {
            map.remove_feature_state(
                &key.source_id,
                Some(&key.source_layer_id),
                key.feature_id.as_deref(),
                key.state_key.as_deref(),
            );
            Ok(())
        })
    }

    // MARK: - Style

    /// Loads the map's style from `url`.
    ///
    /// Completes once the style document has loaded and parsed, with
    /// [`ErrorKind::Native`](mln_style::error::ErrorKind) when it fails to, and with
    /// [`ErrorKind::Cancelled`](mln_style::error::ErrorKind) when another style load
    /// starts or the map is released first. The style's sources, sprites and
    /// glyphs then load in the background. Commands that change the style
    /// fail with [`ErrorKind::InvalidState`](mln_style::error::ErrorKind) until the
    /// document has parsed.
    pub fn set_style_url(&self, url: impl Into<String>) -> Result<Completion<Applied>, Error> {
        let url = url.into();
        if url.is_empty() {
            return Err(Error::invalid_argument("style URL must not be empty"));
        }
        self.load_style(StyleLoad::Url(url))
    }

    /// Loads the map's style from JSON text. Completes as
    /// [`set_style_url`](Self::set_style_url) does, once the style has
    /// parsed, but with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind)
    /// for JSON that isn't a valid style. A style that fails to parse leaves
    /// the map's style as it was.
    pub fn set_style_json(&self, json: impl Into<String>) -> Result<Completion<Applied>, Error> {
        let json = json.into();
        if json.is_empty() {
            return Err(Error::invalid_argument("style JSON must not be empty"));
        }
        self.load_style(StyleLoad::Json(json))
    }

    fn load_style(&self, load: StyleLoad) -> Result<Completion<Applied>, Error> {
        self.post(move |core, outbox, id, sequence, resolver| {
            load_style(core, id, sequence, load, resolver, outbox);
        })
    }

    /// The JSON of the style that the map loaded, without the changes made
    /// since.
    pub fn loaded_style_json(&self) -> Result<Completion<String>, Error> {
        self.query(|map| Ok(map.style().get_json().to_owned()))
    }

    /// The URL of the map's style, or `None` for a style loaded from JSON.
    pub fn style_url(&self) -> Result<Completion<Option<String>>, Error> {
        self.query(|map| Ok(map.style_url().filter(|url| !url.is_empty()).map(str::to_owned)))
    }

    /// Adds a source to the style from its JSON. See
    /// `mln_map::Map::add_source_json` for the errors.
    pub fn add_style_source_json(
        &self,
        source_id: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (source_id, json) = (source_id.into(), json.into());
        self.command(move |map| map.add_source_json(&source_id, &json))
    }

    /// Removes a source from the style. Completes with
    /// [`ErrorKind::NotFound`](mln_style::error::ErrorKind) when the style has no
    /// such source, and [`ErrorKind::InvalidState`](mln_style::error::ErrorKind)
    /// when a layer uses it or the style isn't loaded. The source's features
    /// lose their state, and a custom source's callbacks drop before the
    /// completion.
    pub fn remove_style_source(&self, source_id: impl Into<String>) -> Result<Completion<Applied>, Error> {
        let source_id = source_id.into();
        self.send(
            move |entry, _| {
                let map = &mut entry.core;
                if !map.style_document_loaded() {
                    return Err(style_not_loaded());
                }
                if map.source(&source_id).is_none() {
                    return Err(Error::not_found(format!("source \"{source_id}\" does not exist")));
                }
                if let Some(layer) = map.style().get_layers().find(|layer| layer.get_source_id() == source_id) {
                    let message = format!("source \"{source_id}\" is used by layer \"{}\"", layer.get_id());
                    return Err(Error::invalid_state(message));
                }
                map.remove_source(&source_id);
                entry.core.remove_source_feature_state(&source_id);
                Ok(())
            },
            |(), entry| entry.applied(),
        )
    }

    /// Adds a layer to the style from its JSON, below `before_layer_id` or
    /// above every layer. See `Style::add_layer_json` for the errors.
    pub fn add_style_layer_json(
        &self,
        json: impl Into<String>,
        before_layer_id: Option<String>,
    ) -> Result<Completion<Applied>, Error> {
        let json = json.into();
        self.command(move |map| map.edit_loaded_style(|style| style.add_layer_json(&json, before_layer_id.as_deref())))
    }

    /// Adds a custom layer named `layer_id` that `host` draws, below
    /// `before_layer_id` or above every layer. See `Style::add_layer` for the
    /// errors. The map lets go of `host` once the style no longer has
    /// the layer, and no renderer draws it.
    pub fn add_custom_layer(
        &self,
        layer_id: impl Into<String>,
        host: Arc<dyn CustomLayerHost>,
        before_layer_id: Option<String>,
    ) -> Result<Completion<Applied>, Error> {
        let layer_id = layer_id.into();
        self.command(move |map| {
            map.edit_loaded_style(|style| {
                let layer = Custom::layer(style.registry(), &layer_id, host);
                style.add_layer(layer, before_layer_id.as_deref())
            })
        })
    }

    /// Removes a layer from the style. Completes with
    /// [`ErrorKind::NotFound`](mln_style::error::ErrorKind) when the style has no
    /// such layer, and [`ErrorKind::InvalidState`](mln_style::error::ErrorKind)
    /// while the style isn't loaded.
    pub fn remove_style_layer(&self, layer_id: impl Into<String>) -> Result<Completion<Applied>, Error> {
        let layer_id = layer_id.into();
        self.command(move |map| map.edit_loaded_style(|style| style.apply(Edit::RemoveLayer(layer_id))))
    }

    /// A layer of the style as JSON, or `None` when the style has no such
    /// layer.
    pub fn style_layer_json(&self, layer_id: impl Into<String>) -> Result<Completion<Option<String>>, Error> {
        let layer_id = layer_id.into();
        self.query(move |map| Ok(map.style().layer_json(&layer_id)))
    }

    /// Sets a property of a layer to a JSON value. See
    /// `Style::set_layer_property_json` for the errors.
    pub fn set_style_layer_property(
        &self,
        layer_id: impl Into<String>,
        name: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (layer_id, name, json) = (layer_id.into(), name.into(), json.into());
        self.command(move |map| map.edit_loaded_style(|style| style.set_layer_property_json(&layer_id, &name, &json)))
    }

    /// A property of a layer as JSON, or `None` when the layer doesn't set
    /// it. See `Style::layer_property_json` for the errors.
    pub fn style_layer_property(
        &self,
        layer_id: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Completion<Option<String>>, Error> {
        let (layer_id, name) = (layer_id.into(), name.into());
        self.query(move |map| map.style().layer_property_json(&layer_id, &name))
    }

    /// Sets a root property of the style to a JSON value, without
    /// reloading the style. See `Style::set_root_property_json` for the
    /// properties that change and the errors.
    pub fn set_style_root_property(
        &self,
        name: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (name, json) = (name.into(), json.into());
        self.command(move |map| map.edit_loaded_style(|style| style.set_root_property_json(&name, &json)))
    }

    /// A root property of the style as JSON, or `None` when neither the
    /// style nor the map sets it. See `Style::root_property_json`.
    pub fn style_root_property(&self, name: impl Into<String>) -> Result<Completion<Option<String>>, Error> {
        let name = name.into();
        self.query(move |map| map.style().root_property_json(&name))
    }

    /// Sets a property of the style's light to a JSON value.
    pub fn set_style_light_property(
        &self,
        name: impl Into<String>,
        json: impl Into<String>,
    ) -> Result<Completion<Applied>, Error> {
        let (name, json) = (name.into(), json.into());
        self.command(move |map| map.edit_loaded_style(|style| style.set_light_property_json(&name, &json)))
    }

    /// A property of the style's light as JSON, or `None` when the light
    /// doesn't set it.
    pub fn style_light_property(&self, name: impl Into<String>) -> Result<Completion<Option<String>>, Error> {
        let name = name.into();
        self.query(move |map| Ok(map.style().light_property_json(&name)))
    }

    // MARK: - Camera

    /// A command that moves the camera: `apply` starts a camera command,
    /// whose end nobody waits for.
    fn camera_command(
        &self,
        apply: impl FnOnce(&mut mln_map::Map) -> Result<CameraCommandId, Error> + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        self.send_camera(apply, None)
    }

    /// A command that moves the camera, whose end `on_end` receives. `on_end`
    /// is in place before the command is sent, so it runs on the runtime's
    /// thread, or on this one with
    /// [`ErrorKind::Cancelled`](mln_style::error::ErrorKind) when the send fails.
    fn camera_command_then(
        &self,
        apply: impl FnOnce(&mut mln_map::Map) -> Result<CameraCommandId, Error> + Send + 'static,
        on_end: impl FnOnce(Result<TransitionEnd, Error>) + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        let (ended, end) = self.shared.runtime.completion();
        ended.on_complete(on_end);
        self.send_camera(apply, Some(end))
    }

    /// Sends a camera command, and `end`, if any, the command's end: a
    /// failure ends it as cancelled.
    fn send_camera(
        &self,
        apply: impl FnOnce(&mut mln_map::Map) -> Result<CameraCommandId, Error> + Send + 'static,
        end: Option<Resolver<TransitionEnd>>,
    ) -> Result<Completion<Applied>, Error> {
        self.send(
            move |entry, outbox| match (apply(&mut entry.core), end) {
                (Ok(command), Some(end)) => {
                    entry.camera_commands.insert(command, end);
                    Ok(())
                }
                (Ok(_), None) => Ok(()),
                (Err(error), end) => {
                    if let Some(end) = end {
                        let generation = entry.core.generation();
                        let cancelled =
                            TransitionEnd { outcome: TransitionOutcome::Cancelled, generation, ..Default::default() };
                        outbox.resolve(end, Ok(cancelled));
                    }
                    Err(error)
                }
            },
            |(), entry| entry.applied(),
        )
    }

    /// Moves the camera to absolute values, by a jump, an ease or a flight.
    pub fn update_camera(&self, update: CameraUpdate) -> Result<Completion<Applied>, Error> {
        self.camera_command(move |map| Ok(map.update_camera(&update)))
    }

    /// Moves the camera as [`update_camera`](Self::update_camera) does, and
    /// calls `on_end` with how the change ended, on the runtime's thread:
    /// once the map has published the snapshot that shows where the change
    /// left the camera, whose generation the end reports, and before the
    /// returned completion when the change ends within the command. A
    /// command that fails ends as cancelled, and one that the map's release
    /// ends as closed, with generation 0. `on_end` runs on this thread only
    /// with [`ErrorKind::Cancelled`](mln_style::error::ErrorKind), when the send
    /// fails.
    pub fn update_camera_then(
        &self,
        update: CameraUpdate,
        on_end: impl FnOnce(Result<TransitionEnd, Error>) + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        self.camera_command_then(move |map| Ok(map.update_camera(&update)), on_end)
    }

    /// Moves the camera relative to where it is, as a gesture does.
    /// Completes with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for
    /// a delta that the camera can't apply.
    pub fn apply_camera_delta(&self, delta: CameraDelta) -> Result<Completion<Applied>, Error> {
        self.camera_command(move |map| {
            map.apply_camera_delta(&delta).map_err(|error| Error::invalid_argument(error.to_string()))
        })
    }

    /// Applies `delta` as [`apply_camera_delta`](Self::apply_camera_delta)
    /// does, and calls `on_end` with how the change ended, as
    /// [`update_camera_then`](Self::update_camera_then) does.
    pub fn apply_camera_delta_then(
        &self,
        delta: CameraDelta,
        on_end: impl FnOnce(Result<TransitionEnd, Error>) + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        self.camera_command_then(
            move |map| map.apply_camera_delta(&delta).map_err(|error| Error::invalid_argument(error.to_string())),
            on_end,
        )
    }

    /// The camera that fits `area` in the map's view, after the commands
    /// sent before. It keeps the map's bearing and pitch unless `options`
    /// sets them, and its zoom stays within the map's zoom limits.
    ///
    /// Completes with [`ErrorKind::InvalidArgument`](mln_style::error::ErrorKind) for
    /// an empty area, and for padding that leaves no room for it.
    pub fn camera_for(
        &self,
        area: CameraFitArea,
        options: CameraFitOptions,
    ) -> Result<Completion<CameraOptions>, Error> {
        self.query(move |map| camera::camera_for(map, &area, &options))
    }

    /// Moves the camera to the one that [`camera_for`](Self::camera_for)
    /// finds once the commands sent before have run, as
    /// [`update_camera`](Self::update_camera) moves it.
    ///
    /// Completes as `camera_for` fails, and then leaves the camera where it
    /// is and ends the animation as cancelled.
    pub fn fit_camera(&self, area: CameraFitArea, update: CameraFitUpdate) -> Result<Completion<Applied>, Error> {
        self.camera_command(move |map| fit(map, &area, update))
    }

    /// Fits the camera as [`fit_camera`](Self::fit_camera) does, and calls
    /// `on_end` with how the change ended, as
    /// [`update_camera_then`](Self::update_camera_then) does.
    pub fn fit_camera_then(
        &self,
        area: CameraFitArea,
        update: CameraFitUpdate,
        on_end: impl FnOnce(Result<TransitionEnd, Error>) + Send + 'static,
    ) -> Result<Completion<Applied>, Error> {
        self.camera_command_then(move |map| fit(map, &area, update), on_end)
    }

    /// Stops every camera animation.
    pub fn cancel_camera_transitions(&self) -> Result<Completion<Applied>, Error> {
        self.command(|map| {
            map.cancel_transitions();
            Ok(())
        })
    }

    /// Stops the camera animations whose transition ID is `transition_id`.
    pub fn cancel_camera_transitions_with_id(&self, transition_id: u64) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            map.cancel_transitions_with_id(transition_id);
            Ok(())
        })
    }

    /// The camera after the commands sent before.
    pub fn camera(&self) -> Result<Completion<CameraQuery>, Error> {
        self.send(
            |_, _| Ok(()),
            |(), entry| CameraQuery { generation: entry.core.generation(), camera: entry.core.snapshot().view.camera },
        )
    }

    /// The area that `camera` would show, with the fields that `camera`
    /// leaves out from the map's camera. It holds every corner of a rotated
    /// view, and its longitudes can pass ±180, so a view across the
    /// antimeridian gives one continuous range.
    pub fn lat_lng_bounds_for_camera(&self, camera: CameraOptions) -> Result<Completion<LatLngBounds>, Error> {
        self.query(move |map| map.lat_lng_bounds_for_camera_unwrapped(&camera))
    }

    /// Sets the limits on the camera.
    pub fn set_bounds(&self, bounds: BoundOptions) -> Result<Completion<Applied>, Error> {
        self.command(move |map| map.set_bounds(&bounds))
    }

    /// Sets the camera's position and orientation in three dimensions.
    pub fn set_free_camera_options(&self, camera: FreeCameraOptions) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            map.set_free_camera_options(&camera);
            Ok(())
        })
    }

    /// Sets how the map orients and constrains its view.
    pub fn set_viewport_options(&self, options: ViewportOptions) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            camera::set_viewport(map, &options);
            Ok(())
        })
    }

    /// Sets how the map picks the tiles to load.
    pub fn set_tile_options(&self, options: TileOptions) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            camera::set_tile_options(map, &options);
            Ok(())
        })
    }

    pub fn set_projection_mode(&self, mode: ProjectionMode) -> Result<Completion<Applied>, Error> {
        self.command(move |map| {
            map.set_projection_mode(&mode);
            Ok(())
        })
    }

    /// Logs the style's sources and their state.
    pub fn dump_debug_logs(&self) -> Result<Completion<Applied>, Error> {
        self.command(|map| {
            map.dump_debug_logs();
            Ok(())
        })
    }
}

/// A command group of a map that ends when it drops.
#[must_use = "the group ends when it drops"]
pub struct CommandGroup {
    map: Option<Map>,
}

impl CommandGroup {
    /// Ends the group, as [`Map::end_command_group`] does.
    pub fn end(mut self) -> Result<Completion<Applied>, Error> {
        match self.map.take() {
            Some(map) => map.end_command_group(),
            None => Err(Error::invalid_state("the group ended already")),
        }
    }
}

impl Drop for CommandGroup {
    fn drop(&mut self) {
        if let Some(map) = self.map.take() {
            let _ = map.end_command_group();
        }
    }
}

enum StyleLoad {
    Url(String),
    Json(String),
}

/// A style load that hasn't completed.
struct PendingStyle {
    resolver: Resolver<Applied>,
    /// Whether the caller passed the style document, which then is the
    /// argument that fails to parse.
    from_json: bool,
}

/// The render session that a map is reserved for.
struct SessionClaim {
    id: SessionId,
    session: Weak<SessionShared>,
    /// The completion of a dispose that waits for the session to end.
    removal: Option<Resolver<()>>,
}

/// Forgets the session `id` on the runtime's thread, unless the map has
/// bound another since.
fn unbind(core: &mut RuntimeCore, map_id: MapId, id: SessionId) {
    if let Some(entry) = core.maps.get_mut(&map_id)
        && entry.session.as_ref().is_some_and(|link| link.id == id)
    {
        entry.session = None;
    }
}

/// The render session bound to a map, as the runtime's thread knows it.
pub(crate) struct SessionLink {
    id: SessionId,
    queue: SessionQueue,
    /// The session's `awaited_command`.
    awaited: Arc<AtomicU64>,
}

/// A still image on its way, whose outcome the next flush that finds it
/// reports.
struct PendingStill {
    resolver: Resolver<()>,
    /// Why the map refused the image, which the flush after the command
    /// reports.
    refused: Option<Result<(), Error>>,
}

/// A still image's error as the handle reports it: a load failure is the
/// library's, whatever the core called it.
fn still_image_error(error: Error) -> Error {
    match error.kind {
        ErrorKind::InvalidState => error,
        _ => Error::native(error.message),
    }
}

/// A map on the runtime's thread.
pub(crate) struct MapEntry {
    pub(crate) core: mln_map::Map,
    published: Arc<Published>,
    mask: EventMask,
    /// The sequence number of the last command that ran.
    applied: u64,
    /// The last command and frame sample that ran while a burst held the
    /// map's updates back, which the session learns of once they show.
    held_applied: Option<u64>,
    held_sampled: Option<u64>,
    /// The style load in progress.
    pending_style: Option<PendingStyle>,
    pending_still: Option<PendingStill>,
    /// The render session that draws the map.
    session: Option<SessionLink>,
    /// Set while the map has reported something that no flush has taken.
    flush_pending: FlushFlag,
    /// The camera commands whose change hasn't ended.
    camera_commands: HashMap<CameraCommandId, Resolver<TransitionEnd>>,
    /// The callbacks of the custom sources that commands added.
    pub(crate) callback_sources: CallbackSources,
    /// The GeoJSON data that commands set and that is still being built.
    pub(crate) geojson_updates: GeoJsonUpdates,
    /// The repaint posted for when the map changes next on its own
    /// ([`mln_map::Map::repaint_at`]).
    repaint: Option<(Instant, TimedTask)>,
}

impl MapEntry {
    pub(crate) fn applied(&self) -> Applied {
        Applied {
            sequence: self.applied,
            generation: self.core.generation(),
            render_update_generation: self.core.pending_update_generation(),
        }
    }
}

/// Whether a map reported something that no flush has taken: what its
/// reports waker sets. Reports that come before the flush they posted runs
/// share it.
#[derive(Clone, Default)]
struct FlushFlag(Arc<AtomicBool>);

impl FlushFlag {
    /// The map `id`'s reports waker: it sets the flag, and posts a flush to
    /// `queue` when the flag was clear. The posted flush does nothing once
    /// another flush took the reports.
    fn waker(&self, queue: &CoreQueue, id: MapId) -> Waker {
        let (flag, queue) = (self.clone(), queue.downgrade());
        Arc::new(move || {
            if !flag.0.swap(true, Ordering::SeqCst)
                && let Some(queue) = queue.upgrade()
            {
                let flag = flag.clone();
                post(&queue, move |core, outbox| {
                    if flag.is_set() {
                        flush(core, id, outbox);
                    }
                });
            }
        })
    }

    fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn clear(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Builds the map `id` on the runtime's thread.
pub(crate) fn create(
    core: &mut RuntimeCore,
    id: MapId,
    options: &MapOptions,
    published: Arc<Published>,
    resolver: Resolver<Applied>,
    outbox: &mut Outbox,
) {
    let registry = Arc::clone(core.registry.style());
    let mut map = match mln_map::Map::with_registry(Arc::clone(core.loader()), options.map, registry, &core.pool) {
        Ok(map) => map,
        Err(error) => return outbox.resolve(resolver, Err(error)),
    };
    if let Some(json) = &options.fallback_font_faces {
        if let Err(error) = map.set_fallback_font_faces(json) {
            return outbox.resolve(resolver, Err(error));
        }
    } else if options.system_fonts {
        map.set_fallback_font_face_list(mln_system_fonts::font_face().into_iter().collect());
    }
    let _ = published.updates.set(map.update_reader());
    let _ = published.frame_clock.set(map.frame_clock());
    let flush_pending = FlushFlag::default();
    map.set_reports_waker(Some(flush_pending.waker(&core.queue, id)));
    published.applied.store(1, Ordering::SeqCst);
    core.maps.insert(
        id,
        MapEntry {
            core: map,
            published,
            mask: options.event_mask,
            applied: 1,
            held_applied: None,
            held_sampled: None,
            pending_style: None,
            pending_still: None,
            session: None,
            flush_pending,
            camera_commands: HashMap::default(),
            callback_sources: CallbackSources::default(),
            geojson_updates: GeoJsonUpdates::default(),
            repaint: None,
        },
    );
    flush(core, id, outbox);
    if let Some(entry) = core.maps.get(&id) {
        outbox.resolve(resolver, Ok(entry.applied()));
    }
}

/// Runs a command on the map `id`, which must be there: handles what came
/// before it, applies it, publishes the map's state, sends its events, and
/// runs `after` with what `apply` returned. Then it tells the map's render
/// session that the command ran, so `after` can send the session what the
/// session must see first. While a burst holds the map's updates back, it
/// tells the session once they show, so that no frame that waits for the
/// command draws the update before it.
pub(crate) fn apply_command<T>(
    core: &mut RuntimeCore,
    id: MapId,
    sequence: u64,
    outbox: &mut Outbox,
    apply: impl FnOnce(&mut MapEntry, &mut Outbox) -> T,
    after: impl FnOnce(&MapEntry, T, Applied, &mut Outbox),
) {
    // What the runtime's calls change, a flush in the same task takes. What
    // came since then, the map reported: the report posted a flush, or is
    // queued with its waker still on its way.
    if core.maps.get(&id).is_some_and(|entry| entry.flush_pending.is_set() || entry.core.has_reports()) {
        flush(core, id, outbox);
    }
    let Some(entry) = core.maps.get_mut(&id) else { return };
    let value = apply(entry, outbox);
    entry.applied = sequence;
    flush(core, id, outbox);
    let holding = core.holding_updates();
    let Some(entry) = core.maps.get_mut(&id) else { return };
    after(entry, value, entry.applied(), outbox);
    if holding {
        entry.held_applied = Some(sequence);
    } else {
        report_applied(entry, sequence);
    }
}

/// Tells the map's session that the command `sequence` ran, once a frame can
/// show it.
fn report_applied(entry: &MapEntry, sequence: u64) {
    entry.published.applied.store(sequence, Ordering::SeqCst);
    if let Some(link) = &entry.session
        && sequence >= link.awaited.load(Ordering::SeqCst)
    {
        worker_post(&link.queue, move |worker, outbox| worker.map_applied(sequence, outbox));
    }
}

/// Tells the map's session that the map took the frame sample `sample`, once
/// a frame can show it.
fn report_sampled(entry: &MapEntry, sample: u64) {
    entry.published.sampled.fetch_max(sample, Ordering::SeqCst);
    if let Some(link) = &entry.session {
        worker_post(&link.queue, move |worker, outbox| worker.map_sampled(sample, outbox));
    }
}

/// Tells the map's session what ran while a burst held the map's updates
/// back, now that they show.
pub(crate) fn report_held(entry: &mut MapEntry) {
    if let Some(sequence) = entry.held_applied.take() {
        report_applied(entry, sequence);
    }
    if let Some(sample) = entry.held_sampled.take() {
        report_sampled(entry, sample);
    }
}

/// Has the map `id` sample its animations at its frame clock's time for the
/// frame samples that demands asked for.
fn sample_frame(core: &mut RuntimeCore, id: MapId, outbox: &mut Outbox) {
    let Some(entry) = core.maps.get(&id) else { return };
    // A demand from now on asks for another sample.
    entry.published.sample_queued.store(false, Ordering::SeqCst);
    let sample = entry.published.samples_requested.load(Ordering::SeqCst);
    if entry.flush_pending.is_set() || entry.core.has_reports() {
        flush(core, id, outbox);
    }
    let Some(entry) = core.maps.get_mut(&id) else { return };
    entry.core.sample_frame();
    flush(core, id, outbox);
    let holding = core.holding_updates();
    let Some(entry) = core.maps.get_mut(&id) else { return };
    if holding {
        entry.held_sampled = Some(sample);
    } else {
        report_sampled(entry, sample);
    }
}

/// Runs a command on the map `id` and completes with what `finish` makes of
/// its result and the map after it.
pub(crate) fn run_command<T, R: 'static>(
    core: &mut RuntimeCore,
    id: MapId,
    sequence: u64,
    apply: impl FnOnce(&mut MapEntry, &mut Outbox) -> Result<T, Error>,
    finish: impl FnOnce(T, &MapEntry) -> R,
    resolver: Resolver<R>,
    outbox: &mut Outbox,
) {
    if !core.maps.contains_key(&id) {
        return outbox.resolve(resolver, Err(missing_map()));
    }
    apply_command(core, id, sequence, outbox, apply, move |entry, result, _, outbox| {
        outbox.resolve(resolver, result.map(|value| finish(value, entry)));
    });
}

/// Starts a style load on the map `id`. Its completion waits for the load's
/// outcome, which a flush finds among the map's events.
fn load_style(
    core: &mut RuntimeCore,
    id: MapId,
    sequence: u64,
    load: StyleLoad,
    resolver: Resolver<Applied>,
    outbox: &mut Outbox,
) {
    if !core.maps.contains_key(&id) {
        return outbox.resolve(resolver, Err(missing_map()));
    }
    // The flush before the load sends the outcomes of earlier loads to their
    // own completions.
    apply_command(
        core,
        id,
        sequence,
        outbox,
        move |entry, outbox| {
            let from_json = matches!(load, StyleLoad::Json(_));
            if let Some(previous) = entry.pending_style.replace(PendingStyle { resolver, from_json }) {
                outbox.resolve(previous.resolver, Err(Error::cancelled("another style load started")));
            }
            match load {
                StyleLoad::Url(url) => entry.core.load_style_url(&url),
                StyleLoad::Json(json) => entry.core.load_style_json(&json),
            }
        },
        |_, (), _, _| {},
    );
}

/// Handles what the map's style and renderer reported, publishes its state
/// if it changed, and sends its events. A flush with nothing to handle and no
/// change to publish costs little, so callers flush whenever they may need
/// to.
pub(crate) fn flush(core: &mut RuntimeCore, id: MapId, outbox: &mut Outbox) {
    let Some(entry) = core.maps.get_mut(&id) else { return };
    // The flush takes what the map reported so far, and what it reports from
    // now on posts another.
    entry.flush_pending.clear();
    let events = entry.core.drain_events();
    let repaint_at = entry.core.repaint_at();
    if entry.repaint.as_ref().map(|(at, _)| *at) != repaint_at {
        if let Some((_, task)) = entry.repaint.take() {
            core.queue.cancel(task);
        }
        entry.repaint =
            repaint_at.map(|at| (at, post_at(&core.queue, at, move |core, outbox| repaint(core, id, outbox))));
    }
    // The sources that left the style, by a command or a style load, let go
    // of what they held before anything reports the change.
    crate::style::reconcile(entry, outbox);
    let generation = entry.core.generation();
    // The camera commands that ended report the snapshot that shows them.
    let ends: Vec<_> = entry
        .core
        .drain_command_ends()
        .filter_map(|end| Some((entry.camera_commands.remove(&end.command)?, TransitionEnd { generation, ..end.end })))
        .collect();
    // Each change to the state grows the generation, so a snapshot of the
    // same generation shows the state as it is.
    let mut published = entry.published.state.lock();
    let snapshot = match &*published {
        Some(state) if state.snapshot.generation == generation => state.snapshot,
        _ => entry.core.snapshot(),
    };
    // The renderer's ground changes the map's conversions, though not
    // always its state.
    *published = Some(PublishedState { snapshot, ground: entry.core.ground() });
    drop(published);

    let mut style_outcome = None;
    let from_json = entry.pending_style.as_ref().is_some_and(|pending| pending.from_json);
    for event in &events {
        match event {
            MapEvent::StyleLoaded => style_outcome = Some(Ok(())),
            MapEvent::LoadingFailed(StyleLoadError::Parse(message)) if from_json => {
                style_outcome = Some(Err(Error::invalid_argument(message)));
            }
            MapEvent::LoadingFailed(error) => style_outcome = Some(Err(Error::native(error.message()))),
            _ => {}
        }
    }
    let style_outcome = style_outcome.map(|outcome| (outcome, entry.pending_style.take(), entry.applied()));
    let still_outcome = match &mut entry.pending_still {
        Some(pending) => pending
            .refused
            .take()
            .or_else(|| entry.core.take_still_image().map(|result| result.map_err(still_image_error))),
        None => None,
    };
    let still = still_outcome.and_then(|outcome| Some((outcome, entry.pending_still.take()?.resolver)));
    // The session demands a frame for the image while it waits, and lets
    // the demand go once it doesn't.
    let still_image = entry.pending_still.as_ref().and(entry.core.pending_still_image()).unwrap_or(0);
    if entry.published.still_image.swap(still_image, Ordering::SeqCst) != still_image
        && let Some(link) = &entry.session
    {
        worker_post(&link.queue, Worker::still_image_changed);
    }

    let mask = entry.mask;
    let events: Vec<RuntimeEvent> = events
        .into_iter()
        .map(RuntimeEventKind::Map)
        .filter(|kind| mask.contains(kind.event_type()))
        .map(|kind| RuntimeEvent { source: EventSource::Map { id, generation }, kind })
        .collect();
    core.send_events(events, outbox);
    for (resolver, end) in ends {
        outbox.resolve(resolver, Ok(end));
    }

    if let Some((outcome, Some(pending), applied)) = style_outcome {
        outbox.resolve(pending.resolver, outcome.map(|()| applied));
    }
    if let Some((outcome, resolver)) = still {
        outbox.resolve(resolver, outcome);
    }
}

/// Draws the map `id` once its last frame's time to change came.
fn repaint(core: &mut RuntimeCore, id: MapId, outbox: &mut Outbox) {
    let Some(entry) = core.maps.get_mut(&id) else { return };
    entry.repaint = None;
    if entry.core.repaint_at().is_some() {
        entry.core.trigger_repaint();
    }
    flush(core, id, outbox);
}

/// Lets go of the map `id`, after sending the events it still had.
fn remove(core: &mut RuntimeCore, id: MapId, done: Option<Resolver<()>>, outbox: &mut Outbox) {
    flush(core, id, outbox);
    if let Some(mut entry) = core.maps.remove(&id) {
        if let Some((_, task)) = entry.repaint.take() {
            core.queue.cancel(task);
        }
        entry.core.set_waker(None);
        // The handles, which may outlive the map, keep no ground alive.
        if let Some(state) = &mut *entry.published.state.lock() {
            state.ground = None;
        }
        if let Some(pending) = entry.pending_style.take() {
            outbox.resolve(pending.resolver, Err(Error::cancelled("the map was released")));
        }
        if let Some(pending) = entry.pending_still.take() {
            outbox.resolve(pending.resolver, Err(Error::cancelled("the map was released")));
        }
        crate::style::release(&mut entry, outbox);
        // The map's camera changes end as closed as it drops, with no
        // snapshot to show them.
        entry.core.close_camera();
        for end in entry.core.drain_command_ends() {
            if let Some(resolver) = entry.camera_commands.remove(&end.command) {
                outbox.resolve(resolver, Ok(end.end));
            }
        }
    }
    if let Some(done) = done {
        outbox.resolve(done, Ok(()));
    }
}
