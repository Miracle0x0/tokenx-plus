//! Merge one response's snapshots at a time; keep payloads in a private spool.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Seek, SeekFrom};

use crate::records::{CopilotContext, UsageRecord};

type OwnedRecord = (usize, UsageRecord);

pub(super) struct CopilotMerge {
    writer: BufWriter<File>,
    offsets: BTreeMap<(Option<u64>, u64), Vec<u64>>,
    end: u64,
}

impl CopilotMerge {
    pub(super) fn new() -> io::Result<Self> {
        Ok(Self {
            writer: BufWriter::new(tempfile::tempfile()?),
            offsets: BTreeMap::new(),
            end: 0,
        })
    }

    pub(super) fn push(&mut self, owner: usize, record: UsageRecord) -> io::Result<()> {
        let key = (
            record.dedup_key,
            if record.dedup_key.is_some() {
                0
            } else {
                self.end
            },
        );
        let entry = (owner, record);
        let size = bincode::serialized_size(&entry).map_err(codec_error)?;
        bincode::serialize_into(&mut self.writer, &entry).map_err(codec_error)?;
        self.offsets.entry(key).or_default().push(self.end);
        self.end = self
            .end
            .checked_add(size)
            .ok_or_else(|| io::Error::other("Copilot spool offset overflow"))?;
        Ok(())
    }

    pub(super) fn into_groups(
        self,
    ) -> io::Result<impl Iterator<Item = io::Result<Vec<OwnedRecord>>>> {
        let file = self
            .writer
            .into_inner()
            .map_err(|error| error.into_error())?;
        let mut reader = BufReader::new(file);
        Ok(self.offsets.into_values().map(move |offsets| {
            let mut records = Vec::with_capacity(offsets.len());
            for offset in offsets {
                reader.seek(SeekFrom::Start(offset))?;
                records.push(bincode::deserialize_from(&mut reader).map_err(codec_error)?);
            }
            Ok(merge_snapshots(records))
        }))
    }
}

fn codec_error(error: bincode::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn context(record: &UsageRecord) -> CopilotContext {
    record
        .copilot_context
        .expect("Copilot records retain attribution evidence")
}

fn compatible(a: &UsageRecord, b: &UsageRecord) -> bool {
    let ac = context(a);
    let bc = context(b);
    a.model_id == b.model_id
        && a.provider_id == b.provider_id
        && a.timestamp == b.timestamp
        && (!ac.session_observed || !bc.session_observed || a.session_id == b.session_id)
        && (!ac.agent_observed || !bc.agent_observed || a.agent == b.agent)
        && (a.workspace_key.is_none()
            || b.workspace_key.is_none()
            || a.workspace_key == b.workspace_key)
}

fn identical_context(a: &UsageRecord, b: &UsageRecord) -> bool {
    context(a) == context(b)
        && a.model_id == b.model_id
        && a.provider_id == b.provider_id
        && a.timestamp == b.timestamp
        && a.session_id == b.session_id
        && a.agent == b.agent
        && a.workspace_key == b.workspace_key
}

fn merge_snapshots(mut records: Vec<OwnedRecord>) -> Vec<OwnedRecord> {
    records.sort_by(|(ao, a), (bo, b)| {
        let richness = |m: &UsageRecord| {
            let c = context(m);
            u8::from(c.session_observed)
                + u8::from(c.agent_observed)
                + u8::from(m.workspace_key.is_some())
        };
        richness(b)
            .cmp(&richness(a))
            .then_with(|| {
                (
                    &a.model_id,
                    &a.provider_id,
                    &a.session_id,
                    &a.agent,
                    a.timestamp,
                    &a.workspace_key,
                )
                    .cmp(&(
                        &b.model_id,
                        &b.provider_id,
                        &b.session_id,
                        &b.agent,
                        b.timestamp,
                        &b.workspace_key,
                    ))
            })
            .then_with(|| context(a).cmp(&context(b)))
            .then_with(|| ao.cmp(bo))
    });
    // Determine ambiguity before enrichment: a sparse snapshot must not bridge
    // two contradictory observed owners. The group is just one response key.
    let ambiguous: Vec<bool> = records
        .iter()
        .map(|(_, record)| {
            let mut sessions = HashSet::new();
            let mut agents = HashSet::new();
            let mut workspaces = HashSet::new();
            records
                .iter()
                .filter(|(_, other)| compatible(record, other))
                .any(|(_, other)| {
                    let evidence = context(other);
                    if evidence.session_observed {
                        sessions.insert(&other.session_id);
                    }
                    if evidence.agent_observed {
                        agents.insert(&other.agent);
                    }
                    if let Some(workspace) = &other.workspace_key {
                        workspaces.insert(workspace);
                    }
                    sessions.len() > 1 || agents.len() > 1 || workspaces.len() > 1
                })
        })
        .collect();
    let mut merged: Vec<OwnedRecord> = Vec::new();
    let mut merged_ambiguous = Vec::new();
    for ((owner, record), ambiguous) in records.into_iter().zip(ambiguous) {
        let mut candidates = merged
            .iter()
            .enumerate()
            .filter(|(index, (_, other))| {
                if ambiguous || merged_ambiguous[*index] {
                    identical_context(&record, other)
                } else {
                    compatible(&record, other)
                }
            })
            .map(|(index, _)| index);
        let first = candidates.next();
        let unique = first.filter(|_| candidates.next().is_none());
        if let Some(index) = unique {
            merge_record(&mut merged[index].1, record);
        } else {
            merged.push((owner, record));
            merged_ambiguous.push(ambiguous);
        }
    }
    merged
}

fn merge_record(existing: &mut UsageRecord, other: UsageRecord) {
    let mut current = context(existing);
    let incoming = context(&other);
    if !current.session_observed && incoming.session_observed {
        existing.session_id = other.session_id;
    }
    if !current.agent_observed && incoming.agent_observed {
        existing.agent = other.agent;
    }
    current.session_observed |= incoming.session_observed;
    current.agent_observed |= incoming.agent_observed;
    existing.copilot_context = Some(current);
    if existing.workspace_key.is_none() {
        existing.workspace_key = other.workspace_key;
        existing.workspace_label = other.workspace_label;
    }
    // Each source passed checked_total, so these two bucket sums cannot overflow.
    let inclusive_input = (existing.tokens.input + existing.tokens.cache_read)
        .max(other.tokens.input + other.tokens.cache_read);
    existing.tokens.cache_read = existing.tokens.cache_read.max(other.tokens.cache_read);
    existing.tokens.input = inclusive_input - existing.tokens.cache_read;
    existing.tokens.output = existing.tokens.output.max(other.tokens.output);
    existing.tokens.cache_write = existing.tokens.cache_write.max(other.tokens.cache_write);
    existing.tokens.reasoning = existing.tokens.reasoning.max(other.tokens.reasoning);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenBreakdown;

    fn record(session: Option<&str>, input: i64, read: i64, output: i64) -> UsageRecord {
        let mut record = UsageRecord::new_with_dedup(
            "gpt-5",
            "openai",
            session.unwrap_or("trace"),
            1,
            TokenBreakdown {
                input,
                output,
                cache_read: read,
                ..Default::default()
            },
            0.0,
            Some(42),
        );
        record.copilot_context = Some(CopilotContext {
            session_observed: session.is_some(),
            agent_observed: false,
        });
        record
    }

    #[test]
    fn snapshots_merge_inclusive_input_and_retain_richer_identity() {
        for reverse in [false, true] {
            let mut rows = vec![
                (0, record(None, 100, 0, 10)),
                (1, record(Some("session"), 70, 30, 20)),
            ];
            if reverse {
                rows.reverse();
            }
            let merged = merge_snapshots(rows);
            assert_eq!(merged.len(), 1);
            assert_eq!(merged[0].1.session_id.as_ref(), "session");
            assert_eq!(merged[0].1.tokens.total(), 120);
            assert_eq!(merged[0].1.tokens.input, 70);
        }
    }

    #[test]
    fn sparse_identity_does_not_bridge_conflicting_sessions_in_any_order() {
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let rows = [
                record(None, 10, 0, 1),
                record(Some("a"), 20, 0, 2),
                record(Some("b"), 30, 0, 3),
            ];
            let merged = merge_snapshots(order.into_iter().map(|i| (i, rows[i].clone())).collect());
            assert_eq!(merged.len(), 3);
            assert_eq!(
                merged.iter().map(|(_, m)| m.tokens.total()).sum::<i64>(),
                66
            );
        }
    }

    #[test]
    fn spool_groups_keyed_rows_and_keeps_unkeyed_rows_independent() {
        let mut spool = CopilotMerge::new().unwrap();
        for index in 0..4 {
            let mut message = record(Some("session"), 100, 0, 20);
            if index > 1 {
                message.dedup_key = None;
            }
            spool.push(index, message).unwrap();
        }
        let groups: Vec<_> = spool
            .into_groups()
            .unwrap()
            .collect::<io::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), 3);
    }

    #[test]
    fn conflicting_observed_contexts_are_retained() {
        for field in ["model", "provider", "timestamp", "agent", "workspace"] {
            let mut a = record(Some("session"), 100, 0, 20);
            let mut b = a.clone();
            match field {
                "model" => b.model_id = "gpt-6".into(),
                "provider" => b.provider_id = "azure".into(),
                "timestamp" => b.timestamp = 2,
                "agent" => {
                    a.copilot_context.as_mut().unwrap().agent_observed = true;
                    b.copilot_context.as_mut().unwrap().agent_observed = true;
                    a.agent = Some("A".into());
                    b.agent = Some("B".into());
                }
                "workspace" => {
                    a.workspace_key = Some("/a".into());
                    b.workspace_key = Some("/b".into());
                }
                _ => unreachable!(),
            }
            assert_eq!(merge_snapshots(vec![(0, a), (1, b)]).len(), 2, "{field}");
        }
    }
}
