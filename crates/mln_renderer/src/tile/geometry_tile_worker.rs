// The in-process transport makes the worker a pool actor
// (mln_actor::actor::Actor) on the map's thread pool, as in C++. Its parent
// GeometryTile lives on the render thread and is reached through a mailbox
// (`ActorRef<GeometryTileInner>`). When the source asks for synchronous
// updates, C++ uses an `OptionalActor` that calls the worker and the tile
// directly; the worker's `parent` is then the tile itself and `SelfRef`
// is `None`.
//
// C++ wraps each message handler in `try { ... } catch (...)` and reports
// the exception to the tile (`GeometryTile::onError`). The port returns the
// exceptions as `Err` (malformed tiles, a too long geometry) and reports
// those; a panic is a bug and is not caught (`catch_unwind` can't catch it
// with `panic = "abort"` or on wasm).
//
// Port-only: the messages between the tile and its worker are data
// (`WorkerMessage`, `TileMessage`) instead of member function calls bound
// into closures, and they travel through a `TileWorkerTransport`
// (tile_worker_transport.rs), so that a worker can also run in another
// wasm instance.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mln_actor::actor::PoolActorRef;
use mln_map_types::MapMode;
use mln_style::expression::GlobalStateMap;
use mln_style::layer_properties::LayerProperties;
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use crate::layout::tile_layout::{Dependencies, Discovered, LayoutInput, LayoutParams, Resolved, build, discover};
use crate::tile::geometry_tile::LayoutResult;
use crate::tile::tile_worker_transport::TileEndpoint;
use crate::tile::worker_codec::fields;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Coalescing,
    NeedsParse,
    NeedsSymbolLayout,
}

/// The messages a `GeometryTile` sends its `GeometryTileWorker`: the
/// worker's public member functions. Port-only: the codec writes them for
/// a worker in another wasm instance (`tile/worker_codec`).
#[derive(mln_wire::Wire)]
pub enum WorkerMessage {
    SetData {
        data: Option<TileData>,
        available_images: Arc<VecSet<String>>,
        correlation_id: u64,
    },
    /// Sets the layers, with the pixel ratio to lay them out for.
    SetLayers {
        #[wire(with = "fields::layers")]
        layers: Vec<Arc<dyn LayerProperties>>,
        global_state: Option<Arc<GlobalStateMap>>,
        available_images: Arc<VecSet<String>>,
        subdivision_granularity: SubdivisionGranularitySetting,
        pixel_ratio: f32,
        correlation_id: u64,
    },
    /// Parses again with the images added since the last parse.
    SetAvailableImages {
        available_images: Arc<VecSet<String>>,
        correlation_id: u64,
    },
    Reset {
        correlation_id: u64,
    },
    SetShowCollisionBoxes {
        show_collision_boxes: bool,
        correlation_id: u64,
    },
    /// The answer to the worker's `GetDependencies` with id `request`.
    Resolved {
        resolved: Resolved,
        request: u64,
    },
}

impl WorkerMessage {
    /// The message's correlation id, or 0 for the answer to a dependency
    /// request, which has none.
    #[must_use]
    pub(crate) fn correlation_id(&self) -> u64 {
        match self {
            WorkerMessage::SetData { correlation_id, .. }
            | WorkerMessage::SetLayers { correlation_id, .. }
            | WorkerMessage::SetAvailableImages { correlation_id, .. }
            | WorkerMessage::Reset { correlation_id }
            | WorkerMessage::SetShowCollisionBoxes { correlation_id, .. } => *correlation_id,
            WorkerMessage::Resolved { .. } => 0,
        }
    }
}

/// The messages a `GeometryTileWorker` sends its `GeometryTile`: the tile's
/// member functions the worker calls. Port-only: the codec writes them for
/// a worker in another wasm instance (`tile/worker_codec`).
#[derive(mln_wire::Wire)]
pub enum TileMessage {
    /// A layout. `request` is the id of the worker's last `GetDependencies`
    /// (0 before its first), and `resolved` whether the layout used its
    /// answer: the tile frees the atlas bins of the answers no layout can
    /// use any more.
    Layout {
        result: LayoutResult,
        correlation_id: u64,
        request: u64,
        resolved: bool,
    },
    Error {
        error: Error,
        correlation_id: u64,
    },
    /// The glyphs and images the layout of the worker's latest parse needs,
    /// which the tile answers with `WorkerMessage::Resolved`. Requests are
    /// numbered from 1.
    GetDependencies {
        dependencies: Dependencies,
        request: u64,
    },
}

/// The worker's reference to itself: `None` for the synchronous worker, whose
/// self-sent messages are direct calls.
pub(crate) type SelfRef = Option<PoolActorRef<GeometryTileWorker>>;

/// The tile data a `GeometryTileWorker` received.
enum ReceivedData {
    /// No `set_data` since the worker was created or reset.
    Pending,
    /// The tile's data; `None` when the tile has no data.
    Received(Option<TileData>),
}

pub(crate) struct GeometryTileWorker {
    self_ref: SelfRef,
    parent: Box<dyn TileEndpoint>,

    id: OverscaledTileID,
    obsolete: Arc<AtomicBool>,
    mode: MapMode,
    pixel_ratio: f32,
    capture_rendered_features: bool,

    /// The last parse's result, until its layout is built.
    discovered: Option<Discovered>,

    state: State,
    correlation_id: u64,
    /// The id of the last `GetDependencies`.
    request: u64,
    /// Whether the last parse's layout waits for the answer to `request`.
    awaiting: bool,
    /// The answer to `request`, while `awaiting`.
    resolved: Option<Resolved>,

    // `None` until the worker has received the layers.
    layers: Option<Vec<Arc<dyn LayerProperties>>>,
    data: ReceivedData,

    // The style's global state, used by "global-state" expressions during
    // layout.
    global_state: Option<Arc<GlobalStateMap>>,

    font_faces: Option<Arc<mln_text::glyph::FontFaces>>,
    available_images: Arc<VecSet<String>>,

    show_collision_boxes: bool,
    subdivision_granularity: SubdivisionGranularitySetting,
    first_load: bool,
}

impl GeometryTileWorker {
    #[expect(clippy::too_many_arguments, reason = "the worker's tile, parent and render mode, which it keeps")]
    pub(crate) fn new(
        self_ref: SelfRef,
        parent: Box<dyn TileEndpoint>,
        id: OverscaledTileID,
        obsolete: Arc<AtomicBool>,
        mode: MapMode,
        pixel_ratio: f32,
        show_collision_boxes: bool,
        font_faces: Option<Arc<mln_text::glyph::FontFaces>>,
        capture_rendered_features: bool,
    ) -> Self {
        GeometryTileWorker {
            self_ref,
            parent,
            id,
            obsolete,
            mode,
            pixel_ratio,
            capture_rendered_features,
            discovered: None,
            state: State::Idle,
            correlation_id: 0,
            request: 0,
            awaiting: false,
            resolved: None,
            layers: None,
            data: ReceivedData::Pending,
            global_state: None,
            font_faces,
            available_images: Arc::new(VecSet::new()),
            show_collision_boxes,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            first_load: true,
        }
    }

    /// What a layout reads: `None` until the worker has the data and the
    /// layers.
    fn layout_input(&self) -> Option<LayoutInput<'_>> {
        let (ReceivedData::Received(data), Some(layers)) = (&self.data, &self.layers) else {
            return None;
        };
        Some(LayoutInput {
            id: self.id,
            data: data.as_ref(),
            layers,
            global_state: self.global_state.as_ref(),
            font_faces: self.font_faces.as_ref(),
            available_images: &self.available_images,
            params: LayoutParams {
                mode: self.mode,
                pixel_ratio: self.pixel_ratio,
                show_collision_boxes: self.show_collision_boxes,
                subdivision_granularity: self.subdivision_granularity,
                first_load: self.first_load,
                capture_rendered_features: self.capture_rendered_features,
            },
        })
    }

    /// Runs the member function a message stands for.
    pub(crate) fn handle(&mut self, message: WorkerMessage) {
        match message {
            WorkerMessage::SetData { data, available_images, correlation_id } => {
                self.set_data(data, available_images, correlation_id);
            }
            WorkerMessage::SetLayers {
                layers,
                global_state,
                available_images,
                subdivision_granularity,
                pixel_ratio,
                correlation_id,
            } => {
                self.pixel_ratio = pixel_ratio;
                self.set_layers(layers, global_state, available_images, subdivision_granularity, correlation_id);
            }
            WorkerMessage::SetAvailableImages { available_images, correlation_id } => {
                self.set_available_images(available_images, correlation_id);
            }
            WorkerMessage::Reset { correlation_id } => self.reset(correlation_id),
            WorkerMessage::SetShowCollisionBoxes { show_collision_boxes, correlation_id } => {
                self.set_show_collision_boxes(show_collision_boxes, correlation_id);
            }
            WorkerMessage::Resolved { resolved, request } => self.on_resolved(resolved, request),
        }
    }

    /// Runs `f` as the C++ `try { f } catch (...) { parent.invoke(
    /// &GeometryTile::onError, std::current_exception(), correlationID); }`.
    fn guarded(&mut self, f: impl FnOnce(&mut Self) -> Result<(), Error>) {
        let Err(error) = f(self) else {
            return;
        };
        self.parent.send(TileMessage::Error { error, correlation_id: self.correlation_id });
    }

    /*
       GeometryTileWorker is a state machine. This is its transition diagram.
       States are indicated by [state], lines are transitions triggered by
       messages, (parentheses) are actions taken on transition.

                             [Idle] <-------------------------.
                                |                             |
            set{Data,Layers}, symbolDependenciesChanged,      |
                        setShowCollisionBoxes                 |
                                |                             |
       (do parse and/or symbol layout; self-send "coalesced") |
                                v                             |
                          [Coalescing] --- coalesced ---------.
                             |   |
                 .-----------.   .---------------------.
                 |                                     |
       .--- set{Data,Layers}                setShowCollisionBoxes,
       |         |                         symbolDependenciesChanged --.
       |         |                                     |               |
       |         v                                     v               |
       .-- [NeedsParse] <-- set{Data,Layers} -- [NeedsSymbolLayout] ---.
                 |                                     |
             coalesced                             coalesced
                 |                                     |
                 v                                     v
       (do parse or symbol layout; self-send "coalesced"; goto [coalescing])

       See the C++ file for the full description.
    */

    pub(crate) fn set_data(
        &mut self,
        data: Option<TileData>,
        available_images: Arc<VecSet<String>>,
        correlation_id: u64,
    ) {
        self.guarded(|this| {
            this.data = ReceivedData::Received(data);
            this.correlation_id = correlation_id;
            this.available_images = available_images;

            match this.state {
                State::Idle => {
                    this.parse()?;
                    this.coalesce();
                }
                State::Coalescing | State::NeedsParse | State::NeedsSymbolLayout => {
                    this.state = State::NeedsParse;
                }
            }
            Ok(())
        });
    }

    pub(crate) fn set_layers(
        &mut self,
        layers: Vec<Arc<dyn LayerProperties>>,
        global_state: Option<Arc<GlobalStateMap>>,
        available_images: Arc<VecSet<String>>,
        subdivision_granularity: SubdivisionGranularitySetting,
        correlation_id: u64,
    ) {
        self.guarded(|this| {
            this.layers = Some(layers);
            this.global_state = global_state;
            this.correlation_id = correlation_id;
            this.available_images = available_images;
            this.subdivision_granularity = subdivision_granularity;

            match this.state {
                State::Idle => {
                    this.parse()?;
                    this.coalesce();
                }
                State::Coalescing | State::NeedsSymbolLayout => {
                    this.state = State::NeedsParse;
                }
                State::NeedsParse => {}
            }
            Ok(())
        });
    }

    pub(crate) fn set_available_images(&mut self, available_images: Arc<VecSet<String>>, correlation_id: u64) {
        self.guarded(|this| {
            this.available_images = available_images;
            this.correlation_id = correlation_id;

            match this.state {
                State::Idle => {
                    this.parse()?;
                    this.coalesce();
                }
                State::Coalescing | State::NeedsSymbolLayout => {
                    this.state = State::NeedsParse;
                }
                State::NeedsParse => {}
            }
            Ok(())
        });
    }

    pub(crate) fn reset(&mut self, correlation_id: u64) {
        self.layers = None;
        self.data = ReceivedData::Pending;
        self.discovered = None;
        self.awaiting = false;
        self.resolved = None;
        self.correlation_id = correlation_id;

        match self.state {
            State::Idle | State::NeedsParse => {}
            State::Coalescing | State::NeedsSymbolLayout => {
                self.state = State::NeedsParse;
            }
        }
    }

    pub(crate) fn set_show_collision_boxes(&mut self, show_collision_boxes: bool, correlation_id: u64) {
        self.guarded(|this| {
            this.show_collision_boxes = show_collision_boxes;
            this.correlation_id = correlation_id;

            match this.state {
                State::Idle => {
                    if !this.has_pending_parse_result() {
                        // Trigger parse if nothing is in flight, otherwise
                        // symbol layout will automatically pick up the change
                        this.parse()?;
                        this.coalesce();
                    }
                }
                State::Coalescing => {
                    this.state = State::NeedsSymbolLayout;
                }
                State::NeedsSymbolLayout | State::NeedsParse => {}
            }
            Ok(())
        });
    }

    fn symbol_dependencies_changed(&mut self) {
        self.guarded(|this| {
            match this.state {
                State::Idle => {
                    if this.has_pending_parse_result() {
                        this.finalize_layout()?;
                        this.coalesce();
                    }
                }
                State::Coalescing => {
                    if this.has_pending_parse_result() {
                        this.state = State::NeedsSymbolLayout;
                    }
                }
                State::NeedsSymbolLayout | State::NeedsParse => {}
            }
            Ok(())
        });
    }

    fn coalesced(&mut self) {
        self.guarded(|this| {
            match this.state {
                State::Idle => {
                    debug_assert!(false);
                }
                State::Coalescing => {
                    this.state = State::Idle;
                }
                State::NeedsParse => {
                    this.parse()?;
                    this.coalesce();
                }
                State::NeedsSymbolLayout => {
                    // We may have entered NeedsSymbolLayout while coalescing
                    // after a performLayout. In that case, we need to start
                    // over with parsing in order to do another layout.
                    if this.has_pending_parse_result() {
                        this.finalize_layout()?;
                    } else {
                        this.parse()?;
                    }
                    this.coalesce();
                }
            }
            Ok(())
        });
    }

    /// Enters the coalescing state and sends itself `coalesced`; the
    /// synchronous worker calls it directly.
    fn coalesce(&mut self) {
        self.state = State::Coalescing;
        match &self.self_ref {
            Some(self_ref) => self_ref.invoke(|worker: &mut GeometryTileWorker| worker.coalesced()),
            // OptionalActorRef with an object: a direct call.
            None => self.coalesced(),
        }
    }

    /// Takes the answer to the dependency request `request`, unless the
    /// worker parsed again since.
    pub(crate) fn on_resolved(&mut self, resolved: Resolved, request: u64) {
        if !self.awaiting || request != self.request {
            return;
        }
        self.resolved = Some(resolved);
        self.symbol_dependencies_changed();
    }

    fn parse(&mut self) -> Result<(), Error> {
        self.discovered = None;
        self.awaiting = false;
        self.resolved = None;
        let Some(input) = self.layout_input() else {
            return Ok(());
        };
        let Some(mut discovered) = discover(&input, &self.obsolete)? else {
            return Ok(());
        };
        // A layout that waits for nothing still asks for the images it found
        // unavailable, so that the tile lays it out again when one comes.
        if discovered.has_waiting_layouts() || !discovered.dependencies.is_empty() {
            self.request += 1;
            self.awaiting = true;
            let dependencies = std::mem::take(&mut discovered.dependencies);
            self.parent.send(TileMessage::GetDependencies { dependencies, request: self.request });
        }
        self.discovered = Some(discovered);

        self.finalize_layout()
    }

    fn has_pending_parse_result(&self) -> bool {
        self.discovered.is_some()
    }

    fn finalize_layout(&mut self) -> Result<(), Error> {
        if self.awaiting && self.resolved.is_none() {
            return Ok(());
        }
        let Some(discovered) = self.discovered.take() else {
            return Ok(());
        };
        let resolved = self.resolved.take().unwrap_or_default();
        let used = std::mem::take(&mut self.awaiting);

        // `discovered` is only set while the worker has its data and layers.
        let Some(input) = self.layout_input() else {
            return Ok(());
        };
        let Some(built) = build(discovered, &input, &resolved, &self.obsolete)? else {
            return Ok(());
        };

        self.first_load = false;

        let result = LayoutResult::new(built.render_data, Arc::new(built.feature_index));
        self.parent.send(TileMessage::Layout {
            result,
            correlation_id: self.correlation_id,
            request: self.request,
            resolved: used,
        });
        Ok(())
    }
}
