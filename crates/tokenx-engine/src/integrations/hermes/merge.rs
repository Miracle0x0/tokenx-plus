//! Select one coherent copy per usage group, preferring wider logged coverage.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use super::logs::{group_key, GroupKey};
use crate::input_health::{InputStatus, RejectionSummary};
use crate::integrations::{cache, BoundUsageSink, FoldContext, InputPipelineError, ParsedUnit};
use crate::records::UsageRecord;
use crate::TokenBreakdown;

struct Candidate {
    owner: usize,
    offset: u64,
    tokens: TokenBreakdown,
    logged_days: usize,
}

pub(super) struct HermesFold {
    writer: BufWriter<File>,
    end: u64,
    groups: BTreeMap<GroupKey, Candidate>,
    counts: BTreeMap<Arc<str>, i32>,
    health: Vec<(PathBuf, InputStatus, RejectionSummary)>,
}

fn codec_error(error: bincode::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

impl HermesFold {
    pub(super) fn new() -> Result<Self, InputPipelineError> {
        Ok(Self {
            writer: BufWriter::new(
                tempfile::tempfile().map_err(InputPipelineError::HermesReconciliation)?,
            ),
            end: 0,
            groups: BTreeMap::new(),
            counts: BTreeMap::new(),
            health: Vec::new(),
        })
    }

    pub(super) fn consume(
        &mut self,
        parsed: Vec<ParsedUnit>,
        ctx: &mut FoldContext<'_>,
    ) -> Result<(), InputPipelineError> {
        for unit in parsed {
            let cache::ResolvedUnit {
                unit,
                mut messages,
                status,
                mut rejections,
                ..
            } = cache::resolve_unit(unit, ctx)?;
            rejections.merge(&crate::retain_source_eligible_messages(&mut messages));
            let mut groups: BTreeMap<GroupKey, Vec<UsageRecord>> = BTreeMap::new();
            let mut counts: BTreeMap<Arc<str>, i32> = BTreeMap::new();
            for record in messages {
                let count = counts.entry(record.session_id.clone()).or_default();
                *count = count
                    .checked_add(record.message_count)
                    .ok_or_else(|| InputPipelineError::contract("Hermes message count overflow"))?;
                groups.entry(group_key(&record)).or_default().push(record);
            }
            for (session, count) in counts {
                self.counts.entry(session).or_insert(count);
            }
            for (key, records) in groups {
                ctx.cancellation()
                    .check(crate::engine::AcquisitionPhase::Folding)?;
                let tokens = records
                    .iter()
                    .try_fold(TokenBreakdown::default(), |total, record| {
                        total.checked_add(&record.tokens)
                    })
                    .ok_or_else(|| {
                        InputPipelineError::contract("Hermes split exceeds its authoritative total")
                    })?;
                let logged_days = records
                    .iter()
                    .filter(|record| !record.is_aggregate)
                    .filter_map(|record| {
                        ctx.calendar()
                            .local_date_and_hour(record.timestamp)
                            .map(|(day, _)| day)
                    })
                    .collect::<BTreeSet<_>>()
                    .len();
                if self
                    .groups
                    .get(&key)
                    .is_some_and(|old| old.tokens != tokens || old.logged_days >= logged_days)
                {
                    continue;
                }
                let offset = self.end;
                bincode::serialize_into(&mut self.writer, &records)
                    .map_err(codec_error)
                    .map_err(InputPipelineError::HermesReconciliation)?;
                let size = bincode::serialized_size(&records)
                    .map_err(codec_error)
                    .map_err(InputPipelineError::HermesReconciliation)?;
                self.end = self.end.checked_add(size).ok_or_else(|| {
                    InputPipelineError::contract("Hermes reconciliation file size overflow")
                })?;
                self.groups.insert(
                    key,
                    Candidate {
                        owner: self.health.len(),
                        offset,
                        tokens,
                        logged_days,
                    },
                );
            }
            self.health.push((unit.path, status, rejections));
        }
        Ok(())
    }

    pub(super) fn finish(
        mut self,
        ctx: &mut FoldContext<'_>,
        sink: &mut BoundUsageSink<'_>,
    ) -> Result<(), InputPipelineError> {
        let file = self
            .writer
            .into_inner()
            .map_err(|error| InputPipelineError::HermesReconciliation(error.into_error()))?;
        let mut reader = BufReader::new(file);
        for (key, candidate) in self.groups {
            ctx.cancellation()
                .check(crate::engine::AcquisitionPhase::Folding)?;
            reader
                .seek(SeekFrom::Start(candidate.offset))
                .map_err(InputPipelineError::HermesReconciliation)?;
            let mut records: Vec<UsageRecord> = bincode::deserialize_from(&mut reader)
                .map_err(codec_error)
                .map_err(InputPipelineError::HermesReconciliation)?;
            for record in &mut records {
                record.message_count = 0;
            }
            if let Some(first) = records.first_mut() {
                first.message_count = self.counts.remove(&key.0).unwrap_or(0);
            }
            let rejections = &mut self.health[candidate.owner].2;
            rejections.merge(&crate::price_source_eligible_messages(
                &mut records,
                ctx.pricing,
                &ctx.model_mappings,
            ));
            rejections.merge(&cache::emit_messages(records, sink));
        }
        for (path, status, rejections) in self.health {
            ctx.record_health(path, status, rejections);
        }
        Ok(())
    }
}
