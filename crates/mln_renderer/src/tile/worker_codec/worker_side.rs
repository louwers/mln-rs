// Port-only: a worker instance's side of the worker codec (mod.rs): decodes
// the jobs the map side sends and encodes their replies.

use mln_style::serialization::{SharedExpression, SharedLayer, SharedLayerProperties};
use mln_util::error::Error;
use mln_util::serialization::{payload, shared};

use super::envelope::{self, Encoded};
use super::fields::{SharedAvailableImages, SharedSourceStyle};
use crate::tile::job::{Job, JobResult, JobTimes, Reply};

/// A worker instance's side of its jobs.
pub(crate) struct WorkerCodec {
    /// The values the map side defined, by key.
    tables: shared::Tables,
}

impl Default for WorkerCodec {
    fn default() -> Self {
        let tables = shared::Tables::new()
            .copies::<SharedSourceStyle>()
            .copies::<SharedAvailableImages>()
            .copies::<SharedLayer>()
            .copies::<SharedLayerProperties>()
            .copies::<SharedExpression>();
        WorkerCodec { tables }
    }
}

impl WorkerCodec {
    pub(crate) fn new() -> Self {
        WorkerCodec::default()
    }

    /// Decodes a job's body: forgets what the map side lists as gone, then
    /// reads the job.
    pub(crate) fn decode_job(&mut self, body: &[u8]) -> Result<Job, String> {
        let (forget, job): (Vec<(String, Vec<u64>)>, &[u8]) = envelope::take(body)?;
        self.tables.forget(&forget);
        self.tables.open(|| envelope::decode(job))
    }

    /// Encodes `reply`, with how long its stages took, its arrays as
    /// payloads. A result that can't cross becomes `Failed`.
    pub(crate) fn encode_reply(&mut self, reply: &Reply, times: &JobTimes) -> Encoded {
        let mut head = Vec::new();
        envelope::write_reply_header(&mut head, &reply.id, times);
        let encode = |result: &JobResult| {
            let mut bytes = head.clone();
            envelope::append(&mut bytes, result)?;
            Ok::<_, String>(bytes)
        };
        let (bytes, payloads) = payload::encode(|| {
            self.tables
                .open(|| encode(&reply.result))
                .or_else(|failure| self.tables.open(|| encode(&JobResult::Failed(Error::native(failure)))))
        });
        Encoded { bytes: bytes.unwrap_or_default(), payloads }
    }
}
