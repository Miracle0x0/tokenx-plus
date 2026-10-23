//! Provenance is captured by the price application, never inferred from cost.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};

/// The selected rate row and observed billing dimensions for a catalog estimate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceOrigin {
    #[serde(deserialize_with = "crate::records::intern::de_intern")]
    pub source: Arc<str>,
    #[serde(deserialize_with = "crate::records::intern::de_intern")]
    pub matched_key: Arc<str>,
    #[serde(deserialize_with = "crate::records::intern::de_intern_opt")]
    pub billing_provider: Option<Arc<str>>,
    #[serde(deserialize_with = "crate::records::intern::de_intern_opt")]
    pub service_tier: Option<Arc<str>>,
}

/// A deterministic union of origins and the pricing coverage of participating usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostProvenance {
    pub complete: bool,
    pub sources: BTreeSet<PriceOrigin>,
}

impl Default for CostProvenance {
    fn default() -> Self {
        Self {
            complete: true,
            sources: BTreeSet::new(),
        }
    }
}

impl CostProvenance {
    pub(crate) fn include(&mut self, record: &crate::records::UsageRecord) {
        self.complete &= record.cost_is_complete;
        self.sources.extend(record.price_origin.iter().cloned());
    }

    pub(crate) fn merge(&mut self, other: &Self) {
        self.complete &= other.complete;
        self.sources.extend(other.sources.iter().cloned());
    }
}

pub(crate) struct CostEstimate {
    pub cost: f64,
    pub origin: Option<PriceOrigin>,
    pub complete: bool,
}

pub(super) fn covers_usage(pricing: &super::ModelPricing, usage: &crate::TokenBreakdown) -> bool {
    [
        (usage.input > 0, pricing.input_cost_per_token),
        (
            usage.output > 0 || usage.reasoning > 0,
            pricing.output_cost_per_token,
        ),
        (usage.cache_read > 0, pricing.cache_read_input_token_cost),
        (
            usage.cache_write > usage.cache_write_1h,
            pricing.cache_creation_input_token_cost,
        ),
        (
            usage.cache_write_1h > 0,
            pricing.cache_creation_input_token_cost_above_1hr,
        ),
    ]
    .into_iter()
    .all(|(used, rate)| !used || rate.is_some_and(|rate| rate.is_finite() && rate >= 0.0))
}
