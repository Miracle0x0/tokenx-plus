use anyhow::Result;
use std::sync::Arc;

use crate::cli::{PendingPricing, StartupSnapshot};
use crate::generation_cache::{load_generation_cache, RetryBackoff};

use super::local_usage::{DetailSelections, InstalledGeneration};

pub(super) struct StartupLoad {
    pub(super) paths: crate::product_paths::ProductPaths,
    pub(super) acquisition: tokenx_engine::AcquisitionEngine,
    pub(super) cached: Option<Box<InstalledGeneration>>,
    pub(super) needs_refresh: bool,
    pub(super) retry_backoff: Option<RetryBackoff>,
    pub(super) warning: Option<String>,
}

pub(super) fn load_startup(
    startup: StartupSnapshot<PendingPricing>,
    date_range: tokenx_engine::DateRange,
    query: tokenx_engine::UsageQuery,
) -> Result<StartupLoad> {
    let startup = startup.bind_local_pricing();
    let acquisition = crate::acquisition::acquisition_engine_with_dsh_home(
        startup.paths.cache_dir(),
        startup.input.home,
        startup.input.universe,
        date_range,
        startup.settings.scanner,
        startup.calendar,
        startup.pricing,
        startup.input.dsh_home,
        startup.settings.model_mappings,
    )?;
    let pricing_diagnostics = acquisition.pricing_snapshot().diagnostics().to_vec();
    let (cached, mut needs_refresh, retry_backoff, mut warning) = super::decide_initial_data(
        load_generation_cache(&startup.paths.generation_cache_file(), acquisition.config())
            .with_pricing_diagnostics(pricing_diagnostics),
    );
    let cached = match cached {
        Some(generation) => {
            match InstalledGeneration::new(Arc::new(generation), query, DetailSelections::default())
            {
                Ok(prepared) => Some(Box::new(prepared)),
                Err(error) => {
                    warning = Some(
                        rust_i18n::t!("tui.core.cache.rejected", error = format!("{error:#}"))
                            .into_owned(),
                    );
                    needs_refresh = true;
                    None
                }
            }
        }
        None => None,
    };
    Ok(StartupLoad {
        paths: startup.paths,
        acquisition,
        cached,
        needs_refresh,
        retry_backoff,
        warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenx_engine::{ClientId, ClientUniverse, DateRange, GroupBy, UsageQuery};

    fn snapshot(root: &std::path::Path) -> StartupSnapshot<PendingPricing> {
        StartupSnapshot {
            paths: crate::product_paths::ProductPaths::at(root.join("config")),
            input: crate::cli::ResolvedInputScope {
                home: root.to_path_buf(),
                dsh_home: None,
                universe: ClientUniverse::new([ClientId::Amp]).unwrap(),
                restricted: true,
            },
            settings: Default::default(),
            calendar: tokenx_engine::CalendarContext::explicit("UTC").unwrap(),
            pricing: PendingPricing,
        }
    }

    fn query(client: ClientId) -> UsageQuery {
        UsageQuery::full(
            &ClientUniverse::new([client]).unwrap(),
            GroupBy::default(),
            chrono::NaiveDate::from_ymd_opt(2026, 7, 26).unwrap(),
        )
    }

    #[test]
    fn fresh_cache_prepares_all_local_views_without_requesting_acquisition() {
        let root = tempfile::TempDir::new().unwrap();
        let initial = load_startup(
            snapshot(root.path()),
            DateRange::none(),
            query(ClientId::Amp),
        )
        .unwrap();
        assert!(initial.cached.is_none());
        assert!(initial.needs_refresh);
        let generation = initial.acquisition.acquire().unwrap();
        crate::generation_cache::save_generation_cache(
            &initial.paths.generation_cache_file(),
            &generation,
        )
        .unwrap();

        let loaded = load_startup(
            snapshot(root.path()),
            DateRange::none(),
            query(ClientId::Amp),
        )
        .unwrap();
        assert!(!loaded.needs_refresh);
        assert!(loaded.warning.is_none());
        let prepared = loaded.cached.unwrap();
        assert_eq!(
            prepared.generation().source_fingerprint(),
            generation.source_fingerprint()
        );
        assert_eq!(prepared.view().total_tokens, 0);
        assert!(prepared.sessions().sessions().is_empty());
    }

    #[test]
    fn cached_projection_failure_requests_acquisition_and_exposes_warning() {
        let root = tempfile::TempDir::new().unwrap();
        let initial = load_startup(
            snapshot(root.path()),
            DateRange::none(),
            query(ClientId::Amp),
        )
        .unwrap();
        let generation = initial.acquisition.acquire().unwrap();
        crate::generation_cache::save_generation_cache(
            &initial.paths.generation_cache_file(),
            &generation,
        )
        .unwrap();

        let loaded = load_startup(
            snapshot(root.path()),
            DateRange::none(),
            query(ClientId::Codex),
        )
        .unwrap();
        assert!(loaded.cached.is_none());
        assert!(loaded.needs_refresh);
        assert!(loaded
            .warning
            .unwrap()
            .contains("cached generation rejected"));
    }
}
