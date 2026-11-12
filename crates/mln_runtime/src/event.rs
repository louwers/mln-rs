//! What a runtime and its maps report to the runtime's event handler.

use std::ops::{BitOr, BitOrAssign};
use std::sync::Arc;

use mln_map::MapEvent;
use mln_storage::ResourceError;
use mln_storage::offline::OfflineRegionStatus;

use crate::map::MapId;

/// The runtime's event handler. It runs on the runtime's thread with each
/// batch of events, oldest first.
pub type EventHandler = Arc<dyn Fn(&[RuntimeEvent]) + Send + Sync>;

/// What raised an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventSource {
    Runtime,
    /// A map: a snapshot of it with `generation` or a later one includes
    /// what the event reports.
    Map {
        id: MapId,
        generation: u64,
    },
}

impl EventSource {
    /// The map that raised the event.
    #[must_use]
    pub fn map_id(&self) -> Option<MapId> {
        match self {
            EventSource::Runtime => None,
            EventSource::Map { id, .. } => Some(*id),
        }
    }
}

/// An event of a runtime or of one of its maps.
#[derive(Clone, Debug)]
pub struct RuntimeEvent {
    pub source: EventSource,
    pub kind: RuntimeEventKind,
}

/// What an event reports.
#[derive(Clone, Debug)]
pub enum RuntimeEventKind {
    /// An event of a map.
    Map(MapEvent),
    /// An observed offline region's download made progress or changed state.
    OfflineRegionStatusChanged { region_id: i64, status: OfflineRegionStatus },
    /// A resource that an observed offline region needs failed to download.
    OfflineRegionResponseError { region_id: i64, error: ResourceError },
    /// An observed region's download stopped because the offline regions hold
    /// as many tiles of MapLibre's demo tile server as `limit` allows.
    OfflineRegionTileCountLimitExceeded { region_id: i64, limit: u64 },
}

impl RuntimeEventKind {
    #[must_use]
    pub fn event_type(&self) -> EventType {
        match self {
            RuntimeEventKind::Map(event) => map_event_type(event),
            RuntimeEventKind::OfflineRegionStatusChanged { .. } => EventType::OfflineRegionStatusChanged,
            RuntimeEventKind::OfflineRegionResponseError { .. } => EventType::OfflineRegionResponseError,
            RuntimeEventKind::OfflineRegionTileCountLimitExceeded { .. } => {
                EventType::OfflineRegionTileCountLimitExceeded
            }
        }
    }
}

/// The type of an event. The values are maplibre-native-ffi's
/// `mln_runtime_event_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u32)]
pub enum EventType {
    MapCameraWillChange = 1,
    MapCameraIsChanging = 2,
    MapCameraDidChange = 3,
    MapStyleLoaded = 4,
    MapLoadingStarted = 5,
    MapLoadingFinished = 6,
    MapLoadingFailed = 7,
    MapIdle = 8,
    MapRenderError = 9,
    MapRenderFrameStarted = 10,
    MapRenderFrameFinished = 11,
    MapRenderMapStarted = 12,
    MapRenderMapFinished = 13,
    MapStyleImageMissing = 14,
    MapTileAction = 15,
    MapSourceChanged = 16,
    OfflineRegionStatusChanged = 17,
    OfflineRegionResponseError = 18,
    OfflineRegionTileCountLimitExceeded = 19,
    MapGlyphsRequested = 20,
    MapGlyphsLoaded = 21,
    MapGlyphsError = 22,
    MapSpriteRequested = 23,
    MapSpriteLoaded = 24,
    MapSpriteError = 25,
}

fn map_event_type(event: &MapEvent) -> EventType {
    match event {
        MapEvent::CameraWillChange(_) => EventType::MapCameraWillChange,
        MapEvent::CameraIsChanging => EventType::MapCameraIsChanging,
        MapEvent::CameraDidChange(_) => EventType::MapCameraDidChange,
        MapEvent::StyleLoaded => EventType::MapStyleLoaded,
        MapEvent::LoadingStarted => EventType::MapLoadingStarted,
        MapEvent::LoadingFinished => EventType::MapLoadingFinished,
        MapEvent::LoadingFailed(_) => EventType::MapLoadingFailed,
        MapEvent::Idle => EventType::MapIdle,
        MapEvent::RenderError(_) => EventType::MapRenderError,
        MapEvent::RenderFrameStarted => EventType::MapRenderFrameStarted,
        MapEvent::RenderFrameFinished(_) => EventType::MapRenderFrameFinished,
        MapEvent::RenderMapStarted => EventType::MapRenderMapStarted,
        MapEvent::RenderMapFinished(_) => EventType::MapRenderMapFinished,
        MapEvent::StyleImageMissing { .. } => EventType::MapStyleImageMissing,
        MapEvent::TileAction { .. } => EventType::MapTileAction,
        MapEvent::SourceChanged { .. } => EventType::MapSourceChanged,
        MapEvent::GlyphsRequested { .. } => EventType::MapGlyphsRequested,
        MapEvent::GlyphsLoaded { .. } => EventType::MapGlyphsLoaded,
        MapEvent::GlyphsError { .. } => EventType::MapGlyphsError,
        MapEvent::SpriteRequested(_) => EventType::MapSpriteRequested,
        MapEvent::SpriteLoaded(_) => EventType::MapSpriteLoaded,
        MapEvent::SpriteError(..) => EventType::MapSpriteError,
    }
}

/// A set of event types. The bits are maplibre-native-ffi's
/// `mln_runtime_event_flag`: `1 << type` for each type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EventMask(u64);

const fn bit(event_type: EventType) -> u64 {
    1 << event_type as u32
}

impl EventMask {
    pub const NONE: EventMask = EventMask(0);
    /// Every event that a map raises.
    pub const ALL_MAP_EVENTS: EventMask = EventMask(
        bit(EventType::MapCameraWillChange)
            | bit(EventType::MapCameraIsChanging)
            | bit(EventType::MapCameraDidChange)
            | bit(EventType::MapStyleLoaded)
            | bit(EventType::MapLoadingStarted)
            | bit(EventType::MapLoadingFinished)
            | bit(EventType::MapLoadingFailed)
            | bit(EventType::MapIdle)
            | bit(EventType::MapRenderError)
            | bit(EventType::MapRenderFrameStarted)
            | bit(EventType::MapRenderFrameFinished)
            | bit(EventType::MapRenderMapStarted)
            | bit(EventType::MapRenderMapFinished)
            | bit(EventType::MapStyleImageMissing)
            | bit(EventType::MapTileAction)
            | bit(EventType::MapSourceChanged)
            | bit(EventType::MapGlyphsRequested)
            | bit(EventType::MapGlyphsLoaded)
            | bit(EventType::MapGlyphsError)
            | bit(EventType::MapSpriteRequested)
            | bit(EventType::MapSpriteLoaded)
            | bit(EventType::MapSpriteError),
    );
    /// Every event that a runtime raises: those of its offline regions.
    pub const ALL_RUNTIME_EVENTS: EventMask = EventMask(
        bit(EventType::OfflineRegionStatusChanged)
            | bit(EventType::OfflineRegionResponseError)
            | bit(EventType::OfflineRegionTileCountLimitExceeded),
    );
    pub const ALL: EventMask = EventMask(Self::ALL_MAP_EVENTS.0 | Self::ALL_RUNTIME_EVENTS.0);

    /// The mask with these bits, or `None` when a bit names no event type.
    #[must_use]
    pub const fn from_bits(bits: u64) -> Option<EventMask> {
        if bits & !Self::ALL.0 == 0 { Some(EventMask(bits)) } else { None }
    }

    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, event_type: EventType) -> bool {
        self.0 & bit(event_type) != 0
    }

    /// The mask with `event_type` added.
    #[must_use]
    pub const fn with(self, event_type: EventType) -> EventMask {
        EventMask(self.0 | bit(event_type))
    }

    /// The mask with `event_type` removed.
    #[must_use]
    pub const fn without(self, event_type: EventType) -> EventMask {
        EventMask(self.0 & !bit(event_type))
    }
}

impl Default for EventMask {
    /// Every event.
    fn default() -> Self {
        EventMask::ALL
    }
}

impl From<EventType> for EventMask {
    fn from(event_type: EventType) -> Self {
        EventMask(bit(event_type))
    }
}

impl BitOr for EventMask {
    type Output = EventMask;

    fn bitor(self, other: EventMask) -> EventMask {
        EventMask(self.0 | other.0)
    }
}

impl BitOr<EventType> for EventMask {
    type Output = EventMask;

    fn bitor(self, other: EventType) -> EventMask {
        self.with(other)
    }
}

impl BitOrAssign for EventMask {
    fn bitor_assign(&mut self, other: EventMask) {
        self.0 |= other.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_masks_match_ffis_flags() {
        assert_eq!(1 << 1, EventMask::from(EventType::MapCameraWillChange).bits());
        assert_eq!(0x03FF_FFFE, EventMask::ALL.bits());
        assert_eq!(0b111 << 17, EventMask::ALL_RUNTIME_EVENTS.bits());
        assert_eq!(None, EventMask::from_bits(1));
        assert_eq!(None, EventMask::from_bits(1 << 26));
        assert_eq!(Some(EventMask::ALL), EventMask::from_bits(EventMask::ALL.bits()));
    }

    #[test]
    fn a_mask_adds_and_removes_types() {
        let mask = EventMask::NONE | EventType::MapIdle;
        assert!(mask.contains(EventType::MapIdle));
        assert!(!mask.contains(EventType::MapStyleLoaded));
        assert_eq!(EventMask::NONE, mask.without(EventType::MapIdle));
    }
}
