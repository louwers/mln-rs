// Port-only: the map side of the worker codec (mod.rs), for one worker
// instance: encodes the jobs for that instance and decodes their replies.

use std::sync::Arc;

use mln_style::serialization::{SharedExpression, SharedLayer};
use mln_util::error::Error;
use mln_util::serialization::{payload, shared};

use super::envelope::{self, Encoded};
use super::fields::{SharedAvailableImages, SharedEvaluatedLayer, SharedSourceStyle};
use super::session;
use crate::Registry;
use crate::tile::job::{JobResult, Run};

/// The map side of one worker instance's jobs.
pub(crate) struct MapCodec {
    /// The keys of what the instance has of this side's: source styles,
    /// layers, their properties and expressions, and sets of images.
    tables: shared::Tables,
    registry: Arc<Registry>,
}

impl MapCodec {
    /// The map side of a new instance, whose jobs name `registry`'s
    /// registrations.
    pub(crate) fn new(registry: Arc<Registry>) -> Self {
        MapCodec {
            tables: shared::Tables::new()
                .origin::<SharedSourceStyle>()
                .origin::<SharedAvailableImages>()
                .origin::<SharedLayer>()
                .origin::<SharedEvaluatedLayer>()
                .origin::<SharedExpression>(),
            registry,
        }
    }

    /// The frame of `run`: its header, then the keys of the shared values
    /// that are gone on this side, then the job. Fails for what can't cross,
    /// which then defines nothing.
    pub(crate) fn encode_run(&mut self, run: &Run) -> Result<Vec<u8>, String> {
        let job = self.tables.open(|| {
            let mut bytes = Vec::new();
            envelope::append(&mut bytes, &run.job)?;
            Ok::<_, String>(bytes)
        })?;
        // After the job, which keeps alive what it defines.
        let forget = self.tables.forget_dead();
        let mut bytes = Vec::new();
        envelope::write_job_header(&mut bytes, &run.id);
        envelope::append(&mut bytes, &forget)?;
        bytes.extend_from_slice(&job);
        Ok(bytes)
    }

    /// The result in a reply's body. A body this side can't decode becomes
    /// `Failed`.
    pub(crate) fn decode_result(&mut self, body: Encoded) -> JobResult {
        let Encoded { bytes, payloads } = body;
        let Self { tables, registry } = self;
        let decode = || tables.open(|| envelope::decode::<JobResult>(&bytes));
        payload::decode(payloads, || session(registry, decode))
            .unwrap_or_else(|error| JobResult::Failed(Error::native(format!("a tile worker's reply: {error}"))))
    }
}
