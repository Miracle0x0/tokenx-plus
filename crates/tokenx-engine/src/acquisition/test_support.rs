#![cfg(test)]

use std::path::{Path, PathBuf};

use super::{
    fold_prepared_local_inputs_with_pricing_with_cancellation, prepare_inventory,
    stream_local_inputs_into_accumulator_with_cancellation, FoldOutcome, PreparedInventory,
};
use crate::{
    input_health, integrations, pricing, projection, records, scanner, AcquisitionCancellation,
    AcquisitionError, AttributedUsageRecord, CalendarContext, ClientId, ClientRootOverrides,
    ClientUniverse, DataHealth, DateRange, GroupBy, ModelMappings,
};

#[derive(Debug, Clone, Default)]
pub(crate) struct TestAcquisitionRequest {
    pub(crate) home_dir: Option<PathBuf>,
    pub(crate) clients: Option<Vec<ClientId>>,
    pub(crate) date_range: DateRange,
    pub(crate) scanner_settings: scanner::ScannerSettings,
}

pub(crate) fn parse_all_messages_with_pricing(
    home_dir: &str,
    clients: &[String],
    pricing: Option<&pricing::PricingService>,
) -> Result<Vec<AttributedUsageRecord>, AcquisitionError> {
    parse_all_messages_with_pricing_with_settings(
        home_dir,
        clients,
        pricing,
        &scanner::ScannerSettings::default(),
    )
}

pub(crate) fn parse_all_messages_with_pricing_with_settings(
    home_dir: &str,
    clients: &[String],
    pricing: Option<&pricing::PricingService>,
    scanner_settings: &scanner::ScannerSettings,
) -> Result<Vec<AttributedUsageRecord>, AcquisitionError> {
    parse_all_messages_with_health_with_settings(home_dir, clients, pricing, scanner_settings)
        .map(|(messages, _)| messages)
}

pub(crate) fn parse_all_messages_with_health(
    home_dir: &str,
    clients: &[String],
    pricing: Option<&pricing::PricingService>,
) -> Result<(Vec<AttributedUsageRecord>, DataHealth), AcquisitionError> {
    parse_all_messages_with_health_with_settings(
        home_dir,
        clients,
        pricing,
        &scanner::ScannerSettings::default(),
    )
}

pub(crate) fn parse_all_messages_with_health_with_settings(
    home_dir: &str,
    clients: &[String],
    pricing: Option<&pricing::PricingService>,
    scanner_settings: &scanner::ScannerSettings,
) -> Result<(Vec<AttributedUsageRecord>, DataHealth), AcquisitionError> {
    let clients = clients
        .iter()
        .map(|client| {
            ClientId::from_str(client)
                .unwrap_or_else(|| panic!("test requested unknown local client `{client}`"))
        })
        .collect::<Vec<_>>();
    let prepared = prepare_test_inventory(TestAcquisitionRequest {
        home_dir: Some(PathBuf::from(home_dir)),
        clients: Some(clients),
        scanner_settings: scanner_settings.clone(),
        ..TestAcquisitionRequest::default()
    })?;
    let mut all_messages: Vec<AttributedUsageRecord> = Vec::new();
    let outcome = fold_prepared_local_inputs_with_pricing(prepared, pricing, &mut all_messages)?;
    Ok((all_messages, outcome.health))
}

pub(crate) fn input_cache_dir_for_test_home(home_dir: &Path) -> PathBuf {
    home_dir.join(".tokenx-test-cache/input")
}

pub(crate) fn fold_prepared_local_inputs_with_pricing(
    prepared: PreparedInventory,
    pricing: Option<&pricing::PricingService>,
    sink: &mut dyn integrations::AttributedUsageSink,
) -> Result<FoldOutcome, AcquisitionError> {
    fold_prepared_local_inputs_with_pricing_with_cancellation(
        prepared,
        pricing,
        CalendarContext::explicit("UTC").expect("UTC is a valid IANA timezone"),
        sink,
        &ModelMappings::default(),
        &AcquisitionCancellation::default(),
    )
}

pub(crate) fn stream_local_inputs_into_accumulator(
    prepared: PreparedInventory,
    pricing: Option<&pricing::PricingService>,
    accumulator: &mut crate::aggregate::GenerationAccumulator,
) -> Result<FoldOutcome, AcquisitionError> {
    stream_local_inputs_into_accumulator_with_cancellation(
        prepared,
        pricing,
        accumulator,
        CalendarContext::explicit("UTC").expect("UTC is a valid IANA timezone"),
        &ModelMappings::default(),
        &AcquisitionCancellation::default(),
    )
}

pub(crate) fn prepare_test_inventory(
    options: TestAcquisitionRequest,
) -> Result<PreparedInventory, AcquisitionError> {
    let home_dir = match options.home_dir {
        Some(home_dir) => home_dir,
        None => dirs::home_dir().ok_or_else(|| {
            AcquisitionError::invalid_environment_message(
                "HOME directory not specified and could not determine home directory",
            )
        })?,
    };
    let clients = match options.clients {
        Some(clients) => ClientUniverse::new(clients)
            .expect("an explicit test client universe must not be empty"),
        None => ClientUniverse::all(),
    };
    prepare_inventory(
        &home_dir,
        clients,
        options.date_range,
        &options.scanner_settings,
        &ClientRootOverrides::default(),
        input_cache_dir_for_test_home(&home_dir),
        &AcquisitionCancellation::default(),
    )
}

pub(crate) fn filter_usage_records(
    messages: Vec<AttributedUsageRecord>,
    options: &TestAcquisitionRequest,
) -> Vec<AttributedUsageRecord> {
    let mut filtered = messages;
    if !options.date_range.is_unfiltered() {
        let calendar = CalendarContext::explicit("UTC").expect("UTC is a valid IANA timezone");
        filtered.retain(|message| {
            calendar
                .local_date_and_hour(message.timestamp)
                .map(|(date, _)| date)
                .is_some_and(|date| options.date_range.contains(date))
        });
    }
    filtered
}

pub(crate) fn load_test_usage(
    options: TestAcquisitionRequest,
    group_by: GroupBy,
    pricing: Option<&pricing::PricingService>,
) -> Result<projection::UsageProjection, AcquisitionError> {
    let prepared = prepare_test_inventory(options)?;
    load_prepared_test_usage(prepared, group_by, pricing)
}

pub(crate) fn load_prepared_test_usage(
    prepared: PreparedInventory,
    group_by: GroupBy,
    pricing: Option<&pricing::PricingService>,
) -> Result<projection::UsageProjection, AcquisitionError> {
    load_prepared_test_usage_with_health(prepared, group_by, pricing).map(|(data, _)| data)
}

pub(crate) fn load_test_usage_with_health(
    options: TestAcquisitionRequest,
    group_by: GroupBy,
    pricing: Option<&pricing::PricingService>,
) -> Result<(projection::UsageProjection, input_health::HealthSummary), AcquisitionError> {
    let prepared = prepare_test_inventory(options)?;
    load_prepared_test_usage_with_health(prepared, group_by, pricing)
}

pub(crate) fn load_prepared_test_usage_with_health(
    prepared: PreparedInventory,
    group_by: GroupBy,
    pricing: Option<&pricing::PricingService>,
) -> Result<(projection::UsageProjection, input_health::HealthSummary), AcquisitionError> {
    let date_range = prepared.date_range.clone();
    let mut accumulator = crate::aggregate::GenerationAccumulator::new(
        date_range,
        CalendarContext::explicit("UTC").expect("UTC is a valid IANA timezone"),
    );
    let health = match stream_local_inputs_into_accumulator(prepared, pricing, &mut accumulator) {
        Ok(outcome) => outcome.health,
        Err(error) => {
            drop(accumulator);
            records::intern::prune_dead();
            return Err(error);
        }
    };
    let effective_date =
        chrono::NaiveDate::from_ymd_opt(2026, 7, 26).expect("test projection date is valid");
    let usage_index = accumulator
        .into_usage_index()
        .map_err(AcquisitionError::operational)?;
    records::intern::prune_dead();
    let data = usage_index
        .project_usage(&group_by, effective_date)
        .map_err(AcquisitionError::operational)?;
    Ok((data, health.summarize()))
}
