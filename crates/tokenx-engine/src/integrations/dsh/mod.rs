pub(crate) mod decode;

use std::collections::HashSet;
use std::path::Path;

use rayon::prelude::*;

use crate::input_record_cache::DecoderId;
use crate::integrations::cache as pipeline_cache;
use crate::integrations::discover as source_discovery;
use crate::integrations::{
    BoundUsageSink, DecoderKind, DiscoveredInput, DiscoveryContext, FingerprintPolicy, FoldContext,
    InputDiscoveryError, IntegrationDriver, ParseContext, ParsedBatchInput, ParsedUnit, SourceSpec,
};

fn dsh_session_file(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("session.jsonl" | "session.jsonl.zstd")
    )
}

const SOURCE: SourceSpec = SourceSpec::home(
    ".dsh/sessions",
    crate::integrations::SourceMatcher::new(dsh_session_file),
);

pub(crate) struct Driver;

impl IntegrationDriver for Driver {
    fn discover_inputs(
        &self,
        ctx: &DiscoveryContext<'_>,
    ) -> Result<Vec<DiscoveredInput>, InputDiscoveryError> {
        let default_root = ctx
            .dsh_home
            .map(|root| root.join("sessions"))
            .unwrap_or_else(|| SOURCE.resolve(ctx.home_dir));
        let mut paths = source_discovery::scan_roots(ctx, [default_root], SOURCE.matcher())?;
        paths.extend(source_discovery::scan_roots(
            ctx,
            source_discovery::extra_roots_for_client(ctx.client, ctx)?,
            SOURCE.matcher(),
        )?);
        source_discovery::input_units_from_paths(
            ctx.client,
            paths,
            FingerprintPolicy::PlainFile,
            DecoderKind::plain(DecoderId::Dsh),
        )
    }

    fn parse_inputs(
        &self,
        units: Vec<crate::integrations::ExecutionInput>,
        ctx: &ParseContext<'_>,
    ) -> Vec<ParsedUnit> {
        units
            .into_par_iter()
            .map(|unit| pipeline_cache::load_or_scan_unit_with(unit, ctx, decode::parse_dsh_file))
            .collect()
    }

    fn plan_cache_hit(
        &self,
        unit: crate::integrations::PreparedInput,
        input_cache: &crate::input_record_cache::InputRecordShardStore,
    ) -> Result<crate::integrations::CacheHitPlan, crate::integrations::InputPlanningError> {
        pipeline_cache::plan_cache_hit(unit, input_cache)
    }

    fn fold(
        &self,
        parsed: Vec<ParsedUnit>,
        ctx: &mut FoldContext<'_>,
        sink: &mut BoundUsageSink<'_>,
    ) -> Result<(), crate::integrations::InputPipelineError> {
        let mut seen = HashSet::new();
        fold_dsh_units(parsed, ctx, sink, &mut seen)
    }

    fn fold_batches(
        &self,
        batches: &mut ParsedBatchInput,
        ctx: &mut FoldContext<'_>,
        sink: &mut BoundUsageSink<'_>,
    ) -> Result<(), crate::integrations::InputPipelineError> {
        let mut seen = HashSet::new();
        while let Some(parsed) = batches.next(ctx)? {
            fold_dsh_units(parsed, ctx, sink, &mut seen)?;
        }
        Ok(())
    }
}

fn fold_dsh_units(
    parsed: Vec<ParsedUnit>,
    ctx: &mut FoldContext<'_>,
    sink: &mut BoundUsageSink<'_>,
    seen: &mut HashSet<u64>,
) -> Result<(), crate::integrations::InputPipelineError> {
    pipeline_cache::fold_units_with_filter(parsed, ctx, sink, |_, messages| {
        messages
            .into_iter()
            .filter(|message| crate::should_keep_deduped_message(seen, message))
            .collect()
    })
}

pub(crate) static DRIVER: Driver = Driver;

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::clients::ClientId;
    use crate::input_record_cache::{self, DecoderVersion};

    fn write_file(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn scan_context<'a>(
        home_dir: &'a Path,
        settings: &'a crate::scanner::ScannerSettings,
    ) -> DiscoveryContext<'a> {
        scan_context_with_dsh_home(home_dir, settings, None)
    }

    fn scan_context_with_dsh_home<'a>(
        home_dir: &'a Path,
        settings: &'a crate::scanner::ScannerSettings,
        dsh_home: Option<&'a Path>,
    ) -> DiscoveryContext<'a> {
        DiscoveryContext {
            client: ClientId::Dsh,
            home_dir,
            dsh_home,
            scanner_settings: settings,
            cancellation: crate::engine::AcquisitionCancellation::default(),
        }
    }

    #[test]
    fn matcher_accepts_only_dsh_session_file_names() {
        assert!(dsh_session_file(Path::new("session.jsonl")));
        assert!(dsh_session_file(Path::new("session.jsonl.zstd")));
        assert!(!dsh_session_file(Path::new("other.jsonl")));
        assert!(!dsh_session_file(Path::new("session.jsonl.zst")));
        assert!(!dsh_session_file(Path::new("old-session.jsonl.zstd")));
    }

    #[test]
    fn driver_discovers_default_and_extra_session_transcripts() {
        let home = tempfile::TempDir::new().unwrap();
        let default_path = home
            .path()
            .join(".dsh/sessions/workspace-a/session-a/session.jsonl.zstd");
        let extra_root = home.path().join("dsh-import");
        let extra_path = extra_root.join("workspace-b/session-b/session.jsonl");
        let unrelated_path = extra_root.join("workspace-b/session-b/other.jsonl");
        for path in [&default_path, &extra_path, &unrelated_path] {
            write_file(path, "");
        }

        let settings = crate::scanner::ScannerSettings {
            extra_scan_paths: [(ClientId::Dsh, vec![extra_root])].into_iter().collect(),
            ..Default::default()
        };

        let units = DRIVER
            .discover_inputs(&scan_context(home.path(), &settings))
            .unwrap();
        let mut expected = vec![default_path, extra_path];
        expected.sort_unstable();

        assert_eq!(
            units
                .iter()
                .map(|unit| unit.path.clone())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(units.iter().all(|unit| {
            unit.fingerprint_policy == FingerprintPolicy::PlainFile
                && unit.decoder.version() == DecoderVersion::current(DecoderId::Dsh)
        }));
    }

    #[test]
    fn driver_uses_the_captured_dsh_home_for_default_sessions() {
        let home = tempfile::TempDir::new().unwrap();
        let custom = tempfile::TempDir::new().unwrap();
        let custom_path = custom
            .path()
            .join("sessions/workspace/session/session.jsonl");
        let ignored_path = home
            .path()
            .join(".dsh/sessions/workspace/ignored/session.jsonl");
        write_file(&custom_path, "");
        write_file(&ignored_path, "");

        let settings = crate::scanner::ScannerSettings::default();
        let ctx = scan_context_with_dsh_home(home.path(), &settings, Some(custom.path()));
        let units = DRIVER.discover_inputs(&ctx).unwrap();

        assert_eq!(units.len(), 1);
        assert_eq!(units[0].path, custom_path);
    }

    fn duplicate_session_paths(root: &Path) -> Vec<PathBuf> {
        let row = r#"{"type":"assistant/message","time":1785730448979,"data":{"turn":1,"message":{"id":"shared-message","source":{"provider":"deepseek","model":"deepseek-reasoner"}},"usage":{"inputTokens":10,"outputTokens":5}}}"#;
        ["session-a", "session-b"]
            .into_iter()
            .map(|session_id| {
                let path = root.join(session_id).join("session.jsonl");
                write_file(
                    &path,
                    &format!(
                        "{{\"type\":\"session\",\"id\":\"{session_id}\",\"cwd\":\"/work\"}}\n{row}\n"
                    ),
                );
                path
            })
            .collect()
    }

    fn fold_batched(
        paths: &[PathBuf],
        cache: &mut input_record_cache::InputRecordShardStore,
    ) -> Vec<crate::AttributedUsageRecord> {
        let units = paths
            .iter()
            .cloned()
            .map(|path| {
                DiscoveredInput::plain_file(path, DecoderKind::plain(DecoderId::Dsh))
                    .prepare_snapshot()
                    .unwrap()
            })
            .collect();
        let binding = crate::integrations::integration_for(ClientId::Dsh);
        let mut batches = ParsedBatchInput::new(binding, units);
        let mut messages = Vec::new();
        let mut ctx = FoldContext::new(binding, cache, None);
        let mut sink = BoundUsageSink::new(binding, &mut messages);
        DRIVER
            .fold_batches(&mut batches, &mut ctx, &mut sink)
            .unwrap();
        messages
    }

    #[test]
    fn batched_fold_deduplicates_across_batches_with_cold_and_warm_cache_parity() {
        let dir = tempfile::TempDir::new().unwrap();
        let paths = duplicate_session_paths(dir.path());

        let (cold, warm) = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(|| {
                let mut cache = input_record_cache::InputRecordShardStore::default();
                let cold = fold_batched(&paths, &mut cache);
                let warm = fold_batched(&paths, &mut cache);
                (cold, warm)
            });

        assert_eq!(cold.len(), 1);
        assert_eq!(warm, cold);
        assert_eq!(cold[0].client, ClientId::Dsh);
    }
}
