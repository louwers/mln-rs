// Port-only: a worker instance's side of the worker codec (mod.rs): decodes
// the jobs the map side sends and encodes their replies.

use std::sync::Arc;

use mln_style::error::Error;
use mln_wire::payload::Payloads;
use mln_wire::shared::Role;

use super::envelope::{self, Encoded};
use super::fields::Tables;
use crate::Registry;
use crate::worker::job::{Job, JobResult, JobTimes, Reply};

/// A worker instance's side of its jobs.
pub(crate) struct WorkerCodec {
    /// The values the map side defined, by key.
    tables: Tables,
    registry: Arc<Registry>,
}

impl WorkerCodec {
    pub(crate) fn new(registry: Arc<Registry>) -> Self {
        WorkerCodec { tables: Tables::new(Role::Copies), registry }
    }

    /// Decodes a job's body: forgets what the map side lists as gone, then
    /// reads the job. Fails for a job that names a registration this side's
    /// registry lacks.
    pub(crate) fn decode_job(&mut self, body: &[u8]) -> Result<Job, String> {
        let (forget, job): (Vec<Vec<u64>>, &[u8]) = envelope::take(body)?;
        self.tables.forget(&forget);
        envelope::decode(job, (&mut self.tables, &*self.registry))
    }

    /// Encodes `reply`, with how long its stages took, its arrays as
    /// payloads. A result that can't cross becomes `Failed`.
    pub(crate) fn encode_reply(&mut self, reply: &Reply, times: &JobTimes) -> Encoded {
        let mut head = Vec::new();
        envelope::write_reply_header(&mut head, &reply.id, times);
        let tables = &mut self.tables;
        let mut encode = |result: &JobResult| {
            let mut bytes = head.clone();
            let mut payloads = Payloads::new();
            let written = envelope::append(&mut bytes, result, (&mut payloads, &mut *tables));
            tables.settle(written.is_ok());
            written.map(|()| Encoded { bytes, payloads: payloads.into_vec() })
        };
        encode(&reply.result).or_else(|failure| encode(&JobResult::Failed(Error::native(failure)))).unwrap_or_default()
    }
}
