//! Source eligibility, canonical identity, and runtime token pricing.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::records::UsageRecord;
use crate::{
    input_health, model_aliases, pricing, provider_identity, records, ModelMappings, TokenBreakdown,
};

#[cfg(test)]
pub(crate) mod test_support;

pub(crate) fn positive_token_total(tokens: &TokenBreakdown) -> Option<i64> {
    let buckets = [
        tokens.input,
        tokens.output,
        tokens.cache_read,
        tokens.cache_write,
        tokens.reasoning,
    ];
    if buckets.into_iter().any(|value| value < 0) || tokens.cache_write_1h < 0 {
        return None;
    }
    buckets.into_iter().try_fold(0_i64, i64::checked_add)
}

pub(crate) fn has_positive_tokens(tokens: &TokenBreakdown) -> bool {
    tokens.input > 0
        || tokens.output > 0
        || tokens.cache_read > 0
        || tokens.cache_write > 0
        || tokens.reasoning > 0
}

pub(crate) fn apply_canonical_token_pricing(
    message: &mut records::UsageRecord,
    pricing: Option<&pricing::PricingService>,
) -> Result<(), pricing::PricingComputationError> {
    message.cost = 0.0;
    message.pricing_error = None;
    message.price_origin = None;
    message.cost_is_complete = false;

    let Some(pricing) = pricing else {
        return Ok(());
    };
    if model_aliases::is_explicitly_unpriced_model_id(&message.model_id) {
        return Ok(());
    }

    let estimate = pricing.estimate_canonical_usage(
        &message.model_id,
        Some(message.provider_id.as_ref()),
        &message.tokens,
        pricing::UsagePricingContext {
            timestamp_ms: Some(message.timestamp),
            service_tier: message.service_tier.as_deref(),
            billing_provider: message.billing_provider.as_deref(),
            is_aggregate: message.is_aggregate,
        },
    )?;

    message.cost = estimate.cost;
    message.price_origin = estimate.origin;
    message.cost_is_complete = estimate.complete;
    Ok(())
}

fn refresh_derived_message_fields(message: &mut records::UsageRecord) {
    if let Some(provider) = provider_identity::provider_override_from_model_and_provider(
        &message.model_id,
        &message.provider_id,
    ) {
        message.provider_id = records::intern::intern(provider);
    }
}

fn canonicalize_message_provider(message: &mut records::UsageRecord) {
    let provider =
        provider_identity::finalized_provider_id(&message.provider_id, &message.model_id);
    message.provider_id = records::intern::intern(&provider);
}

fn canonicalize_message_model(
    message: &mut records::UsageRecord,
    model_cache: &mut HashMap<(Arc<str>, Arc<str>), Arc<str>>,
    mappings: &ModelMappings,
) {
    let key = (
        Arc::clone(&message.raw_model_id),
        Arc::clone(&message.model_id),
    );
    if let Some(canonical) = model_cache.get(&key) {
        message.model_id = Arc::clone(canonical);
        return;
    }

    let raw = Arc::clone(&message.model_id);
    let canonical = mappings.canonicalize_observation(&message.raw_model_id, &raw);
    let canonical = if canonical == raw.as_ref() {
        Arc::clone(&raw)
    } else {
        records::intern::intern(&canonical)
    };

    model_cache.insert(key, Arc::clone(&canonical));
    message.model_id = canonical;
}

pub(crate) enum RecordFinalization {
    Accept,
    Filter,
    Reject(input_health::RecordRejectionReason),
}

pub(crate) fn record_finalization(record: &records::UsageRecord) -> RecordFinalization {
    use input_health::RecordRejectionReason;

    if record.model_id.trim().is_empty() {
        return RecordFinalization::Reject(RecordRejectionReason::MissingModel);
    }
    if record.session_id.trim().is_empty() {
        return RecordFinalization::Reject(RecordRejectionReason::MissingSession);
    }
    if record.timestamp <= 0
        || chrono::DateTime::<chrono::Utc>::from_timestamp_millis(record.timestamp).is_none()
    {
        return RecordFinalization::Reject(RecordRejectionReason::MissingTimestamp);
    }
    if [
        record.tokens.input,
        record.tokens.output,
        record.tokens.cache_read,
        record.tokens.cache_write,
        record.tokens.reasoning,
    ]
    .into_iter()
    .any(|value| value < 0)
        || record.tokens.cache_write_1h < 0
        || record.tokens.cache_write_1h > record.tokens.cache_write
        || record.tokens.checked_total().is_none()
    {
        return RecordFinalization::Reject(RecordRejectionReason::InvalidUsageRecord);
    }
    if record.message_count < 0 {
        return RecordFinalization::Reject(RecordRejectionReason::InvalidUsageRecord);
    }
    if !has_positive_tokens(&record.tokens) {
        return RecordFinalization::Filter;
    }
    RecordFinalization::Accept
}

pub(crate) fn retain_source_eligible_messages<M: AsMut<records::UsageRecord>>(
    messages: &mut Vec<M>,
) -> input_health::RejectionSummary {
    let mut rejections = input_health::RejectionSummary::default();
    messages.retain_mut(|message| match record_finalization(message.as_mut()) {
        RecordFinalization::Accept => true,
        RecordFinalization::Filter => false,
        RecordFinalization::Reject(reason) => {
            rejections.record(reason);
            false
        }
    });
    rejections
}

pub(crate) fn price_source_eligible_messages<M: AsMut<records::UsageRecord>>(
    messages: &mut Vec<M>,
    pricing: Option<&pricing::PricingService>,
    model_mappings: &ModelMappings,
) -> input_health::RejectionSummary {
    let mut model_cache = HashMap::new();
    let mut rejections = input_health::RejectionSummary::default();

    messages.retain_mut(|message| {
        let message = message.as_mut();
        canonicalize_message_model(message, &mut model_cache, model_mappings);
        refresh_derived_message_fields(message);
        canonicalize_message_provider(message);
        if let Err(error) = apply_canonical_token_pricing(message, pricing) {
            match error {
                pricing::PricingComputationError::UnsupportedServiceTier { .. }
                | pricing::PricingComputationError::MissingServiceTierRate { .. }
                | pricing::PricingComputationError::MissingCacheWrite1hRate
                | pricing::PricingComputationError::MissingBillingRate { .. }
                | pricing::PricingComputationError::AmbiguousRequestTier { .. } => {
                    message.pricing_error = Some(error);
                }
                _ => {
                    rejections
                        .record(input_health::RecordRejectionReason::PricingComputationFailed);
                    return false;
                }
            }
        }
        true
    });
    rejections
}

pub(crate) fn should_keep_deduped_message(
    seen_keys: &mut HashSet<u64>,
    message: &UsageRecord,
) -> bool {
    message.dedup_key.is_none_or(|key| seen_keys.insert(key))
}
