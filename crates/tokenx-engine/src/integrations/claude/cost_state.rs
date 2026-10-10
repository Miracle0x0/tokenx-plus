//! Supplement transcript usage with recorded cumulative session/model totals.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;

use crate::input_health::{RecordRejectionReason, RejectionSummary};
use crate::records::UsageRecord;
use crate::TokenBreakdown;

#[derive(Deserialize)]
struct EntryType {
    #[serde(rename = "type")]
    entry_type: String,
}

pub(super) fn is_snapshot(line: &str) -> bool {
    serde_json::from_str::<EntryType>(line).is_ok_and(|entry| entry.entry_type == "cost-state")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CostState {
    session_id: String,
    start_time: i64,
    model_usage: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelUsage {
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
}

pub(super) fn append_records(
    line: &str,
    messages: &mut Vec<UsageRecord>,
    rejections: &mut RejectionSummary,
) {
    let state: CostState = match serde_json::from_str(line) {
        Ok(state) => state,
        Err(_) => {
            rejections.record(RecordRejectionReason::MalformedRecord);
            return;
        }
    };
    for (model, value) in state.model_usage {
        let usage: ModelUsage = match serde_json::from_value(value) {
            Ok(usage) => usage,
            Err(_) => {
                rejections.record(RecordRejectionReason::MalformedRecord);
                continue;
            }
        };
        let mut record = UsageRecord::new(
            &model,
            crate::provider_identity::observed_provider_id("", &model),
            &state.session_id,
            state.start_time,
            TokenBreakdown {
                input: usage.input_tokens,
                // thinkingTokens is already included in outputTokens.
                output: usage.output_tokens,
                cache_read: usage.cache_read_input_tokens,
                cache_write: usage.cache_creation_input_tokens,
                ..Default::default()
            },
            0.0,
        );
        record.claude_is_cost_snapshot = true;
        record.claude_input_is_split = true;
        record.message_count = 0;
        match crate::record_finalization(&record) {
            crate::RecordFinalization::Accept => messages.push(record),
            crate::RecordFinalization::Filter => {}
            crate::RecordFinalization::Reject(reason) => rejections.record(reason),
        }
    }
}

type ModelKey = (Arc<str>, String);
type ModelTotals = BTreeMap<Arc<str>, (usize, UsageRecord)>;

fn model_key(record: &UsageRecord) -> ModelKey {
    // Reconcile source spellings (including [1m]) before user mappings, which
    // can intentionally combine otherwise distinct models for reporting.
    (
        Arc::clone(
            record
                .claude_session_id
                .as_ref()
                .unwrap_or(&record.session_id),
        ),
        crate::model_aliases::normalize_model_syntax(&record.raw_model_id),
    )
}

#[derive(Default)]
pub(super) struct CostStateReconciliation {
    totals: BTreeMap<ModelKey, ModelTotals>,
}

impl CostStateReconciliation {
    pub(super) fn push(&mut self, owner: usize, record: UsageRecord) {
        self.totals
            .entry(model_key(&record))
            .or_default()
            .entry(Arc::clone(&record.raw_model_id))
            .and_modify(|(_, existing)| {
                // Each file supplies its last snapshot. Mirrored files may
                // carry older cumulative totals; never sum these copies.
                existing.tokens.input = existing.tokens.input.max(record.tokens.input);
                existing.tokens.output = existing.tokens.output.max(record.tokens.output);
                existing.tokens.cache_read =
                    existing.tokens.cache_read.max(record.tokens.cache_read);
                existing.tokens.cache_write =
                    existing.tokens.cache_write.max(record.tokens.cache_write);
            })
            .or_insert((owner, record));
    }

    pub(super) fn subtract_transcript(&mut self, record: &UsageRecord) {
        if self.totals.is_empty() {
            return;
        }
        let Some(totals) = self.totals.get_mut(&model_key(record)) else {
            return;
        };
        // Detailed records keep their timestamps and cache durations. Only the
        // positive, unrepresented part of each cumulative bucket is emitted.
        let mut uncovered = record.tokens.clone();
        if let Some((_, exact)) = totals.get_mut(&record.raw_model_id) {
            subtract_buckets(&mut exact.tokens, &mut uncovered);
        }
        for (_, remaining) in totals.values_mut() {
            subtract_buckets(&mut remaining.tokens, &mut uncovered);
        }
    }

    pub(super) fn validate(
        &mut self,
        mut reject: impl FnMut(usize, crate::input_health::RecordRejectionReason),
    ) {
        self.totals.retain(|_, totals| {
            totals.retain(
                |_, (owner, record)| match crate::record_finalization(record) {
                    crate::RecordFinalization::Accept => true,
                    crate::RecordFinalization::Filter => false,
                    crate::RecordFinalization::Reject(reason) => {
                        reject(*owner, reason);
                        false
                    }
                },
            );
            !totals.is_empty()
        });
    }

    pub(super) fn into_records(self) -> impl Iterator<Item = (usize, UsageRecord)> {
        self.totals.into_values().flat_map(BTreeMap::into_values)
    }
}

fn subtract_buckets(total: &mut crate::TokenBreakdown, detail: &mut crate::TokenBreakdown) {
    for (total, detail) in [
        (&mut total.input, &mut detail.input),
        (&mut total.output, &mut detail.output),
        (&mut total.cache_read, &mut detail.cache_read),
        (&mut total.cache_write, &mut detail.cache_write),
    ] {
        let covered = (*total).min(*detail);
        *total -= covered;
        *detail -= covered;
    }
}
