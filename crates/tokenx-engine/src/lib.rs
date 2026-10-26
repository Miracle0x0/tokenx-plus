#![deny(clippy::all)]

mod acquisition;
mod acquisition_error;
mod client_catalog;
pub mod clients;
mod engine;
mod finalization;
pub mod fs_atomic;
mod generation;
mod input_footprint;
pub mod input_health;
mod input_record_cache;
mod integrations;
mod model_aliases;
mod model_mappings;
pub use model_mappings::{ModelMappingRule, ModelMappings, ModelMappingsParseError};
mod machine_labels;
pub mod pricing;
mod provider_identity;
mod records;
pub mod scanner;
mod scanner_settings;
mod token_imputation;

mod aggregate;
pub mod projection;

pub use acquisition_error::{AcquisitionError, AcquisitionErrorKind};
pub use aggregate::{
    aggregate_by_period, build_contribution_graph_for_today, build_period_usage,
    calculate_streaks_for_today, find_peak_hour, DateRange, DateRangeError, FrozenUsageIndex,
    InvalidCostKind, PeriodBucket, SessionTokens, SessionUsage, UsageAggregationError,
    UsageIndexValidationError, UsageProjectionError, UNKNOWN_WORKSPACE_LABEL,
};
pub use clients::{ClientId, ClientIdentity};
pub use engine::{
    AcquisitionCancellation, AcquisitionCancelled, AcquisitionEngine, AcquisitionPhase,
    GenerationBuildError, PreparedAcquisition,
};
pub use generation::{
    AcquisitionConfig, AcquisitionConfigError, CalendarContext, ClientRootOverrides,
    ClientSelection, ClientUniverse, Generation, GenerationError, PricingContext, UsageQuery,
};
pub use input_footprint::{InputFootprint, InputFootprintOverflow};
pub use input_health::{
    DataHealth, InputDiagnosticKind, InputFailure, InputHealth, InputStatus, RecordRejectionReason,
    RejectionEntry, RejectionSummary, ScannedInput,
};
pub use input_record_cache::{
    prune_input_record_cache, InputRecordCachePruneError, InputRecordCachePruneStats,
};
pub use projection::{ModelProjection, UsageProjection};
pub use provider_identity::{inferred_provider_from_model, normalize_provider_for_grouping};
pub use records::{AttributedUsageRecord, BillingProvider};

/// Canonicalize a raw model string for callers that do not already hold a
/// finalized `AttributedUsageRecord`.
///
/// Local usage aggregation consumes finalized messages directly and treats
/// `AttributedUsageRecord.model_id` as already canonical.
#[doc(hidden)]
pub fn normalize_model_for_grouping(model_id: &str) -> String {
    model_aliases::canonicalize_model_id(model_id)
}

#[doc(hidden)]
pub fn build_usage_index(
    messages: &[AttributedUsageRecord],
    date_range: DateRange,
    calendar: CalendarContext,
) -> Result<FrozenUsageIndex, UsageAggregationError> {
    let mut accumulator = aggregate::GenerationAccumulator::new(date_range, calendar);
    for message in messages {
        if let aggregate::RecordAggregationOutcome::Rejected(error) = accumulator.push(message) {
            return Err(error);
        }
    }
    accumulator.into_usage_index()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GroupBy {
    #[default]
    Model,
    ClientModel,
    ClientProviderModel,
    WorkspaceModel,
    MachineModel,
}

impl std::fmt::Display for GroupBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupBy::Model => write!(f, "model"),
            GroupBy::ClientModel => write!(f, "client,model"),
            GroupBy::ClientProviderModel => write!(f, "client,provider,model"),
            GroupBy::WorkspaceModel => write!(f, "workspace,model"),
            GroupBy::MachineModel => write!(f, "machine,model"),
        }
    }
}

impl std::str::FromStr for GroupBy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized: String = s.split(',').map(|p| p.trim()).collect::<Vec<_>>().join(",");
        match normalized.to_lowercase().as_str() {
            "model" => Ok(GroupBy::Model),
            "client,model" => Ok(GroupBy::ClientModel),
            "client,provider,model" => Ok(GroupBy::ClientProviderModel),
            "workspace,model" => Ok(GroupBy::WorkspaceModel),
            "machine,model" => Ok(GroupBy::MachineModel),
            _ => Err(format!(
                "Invalid group-by value: '{}'. Valid options: model, client,model, client,provider,model, workspace,model, machine,model",
                s
            )),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TokenBreakdown {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    /// One-hour cache creation tokens, already included in `cache_write`.
    pub cache_write_1h: i64,
    pub reasoning: i64,
}

impl TokenBreakdown {
    pub fn checked_add(&self, other: &Self) -> Option<Self> {
        Some(Self {
            input: self.input.checked_add(other.input)?,
            output: self.output.checked_add(other.output)?,
            cache_read: self.cache_read.checked_add(other.cache_read)?,
            cache_write: self.cache_write.checked_add(other.cache_write)?,
            cache_write_1h: self.cache_write_1h.checked_add(other.cache_write_1h)?,
            reasoning: self.reasoning.checked_add(other.reasoning)?,
        })
    }

    pub fn checked_total(&self) -> Option<i64> {
        [
            self.input,
            self.output,
            self.cache_read,
            self.cache_write,
            self.reasoning,
        ]
        .into_iter()
        .try_fold(0_i64, i64::checked_add)
    }

    pub fn total(&self) -> i64 {
        self.checked_total().expect("token total exceeds i64::MAX")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SourceFingerprint([u8; 32]);

impl SourceFingerprint {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
