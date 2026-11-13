// Events, snapshots and style loads. They follow maplibre-native-ffi's ABI
// tests (tests/native/abi/runtime/events.c and event_generations.c at
// eae96a42c) where those test the same rules.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use httpmock::MockServer;
use mln_map::MapEvent;
use mln_runtime::{ErrorKind, EventMask, EventSource, EventType, MapOptions, RuntimeEvent, RuntimeEventKind};
use support::{EMPTY_STYLE, EventLog, map, map_options, runtime, wait, wait_err};

fn is_map_event(event: &RuntimeEvent, event_type: EventType) -> bool {
    event.kind.event_type() == event_type
}

#[test]
fn a_command_sends_its_events_before_it_completes() {
    let runtime = runtime();
    let map = map(&runtime);
    let order = Arc::new(Mutex::new(Vec::new()));
    let events = order.clone();
    wait(runtime.set_event_handler(move |batch| {
        events.lock().unwrap().extend(batch.iter().map(|event| format!("{:?}", event.kind.event_type())));
    }));
    let done = order.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    map.set_style_json(EMPTY_STYLE).unwrap().on_complete(move |result| {
        done.lock().unwrap().push("completed".to_owned());
        tx.send(result).unwrap();
    });
    rx.recv_timeout(support::TIMEOUT).unwrap().unwrap();
    let order = order.lock().unwrap();
    let position = |name: &str| order.iter().position(|entry| entry == name);
    assert!(position("MapLoadingStarted") < position("MapStyleLoaded"), "{order:?}");
    assert!(position("MapStyleLoaded") < position("completed"), "{order:?}");
}

#[test]
fn an_event_comes_after_a_snapshot_that_includes_it() {
    let runtime = runtime();
    let map = map(&runtime);
    let checked = Arc::new(Mutex::new(Vec::new()));
    let (record, inner) = (checked.clone(), map.clone());
    wait(runtime.set_event_handler(move |batch| {
        let snapshot = inner.snapshot().unwrap();
        for event in batch {
            if let EventSource::Map { generation, .. } = event.source {
                record.lock().unwrap().push((generation, snapshot.generation));
            }
        }
    }));
    let applied = wait(map.set_style_json(EMPTY_STYLE));
    wait(map.update_camera(mln_map::CameraUpdate {
        camera: mln_map::CameraOptions { zoom: Some(2.0), ..Default::default() },
        ..mln_map::CameraUpdate::default()
    }));
    let checked = checked.lock().unwrap();
    assert!(!checked.is_empty());
    assert!(checked.iter().all(|(event, snapshot)| event <= snapshot), "{checked:?}");
    assert!(checked.windows(2).all(|pair| pair[0].0 <= pair[1].0), "{checked:?}");
    assert!(checked.iter().any(|(event, _)| *event <= applied.generation));
    drop(checked);
    wait(runtime.clear_event_handler());
}

#[test]
fn every_event_names_its_map() {
    let runtime = runtime();
    let log = EventLog::install(&runtime);
    let (first, second) = (map(&runtime), map(&runtime));
    wait(first.set_style_json(EMPTY_STYLE));
    wait(second.set_style_json(EMPTY_STYLE));
    let loaded: Vec<_> = log
        .events()
        .into_iter()
        .filter(|event| is_map_event(event, EventType::MapStyleLoaded))
        .map(|event| event.source.map_id())
        .collect();
    assert_eq!(vec![Some(first.id()), Some(second.id())], loaded);
}

#[test]
fn the_event_mask_selects_the_events() {
    let runtime = runtime();
    let log = EventLog::install(&runtime);
    let (map, created) =
        runtime.create_map(MapOptions { event_mask: EventType::MapStyleLoaded.into(), ..map_options() }).unwrap();
    created.wait().unwrap();
    assert_eq!(EventMask::from(EventType::MapStyleLoaded), map.event_mask());
    wait(map.set_style_json(EMPTY_STYLE));
    wait(map.set_event_mask(EventMask::ALL.without(EventType::MapStyleLoaded)));
    assert_eq!(EventMask::ALL.without(EventType::MapStyleLoaded), map.event_mask());
    wait(map.set_style_json(EMPTY_STYLE));
    let types: Vec<_> = log.events().iter().map(|event| event.kind.event_type()).collect();
    assert_eq!(1, types.iter().filter(|t| **t == EventType::MapStyleLoaded).count(), "{types:?}");
    assert!(types.contains(&EventType::MapLoadingStarted), "{types:?}");
    assert!(types[0] == EventType::MapStyleLoaded, "{types:?}");
}

#[test]
fn a_cleared_handler_hears_nothing() {
    let runtime = runtime();
    let log = EventLog::install(&runtime);
    wait(runtime.clear_event_handler());
    let map = map(&runtime);
    wait(map.set_style_json(EMPTY_STYLE));
    assert!(log.events().is_empty());
}

#[test]
fn a_style_that_fails_to_parse_fails_its_load_and_keeps_the_style() {
    let runtime = runtime();
    let log = EventLog::install(&runtime);
    let map = map(&runtime);
    let layer = r#"{"version":8,"sources":{},"layers":[{"id":"bg","type":"background"}]}"#;
    wait(map.set_style_json(layer));
    let error = wait_err(map.set_style_json("{"));
    assert_eq!(ErrorKind::InvalidArgument, error.kind);
    assert!(log.wait_for(|event| matches!(event.kind, RuntimeEventKind::Map(MapEvent::LoadingFailed(_)))).is_some());
    assert!(wait(map.style_layer_json("bg")).is_some());
    assert_eq!(layer, wait(map.loaded_style_json()));
    assert_eq!(ErrorKind::InvalidArgument, map.set_style_json("").unwrap_err().kind);
}

#[test]
fn a_style_url_loads_and_names_the_style() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("GET").path("/style.json");
        then.status(200).body(EMPTY_STYLE);
    });
    server.mock(|when, then| {
        when.method("GET").path("/broken.json");
        then.status(200).body("{");
    });
    let runtime = runtime();
    let map = map(&runtime);
    let url = server.url("/style.json");
    wait(map.set_style_url(&url));
    assert_eq!(Some(url), wait(map.style_url()));
    assert_eq!(EMPTY_STYLE, wait(map.loaded_style_json()));

    wait(map.set_style_json(EMPTY_STYLE));
    assert_eq!(None, wait(map.style_url()));
    assert_eq!(ErrorKind::Native, wait_err(map.set_style_url(server.url("/missing.json"))).kind);
    // The URL is the argument, which a document that fails to parse doesn't
    // make invalid.
    assert_eq!(ErrorKind::Native, wait_err(map.set_style_url(server.url("/broken.json"))).kind);
}

#[test]
fn a_new_style_load_cancels_the_one_in_progress() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method("GET").path("/slow.json");
        then.status(200).delay(Duration::from_millis(300)).body(EMPTY_STYLE);
    });
    let runtime = runtime();
    let map = map(&runtime);
    let slow = map.set_style_url(server.url("/slow.json")).unwrap();
    let fast = map.set_style_json(EMPTY_STYLE).unwrap();
    assert_eq!(ErrorKind::Cancelled, slow.wait().unwrap_err().kind);
    fast.wait().unwrap();

    let pending = map.set_style_url(server.url("/slow.json")).unwrap();
    wait(map.release());
    assert_eq!(ErrorKind::Cancelled, pending.wait().unwrap_err().kind);
}

#[test]
fn style_commands_report_ffis_error_kinds() {
    let runtime = runtime();
    let map = map(&runtime);
    let source = r#"{"type":"geojson","data":{"type":"FeatureCollection","features":[]}}"#;
    assert_eq!(ErrorKind::InvalidState, wait_err(map.add_style_source_json("points", source)).kind);
    assert_eq!(ErrorKind::InvalidState, wait_err(map.remove_style_layer("none")).kind);

    wait(map.set_style_json(EMPTY_STYLE));
    wait(map.add_style_source_json("points", source));
    assert_eq!(ErrorKind::AlreadyExists, wait_err(map.add_style_source_json("points", source)).kind);
    wait(map.add_style_layer_json(r#"{"id":"dots","type":"circle","source":"points"}"#, None));
    wait(map.add_style_layer_json(r#"{"id":"bg","type":"background"}"#, Some("dots".to_owned())));
    assert_eq!(ErrorKind::InvalidState, wait_err(map.remove_style_source("points")).kind);
    assert_eq!(ErrorKind::NotFound, wait_err(map.remove_style_source("lines")).kind);

    wait(map.set_style_layer_property("dots", "circle-radius", "4.5"));
    assert_eq!(Some("4.5".to_owned()), wait(map.style_layer_property("dots", "circle-radius")));
    wait(map.set_style_light_property("intensity", "0.25"));
    assert_eq!(Some("0.25".to_owned()), wait(map.style_light_property("intensity")));

    wait(map.remove_style_layer("dots"));
    assert_eq!(None, wait(map.style_layer_json("dots")));
    assert_eq!(ErrorKind::NotFound, wait_err(map.remove_style_layer("dots")).kind);
    wait(map.remove_style_source("points"));
}
