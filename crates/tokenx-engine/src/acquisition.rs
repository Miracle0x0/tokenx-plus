//! Discovery, prepared input inventory, and the ordered single-copy fold.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use sha2::{Digest, Sha256};

use crate::{
    aggregate, input_health, input_record_cache, integrations, machine_labels, pricing, scanner,
    AcquisitionCancellation, AcquisitionError, AcquisitionPhase, AttributedUsageRecord,
    CalendarContext, ClientRootOverrides, ClientUniverse, DataHealth, DateRange, InputFailure,
    InputFootprint, InputHealth, InputStatus, ModelMappings, RejectionSummary, SourceFingerprint,
};

#[cfg(test)]
pub(crate) mod test_support;

/// A one-shot inventory of discovered local inputs and their pre-parse
/// metadata snapshots. It is intentionally non-`Clone`: execution consumes
/// the exact units whose signature was compared by the caller.
pub(crate) struct PreparedInventory {
    pub(crate) date_range: DateRange,
    pub(crate) groups: Vec<integrations::PreparedIntegrationInputs>,
    pub(crate) signature: SourceFingerprint,
    pub(crate) input_footprint: InputFootprint,
    pub(crate) health: DataHealth,
    pub(crate) input_cache_dir: PathBuf,
}

impl PreparedInventory {
    pub(crate) fn source_fingerprint(&self) -> SourceFingerprint {
        self.signature
    }
}

fn selected_client_footprint(clients: &ClientUniverse) -> InputFootprint {
    InputFootprint::for_clients(clients.iter())
}

pub(crate) fn inventory_input_footprint(
    clients: &ClientUniverse,
    groups: &[integrations::PreparedIntegrationInputs],
) -> InputFootprint {
    let mut footprint = selected_client_footprint(clients);
    for group in groups {
        let client = group.binding.client;
        let mut seen = HashSet::new();
        for unit in &group.units {
            unit.snapshot().visit_present_files(|identity, size| {
                if seen.insert(identity) {
                    footprint
                        .add_bytes(client, size)
                        .expect("input data size must fit in u64");
                }
            });
        }
    }
    footprint
}

pub(crate) struct FoldOutcome {
    pub(crate) source_fingerprint: SourceFingerprint,
    pub(crate) input_footprint: InputFootprint,
    pub(crate) health: DataHealth,
}

fn fold_prepared_local_inputs_with_pricing_with_cancellation(
    prepared: PreparedInventory,
    pricing: Option<&pricing::PricingService>,
    calendar: CalendarContext,
    sink: &mut dyn integrations::AttributedUsageSink,
    model_mappings: &ModelMappings,
    cancellation: &AcquisitionCancellation,
) -> Result<FoldOutcome, AcquisitionError> {
    cancellation
        .check(AcquisitionPhase::Planning)
        .map_err(AcquisitionError::cancelled)?;
    let PreparedInventory {
        groups,
        signature: source_fingerprint,
        input_footprint,
        mut health,
        input_cache_dir,
        ..
    } = prepared;
    let mut input_cache = match input_record_cache::InputRecordShardStore::open(&input_cache_dir) {
        Ok(cache) => cache,
        Err(error) => input_record_cache::InputRecordShardStore::without_initialization(
            &input_cache_dir,
            &error,
        ),
    };

    let parse_result = integrations::run_prepared_integrations(
        groups,
        &mut input_cache,
        pricing,
        calendar,
        sink,
        model_mappings,
        cancellation,
    )
    .map(|parsed_health| health.merge(parsed_health));

    cancellation
        .check(AcquisitionPhase::CacheFinalization)
        .map_err(AcquisitionError::cancelled)?;
    let cache_result = input_cache.save_if_dirty();
    let result = match (parse_result, cache_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(parse_error), Ok(())) => Err(parse_error),
        (Ok(()), Err(_)) => Ok(()),
        (Err(parse_error), Err(_)) => Err(parse_error),
    };
    if let Some((kind, failure)) = input_cache.disabled_diagnostic() {
        health.record_global_diagnostic(input_cache_dir, kind, failure);
    }
    result
        .map(|()| FoldOutcome {
            source_fingerprint,
            input_footprint,
            health,
        })
        .map_err(|error| {
            if error.is_cancelled() {
                AcquisitionError::cancelled(error)
            } else {
                AcquisitionError::operational(error)
            }
        })
}

struct AccumulationSink<'a>(&'a mut crate::aggregate::GenerationAccumulator);

impl integrations::AttributedUsageSink for AccumulationSink<'_> {
    fn push_record(
        &mut self,
        message: AttributedUsageRecord,
    ) -> integrations::AttributedUsageSinkOutcome {
        match self.0.push(&message) {
            aggregate::RecordAggregationOutcome::Retained
            | aggregate::RecordAggregationOutcome::Filtered => {
                integrations::AttributedUsageSinkOutcome::Retained
            }
            aggregate::RecordAggregationOutcome::Rejected(_) => {
                integrations::AttributedUsageSinkOutcome::Rejected(
                    input_health::RecordRejectionReason::AggregationOverflow,
                )
            }
            aggregate::RecordAggregationOutcome::Failed => {
                integrations::AttributedUsageSinkOutcome::Failed
            }
        }
    }
}

pub(crate) fn stream_local_inputs_into_accumulator_with_cancellation(
    prepared: PreparedInventory,
    pricing: Option<&pricing::PricingService>,
    accumulator: &mut crate::aggregate::GenerationAccumulator,
    calendar: CalendarContext,
    model_mappings: &ModelMappings,
    cancellation: &AcquisitionCancellation,
) -> Result<FoldOutcome, AcquisitionError> {
    let mut sink = AccumulationSink(accumulator);
    fold_prepared_local_inputs_with_pricing_with_cancellation(
        prepared,
        pricing,
        calendar,
        &mut sink,
        model_mappings,
        cancellation,
    )
}

pub(crate) fn prepare_inventory(
    home_dir: &Path,
    clients: ClientUniverse,
    date_range: DateRange,
    scanner_settings: &scanner::ScannerSettings,
    client_roots: &ClientRootOverrides,
    input_cache_dir: PathBuf,
    cancellation: &AcquisitionCancellation,
) -> Result<PreparedInventory, AcquisitionError> {
    cancellation
        .check(AcquisitionPhase::Discovery)
        .map_err(AcquisitionError::cancelled)?;
    scanner_settings
        .validate()
        .map_err(AcquisitionError::invalid_environment)?;
    let machine_labels = Arc::new(
        machine_labels::MachineLabels::resolve(scanner_settings)
            .map_err(AcquisitionError::invalid_environment)?,
    );
    let selected_integrations = integrations::selected_integrations(&clients);
    let prepared = prepare_selected_integrations(selected_integrations, |binding| {
        prepare_integration(
            binding,
            home_dir,
            scanner_settings,
            client_roots.for_client(binding.client),
            cancellation,
        )
    })?;
    let mut health = DataHealth::default();
    let mut groups = Vec::with_capacity(prepared.len());
    for mut outcome in prepared {
        outcome.group.machine_labels = Arc::clone(&machine_labels);
        groups.push(outcome.group);
        health.merge(outcome.health);
    }
    cancellation
        .check(AcquisitionPhase::Discovery)
        .map_err(AcquisitionError::cancelled)?;
    let signature = source_fingerprint(&clients, &groups);
    let input_footprint = inventory_input_footprint(&clients, &groups);
    Ok(PreparedInventory {
        date_range,
        groups,
        signature,
        input_footprint,
        health,
        input_cache_dir,
    })
}

pub(crate) struct PreparedIntegrationOutcome {
    pub(crate) group: integrations::PreparedIntegrationInputs,
    pub(crate) health: DataHealth,
}

pub(crate) fn prepare_selected_integrations<Prepare>(
    selected: Vec<integrations::IntegrationBinding>,
    prepare: Prepare,
) -> Result<Vec<PreparedIntegrationOutcome>, AcquisitionError>
where
    Prepare: Fn(integrations::IntegrationBinding) -> Result<PreparedIntegrationOutcome, AcquisitionError>
        + Sync
        + Send,
{
    selected.into_par_iter().map(prepare).collect()
}

fn prepare_integration(
    binding: integrations::IntegrationBinding,
    home_dir: &Path,
    scanner_settings: &scanner::ScannerSettings,
    root_override: Option<&Path>,
    cancellation: &AcquisitionCancellation,
) -> Result<PreparedIntegrationOutcome, AcquisitionError> {
    cancellation
        .check(AcquisitionPhase::Discovery)
        .map_err(AcquisitionError::cancelled)?;
    let scan_ctx = integrations::DiscoveryContext {
        client: binding.client,
        home_dir,
        root_override,
        scanner_settings,
        cancellation: cancellation.clone(),
    };
    let mut health = DataHealth::default();
    // Third-party input and snapshot failures stay inside their input's
    // failure domain.
    let discovered = match binding.driver.discover_inputs(&scan_ctx) {
        Ok(units) => units,
        Err(error) => {
            if error.is_cancelled() {
                return Err(AcquisitionError::cancelled(error));
            }
            health.record(InputHealth {
                client: binding.client,
                path: error.path.clone(),
                status: InputStatus::Unavailable {
                    failure: InputFailure::new(error.operation, error.to_string()),
                },
                rejections: RejectionSummary::default(),
            });
            return Ok(PreparedIntegrationOutcome {
                group: integrations::PreparedIntegrationInputs {
                    machine_labels: Arc::default(),
                    binding,
                    units: Vec::new(),
                },
                health,
            });
        }
    };
    cancellation
        .check(AcquisitionPhase::Discovery)
        .map_err(AcquisitionError::cancelled)?;
    let mut units = Vec::with_capacity(discovered.len());
    for unit in discovered {
        cancellation
            .check(AcquisitionPhase::Discovery)
            .map_err(AcquisitionError::cancelled)?;
        let client = binding.client;
        let path = unit.path.clone();
        match unit.prepare_snapshot() {
            Ok(unit) => units.push(unit),
            Err(source) => {
                health.record(InputHealth {
                    client,
                    path,
                    status: InputStatus::Unavailable {
                        failure: InputFailure::new(
                            "snapshot input metadata and identity",
                            source.to_string(),
                        ),
                    },
                    rejections: RejectionSummary::default(),
                });
            }
        }
    }
    Ok(PreparedIntegrationOutcome {
        group: integrations::PreparedIntegrationInputs {
            binding,
            units,
            machine_labels: Arc::default(),
        },
        health,
    })
}

const SOURCE_FINGERPRINT_VERSION: u32 = 2;

pub(crate) fn source_fingerprint(
    clients: &ClientUniverse,
    groups: &[integrations::PreparedIntegrationInputs],
) -> SourceFingerprint {
    let mut hasher = Sha256::new();
    input_record_cache::hash_inventory_bytes(&mut hasher, b"tokenx/local-input-inventory");
    hasher.update(SOURCE_FINGERPRINT_VERSION.to_le_bytes());
    input_record_cache::hash_inventory_len(&mut hasher, clients.iter().len());
    for client in clients.iter() {
        input_record_cache::hash_inventory_bytes(&mut hasher, client.as_str().as_bytes());
    }
    input_record_cache::hash_inventory_len(&mut hasher, groups.len());
    for group in groups {
        input_record_cache::hash_inventory_bytes(
            &mut hasher,
            group.binding.client.as_str().as_bytes(),
        );
        input_record_cache::hash_inventory_len(&mut hasher, group.units.len());
        for unit in &group.units {
            hasher.update(unit.inventory_signature_digest());
            input_record_cache::hash_inventory_bytes(
                &mut hasher,
                group.machine_labels.label(&unit.path).as_bytes(),
            );
        }
    }
    SourceFingerprint(hasher.finalize().into())
}
