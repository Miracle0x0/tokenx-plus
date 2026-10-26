use super::{price_source_eligible_messages, retain_source_eligible_messages};
use crate::{input_health, pricing, records, ModelMappings};

pub(crate) fn finalize_message_identities<M: AsMut<records::UsageRecord>>(messages: &mut Vec<M>) {
    let _ = retain_source_eligible_messages(messages);
    let _ = price_source_eligible_messages(messages, None, &ModelMappings::default());
}

pub(crate) fn finalize_token_priced_messages<M: AsMut<records::UsageRecord>>(
    messages: &mut Vec<M>,
    pricing: Option<&pricing::PricingService>,
) -> input_health::RejectionSummary {
    let mut rejections = retain_source_eligible_messages(messages);
    rejections.merge(&price_source_eligible_messages(
        messages,
        pricing,
        &ModelMappings::default(),
    ));
    rejections
}
