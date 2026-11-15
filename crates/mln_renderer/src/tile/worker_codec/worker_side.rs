// Port-only: a worker instance's side of the worker codec (mod.rs): decodes
// the jobs the map side sends and encodes their replies.

use std::sync::Arc;

use mln_style::serialization::{SharedExpression, SharedLayer, SharedLayerProperties};
use mln_util::error::Error;
use mln_util::serialization::{payload, shared};

use super::envelope::{self, Encoded};
use super::fields::{SharedAvailableImages, SharedSourceStyle};
use super::session;
use crate::Registry;
use crate::tile::job::{Job, JobResult, JobTimes, Reply};

/// A worker instance's side of its jobs.
pub(crate) struct WorkerCodec {
    /// The values the map side defined, by key.
    tables: shared::Tables,
    /// What the jobs' registrations resolve against.
    registry: Arc<Registry>,
    /// Whether the map side's registry has `registry`'s digest, once its
    /// first frame told.
    same_registry: Option<bool>,
}

impl WorkerCodec {
    /// An instance's side, which resolves the jobs' registrations against
    /// `registry`.
    pub(crate) fn new(registry: Arc<Registry>) -> Self {
        let tables = shared::Tables::new()
            .copies::<SharedSourceStyle>()
            .copies::<SharedAvailableImages>()
            .copies::<SharedLayer>()
            .copies::<SharedLayerProperties>()
            .copies::<SharedExpression>();
        WorkerCodec { tables, registry, same_registry: None }
    }

    /// Decodes a job's body: checks the map side's registry digest, forgets
    /// what the map side lists as gone, then reads the job. Fails every job
    /// once the digest differs from this side's.
    pub(crate) fn decode_job(&mut self, body: &[u8]) -> Result<Job, String> {
        let (digest, body): (Option<u64>, &[u8]) = envelope::take(body)?;
        if let Some(digest) = digest {
            self.same_registry = Some(digest == self.registry.digest());
        }
        match self.same_registry {
            Some(true) => {}
            Some(false) => return Err("the tile worker's registrations differ from the map's".to_owned()),
            None => return Err("a job before the map's registry digest".to_owned()),
        }
        let (forget, job): (Vec<(String, Vec<u64>)>, &[u8]) = envelope::take(body)?;
        self.tables.forget(&forget);
        session(&self.registry, || self.tables.open(|| envelope::decode(job)))
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
