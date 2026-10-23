//! Authoritative per-turn usage. Money fields are deliberately not decoded.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::input_health::RecordRejectionReason;
use crate::TokenBreakdown;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Usage {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cached_read_tokens: Option<i64>,
    cached_write_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    total_tokens: Option<i64>,
    model_calls: Option<i32>,
    model_usage: Option<BTreeMap<String, Usage>>,
}

pub(super) struct ModelUsage {
    pub model: String,
    pub tokens: TokenBreakdown,
    pub calls: Option<i32>,
}

impl Usage {
    fn tokens(&self) -> Result<TokenBreakdown, RecordRejectionReason> {
        let input = self.input_tokens.unwrap_or(0);
        let output = self.output_tokens.unwrap_or(0);
        let cache_read = self.cached_read_tokens.unwrap_or(0);
        let cache_write = self.cached_write_tokens.unwrap_or(0);
        let reasoning = self.reasoning_tokens.unwrap_or(0);
        let invalid = RecordRejectionReason::MalformedRecord;
        if [
            input,
            output,
            cache_read,
            cache_write,
            reasoning,
            self.total_tokens.unwrap_or(0),
        ]
        .into_iter()
        .any(|count| count < 0)
            || self.model_calls.is_some_and(|calls| calls <= 0)
        {
            return Err(invalid);
        }
        let total = input.checked_add(output).ok_or(invalid)?;
        if self.total_tokens.is_some_and(|reported| reported != total) {
            return Err(invalid);
        }
        let caches = cache_read.checked_add(cache_write).ok_or(invalid)?;
        if caches > input {
            return Err(invalid);
        }
        let reasoning = reasoning.min(output);
        Ok(TokenBreakdown {
            input: input - caches,
            output: output - reasoning,
            cache_read,
            cache_write,
            reasoning,
            ..Default::default()
        })
    }
}

pub(super) fn parse(
    value: &Value,
    current_model: Option<&str>,
) -> Result<Option<Vec<ModelUsage>>, RecordRejectionReason> {
    if value
        .pointer("/params/update/sessionUpdate")
        .and_then(Value::as_str)
        != Some("turn_completed")
    {
        return Ok(None);
    }
    let Some(raw) = value.pointer("/params/update/usage") else {
        return Ok(None);
    };
    let usage: Usage =
        serde_json::from_value(raw.clone()).map_err(|_| RecordRejectionReason::MalformedRecord)?;
    if let Some(models) = usage
        .model_usage
        .as_ref()
        .filter(|models| !models.is_empty())
    {
        let mut rows = Vec::new();
        let mut total = 0_i64;
        let mut totals = TokenBreakdown::default();
        let mut calls = Some(0_i32);
        for (model, detail) in models {
            let tokens = detail.tokens()?;
            total = total
                .checked_add(tokens.total())
                .ok_or(RecordRejectionReason::MalformedRecord)?;
            totals.input += tokens.input;
            totals.output += tokens.output;
            totals.cache_read += tokens.cache_read;
            totals.cache_write += tokens.cache_write;
            totals.reasoning += tokens.reasoning;
            calls = match (calls, detail.model_calls) {
                (Some(sum), Some(count)) => Some(
                    sum.checked_add(count)
                        .ok_or(RecordRejectionReason::MalformedRecord)?,
                ),
                _ => None,
            };
            if tokens.total() == 0 {
                continue;
            }
            if model.trim().is_empty() {
                return Err(RecordRejectionReason::MissingModel);
            }
            rows.push(ModelUsage {
                model: model.trim().to_owned(),
                tokens,
                calls: detail
                    .model_calls
                    .or_else(|| (models.len() == 1).then_some(usage.model_calls).flatten()),
            });
        }
        for (observed, expected) in [
            (
                usage.input_tokens,
                totals.input + totals.cache_read + totals.cache_write,
            ),
            (usage.output_tokens, totals.output + totals.reasoning),
            (usage.cached_read_tokens, totals.cache_read),
            (usage.cached_write_tokens, totals.cache_write),
            (
                usage
                    .reasoning_tokens
                    .map(|count| count.min(totals.output + totals.reasoning)),
                totals.reasoning,
            ),
            (usage.total_tokens, total),
        ] {
            if observed.is_some_and(|observed| observed != expected) {
                return Err(RecordRejectionReason::MalformedRecord);
            }
        }
        if usage
            .model_calls
            .is_some_and(|count| count <= 0 || calls.is_some_and(|sum| sum != count))
        {
            return Err(RecordRejectionReason::MalformedRecord);
        }
        return Ok((total > 0).then_some(rows));
    }
    let top = usage.tokens()?;
    if top.total() == 0 {
        return Ok(None);
    }
    let model = current_model
        .filter(|model| !model.trim().is_empty())
        .ok_or(RecordRejectionReason::MissingModel)?;
    Ok(Some(vec![ModelUsage {
        model: model.to_owned(),
        tokens: top,
        calls: usage.model_calls,
    }]))
}

pub(super) fn prompt_id(value: &Value) -> Option<&str> {
    [
        "/params/update/promptId",
        "/params/update/prompt_id",
        "/params/_meta/promptId",
        "/params/_meta/prompt_id",
    ]
    .into_iter()
    .find_map(|path| value.pointer(path).and_then(Value::as_str))
    .filter(|id| !id.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn per_model_usage_can_supply_all_buckets_but_must_reconcile_present_turn_fields() {
        let mut row = json!({"params":{"update":{"sessionUpdate":"turn_completed","usage":{
            "totalTokens":120,"modelUsage":{"grok-4.5":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":60}}
        }}}});
        assert_eq!(parse(&row, None).unwrap().unwrap()[0].tokens.total(), 120);
        row["params"]["update"]["usage"]["cachedReadTokens"] = json!(70);
        assert!(matches!(
            parse(&row, None),
            Err(RecordRejectionReason::MalformedRecord)
        ));
    }
}
