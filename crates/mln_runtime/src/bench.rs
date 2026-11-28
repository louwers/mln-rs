//! Entry points to the runtime's internal stages, as functions over data,
//! for the benchmarks (`tools/bench`) to measure, and the settings that
//! make a runtime do the same work on every run. Built only with the
//! `bench` feature, and hidden from the documentation: they aren't API.

use std::sync::Arc;

use geojson::GeoJson;
use mln_engine::Registry;
use mln_engine::exec::pool::Pool;
use mln_engine::storage::FileSource;
use mln_gfx::rendering_stats::RenderingStats;
use mln_style::error::Error;
use mln_style::time::Instant;

use crate::{Completion, Map, RenderSession, Runtime, RuntimeOptions};

/// Parses GeoJSON text as [`Map::set_geojson_source_data`](crate::Map::set_geojson_source_data)
/// does, before the source builds its data from it.
pub fn parse_geojson(json: &str) -> Result<GeoJson, Error> {
    crate::style::parse_geojson(json)
}

/// Starts a runtime as [`Runtime::with_registry`] does, whose maps load
/// through `file_source` instead of the runtime's own loader, and whose
/// background work, its maps' and their render sessions', runs on `pool`,
/// such as a pool without threads that the benchmark runs when it waits.
pub fn runtime(registry: Arc<Registry>, pool: Pool, file_source: Arc<dyn FileSource>) -> Result<Runtime, Error> {
    Runtime::with_parts(&RuntimeOptions::default(), registry, pool, Some(file_source))
}

/// Holds `map`'s frame clock at `time`: the frames demanded from now on
/// sample the map's animations and fades at it, rather than at the time
/// they were demanded.
pub fn hold_frame_clock(map: &Map, time: Instant) {
    map.hold_frame_clock(time);
}

/// The rendering stats of `session`'s renderer: what it has made and drawn
/// so far, and holds.
pub fn rendering_stats(session: &RenderSession) -> Result<Completion<RenderingStats>, Error> {
    session.operation(|worker, _| Ok(worker.renderer()?.rendering_stats()))
}

/// Completes once the GPU has finished the work that `session` submitted,
/// as a host waits before it presents a frame.
pub fn finish_gpu_work(session: &RenderSession) -> Result<Completion<()>, Error> {
    session.operation(|worker, _| {
        let mut finished = Ok(());
        worker.with_gpu(|gpu| finished = gpu.flush(true))?;
        finished.map_err(|error| Error::native(format!("the GPU failed to finish its work: {error}")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_geojson_as_a_source_takes_it() {
        assert!(matches!(
            parse_geojson(r#"{"type":"FeatureCollection","features":[]}"#),
            Ok(GeoJson::FeatureCollection(_))
        ));
        assert!(parse_geojson("{").is_err());
    }
}
